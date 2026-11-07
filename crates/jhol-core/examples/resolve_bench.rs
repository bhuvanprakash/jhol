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

    // The read side: the resolver's view of the cache directory, using each
    // `.idx` beside a packument when it is valid and falling back to
    // gunzip+JSON when it is not.
    {
        let t = Instant::now();
        let (idx, stats) = jhol_core::jagr_index::load_cache_dir(Path::new(&dir));
        let load_us = t.elapsed().as_micros();
        println!(
            "[bench] cache view: {:.1} ms ({} from index, {} from json, {} skipped, {} packages)",
            load_us as f64 / 1000.0,
            stats.from_index,
            stats.from_json,
            stats.skipped,
            idx.packages.len()
        );
        // Cold cache: write the indexes so the next run shows the warm number.
        if stats.from_index == 0 {
            let mut written = 0usize;
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().map(|x| x != "json").unwrap_or(true) {
                        continue;
                    }
                    let Ok(raw) = std::fs::read(&p) else { continue };
                    let body: &[u8] = match raw.iter().position(|b| *b == b'\n') {
                        Some(i) if raw[..i].windows(4).any(|w| w == b"gzip") => &raw[i + 1..],
                        _ => &raw[..],
                    };
                    let mut d = flate2::read::GzDecoder::new(body);
                    let mut buf = Vec::new();
                    if d.read_to_end(&mut buf).is_err() {
                        continue;
                    }
                    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) else {
                        continue;
                    };
                    let Some(name) = v.get("name").and_then(|n| n.as_str()) else {
                        continue;
                    };
                    let pkg = jhol_core::jagr_index::package_from_packument(name, &v);
                    if jhol_core::jagr_index::write_index_beside(&p, &pkg).is_ok() {
                        written += 1;
                    }
                }
            }
            println!("[bench] wrote {} indexes; re-run for the warm number", written);
        }
    }

    // Binary index (idea 1): the resolver's facts without gunzip/JSON.
    {
        let t_build = Instant::now();
        let index = jhol_core::jagr_index::ResolverIndex::build(&packuments);
        let build_us = t_build.elapsed().as_micros();
        let t_enc = Instant::now();
        let bytes = index.encode();
        let enc_us = t_enc.elapsed().as_micros();
        let t_dec = Instant::now();
        let decoded = jhol_core::jagr_index::ResolverIndex::decode(&bytes)
            .expect("index round-trips");
        let dec_us = t_dec.elapsed().as_micros();
        let t_chart = Instant::now();
        let (name_to_id, versions, version_strings, dp, pr) = decoded.chart_inputs();
        let constraints: HashMap<u32, jhol_core::jagr_infinite::chart::VersionInterval> =
            name_to_id
                .values()
                .map(|id| (*id, jhol_core::jagr_infinite::chart::VersionInterval::full()))
                .collect();
        let chart = jhol_core::jagr_infinite::atlas::build_chart_from_constraints(
            &constraints,
            &versions,
            &version_strings,
            &dp,
            &pr,
        );
        let chart_us = t_chart.elapsed().as_micros();
        println!(
            "[bench] index: build+encode {:.1} ms ({} bytes) | decode {:.2} ms | chart from index {:.1} ms | packages={}",
            (build_us + enc_us) as f64 / 1000.0,
            bytes.len(),
            dec_us as f64 / 1000.0,
            chart_us as f64 / 1000.0,
            chart.packages.len()
        );
    }

    // Nested (per-dependent instances): the multi-major path, with the
    // fetch-and-retry loop a real install performs.
    {
        let t = Instant::now();
        let mut world = packuments.clone();
        let mut fetches = 0usize;
        let mut outcome = jhol_core::jagr_nest::resolve(&world, &deps);
        loop {
            match outcome {
                Ok(_) => break,
                Err(ref e) if e.needs_fetch() && fetches < 600 => {
                    fetches += 1;
                    let name = e.package.clone();
                    match jhol_core::registry::fetch_metadata(&name) {
                        Ok(mut doc) => {
                            jhol_core::registry::prune_packument_in_place(&mut doc);
                            world.insert(name, doc);
                        }
                        Err(err) => {
                            println!("[bench] fetch {name} failed: {err}");
                            break;
                        }
                    }
                    outcome = jhol_core::jagr_nest::resolve(&world, &deps);
                }
                Err(_) => break,
            }
        }
        if fetches > 0 {
            println!("[bench] nested: fetched {} additional packuments", fetches);
        }
        match outcome {
            Ok(r) => {
                println!(
                    "[bench] nested: {} instances across {} packages, {} packages with >1 version, {:.1} ms",
                    r.instances.len(),
                    r.flatten_for_tree().len(),
                    r.duplicated_packages(),
                    t.elapsed().as_secs_f64() * 1000.0
                );
                let mut dups: Vec<String> = Vec::new();
                for name in r.flatten_for_tree().keys() {
                    let vs: Vec<&str> = r
                        .instances
                        .iter()
                        .filter(|i| &i.package == name)
                        .map(|i| i.version.as_str())
                        .collect();
                    if vs.len() > 1 {
                        dups.push(format!("{}: {}", name, vs.join(", ")));
                    }
                }
                dups.sort();
                for d in dups.iter().take(6) {
                    println!("[bench]   nested {d}");
                }
            }
            Err(e) if e.needs_fetch() => println!(
                "[bench] nested: needs fetch of {} (required by {}); retry after fetching",
                e.package, e.required_by
            ),
            Err(e) => println!("[bench] nested conflict: {}", e.describe()),
        }
    }

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
