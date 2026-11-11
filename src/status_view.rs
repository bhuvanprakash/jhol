//! The one place where model weight headers are merged into the status struct.
//!
//! `jhol-core` does not parse model files and must not: the AI crate owns that, and a
//! second reader would be a second answer. This module is the single composition point —
//! the CLI, the TUI and the web dashboard all call [`enrich`], so all three show the same
//! parameter counts and the same fit verdict.

use jhol_core::status::Status;
use std::path::{Path, PathBuf};

/// First `.safetensors` file in a directory, in sorted order so two runs agree.
fn first_safetensors(dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "safetensors").unwrap_or(false))
        .collect();
    files.sort();
    files.into_iter().next()
}

fn human_params(p: u64) -> String {
    let f = p as f64;
    if f >= 1e9 {
        format!("{:.2} B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1} M", f / 1e6)
    } else {
        format!("{p}")
    }
}

fn human_bytes(n: u64) -> String {
    let gb = 1024.0 * 1024.0 * 1024.0;
    let mb = 1024.0 * 1024.0;
    if n as f64 >= gb {
        format!("{:.1} GB", n as f64 / gb)
    } else {
        format!("{:.0} MB", n as f64 / mb)
    }
}

/// Fill model rows with what a header alone can tell us, and decide whether they fit.
///
/// A weight that cannot be measured is left `fits: None` with a stated reason — never a
/// pass. `None` means "we do not know", and the renderers print it that way.
pub fn enrich(mut status: Status, root: &Path) -> Status {
    let gpu_mb = status.axes.gpu_mem_mb;
    for artifact in status.artifacts.iter_mut() {
        if artifact.kind != "model" {
            continue;
        }
        let Some(dir) = artifact.path.as_deref() else {
            continue;
        };
        let dir = root.join(dir);
        let summary = match jhol_ai::safetensors::summarise_dir(&dir) {
            Ok(s) => s,
            Err(e) => {
                artifact.reasons.push(format!("header unreadable: {e}"));
                continue;
            }
        };
        artifact.params = Some(summary.params);
        artifact.vram_bytes = Some(summary.vram_bytes());
        artifact.dtypes = summary.dtypes.clone();
        artifact.top_tensors = summary.largest.clone();
        let missing: Vec<String> = summary
            .sidecars
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(n, _)| n.clone())
            .collect();
        artifact.reasons.insert(
            0,
            format!(
                "{} parameters{}",
                human_params(summary.params),
                if summary.shards.len() > 1 {
                    format!(" across {} shards", summary.shards.len())
                } else {
                    String::new()
                }
            ),
        );
        if !missing.is_empty() {
            artifact
                .reasons
                .push(format!("sidecars missing: {}", missing.join(", ")));
        }
        if !summary.unknown_dtypes.is_empty() {
            artifact.reasons.push(format!(
                "unknown dtype width: {}",
                summary.unknown_dtypes.join(", ")
            ));
        }

        let need = summary.vram_bytes();
        match gpu_mb {
            Some(mem) => {
                let need_mb = need.div_ceil(1024 * 1024);
                artifact.fits = Some(need_mb <= mem);
                artifact.reasons.push(format!(
                    "resident {} — host GPU {} MB, so it {}",
                    human_bytes(need),
                    mem,
                    if need_mb <= mem { "fits" } else { "does not fit" }
                ));
            }
            None => {
                artifact.fits = None;
                artifact.reasons.push(format!(
                    "resident {} — no GPU memory detected, so fit is unknown",
                    human_bytes(need)
                ));
            }
        }
    }
    status
}

/// `jhol inspect` and the dashboards resolve a model by name or by path with this.
pub fn resolve_weight_target(root: &Path, target: &str) -> Result<PathBuf, String> {
    let direct = Path::new(target);
    if direct.is_file() {
        return Ok(direct.to_path_buf());
    }
    if direct.is_dir() {
        // Return the directory, not its first shard: a sharded model summarised from one
        // shard under-reports parameters by the number of shards.
        return Ok(direct.to_path_buf());
    }
    let lock = jhol_core::model_lock::ModelLock::load(root)
        .ok_or_else(|| format!("{target}: no models.lock.json to resolve a name against"))?;
    let entry = lock
        .models
        .get(target)
        .ok_or_else(|| format!("{target}: not a path and not a pinned model"))?;
    let dir = root.join(&entry.dir);
    if !dir.is_dir() {
        return Err(format!("{target}: pinned, but {} is not on disk", dir.display()));
    }
    Ok(dir)
}


/// What `inspect` reports, however it was reached — one file, a sharded directory, or a
/// ranged fetch.
pub struct InspectReport {
    pub display: String,
    pub tensors: usize,
    pub params: u64,
    pub data_bytes: u64,
    pub vram_bytes: u64,
    pub header_bytes: u64,
    pub dtypes: std::collections::BTreeMap<String, usize>,
    pub unknown_dtypes: Vec<String>,
    pub largest: Vec<(String, u64)>,
    /// Shard file name -> its data bytes. One entry for an unsharded model.
    pub shards: Vec<(String, u64)>,
    /// Which loader sidecars are present.
    pub sidecars: Vec<(String, bool)>,
    pub bytes_read: u64,
    pub requests: u32,
}

impl InspectReport {
    fn new(display: String, bytes_read: u64, requests: u32) -> Self {
        InspectReport {
            display,
            tensors: 0,
            params: 0,
            data_bytes: 0,
            vram_bytes: 0,
            header_bytes: 0,
            dtypes: Default::default(),
            unknown_dtypes: Vec::new(),
            largest: Vec::new(),
            shards: Vec::new(),
            sidecars: Vec::new(),
            bytes_read,
            requests,
        }
    }
}

/// `hf://org/repo/file[@rev]` and bare URLs become an HTTP URL; anything else is a path.
fn remote_url(target: &str) -> Option<String> {
    if let Some(rest) = target.strip_prefix("hf://") {
        let (path, rev) = match rest.split_once('@') {
            Some((p, r)) => (p, r),
            None => (rest, "main"),
        };
        let mut parts = path.splitn(3, '/');
        let org = parts.next()?;
        let repo = parts.next()?;
        let file = parts.next()?;
        return Some(format!(
            "https://huggingface.co/{org}/{repo}/resolve/{rev}/{file}"
        ));
    }
    if target.starts_with("https://") || target.starts_with("http://") {
        return Some(target.to_string());
    }
    None
}

/// First chunk asked for. Bigger than any real header (tens of KB) so the common case is
/// one request; the rare larger header gets a second range for the remainder.
const HEADER_PROBE_BYTES: u64 = 128 * 1024;

fn from_file(display: String, s: &jhol_ai::safetensors::WeightSummary, read: u64, reqs: u32) -> InspectReport {
    InspectReport {
        display,
        tensors: s.tensors,
        params: s.params,
        data_bytes: s.data_bytes,
        vram_bytes: s.vram_bytes(),
        header_bytes: s.header_bytes as u64,
        dtypes: s.dtypes.clone(),
        unknown_dtypes: s.unknown_dtypes.clone(),
        largest: s.largest.clone(),
        shards: Vec::new(),
        sidecars: Vec::new(),
        bytes_read: read,
        requests: reqs,
    }
}

fn from_dir(dir: &Path) -> Result<InspectReport, String> {
    let m = jhol_ai::safetensors::summarise_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let shards: Vec<(String, u64)> = m
        .shards
        .iter()
        .map(|(name, s)| (name.clone(), s.data_bytes))
        .collect();
    Ok(InspectReport {
        display: dir.display().to_string(),
        tensors: m.shards.iter().map(|(_, s)| s.tensors).sum(),
        params: m.params,
        data_bytes: m.data_bytes,
        vram_bytes: m.vram_bytes(),
        header_bytes: m.header_bytes,
        dtypes: m.dtypes,
        unknown_dtypes: m.unknown_dtypes,
        largest: m.largest,
        shards,
        sidecars: m.sidecars,
        // A local directory costs one header read per shard, and no requests.
        bytes_read: m.header_bytes + 8 * m.shards.len() as u64,
        requests: 0,
    })
}

/// Describe a weight file or a sharded model directory from headers alone, fetching only the
/// header when the file is remote. A 7 GB checkpoint costs one 128 KB range request.
pub fn inspect_target(root: &Path, target: &str) -> Result<InspectReport, String> {
    let Some(url) = remote_url(target) else {
        let path = resolve_weight_target(root, target)?;
        if path.is_dir() {
            return from_dir(&path);
        }
        let summary = jhol_ai::safetensors::summarise(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let read = 8 + summary.header_bytes as u64;
        return Ok(from_file(path.display().to_string(), &summary, read, 0));
    };

    let mut bytes = jhol_core::http_client::get_range(&url, 0, HEADER_PROBE_BYTES - 1)
        .map_err(|e| format!("{url}: {e}"))?;
    let mut requests = 1;
    let need = jhol_ai::safetensors::header_len(&bytes[..8.min(bytes.len())])
        .map_err(|e| format!("{url}: {e}"))?
        + 8;
    if bytes.len() as u64 >= need {
        bytes.truncate(need as usize);
    } else {
        let more = jhol_core::http_client::get_range(&url, bytes.len() as u64, need - 1)
            .map_err(|e| format!("{url}: {e}"))?;
        bytes.extend_from_slice(&more);
        requests += 1;
        if (bytes.len() as u64) < need {
            return Err(format!(
                "{url}: server returned {} of {need} header bytes",
                bytes.len()
            ));
        }
    }
    let summary =
        jhol_ai::safetensors::summarise_bytes(&bytes).map_err(|e| format!("{url}: {e}"))?;
    Ok(from_file(url, &summary, bytes.len() as u64, requests))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jhol_core::status::{Artifact, Axes, Status};

    fn status_with(dir: &str, gpu: Option<u64>) -> Status {
        let mut s = Status::default();
        s.axes = Axes { gpu_mem_mb: gpu, ..Default::default() };
        s.artifacts = vec![Artifact {
            kind: "model".into(),
            name: "w".into(),
            path: Some(dir.into()),
            present: true,
            ..Default::default()
        }];
        s
    }

    /// A safetensors file whose header declares one F32 tensor of `n` elements.
    fn write_weights(path: &Path, n: u64) {
        let header = format!(
            "{{\"t\":{{\"dtype\":\"F32\",\"shape\":[{n}],\"data_offsets\":[0,{}]}}}}",
            n * 4
        );
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend(std::iter::repeat(0u8).take((n * 4) as usize));
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_huggingface_target_becomes_one_resolve_url() {
        assert_eq!(
            remote_url("hf://meta-llama/Llama-3-8B/model.safetensors").unwrap(),
            "https://huggingface.co/meta-llama/Llama-3-8B/resolve/main/model.safetensors"
        );
        assert_eq!(
            remote_url("hf://o/r/m.safetensors@abc123").unwrap(),
            "https://huggingface.co/o/r/resolve/abc123/m.safetensors"
        );
        assert_eq!(remote_url("./local.safetensors"), None);
        assert_eq!(remote_url("llama-tiny"), None);
    }

    #[test]
    fn a_weight_is_measured_from_its_header_and_compared_to_the_host() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        write_weights(&dir.path().join("m/model.safetensors"), 1024);

        // 1024 F32 = 4096 bytes of tensors, 20% overhead -> ~4.9 KB, fits in 1 MB.
        let s = enrich(status_with("m", Some(1)), dir.path());
        let a = &s.artifacts[0];
        assert_eq!(a.params, Some(1024));
        assert_eq!(a.vram_bytes, Some(4096 + 819));
        assert_eq!(a.fits, Some(true));
        assert_eq!(a.dtypes.get("F32"), Some(&1));
    }

    #[test]
    fn without_a_gpu_the_verdict_is_unknown_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        write_weights(&dir.path().join("m/model.safetensors"), 1024);
        let s = enrich(status_with("m", None), dir.path());
        assert_eq!(s.artifacts[0].fits, None, "unknown must never read as a pass");
    }

    #[test]
    fn a_weight_too_big_for_the_host_is_a_failure_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        write_weights(&dir.path().join("m/model.safetensors"), 1024 * 1024); // 4 MiB
        let s = enrich(status_with("m", Some(1)), dir.path()); // 1 MB of GPU memory
        let a = &s.artifacts[0];
        assert_eq!(a.fits, Some(false));
        assert!(a.reasons.iter().any(|r| r.contains("does not fit")), "{:?}", a.reasons);
    }
}
