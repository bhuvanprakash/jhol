//! `jhol inspect <path | dir | pinned-model | hf://…>` — describe weights from headers.
//!
//! A `.safetensors` header maps every tensor to a byte range, so a 7 GB checkpoint answers
//! "how many parameters, which dtypes, how much memory?" after reading a few kilobytes.
//! Real checkpoints are **sharded**, so a directory is read through every shard: describing
//! one shard and calling that the model under-reports parameters by the shard count, which
//! is the kind of wrong that looks right.
//!
//! Nothing is downloaded (beyond a header, for a remote target) and nothing is loaded.

use clap::ArgMatches;

pub fn run(m: &ArgMatches) -> Result<(), String> {
    let target = m
        .get_one::<String>("target")
        .map(|s| s.as_str())
        .ok_or_else(|| "inspect: missing target".to_string())?;
    let root = std::path::Path::new(".");
    // A repository is not a file: inspect the only weight if there is one, otherwise list
    // them. Better than refusing, and far better than blaming the server for a 200.
    let mut target = target.to_string();
    if let Some(resolved) = crate::status_view::resolve_repo(&target) {
        match resolved? {
            crate::status_view::RepoTarget::File(url) => target = url,
            crate::status_view::RepoTarget::Listing(files) => {
                if files.is_empty() {
                    return Err(format!("{target}: no .gguf or .safetensors files"));
                }
                println!("{target} — {} weight file(s)", files.len());
                for (name, bytes) in &files {
                    println!("  {:>10}  {}", human(*bytes), name);
                }
                println!();
                println!(
                    "  pick one: jhol inspect {}/{}",
                    target.trim_end_matches('/'),
                    files[0].0
                );
                return Ok(());
            }
        }
    }
    let report = crate::status_view::inspect_target(root, &target)?;
    let top = m
        .get_one::<String>("top")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(5);

    if crate::flag(m, "json") {
        let dtypes: Vec<serde_json::Value> = report
            .dtypes
            .iter()
            .map(|(d, n)| serde_json::json!({ "dtype": d, "tensors": n }))
            .collect();
        let out = serde_json::json!({
            "path": report.display,
            "format": report.format,
            "architecture": report.architecture,
            "tensors": report.tensors,
            "params": report.params,
            "data_bytes": report.data_bytes,
            "vram_bytes": report.vram_bytes,
            "header_bytes": report.header_bytes,
            "bytes_read": report.bytes_read,
            "requests": report.requests,
            "dtypes": dtypes,
            "unknown_dtypes": report.unknown_dtypes,
            "largest": report.largest.iter().map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
            "shards": report.shards.iter().map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
            "sidecars": report.sidecars.iter().map(|(n, ok)| serde_json::json!({"name": n, "present": ok})).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }

    let host = jhol_core::axes::HostAxes::detect();
    println!("{}", report.display);
    println!(
        "  {} tensors · {} params · {} bytes of tensor data{}",
        report.tensors,
        human_params(report.params),
        report.data_bytes,
        if report.shards.len() > 1 {
            format!(" across {} shards", report.shards.len())
        } else {
            String::new()
        }
    );
    let dtypes: Vec<String> = report
        .dtypes
        .iter()
        .map(|(d, n)| format!("{d}×{n}"))
        .collect();
    println!("  dtypes     {}", dtypes.join(" "));
    if report.format == "gguf" {
        println!(
            "  format     gguf{}",
            report
                .architecture
                .as_deref()
                .map(|a| format!(" · architecture {a}"))
                .unwrap_or_default()
        );
    }
    if !report.unknown_dtypes.is_empty() {
        println!("  unknown    {}", report.unknown_dtypes.join(", "));
    }
    if top > 0 && !report.largest.is_empty() {
        println!("  heaviest tensors");
        for (name, bytes) in report.largest.iter().take(top) {
            println!("    {:<40} {:>10}", name, human(*bytes));
        }
    }
    if !report.sidecars.is_empty() {
        let parts: Vec<String> = report
            .sidecars
            .iter()
            .map(|(n, ok)| format!("{n} {}", if *ok { "✓" } else { "✗" }))
            .collect();
        println!("  sidecars   {}", parts.join("  "));
    }
    match host.gpu_mem_mb {
        Some(mem) => {
            let need_mb = report.vram_bytes.div_ceil(1024 * 1024);
            println!(
                "  resident   {:.1} GB — host GPU {} MB → {}",
                report.vram_bytes as f64 / 1_073_741_824.0,
                mem,
                if need_mb <= mem { "fits" } else { "does not fit" }
            );
        }
        None => println!(
            "  resident   {:.1} GB — no GPU memory detected, fit unknown",
            report.vram_bytes as f64 / 1_073_741_824.0
        ),
    }
    println!(
        "  header     {} bytes read ({} of the file){}",
        report.bytes_read,
        percent(report.bytes_read, report.data_bytes),
        match report.requests {
            0 => String::new(),
            n => format!(" over {n} range request(s)"),
        }
    );
    Ok(())
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

fn percent(read: u64, data: u64) -> String {
    let total = read + data;
    if total == 0 {
        return "?".into();
    }
    format!("{:.4}%", read as f64 * 100.0 / total as f64)
}
