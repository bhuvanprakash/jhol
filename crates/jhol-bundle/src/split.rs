use std::collections::{HashMap, HashSet};

use hex;
use sha2::{Digest, Sha256};

use crate::codegen::OutputFile;
use crate::config::BundleConfig;
use crate::errors::BundleResult;
use crate::graph::{ModuleGraph, ModuleNode};

#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: String,
    pub name: String,
    pub modules: Vec<String>,
    pub is_entry: bool,
    pub dependencies: Vec<String>,
    pub content: String,
}

pub fn split_modules(
    graph: &ModuleGraph,
    transformed: &HashMap<String, String>,
    config: &BundleConfig,
) -> BundleResult<Vec<Chunk>> {
    if !config.code_splitting {
        // Single chunk: all modules
        let all_modules: Vec<String> = graph.sorted_modules();
        let mut content = String::new();
        for key in &all_modules {
            if let Some(src) = transformed.get(key) {
                content.push_str(src);
                content.push('\n');
            }
        }

        let hash = compute_hash(&content);
        let name = "bundle".to_string();

        let chunk = Chunk {
            id: name.clone(),
            name: name.clone(),
            modules: all_modules,
            is_entry: true,
            dependencies: Vec::new(),
            content,
        };
        return Ok(vec![chunk]);
    }

    // Code splitting: find dynamic import points and create chunks
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut assigned = HashSet::new();

    // Entry chunk: contains entry points and their static deps
    let mut entry_modules = Vec::new();
    let mut entry_content = String::new();

    let sorted = graph.sorted_modules();
    for key in &sorted {
        if assigned.contains(key) {
            continue;
        }

        if let Some(entry) = graph.get(key) {
            if let ModuleNode::Source(ref module) = entry.node {
                // Check if this module has dynamic imports
                let has_dynamic = module
                    .imports
                    .iter()
                    .any(|i| i.kind == crate::cas::ImportKind::DynamicImport);

                if has_dynamic {
                    // Create a separate chunk for dynamic imports
                    let chunk_id = format!("chunk_{}", chunks.len());
                    let chunk_name = module
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("chunk")
                        .to_string();

                    let chunk_modules = vec![key.clone()];
                    let mut chunk_content = String::new();

                    if let Some(src) = transformed.get(key) {
                        chunk_content.push_str(src);
                    }

                    assigned.insert(key.clone());

                    chunks.push(Chunk {
                        id: chunk_id,
                        name: chunk_name,
                        modules: chunk_modules,
                        is_entry: false,
                        dependencies: vec!["entry".to_string()],
                        content: chunk_content,
                    });
                } else {
                    entry_modules.push(key.clone());
                    if let Some(src) = transformed.get(key) {
                        entry_content.push_str(src);
                        entry_content.push('\n');
                    }
                    assigned.insert(key.clone());
                }
            }
        }
    }

    // Entry chunk first
    let hash = compute_hash(&entry_content);
    let entry_chunk = Chunk {
        id: "entry".to_string(),
        name: format!("entry-{}", &hash[..8]),
        modules: entry_modules,
        is_entry: true,
        dependencies: Vec::new(),
        content: entry_content,
    };
    chunks.insert(0, entry_chunk);

    Ok(chunks)
}

fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

pub fn generate_chunk_filename(chunk: &Chunk, pattern: &str, ext: &str) -> String {
    let hash = compute_hash(&chunk.content);
    let name = &chunk.name;

    pattern
        .replace("[name]", name)
        .replace("[hash]", &hash[..8])
        .replace("[ext]", ext)
}

pub fn write_chunks(chunks: Vec<Chunk>, config: &BundleConfig) -> BundleResult<Vec<OutputFile>> {
    let mut files = Vec::new();

    for chunk in &chunks {
        let ext = match config.format.as_str() {
            "esm" => "mjs",
            "cjs" => "cjs",
            "iife" => "js",
            _ => "js",
        };

        let filename = generate_chunk_filename(chunk, &config.chunk_pattern, ext);
        let out_path = config.out_dir.join(&filename);

        let mut content = String::new();

        if chunk.is_entry {
            // Entry chunk: load other chunks dynamically
            for dep in &chunk.dependencies {
                let dep_chunk = chunks.iter().find(|c| c.id == *dep);
                if let Some(dep_chunk) = dep_chunk {
                    let dep_file = generate_chunk_filename(dep_chunk, &config.chunk_pattern, ext);
                    content.push_str(&format!("import('{}');\n", dep_file));
                }
            }
        }

        // Module loading code
        content.push_str(&format!(
            "// Chunk: {} ({} modules)\n",
            chunk.name,
            chunk.modules.len()
        ));
        content.push_str(&chunk.content);

        files.push(OutputFile {
            path: out_path,
            content,
            sourcemap: None,
        });
    }

    Ok(files)
}
