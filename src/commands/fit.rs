//! `jhol fit <target> --vram 24 --ctx 8192` — will this model run on the machine I have, or the
//! machine I am about to rent?
//!
//! The answer is computed from the header alone: weights from the tensor descriptors, KV cache
//! from `block_count`, `attention.head_count_kv` and `head_dim`, plus a stated overhead. For a
//! repository, every quant is sized with the same KV and the largest one that fits is
//! recommended — quality you can run beats quality you cannot.

use clap::ArgMatches;

fn gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
}

pub fn run(m: &ArgMatches) -> Result<(), String> {
    let target = m.get_one::<String>("target").map(|s| s.as_str());
    let vram = m
        .get_one::<String>("vram")
        .and_then(|v| v.parse::<f64>().ok())
        .map(|gb| (gb * 1_073_741_824.0) as u64);
    let ctx = m
        .get_one::<String>("ctx")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(8192);
    let batch = m
        .get_one::<String>("batch")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1);

    let kv = m
        .get_one::<String>("kv")
        .map(|v| v.as_str())
        .unwrap_or("f16");
    let precision = jhol_ai::gguf::KvPrecision::parse(kv)
        .ok_or_else(|| format!("fit: unknown --kv {kv}; use f16, q8 or q4"))?;
    let explain = crate::flag(m, "explain");

    // A directory sweep answers "which of these files should I serve?".
    if let Some(dir) = m.get_one::<String>("all") {
        let reports = crate::status_view::fit_sweep(
            std::path::Path::new(dir),
            vram,
            ctx,
            batch,
            precision,
        );
        if reports.is_empty() {
            return Err(format!("fit: no .gguf or .safetensors files in {dir}"));
        }
        if crate::flag(m, "json") {
            let rows: Vec<serde_json::Value> = reports
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "target": r.target,
                        "weights": r.weights,
                        "kv_cache": r.kv,
                        "total": r.total,
                        "fits": r.fits,
                        "max_context": r.max_context,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
            return Ok(());
        }
        println!("{dir} at {ctx} tokens, kv {}{}", precision.label(), match vram {
            Some(v) => format!(", budget {:.1} GB", v as f64 / 1_073_741_824.0),
            None => String::new(),
        });
        for r in &reports {
            let mark = match r.fits {
                Some(true) => "✓",
                Some(false) => "✗",
                None => " ",
            };
            let name = r
                .target
                .rsplit('/')
                .next()
                .unwrap_or(&r.target)
                .to_string();
            println!(
                "  {mark} {:<44} {:>9}{}",
                name,
                gb(r.total),
                r.max_context
                    .map(|c| format!("  up to {c} tokens"))
                    .unwrap_or_default()
            );
        }
        return Ok(());
    }

    let Some(target) = target else {
        return Err("fit: give a model, or --all <dir> to sweep a directory".to_string());
    };
    let root = std::path::Path::new(".");
    let r = crate::status_view::fit_target_with(root, target, vram, ctx, batch, precision)?;

    if crate::flag(m, "json") {
        let quants: Vec<serde_json::Value> = r
            .quants
            .iter()
            .map(|q| {
                serde_json::json!({
                    "name": q.name,
                    "weights": q.weights,
                    "total": q.total,
                    "fits": q.fits,
                    "max_context": q.max_context,
                })
            })
            .collect();
        let out = serde_json::json!({
            "target": r.target,
            "weights": r.weights,
            "kv_cache": r.kv,
            "overhead": r.overhead,
            "total": r.total,
            "context": r.ctx,
            "batch": r.batch,
            "vram": r.vram,
            "fits": r.fits,
            "headroom": r.headroom,
            "meta": {
                "layers": r.meta.layers,
                "heads": r.meta.heads,
                "kv_heads": r.meta.kv_heads,
                "head_dim": r.meta.head_dim,
                "context": r.meta.context,
                "vocab": r.meta.vocab,
                "rope_theta": r.meta.rope_theta,
            },
            "missing_meta": r.missing_meta,
            "quants": quants,
            "recommend": r.recommend,
            "kv_precision": r.kv_precision.label(),
            "max_context": r.max_context,
            "bytes_read": r.bytes_read,
            "requests": r.requests,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }

    println!("{}", r.target);
    if let (Some(layers), Some(heads)) = (r.meta.layers, r.meta.heads) {
        let kv_heads = r.meta.kv_heads.unwrap_or(heads);
        let dim = r.meta.head_dim.unwrap_or(0);
        println!(
            "  {layers} layers · {heads} heads ({kv_heads} kv) · head_dim {dim} · native context {}",
            r.meta.context.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        );
    }
    println!("  weights        {:>9}", gb(r.weights));
    if r.kv > 0 {
        println!(
            "  kv cache       {:>9}   {} tokens, batch {}, {}",
            gb(r.kv),
            r.ctx,
            r.batch,
            r.kv_precision.label()
        );
    } else if !r.missing_meta.is_empty() {
        println!(
            "  kv cache       {:>9}   unknown: header lacks {}",
            "—",
            r.missing_meta.join(", ")
        );
    }
    println!("  overhead       {:>9}   15% of weights", gb(r.overhead));
    println!("  ─────────────────────────");
    println!("  total          {:>9}", gb(r.total));
    match (r.vram, r.fits, r.headroom) {
        (Some(_), Some(true), Some(head)) => {
            println!("  verdict        fits, {:.2} GB to spare", head as f64 / 1_073_741_824.0)
        }
        (Some(_), Some(false), Some(head)) => {
            println!("  verdict        does not fit, {:.2} GB over", (-head) as f64 / 1_073_741_824.0)
        }
        _ => println!("  verdict        pass --vram <GB> to compare against a machine"),
    }
    if let Some(max_ctx) = r.max_context {
        println!(
            "  context        {:>9}   tokens fit in this budget at {}",
            max_ctx,
            r.kv_precision.label()
        );
    }
    if !r.quants.is_empty() {
        println!();
        println!("  quants at {}-token context", r.ctx);
        for q in &r.quants {
            let mark = match q.fits {
                Some(true) => "✓",
                Some(false) => "✗",
                None => " ",
            };
            println!(
                "    {mark} {:<38} {:>9}{}",
                q.name,
                gb(q.total),
                if explain {
                    q.max_context
                        .map(|c| format!("  up to {c} tokens"))
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            );
        }
        if let Some(best) = &r.recommend {
            println!();
            println!("  recommend      {best} — the largest quant that still fits");
        } else if r.vram.is_some() {
            println!();
            println!("  recommend      nothing fits this budget at {} tokens", r.ctx);
        }
        if explain {
            println!();
            println!("  at this budget, longest context that still fits:");
            for q in r.quants.iter().filter(|q| q.fits == Some(true)).take(3) {
                if let Some(c) = q.max_context {
                    println!("    {:<38} {:>9} tokens", q.name, c);
                }
            }
        }
    }
    println!();
    println!(
        "  read from header only: {} bytes{}",
        r.bytes_read,
        match r.requests {
            0 => String::new(),
            n => format!(" over {n} request(s)"),
        }
    );
    Ok(())
}
