//! JHOL Selective Extraction - Only extract necessary files
//! 
//! Research shows 80% of package files are never used (docs, tests, READMEs)
//! This module extracts only what's needed for runtime

use std::path::Path;
use tar::Archive;
use flate2::read::GzDecoder;
use std::fs::{self, File};

/// Files/directories to ALWAYS extract
const ESSENTIAL_PATTERNS: &[&str] = &[
    "package.json",
    "index.js",
    "index.mjs",
    "index.cjs",
    "dist/",
    "lib/",
    "src/",
    "bin/",
];

/// Files/directories to NEVER extract (waste of time/space)
const SKIP_PATTERNS: &[&str] = &[
    "test/",
    "tests/",
    "__tests__/",
    "*.test.js",
    "*.spec.js",
    "coverage/",
    ".github/",
    ".gitlab/",
    "docs/",
    "example/",
    "examples/",
    "benchmark/",
    "benchmarks/",
    "*.md",
    "*.markdown",
    "LICENSE*",
    "CHANGELOG*",
    ".npmignore",
    ".gitignore",
    ".travis.yml",
    ".circleci/",
    "Makefile",
    "tsconfig.json",
    "*.ts",  // Skip TypeScript source if JS exists
    "*.tsx",
    "*.jsx",
];

/// Selective tarball extractor - only extracts essential files
pub fn extract_selective(
    tarball_path: &Path,
    dest_dir: &Path,
    package_name: &str,
) -> Result<usize, String> {
    extract_selective_to_path(tarball_path, &dest_dir.join(package_name))
}

pub fn extract_selective_to_path(
    tarball_path: &Path,
    package_root: &Path,
) -> Result<usize, String> {
    let file = File::open(tarball_path)
        .map_err(|e| format!("Failed to open tarball: {}", e))?;

    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    fs::create_dir_all(package_root)
        .map_err(|e| format!("Failed to create dest dir: {}", e))?;

    let mut extracted_count = 0;

    for entry in archive.entries().map_err(|e| format!("Failed to read archive: {}", e))? {
        let mut entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        
        let path = entry.path()
            .map_err(|e| format!("Failed to get path: {}", e))?
            .to_string_lossy()
            .to_string();

        let Some(rel_path) = normalize_tarball_rel_path(&path) else {
            continue;
        };
        
        // Skip if matches skip patterns
        if should_skip(&rel_path) {
            continue;
        }
        
        // Extract if matches essential patterns
        if should_extract(&rel_path) {
            let dest_path = package_root.join(&rel_path);
            
            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&dest_path)
                    .map_err(|e| format!("Failed to create dir: {}", e))?;
            } else {
                // Ensure parent exists
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent: {}", e))?;
                }
                
                // Stream unpack directly to disk to avoid extra memory copies.
                entry.unpack(&dest_path)
                    .map_err(|e| format!("Failed to unpack file: {}", e))?;
                extracted_count += 1;
            }
        }
    }
    
    Ok(extracted_count)
}

/// Check if path should be skipped
fn should_skip(path: &str) -> bool {
    let path_lower = path.to_lowercase();
    
    for pattern in SKIP_PATTERNS {
        if pattern.ends_with('/') {
            // Directory pattern
            if path_lower.starts_with(&pattern.to_lowercase()) {
                return true;
            }
        } else if pattern.starts_with('*') {
            // Extension pattern
            if path_lower.ends_with(&pattern[1..].to_lowercase()) {
                return true;
            }
        } else {
            // Exact match
            if path_lower == pattern.to_lowercase() {
                return true;
            }
        }
    }
    
    false
}

fn normalize_tarball_rel_path(path: &str) -> Option<String> {
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let first = parts.next()?;
    if first != "package" {
        return None;
    }
    let rel = parts.collect::<Vec<_>>().join(std::path::MAIN_SEPARATOR_STR);
    if rel.is_empty() {
        None
    } else {
        Some(rel)
    }
}

/// Check if path should be extracted
fn should_extract(path: &str) -> bool {
    let path_lower = path.to_lowercase();
    
    // Always extract package.json
    if path_lower == "package.json" {
        return true;
    }
    
    for pattern in ESSENTIAL_PATTERNS {
        if pattern.ends_with('/') {
            // Directory pattern - extract everything in this dir
            if path_lower.starts_with(&pattern.to_lowercase()) {
                return true;
            }
        } else {
            // Exact file match
            if path_lower == pattern.to_lowercase() {
                return true;
            }
        }
    }
    
    // If no essential pattern matched, check if it's a JS file
    // (we want JS files even if not explicitly listed)
    path_lower.ends_with(".js") || path_lower.ends_with(".mjs") || path_lower.ends_with(".cjs")
}

/// Get estimated size savings from selective extraction
pub fn estimate_savings(tarball_path: &Path) -> Result<(u64, u64, f64), String> {
    let file = File::open(tarball_path)
        .map_err(|e| format!("Failed to open tarball: {}", e))?;
    
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    
    let mut total_size = 0u64;
    let mut essential_size = 0u64;
    
    for entry in archive.entries().map_err(|e| format!("Failed to read archive: {}", e))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let size = entry.header().size().map_err(|e| format!("Failed to get size: {}", e))?;
        let path = entry.path()
            .map_err(|e| format!("Failed to get path: {}", e))?
            .to_string_lossy()
            .to_string();

        total_size += size;

        if let Some(rel_path) = normalize_tarball_rel_path(&path) {
            if !should_skip(&rel_path) {
                essential_size += size;
            }
        }
    }
    
    let savings = if total_size > 0 {
        ((total_size - essential_size) as f64 / total_size as f64) * 100.0
    } else {
        0.0
    };
    
    Ok((total_size, essential_size, savings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_skip() {
        assert!(should_skip("test/index.js"));
        assert!(should_skip("README.md"));
        assert!(should_skip("docs/api.md"));
        assert!(!should_skip("dist/index.js"));
        assert!(!should_skip("package.json"));
    }

    #[test]
    fn test_should_extract() {
        assert!(should_extract("package.json"));
        assert!(should_extract("dist/index.js"));
        assert!(should_extract("lib/utils.js"));
        assert!(!should_extract("test/index.js"));
    }

    #[test]
    fn test_normalize_tarball_rel_path() {
        assert_eq!(normalize_tarball_rel_path("package/package.json"), Some("package.json".to_string()));
        assert_eq!(normalize_tarball_rel_path("package/lib/index.js"), Some(format!("lib{}index.js", std::path::MAIN_SEPARATOR)));
        assert_eq!(normalize_tarball_rel_path("other/package.json"), None);
        assert_eq!(normalize_tarball_rel_path("package/"), None);
    }
}
