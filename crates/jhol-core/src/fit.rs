//! Sizing a model into a memory budget — one implementation of the arithmetic.
//!
//! weights come from the tensor descriptors, the KV cache from `block_count`,
//! `attention.head_count_kv` and `head_dim`, and the overhead is a stated share of weights.
//! The CLI, the web panel, the TUI and the MCP tool all call this, so "6.5 GB" cannot mean two
//! different things in two places.

use jhol_ai::gguf::{KvPrecision, ModelMeta};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Fit {
    pub weights: u64,
    pub kv: u64,
    pub overhead: u64,
    pub total: u64,
    pub ctx: u64,
    pub batch: u64,
    pub precision: KvPrecision,
    pub vram: Option<u64>,
    pub fits: Option<bool>,
    pub headroom: Option<i64>,
    pub max_context: Option<u64>,
    pub meta: ModelMeta,
    pub missing_meta: Vec<String>,
}

pub const OVERHEAD_PERCENT: u64 = 15;

/// `f16` | `q8` | `q4`; anything else is not a cache precision we can size.
pub fn parse_precision(s: &str) -> Option<KvPrecision> {
    KvPrecision::parse(s)
}

pub fn overhead(weights: u64) -> u64 {
    weights * OVERHEAD_PERCENT / 100
}

/// The arithmetic, with no I/O. `missing_meta` is carried through so a caller can explain an
/// unknown KV cache instead of showing zero.
pub fn compute(
    meta: &ModelMeta,
    weights: u64,
    ctx: u64,
    batch: u64,
    precision: KvPrecision,
    vram: Option<u64>,
    missing_meta: Vec<String>,
) -> Fit {
    let kv = meta.kv_bytes_at(ctx, batch, precision).unwrap_or(0);
    let ov = overhead(weights);
    let total = weights + kv + ov;
    Fit {
        weights,
        kv,
        overhead: ov,
        total,
        ctx,
        batch,
        precision,
        vram,
        fits: vram.map(|v| total <= v),
        headroom: vram.map(|v| v as i64 - total as i64),
        max_context: vram.and_then(|v| meta.max_context(v, weights, precision)),
        meta: meta.clone(),
        missing_meta,
    }
}

/// Fit a model that is already on disk. No network: an agent or a script can call this on any
/// machine that holds the file.
pub fn fit_local(
    path: &Path,
    ctx: u64,
    batch: u64,
    precision: KvPrecision,
    vram: Option<u64>,
) -> Result<Fit, String> {
    let is_gguf = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("gguf"))
        .unwrap_or(false);
    if is_gguf {
        let m = jhol_ai::gguf::summarise(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(compute(
            &m.meta,
            m.data_bytes.unwrap_or(0),
            ctx,
            batch,
            precision,
            vram,
            m.missing_meta,
        ));
    }
    let mut shards: Vec<std::path::PathBuf> = if path.is_dir() {
        std::fs::read_dir(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "safetensors").unwrap_or(false))
            .collect()
    } else {
        vec![path.to_path_buf()]
    };
    if shards.is_empty() {
        return Err(format!("{}: no .gguf or .safetensors file", path.display()));
    }
    shards.sort();
    let mut weights = 0u64;
    for shard in &shards {
        let s = jhol_ai::safetensors::summarise(shard)
            .map_err(|e| format!("{}: {e}", shard.display()))?;
        weights += s.data_bytes;
    }
    // safetensors carries no loader metadata, so the KV cache is unknown here — which the
    // caller must show as unknown, not as zero.
    let meta = ModelMeta::default();
    Ok(compute(
        &meta,
        weights,
        ctx,
        batch,
        precision,
        vram,
        vec![
            "block_count".into(),
            "attention.head_count".into(),
            "head_dim".into(),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(layers: u64, heads: u64, kv_heads: u64, dim: u64, native: u64) -> ModelMeta {
        ModelMeta {
            layers: Some(layers),
            heads: Some(heads),
            kv_heads: Some(kv_heads),
            head_dim: Some(dim),
            context: Some(native),
            vocab: Some(151936),
            rope_theta: None,
        }
    }

    /// 36 layers, 8 kv heads, 128 dim, 2 B per element, 8192 tokens:
    /// 2 × 36 × 8 × 128 × 2 × 8192 = 1,207,959,552 bytes = 1.12 GiB.
    #[test]
    fn the_kv_cache_matches_a_hand_computation() {
        let f = compute(&meta(36, 32, 8, 128, 40960), 5_021_827_072, 8192, 1, KvPrecision::F16, None, vec![]);
        assert_eq!(f.kv, 1_207_959_552);
        assert_eq!(f.overhead, 5_021_827_072 * 15 / 100);
        assert_eq!(f.total, f.weights + f.kv + f.overhead);
    }

    /// Quantised KV is the GGML block ratio, so q8 is 34/32 bytes per element and q4 is 18/32 —
    /// not a round half and quarter.
    #[test]
    fn quantised_kv_uses_the_block_ratios() {
        let m = meta(4, 8, 8, 64, 4096);
        let f16 = compute(&m, 0, 1024, 1, KvPrecision::F16, None, vec![]).kv;
        let q8 = compute(&m, 0, 1024, 1, KvPrecision::Q8, None, vec![]).kv;
        let q4 = compute(&m, 0, 1024, 1, KvPrecision::Q4, None, vec![]).kv;
        assert!((q8 as f64 / f16 as f64 - 34.0 / 64.0).abs() < 0.01);
        assert!((q4 as f64 / f16 as f64 - 18.0 / 64.0).abs() < 0.01);
    }

    /// The longest context is clamped by what the model was trained for: quoting 135k tokens
    /// for a 40k model would be a number that cannot happen.
    #[test]
    fn the_context_ceiling_is_the_models_own_limit() {
        let m = meta(36, 32, 8, 128, 40960);
        let f = compute(&m, 1_000_000_000, 8192, 1, KvPrecision::F16, Some(64 * 1_073_741_824), vec![]);
        assert_eq!(f.max_context, Some(40960));
    }

    #[test]
    fn an_unknown_header_does_not_invent_a_cache_size() {
        let m = ModelMeta::default();
        let f = compute(&m, 100, 4096, 1, KvPrecision::F16, Some(1_000), vec!["block_count".into()]);
        assert_eq!(f.kv, 0);
        assert_eq!(f.max_context, None);
        assert_eq!(f.missing_meta, vec!["block_count".to_string()]);
    }
}
