//! Priority Benchmark: Test all 3 optimization ideas in a realistic cold-install pipeline.
//!
//! Models the actual jhol metadata pipeline: fetch packuments → parse JSON → cache to disk →
//! re-expansion reads. Every config does IDENTICAL per-packument work (fetch+parse+write);
//! the only differences are the three optimizations. Runs are interleaved and medians taken
//! (same methodology as the project's own benchmark rules).
//!
//! Ideas:
//! 1. In-memory packument cache (re-expansion reads hit RAM instead of disk)
//! 2. Parallel packument prefetch (wave-based parallel fetch instead of sequential BFS)
//! 3. Capsule binary format (bincode stand-in for jhol's own binary format vs JSON+gzip)

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

// ============================================================================
// Realistic dependency graph (bench_cold fixture: 6 deps, ~74 packages)
// ============================================================================

struct DepGraph {
    direct_deps: HashMap<String, String>,
    all_packages: Vec<String>,
    deps: HashMap<String, Vec<String>>,
}

fn build_realistic_graph() -> DepGraph {
    let direct_deps: HashMap<String, String> = [
        ("express".to_string(), "^4.18.2".to_string()),
        ("lodash".to_string(), "^4.17.21".to_string()),
        ("uuid".to_string(), "^9.0.0".to_string()),
        ("chalk".to_string(), "^5.3.0".to_string()),
        ("axios".to_string(), "^1.6.0".to_string()),
        ("react".to_string(), "^18.2.0".to_string()),
    ]
    .into_iter()
    .collect();

    let mut all_packages: Vec<String> = direct_deps.keys().cloned().collect();
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();

    let express_deps = vec![
        "accepts",
        "body-parser",
        "cookie",
        "debug",
        "encodeurl",
        "escape-html",
        "etag",
        "finalhandler",
        "fresh",
        "http-errors",
        "merge-descriptors",
        "methods",
        "on-finished",
        "parseurl",
        "path-to-regexp",
        "proxy-addr",
        "qs",
        "range-parser",
        "safe-buffer",
        "send",
        "serve-static",
        "setprototypeof",
        "statuses",
        "toidentifier",
        "type-is",
        "utils-merge",
        "vary",
    ];
    deps.insert(
        "express".to_string(),
        express_deps.iter().map(|s| s.to_string()).collect(),
    );
    all_packages.extend(express_deps.iter().map(|s| s.to_string()));

    deps.insert("lodash".to_string(), vec![]);
    deps.insert("uuid".to_string(), vec![]);

    let chalk_deps = vec!["ansi-styles", "supports-color"];
    deps.insert(
        "chalk".to_string(),
        chalk_deps.iter().map(|s| s.to_string()).collect(),
    );
    all_packages.extend(chalk_deps.iter().map(|s| s.to_string()));

    let axios_deps = vec!["follow-redirects", "form-data", "proxy-from-env"];
    deps.insert(
        "axios".to_string(),
        axios_deps.iter().map(|s| s.to_string()).collect(),
    );
    all_packages.extend(axios_deps.iter().map(|s| s.to_string()));

    let react_deps = vec!["loose-envify", "js-tokens"];
    deps.insert(
        "react".to_string(),
        react_deps.iter().map(|s| s.to_string()).collect(),
    );
    all_packages.extend(react_deps.iter().map(|s| s.to_string()));

    // Cross-edges (packages discovered by >1 parent — these are the re-expansion reads)
    deps.insert(
        "accepts".to_string(),
        vec!["mime-types", "negotiator"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    deps.insert(
        "body-parser".to_string(),
        vec!["bytes", "content-type", "debug", "depd"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    deps.insert(
        "mime-types".to_string(),
        vec!["mime-db"].iter().map(|s| s.to_string()).collect(),
    );
    deps.insert(
        "supports-color".to_string(),
        vec!["has-flag"].iter().map(|s| s.to_string()).collect(),
    );
    deps.insert(
        "form-data".to_string(),
        vec!["asynckit", "combined-stream", "mime-types"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );

    let extra = vec![
        "array-flatten",
        "asynckit",
        "bytes",
        "call-bind",
        "combined-stream",
        "content-disposition",
        "content-type",
        "cookie-signature",
        "debug",
        "depd",
        "destroy",
        "ee-first",
        "encodeurl",
        "escape-html",
        "etag",
        "finalhandler",
        "forwarded",
        "fresh",
        "function-bind",
        "get-intrinsic",
        "has-flag",
        "has-symbols",
        "http-errors",
        "iconv-lite",
        "inherits",
        "ipaddr.js",
        "is-function",
        "media-typer",
        "merge-descriptors",
        "methods",
        "mime",
        "mime-db",
        "ms",
        "negotiator",
        "object-inspect",
        "on-finished",
        "parseurl",
        "path-to-regexp",
        "proxy-addr",
        "qs",
        "range-parser",
        "raw-body",
        "safe-buffer",
        "safer-buffer",
        "send",
        "serve-static",
        "setprototypeof",
        "side-channel",
        "statuses",
        "toidentifier",
        "type-is",
        "unpipe",
        "utils-merge",
        "vary",
    ];
    all_packages.extend(extra.iter().map(|s| s.to_string()));
    for e in extra {
        deps.entry(e.to_string()).or_default();
    }

    // A couple of extra cross-edges so re-expansion is realistically exercised
    deps.entry("debug".to_string())
        .or_default()
        .push("ms".to_string());
    deps.entry("send".to_string())
        .or_default()
        .push("ms".to_string());

    DepGraph {
        direct_deps,
        all_packages,
        deps,
    }
}

/// In-degree map: how many times each package is discovered (= 1 first fetch
/// + (k-1) re-expansion reads, exactly the pattern the real pipeline pays).
fn in_degrees(graph: &DepGraph) -> HashMap<String, usize> {
    let mut indeg: HashMap<String, usize> = HashMap::new();
    for children in graph.deps.values() {
        for c in children {
            *indeg.entry(c.clone()).or_insert(0) += 1;
        }
    }
    indeg
}

// ============================================================================
// Packument creation (realistic sizes)
// ============================================================================

fn create_packument_for(name: &str) -> serde_json::Value {
    let n_versions = match name {
        "express" => 200,
        "react" => 150,
        "axios" => 100,
        "chalk" => 80,
        "lodash" => 50,
        "uuid" => 30,
        _ => 20,
    };

    let mut versions = serde_json::Map::new();
    for i in 0..n_versions {
        let version = format!("{}.{}.{}", i / 100, (i / 10) % 10, i % 10);
        let mut pkg = serde_json::Map::new();
        pkg.insert(
            "name".to_string(),
            serde_json::Value::String(name.to_string()),
        );
        pkg.insert(
            "version".to_string(),
            serde_json::Value::String(version.clone()),
        );
        if i % 3 == 0 {
            let mut d = serde_json::Map::new();
            d.insert(
                "lodash".to_string(),
                serde_json::Value::String("^4.17.0".to_string()),
            );
            pkg.insert("dependencies".to_string(), serde_json::Value::Object(d));
        }
        let mut dist = serde_json::Map::new();
        dist.insert(
            "tarball".to_string(),
            serde_json::Value::String(format!(
                "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                name, name, version
            )),
        );
        dist.insert(
            "integrity".to_string(),
            serde_json::Value::String(format!("sha512-{}", "a".repeat(128))),
        );
        pkg.insert("dist".to_string(), serde_json::Value::Object(dist));
        versions.insert(version, serde_json::Value::Object(pkg));
    }

    let mut packument = serde_json::Map::new();
    packument.insert(
        "name".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    packument.insert("versions".to_string(), serde_json::Value::Object(versions));
    let mut dist_tags = serde_json::Map::new();
    dist_tags.insert(
        "latest".to_string(),
        serde_json::Value::String("1.0.0".to_string()),
    );
    packument.insert(
        "dist-tags".to_string(),
        serde_json::Value::Object(dist_tags),
    );
    serde_json::Value::Object(packument)
}

fn gzip_bytes(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn gunzip_bytes(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut dec = flate2::read::GzDecoder::new(data);
    let mut buf = Vec::new();
    dec.read_to_end(&mut buf).ok()?;
    Some(buf)
}

// ============================================================================
// Pipeline simulation — identical per-item work in ALL configs
// ============================================================================

#[derive(Clone)]
struct PipelineConfig {
    use_in_memory_cache: bool, // Idea 1
    use_parallel_fetch: bool,  // Idea 2
    use_binary_format: bool,   // Idea 3
}

fn run_pipeline(graph: &DepGraph, config: &PipelineConfig) -> (u128, u128, u128, u128) {
    let parse_acc = AtomicU64::new(0);
    let cache_acc = AtomicU64::new(0);
    let tmp_dir = std::env::temp_dir().join(format!("jhol_prio_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Per-packument work: fetch(create) + parse + write cache — SAME for every config.
    let process = |name: &str| -> serde_json::Value {
        let packument = create_packument_for(name);

        let p = Instant::now();
        let json_bytes = serde_json::to_vec(&packument).unwrap();
        let _: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
        parse_acc.fetch_add(p.elapsed().as_micros() as u64, Ordering::Relaxed);

        let c = Instant::now();
        let bytes = if config.use_binary_format {
            bincode::serialize(&json_bytes).unwrap()
        } else {
            gzip_bytes(&json_bytes)
        };
        let _ = std::fs::write(
            tmp_dir.join(format!("{}.c", name.replace('/', "_"))),
            &bytes,
        );
        cache_acc.fetch_add(c.elapsed().as_micros() as u64, Ordering::Relaxed);

        packument
    };

    let total_start = Instant::now();
    let mut fetched: HashMap<String, serde_json::Value> = HashMap::new();

    if config.use_parallel_fetch {
        // Wave-based parallel fetch. Same per-item work via `process`.
        use rayon::prelude::*;
        let mut queue: VecDeque<String> = graph.direct_deps.keys().cloned().collect();
        let mut seen: HashSet<String> = HashSet::new();
        while !queue.is_empty() {
            let batch: Vec<String> = queue.drain(..).filter(|n| !seen.contains(n)).collect();
            if batch.is_empty() {
                break;
            }
            let results: Vec<(String, serde_json::Value)> = batch
                .par_iter()
                .map(|name| (name.clone(), process(name)))
                .collect();
            for (name, packument) in results {
                seen.insert(name.clone());
                if let Some(children) = graph.deps.get(&name) {
                    for child in children {
                        if !seen.contains(child) {
                            queue.push_back(child.clone());
                        }
                    }
                }
                fetched.insert(name, packument);
            }
        }
    } else {
        // Sequential BFS — same `process` per item.
        let mut queue: VecDeque<String> = graph.direct_deps.keys().cloned().collect();
        while let Some(name) = queue.pop_front() {
            if fetched.contains_key(&name) {
                continue;
            }
            let packument = process(&name);
            if let Some(children) = graph.deps.get(&name) {
                for child in children {
                    if !fetched.contains_key(child) {
                        queue.push_back(child.clone());
                    }
                }
            }
            fetched.insert(name, packument);
        }
    }

    let fetch_time = total_start.elapsed().as_micros();

    // Re-expansion reads: each package discovered by k parents costs (k-1) extra reads.
    // Baseline reads back from disk (gunzip+parse); Idea 1 hits the in-RAM map.
    let indeg = in_degrees(graph);
    let mut mem_cache: HashMap<String, serde_json::Value> = if config.use_in_memory_cache {
        fetched.clone()
    } else {
        HashMap::new()
    };

    let reexp_start = Instant::now();
    for (name, k) in &indeg {
        for _ in 1..*k {
            if config.use_in_memory_cache {
                // Idea 1: RAM lookup
                let _ = mem_cache.get(name).cloned();
            } else {
                // Baseline: disk read + decode + parse
                let path = tmp_dir.join(format!("{}.c", name.replace('/', "_")));
                if let Ok(raw) = std::fs::read(&path) {
                    let p = Instant::now();
                    let json: Vec<u8> = if config.use_binary_format {
                        bincode::deserialize(&raw).unwrap_or_default()
                    } else {
                        gunzip_bytes(&raw).unwrap_or_default()
                    };
                    let _: serde_json::Value = serde_json::from_slice(&json).unwrap();
                    parse_acc.fetch_add(p.elapsed().as_micros() as u64, Ordering::Relaxed);
                }
            }
        }
    }
    let reexp_time = reexp_start.elapsed().as_micros();

    let total_time = total_start.elapsed().as_micros();
    let _ = reexp_time;
    let parse_time = parse_acc.load(Ordering::Relaxed);
    let cache_time = cache_acc.load(Ordering::Relaxed);
    let _ = fetch_time;
    (total_time, fetch_time, parse_time.into(), cache_time.into())
}

fn median(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v[v.len() / 2]
}

// ============================================================================
// Main benchmark — interleaved rounds, medians
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "exploratory benchmark, not a CI gate — run with `cargo test -- --ignored`"]
    fn benchmark_all_three_priorities() {
        let graph = build_realistic_graph();
        println!("\n=== Priority Benchmark: 3 Optimization Ideas (interleaved, median of 7) ===");
        println!(
            "Graph: {} direct deps, {} total packages\n",
            graph.direct_deps.len(),
            graph.all_packages.len()
        );

        let configs: Vec<(&str, PipelineConfig)> = vec![
            (
                "baseline",
                PipelineConfig {
                    use_in_memory_cache: false,
                    use_parallel_fetch: false,
                    use_binary_format: false,
                },
            ),
            (
                "idea1_memory",
                PipelineConfig {
                    use_in_memory_cache: true,
                    use_parallel_fetch: false,
                    use_binary_format: false,
                },
            ),
            (
                "idea2_parallel",
                PipelineConfig {
                    use_in_memory_cache: false,
                    use_parallel_fetch: true,
                    use_binary_format: false,
                },
            ),
            (
                "idea3_binary",
                PipelineConfig {
                    use_in_memory_cache: false,
                    use_parallel_fetch: false,
                    use_binary_format: true,
                },
            ),
            (
                "combined",
                PipelineConfig {
                    use_in_memory_cache: true,
                    use_parallel_fetch: true,
                    use_binary_format: true,
                },
            ),
        ];

        let rounds = 7usize;
        let mut totals: HashMap<&str, Vec<u128>> = HashMap::new();
        let mut parses: HashMap<&str, Vec<u128>> = HashMap::new();
        let mut caches: HashMap<&str, Vec<u128>> = HashMap::new();
        for (name, _) in &configs {
            totals.insert(name, Vec::new());
            parses.insert(name, Vec::new());
            caches.insert(name, Vec::new());
        }

        for round in 0..rounds {
            // Rotate order each round to cancel order effects
            for i in 0..configs.len() {
                let (name, cfg) = &configs[(i + round) % configs.len()];
                let (total, _, parse, cache) = run_pipeline(&graph, cfg);
                totals.get_mut(name).unwrap().push(total);
                parses.get_mut(name).unwrap().push(parse);
                caches.get_mut(name).unwrap().push(cache);
            }
        }

        let base = median(totals["baseline"].clone());
        println!(
            "{:<16} {:>10} {:>10} {:>10} {:>12} {:>10}",
            "config", "total", "parse", "cache", "speedup", "reduction"
        );
        for (name, _) in &configs {
            let t = median(totals[name].clone());
            let p = median(parses[name].clone());
            let c = median(caches[name].clone());
            let speedup = base as f64 / t as f64;
            let reduction = 100.0 * (base as f64 - t as f64) / base as f64;
            println!(
                "{:<16} {:>10} {:>10} {:>10} {:>11.2}x {:>9.1}%",
                name, t, p, c, speedup, reduction
            );
        }

        println!("\n=== Individual contribution (μs saved vs baseline) ===");
        for name in ["idea1_memory", "idea2_parallel", "idea3_binary", "combined"] {
            let t = median(totals[name].clone());
            let saved = base as i64 - t as i64;
            let pct = 100.0 * saved as f64 / base as f64;
            println!(
                "{:<16} {:>8} μs saved ({:>5.1}% of baseline)",
                name, saved, pct
            );
        }

        // Interaction: does combining beat the sum of parts?
        let i1 = base as i64 - median(totals["idea1_memory"].clone()) as i64;
        let i2 = base as i64 - median(totals["idea2_parallel"].clone()) as i64;
        let i3 = base as i64 - median(totals["idea3_binary"].clone()) as i64;
        let all = base as i64 - median(totals["combined"].clone()) as i64;
        println!(
            "\nSum of individual savings: {} μs; actual combined: {} μs",
            i1 + i2 + i3,
            all
        );
        println!("(combined < sum means the ideas overlap on the same time)");
    }
}
