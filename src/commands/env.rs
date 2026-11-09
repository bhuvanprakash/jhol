//! `jhol env` — copy-on-write fork environments.
//!
//! Extracted from `main.rs` otherwise unchanged: the group is self-contained (it touches
//! `.jhol/envs`, the lockfile reader and the fork helper), which makes it the cheapest
//! seam to start taking commands out of a 9,700-line file.

use crate::flag;
use clap::ArgMatches;

pub fn run(sub_m: &ArgMatches) -> Result<(), String> {
    let envs = std::path::Path::new(".jhol/envs");
    match sub_m.subcommand() {
            Some(("fork", m)) => {
                let name = m.get_one::<String>("name").cloned().unwrap_or_default();
                if name.is_empty() || name.contains('/') || name.contains("..") {
                    return Err("env fork: invalid name".into());
                }
                let from = m
                    .get_one::<String>("from")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let dest = envs.join(&name);
                std::fs::create_dir_all(envs).map_err(|e| e.to_string())?;
                let started = std::time::Instant::now();
                // Only node_modules is forked: the fork shares the identity files by
                // copying them, so both environments stay independently lockable.
                let stats = jhol_core::env_fork::fork_tree(
                    &from.join("node_modules"),
                    &dest.join("node_modules"),
                )
                .map_err(|e| format!("env fork: {e}"))?;
                for file in jhol_core::env_fork::identity_files(&from) {
                    if let Some(name) = file.file_name() {
                        let _ = std::fs::copy(&file, dest.join(name));
                    }
                }
                if flag(&m, "json") {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "name": name,
                            "dir": dest.display().to_string(),
                            "dirs": stats.dirs, "files": stats.files,
                            "cloned": stats.cloned, "copied": stats.copied,
                            "referenced_bytes": stats.bytes,
                            "added_bytes": stats.extra_bytes(),
                            "ms": started.elapsed().as_millis() as u64,
                        }))
                        .unwrap_or_default()
                    );
                } else {
                    println!(
                        "forked {name} in {} ms — {}",
                        started.elapsed().as_millis(),
                        stats.describe()
                    );
                    println!("  run it with: cd {} · or copy it back when it works", dest.display());
                }
                return Ok(());
            }
            Some(("list", m)) => {
                let mut names: Vec<String> = std::fs::read_dir(envs)
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| e.path().is_dir())
                            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                names.sort();
                if flag(&m, "json") {
                    println!("{}", serde_json::to_string_pretty(&names).unwrap_or_default());
                } else if names.is_empty() {
                    println!("no forks (jhol env fork <name>)");
                } else {
                    for n in names {
                        println!("{n}");
                    }
                }
                return Ok(());
            }
            Some(("diff", m)) => {
                let a = m.get_one::<String>("a").cloned().unwrap_or_default();
                let b = m.get_one::<String>("b").cloned().unwrap_or_default();
                // Omitting the second name compares the fork with the project it came
                // from, which is the question a fork exists to answer. Requiring two
                // names made the obvious invocation (`env diff exp1`) an error.
                let b_label = if b.is_empty() { ".".to_string() } else { b.clone() };
                let read = |name: &str| {
                    let dir = if name.is_empty() || name == "." {
                        std::path::PathBuf::from(".")
                    } else {
                        envs.join(name)
                    };
                    jhol_core::lockfile::read_resolved_from_dir(&dir).unwrap_or_default()
                };
                let (ma, mb) = (read(&a), read(&b));
                let mut keys: Vec<&String> = ma.keys().chain(mb.keys()).collect();
                keys.sort();
                keys.dedup();
                let mut n = 0;
                for k in keys {
                    let va = ma.get(k);
                    let vb = mb.get(k);
                    if va != vb {
                        n += 1;
                        println!(
                            "{k}: {} -> {}",
                            va.map(String::as_str).unwrap_or("-"),
                            vb.map(String::as_str).unwrap_or("-")
                        );
                    }
                }
                if n == 0 {
                    println!("{a} and {b_label} resolve identically");
                }
                return Ok(());
            }
            Some(("rm", m)) => {
                let name = m.get_one::<String>("name").cloned().unwrap_or_default();
                std::fs::remove_dir_all(envs.join(&name)).map_err(|e| e.to_string())?;
                println!("removed fork {name}");
                return Ok(());
            }
        _ => Err("usage: jhol env fork|list|diff|rm".into()),
    }
}
