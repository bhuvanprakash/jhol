use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::jgrid::format::{GridHeader, Manifest, StringTable, SwissTable, HEADER_SIZE};

/// The main JGrid store — a memory-mappable content-addressed file
pub struct GridStore {
    path: PathBuf,
    header: GridHeader,
    mmap: Option<Mmap>,
    swiss_table: Option<SwissTable>,
    string_table: StringTable,
    manifests: Vec<Manifest>,
}

impl GridStore {
    /// Open an existing JGrid file.
    ///
    /// Every offset comes from the file itself, so all slicing is bounds-checked:
    /// a truncated or corrupted store must return `Err` (and let the caller
    /// rebuild it), never panic.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("open JGrid: {e}"))?;
        let mmap = unsafe { Mmap::map(&file).map_err(|e| format!("mmap JGrid: {e}"))? };
        let len = mmap.len();

        if len < HEADER_SIZE {
            return Err(format!(
                "JGrid store too small: {} bytes (need {})",
                len, HEADER_SIZE
            ));
        }

        let header = GridHeader::read_from(&mut std::io::Cursor::new(&mmap[..HEADER_SIZE]))?;

        // Helper: bounds-check a (offset, size) pair read out of the header.
        let checked =
            |off: usize, size: usize, what: &str| -> Result<std::ops::Range<usize>, String> {
                let end = off
                    .checked_add(size)
                    .ok_or_else(|| format!("JGrid {what} range overflow"))?;
                if end > len {
                    return Err(format!(
                        "JGrid {what} out of bounds ({}..{} > {})",
                        off, end, len
                    ));
                }
                Ok(off..end)
            };

        let st_off = header.get_string_table_offset() as usize;
        let st_sz = header.get_string_table_size() as usize;
        let string_table_bytes = &mmap[checked(st_off, st_sz, "string table")?];
        let string_table = StringTable::from_bytes(string_table_bytes.to_vec());

        let manifests = if header.get_manifest_count() > 0 {
            let m_off = header.get_manifest_offset() as usize;
            let manifest_bytes = &mmap[checked(m_off, 0, "manifest offset")?.start..];
            Manifest::deserialize_index(manifest_bytes, &string_table)?
        } else {
            Vec::new()
        };

        let swiss_table = if header.get_block_count() > 0 {
            let cw_off = header.get_ctrl_word_offset() as usize;
            let cw_sz = header.get_ctrl_word_size() as usize;
            let hs_off = header.get_hash_slot_offset() as usize;
            let hs_sz = header.get_hash_slot_size() as usize;
            let ctrl_bytes = &mmap[checked(cw_off, cw_sz, "swiss control words")?];
            let slot_bytes = &mmap[checked(hs_off, hs_sz, "swiss slots")?];
            Some(SwissTable::deserialize(
                ctrl_bytes,
                slot_bytes,
                header.get_block_count(),
                header.get_hash_seed(),
            ))
        } else {
            None
        };

        Ok(Self {
            path: path.to_path_buf(),
            header,
            mmap: Some(mmap),
            swiss_table,
            string_table,
            manifests,
        })
    }

    /// Create a new JGrid file (writes header only)
    pub fn create(path: &Path) -> Result<Self, String> {
        let header = GridHeader::new();

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(path)
            .map_err(|e| format!("create JGrid: {e}"))?;

        // Write a valid (empty) header. Zeroes here are not a valid magic, so
        // the store was unreadable on the next run (`invalid JGrid magic or
        // version`) and the pipeline fast path never worked.
        let mut header_bytes = Vec::with_capacity(HEADER_SIZE);
        header
            .write_to(&mut header_bytes)
            .map_err(|e| format!("serialize header: {e}"))?;
        header_bytes.resize(HEADER_SIZE, 0);
        file.write_all(&header_bytes)
            .map_err(|e| format!("write header: {e}"))?;
        file.sync_all().map_err(|e| format!("sync header: {e}"))?;

        let mmap = unsafe { Mmap::map(&file).map_err(|e| format!("mmap JGrid: {e}"))? };

        Ok(Self {
            path: path.to_path_buf(),
            header,
            mmap: Some(mmap),
            swiss_table: None,
            string_table: StringTable::new(),
            manifests: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn header(&self) -> GridHeader {
        self.header
    }

    pub fn string_table(&self) -> &StringTable {
        &self.string_table
    }

    pub fn manifests(&self) -> &[Manifest] {
        &self.manifests
    }

    pub fn swiss_table(&self) -> Option<&SwissTable> {
        self.swiss_table.as_ref()
    }

    /// Look up a content block by its SHA-256 hash, returns (offset_in_file, size)
    pub fn find_block(&self, hash: &[u8; 32]) -> Option<(u64, u32)> {
        self.swiss_table.as_ref().and_then(|st| st.lookup(hash))
    }

    /// Read a content block's bytes
    pub fn read_block(&self, hash: &[u8; 32]) -> Option<&[u8]> {
        let (offset, size) = self.find_block(hash)?;
        let mmap = self.mmap.as_ref()?;
        let start = offset as usize;
        let end = start + size as usize;
        if end <= mmap.len() {
            Some(&mmap[start..end])
        } else {
            None
        }
    }

    /// Find a manifest entry by package name and version
    pub fn find_manifest(&self, name: &str, version: &str) -> Option<&Manifest> {
        self.manifests
            .iter()
            .find(|m| m.name == name && m.version == version)
    }

    pub fn find_manifest_mut(&mut self, name: &str, version: &str) -> Option<&mut Manifest> {
        self.manifests
            .iter_mut()
            .find(|m| m.name == name && m.version == version)
    }
}
