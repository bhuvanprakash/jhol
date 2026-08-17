use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::format::{GridHeader, Manifest, StringTable, SwissTable, BLOCK_ALIGN, HEADER_SIZE};

/// Incremental builder for JGrid files
pub struct GridBuilder {
    #[allow(dead_code)]
    path: PathBuf,
    file: File,
    header: GridHeader,
    string_table: StringTable,
    manifests: Vec<Manifest>,
    #[allow(dead_code)]
    swiss_table: Option<SwissTable>,
    block_data: Vec<u8>,
    block_entries: Vec<([u8; 32], u64, u32)>, // (hash, offset, size)

    #[allow(dead_code)]
    pending_strings: Vec<String>,
}

impl GridBuilder {
    /// Create a new builder that will write to the given path
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(false)
            .open(path)
            .map_err(|e| format!("create builder: {e}"))?;

        // Write placeholder header (will be overwritten at finalize)
        let header = GridHeader::new();
        let header_bytes = vec![0u8; HEADER_SIZE];
        (&file)
            .write_all(&header_bytes)
            .map_err(|e| format!("placeholder: {e}"))?;

        Ok(Self {
            path: path.to_path_buf(),
            file,
            header,
            string_table: StringTable::new(),
            manifests: Vec::new(),
            swiss_table: None,
            block_data: Vec::new(),
            block_entries: Vec::new(),
            pending_strings: Vec::new(),
        })
    }

    /// Add a file block to the store
    pub fn add_block(&mut self, content: &[u8]) -> [u8; 32] {
        let hash = SwissTable::compute_hash(content);

        // Check if already added
        if self.block_entries.iter().any(|(h, _, _)| *h == hash) {
            return hash;
        }

        // Pad to page alignment
        let offset = HEADER_SIZE as u64 + self.block_data.len() as u64;
        let aligned_offset = align_up(offset, BLOCK_ALIGN);
        let padding = (aligned_offset - offset) as usize;
        if padding > 0 {
            self.block_data.extend(std::iter::repeat(0u8).take(padding));
        }

        let actual_offset = HEADER_SIZE as u64 + self.block_data.len() as u64;
        self.block_data.extend_from_slice(content);
        self.block_entries
            .push((hash, actual_offset, content.len() as u32));

        hash
    }

    /// Add a manifest for a package version
    pub fn add_manifest(
        &mut self,
        name: &str,
        version: &str,
        files: &[(String, u16, [u8; 32])],
    ) -> Result<(), String> {
        let mut manifest = Manifest::new(name, version);

        // Ensure strings are in the string table
        self.string_table.intern(name);
        self.string_table.intern(version);

        for (path, mode, hash) in files {
            self.string_table.intern(path);
            manifest.add_file(self.string_table.find(path).unwrap().1, *mode, *hash);

            // Also intern path components for tree building
            for part in path.split('/') {
                self.string_table.intern(part);
            }
        }

        // Compute tree hash from concatenated file content hashes
        let mut hasher = Sha256::new();
        for f in &manifest.files {
            hasher.update(&f.content_hash);
        }
        let tree_hash = hasher.finalize();
        let mut th = [0u8; 32];
        th.copy_from_slice(&tree_hash);
        manifest.set_tree_hash(th);

        self.manifests.push(manifest);
        Ok(())
    }

    /// Finalize the JGrid file, writing all sections and updating the header
    pub fn finalize(mut self) -> Result<GridHeader, String> {
        // Serialize manifest first (before string_table is consumed)
        let manifest_bytes = Manifest::serialize_index(&self.manifests, &self.string_table)?;

        // String table
        let string_bytes = self.string_table.into_bytes();
        self.header.string_table_size = string_bytes.len() as u64;
        self.header.string_table_offset = HEADER_SIZE as u64 + self.block_data.len() as u64;

        // Build Swiss Table with automatic rehashing
        let seed = rand_seed();
        let mut swiss = SwissTable::new(self.block_entries.len() as u64, seed);
        let entries: Vec<([u8; 32], u64, u32)> = self
            .block_entries
            .iter()
            .map(|(h, o, s)| (*h, *o, *s))
            .collect();
        swiss
            .insert_or_rehash(&entries, 32)
            .map_err(|e| format!("insert into swiss table: {e}"))?;

        let ctrl_slots = swiss.serialize();
        let ctrl_size = swiss.ctrl_bytes.len() as u64;
        let slot_size = swiss.slots.len() as u64;

        self.header.hash_seed = swiss.seed;
        self.header.block_count = self.block_entries.len() as u64;
        self.header.manifest_count = self.manifests.len() as u32;
        self.header.ctrl_word_size = ctrl_size;
        self.header.hash_slot_size = slot_size;

        // Swiss Table offset — after string table, page-aligned
        let swiss_offset = align_up(
            self.header.string_table_offset + self.header.string_table_size,
            BLOCK_ALIGN,
        );
        self.header.hash_slot_offset = swiss_offset;
        self.header.ctrl_word_offset = swiss_offset - ctrl_size as u64;

        // Manifest offset — after Swiss Table, page-aligned
        let manifest_offset = align_up(
            swiss_offset + self.header.hash_slot_size + self.header.ctrl_word_size,
            BLOCK_ALIGN,
        );
        self.header.manifest_offset = manifest_offset;

        // Total block data
        self.header.total_block_size = self.block_data.len() as u64;
        self.header.block_data_offset = HEADER_SIZE as u64;

        // Write everything
        let file = &mut self.file;

        // Seek to start and write updated header
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("seek: {e}"))?;
        self.header.write_to(file)?;

        // Write content blocks (already contiguous after header)
        file.seek(SeekFrom::Start(HEADER_SIZE as u64))
            .map_err(|e| format!("seek: {e}"))?;
        file.write_all(&self.block_data)
            .map_err(|e| format!("write blocks: {e}"))?;

        // Write string table
        file.seek(SeekFrom::Start(self.header.string_table_offset))
            .map_err(|e| format!("seek: {e}"))?;
        file.write_all(&string_bytes)
            .map_err(|e| format!("write string table: {e}"))?;

        // Write Swiss Table
        file.seek(SeekFrom::Start(self.header.ctrl_word_offset))
            .map_err(|e| format!("seek: {e}"))?;
        file.write_all(&ctrl_slots)
            .map_err(|e| format!("write ctrl: {e}"))?;

        // Write manifest index
        file.seek(SeekFrom::Start(self.header.manifest_offset))
            .map_err(|e| format!("seek: {e}"))?;
        file.write_all(&manifest_bytes)
            .map_err(|e| format!("write manifest: {e}"))?;

        file.sync_all().map_err(|e| format!("sync: {e}"))?;

        Ok(self.header)
    }

    pub fn string_table(&self) -> &StringTable {
        &self.string_table
    }

    pub fn string_table_mut(&mut self) -> &mut StringTable {
        &mut self.string_table
    }
}

fn align_up(val: u64, align: u64) -> u64 {
    (val + align - 1) & !(align - 1)
}

fn rand_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (nanos ^ (nanos >> 32)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_build_and_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jgrid");
        use crate::store::GridStore;

        let mut builder = GridBuilder::create(&path).unwrap();

        let content_a = b"function greet() { return 'hello'; }";
        let hash_a = builder.add_block(content_a);

        let content_b = b"{\"name\":\"test\"}";
        let hash_b = builder.add_block(content_b);

        builder
            .add_manifest(
                "test-pkg",
                "1.0.0",
                &[
                    ("index.js".to_string(), 0o644, hash_a),
                    ("package.json".to_string(), 0o644, hash_b),
                ],
            )
            .unwrap();

        let header = builder.finalize().unwrap();
        assert_eq!(header.get_block_count(), 2);
        assert_eq!(header.get_manifest_count(), 1);

        // Read back
        let store = GridStore::open(&path).unwrap();
        assert_eq!(store.header().get_block_count(), 2);
        assert_eq!(store.manifests().len(), 1);

        let manifest = store.find_manifest("test-pkg", "1.0.0").unwrap();
        assert_eq!(manifest.files.len(), 2);

        let read_a = store.read_block(&hash_a).unwrap();
        assert_eq!(read_a, content_a);

        let read_b = store.read_block(&hash_b).unwrap();
        assert_eq!(read_b, content_b);
    }

    #[test]
    fn test_block_dedup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("dedup.jgrid");

        let mut builder = GridBuilder::create(&path).unwrap();

        let content = b"same content";
        let hash1 = builder.add_block(content);
        let hash2 = builder.add_block(content);

        assert_eq!(hash1, hash2);
        assert_eq!(builder.block_entries.len(), 1);
    }
}
