use sha2::{Digest, Sha256};

pub const SLOTS_PER_GROUP: usize = 16;
pub const SLOT_BYTES: usize = 10;
pub const GROUP_BYTES: usize = SLOTS_PER_GROUP * SLOT_BYTES;
pub const CTRL_BYTES_PER_GROUP: usize = 8;

/// A 10-byte hash table slot
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct HashSlot {
    pub hash_prefix: u16,
    pub block_offset: u40_le,
    pub block_size: u24_le,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct u40_le([u8; 5]);

impl u40_le {
    pub fn new(val: u64) -> Self {
        let mut bytes = [0u8; 5];
        bytes[0] = val as u8;
        bytes[1] = (val >> 8) as u8;
        bytes[2] = (val >> 16) as u8;
        bytes[3] = (val >> 24) as u8;
        bytes[4] = (val >> 32) as u8;
        Self(bytes)
    }

    pub fn get(&self) -> u64 {
        self.0[0] as u64
            | (self.0[1] as u64) << 8
            | (self.0[2] as u64) << 16
            | (self.0[3] as u64) << 24
            | (self.0[4] as u64) << 32
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct u24_le([u8; 3]);

impl u24_le {
    pub fn new(val: u32) -> Self {
        let mut bytes = [0u8; 3];
        bytes[0] = val as u8;
        bytes[1] = (val >> 8) as u8;
        bytes[2] = (val >> 16) as u8;
        Self(bytes)
    }

    pub fn get(&self) -> u32 {
        self.0[0] as u32 | (self.0[1] as u32) << 8 | (self.0[2] as u32) << 16
    }
}

/// Swiss Table hash table for SHA-256 → (offset, size) lookups
pub struct SwissTable {
    pub ctrl_bytes: Vec<u8>,
    pub slots: Vec<u8>,
    pub slot_count: u64,
    pub num_groups: u64,
    pub seed: u64,
}

impl SwissTable {
    pub fn new(capacity: u64, seed: u64) -> Self {
        if capacity == 0 {
            return Self {
                ctrl_bytes: Vec::new(),
                slots: Vec::new(),
                slot_count: 0,
                num_groups: 0,
                seed,
            };
        }
        let slot_count = capacity.next_power_of_two() * 2;
        // Ensure at least 1 group
        let num_groups = (slot_count / SLOTS_PER_GROUP as u64).max(1);

        Self {
            ctrl_bytes: vec![0x00u8; (num_groups as usize) * CTRL_BYTES_PER_GROUP],
            slots: vec![0u8; (num_groups as usize) * GROUP_BYTES],
            slot_count,
            num_groups,
            seed,
        }
    }

    fn hash_to_index_and_stamp(hash: &[u8; 32], seed: u64, num_groups: u64) -> (u64, u8, u16) {
        let h = u64::from_le_bytes(hash[0..8].try_into().unwrap()).wrapping_add(seed);
        // Use top 16 bits for group index, modulo num_groups
        let group_index = (h >> 48) % num_groups;
        // Use next bits for 3-bit stamp (matches the 4-bit/slot encoding: 3 stamp + 1 occupied)
        let stamp = ((h >> 41) & 0x7) as u8;
        // Use first 16 bits of SHA-256 as prefix for fast comparison
        let prefix = u16::from_le_bytes([hash[0], hash[1]]);
        (group_index, stamp, prefix)
    }

    /// Insert a hash → (offset, size) mapping.
    /// Returns Ok(true) if inserted, Ok(false) if group overflow (group full),
    /// and Err for empty table.
    pub fn insert(&mut self, hash: &[u8; 32], offset: u64, size: u32) -> Result<bool, String> {
        if self.num_groups == 0 {
            return Err("empty table".into());
        }
        let (group_idx, stamp, prefix) =
            Self::hash_to_index_and_stamp(hash, self.seed, self.num_groups);

        for probe in 0..SLOTS_PER_GROUP {
            let slot_idx = group_idx as usize * SLOTS_PER_GROUP + probe;
            let ctrl_byte_offset = group_idx as usize * CTRL_BYTES_PER_GROUP + probe / 2;
            let ctrl_bit_offset = if probe % 2 == 0 { 0 } else { 4 };

            let ctrl = self.ctrl_bytes[ctrl_byte_offset];
            let slot_occupied = (ctrl >> (ctrl_bit_offset + 3)) & 1;

            if slot_occupied == 0 {
                let slot_byte_offset = slot_idx * SLOT_BYTES;
                let slot = hash_slot_mut(&mut self.slots, slot_byte_offset);
                slot.hash_prefix = prefix;
                slot.block_offset = u40_le::new(offset);
                slot.block_size = u24_le::new(size);

                let new_ctrl = ctrl & !(0x7 << ctrl_bit_offset) & !(1 << (ctrl_bit_offset + 3))
                    | ((stamp as u8) << ctrl_bit_offset)
                    | (1 << (ctrl_bit_offset + 3));
                self.ctrl_bytes[ctrl_byte_offset] = new_ctrl;
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Insert with automatic rehashing.
    /// Retries with a new seed up to `max_attempts` times before returning overflow.
    pub fn insert_or_rehash(
        &mut self,
        entries: &[([u8; 32], u64, u32)],
        max_attempts: usize,
    ) -> Result<(), String> {
        // Collect group overflow flags per attempt
        for attempt in 0..max_attempts {
            if attempt > 0 {
                self.seed = self.seed.wrapping_add(0x9e3779b97f4a7c15);
                self.clear();
            }

            let mut overflow = false;
            for (hash, offset, size) in entries {
                match self.insert(hash, *offset, *size)? {
                    true => {}
                    false => {
                        overflow = true;
                        break;
                    }
                }
            }

            if !overflow {
                return Ok(());
            }
        }

        // Last resort: grow the table
        let new_capacity = (self.slot_count as usize).next_power_of_two() * 4;
        let mut new_table = Self::new(
            new_capacity as u64,
            self.seed.wrapping_add(0x517cc1b727220a95),
        );
        for (hash, offset, size) in entries {
            match new_table.insert(hash, *offset, *size)? {
                true => {}
                false => {
                    return Err(format!(
                        "SwissTable overflow after rehash+resize ({} slots, {} entries)",
                        new_table.slot_count,
                        entries.len(),
                    ))
                }
            }
        }
        *self = new_table;
        Ok(())
    }

    fn clear(&mut self) {
        self.ctrl_bytes.fill(0x00u8);
        self.slots.fill(0u8);
    }

    /// Look up a SHA-256 hash → (offset, size)
    pub fn lookup(&self, hash: &[u8; 32]) -> Option<(u64, u32)> {
        if self.num_groups == 0 {
            return None;
        }
        let (group_idx, stamp, prefix) =
            Self::hash_to_index_and_stamp(hash, self.seed, self.num_groups);

        for probe in 0..SLOTS_PER_GROUP {
            let slot_idx = group_idx as usize * SLOTS_PER_GROUP + probe;
            let ctrl_byte_offset = group_idx as usize * CTRL_BYTES_PER_GROUP + probe / 2;
            let ctrl_bit_offset = if probe % 2 == 0 { 0 } else { 4 };

            let ctrl = self.ctrl_bytes[ctrl_byte_offset];
            let slot_stamp = (ctrl >> ctrl_bit_offset) & 0x7;
            let slot_occupied = (ctrl >> (ctrl_bit_offset + 3)) & 1;

            if slot_occupied == 0 {
                return None;
            }

            if slot_stamp != stamp {
                continue;
            }

            let slot_byte_offset = slot_idx * SLOT_BYTES;
            let slot = hash_slot(&self.slots, slot_byte_offset);
            if slot.hash_prefix == prefix {
                return Some((slot.block_offset.get(), slot.block_size.get()));
            }
        }

        None
    }

    pub fn slot_count(&self) -> u64 {
        self.slot_count
    }

    pub fn byte_size(&self) -> u64 {
        self.ctrl_bytes.len() as u64 + self.slots.len() as u64
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.ctrl_bytes.len() + self.slots.len());
        out.extend_from_slice(&self.ctrl_bytes);
        out.extend_from_slice(&self.slots);
        out
    }

    pub fn deserialize(ctrl_bytes: &[u8], slots: &[u8], slot_count: u64, seed: u64) -> Self {
        let num_groups = ctrl_bytes.len() as u64 / CTRL_BYTES_PER_GROUP as u64;
        Self {
            ctrl_bytes: ctrl_bytes.to_vec(),
            slots: slots.to_vec(),
            slot_count,
            num_groups,
            seed,
        }
    }

    pub fn compute_hash(data: &[u8]) -> [u8; 32] {
        let result = Sha256::digest(data);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }
}

fn hash_slot(slots: &[u8], offset: usize) -> &HashSlot {
    unsafe { &*(slots[offset..].as_ptr() as *const HashSlot) }
}

fn hash_slot_mut(slots: &mut [u8], offset: usize) -> &mut HashSlot {
    unsafe { &mut *(slots[offset..].as_mut_ptr() as *mut HashSlot) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_lookup() {
        let mut table = SwissTable::new(128, 42);
        let data = b"hello world";
        let hash = SwissTable::compute_hash(data);

        table.insert(&hash, 4096, data.len() as u32).unwrap();
        let result = table.lookup(&hash);
        assert!(result.is_some());
        let (offset, size) = result.unwrap();
        assert_eq!(offset, 4096);
        assert_eq!(size, data.len() as u32);
    }

    #[test]
    fn test_nonexistent_lookup() {
        let table = SwissTable::new(128, 42);
        let hash = SwissTable::compute_hash(b"not inserted");
        assert!(table.lookup(&hash).is_none());
    }

    #[test]
    fn test_many_inserts() {
        let mut table = SwissTable::new(256, 12345);

        for i in 0..100 {
            let data = format!("file content {i}");
            let hash = SwissTable::compute_hash(data.as_bytes());
            table
                .insert(&hash, 4096 * (i + 1) as u64, data.len() as u32)
                .unwrap();
        }
    }

    #[test]
    fn test_many_inserts_with_rehash() {
        let mut table = SwissTable::new(256, 42);
        let entries: Vec<([u8; 32], u64, u32)> = (0..200)
            .map(|i| {
                let data = format!("file content {i}");
                let hash = SwissTable::compute_hash(data.as_bytes());
                (hash, 4096 * (i + 1) as u64, data.len() as u32)
            })
            .collect();
        table.insert_or_rehash(&entries, 32).unwrap();
        assert_eq!(table.slot_count(), 512);
    }

    #[test]
    fn test_u40_le_roundtrip() {
        let vals = [0, 1, 255, 1 << 20, (1 << 40) - 1];
        for v in vals {
            let encoded = u40_le::new(v);
            assert_eq!(encoded.get(), v);
        }
    }

    #[test]
    fn test_u24_le_roundtrip() {
        let vals = [0, 1, 255, 1 << 16, (1 << 24) - 1];
        for v in vals {
            let encoded = u24_le::new(v);
            assert_eq!(encoded.get(), v);
        }
    }
}
