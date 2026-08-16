use std::collections::HashMap;

use crate::format::SwissTable;
use crate::StoreBuilder;

/// Result of ingesting a tarball
pub struct IngestResult {
    pub name: String,
    pub version: String,
    pub files: Vec<(String, u16, [u8; 32])>,
    pub hashes: HashMap<String, [u8; 32]>,
}

/// Ingest raw tarball bytes into content blocks.
/// Returns the list of (path, mode, content_hash) for the package manifest.
///
/// This function handles the raw extraction logic without depending on
/// external tar/gzip crates at this boundary — it accepts pre-decompressed
/// tar entries or raw bytes and splits them into individual content blocks.
pub fn ingest_raw_files(
    name: &str,
    version: &str,
    file_entries: &[(&str, u16, &[u8])],
    builder: &mut StoreBuilder,
) -> Result<IngestResult, String> {
    let mut files = Vec::with_capacity(file_entries.len());
    let mut hashes = HashMap::new();

    for (path, mode, content) in file_entries {
        let hash = builder.add_block(content);
        files.push((path.to_string(), *mode, hash));
        hashes.insert(path.to_string(), hash);
    }

    builder.add_manifest(
        name,
        version,
        &files
            .iter()
            .map(|(p, m, h)| (p.clone(), *m, *h))
            .collect::<Vec<_>>(),
    )?;

    Ok(IngestResult {
        name: name.to_string(),
        version: version.to_string(),
        files,
        hashes,
    })
}

/// Compute SHA-256 hash of data
pub fn hash_content(data: &[u8]) -> [u8; 32] {
    SwissTable::compute_hash(data)
}

/// Verify that content matches the given SHA-256 hash
pub fn verify_content(data: &[u8], expected_hash: &[u8; 32]) -> bool {
    &hash_content(data) == expected_hash
}

/// Process tarball entries: extract (path, mode, decompressed_bytes) tuples
/// from a raw .tgz file. Handles gzip decompression and tar extraction.
///
/// This is a placeholder for the actual tar parsing — the pipeline crate
/// will delegate to flate2 + tar from jhol-core at integration time.
pub fn process_tarball_bytes(tarball_bytes: &[u8]) -> Result<Vec<(String, u16, Vec<u8>)>, String> {
    // Decompress gzip
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tar::Archive;

    let mut decoder = GzDecoder::new(tarball_bytes);
    let mut decompressed = Vec::with_capacity(tarball_bytes.len() * 3);
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("gzip decompress: {e}"))?;

    let mut archive = Archive::new(std::io::Cursor::new(decompressed));
    let mut entries = Vec::new();

    for entry in archive.entries().map_err(|e| format!("tar entries: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar entry: {e}"))?;

        let path = entry
            .path()
            .map_err(|e| format!("tar path: {e}"))?
            .to_string_lossy()
            .to_string();

        // Skip directory entries and special files
        let entry_type = entry.header().entry_type();
        if entry_type != tar::EntryType::Regular && entry_type != tar::EntryType::Symlink {
            continue;
        }

        let mode = entry.header().mode().unwrap_or(0o644);

        if entry_type == tar::EntryType::Symlink {
            let target = entry
                .link_name()
                .map_err(|e| format!("link target: {e}"))?
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            entries.push((path, (mode | 0o120000) as u16, target.into_bytes()));
        } else {
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|e| format!("read entry: {e}"))?;
            entries.push((path, mode as u16, data));
        }
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_ingest_raw_files() {
        let dir = tempfile::tempdir().unwrap();
        let grid_path = dir.path().join("test.jgrid");
        let mut builder = crate::StoreBuilder::create(&grid_path).unwrap();

        let files = vec![
            ("index.js", 0o644, b"console.log('hello')" as &[u8]),
            ("package.json", 0o644, b"{\"name\":\"test\"}"),
        ];

        let result = ingest_raw_files("test-pkg", "1.0.0", &files, &mut builder).unwrap();
        assert_eq!(result.name, "test-pkg");
        assert_eq!(result.files.len(), 2);
        assert!(result.hashes.contains_key("index.js"));
        assert!(result.hashes.contains_key("package.json"));

        builder.finalize().unwrap();
    }

    #[test]
    fn test_hash_content() {
        let data = b"hello world";
        let hash = hash_content(data);
        assert!(hash != [0u8; 32]);
        assert!(verify_content(data, &hash));
        assert!(!verify_content(b"wrong", &hash));
    }
}
