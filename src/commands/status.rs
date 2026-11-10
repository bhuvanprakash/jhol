//! `jhol status` — one screen of truth, text or `--json`.
//!
//! Every field comes from `jhol_core::status`, the same struct the TUI and the web
//! dashboard render. Nothing here collects its own numbers.

use clap::ArgMatches;

pub fn run(m: &ArgMatches) -> Result<(), String> {
    let root = std::path::Path::new(".");
    let status = jhol_core::status::collect(root);
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
    Ok(())
}
