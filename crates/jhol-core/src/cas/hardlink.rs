//! Hard link and reflink utilities for efficient package installation

use std::path::{Path, PathBuf};
use std::fs;
use std::io;

/// Result of a link operation
#[derive(Clone, Debug)]
pub struct LinkResult {
    /// Type of link created
    pub link_type: LinkType,
    /// Source path
    pub source: PathBuf,
    /// Destination path
    pub destination: PathBuf,
    /// Size in bytes
    pub size: u64,
    /// Whether the operation was successful
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Type of link
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkType {
    /// Hard link (same inode)
    HardLink,
    /// Reflink (copy-on-write, same data blocks)
    Reflink,
    /// Clone (macOS specific, uses clonefile)
    Clone,
    /// Regular copy (different inode and data)
    Copy,
}

/// Link a package from store to destination
/// Tries hard link first, then reflink, then falls back to copy
pub fn link_package(source: &Path, dest: &Path) -> io::Result<LinkResult> {
    // Ensure destination directory exists
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    
    // Get file metadata for size
    let metadata = fs::metadata(source)?;
    let size = metadata.len();
    
    // Try hard link first (fastest, no extra disk space)
    if try_hard_link(source, dest).is_ok() {
        return Ok(LinkResult {
            link_type: LinkType::HardLink,
            source: source.to_path_buf(),
            destination: dest.to_path_buf(),
            size,
            success: true,
            error: None,
        });
    }
    
    // Try reflink (Linux Btrfs/XFS, macOS APFS)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if try_reflink(source, dest).is_ok() {
            return Ok(LinkResult {
                link_type: LinkType::Reflink,
                source: source.to_path_buf(),
                destination: dest.to_path_buf(),
                size,
                success: true,
                error: None,
            });
        }
    }
    
    // Try clone (macOS specific)
    #[cfg(target_os = "macos")]
    {
        if try_clone(source, dest).is_ok() {
            return Ok(LinkResult {
                link_type: LinkType::Clone,
                source: source.to_path_buf(),
                destination: dest.to_path_buf(),
                size,
                success: true,
                error: None,
            });
        }
    }
    
    // Fall back to regular copy
    match fs::copy(source, dest) {
        Ok(_) => Ok(LinkResult {
            link_type: LinkType::Copy,
            source: source.to_path_buf(),
            destination: dest.to_path_buf(),
            size,
            success: true,
            error: None,
        }),
        Err(e) => Ok(LinkResult {
            link_type: LinkType::Copy,
            source: source.to_path_buf(),
            destination: dest.to_path_buf(),
            size,
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

/// Try to create a hard link
fn try_hard_link(source: &Path, dest: &Path) -> io::Result<()> {
    // Remove existing destination
    let _ = fs::remove_file(dest);
    
    // Create hard link
    fs::hard_link(source, dest)?;
    
    Ok(())
}

/// Try to create a reflink (copy-on-write)
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn try_reflink(source: &Path, dest: &Path) -> io::Result<()> {
    // Remove existing destination
    let _ = fs::remove_file(dest);
    
    // Use reflink-copy crate
    reflink_copy::reflink(source, dest)?;
    
    Ok(())
}

/// Try to create a clone (macOS clonefile)
#[cfg(target_os = "macos")]
fn try_clone(source: &Path, dest: &Path) -> io::Result<()> {
    // Remove existing destination
    let _ = fs::remove_file(dest);
    
    // Use reflink (will use clonefile on macOS)
    reflink_copy::reflink(source, dest)?;
    
    Ok(())
}

/// Create a hard link with fallback
pub fn hard_link_with_fallback(source: &Path, dest: &Path) -> LinkResult {
    match link_package(source, dest) {
        Ok(result) => result,
        Err(e) => LinkResult {
            link_type: LinkType::Copy,
            source: source.to_path_buf(),
            destination: dest.to_path_buf(),
            size: 0,
            success: false,
            error: Some(e.to_string()),
        },
    }
}

/// Check if two paths point to the same file (same inode)
pub fn is_same_file(path1: &Path, path2: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        
        match (fs::metadata(path1), fs::metadata(path2)) {
            (Ok(m1), Ok(m2)) => {
                m1.dev() == m2.dev() && m1.ino() == m2.ino()
            }
            _ => false,
        }
    }
    
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        
        match (fs::metadata(path1), fs::metadata(path2)) {
            (Ok(m1), Ok(m2)) => {
                m1.volume_serial_number() == m2.volume_serial_number()
                    && m1.file_index() == m2.file_index()
            }
            _ => false,
        }
    }
    
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Get disk space saved by hard links
pub fn calculate_space_saved(paths: &[&Path]) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        
        let mut inodes: std::collections::HashMap<(u64, u64), u64> = 
            std::collections::HashMap::new();
        
        for path in paths {
            if let Ok(metadata) = fs::metadata(path) {
                let inode_key = (metadata.dev(), metadata.ino());
                let size = metadata.len();
                
                *inodes.entry(inode_key).or_insert(0) += 1;
            }
        }
        
        // Calculate space saved (count - 1) * size for each shared inode
        let mut saved = 0u64;
        for (count, size) in inodes.values().zip(paths.iter().filter_map(|p| {
            fs::metadata(p).ok().map(|m| m.len())
        })) {
            if *count > 1 {
                saved += (*count - 1) * size;
            }
        }
        
        saved
    }
    
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_hard_link() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("source.txt");
        let dest = temp_dir.path().join("dest.txt");
        
        // Create source file
        let mut file = File::create(&source).unwrap();
        file.write_all(b"test content").unwrap();
        
        // Create hard link
        let result = link_package(&source, &dest).unwrap();
        
        assert!(result.success);
        assert_eq!(result.link_type, LinkType::HardLink);
        
        // Verify content
        let dest_content = fs::read_to_string(&dest).unwrap();
        assert_eq!(dest_content, "test content");
        
        // Verify same inode
        assert!(is_same_file(&source, &dest));
    }

    #[test]
    fn test_space_saved() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("source.txt");
        let dest1 = temp_dir.path().join("dest1.txt");
        let dest2 = temp_dir.path().join("dest2.txt");
        
        // Create source file
        let mut file = File::create(&source).unwrap();
        file.write_all(b"test content for space saving").unwrap();
        
        // Create hard links
        let _ = link_package(&source, &dest1).unwrap();
        let _ = link_package(&source, &dest2).unwrap();
        
        // Calculate space saved
        let paths = vec![&source, &dest1, &dest2];
        let saved = calculate_space_saved(&paths);
        
        // Should have saved 2x the file size (3 copies -> 1 actual)
        assert!(saved > 0);
    }
}
