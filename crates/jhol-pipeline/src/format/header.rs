use std::io::{Read, Write};

pub const JHGR_MAGIC: &[u8; 9] = b"JHGR0002\0";
pub const HEADER_SIZE: usize = 256;
pub const BLOCK_ALIGN: u64 = 4096;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct GridHeader {
    pub magic: [u8; 9],
    pub version: u32,
    pub flags: u64,
    pub block_count: u64,
    pub manifest_count: u32,
    pub hash_seed: u64,

    pub ctrl_word_offset: u64,
    pub hash_slot_offset: u64,
    pub block_data_offset: u64,
    pub manifest_offset: u64,
    pub string_table_offset: u64,

    pub ctrl_word_size: u64,
    pub hash_slot_size: u64,
    pub total_block_size: u64,
    pub string_table_size: u64,

    pub tree_index_offset: u64,
    pub tree_index_count: u32,

    pub reserved: [u8; 56],
}

impl GridHeader {
    pub fn new() -> Self {
        Self {
            magic: *JHGR_MAGIC,
            version: 2,
            flags: 0,
            block_count: 0,
            manifest_count: 0,
            hash_seed: 0,
            ctrl_word_offset: HEADER_SIZE as u64,
            hash_slot_offset: 0,
            block_data_offset: 0,
            manifest_offset: 0,
            string_table_offset: 0,
            ctrl_word_size: 0,
            hash_slot_size: 0,
            total_block_size: 0,
            string_table_size: 0,
            tree_index_offset: 0,
            tree_index_count: 0,
            reserved: [0u8; 56],
        }
    }

    pub fn is_valid(&self) -> bool {
        let magic = self.magic;
        let version = self.version;
        &magic == JHGR_MAGIC && version >= 1
    }

    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self, String> {
        let mut bytes = [0u8; HEADER_SIZE];
        reader
            .read_exact(&mut bytes)
            .map_err(|e| format!("failed to read header: {e}"))?;
        let header = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const GridHeader) };
        if !header.is_valid() {
            return Err("invalid JGrid magic or version".into());
        }
        Ok(header)
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<(), String> {
        let bytes = unsafe {
            std::slice::from_raw_parts(self as *const GridHeader as *const u8, HEADER_SIZE)
        };
        writer
            .write_all(bytes)
            .map_err(|e| format!("failed to write header: {e}"))
    }

    pub fn has_tree_index(&self) -> bool {
        self.flags & 1 != 0
    }

    pub fn has_perfect_hash(&self) -> bool {
        self.flags & 2 != 0
    }

    // Safe field accessors that avoid unaligned packed reference UB
    pub fn get_block_count(&self) -> u64 {
        self.block_count
    }
    pub fn get_manifest_count(&self) -> u32 {
        self.manifest_count
    }
    pub fn get_ctrl_word_offset(&self) -> u64 {
        self.ctrl_word_offset
    }
    pub fn get_hash_slot_offset(&self) -> u64 {
        self.hash_slot_offset
    }
    pub fn get_block_data_offset(&self) -> u64 {
        self.block_data_offset
    }
    pub fn get_manifest_offset(&self) -> u64 {
        self.manifest_offset
    }
    pub fn get_string_table_offset(&self) -> u64 {
        self.string_table_offset
    }
    pub fn get_ctrl_word_size(&self) -> u64 {
        self.ctrl_word_size
    }
    pub fn get_hash_slot_size(&self) -> u64 {
        self.hash_slot_size
    }
    pub fn get_total_block_size(&self) -> u64 {
        self.total_block_size
    }
    pub fn get_string_table_size(&self) -> u64 {
        self.string_table_size
    }
    pub fn get_tree_index_offset(&self) -> u64 {
        self.tree_index_offset
    }
    pub fn get_tree_index_count(&self) -> u32 {
        self.tree_index_count
    }
    pub fn get_hash_seed(&self) -> u64 {
        self.hash_seed
    }
    pub fn get_flags(&self) -> u64 {
        self.flags
    }
    pub fn get_version(&self) -> u32 {
        self.version
    }
}

impl Default for GridHeader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_write_read() {
        let mut h = GridHeader::new();
        h.block_count = 1000;
        h.manifest_count = 42;

        let mut buf = Vec::new();
        h.write_to(&mut buf).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let h2 = GridHeader::read_from(&mut cursor).unwrap();
        assert!(h2.is_valid());
        // Copy from packed struct to avoid unaligned reference UB
        let block_count = h2.block_count;
        let manifest_count = h2.manifest_count;
        assert_eq!(block_count, 1000);
        assert_eq!(manifest_count, 42);
    }

    #[test]
    fn test_header_invalid_magic() {
        let buf = [0u8; HEADER_SIZE];
        let mut cursor = std::io::Cursor::new(&buf[..]);
        let result = GridHeader::read_from(&mut cursor);
        assert!(result.is_err());
    }
}
