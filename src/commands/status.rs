//! `jhol status` — one screen of truth, text or `--json`.
//!
//! Every field comes from `jhol_core::status`, the same struct the TUI and the web
//! dashboard render. Nothing here collects its own numbers.

use clap::ArgMatches;

fn human(n: u64) -> String {
    let gb = 1024.0 * 1024.0 * 1024.0;
    let mb = 1024.0 * 1024.0;
    if n as f64 >= gb {
        format!("{:.1} GB", n as f64 / gb)
    } else if n as f64 >= mb {
        format!("{:.1} MB", n as f64 / mb)
    } else {
        format!("{} KB", (n / 1024).max(1))
    }
}

pub fn run(m: &ArgMatches) -> Result<(), String> {
    let root = std::path::Path::new(".");
    if crate::flag(m, "watch") {
        let interval = m
            .get_one::<String>("interval")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(500)
            .max(100);
        return watch(root, interval, crate::flag(m, "json"));
    }
    let status = crate::status_view::enrich(jhol_core::status::collect(root), root);
    if crate::flag(m, "json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_else(|e| e.to_string())
        );
        return Ok(());
    }

    let name = if status.project.is_empty() {
        "(unnamed)".to_string()
    } else {
        format!("{} {}", status.project, status.version)
    };
    println!("{name}");
    println!();
    let tree = if status.tree.state == "match" {
        format!("match — {} packages", status.tree.packages)
    } else {
        format!("{} — {}", status.tree.state, status.tree.detail)
    };
    println!("  tree       {tree}");
    println!(
        "  lockfile   {}",
        if status.manifest_matches_lockfile {
            "matches package.json"
        } else {
            "DRIFTED from package.json — run `jhol install`"
        }
    );
    let mut policy = Vec::new();
    if let Some(at) = &status.policy.as_of {
        policy.push(format!("as-of {at}"));
    }
    if let Some(lic) = &status.policy.allowed_licenses {
        policy.push(format!("licences {}", lic.join(",")));
    }
    if status.policy.exclude_vulnerable {
        policy.push("exclude-vulnerable".to_string());
    }
    println!(
        "  policy     {}",
        if policy.is_empty() {
            "none (latest, any licence, CVEs not filtered)".to_string()
        } else {
            policy.join(" · ")
        }
    );
    if status.models.pinned > 0 {
        println!(
            "  models     {} pinned ({})",
            status.models.pinned,
            status.models.names.join(", ")
        );
    }
    if status.python.pinned > 0 {
        println!("  python     {} distributions", status.python.pinned);
    }
    println!(
        "  axes       {}/{} {} · python {} · cuda {}",
        status.axes.os,
        status.axes.arch,
        status.axes.libc,
        status.axes.python.as_deref().unwrap_or("-"),
        status.axes.cuda_driver.as_deref().unwrap_or("none")
    );
    println!(
        "  proof      snapshot {} · attested {} packages, {} models",
        if status.proof.snapshot_ready {
            "ready"
        } else {
            "absent"
        },
        status.proof.attested_packages,
        status.proof.attested_models
    );

    // The four questions, in the order an ML developer asks them.
    if !status.artifacts.is_empty() {
        println!();
        println!("  artifacts");
        for a in &status.artifacts {
            let mark = match a.fits {
                Some(true) => "✓",
                Some(false) => "✗",
                None => "?",
            };
            let size = match a.bytes {
                Some(b) => format!(" {}", human(b)),
                None => String::new(),
            };
            println!("    {mark} {} {} {}{size}", a.kind, a.name, a.version);
            if a.params.is_some() || !a.dtypes.is_empty() {
                let dtypes: Vec<String> = a
                    .dtypes
                    .iter()
                    .map(|(d, n)| format!("{d}×{n}"))
                    .collect();
                if !dtypes.is_empty() {
                    println!("        {} tensors · {}", a.dtypes.values().sum::<usize>(), dtypes.join(" "));
                }
            }
            for r in &a.reasons {
                println!("        {r}");
            }
        }
    }
    if status.plan.missing_bytes > 0 || !status.plan.items.is_empty() {
        println!();
        println!(
            "  plan       {:.1} MB missing, {:.1} MB cached · ~{} s at {} MB/s",
            status.plan.missing_bytes as f64 / 1_048_576.0,
            status.plan.cached_bytes as f64 / 1_048_576.0,
            status.plan.eta_seconds,
            status.plan.throughput_bytes_per_sec / 1_048_576
        );
        for item in status.plan.items.iter().filter(|i| !i.present) {
            println!(
                "        missing  {} ({:.1} MB)",
                item.name,
                item.bytes as f64 / 1_048_576.0
            );
        }
    }
    if !status.forks.is_empty() {
        println!();
        for f in &status.forks {
            println!("  fork       {} ({:.1} MB beyond base)", f.name, f.bytes as f64 / 1_048_576.0);
        }
    }
    if !status.agent.tools.is_empty() {
        println!();
        println!("  agent      {} MCP tools (jhol mcp --stdio)", status.agent.tools.len());
        for t in &status.agent.tools {
            println!("        {:<26} {:<8} {}", t.name, t.cost, t.purpose);
        }
    }

    Ok(())
}

/// Live status from **one** process, so the per-refresh cost is collection and not process
/// start. In a terminal it rewrites one line; piped, it emits one line per refresh so
/// `jhol status --watch | grep -v 'match'` is a usable alarm.
fn watch(root: &std::path::Path, interval_ms: u64, json: bool) -> Result<(), String> {
    use std::io::{IsTerminal, Write};
    let tty = std::io::stdout().is_terminal();
    let mut last = String::new();
    loop {
        // The same trick the dashboard's ETag uses: if nothing a status depends on has
        // changed, collecting again would produce the same answer for milliseconds on end.
        let tag = jhol_core::status::fingerprint(root);
        if tag == last {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            continue;
        }
        last = tag;
        let status = crate::status_view::enrich(jhol_core::status::collect(root), root);
        if json {
            println!(
                "{}",
                serde_json::to_string(&status).unwrap_or_else(|e| e.to_string())
            );
        } else {
            let unfit = status
                .artifacts
                .iter()
                .filter(|a| a.fits == Some(false))
                .count();
            let unknown = status
                .artifacts
                .iter()
                .filter(|a| a.fits.is_none())
                .count();
            let line = format!(
                "{} {} · tree {} · {} pkgs · lock {} · {:.1} MB missing (~{} s) · artifacts {} unfit {} · unknown {} · forks {}",
                if status.project.is_empty() { "(unnamed)" } else { &status.project },
                status.version,
                status.tree.state,
                status.tree.packages,
                if status.manifest_matches_lockfile { "ok" } else { "DRIFTED" },
                status.plan.missing_bytes as f64 / 1_048_576.0,
                status.plan.eta_seconds,
                status.artifacts.len(),
                unfit,
                unknown,
                status.forks.len()
            );
            if tty {
                print!("\r{line:<100}");
            } else {
                println!("{line}");
            }
            let _ = std::io::stdout().flush();
        }
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
}
