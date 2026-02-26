//! JHOL Optimized Download - Checks global cache and binary packages first
//! 
//! Download priority:
//! 1. Global Cache 2.0 (fastest - already downloaded)
//! 2. Binary packages (fast - pre-built, smaller)
//! 3. Regular download (slowest - from npm registry)

use std::path::{Path, PathBuf};
use std::fs;
use crate::global_cache_2::GlobalCache2;

/// Optimized download that checks caches first
pub fn download_optimized(
    package: &str,
    version: &str,
    url: &str,
    integrity: Option<&str>,
    cache_dir: &Path,
    global_cache: &mut GlobalCache2,
    binary_packages_dir: Option<&Path>,
) -> Result<Vec<u8>, String> {
    // Priority 1: Check global cache
    if let Some(data) = global_cache.get(package, version) {
        return Ok(data);
    }
    
    // Priority 2: Check binary packages
    if let Some(bin_dir) = binary_packages_dir {
        if let Some(data) = load_binary_package(bin_dir, package, version)? {
            // Save to global cache for next time
            let _ = global_cache.add(package, version, &data, vec![]);
            return Ok(data);
        }
    }
    
    // Priority 3: Download from registry
    let data = crate::registry::download_tarball_to_store_hash_only(
        url,
        cache_dir,
        &format!("{}@{}", package, version),
        integrity,
    )?;
    
    // Read the downloaded file
    let hash = data;
    let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
    let data = fs::read(&store_path)
        .map_err(|e| format!("Failed to read downloaded file: {}", e))?;
    
    // Save to global cache for next time
    let _ = global_cache.add(package, version, &data, vec![]);
    
    Ok(data)
}

/// Load binary package from binary-packages directory
fn load_binary_package(
    bin_dir: &Path,
    package: &str,
    version: &str,
) -> Result<Option<Vec<u8>>, String> {
    // Check index.json for package hash
    let index_path = bin_dir.join("index.json");
    if !index_path.exists() {
        return Ok(None);
    }
    
    let index_content = fs::read_to_string(&index_path)
        .map_err(|e| format!("Failed to read binary index: {}", e))?;
    
    let index: serde_json::Value = serde_json::from_str(&index_content)
        .map_err(|e| format!("Failed to parse binary index: {}", e))?;
    
    let key = format!("{}@{}", package, version);
    if let Some(hash) = index.get(&key).and_then(|v| v.as_str()) {
        // Load binary package
        let bin_path = bin_dir.join(format!("{}.jhol", hash));
        if bin_path.exists() {
            let data = fs::read(&bin_path)
                .map_err(|e| format!("Failed to read binary package: {}", e))?;
            
            // Extract compressed content from binary format
            return extract_binary_package(&data);
        }
    }
    
    Ok(None)
}

/// Extract content from binary package format
fn extract_binary_package(data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    use flate2::read::GzDecoder;
    
    // Parse binary package header
    if data.len() < 16 {
        return Ok(None);
    }
    
    // Check magic number "JHOL"
    if &data[0..4] != b"JHOL" {
        return Ok(None);
    }
    
    // Read header fields
    let _version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let pkg_name_len = u16::from_le_bytes([data[8], data[9]]) as usize;
    let version_len = u16::from_le_bytes([data[10], data[11]]) as usize;
    let _content_hash = &data[12..44];  // 32 bytes
    let _original_size = u32::from_le_bytes([data[44], data[45], data[46], data[47]]);
    let compressed_size = u32::from_le_bytes([data[48], data[49], data[50], data[51]]) as usize;
    
    // Skip metadata
    let metadata_start = 52;
    let metadata_end = metadata_start + pkg_name_len + version_len;
    
    // Read compressed content
    let compressed_start = metadata_end;
    let compressed_end = compressed_start + compressed_size;
    
    if compressed_end > data.len() {
        return Ok(None);
    }
    
    // Decompress
    let compressed_data = &data[compressed_start..compressed_end];
    let mut decoder = GzDecoder::new(compressed_data);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress binary package: {}", e))?;
    
    Ok(Some(decompressed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_package_format() {
        // Test binary package parsing
        let data = b"JHOL\x01\x00\x06\x00\x07\x00hashhashhashhashhashhashhash\x00\x00\x00\x00\x0a\x00\x00\x00testpkg1.0.0compressed";
        let result = extract_binary_package(data);
        // Should parse without error (even if decompression fails)
        assert!(result.is_err() || result.unwrap().is_some());
    }
}
