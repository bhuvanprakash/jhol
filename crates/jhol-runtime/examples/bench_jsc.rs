#[cfg(feature = "jsc-engine")]
mod inner {
    use std::env;
    use std::path::Path;
    use std::time::Instant;

    pub fn main() {
        let args: Vec<String> = env::args().collect();
        if args.len() < 2 {
            eprintln!("Usage: bench_jsc <js-file> [warmup] [runs]");
            std::process::exit(1);
        }
        let path = Path::new(&args[1]);
        let warmup = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
        let runs = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);

        for _ in 0..warmup {
            let _ = jhol_runtime::jsc_engine::run_file(path);
        }

        let mut times = Vec::with_capacity(runs);
        for _ in 0..runs {
            let start = Instant::now();
            match jhol_runtime::jsc_engine::run_file(path) {
                Ok(_) => times.push(start.elapsed().as_secs_f64() * 1000.0),
                Err(e) => {
                    eprintln!("Runtime error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        let med = times[times.len() / 2];
        println!(
            "{{\"runtime\":\"jsc\",\"file\":\"{}\",\"runs\":{},\"avg_ms\":{:.3},\"median_ms\":{:.3}}}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            runs, avg, med
        );
    }
}

#[cfg(not(feature = "jsc-engine"))]
fn main() {
    eprintln!("bench_jsc requires the jsc-engine feature");
    std::process::exit(1);
}

#[cfg(feature = "jsc-engine")]
fn main() {
    inner::main();
}
