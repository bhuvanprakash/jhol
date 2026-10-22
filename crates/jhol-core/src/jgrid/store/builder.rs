use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::jgrid::format::{
    GridHeader, Manifest, StringTable, SwissTable, BLOCK_ALIGN, HEADER_SIZE,
};

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
    block_data_len: u64,
    block_write_error: Option<String>,
    block_entries: Vec<([u8; 32], u64, u32)>, // (hash, offset, size)
    block_hashes: HashSet<[u8; 32]>,

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

        // Write a valid (empty) header immediately. Writing zeros here left a
        // store that failed the magic check on the *next* run
        // (`invalid JGrid magic or version`), so the pipeline fast path could
        // never be used until the file was deleted by hand.
        let header = GridHeader::new();
        let mut header_bytes = Vec::with_capacity(HEADER_SIZE);
        header
            .write_to(&mut header_bytes)
            .map_err(|e| format!("serialize header: {e}"))?;
        header_bytes.resize(HEADER_SIZE, 0);
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
            block_data_len: 0,
            block_write_error: None,
            block_entries: Vec::new(),
            block_hashes: HashSet::new(),
            pending_strings: Vec::new(),
        })
    }

    /// Add a file block to the store
    pub fn add_block(&mut self, content: &[u8]) -> [u8; 32] {
        let hash = SwissTable::compute_hash(content);

        // Check if already added
        if self.block_hashes.contains(&hash) || self.block_write_error.is_some() {
            return hash;
        }

        let offset = HEADER_SIZE as u64 + self.block_data_len;
        let actual_offset = align_up(offset, BLOCK_ALIGN);
        let padding = (actual_offset - offset) as usize;
        let write_result = (|| -> std::io::Result<()> {
            self.file.seek(SeekFrom::Start(offset))?;
            if padding > 0 {
                self.file
                    .write_all(&[0u8; BLOCK_ALIGN as usize][..padding])?;
            }
            self.file.write_all(content)
        })();
        if let Err(error) = write_result {
            self.block_write_error = Some(error.to_string());
            return hash;
        }
        self.block_data_len = actual_offset - HEADER_SIZE as u64 + content.len() as u64;
        self.block_hashes.insert(hash);
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
            let path_offset = self
                .string_table
                .find(path)
                .map(|(_, off)| off)
                .unwrap_or(0);
            manifest.add_file(path_offset, *mode, *hash);

            // Also intern path components for tree building
            for part in path.split('/') {
                self.string_table.intern(part);
            }
        }

        let tree_entries: Vec<(&str, u16, [u8; 32])> = files
            .iter()
            .map(|(path, mode, hash)| (path.as_str(), *mode, *hash))
            .collect();
        manifest.set_tree_hash(Manifest::compute_tree_hash(&tree_entries));

        self.manifests.push(manifest);
        Ok(())
    }

    /// Finalize the JGrid file, writing all sections and updating the header
    pub fn finalize(mut self) -> Result<GridHeader, String> {
        if let Some(error) = self.block_write_error.take() {
            return Err(format!("write content block: {error}"));
        }
        // Serialize manifest first (before string_table is consumed)
        let manifest_bytes = Manifest::serialize_index(&self.manifests, &self.string_table)?;

        // String table
        let string_bytes = self.string_table.into_bytes();
        self.header.string_table_size = string_bytes.len() as u64;
        self.header.string_table_offset = HEADER_SIZE as u64 + self.block_data_len;

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
        self.header.ctrl_word_offset = swiss_offset - ctrl_size;

        // Manifest offset — after Swiss Table, page-aligned
        let manifest_offset = align_up(
            swiss_offset + self.header.hash_slot_size + self.header.ctrl_word_size,
            BLOCK_ALIGN,
        );
        self.header.manifest_offset = manifest_offset;

        // Total block data
        self.header.total_block_size = self.block_data_len;
        self.header.block_data_offset = HEADER_SIZE as u64;

        // Write everything
        let file = &mut self.file;

        // Seek to start and write updated header
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("seek: {e}"))?;
        self.header.write_to(file)?;

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
    #[test]
    fn add_manifest_tolerates_empty_path() {
        // Regression: an empty path made `string_table.find(path).unwrap()` panic
        // and, in the release profile (`panic = "abort"`), killed the process.
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty-path.jgrid");
        let mut builder = GridBuilder::create(&path).unwrap();
        let hash = builder.add_block(b"x");
        builder
            .add_manifest("pkg", "1.0.0", &[("".to_string(), 0o644, hash)])
            .unwrap();
        builder.finalize().unwrap();
    }

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_build_and_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jgrid");
        use crate::jgrid::store::GridStore;

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
