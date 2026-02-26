//! JHOL Binary Manifest Cache - Like Bun's .npm format
//! 
//! Based on Bun's binary manifest caching technique:
//! - All strings stored in single buffer (deduplication)
//! - Fixed-size structs with offsets (no pointer chasing)
//! - ETag stored for cache validation
//! - 40x faster than JSON parsing
//!
//! Reference: https://bun.com/blog/behind-the-scenes-of-bun-install

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Magic number for binary manifest format
const MANIFEST_MAGIC: u32 = 0x4A484F4C; // "JHOL" in ASCII
const MANIFEST_VERSION: u32 = 1;

/// Binary manifest header
#[derive(Clone, Debug)]
pub struct ManifestHeader {
    pub magic: u32,
    pub version: u32,
    pub string_buffer_size: u32,
    pub package_count: u32,
    pub etag: String,
    pub cached_at: u64,
}

/// Package entry in binary format (fixed size: 32 bytes)
#[derive(Clone, Debug, Default)]
#[repr(C, packed)]
pub struct PackageEntry {
    pub name_offset: u32,    // Offset into string buffer
    pub name_len: u16,
    pub version_offset: u32,
    pub version_len: u16,
    pub tarball_offset: u32,
    pub tarball_len: u16,
    pub integrity_offset: u32,
    pub integrity_len: u16,
    pub dep_count: u16,
    pub dep_offset: u32,     // Offset into dependency array
}

/// Dependency entry (fixed size: 16 bytes)
#[derive(Clone, Debug, Default)]
#[repr(C, packed)]
pub struct DependencyEntry {
    pub name_offset: u32,
    pub name_len: u16,
    pub version_offset: u32,
    pub version_len: u16,
}

/// Binary manifest cache (Structure of Arrays layout)
pub struct BinaryManifest {
    /// Single string buffer (all strings stored once, deduplicated)
    pub string_buffer: Vec<u8>,
    
    /// Package entries (fixed-size array)
    pub packages: Vec<PackageEntry>,
    
    /// Dependency entries (fixed-size array)
    pub dependencies: Vec<DependencyEntry>,
    
    /// Metadata
    pub header: ManifestHeader,
    
    /// Cache file path
    cache_path: PathBuf,
}

impl BinaryManifest {
    /// Create new binary manifest
    pub fn new(cache_dir: &Path) -> Self {
        let cache_path = cache_dir.join("manifest.bin");
        
        Self {
            string_buffer: Vec::new(),
            packages: Vec::new(),
            dependencies: Vec::new(),
            header: ManifestHeader {
                magic: MANIFEST_MAGIC,
                version: MANIFEST_VERSION,
                string_buffer_size: 0,
                package_count: 0,
                etag: String::new(),
                cached_at: 0,
            },
            cache_path,
        }
    }
    
    /// Add string to buffer (with deduplication)
    pub fn add_string(&mut self, s: &str) -> (u32, u16) {
        // Check if string already exists (simple deduplication)
        if let Some(pos) = find_subslice(&self.string_buffer, s.as_bytes()) {
            return (pos as u32, s.len() as u16);
        }
        
        // Add new string
        let offset = self.string_buffer.len() as u32;
        let len = s.len() as u16;
        
        self.string_buffer.extend_from_slice(s.as_bytes());
        self.string_buffer.push(0); // Null terminator
        
        (offset, len)
    }
    
    /// Add package to manifest
    pub fn add_package(
        &mut self,
        name: &str,
        version: &str,
        tarball: &str,
        integrity: &str,
        dependencies: &HashMap<String, String>,
    ) {
        let (name_off, name_len) = self.add_string(name);
        let (ver_off, ver_len) = self.add_string(version);
        let (tar_off, tar_len) = self.add_string(tarball);
        let (int_off, int_len) = self.add_string(integrity);
        
        let dep_offset = self.dependencies.len() as u32;
        
        // Add dependencies
        for (dep_name, dep_ver) in dependencies {
            let (dn_off, dn_len) = self.add_string(dep_name);
            let (dv_off, dv_len) = self.add_string(dep_ver);
            
            self.dependencies.push(DependencyEntry {
                name_offset: dn_off,
                name_len: dn_len,
                version_offset: dv_off,
                version_len: dv_len,
            });
        }
        
        self.packages.push(PackageEntry {
            name_offset: name_off,
            name_len,
            version_offset: ver_off,
            version_len: ver_len,
            tarball_offset: tar_off,
            tarball_len: tar_len,
            integrity_offset: int_off,
            integrity_len: int_len,
            dep_count: dependencies.len() as u16,
            dep_offset: dep_offset,
        });
    }
    
    /// Serialize to binary format
    pub fn serialize(&self) -> Result<Vec<u8>, String> {
        let mut buffer = Vec::new();
        
        // Write header
        buffer.extend_from_slice(&self.header.magic.to_le_bytes());
        buffer.extend_from_slice(&self.header.version.to_le_bytes());
        buffer.extend_from_slice(&(self.string_buffer.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&(self.packages.len() as u32).to_le_bytes());
        
        // Write ETag
        let etag_bytes = self.header.etag.as_bytes();
        buffer.extend_from_slice(&(etag_bytes.len() as u32).to_le_bytes());
        buffer.extend_from_slice(etag_bytes);
        
        // Write timestamp
        buffer.extend_from_slice(&self.header.cached_at.to_le_bytes());
        
        // Write string buffer
        buffer.extend_from_slice(&(self.string_buffer.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&self.string_buffer);
        
        // Write package entries
        for pkg in &self.packages {
            buffer.extend_from_slice(&pkg.name_offset.to_le_bytes());
            buffer.extend_from_slice(&pkg.name_len.to_le_bytes());
            buffer.extend_from_slice(&pkg.version_offset.to_le_bytes());
            buffer.extend_from_slice(&pkg.version_len.to_le_bytes());
            buffer.extend_from_slice(&pkg.tarball_offset.to_le_bytes());
            buffer.extend_from_slice(&pkg.tarball_len.to_le_bytes());
            buffer.extend_from_slice(&pkg.integrity_offset.to_le_bytes());
            buffer.extend_from_slice(&pkg.integrity_len.to_le_bytes());
            buffer.extend_from_slice(&pkg.dep_count.to_le_bytes());
            buffer.extend_from_slice(&pkg.dep_offset.to_le_bytes());
        }
        
        // Write dependency entries
        for dep in &self.dependencies {
            buffer.extend_from_slice(&dep.name_offset.to_le_bytes());
            buffer.extend_from_slice(&dep.name_len.to_le_bytes());
            buffer.extend_from_slice(&dep.version_offset.to_le_bytes());
            buffer.extend_from_slice(&dep.version_len.to_le_bytes());
        }
        
        Ok(buffer)
    }
    
    /// Deserialize from binary format
    pub fn deserialize(data: &[u8]) -> Result<Self, String> {
        let mut pos = 0;
        
        // Read header
        let magic = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        pos += 4;
        
        if magic != MANIFEST_MAGIC {
            return Err("Invalid manifest magic".to_string());
        }
        
        let version = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        pos += 4;
        
        let _string_buffer_size = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        pos += 4;
        
        let package_count = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        pos += 4;
        
        let etag_len = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        pos += 4;
        
        let etag = String::from_utf8_lossy(&data[pos..pos+etag_len as usize]).to_string();
        pos += etag_len as usize;
        
        let cached_at = u64::from_le_bytes([
            data[pos], data[pos+1], data[pos+2], data[pos+3],
            data[pos+4], data[pos+5], data[pos+6], data[pos+7],
        ]);
        pos += 8;
        
        let string_buffer_len = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        pos += 4;
        
        let string_buffer = data[pos..pos+string_buffer_len as usize].to_vec();
        pos += string_buffer_len as usize;
        
        // Read package entries
        let mut packages = Vec::with_capacity(package_count as usize);
        for _ in 0..package_count {
            let pkg = PackageEntry {
                name_offset: u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]),
                name_len: u16::from_le_bytes([data[pos+4], data[pos+5]]),
                version_offset: u32::from_le_bytes([data[pos+6], data[pos+1], data[pos+8], data[pos+9]]),
                version_len: u16::from_le_bytes([data[pos+10], data[pos+11]]),
                tarball_offset: u32::from_le_bytes([data[pos+12], data[pos+13], data[pos+14], data[pos+15]]),
                tarball_len: u16::from_le_bytes([data[pos+16], data[pos+17]]),
                integrity_offset: u32::from_le_bytes([data[pos+18], data[pos+19], data[pos+20], data[pos+21]]),
                integrity_len: u16::from_le_bytes([data[pos+22], data[pos+23]]),
                dep_count: u16::from_le_bytes([data[pos+24], data[pos+25]]),
                dep_offset: u32::from_le_bytes([data[pos+26], data[pos+27], data[pos+28], data[pos+29]]),
            };
            pos += 30;
            packages.push(pkg);
        }
        
        // Read dependency entries
        let remaining_deps = (data.len() - pos) / 8;
        let mut dependencies = Vec::with_capacity(remaining_deps);
        for _ in 0..remaining_deps {
            let dep = DependencyEntry {
                name_offset: u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]),
                name_len: u16::from_le_bytes([data[pos+4], data[pos+5]]),
                version_offset: u32::from_le_bytes([data[pos+6], data[pos+7], data[pos+8], data[pos+9]]),
                version_len: u16::from_le_bytes([data[pos+10], data[pos+11]]),
            };
            pos += 12;
            dependencies.push(dep);
        }
        
        Ok(Self {
            string_buffer,
            packages,
            dependencies,
            header: ManifestHeader {
                magic,
                version,
                string_buffer_size: string_buffer_len,
                package_count,
                etag,
                cached_at,
            },
            cache_path: PathBuf::new(),
        })
    }
    
    /// Save to disk
    pub fn save(&self) -> Result<(), String> {
        let data = self.serialize()?;
        fs::write(&self.cache_path, &data)
            .map_err(|e| format!("Failed to write manifest: {}", e))
    }
    
    /// Load from disk
    pub fn load(cache_dir: &Path) -> Result<Self, String> {
        let cache_path = cache_dir.join("manifest.bin");
        let data = fs::read(&cache_path)
            .map_err(|e| format!("Failed to read manifest: {}", e))?;
        
        let mut manifest = Self::deserialize(&data)?;
        manifest.cache_path = cache_path;
        Ok(manifest)
    }
    
    /// Check if cache is valid using ETag
    pub fn is_valid(&self, remote_etag: &str) -> bool {
        self.header.etag == remote_etag
    }
    
    /// Update ETag
    pub fn set_etag(&mut self, etag: &str) {
        self.header.etag = etag.to_string();
        self.header.cached_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }
    
    /// Get string from buffer
    pub fn get_string(&self, offset: u32, len: u16) -> &str {
        let bytes = &self.string_buffer[offset as usize..(offset + len as u32) as usize];
        std::str::from_utf8(bytes).unwrap_or("")
    }
}

/// Find subslice in buffer (for string deduplication)
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    
    haystack.windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_manifest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut manifest = BinaryManifest::new(temp_dir.path());
        
        let mut deps = HashMap::new();
        deps.insert("lodash".to_string(), "^4.17.0".to_string());
        
        manifest.add_package("test-pkg", "1.0.0", "https://example.com/pkg.tgz", "sha256-abc", &deps);
        manifest.set_etag("W/\"abc123\"");
        
        // Serialize and deserialize
        let data = manifest.serialize().unwrap();
        let loaded = BinaryManifest::deserialize(&data).unwrap();
        
        assert_eq!(loaded.packages.len(), 1);
        assert_eq!(loaded.header.etag, "W/\"abc123\"");
    }
    
    #[test]
    fn test_string_deduplication() {
        let mut manifest = BinaryManifest::new(Path::new("/tmp"));
        
        // Add same string twice
        let (off1, len1) = manifest.add_string("test");
        let (off2, len2) = manifest.add_string("test");
        
        // Should return same offset (deduplication)
        assert_eq!(off1, off2);
        assert_eq!(len1, len2);
    }
}
