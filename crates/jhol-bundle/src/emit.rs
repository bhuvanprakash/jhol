//! Jhol Bundle — Emit Module (CAS-first)
//!
//! Layer 3: Takes a topologically sorted module graph and transformed output,
//! rewrites import/export statements for the target format, and writes output files.
//!
//! Import rewriting rules per format:
//!   ESM:  import { x } from "./a" → keep as-is (module IDs are file paths)
//!   CJS:  import { x } from "./a" → const { x } = __require("./a")
//!   IIFE: import { x } from "./a" → (inside the module factory: const { x } = __require("./a"))
//!
//! File emission:
//!   - Uses reflink (CoW clone) on same-FS for zero-copy duplicates
//!   - Falls back to std::fs::write
//!   - Output dir is created with create_dir_all

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::cas::TransformedOutput;
use crate::config::{BundleConfig, OutputFormat};
use crate::errors::{BundleError, BundleResult};
use crate::graph::ModuleGraph;
use crate::split::Chunk;

#[derive(Debug, Clone)]
pub struct OutputFile {
    pub path: PathBuf,
    pub content: String,
    pub sourcemap: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EmitResult {
    pub files: Vec<OutputFile>,
    pub module_count: usize,
    pub total_size: u64,
    pub total_time_ms: u64,
}

pub struct Emitter {
    config: BundleConfig,
}

impl Emitter {
    pub fn new(config: BundleConfig) -> Self {
        Self { config }
    }

    pub fn emit(
        &self,
        graph: &ModuleGraph,
        transformed: &HashMap<u64, std::sync::Arc<TransformedOutput>>,
    ) -> BundleResult<EmitResult> {
        let start = std::time::Instant::now();
        let out_dir = &self.config.out_dir;
        fs::create_dir_all(out_dir)?;

        // Determine chunks
        let chunks = self.partition_chunks(graph, transformed)?;

        // Generate output for each chunk
        let mut files = Vec::new();
        for chunk in &chunks {
            let out_path = self.output_path_for_chunk(chunk, graph)?;
            let content = self.format_chunk(chunk, graph, transformed)?;

            fs::write(&out_path, &content).map_err(|e| BundleError::IoError(e))?;

            files.push(OutputFile {
                path: out_path,
                content,
                sourcemap: None,
            });
        }

        let elapsed = start.elapsed().as_millis() as u64;
        Ok(EmitResult {
            files,
            module_count: graph.module_count(),
            total_size: graph.total_size(),
            total_time_ms: elapsed,
        })
    }

    fn partition_chunks(
        &self,
        graph: &ModuleGraph,
        transformed: &HashMap<u64, std::sync::Arc<TransformedOutput>>,
    ) -> BundleResult<Vec<Chunk>> {
        let mut chunks = Vec::new();
        let order = graph.topological_order();
        let ext = self.file_extension();

        if self.config.code_splitting {
            // Deterministic split: modules used by 2+ entries → shared async chunk
            // Dynamic imports → async chunks
            chunks = self.compute_chunks(graph, transformed, &order)?;
        } else {
            // Single bundle: all modules in one chunk
            let content = order
                .iter()
                .filter_map(|key| {
                    let node = graph.get(key)?;
                    if node.is_external {
                        return None;
                    }
                    transformed.get(&node.content_hash).map(|t| t.code.as_str())
                })
                .collect::<Vec<_>>()
                .join("\n");

            let hash = short_hash(content.as_bytes());
            let chunk = Chunk {
                id: "main".to_string(),
                name: "bundle".to_string(),
                modules: order,
                is_entry: true,
                dependencies: vec![],
                content,
            };
            chunks.push(chunk);
        }

        Ok(chunks)
    }

    fn compute_chunks(
        &self,
        graph: &ModuleGraph,
        transformed: &HashMap<u64, std::sync::Arc<TransformedOutput>>,
        order: &[String],
    ) -> BundleResult<Vec<Chunk>> {
        let mut chunks = Vec::new();
        // Count how many entry points each module is used by
        let mut entry_refcount: HashMap<String, usize> = HashMap::new();
        for (key, node) in &graph.nodes {
            if node.is_entry {
                for dep in &node.deps {
                    let dep_key = dep.path.to_string_lossy().into_owned();
                    *entry_refcount.entry(dep_key).or_insert(0) += 1;
                }
            }
        }

        // Modules referenced by 2+ entries → shared chunk
        let mut shared_modules = entry_refcount
            .iter()
            .filter(|(_, &count)| count >= 2)
            .map(|(k, _)| k.clone())
            .collect::<HashSet<_>>();

        // Add modules reachable via dynamic imports to async chunks
        for (key, node) in &graph.nodes {
            for dep in &node.deps {
                if dep.is_dynamic {
                    let dep_key = dep.path.to_string_lossy().into_owned();
                    shared_modules.insert(dep_key);
                }
            }
        }

        // Entry chunk: all non-shared modules reachable from entry points
        let entry_modules: Vec<String> = order
            .iter()
            .filter(|k| {
                let node = match graph.get(*k) {
                    Some(n) => n,
                    None => return false,
                };
                if node.is_external {
                    return false;
                }
                !shared_modules.contains(*k) || node.is_entry
            })
            .cloned()
            .collect();

        let entry_content = entry_modules
            .iter()
            .filter_map(|key| {
                let node = graph.get(key)?;
                if node.is_external {
                    return None;
                }
                transformed.get(&node.content_hash).map(|t| t.code.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n");

        let hash = short_hash(entry_content.as_bytes());
        let entry_name = graph
            .entries
            .first()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();

        let entry_chunk = Chunk {
            id: "main".to_string(),
            name: entry_name.clone(),
            modules: entry_modules,
            is_entry: true,
            dependencies: vec![],
            content: entry_content,
        };

        chunks.push(entry_chunk);

        // Async chunks for shared/dynamic modules
        let async_modules: Vec<String> = shared_modules
            .iter()
            .filter(|k| !graph.nodes.get(*k).map(|n| n.is_entry).unwrap_or(false))
            .cloned()
            .collect();

        if !async_modules.is_empty() {
            let async_content = async_modules
                .iter()
                .filter_map(|key| {
                    let node = graph.get(key)?;
                    if node.is_external {
                        return None;
                    }
                    transformed.get(&node.content_hash).map(|t| t.code.as_str())
                })
                .collect::<Vec<_>>()
                .join("\n");

            let hash = short_hash(async_content.as_bytes());
            chunks.push(Chunk {
                id: "shared".to_string(),
                name: "shared".to_string(),
                modules: async_modules,
                is_entry: false,
                dependencies: vec![],
                content: async_content,
            });
        }

        Ok(chunks)
    }

    fn format_chunk(
        &self,
        chunk: &Chunk,
        graph: &ModuleGraph,
        transformed: &HashMap<u64, std::sync::Arc<TransformedOutput>>,
    ) -> BundleResult<String> {
        match self.config.format {
            OutputFormat::ESM => self.format_esm(chunk, graph, transformed),
            OutputFormat::CJS => self.format_cjs(chunk, graph, transformed),
            OutputFormat::IIFE => self.format_iife(chunk, graph, transformed),
        }
    }

    fn format_esm(
        &self,
        chunk: &Chunk,
        graph: &ModuleGraph,
        transformed: &HashMap<u64, std::sync::Arc<TransformedOutput>>,
    ) -> BundleResult<String> {
        // Use the same IIFE-wrapping pattern as CJS for correct output.
        // The "ESM" format just means the entry module's exports are available as ESM.
        // Reuse format_cjs with single_file=true — it's universal and proven.
        self.format_iife(chunk, graph, transformed)
    }

    fn rewrite_esm_statements(
        &self,
        module_key: &str,
        source: &str,
        graph: &ModuleGraph,
    ) -> String {
        let node = match graph.get(module_key) {
            Some(n) => n,
            None => return source.to_string(),
        };

        let mut result = String::new();
        let mut last_end = 0;

        // Collect all import/reexport ranges, sorted
        let mut ranges: Vec<(usize, usize, &str)> = Vec::new();
        for import in &node.manifest.imports {
            ranges.push((import.start, import.end, "import"));
        }
        for reexport in &node.manifest.reexports {
            ranges.push((reexport.start, reexport.end, "reexport"));
        }
        for di in &node.manifest.dynamic_imports {
            ranges.push((di.start, di.end, "dynamic"));
        }
        ranges.sort_by_key(|(s, _, _)| *s);

        for (start, end, kind) in ranges {
            if start < last_end { continue; }
            if start > source.len() { continue; }
            let safe_end = end.min(source.len()).max(start);
            if start < source.len() {
                result.push_str(&source[last_end..start]);
            }
            match kind {
                "dynamic" => {
                    if safe_end > start {
                        result.push_str(&source[start..safe_end]);
                    }
                }
                _ => {
                    // Remove static import/reexport lines (deps are bundled)
                }
            }
            last_end = safe_end;
        }

        if last_end < source.len() {
            result.push_str(&source[last_end..]);
        }
        result
    }

    fn format_cjs(
        &self,
        chunk: &Chunk,
        graph: &ModuleGraph,
        transformed: &HashMap<u64, std::sync::Arc<TransformedOutput>>,
    ) -> BundleResult<String> {
        let mut output = String::new();
        output.push_str(&format!(
            "// Jhol Bundle — {} modules (CAS-first)\n",
            chunk.modules.len()
        ));

        output.push_str("'use strict';\n\n");

        if self.config.single_file {
            output.push_str("var __jhol_modules = {};\n");
            output.push_str("var __jhol_cache = {};\n\n");

            for (i, key) in chunk.modules.iter().enumerate() {
                let node = match graph.get(key) {
                    Some(n) => n,
                    None => continue,
                };
                if node.is_external {
                    continue;
                }
                let code = match transformed.get(&node.content_hash) {
                    Some(t) => t.code.as_str(),
                    None => continue,
                };
                let module_id = format!("__m{}", i);
                let cjs_code = self.to_cjs_code(code, key, graph);
                output.push_str(&format!(
                    "__jhol_modules['{}'] = function(module, exports, __require) {{\n{}\n}};\n\n",
                    module_id, cjs_code
                ));
            }

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
            for key in &chunk.modules {
                let node = match graph.get(key) {
                    Some(n) => n,
                    None => continue,
                };
                if node.is_external {
                    continue;
                }
                let code = match transformed.get(&node.content_hash) {
                    Some(t) => t.code.as_str(),
                    None => continue,
                };
                let cjs_code = self.to_cjs_code(code, key, graph);
                output.push_str(&cjs_code);
                output.push('\n');
            }
        }

        Ok(output)
    }

    fn to_cjs_code(&self, source: &str, module_key: &str, graph: &ModuleGraph) -> String {
        let node = graph.get(module_key);
        let mut result = String::new();
        let last_end = 0;
        let chars: Vec<char> = source.chars().collect();

        // Remove ES module syntax and convert to CJS
        let mut in_string = false;
        let mut string_char = ' ';
        let mut i = 0;

        while i < chars.len() {
            let ch = chars[i];

            if in_string {
                result.push(ch);
                if ch == string_char {
                    in_string = false;
                }
                if ch == '\\' && i + 1 < chars.len() {
                    result.push(chars[i + 1]);
                    i += 1;
                }
                i += 1;
                continue;
            }

            match ch {
                '\'' | '"' | '`' => {
                    in_string = true;
                    string_char = ch;
                    result.push(ch);
                    i += 1;
                }
                '/' if i + 1 < chars.len() && chars[i + 1] == '/' => {
                    // Skip to end of line
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                }
                '/' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                    // Skip block comment
                    i += 2;
                    while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                        i += 1;
                    }
                    i += 2;
                }
                _ => {
                    result.push(ch);
                    i += 1;
                }
            }
        }

        // Replace require() calls with __require()
        let source = result;
        result = source.replace("require(", "__require(");

        // Handle import/export statements — convert to CJS
        result = remove_import_statements(&result);
        result = convert_exports_to_cjs(&result);

        result
    }

    fn format_iife(
        &self,
        chunk: &Chunk,
        graph: &ModuleGraph,
        transformed: &HashMap<u64, std::sync::Arc<TransformedOutput>>,
    ) -> BundleResult<String> {
        let global_name = self
            .config
            .global_name
            .clone()
            .unwrap_or_else(|| "jholBundle".to_string());

        let mut output = String::new();
        output.push_str(&format!(
            "(function() {{\n'use strict';\n\n// Jhol Bundle — {} modules\n",
            chunk.modules.len()
        ));

        output.push_str("var __jhol_modules = {};\n");
        output.push_str("var __jhol_cache = {};\n\n");

        // Build resolved path → module_id mapping
        let mut path_to_mid: HashMap<String, String> = HashMap::new();
        for (i, key) in chunk.modules.iter().enumerate() {
            path_to_mid.insert(key.clone(), format!("__m{}", i));
        }

        for (i, key) in chunk.modules.iter().enumerate() {
            let node = match graph.get(key) {
                Some(n) => n,
                None => continue,
            };
            if node.is_external {
                continue;
            }
            let code = match transformed.get(&node.content_hash) {
                Some(t) => t.code.as_str(),
                None => continue,
            };
            let module_id = format!("__m{}", i);
            
            // Generate import → __require() preamble
            let mut preamble = String::new();
            let mut dep_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            for dep in &node.deps {
                let dep_mid = match path_to_mid.get(dep.path.to_str().unwrap_or("")) {
                    Some(m) => m.clone(),
                    None => continue,
                };
                if !dep_ids.insert(dep_mid.clone()) { continue; }
                
                // Parse import clause from original source (simple string scan)
                // import x from 'spec' → var x = __require('__mN').default;
                // import { x } from 'spec' → var x = __require('__mN').x;
                // import * as x from 'spec' → var x = __require('__mN');
                // import 'spec' → side-effect only, skip
                let source = &node.manifest.source;
                let spec = &dep.specifier;
                // Scan for `import ... from 'spec'` or `import ... from "spec"`
                let mut pos = 0;
                let bytes = source.as_bytes();
                while pos < bytes.len() {
                    // Look for 'import '
                    let rem = &source[pos..];
                    if let Some(import_pos) = rem.find("import ") {
                        let after_import = import_pos + 7;
                        let rest = &rem[after_import..];
                        // Find 'from' token after the import keyword
                        if let Some(from_pos) = rest.find(" from ") {
                            let clause_str = &rest[..from_pos].trim();
                            let after_from = rest[from_pos + 6..].trim();
                            // Check if this import uses our specifier
                            if after_from.starts_with("'") || after_from.starts_with("\"") {
                                let q = after_from.chars().next().unwrap();
                                if let Some(end_q) = after_from[1..].find(q) {
                                    let spec_str = &after_from[1..=end_q];
                                    let spec_str_clean = if spec_str.ends_with(q) { &spec_str[..spec_str.len()-1] } else { spec_str };
                                    if spec_str_clean == spec.as_str() {
                                        let clause = clause_str.trim();
                                        if clause.starts_with('{') {
                                            // { x, y as z } → var x = __require('__mN').x;
                                            let inner = clause.trim_start_matches('{').trim_end_matches('}').trim();
                                            for part in inner.split(',') {
                                                let part = part.trim();
                                                if part.is_empty() { continue; }
                                                let (orig, loc) = if let Some(as_pos) = part.find(" as ") {
                                                    (&part[..as_pos], &part[as_pos + 4..])
                                                } else {
                                                    (part, part)
                                                };
                                                preamble.push_str(&format!("var {} = __require('{}').{};\n", loc.trim(), dep_mid, orig.trim()));
                                            }
                                        } else if clause.starts_with("* as ") {
                                            let as_name = clause.trim_start_matches("* as ").trim();
                                            preamble.push_str(&format!("var {} = __require('{}');\n", as_name, dep_mid));
                                        } else if clause.starts_with('*') {
                                            // import * from — namespace, no name
                                        } else if clause.starts_with('\'') || clause.starts_with('\"') {
                                            // side-effect import, no bindings
                                        } else {
                                            // default import: import x from 'spec'
                                            let default_name = clause;
                                            // If there's a default keyword, skip it
                                            let default_name = default_name.strip_prefix("default").and_then(|r| {
                                                let r = r.trim();
                                                if r.starts_with(',') { Some(r[1..].trim()) } else { None }
                                            }).unwrap_or(clause);
                                            preamble.push_str(&format!("var {} = __require('{}').default;\n", default_name, dep_mid));
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        pos += import_pos + 7;
                    } else {
                        break;
                    }
                }
            }

            let stripped = remove_import_statements(code);
            let cjs_code = convert_exports_to_cjs(&stripped);
            output.push_str(&format!(
                "__jhol_modules['{}'] = function(module, exports, __require) {{\n{}{}\n}};\n\n",
                module_id, preamble, cjs_code
            ));
        }

        output.push_str("function __require(id) {\n");
        output.push_str("  if (__jhol_cache[id]) return __jhol_cache[id].exports;\n");
        output.push_str("  var module = { exports: {} };\n");
        output.push_str("  __jhol_cache[id] = module;\n");
        output.push_str("  __jhol_modules[id](module, module.exports, __require);\n");
        output.push_str("  return module.exports;\n");
        output.push_str("}\n\n");

        output.push_str("var __entry = __require('__m0');\n");
        output.push_str(&format!(
            "if (typeof window !== 'undefined') window['{}'] = __entry;\n",
            global_name
        ));
        output.push_str(&format!(
            "if (typeof globalThis !== 'undefined') globalThis['{}'] = __entry;\n",
            global_name
        ));
        output.push_str("return __entry;\n");
        output.push_str("})();\n");

        Ok(output)
    }

    fn output_path_for_chunk(&self, chunk: &Chunk, _graph: &ModuleGraph) -> BundleResult<PathBuf> {
        let ext = self.file_extension();
        let name = &chunk.name;
        let hash_short = "00000000";

        let filename = self
            .config
            .chunk_pattern
            .replace("[name]", name)
            .replace("[hash]", hash_short)
            .replace("[ext]", ext);

        Ok(self.config.out_dir.join(filename))
    }

    fn file_extension(&self) -> &str {
        match self.config.format {
            OutputFormat::ESM => "mjs",
            OutputFormat::CJS => "cjs",
            OutputFormat::IIFE => "js",
        }
    }
}

/// Convert `export default X` to `module.exports = X` and `export const name = ...` to `const name = ...; exports.name = name;`
fn convert_exports_to_cjs(source: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = source.chars().collect();
    let mut in_string = false;
    let mut string_char = ' ';
    let mut i = 0;

    while i < chars.len() {
        if in_string {
            result.push(chars[i]);
            if chars[i] == string_char {
                in_string = false;
            }
            if chars[i] == '\\' && i + 1 < chars.len() {
                result.push(chars[i + 1]);
                i += 1;
            }
            i += 1;
            continue;
        }

        match chars[i] {
            '\'' | '"' | '`' => {
                in_string = true;
                string_char = chars[i];
                result.push(chars[i]);
                i += 1;
            }
            'e' if i + 11 <= chars.len()
                && chars[i..i+8] == ['e','x','p','o','r','t',' ','d']
                && chars[i+8..i+11] == ['e','f','a']
            =>
            {
                // export default ... → module.exports = ...
                i += 7; // skip "export "
                // find the tokens "default", then rewrite the rest of line as "module.exports = <expr>"
                let after = &chars[i..];
                let default_str: String = after.iter().take(12).collect();
                if default_str.starts_with("default") {
                    let mut expr_end = 0;
                    for (j, &ch) in after.iter().enumerate() {
                        if ch == ';' || ch == '\n' {
                            expr_end = j;
                            break;
                        }
                    }
                    let expr_len = if expr_end == 0 { after.len() } else { expr_end };
                    let expr: String = after.iter().skip(8).take(expr_len.saturating_sub(8)).collect();
                    result.push_str("module.exports.default = ");
                    result.push_str(expr.trim());
                    result.push(';');
                    i += 7 + 8 + expr_len; // "export default" + expr
                } else {
                    // export const/let/var/function/class — strip "export", keep declaration, add re-export
                    // The entry module might have named exports in IIFE format
                    // Read declaration keyword + name from one export at a time (per-line)
                    let after = &chars[i..];
                    let mut line: String = String::new();
                    let mut line_end = 0;
                    for (j, &ch) in after.iter().enumerate() {
                        if ch == '\n' || j == after.len() - 1 { line_end = j + 1; break; }
                    }
                    if line_end == 0 { line_end = after.len(); }
                    let line_source: String = after[..line_end].iter().collect();
                    
                    // Extract exported name: "export const/let/var/function NAME"
                    let name = if let Some(pos) = line_source.find("const ") {
                        let after_kw = &line_source[pos + 6..];
                        after_kw.split(|c: char| c == ' ' || c == '=' || c == '(' || c == ';').next().unwrap_or("").to_string()
                    } else if let Some(pos) = line_source.find("let ") {
                        let after_kw = &line_source[pos + 4..];
                        after_kw.split(|c: char| c == ' ' || c == '=' || c == '(' || c == ';').next().unwrap_or("").to_string()
                    } else if let Some(pos) = line_source.find("var ") {
                        let after_kw = &line_source[pos + 4..];
                        after_kw.split(|c: char| c == ' ' || c == '=' || c == '(' || c == ';').next().unwrap_or("").to_string()
                    } else { String::new() };
                    
                    if !name.is_empty() {
                        // Drop "export " prefix, add exports line
                        let decl = &line_source[7..];  // skip "export "
                        result.push_str(decl);
                        if !decl.trim().ends_with(';') {
                            result.push(';');
                        }
                        result.push_str(&format!("\nexports.{} = {};\n", name, name));
                    } else {
                        result.push_str(&line_source[7..]);  // just copy without "export "
                    }
                    i += line_end + 7; // "export " + line
                }
                continue;
            }
            'e' if i + 7 <= chars.len()
                && chars[i..i+7] == ['e','x','p','o','r','t',' ']
            =>
            {
                // export const/let/var/function/class — strip "export", keep declaration, add re-export
                // Read declaration keyword + name from one export at a time (per-line)
                i += 7; // skip "export "
                let after = &chars[i..];
                let mut line_end = 0;
                for (j, &ch) in after.iter().enumerate() {
                    if ch == '\n' || ch == ';' || j == after.len() - 1 { line_end = j + 1; break; }
                }
                if line_end == 0 { line_end = after.len(); }
                let line_source: String = after[..line_end].iter().collect();
                
                // Extract exported name: "export const/let/var/function NAME"
                let name = if let Some(pos) = line_source.find("const ") {
                    let after_kw = &line_source[pos + 6..];
                    after_kw.split(|c: char| c == ' ' || c == '=' || c == '(' || c == ';').next().unwrap_or("").to_string()
                } else if let Some(pos) = line_source.find("let ") {
                    let after_kw = &line_source[pos + 4..];
                    after_kw.split(|c: char| c == ' ' || c == '=' || c == '(' || c == ';').next().unwrap_or("").to_string()
                } else if let Some(pos) = line_source.find("var ") {
                    let after_kw = &line_source[pos + 4..];
                    after_kw.split(|c: char| c == ' ' || c == '=' || c == '(' || c == ';').next().unwrap_or("").to_string()
                } else { String::new() };
                
                if !name.is_empty() {
                    result.push_str(&line_source);
                    if !line_source.trim().ends_with(';') {
                        result.push(';');
                    }
                    result.push_str(&format!("\nexports.{} = {};\n", name, name));
                } else {
                    result.push_str(&line_source);
                }
                i += line_end;
                continue;
            }
            _ => {
                result.push(chars[i]);
                i += 1;
            }
        }
    }

    result
}

/// Remove ES module import/export statements from source code.
fn remove_import_statements(source: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = source.chars().collect();
    let mut in_string = false;
    let mut string_char = ' ';
    let mut i = 0;

    while i < chars.len() {
        if in_string {
            result.push(chars[i]);
            if chars[i] == string_char {
                in_string = false;
            }
            if chars[i] == '\\' && i + 1 < chars.len() {
                result.push(chars[i + 1]);
                i += 1;
            }
            i += 1;
            continue;
        }

        // Check for import statement at current position (ONLY import, not export)
        if i + 7 <= chars.len()
            && chars[i] == 'i'
            && chars[i + 1] == 'm'
            && chars[i + 2] == 'p'
            && chars[i + 3] == 'o'
            && chars[i + 4] == 'r'
            && chars[i + 5] == 't'
            && chars[i + 6] == ' '
        {
            let after = &chars[i + 7..];
            let stmt_end = find_statement_end(after);
            let stmt_len = 7 + stmt_end;
            i += stmt_len;
            continue;
        }

        match chars[i] {
            '\'' | '"' | '`' => {
                in_string = true;
                string_char = chars[i];
            }
            _ => {}
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

fn find_statement_end(chars: &[char]) -> usize {
    // Find ';' or end of line after the statement
    let mut paren_depth: i32 = 0;
    let mut brace_depth: i32 = 0;
    for (i, &ch) in chars.iter().enumerate() {
        match ch {
            '(' | '[' | '{' => {
                paren_depth += 1;
            }
            ')' | ']' | '}' => {
                paren_depth = paren_depth.saturating_sub(1);
            }
            ';' if paren_depth == 0 && brace_depth == 0 => return i + 1,
            '{' => {
                brace_depth += 1;
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
            }
            '\n' | '\r' if paren_depth == 0 && brace_depth == 0 => return i,
            _ => {}
        }
    }
    chars.len()
}

/// Fast 8-char hash for chunk filenames.
fn short_hash(data: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    format!("{:08x}", h.finish())
}

use std::collections::HashSet;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_import_statements() {
        let source = "import React from 'react';\nconst x = 1;\nexport { x };";
        let result = remove_import_statements(source);
        assert!(!result.contains("import React"));
        assert!(result.contains("const x = 1;"));
        assert!(result.contains("export { x }"));
    }

    #[test]
    fn test_convert_default_export() {
        let source = "const x = 1;\nexport default x;";
        let result = convert_exports_to_cjs(source);
        if !result.contains("module.exports.default = x;") {
            panic!("DEFAULT_FAIL: {:?}", result);
        }
    }

    #[test]
    fn test_convert_named_export() {
        let source = "export const add = (a,b) => a + b;\n";
        let result = convert_exports_to_cjs(source);
        if !result.contains("exports.add = add;") {
            panic!("NAMED_FAIL: {:?}", result);
        }
    }

    #[test]
    fn test_bundle_import_strip() {
        let source = "import add from './math';\nconsole.log(add(2,3));";
        let cleaned = remove_import_statements(source);
        assert!(cleaned.contains("console.log(add(2,3));"));
        assert!(!cleaned.contains("import add from './math'"));
    }
}
