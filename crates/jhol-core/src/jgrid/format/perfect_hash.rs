/// CHD (Compress, Hash, Displace) minimal perfect hash for a static set of SHA-256 hashes.
///
/// Maps N keys to unique indices in [0, N) with O(1) worst-case time — no probes.
/// Used as an optimisation layer above the Swiss Table for small-to-medium block counts.
#[derive(Clone, Debug)]
pub struct PerfectHash {
    num_keys: usize,
    seeds: Vec<u32>,
    num_buckets: usize,
    salt: u64,
}

impl PerfectHash {
    /// Build a minimal perfect hash from the given SHA-256 hashes.
    /// Returns None if construction fails (extremely rare for <65536 keys).
    pub fn build(keys: &[[u8; 32]]) -> Option<Self> {
        let n = keys.len();
        if n == 0 {
            return Some(Self {
                num_keys: 0,
                seeds: Vec::new(),
                num_buckets: 0,
                salt: 0,
            });
        }

        // Number of buckets = max(1, n / 4) so average bucket size ≈ 4
        let num_buckets = (n / 4).max(1);

        // Try salts to build the PHF
        for salt_try in 0..512u64 {
            let salt = salt_try.wrapping_mul(0x9e3779b97f4a7c15);

            // First-level: assign each key to a bucket
            let mut raw_buckets: Vec<Vec<(usize, [u8; 32])>> = vec![Vec::new(); num_buckets];
            for (idx, key) in keys.iter().enumerate() {
                let bkt = (bucket_hash(key, salt) as usize) % num_buckets;
                raw_buckets[bkt].push((idx, *key));
            }

            // Build indexed buckets for sorting
            let mut indexed_buckets: Vec<(usize, Vec<(usize, [u8; 32])>)> = raw_buckets
                .into_iter()
                .enumerate()
                .filter(|(_, v)| !v.is_empty())
                .collect();

            // Sort by bucket size descending
            indexed_buckets.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

            // Second-level: find per-bucket seeds
            let mut seeds = vec![0u32; num_buckets];
            let mut occupied = vec![false; n];
            let mut fail = false;

            for &(bucket_idx, ref bucket) in &indexed_buckets {
                let mut found_seed = None;
                for s in 0u32..65536u32 {
                    let mut slots = Vec::with_capacity(bucket.len());
                    let mut slot_occupied = false;
                    for (_, key) in bucket.iter() {
                        let slot = (displace_hash(key, salt, s) as usize) % n;
                        if occupied[slot] || slots.contains(&slot) {
                            slot_occupied = true;
                            break;
                        }
                        slots.push(slot);
                    }
                    if slot_occupied {
                        continue;
                    }
                    // All slots free — commit
                    for &slot in &slots {
                        occupied[slot] = true;
                    }
                    found_seed = Some(s);
                    break;
                }

                match found_seed {
                    Some(seed) => {
                        seeds[bucket_idx] = seed;
                    }
                    None => {
                        fail = true;
                        break;
                    }
                }
            }

            if !fail {
                return Some(Self {
                    num_keys: n,
                    seeds,
                    num_buckets,
                    salt,
                });
            }
        }

        None
    }

    /// Look up a SHA-256 hash → unique index in [0, num_keys).
    /// Returns Some(index) for any key.
    pub fn lookup(&self, hash: &[u8; 32]) -> Option<u32> {
        if self.num_keys == 0 {
            return None;
        }
        let bkt = (bucket_hash(hash, self.salt) as usize) % self.num_buckets;
        let seed = self.seeds[bkt];
        let slot = (displace_hash(hash, self.salt, seed) as usize) % self.num_keys;
        Some(slot as u32)
    }

    pub fn num_keys(&self) -> usize {
        self.num_keys
    }
}

/// First-level hash: maps key → bucket index
fn bucket_hash(key: &[u8; 32], salt: u64) -> u64 {
    let lo = u64::from_le_bytes(key[0..8].try_into().unwrap());
    let hi = u64::from_le_bytes(key[8..16].try_into().unwrap());
    lo.wrapping_mul(salt) ^ hi.rotate_left(17)
}

/// Second-level hash: maps key → slot within a bucket
fn displace_hash(key: &[u8; 32], salt: u64, seed: u32) -> u64 {
    let lo = u64::from_le_bytes(key[16..24].try_into().unwrap());
    let hi = u64::from_le_bytes(key[24..32].try_into().unwrap());
    lo.wrapping_mul(salt ^ seed as u64)
        .wrapping_add(hi.rotate_left(seed & 63))
        .wrapping_add(seed as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn test_empty() {
        let ph = PerfectHash::build(&[]).unwrap();
        assert_eq!(ph.num_keys(), 0);
        assert!(ph.lookup(&[0u8; 32]).is_none());
    }

    #[test]
    fn test_single_key() {
        let keys = vec![[1u8; 32]];
        let ph = PerfectHash::build(&keys).unwrap();
        assert_eq!(ph.num_keys(), 1);
        assert_eq!(ph.lookup(&keys[0]).unwrap(), 0);
    }

    #[test]
    fn test_real_sha_hashes() {
        let data: &[&[u8]] = &[b"hello", b"world", b"foo", b"bar", b"baz"];
        let keys: Vec<[u8; 32]> = data
            .iter()
            .map(|d| {
                let mut h = [0u8; 32];
                h.copy_from_slice(&sha2::Sha256::digest(d));
                h
            })
            .collect();

        let ph = PerfectHash::build(&keys).unwrap();
        assert_eq!(ph.num_keys(), 5);

        let mut slots = [false; 5];
        for k in &keys {
            let idx = ph.lookup(k).unwrap() as usize;
            assert!(idx < 5, "index {idx} out of range");
            assert!(!slots[idx], "duplicate slot {idx}");
            slots[idx] = true;
        }
        assert!(slots.iter().all(|&x| x), "all slots should be occupied");
    }
}
