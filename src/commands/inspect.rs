//! `jhol inspect <path | pinned-model>` — describe a weight file from its header.
//!
//! A `.safetensors` header maps every tensor to a byte range, so a 7 GB checkpoint
//! answers "how many parameters, which dtypes, how much memory?" after reading a few
//! kilobytes. Nothing is downloaded and nothing is loaded.
//!
//! Node has no equivalent operation, and neither does any package manager: this is the
//! reason weights can be treated as first-class artifacts.

use clap::ArgMatches;

pub fn run(m: &ArgMatches) -> Result<(), String> {
    let target = m
        .get_one::<String>("target")
        .map(|s| s.as_str())
        .ok_or_else(|| "inspect: missing target".to_string())?;
    let root = std::path::Path::new(".");
    let inspected = crate::status_view::inspect_target(root, target)?;
    let summary = &inspected.summary;

    if crate::flag(m, "json") {
        let dtypes: Vec<serde_json::Value> = summary
            .dtypes
            .iter()
            .map(|(d, n)| serde_json::json!({ "dtype": d, "tensors": n }))
            .collect();
        let out = serde_json::json!({
            "path": inspected.display,
            "bytes_read": inspected.bytes_read,
            "requests": inspected.requests,
            "tensors": summary.tensors,
            "params": summary.params,
            "params_human": summary.params_human(),
            "data_bytes": summary.data_bytes,
            "vram_bytes": summary.vram_bytes(),
            "header_bytes": summary.header_bytes,
            "dtypes": dtypes,
            "unknown_dtypes": summary.unknown_dtypes,
            "largest": summary.largest.iter().map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }

    let host = jhol_core::axes::HostAxes::detect();
    println!("{}", inspected.display);
    println!(
        "  {} tensors · {} params · {} bytes of tensor data",
        summary.tensors,
        summary.params_human(),
        summary.data_bytes
    );
    let dtypes: Vec<String> = summary
        .dtypes
        .iter()
        .map(|(d, n)| format!("{d}×{n}"))
        .collect();
    println!("  dtypes     {}", dtypes.join(" "));
    if !summary.unknown_dtypes.is_empty() {
        println!("  unknown    {}", summary.unknown_dtypes.join(", "));
    }
    let top = m
        .get_one::<String>("top")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(5);
    if top > 0 && !summary.largest.is_empty() {
        println!("  heaviest tensors");
        for (name, bytes) in summary.largest.iter().take(top) {
            println!("    {:<40} {:>10}", name, human(*bytes));
        }
    }
    let need = summary.vram_bytes();
    match host.gpu_mem_mb {
        Some(mem) => {
            let need_mb = need.div_ceil(1024 * 1024);
            println!(
                "  resident   {:.1} GB — host GPU {} MB → {}",
                need as f64 / 1_073_741_824.0,
                mem,
                if need_mb <= mem { "fits" } else { "does not fit" }
            );
        }
        None => println!(
            "  resident   {:.1} GB — no GPU memory detected, fit unknown",
            need as f64 / 1_073_741_824.0
        ),
    }
    println!(
        "  header     {} bytes read ({} of the file){}",
        inspected.bytes_read,
        percent(inspected.bytes_read, summary.data_bytes),
        match inspected.requests {
            0 => String::new(),
            n => format!(" over {n} range request(s)"),
        }
    );
    Ok(())
}

fn human(n: u64) -> String {
    let gb = 1024.0 * 1024.0 * 1024.0;
    let mb = 1024.0 * 1024.0;
    if n as f64 >= gb {
        format!("{:.2} GB", n as f64 / gb)
    } else if n as f64 >= mb {
        format!("{:.1} MB", n as f64 / mb)
    } else {
        format!("{} KB", (n / 1024).max(1))
    }
}

fn percent(header: u64, data: u64) -> String {
    let total = header + data;
    if total == 0 {
        return "?".into();
    }
    format!("{:.4}%", header as f64 * 100.0 / total as f64)
}
