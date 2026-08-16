# JGrid — Binary Format Specification

**Version 2 | Revision 2026-07-26**

JGrid is the native content-addressed storage format for the Jhol Instant
Materialization Pipeline (jhol-pipeline). It replaces `.tgz` tarballs with a
memory-mappable, clone-ready binary archive that enables sub-millisecond package
materialization.

---

## 1. Overview

A JGrid file (`.jgrid`) is a single self-describing binary file that stores:

- **Content blocks**: deduplicated byte buffers addressed by SHA-256 hash
- **Swiss Table**: an O(1) hash-index from SHA-256 → (offset, size) via 16-bit
  control-word + 56-bit hash-slot SIMD-probe table
- **Package manifests**: per-package file manifests mapping path → content hash
- **String table**: interned path strings shared across manifests
- **Tree DAG**: optional serialized directory tree (for clone-file materialization)
- **Bundle index**: optional hash → offset index into pre-assembled L0 bundle file
  (stored sidecar, not in the JGrid itself)

The format is designed for:
- **mmap access**: no parsing at read time — all offsets and sizes are stored
  as absolute byte positions in the header
- **Zero-copy reads**: content blocks are returned as `&[u8]` slices directly
  from the mmap'd region
- **O(1) lookups**: Swiss Table with SIMD-optimized probe (SSE2/AVX2 on x86,
  NEON on ARM) resolves SHA-256 → file location in ~2–6 ns

---

## 2. File Layout

```
┌─────────────────────────────────────────────────┐
│ Header (256 bytes)                              │
├─────────────────────────────────────────────────┤
│ Control Words [ctrl_word_size bytes]            │
├─────────────────────────────────────────────────┤
│ Hash Slots [hash_slot_size bytes]               │
├─────────────────────────────────────────────────┤
│ Content Blocks (each 4KB-aligned)               │
├─────────────────────────────────────────────────┤
│ Manifests (variable-length, manifest_offset)    │
├─────────────────────────────────────────────────┤
│ String Table (variable-length)                  │
├─────────────────────────────────────────────────┤
│ Tree Index (optional, tree_index_offset)        │
└─────────────────────────────────────────────────┘
```

All multi-byte integers are **little-endian**.

---

## 3. Header (256 bytes)

```c
struct GridHeader {
    char     magic[9];        // "JHGR0002\0"
    uint32_t version;         // 2
    uint64_t flags;           // bit 0: has_tree_index, bit 1: has_perfect_hash
    uint64_t block_count;     // number of unique content blocks
    uint32_t manifest_count;  // number of package manifests
    uint64_t hash_seed;       // seed for Swiss Table hash function

    uint64_t ctrl_word_offset;    // absolute byte offset to control words
    uint64_t hash_slot_offset;    // absolute byte offset to hash slots
    uint64_t block_data_offset;   // absolute byte offset to first content block
    uint64_t manifest_offset;     // absolute byte offset to manifest index
    uint64_t string_table_offset; // absolute byte offset to string table

    uint64_t ctrl_word_size;      // byte size of control word array
    uint64_t hash_slot_size;      // byte size of hash slot array
    uint64_t total_block_size;    // sum of all content block sizes (before padding)
    uint64_t string_table_size;   // byte size of string table

    uint64_t tree_index_offset;   // absolute byte offset to tree index (0 if absent)
    uint32_t tree_index_count;    // number of tree index entries

    uint8_t  reserved[56];        // zeroed
};
```

**Total: 256 bytes.**

`flags` bits:
- `0x01` — Tree Index present (L1 clone-file materialization support)
- `0x02` — Perfect Hash present (optional minimal-perfect-hash layer)

---

## 4. Swiss Table (Hash Index)

A Swiss Table maps a 32-byte SHA-256 hash to `(block_offset, block_size)` in
O(1) expected time using a 16-bit control word for SIMD probing.

### 4.1 Layout

The Swiss Table has two arrays of equal length (`capacity`):

```
capacity = next_power_of_two(block_count * load_factor_reciprocal)

Default load factor: 87.5% (7/8)
capacity = next_power_of_two(block_count * 8 / 7)
```

**Control Words** (`ctrl_word_size = capacity` bytes):
Each control word is an 8-bit value encoding the low 7 bits of the hash
(bit 7 = occupancy flag):

```
bit 7: occupancy (1 = slot occupied, 0 = empty)
bits 0–6: low 7 bits of SHA-256[0]
```

Empty slots have control word `0b10000000` (0x80).

**Hash Slots** (`hash_slot_size = capacity * 7` bytes):
Each occupied slot stores a 56-bit value packed as 7 bytes (little-endian):

```
bytes 0–5:  block offset (48 bits, up to 256 TB file)
byte 6:     block size (8 bits, 256-byte granularity → up to 64 KB)
```

`block_size = slot_byte_6 * 256`

### 4.2 Lookup

```
hash = SHA-256(content)
h1   = hash[0..7] as u64
h2   = hash[7]    // byte 7
ctrl = (h1 >> 57) as u8 & 0x7F | 0x80

group_base = ((h1 >> 48) as u16 * 8) & (capacity - 1)

For each slot in the SIMD group at group_base:
    if ctrl_words[slot] == ctrl:
        // Probe: check full hash match on content block
        offset = read_u48(hash_slots[slot * 7 ..])
        size   = hash_slots[slot * 7 + 6] * 256
        read full SHA-256 from block_data[offset..offset+32]
        if SHA-256 matches:
            return (offset + 32, size - 32)  // skip the embedded hash
return None
```

### 4.3 Insertion

```
For each content block:
    1. Hash with SHA-256 → full_hash
    2. Compute h1 = full_hash[0..7], h2 = full_hash[7]
    3. ctrl = (h1 >> 57) as u8 & 0x7F | 0x80
    4. Probe for empty slot via SIMD `_mm_cmpeq_epi8` on control word group
    5. Write control word, then hash slot (offset, size/256)
    6. Write block data at block_data_offset + total_block_size, aligned to 4 KB
```

---

## 5. Content Blocks

Each block is stored at a 4 KB-aligned offset:

```
[block_data_offset] → SHA-256[0..32] | block_content[0..size-32]
                     ├── 32 bytes SHA-256 hash
                     └── block content (included for integrity verification)
```

Each block is padded to the next 4 KB boundary using zero bytes.

---

## 6. Manifests

Manifests are stored as a flat index at `manifest_offset`.

### 6.1 Manifest Index Format

```
manifest_count = number of manifests
For each manifest:
    name_len:   u32 (LE)
    name:       UTF-8 string (name_len bytes)
    version_len: u32 (LE)
    version:    UTF-8 string (version_len bytes)
    file_count: u32 (LE)
    For each file:
        path_offset: u32 (LE)  // offset into string table
        mode:        u16 (LE)  // Unix file mode (0o644, 0o755, etc.)
        content_hash: [u8; 32] // SHA-256 of content block
```

### 6.2 Lookup

Manifests are accessed via sequential scan with O(N) complexity, but `N` is
typically small (1–100 packages). For larger stores, an optional B-tree index
can be added at a future `tree_index_offset`.

---

## 7. String Table

A simple interned string table for file paths shared across manifests.

```
string_table_size: u32 (LE)  // total byte size
For each string:
    offset: u32 (LE)  // byte offset into the raw string data segment
    len:    u32 (LE)  // byte length
    string_data:     // all string bytes concatenated, no delimiters
```

Strings are referenced by their `offset` field in file entries.
Duplicates are deduplicated: the same path string always gets the same offset.

**Search**: for look-up by string content, a hash map is used in the reader
(built at deserialization time). The on-disk format is a flat array of
`(offset, len)` pairs plus the concatenated byte data.

---

## 8. Tree DAG (Optional)

If `flags & 0x01`, a serialized directory tree is stored at `tree_index_offset`.
This enables L1 clone-file materialization — pre-assembling a tmpfs-style
directory tree that can be clonefile'd into `node_modules` in a single syscall
per package.

The tree index consists of:
```
tree_index_count: u32 (LE)
For each tree:
    root_hash: [u8; 32]  // content hash of the serialized DirEntry tree
    manifest_index: u32   // index into the manifests array
```

Each `DirEntry` is serialized recursively:
```
enum DirEntry {
    File { name: String, hash: [u8; 32], mode: u16 },
    Directory { name: String, entries: Vec<DirEntry> },
}
```

Serialization (depth-first):
```
entry_type: u8 (0 = File, 1 = Directory)
name_len:   u32 (LE)
name:       UTF-8 string
if File:
    hash: [u8; 32]
    mode: u16 (LE)
if Directory:
    child_count: u32 (LE)
    children:    recursive DirEntry serialization
```

---

## 9. L0 Bundle Cache (Sidecar)

The L0 bundle cache is stored as a separate file alongside the JGrid, not inside
it. The file `bundles.jhcb` (JHol Cache Bundle) maps dependency-set hashes to
pre-assembled `node_modules/` archive files.

Format (TBD: simple tar or a flat file per hash).

---

## 10. Alignment & Padding

- Header: always at offset 0, exactly 256 bytes
- Swiss Table control words: immediately after header (offset 256)
- Content blocks: each starts at the next 4 KB boundary
- Manifests and string table: no alignment requirement (after block data)

---

## 11. Integrity & Verification

Each content block stores its own SHA-256 hash in the first 32 bytes of the
block data region. Reads should verify the hash matches the address key.
The Swiss Table's 16-bit control word provides an additional integrity check
(7 bits of the hash are encoded in the control word, giving 128:1 odds against
random false positives).

---

## 12. Version History

| Version | Changes |
|---------|---------|
| 1       | Initial prototype (unused) |
| 2       | Added 48-bit offsets, 7-byte hash slots, tree index support |

---

## 13. File Extension

`.jgrid` (convention). The magic bytes `JHGR0002\0` identify the format.

---

## 14. Performance Targets

| Operation | Target | Notes |
|-----------|--------|-------|
| Swiss Table lookup (hit) | ≤ 8 ns | SIMD probe, L1/L2 cache resident |
| Swiss Table lookup (miss) | ≤ 4 ns | SIMD empty-slot detection |
| Read content block (mmap) | ≤ 1 μs | zero-copy `&[u8]` slice |
| L0 bundle clone | ≤ 200 μs | single `clonefile` syscall per package dir |
| L1 tree clone | ≤ 50 μs | per-package tree copy |
| L2 block materialize (10 files) | ≤ 500 μs | per-package |
