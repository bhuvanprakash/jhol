//! Resolution benchmark over a real packument cache.
//!
//! Usage:
//!   cargo run -q -p jhol-core --example resolve_bench -- <packument-dir> [root...]
//!
//! The cache stores HTTP bodies as `W/"etag"\tgzip\n<gzip bytes>`, which is what
//! an install actually reads — so the phases measured here are the real ones:
//! gunzip, JSON parse, chart build, propagation, search.
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

fn load_packuments(dir: &Path) -> (HashMap<String, serde_json::Value>, u128) {
    let load_start = Instant::now();
    let mut out = HashMap::new();
    let mut failed = 0usize;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().map(|x| x != "json").unwrap_or(true) {
                continue;
            }
            let Ok(raw) = std::fs::read(&path) else { continue };
            // Strip the cache header line before the gzip payload.
            let body: &[u8] = match raw.iter().position(|b| *b == b'\n') {
                Some(i) if raw[..i].windows(4).any(|w| w == b"gzip") => &raw[i + 1..],
                _ => &raw[..],
            };
            let mut d = flate2::read::GzDecoder::new(body);
            let mut buf = Vec::new();
            if d.read_to_end(&mut buf).is_err() {
                failed += 1;
                continue;
            }
            let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&buf) else {
                failed += 1;
                continue;
            };
            jhol_core::registry::prune_packument_in_place(&mut v);
            if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                out.insert(name.to_string(), v);
            } else {
                failed += 1;
            }
        }
    }
    if failed > 0 {
        eprintln!("[bench] skipped {} unreadable entries", failed);
    }
    (out, load_start.elapsed().as_micros())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args
        .first()
        .cloned()
        .unwrap_or_else(|| "/tmp/real-cache/packuments".to_string());
    let roots: Vec<String> = if args.len() > 1 {
        args[1..].to_vec()
    } else {
        vec!["express".to_string()]
    };

    let (packuments, load_us) = load_packuments(Path::new(&dir));
    let deps: HashMap<String, String> = roots
        .iter()
        .map(|r| (r.clone(), "*".to_string()))
        .collect();
    println!(
        "[bench] loaded {} packuments (gunzip + JSON parse) in {:.1} ms",
        packuments.len(),
        load_us as f64 / 1000.0
    );

    let (selection, s, err) = jhol_core::jagr_infinite::select_versions_stats(&packuments, &deps);
    println!(
        "[bench] packages={} edges={} distinct_specs={} | chart={:.1} ms propagate={:.1} ms search={:.1} ms | selected={} total={:.1} ms",
        s.packages,
        s.edges,
        s.distinct_specs,
        s.chart_us as f64 / 1000.0,
        s.propagate_us as f64 / 1000.0,
        s.search_us as f64 / 1000.0,
        selection.as_ref().map(|m| m.len()).unwrap_or(0),
        (s.chart_us + s.propagate_us + s.search_us) as f64 / 1000.0
    );
    if let Some(e) = err {
        let short: String = e.chars().take(220).collect();
        println!("[bench] unresolved: {}", short);
    }
}
