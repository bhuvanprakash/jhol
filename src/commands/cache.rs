//! `jhol cache` — list, size, prune, gc, export, import, clean, telemetry, key.

use clap::ArgMatches;

pub fn run(sub: &ArgMatches) -> Result<(), String> {
    // `cache clean` must work in exactly the state that breaks cache
    // initialisation: a read-only or foreign-owned cache directory is when
    // you need it most. Every other subcommand still requires a usable
    // cache, because it reads or writes one.
    if let Err(e) = jhol_core::init_cache() {
        if !matches!(sub.subcommand(), Some(("clean", _))) {
            return Err(format!("Failed to initialize cache: {}", e));
        }
        if std::env::var("JHOL_VERBOSE").is_ok() {
            eprintln!("[jhol] cache init failed ({e}); continuing with clean");
        }
    }
    match sub.subcommand() {
        Some(("list", _)) => {
            let list = jhol_core::list_cached_packages()
                .map_err(|e| format!("Failed to list cache: {}", e))?;
            let (bytes, _) = jhol_core::cache_size_bytes()
                .map_err(|e| format!("Failed to get cache size: {}", e))?;
            if list.is_empty() {
                crate::dim("No cached packages.");
            } else {
                crate::info(&format!("Cached packages ({})", list.len()));
                for name in list {
                    println!("  {}", name);
                }
                crate::dim(&format!("Total size: {} MB", bytes / 1024 / 1024));
            }
        }
        Some(("size", _)) => {
            let (bytes, count) = jhol_core::cache_size_bytes()
                .map_err(|e| format!("Failed to get cache size: {}", e))?;
            crate::info(&format!(
                "Cache: {} tarball(s), {} MB",
                count,
                bytes / 1024 / 1024
            ));
        }
        Some(("prune", sub_prune)) => {
            let keep = sub_prune.get_one::<usize>("keep").copied();
            let n = jhol_core::cache_prune(keep)
                .map_err(|e| format!("Failed to prune cache: {}", e))?;
            crate::success(&format!("Pruned {} tarball(s).", n));
        }
        Some(("gc", gc_m)) => {
            let dry = crate::flag(&gc_m, "dry-run");
            let mut lockfiles: Vec<std::path::PathBuf> = gc_m
                .get_many::<String>("from")
                .map(|v| v.map(std::path::PathBuf::from).collect())
                .unwrap_or_default();
            if lockfiles.is_empty() {
                lockfiles.push(std::path::PathBuf::from("package-lock.json"));
            }
            match jhol_core::registry_query::store_gc(&lockfiles, dry) {
                Ok((kept, freed, files)) => {
                    if dry {
                        println!(
                            "gc (dry run): {} tarball(s) reachable, {} file(s) / {} freeable",
                            kept,
                            files,
                            crate::human_bytes(freed)
                        );
                    } else {
                        println!(
                            "gc: {} tarball(s) kept, freed {} file(s) / {}",
                            kept,
                            files,
                            crate::human_bytes(freed)
                        );
                    }
                }
                Err(e) => return Err(format!("cache gc: {}", e)),
            }
        }
        Some(("export", sub_exp)) => {
            let dir = sub_exp
                .get_one::<String>("dir")
                .map(|s| s.as_str())
                .unwrap();
            let n = jhol_core::cache_export(std::path::Path::new(dir))
                .map_err(|e| format!("Export failed: {}", e))?;
            crate::success(&format!("Exported {} package(s) to {}.", n, dir));
        }
        Some(("import", sub_imp)) => {
            let dir = sub_imp
                .get_one::<String>("dir")
                .map(|s| s.as_str())
                .unwrap();
            let n = jhol_core::cache_import(std::path::Path::new(dir))
                .map_err(|e| format!("Import failed: {}", e))?;
            crate::success(&format!("Imported {} package(s) from {}.", n, dir));
        }
        Some(("clean", _)) => {
            let report = jhol_core::cache_clean_report()
                .map_err(|e| format!("Failed to clean cache: {}", e))?;
            if report.freed_bytes > 0 && report.removed > 0 {
                crate::success(&format!(
                    "Removed {} cached package(s), freed {}.",
                    report.removed,
                    crate::human_bytes(report.freed_bytes)
                ));
            } else {
                crate::success(&format!("Removed {} cached package(s).", report.removed));
            }
            if !report.failed.is_empty() {
                eprintln!(
                    "jhol: {} entr{} could not be removed:",
                    report.failed.len(),
                    if report.failed.len() == 1 { "y" } else { "ies" }
                );
                for (path, reason) in report.failed.iter().take(5) {
                    eprintln!("  {}  ({})", path.display(), reason);
                }
                if report.failed.len() > 5 {
                    eprintln!("  … and {} more", report.failed.len() - 5);
                }
                if let Some(hint) = report.hint() {
                    eprintln!("\n{}", hint);
                }
                // Partial failure is failure: a script that runs
                // `jhol cache clean` and checks the status must not be told
                // the cache is empty when entries survived.
                return Err(format!(
                    "{} cached entr{} could not be removed",
                    report.failed.len(),
                    if report.failed.len() == 1 { "y" } else { "ies" }
                ));
            }
        }
        Some(("telemetry", _)) => {
            let v = jhol_core::read_fallback_telemetry();
            println!(
                "{}",
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
            );
        }
        Some(("key", _)) => {
            let hash = jhol_core::lockfile_content_hash(std::path::Path::new("."))
                .unwrap_or_else(|| "none".to_string());
            println!("{}", hash);
        }
        Some(("mirror", sub)) => match sub.subcommand() {
            Some(("stats", _)) => {
                let stats = jhol_core::resolution_mirror::global_mirror().stats();
                crate::info(&format!("JMR Resolution Mirror:"));
                println!("  Total entries:    {}", stats.total_entries);
                println!("  Fresh entries:    {}", stats.fresh_entries);
                println!("  Expired entries:  {}", stats.expired_entries);
                if stats.total_entries > 0 {
                    let pct = (stats.fresh_entries as f64 / stats.total_entries as f64) * 100.0;
                    println!("  Fresh ratio:      {:.1}%", pct);
                }
            }
            Some(("prune", _)) => {
                let n = jhol_core::resolution_mirror::global_mirror()
                    .prune()
                    .map_err(|e| format!("Failed to prune mirror: {}", e))?;
                crate::success(&format!("Pruned {} expired mirror entries.", n));
            }
            Some(("clear", _)) => {
                jhol_core::resolution_mirror::global_mirror()
                    .clear()
                    .map_err(|e| format!("Failed to clear mirror: {}", e))?;
                crate::success("Cleared all mirror entries.");
            }
            Some(("lookup", sub_l)) => {
                let pkg = sub_l
                    .get_one::<String>("package")
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let (name, range) = if pkg.contains('@') && !pkg.starts_with('@') {
                    let parts: Vec<&str> = pkg.splitn(2, '@').collect();
                    (parts[0], if parts.len() > 1 { parts[1] } else { "*" })
                } else if pkg.starts_with('@') {
                    let at_idx = pkg.rfind('@').unwrap_or(0);
                    if at_idx > 0 {
                        (&pkg[..at_idx], &pkg[at_idx + 1..])
                    } else {
                        (pkg, "*")
                    }
                } else {
                    (pkg, "*")
                };
                match jhol_core::resolution_mirror::global_mirror().lookup(name, range) {
                    Some(entry) => {
                        crate::info(&format!("Mirror entry for {}@{}", name, range));
                        println!("  Version:          {}", entry.version);
                        println!("  Dep signature:    {}", entry.dep_signature);
                        println!("  Dependencies:     {}", entry.deps.len());
                        println!(
                            "  Tarball URL:      {}",
                            entry.tarball_url.as_deref().unwrap_or("(none)")
                        );
                        println!(
                            "  Integrity:        {}",
                            entry.integrity.as_deref().unwrap_or("(none)")
                        );
                    }
                    None => crate::dim(&format!("No mirror entry for {}@{}", name, range)),
                }
            }
            _ => {
                crate::dim("Use `jhol cache mirror stats`, `jhol cache mirror prune`, `jhol cache mirror clear`, or `jhol cache mirror lookup <package>`.");
            }
        },
        _ => {
            crate::dim("Use `jhol cache list`, `jhol cache size`, `jhol cache prune`, `jhol cache export <dir>`, `jhol cache import <dir>`, `jhol cache clean`, `jhol cache telemetry`, or `jhol cache key`.");
        }
    }
    Ok(())
}
