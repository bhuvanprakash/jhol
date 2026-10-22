use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::{BundleConfig, OutputFormat};
use crate::errors::{BundleError, BundleResult};
use crate::graph::{ModuleGraph, ModuleNode};

#[derive(Debug, Clone)]
pub struct OutputFile {
    pub path: PathBuf,
    pub content: String,
    pub sourcemap: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BundleResult_ {
    pub files: Vec<OutputFile>,
    pub module_count: usize,
    pub total_size: u64,
    pub total_time_ms: u64,
}

pub struct CodeGenerator {
    config: BundleConfig,
}

impl CodeGenerator {
    pub fn new(config: BundleConfig) -> Self {
        Self { config }
    }

    pub fn generate(
        &self,
        graph: &ModuleGraph,
        transformed: &HashMap<String, String>,
    ) -> BundleResult<BundleResult_> {
        let start = std::time::Instant::now();
        let mut files = Vec::new();

        // Ensure output directory exists
        std::fs::create_dir_all(&self.config.out_dir).map_err(|e| BundleError::IoError(e))?;

        match self.config.format {
            OutputFormat::ESM => {
                let file = self.generate_esm(graph, transformed)?;
                files.push(file);
            }
            OutputFormat::CJS => {
                let file = self.generate_cjs(graph, transformed)?;
                files.push(file);
            }
            OutputFormat::IIFE => {
                let file = self.generate_iife(graph, transformed)?;
                files.push(file);
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;

        Ok(BundleResult_ {
            files,
            module_count: graph.module_count(),
            total_size: graph.total_size(),
            total_time_ms: elapsed,
        })
    }

    fn generate_esm(
        &self,
        graph: &ModuleGraph,
        transformed: &HashMap<String, String>,
    ) -> BundleResult<OutputFile> {
        let sorted = graph.sorted_modules();
        let chunks: Vec<(Vec<String>, String)> = Vec::new();

        // Collect entry point info
        let entries: Vec<String> = graph
            .entries
            .iter()
            .map(|e| {
                let canonical = std::fs::canonicalize(e).unwrap_or_else(|_| e.clone());
                let key = canonical.to_string_lossy().to_string();
                if let Some(entry) = graph.get(&key) {
                    if let ModuleNode::Source(ref module) = entry.node {
                        let rel = self.relative_path(&module.path);
                        return rel;
                    }
                }
                key
            })
            .collect();

        // Bundle all modules into a single ESM output
        let mut output = String::new();

        // Add a note about the bundle
        output.push_str(&format!("// Jhol Bundle — {} modules\n", sorted.len()));

        // Generate module code without import/export wrapping for single-module bundles
        if sorted.len() == 1 {
            if let Some(first) = sorted.first() {
                if let Some(content) = transformed.get(first) {
                    output.push_str(content);
                }
            }
        } else {
            // Multi-module: rewrite import paths
            for key in &sorted {
                if let Some(content) = transformed.get(key) {
                    // Extract the relative path for this module
                    let module_rel = if let Some(entry) = graph.get(key) {
                        if let ModuleNode::Source(ref module) = entry.node {
                            self.relative_path(&module.path)
                        } else {
                            key.clone()
                        }
                    } else {
                        key.clone()
                    };

                    // Remove import/export statements and replace with module references
                    let processed = self.rewrite_esm_imports(key, content, graph);
                    output.push_str(&format!("// --- {} ---\n", module_rel));
                    output.push_str(&processed);
                    output.push('\n');
                }
            }
        }

        let entry_name = entries
            .first()
            .cloned()
            .unwrap_or_else(|| "bundle".to_string());
        let out_path = self.config.out_dir.join(format!(
            "{}.mjs",
            entry_name.rsplit('/').next().unwrap_or("bundle")
        ));
        let out_path = out_path.with_extension("mjs");

        Ok(OutputFile {
            path: out_path,
            content: output,
            sourcemap: None,
        })
    }

    fn generate_cjs(
        &self,
        graph: &ModuleGraph,
        transformed: &HashMap<String, String>,
    ) -> BundleResult<OutputFile> {
        let sorted = graph.sorted_modules();
        let mut output = String::new();

        output.push_str(&format!("// Jhol Bundle — {} modules\n", sorted.len()));

        // Resolve a specifier to its bundle module id for the module at `key`.
        // Shared by every path so `require()` handling cannot diverge again.
        let resolve_for = |key: &str, spec: &str| -> Option<String> {
            let node = graph.get(key)?;
            let dep = node
                .deps
                .iter()
                .find(|d| !d.is_external && d.specifier == spec)?;
            let dep_path = dep.path.to_str()?;
            sorted
                .iter()
                .position(|k| k == dep_path)
                .map(|idx| format!("__m{}", idx))
        };

        if self.config.single_file {
            // Generate a CommonJS bundle with module registry
            output.push_str("var __jhol_modules = {};\n");
            output.push_str("var __jhol_cache = {};\n\n");

            for (i, key) in sorted.iter().enumerate() {
                if let Some(content) = transformed.get(key) {
                    let module_id = format!("__m{}", i);
                    let processed = crate::rewrite::rewrite_require_calls(content, &|spec| {
                        resolve_for(key, spec)
                    });
                    output.push_str(&format!(
                        "__jhol_modules['{}'] = function(module, exports, require) {{\n{}\n}};\n\n",
                        module_id, processed
                    ));
                }
            }

            // Generate the main module execution
            output.push_str("// Boot entry module\n");
            output.push_str("(function() {\n");
            output.push_str("  function __require(id) {\n");
            output.push_str("    if (__jhol_cache[id]) return __jhol_cache[id].exports;\n");
            output.push_str("    var module = { exports: {} };\n");
            output.push_str("    __jhol_cache[id] = module;\n");
            output.push_str("    __jhol_modules[id](module, module.exports, __require);\n");
            output.push_str("    return module.exports;\n");
            output.push_str("  }\n");
            output.push_str("  __require('__m0');\n");
            output.push_str("})();\n");
        } else {
            // Simply concatenate
            for key in &sorted {
                if let Some(content) = transformed.get(key) {
                    let processed = crate::rewrite::rewrite_require_calls(content, &|spec| {
                        resolve_for(key, spec)
                    });
                    output.push_str(&processed);
                    output.push('\n');
                }
            }
        }

        let out_path = self.config.out_dir.join("bundle.cjs");

        Ok(OutputFile {
            path: out_path,
            content: output,
            sourcemap: None,
        })
    }

    fn generate_iife(
        &self,
        graph: &ModuleGraph,
        transformed: &HashMap<String, String>,
    ) -> BundleResult<OutputFile> {
        let global_name = self
            .config
            .global_name
            .clone()
            .unwrap_or_else(|| "jholBundle".to_string());

        let mut output = String::new();

        output.push_str(&format!(
            "(function() {{\n'use strict';\n\n// Jhol Bundle — {} modules\n",
            graph.module_count()
        ));

        // Module registry for IIFE
        output.push_str("var __jhol_modules = {};\n");
        output.push_str("var __jhol_cache = {};\n\n");

        let sorted = graph.sorted_modules();
        // Same specifier -> module-id resolver as the CommonJS path, so both
        // generators share one `require()` rewriting implementation.
        let resolve_for = |key: &str, spec: &str| -> Option<String> {
            let node = graph.get(key)?;
            let dep = node
                .deps
                .iter()
                .find(|d| !d.is_external && d.specifier == spec)?;
            let dep_path = dep.path.to_str()?;
            sorted
                .iter()
                .position(|k| k == dep_path)
                .map(|idx| format!("__m{}", idx))
        };
        // The entry is not necessarily `__m0` (modules are emitted in dependency
        // order), so look it up rather than hardcoding the id.
        let mut entry_module_id: Option<String> = None;
        for (i, key) in sorted.iter().enumerate() {
            if graph.get(key).map(|n| n.is_entry).unwrap_or(false) {
                entry_module_id = Some(format!("__m{}", i));
            }
            if let Some(content) = transformed.get(key) {
                let module_id = format!("__m{}", i);
                let processed =
                    crate::rewrite::rewrite_require_calls(content, &|spec| resolve_for(key, spec));
                output.push_str(&format!(
                    "__jhol_modules['{}'] = function(module, exports, require) {{\n{}\n}};\n\n",
                    module_id, processed
                ));
            }
        }

        // Require function
        output.push_str("function __require(id) {\n");
        output.push_str("  if (__jhol_cache[id]) return __jhol_cache[id].exports;\n");
        output.push_str("  var module = { exports: {} };\n");
        output.push_str("  __jhol_cache[id] = module;\n");
        output.push_str("  __jhol_modules[id](module, module.exports, __require);\n");
        output.push_str("  return module.exports;\n");
        output.push_str("}\n\n");

        // Execute entry and expose to global scope
        output.push_str(&format!(
            "var __entry = __require('{}');\n",
            entry_module_id.unwrap_or_else(|| "__m0".to_string())
        ));
        output.push_str(&format!(
            "if (typeof window !== 'undefined') window['{}'] = __entry;\n",
            global_name
        ));
        output.push_str(&format!(
            "if (typeof globalThis !== 'undefined') globalThis['{}'] = __entry;\n",
            global_name
        ));
        output.push_str(&format!("return __entry;\n",));

        output.push_str("})();\n");

        let out_path = self.config.out_dir.join("bundle.js");

        Ok(OutputFile {
            path: out_path,
            content: output,
            sourcemap: None,
        })
    }

    fn rewrite_esm_imports(&self, module_key: &str, source: &str, graph: &ModuleGraph) -> String {
        // Remove import/export statements and replace with module scope references
        let mut result = String::new();
        let mut last_end = 0;

        if let Some(entry) = graph.get(module_key) {
            if let ModuleNode::Source(ref module) = entry.node {
                let mut sorted_imports: Vec<_> = module.imports.clone();
                sorted_imports.sort_by_key(|i| i.start);

                for import in &sorted_imports {
                    // Add content before this import
                    result.push_str(&module.source[last_end..import.start]);
                    last_end = import.end;

                    match import.kind {
                        crate::cas::ImportKind::StaticImport | crate::cas::ImportKind::ReExport => {
                            // Remove import line (the bundler resolves dependencies on its own)
                            result.push_str("");
                        }
                        crate::cas::ImportKind::DynamicImport => {
                            // Keep dynamic imports as-is for code splitting
                            result.push_str(&module.source[import.start..import.end]);
                        }
                        crate::cas::ImportKind::Require => {
                            // Keep require calls as-is for CJS compatibility
                            result.push_str(&module.source[import.start..import.end]);
                        }
                    }
                }
            }
        }

        // Add remaining content after last import
        if let Some(entry) = graph.get(module_key) {
            if let ModuleNode::Source(ref module) = entry.node {
                result.push_str(&module.source[last_end..]);
            }
        }

        result
    }

    fn relative_path(&self, path: &Path) -> String {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let out_dir = std::fs::canonicalize(&self.config.out_dir)
            .unwrap_or_else(|_| self.config.out_dir.clone());

        let mut rel = String::new();
        let mut ca = canonical.components().peekable();
        let mut oa = out_dir.components().peekable();

        while let (Some(c), Some(o)) = (ca.peek(), oa.peek()) {
            if c == o {
                ca.next();
                oa.next();
            } else {
                break;
            }
        }

        for _ in oa {
            rel.push_str("../");
        }

        for c in ca {
            rel.push_str(&c.as_os_str().to_string_lossy());
            rel.push('/');
        }

        if rel.ends_with('/') {
            rel.pop();
        }

        if rel.is_empty() {
            rel.push('.');
        }

        rel
    }
}
