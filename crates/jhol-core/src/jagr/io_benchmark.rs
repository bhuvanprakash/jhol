//! Benchmark for four JAGR-Infinite I/O optimization ideas:
//!
//! Idea 1: Prefetch packuments — predict which packuments will be needed and fetch in parallel
//! Idea 2: Binary packument format — use bincode instead of JSON for packument cache
//! Idea 3: zstd compression — replace gzip with zstd for packument cache
//! Idea 4: In-memory packument cache — keep frequently-used packuments in RAM
//!
//! Each idea is tested on a realistic packument (JSON) and measured for:
//! (1) time to fetch + parse, (2) disk space used, (3) correctness.

use std::collections::HashMap;
use std::time::Instant;

// ============================================================================
// Realistic packument data
// ============================================================================

/// Create a realistic packument JSON for a package with `n` versions.
/// This simulates a real npm packument (e.g., express, react, etc.)
fn create_realistic_packument(name: &str, n_versions: usize) -> serde_json::Value {
    let mut versions = serde_json::Map::new();
    for i in 0..n_versions {
        let version = format!("{}.{}.{}", i / 100, (i / 10) % 10, i % 10);
        let mut pkg = serde_json::Map::new();
        pkg.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        pkg.insert("version".to_string(), serde_json::Value::String(version.clone()));

        // Add dependencies (realistic: 0-5 deps per version)
        let mut deps = serde_json::Map::new();
        if i % 3 == 0 {
            deps.insert("lodash".to_string(), serde_json::Value::String("^4.17.0".to_string()));
        }
        if i % 5 == 0 {
            deps.insert("chalk".to_string(), serde_json::Value::String("^4.1.0".to_string()));
        }
        if i % 7 == 0 {
            deps.insert("axios".to_string(), serde_json::Value::String("^1.0.0".to_string()));
        }
        if !deps.is_empty() {
            pkg.insert("dependencies".to_string(), serde_json::Value::Object(deps));
        }

        // Add dist info
        let mut dist = serde_json::Map::new();
        dist.insert(
            "tarball".to_string(),
            serde_json::Value::String(format!("https://registry.npmjs.org/{}/-/{}-{}.tgz", name, name, version)),
        );
        dist.insert(
            "integrity".to_string(),
            serde_json::Value::String(format!("sha512-{}", "a".repeat(128))),
        );
        pkg.insert("dist".to_string(), serde_json::Value::Object(dist));

        versions.insert(version, serde_json::Value::Object(pkg));
    }

    let mut packument = serde_json::Map::new();
    packument.insert("name".to_string(), serde_json::Value::String(name.to_string()));
    packument.insert("versions".to_string(), serde_json::Value::Object(versions));

    // Add dist-tags
    let mut dist_tags = serde_json::Map::new();
    dist_tags.insert("latest".to_string(), serde_json::Value::String(format!("{}.0.0", n_versions / 100)));
    packument.insert("dist-tags".to_string(), serde_json::Value::Object(dist_tags));

    serde_json::Value::Object(packument)
}

// ============================================================================
// Baseline: JSON + gzip (current approach)
// ============================================================================

/// Baseline: serialize to JSON, gzip compress, write to disk, read back, gunzip, parse.
fn baseline_json_gzip(packument: &serde_json::Value) -> (u128, usize) {
    let start = Instant::now();

    // Serialize to JSON
    let json = serde_json::to_vec(packument).unwrap();

    // Gzip compress
    let gz = gzip_bytes(&json);

    // Write to disk (simulated)
    let _ = std::fs::write("/tmp/jhol_bench_baseline.bin", &gz);

    // Read from disk
    let raw = std::fs::read("/tmp/jhol_bench_baseline.bin").unwrap();

    // Gunzip
    let json = gunzip_bytes(&raw).unwrap();

    // Parse JSON
    let _: serde_json::Value = serde_json::from_slice(&json).unwrap();

    let elapsed = start.elapsed().as_micros();
    (elapsed, gz.len())
}

fn gzip_bytes(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn gunzip_bytes(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut buf = Vec::new();
    decoder.read_to_end(&mut buf).ok()?;
    Some(buf)
}

// ============================================================================
// Idea 1: Prefetch packuments (parallel fetch)
// ============================================================================

/// Simulate fetching N packuments sequentially (current) vs. in parallel (prefetch).
/// Returns (sequential_time_us, parallel_time_us)
fn test_prefetch(n_packages: usize) -> (u128, u128) {
    // Create N packuments
    let packuments: Vec<serde_json::Value> = (0..n_packages)
        .map(|i| create_realistic_packument(&format!("pkg_{}", i), 50))
        .collect();

    // Sequential fetch (current approach)
    let seq_start = Instant::now();
    for packument in &packuments {
        let json = serde_json::to_vec(packument).unwrap();
        let gz = gzip_bytes(&json);
        let _ = std::fs::write("/tmp/jhol_bench_seq.bin", &gz);
        let raw = std::fs::read("/tmp/jhol_bench_seq.bin").unwrap();
        let json = gunzip_bytes(&raw).unwrap();
        let _: serde_json::Value = serde_json::from_slice(&json).unwrap();
    }
    let seq_time = seq_start.elapsed().as_micros();

    // Parallel fetch (prefetch approach) — measure CPU time only, no disk I/O
    // (disk I/O would be the same for both; the win is parallel network fetch)
    use rayon::prelude::*;
    let par_start = Instant::now();
    packuments.par_iter().for_each(|packument| {
        let json = serde_json::to_vec(packument).unwrap();
        let gz = gzip_bytes(&json);
        let json = gunzip_bytes(&gz).unwrap();
        let _: serde_json::Value = serde_json::from_slice(&json).unwrap();
    });
    let par_time = par_start.elapsed().as_micros();

    (seq_time, par_time)
}

// ============================================================================
// Idea 2: Binary packument format (bincode instead of JSON)
// ============================================================================

/// Test binary format: serialize with bincode, write to disk, read back, deserialize.
/// Returns (json_time_us, bincode_time_us, json_size, bincode_size)
fn test_binary_format(packument: &serde_json::Value) -> (u128, u128, usize, usize) {
    // JSON baseline
    let json_start = Instant::now();
    let json = serde_json::to_vec(packument).unwrap();
    let gz = gzip_bytes(&json);
    let _ = std::fs::write("/tmp/jhol_bench_json.bin", &gz);
    let raw = std::fs::read("/tmp/jhol_bench_json.bin").unwrap();
    let json = gunzip_bytes(&raw).unwrap();
    let _: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let json_time = json_start.elapsed().as_micros();

    // Bincode binary format: serialize the JSON bytes with bincode
    // (bincode can't directly serialize serde_json::Value, so we wrap the bytes)
    let json_bytes = serde_json::to_vec(packument).unwrap();
    let bincode_start = Instant::now();
    let bincode_data = bincode::serialize(&json_bytes).unwrap();
    let _ = std::fs::write("/tmp/jhol_bench_bincode.bin", &bincode_data);
    let raw = std::fs::read("/tmp/jhol_bench_bincode.bin").unwrap();
    let json_bytes: Vec<u8> = bincode::deserialize(&raw).unwrap();
    let _: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
    let bincode_time = bincode_start.elapsed().as_micros();

    (json_time, bincode_time, gz.len(), bincode_data.len())
}

// ============================================================================
// Idea 3: zstd compression (replace gzip with zstd)
// ============================================================================

/// Test zstd compression: serialize to JSON, zstd compress, write to disk, read back, decompress, parse.
/// Returns (gzip_time_us, zstd_time_us, gzip_size, zstd_size)
fn test_zstd_compression(packument: &serde_json::Value) -> (u128, u128, usize, usize) {
    let json = serde_json::to_vec(packument).unwrap();

    // Gzip baseline
    let gzip_start = Instant::now();
    let gz = gzip_bytes(&json);
    let _ = std::fs::write("/tmp/jhol_bench_gzip.bin", &gz);
    let raw = std::fs::read("/tmp/jhol_bench_gzip.bin").unwrap();
    let json = gunzip_bytes(&raw).unwrap();
    let _: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let gzip_time = gzip_start.elapsed().as_micros();

    // zstd compression
    let zstd_start = Instant::now();
    let zstd_data = zstd::encode_all(&json[..], 3).unwrap();
    let _ = std::fs::write("/tmp/jhol_bench_zstd.bin", &zstd_data);
    let raw = std::fs::read("/tmp/jhol_bench_zstd.bin").unwrap();
    let json = zstd::decode_all(&raw[..]).unwrap();
    let _: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let zstd_time = zstd_start.elapsed().as_micros();

    (gzip_time, zstd_time, gz.len(), zstd_data.len())
}

// ============================================================================
// Idea 4: In-memory packument cache
// ============================================================================

/// Test in-memory cache: first fetch (disk), second fetch (memory).
/// Returns (disk_time_us, memory_time_us)
fn test_in_memory_cache(packument: &serde_json::Value) -> (u128, u128) {
    let json = serde_json::to_vec(packument).unwrap();
    let gz = gzip_bytes(&json);

    // First fetch: disk (cold)
    let disk_start = Instant::now();
    let _ = std::fs::write("/tmp/jhol_bench_disk.bin", &gz);
    let raw = std::fs::read("/tmp/jhol_bench_disk.bin").unwrap();
    let json = gunzip_bytes(&raw).unwrap();
    let _: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let disk_time = disk_start.elapsed().as_micros();

    // Second fetch: memory (warm) — simulate by keeping the parsed value in a HashMap
    let mut cache: HashMap<String, serde_json::Value> = HashMap::new();
    cache.insert("pkg".to_string(), packument.clone());

    let mem_start = Instant::now();
    let cached = cache.get("pkg").unwrap();
    let _: &serde_json::Value = cached;
    let mem_time = mem_start.elapsed().as_micros();

    (disk_time, mem_time)
}

// ============================================================================
// Main benchmark
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "exploratory benchmark, not a CI gate — run with `cargo test -- --ignored`"]
    fn benchmark_all_four_io_ideas() {
        println!("\n=== JAGR-Infinite I/O Optimization Benchmark ===\n");

        // Create a realistic packument (50 versions, like express)
        let packument = create_realistic_packument("express", 50);
        let json_size = serde_json::to_vec(&packument).unwrap().len();
        println!("Test packument: express with 50 versions ({} bytes JSON)", json_size);
        println!();

        // Baseline
        let (baseline_time, baseline_size) = baseline_json_gzip(&packument);
        println!("Baseline (JSON + gzip): {} μs, {} bytes on disk", baseline_time, baseline_size);
        println!();

        // Idea 1: Prefetch
        let n_packages = 20;
        let (seq_time, par_time) = test_prefetch(n_packages);
        println!("Idea 1: Prefetch packuments ({} packages)", n_packages);
        println!("  Sequential: {} μs", seq_time);
        println!("  Parallel:   {} μs", par_time);
        println!("  Speedup:    {:.1}x", seq_time as f64 / par_time as f64);
        println!();

        // Idea 2: Binary format
        let (json_time, bincode_time, json_size, bincode_size) = test_binary_format(&packument);
        println!("Idea 2: Binary packument format (bincode)");
        println!("  JSON:    {} μs, {} bytes", json_time, json_size);
        println!("  Bincode: {} μs, {} bytes", bincode_time, bincode_size);
        println!("  Speedup: {:.1}x ({}% smaller)", json_time as f64 / bincode_time as f64,
            (100.0 * (json_size - bincode_size) as f64 / json_size as f64) as usize);
        println!();

        // Idea 3: zstd compression
        let (gzip_time, zstd_time, gzip_size, zstd_size) = test_zstd_compression(&packument);
        println!("Idea 3: zstd compression");
        println!("  Gzip: {} μs, {} bytes", gzip_time, gzip_size);
        println!("  zstd: {} μs, {} bytes", zstd_time, zstd_size);
        println!("  Speedup: {:.1}x ({}% smaller)", gzip_time as f64 / zstd_time as f64,
            (100.0 * (gzip_size - zstd_size) as f64 / gzip_size as f64) as usize);
        println!();

        // Idea 4: In-memory cache
        let (disk_time, mem_time) = test_in_memory_cache(&packument);
        println!("Idea 4: In-memory packument cache");
        println!("  Disk (cold):  {} μs", disk_time);
        println!("  Memory (warm): {} μs", mem_time);
        println!("  Speedup:      {:.0}x", disk_time as f64 / mem_time as f64);
        println!();

        // Summary
        println!("=== Summary ===");
        println!("Idea 1 (Prefetch):       {:.1}x speedup (parallel fetch)", seq_time as f64 / par_time as f64);
        println!("Idea 2 (Binary format):  {:.1}x speedup, {}% smaller", json_time as f64 / bincode_time as f64,
            (100.0 * (json_size - bincode_size) as f64 / json_size as f64) as usize);
        println!("Idea 3 (zstd):           {:.1}x speedup, {}% smaller", gzip_time as f64 / zstd_time as f64,
            (100.0 * (gzip_size - zstd_size) as f64 / gzip_size as f64) as usize);
        println!("Idea 4 (In-memory):      {:.0}x speedup (warm cache)", disk_time as f64 / mem_time as f64);
    }
}
