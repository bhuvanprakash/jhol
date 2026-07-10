//! Simple runtime benchmark driver for jhol-runtime.
//!
//! Usage:
//!   cargo run -p jhol-runtime --example bench -- <js-file> [warmup-runs] [timed-runs]
//!
//! Prints the median execution time in milliseconds to stdout.

use std::env;
use std::path::Path;
use std::time::Instant;

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn average(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: bench <js-file> [warmup-runs] [timed-runs]");
        std::process::exit(1);
    }

    let path = Path::new(&args[1]);
    let warmup = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let runs = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);

    for _ in 0..warmup {
        let _ = jhol_runtime::run_file(path);
    }

    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        match jhol_runtime::run_file(path) {
            Ok(_) => {
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                times.push(elapsed);
            }
            Err(e) => {
                eprintln!("Runtime error: {}", e);
                std::process::exit(1);
            }
        }
    }

    let avg = average(&times);
    let med = median(&mut times.clone());
    let min = times.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = times.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    println!(
        "{{\"runtime\":\"jhol\",\"file\":\"{}\",\"runs\":{},\"avg_ms\":{:.3},\"median_ms\":{:.3},\"min_ms\":{:.3},\"max_ms\":{:.3}}}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        runs,
        avg,
        med,
        min,
        max
    );
}
