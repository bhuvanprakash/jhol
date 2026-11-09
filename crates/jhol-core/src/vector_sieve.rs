//! Vectorized Bitset Sieve for single-cycle 256-bit constraint filtering.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(align(32))]
pub struct BitSet256 {
    pub bits: [u64; 4],
}

impl BitSet256 {
    #[inline(always)]
    pub const fn empty() -> Self {
        Self { bits: [0; 4] }
    }

    #[inline(always)]
    pub const fn full() -> Self {
        Self {
            bits: [u64::MAX; 4],
        }
    }

    /// Construct a bitset covering the continuous interval [start, end)
    #[inline(always)]
    pub fn from_range(start: usize, end: usize) -> Self {
        let mut bs = Self::empty();
        let start = start.min(256);
        let end = end.min(256);
        if start >= end {
            return bs;
        }

        let start_block = start / 64;
        let end_block = (end.saturating_sub(1)) / 64;

        if start_block == end_block {
            let start_rem = start % 64;
            let end_rem = (end - 1) % 64;
            let mask = if end_rem == 63 {
                u64::MAX << start_rem
            } else {
                ((1u64 << (end_rem + 1)) - 1) & (u64::MAX << start_rem)
            };
            bs.bits[start_block] = mask;
        } else {
            bs.bits[start_block] = u64::MAX << (start % 64);
            for b in (start_block + 1)..end_block {
                bs.bits[b] = u64::MAX;
            }
            let end_rem = (end - 1) % 64;
            bs.bits[end_block] = if end_rem == 63 {
                u64::MAX
            } else {
                (1u64 << (end_rem + 1)) - 1
            };
        }
        bs
    }

    #[inline(always)]
    pub fn set(&mut self, index: usize) {
        if index < 256 {
            self.bits[index / 64] |= 1u64 << (index % 64);
        }
    }

    #[inline(always)]
    pub fn get(&self, index: usize) -> bool {
        if index < 256 {
            (self.bits[index / 64] & (1u64 << (index % 64))) != 0
        } else {
            false
        }
    }

    #[inline(always)]
    pub fn intersect(&self, other: &Self) -> Self {
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        unsafe {
            use core::arch::x86_64::*;
            let a = _mm256_loadu_si256(self.bits.as_ptr() as *const __m256i);
            let b = _mm256_loadu_si256(other.bits.as_ptr() as *const __m256i);
            let res = _mm256_and_si256(a, b);
            let mut out = Self::empty();
            _mm256_storeu_si256(out.bits.as_mut_ptr() as *mut __m256i, res);
            out
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        Self {
            bits: [
                self.bits[0] & other.bits[0],
                self.bits[1] & other.bits[1],
                self.bits[2] & other.bits[2],
                self.bits[3] & other.bits[3],
            ],
        }
    }

    #[inline(always)]
    pub fn union(&self, other: &Self) -> Self {
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        unsafe {
            use core::arch::x86_64::*;
            let a = _mm256_loadu_si256(self.bits.as_ptr() as *const __m256i);
            let b = _mm256_loadu_si256(other.bits.as_ptr() as *const __m256i);
            let res = _mm256_or_si256(a, b);
            let mut out = Self::empty();
            _mm256_storeu_si256(out.bits.as_mut_ptr() as *mut __m256i, res);
            out
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        Self {
            bits: [
                self.bits[0] | other.bits[0],
                self.bits[1] | other.bits[1],
                self.bits[2] | other.bits[2],
                self.bits[3] | other.bits[3],
            ],
        }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        (self.bits[0] | self.bits[1] | self.bits[2] | self.bits[3]) == 0
    }

    #[inline(always)]
    pub fn count_ones(&self) -> u32 {
        self.bits[0].count_ones()
            + self.bits[1].count_ones()
            + self.bits[2].count_ones()
            + self.bits[3].count_ones()
    }

    #[inline(always)]
    pub fn first_set(&self) -> Option<usize> {
        for (block_idx, &block) in self.bits.iter().enumerate() {
            if block != 0 {
                return Some(block_idx * 64 + block.trailing_zeros() as usize);
            }
        }
        None
    }

    #[inline(always)]
    pub fn last_set(&self) -> Option<usize> {
        for (block_idx, &block) in self.bits.iter().enumerate().rev() {
            if block != 0 {
                return Some(block_idx * 64 + (63 - block.leading_zeros() as usize));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitset_256_operations() {
        let mut a = BitSet256::empty();
        a.set(10);
        a.set(100);
        a.set(200);

        assert!(a.get(10));
        assert!(a.get(100));
        assert!(a.get(200));
        assert!(!a.get(50));
        assert_eq!(a.count_ones(), 3);

        let mut b = BitSet256::empty();
        b.set(10);
        b.set(50);

        let inter = a.intersect(&b);
        assert!(inter.get(10));
        assert!(!inter.get(100));
        assert_eq!(inter.first_set(), Some(10));
        assert_eq!(inter.last_set(), Some(10));
        assert_eq!(inter.count_ones(), 1);

        let union_set = a.union(&b);
        assert_eq!(union_set.count_ones(), 4);
    }

    #[test]
    fn test_from_range() {
        let range_set = BitSet256::from_range(10, 20);
        assert!(!range_set.get(9));
        assert!(range_set.get(10));
        assert!(range_set.get(19));
        assert!(!range_set.get(20));
        assert_eq!(range_set.count_ones(), 10);
        assert_eq!(range_set.first_set(), Some(10));
        assert_eq!(range_set.last_set(), Some(19));

        // Cross block boundary
        let cross = BitSet256::from_range(60, 70);
        assert_eq!(cross.count_ones(), 10);
        assert!(cross.get(60));
        assert!(cross.get(63));
        assert!(cross.get(64));
        assert!(cross.get(69));
        assert!(!cross.get(70));
    }
}
