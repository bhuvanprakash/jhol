use std::collections::HashSet;
use std::path::Path;

use crate::store::GridStore;

/// Garbage collector for orphaned content blocks in a JGrid file.
///
/// Rebuilds the file containing only blocks referenced by at least one manifest.
/// Orphaned blocks (from uninstalled packages) are dropped.
pub struct GridGc;

impl GridGc {
    /// Rebuild a JGrid, keeping only referenced blocks.
    ///
    /// Returns the number of orphan blocks removed.
    pub fn run(store: &GridStore, output_path: &Path) -> Result<u64, String> {
        let total_blocks = store.header().get_block_count();

        // Collect all hashes referenced by manifests
        let mut referenced = HashSet::new();
        for manifest in store.manifests() {
            for file in &manifest.files {
                referenced.insert(file.content_hash);
            }
        }

        let referenced_count = referenced.len() as u64;
        let removed = total_blocks.saturating_sub(referenced_count);

        if removed == 0 {
            return Ok(0);
        }

        // Rebuild: copy only referenced blocks, keep all file entries
        let mut builder = crate::store::GridBuilder::create(output_path)?;
        for manifest in store.manifests() {
            let mut files = Vec::new();
            for file in &manifest.files {
                let hash = file.content_hash;
                if let Some(content) = store.read_block(&hash) {
                    builder.add_block(content);
                }
                files.push((
                    store.string_table().get(file.path_offset).to_string(),
                    file.mode,
                    hash,
                ));
            }
            builder.add_manifest(&manifest.name, &manifest.version, &files)?;
        }
        builder.finalize()?;

        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GridBuilder;
    use tempfile::tempdir;

    #[test]
    fn test_gc_no_orphans() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("store.jgrid");

        let mut builder = GridBuilder::create(&path).unwrap();
        let h = builder.add_block(b"shared");
        builder
            .add_manifest("pkg", "1.0.0", &[("f.js".to_string(), 0o644, h)])
            .unwrap();
        builder.finalize().unwrap();

        let store = GridStore::open(&path).unwrap();
        let gc_path = dir.path().join("gc.jgrid");
        let removed = GridGc::run(&store, &gc_path).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_gc_removes_orphan() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("store.jgrid");

        let mut builder = GridBuilder::create(&path).unwrap();
        let h_used = builder.add_block(b"used");
        builder.add_block(b"orphan");
        builder
            .add_manifest("pkg", "1.0.0", &[("f.js".to_string(), 0o644, h_used)])
            .unwrap();
        builder.finalize().unwrap();

        let store = GridStore::open(&path).unwrap();
        assert_eq!(store.header().get_block_count(), 2);

        let gc_path = dir.path().join("gc.jgrid");
        let removed = GridGc::run(&store, &gc_path).unwrap();
        assert_eq!(removed, 1);

        let gc_store = GridStore::open(&gc_path).unwrap();
        assert_eq!(gc_store.header().get_block_count(), 1);
    }
}
