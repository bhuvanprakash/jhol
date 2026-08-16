# Jhol Pipeline — The JGrid Instant Layer

> **Sub-0.5ms package materialization through content-addressed, memory-mappable, clone-ready storage.**
> A new storage format for package management that eliminates the tarball as the canonical cached artifact.

---

## 1. Philosophy

Every package manager today treats the **tarball** (`.tgz`) as the canonical cached artifact. This fundamental design choice forces two expensive operations on EVERY install — even when the tarball is already cached locally:

1. **Gzip decompression** — CPU-bound, ~30-80ms per package
2. **Tar extraction** — CPU + disk I/O, ~50-150ms per package

These operations happen **every single time** you run `npm install`, `yarn`, `pnpm install`, or even a cached `jhol install`.

**The core insight: tarballs are a transport format, not a storage format.**

JGrid inverts this. Instead of archiving content into transport-oriented containers (tars), we store content as **individual, content-addressed, uncompressed file blocks** in a memory-mappable container format. Package structure is represented as a **merkle tree DAG** of content hashes — the same structure that powers git and IPFS.

**The revolution:** Installing a cached package is no longer "decompress + extract + copy." It is:

```
read manifest → lookup content block offsets in Swiss Table → clonefile from mmap → done
```

Zero CPU for decompression. Zero I/O for extraction. One syscall per directory tree.

---

## 2. The Three-Tier Materialization Pyramid

Packages are **never** installed via per-file reflink. Instead, we build a pyramid of pre-assembled artifacts:

```
                    ╔══════════════════════════╗
                    ║  L0: Clone-Ready Bundle  ║  ← <0.1ms
                    ║  entire node_modules/     ║
                    ║  single clonefile() call  ║
                    ╚══════════════════════════╝
                             ⬆
                    ╔══════════════════════════╗
                    ║  L1: Per-Package Tree    ║  ← <3ms (50 pkgs)
                    ║  pre-assembled directory  ║
                    ║  trees/ dir on disk      ║
                    ╚══════════════════════════╝
                             ⬆
                    ╔══════════════════════════╗
                    ║  L2: JGrid Content Blocks ║  ← <100ms
                    ║  mmap'd, content-addressed║
                    ║  raw file bytes           ║
                    ╚══════════════════════════╝
                             ⬆
                    ╔══════════════════════════╗
                    ║  Source: npm tarball     ║  ← <300ms (cold)
                    ║  registry fetch          ║     once per file
                    ╚══════════════════════════╝
```

### L2 — JGrid Content Blocks

Raw file content stored in a `.jgrid` file. Each file is a **content-addressed block** — identified by its SHA-256 hash, accessible via a Swiss Table-based hash table (SIMD-accelerated probe).

- **Location:** `~/.jhol-cache/store.jgrid` (single file, append-only)
- **Content:** raw, uncompressed file bytes (page-aligned for zero-copy)
- **Index:** Swiss Table with ARM NEON / SSE2 16-way parallel probe
- **Lookup:** O(1) expected, ~30ns with SIMD
- **Deduplication:** global — same byte content across any packages = stored once

### L1 — Pre-Assembled Directory Trees

A complete package directory (`package.json`, `index.js`, `lib/`, etc.) materialized to disk as a real directory. This is the **instant-materialization artifact** — once a tree exists, cloning it to `node_modules/` is a single kernel syscall.

- **Location:** `~/.jhol-cache/trees/<sha256(pkg@version)>/`
- **Creation:** built from JGrid content blocks (reading each file by hash, writing to disk)
- **Materialization:** `clonefile()` on macOS APFS, `FICLONE` ioctl on Linux Btrfs/XFS
- **Timing:** First build ~80ms; subsequent clonefile ~0.05ms

### L0 — Clone-Ready Bundle

An entire `node_modules/` directory with all resolved packages, pre-assembled and ready for instant cloning. Created lazily on first project install, cached forever.

- **Location:** `~/.jhol-cache/bundles/<sha256(sorted_lockfile_entries)>/`
- **Creation:** clone each package tree into a staging directory
- **Materialization:** single `clonefile()` syscall for the entire bundle
- **Timing:** ~0.05ms TOTAL (one syscall, kernel COW, zero data copy)

---

## 3. The JGrid Binary Format — Full Specification

The `.jgrid` file is a memory-mappable container optimized for zero-copy access with SIMD-accelerated hash lookup. It is the canonical storage artifact — it replaces both the `.tgz` tarball AND the unpacked directory in the cache.

### 3.1 File Layout

```
┌──────────────────────────────────────────────────────────────┐
│ HEADER (256 bytes, page-aligned)                              │
│  [magic: "JHGR0002\0"] [version: u32] [flags: u64]          │
│  [block_count: u64] [manifest_count: u32] [hash_seed: u64]   │
│  section offsets and sizes for all regions                   │
│  [tree_index_offset: u64] [tree_index_count: u32]             │
│  reserved: [56 bytes]                                        │
├──────────────────────────────────────────────────────────────┤
│ CONTROL BYTES (Swiss Table metadata, ≈6% overhead)           │
│  1 byte per 16-slot hash table group                         │
│  Each byte: 7-bit markers + empty/occupied/deleted flags     │
├──────────────────────────────────────────────────────────────┤
│ HASH TABLE (open-addressing Swiss Table, 16-slot groups)     │
│  Each slot: [hash_prefix: u16] [block_offset: u40]           │
│            [block_size: u24] = 10 bytes/slot                 │
│  2× block_count capacity (50% max load factor)               │
│  SIMD probe: load 16 slots → compare hash_prefixes in 1 op   │
├──────────────────────────────────────────────────────────────┤
│ CONTENT BLOCKS (raw file data, 4KB page-aligned)             │
│  Block 0: [page align] bytes of "export function map..."     │
│  Block 1: [page align] bytes of "{\"name\":\"lodash\"..."    │
│  ...                                                         │
│  UNCOMPRESSED — zero CPU to read; pages served from kernel   │
│  page cache via mmap. Content identity = SHA-256 of bytes.   │
├──────────────────────────────────────────────────────────────┤
│ STRING TABLE (null-terminated, deduplicated UTF-8 paths)     │
│  "package.json\0index.js\0lib/index.js\0README.md\0..."     │
│  Sorted for binary search prefix matching                    │
├──────────────────────────────────────────────────────────────┤
│ MANIFEST INDEX (package@version → file entries)              │
│  count: u32 | entries: [(name_off, ver_off, file_count,      │
│    tree_hash[32], files[(path_off, mode, content_hash[32])])]│
│  Sorted by package@version for binary search                 │
├──────────────────────────────────────────────────────────────┤
│ TREE DAG INDEX (directory tree hash → serialized DirEntry)   │
│  count: u32 | entries: [(tree_hash[32], offset, size)]       │
│  Each DirEntry: [{kind, name, hash_or_child_tree_hash}]      │
│  Optional section — built lazily, enables clonefile materialize│
└──────────────────────────────────────────────────────────────┘
```

### 3.2 Header Structure

```rust
#[repr(C, packed)]
struct GridHeader {
    magic: [u8; 9],             // "JHGR0002\0"
    version: u32,               // 2
    flags: u64,                 // bit 0: has_tree_index, bit 1: has_perfect_hash, etc.
    
    // Block counts
    block_count: u64,           // total unique content blocks
    manifest_count: u32,        // total package manifests
    hash_seed: u64,             // seed for Swiss Table hash function
    
    // Section offsets (from file start)
    ctrl_word_offset: u64,
    hash_slot_offset: u64,
    block_data_offset: u64,
    manifest_offset: u64,
    string_table_offset: u64,
    
    // Section sizes
    ctrl_word_size: u64,
    hash_slot_size: u64,
    total_block_size: u64,      // sum of all content block sizes
    string_table_size: u64,
    
    // Optional tree index
    tree_index_offset: u64,     // 0 if absent
    tree_index_count: u32,      // 0 if absent
    
    reserved: [56; u8],         // zeroed
}
```

Total header: 256 bytes (9 + 4 + 8 + 8 + 4 + 8 + 6×8 + 5×8 + 8 + 4 + 56 = 256).

### 3.3 Swiss Table Hash Table

The hash table is the heart of the format. It enables O(1) lookup from SHA-256 hash → content block location in the same file.

**Group structure:** 16 slots per group. Groups are contiguous in memory.

**Slot layout (10 bytes each):**
```rust
struct HashSlot {
    hash_prefix: u16,   // first 16 bits of SHA-256 — early rejection
    block_offset: u40,  // byte offset within the .jgrid file (max 1TB file)
    block_size: u24,    // size in bytes (max 16MB per block — more than enough)
}
```

**Control bytes (1 per group = 1 per 16 slots):**
```rust
struct CtrlWord {
    // 7 bits for the 7-bit "stamp" of each slot in the group
    // 1 bit for empty flag of each slot
    // Packed into 8 bytes per group using bitwise operations
}
```

**Lookup algorithm:**
```
1. hash = SHA-256(content_to_find)
2. group_index = (hash >> 16) % num_groups
3. stamp = (hash >> 57) & 0x7f  // 7-bit stamp from top bits
4. LOAD control_word[group_index / 8]  // 8 bytes for the group
5. SIMD: XMM = load_16_slots_packed(hash_slot_base + group_index * 16 * 10)
6. Compare each slot's hash_prefix with hash[0..2] (16 at once)
7. For matches: verify full hash if needed by comparing hash bytes
8. Return (block_offset, block_size)
```

On ARM NEON: 2 instructions (vld1q + vceqq) to compare 16 prefixes.
On x86 SSE2: similar with movdqa + pcmpeqb.
Portable fallback: 64-bit bitwise ops (like Go 1.24 Swiss Table implementation).

**Load factor:** 50% max (2× slots as blocks). At 50%, average probes ≈ 1.05.

### 3.4 Content Blocks

Each block is a contiguous region of raw file bytes. Blocks are:
- **Page-aligned** (start at 4KB boundaries relative to the file)
- **Uncompressed** — stored as-is (SHA-256 hash is the identity)
- **Read via mmap** — kernel page cache serves the bytes; no read() syscalls

Alignment to 4KB pages ensures:
- No page-crossing for any block (max 16MB, but page aligned start)
- Direct `mmap` + offset access without worrying about partial pages
- `copy_file_range` syscalls work on full pages (optimized in kernel)

### 3.5 Manifest Index

Each package version is represented as a sorted entry in the manifest index:

```rust
struct ManifestEntry {
    name_offset: u32,        // into string table
    version_offset: u32,     // into string table
    file_count: u32,         // number of files
    tree_hash: [u8; 32],     // SHA-256 of the directory tree DAG (0 if not built)
    files: [FileEntry; file_count],
}

struct FileEntry {
    path_offset: u32,        // into string table
    mode: u16,               // file permissions (0o644, 0o755, etc.)
    content_hash: [u8; 32],  // SHA-256 → hash table key
}
```

The manifest index is sorted by `(name, version)` for binary search.

### 3.6 Tree DAG (Directory Tree)

A directory tree is a content-addressed merkle tree where each node is:

```rust
enum DirEntry {
    File {
        name: String,          // from string table
        content_hash: [u8; 32], // SHA-256 of file content
        mode: u16,
    },
    Directory {
        name: String,
        entries: Vec<DirEntry>,
        // Hash of this directory = SHA-256(concat(sorted(entries)))
    },
    Symlink {
        name: String,
        target: String,
    },
}
```

The tree is serialized recursively, depth-first. Each directory has a computed SHA-256 hash based on its sorted entries. This hash is the key in the Tree DAG index and is stored in the `tree_hash` field of `ManifestEntry`.

This structure enables:
- **Content-addressed directory verification** — the tree hash is computed from content, so verifying a directory is O(1) hash comparison
- **Deduplication of directory structures** — two packages with identical directory layouts share the same tree hash
- **Incremental tree building** — changing one file changes only the hash of its containing directory, not the entire tree

### 3.7 String Table

All path strings, package names, and version strings are stored in a single null-terminated, deduplicated string table. Strings are sorted for binary search prefix matching.

Example content:
```
\0package.json\0index.js\0lib/\0lib/index.js\0README.md\0lodash\04.17.21\0chalk\05.3.0\0
```

The initial `\0` ensures offset 0 is an empty string (safety sentinel).

---

## 4. Pipeline Architecture

### 4.1 Ingestion Pipeline (Tarball → JGrid)

```
┌──────────┐   ┌──────────┐   ┌───────────┐   ┌──────────┐
│ Download  │   │Decompress│   │Tar Extract │   │ Hash &   │
│ .tgz from │ → │ (gzip)   │ → │ (stream)   │ → │ Dedup    │
│ registry  │   │          │   │            │   │ (SHA-256)│
└──────────┘   └──────────┘   └───────────┘   └──────────┘
                                                    │
                                                    ▼
                                              ┌──────────┐
                                              │ Write to │
                                              │ JGrid +  │
                                              │ Tree DAG │
                                              └──────────┘
```

**Streaming model:** The tarball is NOT fully downloaded before processing. We stream:
1. HTTP response → gzip decompress → tar entry reader → per-file SHA-256 → JGrid block append

This means memory usage is O(largest_file), not O(tarball_size).

Per file:
1. Compute SHA-256 of file bytes
2. Check if hash exists in Swiss Table (dedup)
3. If new: append bytes to JGrid content blocks section (page-aligned)
4. Record (path, mode, content_hash) in package manifest

After all files processed:
1. Build directory tree DAG from manifest
2. Write tree to `~/.jhol-cache/trees/<tree_hash>/` (materialized on disk)
3. Update JGrid manifest index with tree_hash

### 4.2 Materialization Pipeline (JGrid → node_modules)

```
┌──────────┐   ┌────────────┐   ┌────────────┐   ┌──────────┐
│ Resolve   │   │  Check     │   │   Check    │   │  Check   │
│ dep set → │ → │ Bundle     │ → │ Per-Pkg    │ → │ JGrid    │
│ hash      │   │ Cache (L0) │   │ Trees (L1) │   │ (L2)     │
└──────────┘   └────────────┘   └────────────┘   └──────────┘
                    │                 │               │
           ┌────────┘                 │               │
           ▼                          ▼               ▼
    ┌──────────────┐       ┌──────────────┐  ┌──────────────┐
    │ clonefile()  │       │clonefile()   │  │Build trees   │
    │ bundle→nm    │       │trees→staging │  │from JGrid    │
    │ ←0.05ms      │       │ ←0.05ms/pkg  │  │ blocks→trees │
    └──────────────┘       └──────┬───────┘  │ ←80ms        │
                                  │           └──────┬───────┘
                                  ▼                  ▼
                          ┌──────────────────────────────┐
                          │  Save bundle for next time   │
                          │  clonefile(bundle, nm)       │
                          │  ←0.05ms TOTAL               │
                          └──────────────────────────────┘
```

### 4.3 The L0/L1/L2 Strategy in Detail

**L0 Hit (Clone-Ready Bundle exists):** — **<0.1ms**
```rust
let dep_hash = compute_dep_set_hash(&resolved_packages);
let bundle_path = cache_dir.join("bundles").join(&dep_hash);
if bundle_path.exists() {
    clone_directory(&bundle_path.join("node_modules"), "./node_modules")?;
    // DONE in ~0.05ms
}
```

**L1 Hit (Trees exist, no bundle):** — **<3ms (50 packages)**
```rust
let staging = create_temp_dir();
for pkg in &resolved_packages {
    let tree_hash = jgrid.lookup_tree_hash(pkg.name, pkg.version)?;
    let tree_path = cache_dir.join("trees").join(&tree_hash);
    if tree_path.exists() {
        clone_directory(&tree_path, &staging.join("node_modules").join(pkg.name))?;
        // ~0.05ms per package
    }
}
save_bundle(&staging, &dep_hash)?;   // Cache for next time
clone_directory(&staging.join("node_modules"), "./node_modules")?;
```

**L2 Hit (JGrid exists, no trees):** — **<100ms**
```rust
let staging = create_temp_dir();
for pkg in &resolved_packages {
    let manifest = jgrid.read_manifest(pkg.name, pkg.version)?;
    let tree_path = cache_dir.join("trees").join(&manifest.tree_hash);
    
    // Build the tree directory from JGrid content blocks
    build_tree_from_manifest(&jgrid, &manifest, &tree_path)?;
    
    // Now clone into staging
    clone_directory(&tree_path, &staging.join("node_modules").join(pkg.name))?;
}
save_bundle(&staging, &dep_hash)?;
clone_directory(&staging.join("node_modules"), "./node_modules")?;
```

**Full Cold (no JGrid, need tarball):** — **<300ms per package**
```rust
let tarball_bytes = download_tarball(pkg.tarball_url)?;
let manifest = ingest_tarball(&tarball_bytes, &jgrid)?;
// Build trees, save bundle, clone
```

---

## 5. Cutting-Edge Techniques Incorporated

| Technique | Source | Year | What it does for JGrid |
|-----------|--------|------|----------------------|
| **Swiss Table + SIMD probing** | Google Abseil, Go 1.24 maps | 2017/2025 | 16-slot hash table groups probed in parallel with NEON/SSE → ~30ns lookups vs ~200ns traditional |
| **Perfect hash for static region** | maph (sub-100ns KV store) | 2024 | 80% content blocks use collision-free perfect hash → O(1) worst-case, no probes |
| **APFS directory clonefile** | Apple APFS kernel | 2016+ | Entire package tree cloned with ONE syscall → atomic, zero-byte-copy materialization |
| **FICLONE/FICLONERANGE** | Linux Btrfs/XFS | 2013+ | Same as clonefile on Linux → portable zero-copy tree cloning |
| **Copy-on-Abundant-Write** | BetrFS (USENIX FAST) | 2020 | Pre-assembled trees never physically copied; kernel COW lazily handles divergence |
| **VectorCDC SIMD chunking** | USENIX FAST | 2025 | 21-46× faster content-defined chunking via AVX-512, applicable to tarball parsing |
| **Merkle tree DAG** | Git/IPFS | 2005/2015 | Content-addressed directory trees enable O(1) verification + dedup of directory structures |
| **Zero-copy mmap'd I/O** | OS kernel (standard) | — | Content bytes served from kernel page cache via mmap; no read() syscalls on hot path |
| **MADV_POPULATE_READ** | Linux 5.14+ | 2021 | Pre-fault all needed pages synchronously before install → zero page faults during materialization |
| **GogetaFS LFP mapping** | USENIX FAST | 2025 | Merge content hash with filesystem metadata → one I/O instead of two |
| **xxh3 hash** | Cyan4973/xxHash | 2019 | ~50GB/s hashing for dep-set hash computation (not cryptographic) |
| **Cache-line aligned slots** | maph, standard practice | — | 64-byte aligned hash slots prevent false sharing in CPU cache |

---

## 6. Sub-0.5ms Timing Analysis

### 6.1 L0 Hit (The Common Case — Repeat Install)

After the first install of a project, every subsequent `jhol install` takes:

| Step | Time | Detail |
|------|------|--------|
| Compute dep set hash | 0.01ms | xxh3 of sorted `(name, version)` tuples |
| Check bundle cache | 0.001ms | `bundles/<hash>/node_modules` stat + exists |
| clonefile syscall | 0.05ms | APFS directory clone — kernel copies extent map only |
| **Total** | **~0.06ms** | **← 60μs** |

The 0.05ms for clonefile is the kernel overhead of:
1. Creating the destination directory inode
2. Copying the source directory's extent map
3. Incrementing refcounts on shared blocks
4. Committing the APFS transaction

Zero bytes of file content are read or written. The entire operation is metadata-only.

### 6.2 L1 Hit (Different Project, Same Packages)

When trees exist for all packages but no bundle for this specific set:

| Step | Time (50 packages) | Detail |
|------|--------------------|--------|
| Lookup tree hash for each | 0.05ms | binary search in manifest index |
| clonefile each tree → staging | 2.5ms (50 × 0.05ms) | per-package clone |
| Save as bundle | 0.05ms | clone staging → bundles/<hash>/ |
| clonefile bundle → node_modules | 0.05ms | final materialization |
| **Total** | **~2.65ms** | **← still under 3ms** |

On APFS with directory-level clonefile, the staging step can be eliminated by cloning all trees directly into a single staging directory and using one more directory clone for the final step.

### 6.3 L2 Hit (First Install of a Package)

When JGrid exists but this specific package's tree hasn't been built yet:

| Step | Time (50 packages) | Detail |
|------|--------------------|--------|
| Read manifest for each | 0.1ms | Swiss Table lookup for manifest offset |
| Build tree from JGrid blocks | 80ms (first time) | iterate files, write each to disk |
| clonefile trees → staging | 2.5ms | per-package clone |
| Save bundle | 0.05ms | clone staging → bundle |
| clonefile bundle → nm | 0.05ms | final |
| **Total** | **~83ms** | **tree building dominates — happens once per pkg ever** |

### 6.4 Full Cold (First Install Ever)

| Step | Time | Detail |
|------|------|--------|
| Download tarballs | 200ms per pkg (parallel) | HTTP from registry |
| Decompress gzip + extract tar | 80ms per pkg | CPU-bound, once per unique file |
| SHA-256 each file | 10ms per pkg | ~500MB/s hashing |
| Write JGrid blocks | 20ms per pkg | page-aligned append |
| Build trees + materialize | 100ms | from JGrid |
| **Total** | **~400ms** | **download dominant — but only once per unique file** |

### 6.5 Comparison with Current System

| Scenario | Current jhol (warm) | JGrid L0 | Speedup |
|----------|-------------------|----------|---------|
| Repeat install (5 pkgs) | 152ms | **0.06ms** | **2533×** |
| Different project (5 pkgs) | 152ms | **2.65ms** | **57×** |
| First install (5 pkgs, cold) | 1390ms | **~400ms** | **3.5×** |
| Binary size | 6.3MB | +1.2MB | — |

---

## 7. Crate Structure

```
crates/jhol-pipeline/
├── Cargo.toml
├── PLAN.md                    # This document
└── src/
    ├── lib.rs                 # Public API: JGridPipeline struct
    │
    ├── format/                # JGrid binary format
    │   ├── mod.rs             # Re-exports
    │   ├── header.rs          # GridHeader: read/write/validate (256 bytes)
    │   ├── swiss_table.rs     # SIMD Swiss Table: hash→block location
    │   ├── manifest.rs        # Package manifest encoder/decoder
    │   ├── string_table.rs    # Deduplicated path string table
    │   └── tree_dag.rs        # Merkle directory tree DAG
    │
    ├── store/                 # On-disk store management
    │   ├── mod.rs
    │   ├── grid_store.rs      # Mmap'd JGrid: open, read, write blocks
    │   ├── builder.rs         # Incremental JGrid builder
    │   ├── tree_cache.rs      # ~/.jhol-cache/trees/ management
    │   └── bundle_cache.rs    # ~/.jhol-cache/bundles/ management
    │
    ├── ingester/              # Tarball → JGrid conversion
    │   ├── mod.rs
    │   ├── from_tarball.rs    # Stream tarball → extract → hash → JGrid
    │   └── tree_builder.rs    # Build directory DAG from manifest
    │
    ├── materialize/           # JGrid → node_modules
    │   ├── mod.rs
    │   ├── cloner.rs          # Platform clonefile abstraction
    │   └── installer.rs       # L0/L1/L2 materialization strategy
    │
    └── spec.md                # Format specification reference
```

**Dependencies:** Only `sha2`, `hex`, `memmap2`, `serde`, `xxhash-rust` — all already present in jhol-core's dependency tree.

**`Cargo.toml`:**
```toml
[package]
name = "jhol-pipeline"
version = "0.1.0"
edition = "2021"
description = "JGrid instant materialization pipeline for Jhol"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
sha2 = "0.10"
hex = "0.4"
memmap2 = "0.9"
xxhash-rust = { version = "0.8", features = ["xxh3"] }

[dev-dependencies]
tempfile = "3"
criterion = "0.5"

[[bench]]
name = "materialization"
harness = false
```

---

## 8. Integration into jhol-core

### 8.1 Install Flow Modification

Current `install.rs` flow:
```
resolve → download tarballs → store/<hash>.tgz → decompress → extract → 
store_unpacked/<hash>/ → link_package_from_store → node_modules/
```

New flow:
```
resolve → TRY pipeline:
            grid_pipeline.materialize(deps, node_modules)
            - L0: single clonefile, done
            - L1: clonefile per pkg → save bundle → done
            - L2: build trees from JGrid → L1 → done
            - MISS: fall through
          FALLBACK: traditional path (unchanged)
          BACKGROUND: ingest tarballs into JGrid for next time
```

### 8.2 API Surface

```rust
// lib.rs — Public API

pub struct JGridPipeline {
    grid_store: GridStore,
    tree_cache: TreeCache,
    bundle_cache: BundleCache,
}

impl JGridPipeline {
    /// Open or create the JGrid pipeline at the given cache directory
    pub fn new(cache_dir: &Path) -> Result<Self>;
    
    /// Materialize a set of resolved packages into node_modules
    /// Returns Ok(true) if materialized via pipeline, Ok(false) if cache miss (caller should fall back)
    pub fn materialize(
        &self,
        packages: &[ResolvedPackage],
        node_modules: &Path,
    ) -> Result<bool>;
    
    /// Ingest a downloaded tarball into the JGrid store
    pub fn ingest_tarball(
        &self,
        pkg_name: &str,
        pkg_version: &str,
        tarball_bytes: &[u8],
    ) -> Result<ManifestHash>;
    
    /// Check if a bundle exists for the given dependency set
    pub fn has_bundle(&self, dep_set_hash: &str) -> bool;
    
    /// Get the JGrid store (for direct block operations)
    pub fn store(&self) -> &GridStore;
}
```

### 8.3 Zero-Regret Guarantee

The pipeline is **purely additive** — it adds a new fast path while keeping the existing path intact:

1. `materialize()` returns `Ok(false)` if no cache hit at any tier
2. Caller falls back to the existing tarball→extract→link path
3. After fallback, `ingest_tarball()` populates the JGrid for next time
4. On the next install, the pipeline succeeds

This means:
- Zero regression risk — existing behavior is completely preserved
- Gradual adoption — benefits grow as the JGrid cache warms up
- Atomic fallback — no partial installs from pipeline failures

---

## 9. Implementation Phases

### Phase 0: Foundation (Current Phase)
- Create crate structure and workspace entry
- Implement `format/` modules: Header, SwissTable, StringTable, Manifest, TreeDAG
- Implement `GridStore` (mmap read/write)
- Implement `GridBuilder` (incremental construction)
- Unit tests for each format component

### Phase 1: Store Layer
- Implement `TreeCache` — materialized tree management
- Implement `BundleCache` — clone-ready bundle management
- Implement `Cloner` — platform clonefile abstraction (APFS, FICLONE, copy fallback)
- Integration tests for store operations

### Phase 2: Ingestion
- Implement `FromTarball` — stream tarball bytes → extract → hash → JGrid
- Implement `TreeBuilder` — build directory tree DAG from manifest
- Wire into jhol-core's download path (post-download background ingest)

### Phase 3: Materialization
- Implement `Installer` — L0/L1/L2 materialization strategy
- Wire into `install.rs` — `try_materialize()` before traditional path
- Wire post-fallback ingest

### Phase 4: Performance Optimization
- SIMD Swiss Table probes (NEON, SSE2, portable)
- Perfect hash for static content region
- Pre-faulting with MADV_WILLNEED/MADV_POPULATE_READ
- Bundle pre-computation on background thread
- Criterion benchmarks for sub-0.5ms verification

### Phase 5: Polish
- GC for orphaned trees/blocks
- `jhol cache prune --pipeline` command
- Store integrity verification on open
- Cross-platform testing (Linux Btrfs, macOS APFS)

---

## 10. Benchmarking and Verification Plan

### 10.1 Microbenchmarks (Criterion)

| Benchmark | Target | What it measures |
|-----------|--------|-----------------|
| `swiss_table_lookup` | <50ns per lookup | SHA-256 hash → block offset |
| `ingest_tarball_lodash` | <100ms | Stream tarball → JGrid |
| `build_tree_lodash` | <80ms | Build directory tree from Manhattan |
| `clonefile_single_pkg` | <0.1ms | clonefile a 50-file package tree |
| `clonefile_bundle` | <0.1ms | clonefile a 5000-file bundle |
| `materialize_l0` | <0.1ms | Full L0 materialization |
| `materialize_l1_50` | <3ms | L1 for 50 packages |
| `string_table_lookup` | <50ns | Path → offset in string table |

### 10.2 Integration Tests

| Test | Method |
|------|--------|
| JGrid round-trip | Write manifest → read back → verify files match |
| Tree DAG consistency | Build tree → compute hash → rebuild from hash → diff |
| Bundle materialization | Create bundle → clone → verify files match originals |
| Platform clonefile | Clone tree → verify files exist, same content, COW |
| Streaming ingestion | Download tarball → ingest → verify SHA-256 of extracted = predicted |
| Fallback guarantee | Artificially fail pipeline → verify traditional path works |

### 10.3 System-Level Verifications

| Metric | Current | Target |
|--------|---------|--------|
| Warm install (5 packages, repeat) | 152ms | **<0.5ms** |
| Warm install (50 packages, repeat) | ~800ms | **<3ms** |
| First install (5 packages, cold) | 1390ms | **<500ms** |
| Binary size | 6.3MB | +<1.5MB |
| Disk usage (lodash × 5 versions) | 5 tarballs + 5 unpacked | unique files only |
| Integrity guarantee | SHA-256 per package | SHA-256 per file |
| `package-lock.json` compatibility | full | full (unchanged) |

---

## 11. What Makes This Truly Revolutionary

1. **No tarball as storage format** — JGrid eliminates gzip decompression and tar extraction from the install path entirely. The `.tgz` is only a transport format, downloaded from the registry and immediately consumed into content-addressed blocks.

2. **Single-syscall materialization** — A `clonefile()` of a pre-assembled directory tree beats per-file operations by orders of magnitude. The kernel does the work atomically in O(tree_size) metadata updates, not O(file_count × file_size) data copies.

3. **Global content deduplication** — Same file across different packages or versions is stored exactly once. `lodash@4.17.21` and `lodash@4.17.22` that share 90% of files use 90% less cache storage. No current package manager does this at the file-content level.

4. **Content-addressed directory trees** — Directory structures are verified by their own SHA-256 hash, not iterated. Verifying that a cloned tree is correct is O(1) hash comparison, not O(files) stat calls.

5. **Passive cache building** — The JGrid cache grows organically from usage. No background daemon, no pre-seeded binary bundle, no deployment infrastructure. Install a package once and it's cached forever in instant-materialization form.

6. **Sub-millisecond installs** — 0.06ms for the common case (repeat install) is faster than Node.js can `stat("node_modules")`. The bottleneck shifts from I/O to the JavaScript runtime startup.

7. **No copying competitors** — npm stores tarballs. Yarn stores tarballs. pnpm stores tarballs + hardlink farms. Bun stores tarballs + pre-extracted cache. JGrid is the first format designed from the ground up for content-addressed, clonefile-based, sub-millisecond materialization.

---

## 12. Future Directions

### 12.1 Content-Addressed Network Protocol
Once the JGrid store is populated, packages can be fetched from a peer-to-peer content network using SHA-256 hashes as content identifiers. This enables:
- Offline install of any package whose content has ever been seen
- LAN sharing of content blocks between developers
- Registry-independent package verification

### 12.2 Virtual Filesystem Mount
With the JGrid format supporting mmap-based zero-copy access, `node_modules` could be served via a FUSE filesystem backed directly by the JGrid file. This would eliminate even the clonefile syscall — `require("lodash")` would read bytes directly from the kernel page cache via the FUSE mount.

### 12.3 Differential Version Updates
When updating from `lodash@4.17.21` to `4.17.22`, instead of downloading the full tarball, download only the content blocks whose SHA-256 hashes differ between versions. The manifest diff tells us exactly which files changed.

### 12.4 Content-Aware GC
The global content store grows monotonically. GC can be content-aware:
- Files referenced by at least one installed package are kept
- Files referenced by no installed package but less than 30 days old are kept
- Eviction is block-level, never whole-package

---

*"The tarball is transport. The content is the cache."*
