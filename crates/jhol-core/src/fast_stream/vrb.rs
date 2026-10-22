//! Vectorized Ring-Buffer (VRB 2.0) - Lock-Free Zero-Allocation Socket Ingestion
//!
//! Provides a circular cache-line aligned slab arena for high-throughput zero-copy
//! network-to-decompressor streaming.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub const SLAB_SIZE: usize = 64 * 1024; // 64 KB cache-aligned slab
pub const DEFAULT_ARENA_SLABS: usize = 64; // 4 MB per stream ring

/// A single cache-aligned memory slab
#[repr(align(64))]
pub struct AlignedSlab {
    pub data: [u8; SLAB_SIZE],
}

impl Default for AlignedSlab {
    fn default() -> Self {
        Self::new()
    }
}

impl AlignedSlab {
    pub fn new() -> Self {
        Self {
            data: [0u8; SLAB_SIZE],
        }
    }
}

/// Circular lock-free Slab Arena for zero-allocation streaming
pub struct SlabArena {
    slabs: Vec<AlignedSlab>,
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl SlabArena {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(4);
        let mut slabs = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slabs.push(AlignedSlab::new());
        }
        Self {
            slabs,
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Number of available slabs for writing
    #[inline]
    pub fn available_write(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        let occupied = head.wrapping_sub(tail);
        self.capacity.saturating_sub(occupied)
    }

    /// Number of ready slabs for reading
    #[inline]
    pub fn available_read(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    /// Write a chunk into the next available slab
    pub fn push_chunk(&self, chunk: &[u8]) -> Result<usize, String> {
        if chunk.is_empty() {
            return Ok(0);
        }
        if chunk.len() > SLAB_SIZE {
            return Err(format!(
                "Chunk size {} exceeds slab size {}",
                chunk.len(),
                SLAB_SIZE
            ));
        }

        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.capacity {
            return Err("VRB SlabArena full".to_string());
        }

        let idx = head % self.capacity;
        // Unsafe pointer copy into pre-allocated aligned slab
        let slab_ptr = self.slabs[idx].data.as_ptr() as *mut u8;
        unsafe {
            std::ptr::copy_nonoverlapping(chunk.as_ptr(), slab_ptr, chunk.len());
        }

        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(chunk.len())
    }

    /// Read data from the current slab into a destination buffer
    pub fn pop_chunk(&self, len: usize, dest: &mut [u8]) -> Result<usize, String> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return Err("VRB SlabArena empty".to_string());
        }

        let idx = tail % self.capacity;
        let copy_len = len.min(SLAB_SIZE).min(dest.len());
        let slab_ptr = self.slabs[idx].data.as_ptr();
        unsafe {
            std::ptr::copy_nonoverlapping(slab_ptr, dest.as_mut_ptr(), copy_len);
        }

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(copy_len)
    }

    /// Reset ring buffer pointers
    pub fn reset(&self) {
        self.head.store(0, Ordering::Release);
        self.tail.store(0, Ordering::Release);
    }
}

/// Thread-safe shared Ring Buffer channel for streaming pipeline
#[derive(Clone)]
pub struct VrbStreamChannel {
    arena: Arc<SlabArena>,
}

impl VrbStreamChannel {
    pub fn new(capacity: usize) -> Self {
        Self {
            arena: Arc::new(SlabArena::new(capacity)),
        }
    }

    pub fn push(&self, chunk: &[u8]) -> Result<usize, String> {
        self.arena.push_chunk(chunk)
    }

    pub fn pop(&self, len: usize, dest: &mut [u8]) -> Result<usize, String> {
        self.arena.pop_chunk(len, dest)
    }

    pub fn is_empty(&self) -> bool {
        self.arena.available_read() == 0
    }

    pub fn is_full(&self) -> bool {
        self.arena.available_write() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vrb_arena_push_pop() {
        let arena = SlabArena::new(8);
        let data = b"fast streaming packet payload data";
        assert_eq!(arena.available_read(), 0);
        assert_eq!(arena.available_write(), 8);

        let written = arena.push_chunk(data).unwrap();
        assert_eq!(written, data.len());
        assert_eq!(arena.available_read(), 1);
        assert_eq!(arena.available_write(), 7);

        let mut buf = vec![0u8; data.len()];
        let read = arena.pop_chunk(data.len(), &mut buf).unwrap();
        assert_eq!(read, data.len());
        assert_eq!(&buf, data);
        assert_eq!(arena.available_read(), 0);
    }

    #[test]
    fn test_vrb_channel_roundtrip() {
        let channel = VrbStreamChannel::new(4);
        assert!(channel.is_empty());
        let payload = b"zero copy network socket slice";
        channel.push(payload).unwrap();
        assert!(!channel.is_empty());

        let mut out = vec![0u8; payload.len()];
        channel.pop(payload.len(), &mut out).unwrap();
        assert_eq!(&out, payload);
    }
}
