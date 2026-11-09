//! Resolve one root against a packument cache and report the instance count.
//!
//! Usage: nest_bench <cache-dir> <name> <spec>
//! Used to measure whether a change makes the tree smaller, not just faster:
//! every avoidable instance is an extra directory to write and verify.
use std::collections::HashMap;
use std::io::Read;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().cloned().unwrap_or_default();
    let name = args.get(1).cloned().unwrap_or_else(|| "jest".into());
    let spec = args.get(2).cloned().unwrap_or_else(|| "29.7.0".into());

    let mut world: HashMap<String, serde_json::Value> = HashMap::new();
    let t_load = Instant::now();
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
            let mut buf = Vec::new();
            if flate2::read::GzDecoder::new(body)
                .read_to_end(&mut buf)
                .is_err()
            {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) else {
                continue;
            };
            if let Some(n) = v.get("name").and_then(|n| n.as_str()) {
                world.insert(n.to_string(), v);
            }
        }
    }
    println!(
        "[nest] loaded {} packuments in {:.0} ms",
        world.len(),
        t_load.elapsed().as_secs_f64() * 1000.0
    );

    let deps = HashMap::from([(name.clone(), spec.clone())]);
    let t = Instant::now();
    match jhol_core::jagr_nest::resolve_with_options(&world, &deps, &Default::default()) {
        Ok(r) => {
            let mut dups: Vec<String> = Vec::new();
            for pkg in r.flatten_for_tree().keys() {
                let vs: Vec<&str> = r
                    .instances
                    .iter()
                    .filter(|i| &i.package == pkg)
                    .map(|i| i.version.as_str())
                    .collect();
                if vs.len() > 1 {
                    dups.push(format!("{pkg}: {}", vs.join(", ")));
                }
            }
            dups.sort();
            println!(
                "[nest] {name}@{spec}: {} instances across {} packages, {} with >1 version, {:.1} ms",
                r.instances.len(),
                r.flatten_for_tree().len(),
                r.duplicated_packages(),
                t.elapsed().as_secs_f64() * 1000.0
            );
            for d in dups.iter().take(10) {
                println!("[nest]   {d}");
            }
        }
        Err(e) => println!("[nest] conflict: {}", e.describe()),
    }
}
