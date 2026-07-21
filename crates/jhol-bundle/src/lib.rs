//! Jhol Bundle — Public API
//!
//! CAS-first JavaScript/TypeScript/JSX bundler.
//! Pipeline: ingest → parse → transform → graph → tree-shake → emit
//!
//! Key insight: every unique content hash is parsed and transformed exactly once.
//! Identical files across projects and rebuilds return instantly from CAS.
//!
//! Usage:
//!   let config = BundleConfig::default();
//!   config.entries = vec![PathBuf::from("./src/index.ts")];
//!   config.out_dir = PathBuf::from("./dist");
//!   let result = bundle(&config)?;

pub mod cas;
pub mod emit;
pub mod graph;
pub mod ingest;
pub mod parse;
pub mod transform;
pub mod tree_shake;

pub mod codegen;
pub mod config;
pub mod css;
pub mod errors;
pub mod externals;
pub mod minify;
pub mod parser;
pub mod plugins;
pub mod sourcemap;
pub mod split;
pub mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use emit::{EmitResult, Emitter};
use errors::{BundleError, BundleResult};
use graph::ModuleGraph;
use ingest::IngestStatus;
use tree_shake::TreeShaker;

use crate::cas::{bundle_cas_store, ParseManifest, TransformedOutput};
use crate::config::BundleConfig;
use crate::emit::OutputFile;

pub const JHOL_BUNDLE_VERSION: &str = "1.0.2";

#[derive(Debug, Clone)]
pub struct BundleResult_ {
    pub files: Vec<OutputFile>,
    pub module_count: usize,
    pub total_size: u64,
    pub total_time_ms: u64,
}

impl From<EmitResult> for BundleResult_ {
    fn from(e: EmitResult) -> Self {
        Self {
            files: e.files,
            module_count: e.module_count,
            total_size: e.total_size,
            total_time_ms: e.total_time_ms,
        }
    }
}

/// Bundle modules from entry points into output files using the CAS-first pipeline.
pub fn bundle(config: &BundleConfig) -> BundleResult<BundleResult_> {
    let start = Instant::now();

    // Initialize CAS store (process-lifetime, lazily)
    init_cas_store()?;

    let entries: Vec<PathBuf> = config.entries.clone();

    // ── Layer 1: Ingest ──────────────────────────────────────────────────
    // Read files, hash content, CAS lookup. Returns cached or needs-parsing.
    let (_hits, misses): (Vec<ParseManifest>, Vec<IngestStatus>) =
        ingest::ingest_files(&entries, config)?;

    let mut all_manifests: Vec<ParseManifest> = _hits;

    // Parse the misses and store in CAS
    let parsed: Vec<ParseManifest> = misses
        .into_iter()
        .filter_map(|status| {
            let IngestStatus::Miss {
                path,
                content_hash,
                source,
                file_type,
            } = status
            else {
                return None;
            };
            let manifest = match parse::parse_to_manifest(
                path.clone(),
                source,
                file_type,
                content_hash,
                entries
                    .iter()
                    .any(|e| fs::canonicalize(e).ok().as_ref() == Some(&path)),
                config,
            ) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Parse error for {}: {}", path.display(), e);
                    return None;
                }
            };
            // Store in CAS
            if let Some(cas) = bundle_cas_store().read().unwrap().as_ref() {
                let _ = cas.put_manifest(&manifest);
            }
            Some(manifest)
        })
        .collect();

    all_manifests.extend(parsed);

    // ── Layer 2: Transform ─────────────────────────────────────────────────
    // Transform all manifests. CAS-first: checks cache, transforms only on miss.
    let transformed: HashMap<u64, Arc<TransformedOutput>> =
        transform::transform_all(&all_manifests, config)?;

    // ── Layer 3: Build module graph ───────────────────────────────────────
    let mut graph = ModuleGraph::new(config.clone());
    let entry_paths: Vec<PathBuf> = entries
        .iter()
        .map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    graph.build_from_manifests(all_manifests, entry_paths.clone())?;

    // ── Fill missing transformed outputs ──────────────────────────────────
    // Graph builder discovers transitive deps not in all_manifests.
    // Transform those too so emit has all content hashes.
    let mut expanded_transformed = transformed.clone();
    for node in graph.nodes.values() {
        if !expanded_transformed.contains_key(&node.content_hash) && !node.is_external
            && node.content_hash != 0 && node.manifest.path.exists()
        {
            let source = match std::fs::read_to_string(&node.manifest.path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let manifest = ParseManifest {
                content_hash: node.content_hash,
                ast_hash: node.manifest.ast_hash,
                path: node.manifest.path.clone(),
                file_type: node.manifest.file_type.clone(),
                source,
                imports: node.manifest.imports.clone(),
                reexports: node.manifest.reexports.clone(),
                dynamic_imports: node.manifest.dynamic_imports.clone(),
                requires: node.manifest.requires.clone(),
                exports: node.manifest.exports.clone(),
                dependency_hashes: node.manifest.dependency_hashes.clone(),
                has_side_effects: node.manifest.has_side_effects,
                is_entry: false,
            };
            if let Ok(t) = crate::transform::transform_manifest(&manifest, config) {
                expanded_transformed.insert(node.content_hash, t);
            }
        }
    }

    // ── Layer 4: Tree shake ───────────────────────────────────────────────
    let shaker = TreeShaker::new();
    let shake_result = shaker.shake(&graph);

    // ── Layer 5: Emit ─────────────────────────────────────────────────────
    let emitter = Emitter::new(config.clone());
    let mut emit_result = emitter.emit(&graph, &expanded_transformed)?;

    // Filter to only used modules in output stats
    let used_count = shake_result.used_hashes.len();

    // ── CSS bundling (separate path) ──────────────────────────────────────
    if config.bundle_css {
        let css_files: Vec<PathBuf> = graph
            .nodes
            .values()
            .filter(|n| n.manifest.file_type == "css")
            .map(|n| PathBuf::from(&n.manifest.path))
            .collect();

        if !css_files.is_empty() {
            let css_bundle = css::bundle_css(&css_files, config)?;
            if !css_bundle.content.is_empty() {
                let css_path = config.out_dir.join("bundle.css");
                std::fs::write(&css_path, &css_bundle.content)
                    .map_err(|e| BundleError::IoError(e))?;
                emit_result.files.push(OutputFile {
                    path: css_path,
                    content: css_bundle.content,
                    sourcemap: None,
                });
            }
        }
    }

    let total_time_ms = start.elapsed().as_millis() as u64;

    if config.stats {
        print_bundle_stats(&emit_result, used_count, total_time_ms);
    }

    Ok(BundleResult_::from(emit_result))
}

/// Bundle with watch mode — watches entry files and rebuilds incrementally.
pub fn bundle_watch(config: BundleConfig) -> BundleResult<()> {
    init_cas_store()?;

    let config_clone = config.clone();
    let mut watch_build = watch::WatchBuild::new(move || {
        let result = bundle(&config_clone)?;
        print_bundle_stats(
            &crate::emit::EmitResult {
                files: result.files,
                module_count: result.module_count,
                total_size: result.total_size,
                total_time_ms: result.total_time_ms,
            },
            result.module_count,
            result.total_time_ms,
        );
        Ok(())
    })?;

    for entry in &config.entries {
        if let Some(parent) = entry.parent() {
            watch_build.watcher.watch_directory(parent)?;
        }
        watch_build.watcher.watch_files(&config.entries)?;
    }

    watch_build.run()
}

/// Bundle for a specific target (convenience API).
pub fn bundle_for_target(
    entries: &[PathBuf],
    out_dir: &Path,
    format: &str,
    target: &str,
    minify: bool,
) -> BundleResult<BundleResult_> {
    let mut config = BundleConfig::default();
    config.entries = entries.to_vec();
    config.out_dir = out_dir.to_path_buf();

    if let Some(f) = config::OutputFormat::from_str(format) {
        config.format = f;
    }
    if let Some(t) = config::Target::from_str(target) {
        config.target = t;
    }
    config.minify = minify;

    bundle(&config)
}

fn init_cas_store() -> BundleResult<()> {
    let max_entries = std::env::var("JHOL_BUNDLE_CACHE_ENTRIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50000);

    if bundle_cas_store().read().unwrap().is_none() {
        cas::init_bundle_cas(max_entries)?;
    }
    Ok(())
}

fn print_bundle_stats(result: &crate::emit::EmitResult, used_count: usize, total_time_ms: u64) {
    println!(
        "  {} modules bundled ({} used, {} tree-shaken) in {}ms",
        result.module_count,
        used_count,
        result.module_count.saturating_sub(used_count),
        total_time_ms
    );
    for file in &result.files {
        let size = file.content.len();
        let size_str = if size > 1024 * 1024 {
            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
        } else if size > 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else {
            format!("{} B", size)
        };
        println!("  {} ({})", file.path.display(), size_str);
    }
}

use std::fs;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundle_config_default() {
        let config = BundleConfig::default();
        assert_eq!(config.format.as_str(), "esm");
        assert!(!config.minify);
    }

    #[test]
    fn test_init_cas_store() {
        std::env::remove_var("JHOL_BUNDLE_CACHE_ENTRIES");
        init_cas_store().unwrap();
        assert!(bundle_cas_store().read().unwrap().is_some());
    }
}
