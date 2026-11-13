//! Ultra-thin install trampoline (Instant Lock front-door).
//!
//! 1. Warm no-op: `node_modules/.jhol-ok` mtime ≥ `package.json` → exit 0 (< 0.2ms)
//! 2. JGrid L0 Direct Projection: CoW clone cached bundle → exit 0 (< 1.5ms)
//! 3. Fall through: exec `jhol`

use std::env;
use std::path::Path;
use std::process::Command;

mod fmt_lane;
mod proof;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if try_instant_trampoline(&args) {
        std::process::exit(0);
    }
    // `jhol fmt <files>` where every file is provably already formatted: print what the
    // full CLI would print and stop. Anything uncertain falls through to the CLI.
    if let Ok(cwd) = env::current_dir() {
        if let Some(n) = fmt_lane::try_fresh(&args, &cwd) {
            eprintln!("fmt: {} file(s), 0 changed, engine {}", n, fmt_lane::FMT_ENGINE);
            std::process::exit(0);
        }
    }

    // global-install layout: real CLI is `jhol-bin` (this file is `jhol`/front);
    // dev checkouts keep an adjacent `jhol`.
    let mut bin = env::current_exe().unwrap_or_default();
    let mut real = bin.clone();
    real.set_file_name(if cfg!(windows) {
        "jhol-bin.exe"
    } else {
        "jhol-bin"
    });
    if real.is_file() {
        bin = real;
    } else {
        bin.set_file_name(if cfg!(windows) { "jhol.exe" } else { "jhol" });
    }
    // Loop guard: the full CLI is always the large binary. A small sibling
    // (e.g. the 34 KB front door named `jhol`) must never be exec'd back —
    // front → trampoline → front would spin forever on an incomplete layout.
    if let Ok(md) = std::fs::metadata(&bin) {
        if md.len() < 1_000_000 {
            eprintln!(
                "jhol: full CLI (jhol-bin) not found next to the trampoline \
                 — install layout incomplete"
            );
            std::process::exit(127);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new(&bin).args(&args).exec();
        eprintln!("jhol: failed to exec {}: {}", bin.display(), err);
        std::process::exit(127);
    }
    #[cfg(not(unix))]
    {
        match Command::new(&bin).args(&args).status() {
            Ok(st) => std::process::exit(st.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("jhol: failed to exec {}: {}", bin.display(), e);
                std::process::exit(127);
            }
        }
    }
}

/// Which argv shapes the lanes are allowed to answer. Mirrors jhol-bin's
/// `is_plain_install_before_clap` whitelist so the two front doors agree.
/// See the comment in `try_instant_trampoline` for per-flag soundness.
fn plain_install_gate(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    let mut has_install_cmd = false;
    for arg in args {
        match arg.as_str() {
            "install" | "i" => has_install_cmd = true,
            "-q" | "--quiet" | "--native-only" | "--no-scripts" | "--offline" => {}
            _ => return false,
        }
    }
    has_install_cmd
}

fn try_instant_trampoline(args: &[String]) -> bool {
    let timing = std::env::var("JHOL_TRAMP_TIMING").is_ok();
    let t0 = std::time::Instant::now();
    if !plain_install_gate(args) {
        return false;
    }

    // The prepared proof checks marker identity and the live layout.
    if proof::prepared_tree_is_valid(Path::new(".")) {
        if timing {
            eprintln!("[tramp] lane1 noop hit: {:?}", t0.elapsed());
        }
        return true;
    }
    let t1 = std::time::Instant::now();

    // 2. Direct L0 projection from JGrid bundle cache
    if proof::try_direct_bundle_projection(Path::new(".")) {
        if timing {
            eprintln!("[tramp] lane2 projection hit: {:?}", t0.elapsed());
        }
        return true;
    }
    if timing {
        eprintln!("[tramp] after lane1+lane2 checks: {:?}", t1.elapsed());
    }
    let t2 = std::time::Instant::now();

    // 2b. Fresh project (no lockfile): replay the bundle published by a previous
    // byte-identical install — same contract as reusing a committed lockfile.
    if proof::try_fresh_project_bundle_replay(Path::new(".")) {
        if timing {
            eprintln!(
                "[tramp] lane2b hit: total {:?} | since-lane2-end {:?}",
                t0.elapsed(),
                t2.elapsed()
            );
        }
        return true;
    }
    if timing {
        eprintln!("[tramp] all lanes missed: {:?}", t0.elapsed());
    }
    false
}

#[cfg(test)]
mod forwarding_tests {
    #[test]
    fn whitelisted_flags_pass_everything_else_forwards() {
        // lanes may answer these (gate passes); tree state decides hit/miss
        for argv in [
            vec!["install"],
            vec!["install", "--quiet"],
            vec!["install", "-q"],
            vec!["install", "--native-only"],
            vec!["install", "--no-scripts"],
            vec!["install", "--offline"],
            vec!["i", "--quiet"],
        ] {
            let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            assert!(
                super::plain_install_gate(&owned),
                "gate must pass: {argv:?}"
            );
        }
        // everything else must reach the real CLI
        for argv in [
            vec!["install", "--frozen"],
            vec!["install", "--lockfile-only"],
            vec!["install", "--production"],
            vec!["install", "--strict-peer-deps"],
            vec!["install", "--prefer=latest"],
            vec!["install", "a@1.0.0"],
            vec!["install", "express"],
            vec!["ci"],
            vec!["install", "--quiet", "express"],
        ] {
            let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            assert!(
                !super::plain_install_gate(&owned),
                "gate must reject: {argv:?}"
            );
        }
    }
}
