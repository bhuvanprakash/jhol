# Jhol Core Innovations Design Document

**Version:** 1.0  
**Date:** May 27, 2026  
**Status:** Implementation Ready

---

## Overview

This document describes the five core innovations that make Jhol a revolutionary package manager. Each innovation is designed to be implemented without simplifying or removing any existing functionality.

---

## 1. MPT Store (Merkle Patricia Trie)

### Purpose
Git-like content-addressed object store using Merkle Patricia Trie structure for efficient dependency graph storage and retrieval.

### Architecture

```
~/.jhol/
├── objects/                    # Content-addressed storage (Git-style)
│   ├── pkg/                    # Package manifest objects
│   │   └── sha256:{hash}       # One file per package@version
│   ├── dep/                    # Dependency tree objects
│   │   └── sha256:{hash}       # One file per resolved dep graph
│   └── mft/                    # Merkle tree forests (complete installs)
│       └── sha256:{hash}       # One file per install manifest
├── refs/                       # Named references (like Git refs)
│   ├── react@18.2.0           # → sha256:{hash}
│   ├── lodash@4.17.21         # → sha256:{hash}
│   └── {project}/HEAD         # → sha256:{hash}
└── state/
    └── HEAD                    # Current state pointer
```

### Data Structures

```rust
// Merkle Patricia Trie Node Types
pub enum MPTNode {
    Leaf(MPTMerkleNode),        // Terminal node with content hash
    Extension(MPTExtensionNode), // Compressed path segment
    Branch(MPTBranchNode),     // 16-way branch for nibble paths
}

// Merkle Node - represents a package in the dependency tree
pub struct MPTMerkleNode {
    pub package_name: String,
    pub version: String,
    pub content_hash: String,   // SHA256 of actual package content
    pub metadata_hash: String,  // SHA256 of package metadata
    pub deps: Vec<String>,      // Child node hashes
    pub depth: u32,
}

// Extension Node - compressed path for efficiency
pub struct MPTExtensionNode {
    pub path_nibbles: Vec<u8>, // Compressed path segment
    pub child_hash: String,
}

// Branch Node - 16-way branch for nibble paths
pub struct MPTBranchNode {
    pub children: [Option<String>; 16], // One per nibble (0-f)
    pub value_hash: Option<String>,     // Value if path terminates here
}
```

### Key Properties
- **Selective Disclosure**: Prove membership of specific dependency without revealing entire tree
- **Efficient Lookups**: O(path_length) for exact matches, ~O(max_depth) for prefix queries
- **Automatic Deduplication**: Same content hash = same node = shared storage
- **Path Compression**: Extension nodes compress common prefixes

### Operations

```rust
impl MPTStore {
    // Insert a new dependency into the trie
    pub fn insert(&mut self, package: &PackageRef, deps: Vec<String>) -> Result<String>;

    // Lookup by package name (path in trie)
    pub fn lookup(&self, package: &str) -> Result<Option<MPTNode>>;

    // Get merkle proof for selective disclosure
    pub fn get_proof(&self, package: &str) -> Result<MerkleProof>;

    // Verify proof against root hash
    pub fn verify_proof(proof: &MerkleProof, root_hash: &str) -> bool;

    // Compute root hash of entire trie
    pub fn root_hash(&self) -> String;

    // Serialize/deserialize for persistence
    pub fn save(&self, path: &Path) -> Result<()>;
    pub fn load(path: &Path) -> Result<Self>;
}
```

### Why Merkle Patricia Trie Over Alternatives

| Structure | Lookup | Insert | Deduplication | Selective Disclosure |
|-----------|--------|--------|--------------|---------------------|
| Merkle Tree | O(log n) | O(log n) | Yes | Limited |
| Merkle DAG | O(n) | O(n) | Yes | No |
| Patricia Trie | O(path) | O(path) | No | Yes |
| **MPT (Ours)** | O(path) | O(path) | **Yes** | **Yes** |
| SQLite | O(1) | O(1) | Manual | No |

---

## 2. Content-Addressed Functions (CAF)

### Purpose
Packages identified by SHA256 of their content, not by version string. Enables true immutability and automatic cross-version deduplication.

### Architecture

```rust
// Content-Addressed Package
pub struct ContentAddressedPackage {
    pub content_hash: String,           // SHA256 of tarball content
    pub exports: Vec<ExportEntry>,       // What this package exports
    pub integrity: IntegrityMetadata,    // Registry integrity data
    pub metadata: PackageMetadata,      // Original package metadata
}

// Export Entry - what files/functions are available
pub struct ExportEntry {
    pub path: String,                    // e.g., "dist/index.js"
    pub content_hash: String,            // SHA256 of this specific file
    pub exports_type: ExportType,        // CommonJS, ESM, TypeScript
}

// Integrity Metadata - from registry
pub struct IntegrityMetadata {
    pub registry: String,                // "npmjs.org"
    pub registry_integrity: String,      // Registry's integrity field
    pub shasum: Option<String>,          // Legacy shasum if present
    pub signers: Vec<SignerInfo>,        // Who signed this artifact
}
```

### Key Properties
- **Version Agnostic**: Two packages with identical content have identical hash
- **Tamper Evidence**: Any modification changes hash → attack detected
- **Cross-Version Deduplication**: lodash@4.17.20 and 4.17.21 with 95% same code = hash overlap
- **Content Verification**: Every byte verified against stored hash

### Operations

```rust
impl ContentAddressedStore {
    // Store package by content hash
    pub fn store(&mut self, content: &[u8], metadata: PackageMetadata) -> Result<String>;

    // Retrieve by content hash
    pub fn get(&self, content_hash: &str) -> Result<Option<ContentAddressedPackage>>;

    // Verify content matches hash
    pub fn verify(&self, content_hash: &str, content: &[u8]) -> Result<bool>;

    // Find packages with similar content (for deduplication analysis)
    pub fn find_similar(&self, content_hash: &str) -> Result<Vec<String>>;

    // Export integrity report for auditing
    pub fn integrity_report(&self) -> Result<IntegrityReport>;
}
```

### Example

```
Traditional:
  lodash@4.17.20 → install → files in node_modules
  lodash@4.17.21 → install → files in node_modules (duplicated!)

CAF:
  sha256:abc123 (lodash@4.17.20 content) → stored once
  sha256:def456 (lodash@4.17.21 content)
    └─ 95% same as abc123 → only 5% new content stored
```

---

## 3. Install Provenance Log (IPL)

### Purpose
Append-only, tamper-evident log of every install operation. Certificate Transparency for packages.

### Architecture

```
~/.jhol/audit-log/
├── ipl/                         # Install Provenance Log
│   ├── log                      # Append-only log file
│   └── index/                   # Index for fast lookups
└── attestations/                # Signed attestations
    └── {timestamp}-{root_hash}  # Individual attestation files
```

### Data Structures

```rust
// Install Provenance Log Entry
pub struct IPLEntry {
    pub version: u8,                    // Log format version
    pub timestamp: u64,                 // Unix timestamp (milliseconds)
    pub sequence: u64,                  // Monotonic sequence number
    pub root_hash: String,             // MPT root hash of installed deps
    pub platform: String,               // e.g., "darwin-arm64"
    pub node_version: String,           // e.g., "20.15.0"
    pub registry: String,               // e.g., "npmjs.org"
    pub install_type: InstallType,      // Fresh | Cached | Fallback | Offline
    pub integrity_hash: String,          // SHA256 of entire install state
    pub prev_entry_hash: String,         // Hash of previous entry (chain)
    pub metadata: InstallMetadata,      // Additional context
}

// Install Type
pub enum InstallType {
    Fresh,         // Downloaded from network
    Cached,        // Used cache only
    Fallback,      // Fell back to npm/bun
    Offline,       // 100% offline install
    Update,        // Package update
    Remove,        // Package removal
}

// Merkle Proof for selective disclosure
pub struct MerkleProof {
    pub package_name: String,
    pub path_hashes: Vec<String>,       // Hashes from leaf to root
    pub path_positions: Vec<bool>,      // Left/right position at each level
    pub root_hash: String,
}
```

### Key Properties
- **Append-Only**: New entries only, never modify or delete
- **Chain Integrity**: Each entry hash includes previous entry hash (like Git)
- **Tamper Evidence**: Any modification breaks the chain
- **Selective Disclosure**: Prove specific package was installed without revealing all
- **Forensic Ready**: Answer "what happened on date X" questions

### Operations

```rust
impl ProvenanceLog {
    // Append new entry
    pub fn append(&mut self, entry: IPLEntry) -> Result<u64>;

    // Get entry by sequence number
    pub fn get(&self, sequence: u64) -> Result<Option<IPLEntry>>;

    // Get entries in time range
    pub fn query_range(&self, start: u64, end: u64) -> Result<Vec<IPLEntry>>;

    // Verify chain integrity
    pub fn verify_chain(&self) -> Result<bool>;

    // Get merkle proof for specific package in historical install
    pub fn get_package_proof(&self, sequence: u64, package: &str) -> Result<MerkleProof>;

    // Export attestation (signed statement)
    pub fn attest(&self, sequence: u64) -> Result<Attestation>;

    // Verify attestation
    pub fn verify_attestation(&self, attestation: &Attestation) -> Result<bool>;
}
```

### Example Log Entry

```json
{
  "version": 1,
  "timestamp": 1748342400000,
  "sequence": 142857,
  "root_hash": "sha256:abc123def456...",
  "platform": "darwin-arm64",
  "node_version": "20.15.0",
  "registry": "npmjs.org",
  "install_type": "Cached",
  "integrity_hash": "sha256:789xyz...",
  "prev_entry_hash": "sha256:prev123...",
  "metadata": {
    "project": "/Users/bhuvan/project",
    "lockfile_hash": "sha256:lock123...",
    "duration_ms": 12,
    "packages_count": 47
  }
}
```

---

## 4. Global Resolution Memory (GRM)

### Purpose
Persistent, local-first knowledge base of resolution results. Works 100% offline. Optional sync to any cloud storage you provide.

### Architecture

```
~/.jhol/grm/
├── database/                   # SQLite database (lightweight, optional)
│   └── grm.db
├── objects/                    # Git-style object store (primary)
│   ├── resolution/             # Resolved dependency graphs
│   │   └── sha256:{hash}
│   ├── manifest/               # Package manifests
│   │   └── sha256:{hash}
│   └── tree/                   # Directory trees
│       └── sha256:{hash}
├── refs/
│   ├── {ecosystem}/{package}@{version}  # Point to latest resolution
│   └── local/                           # Local overrides
├── sync/                      # Sync configuration (user-provided)
│   └── config.toml
└── cache/                     # Hot cache for fast lookups
    └── lru.cache
```

### Data Structures

```rust
// Resolution Record
pub struct ResolutionRecord {
    pub id: u64,
    pub package_spec: String,          // Original package specifier
    pub resolved_version: String,       // Resolved version
    pub content_hash: String,           // SHA256 of package content
    pub dependency_hashes: Vec<String>, // Hashes of all deps
    pub platform: String,               // Platform constraints
    pub node_version: String,           // Node version
    pub resolution_hash: String,        // SHA256 of entire resolution
    pub cached_at: u64,                 // When this was resolved
    pub hit_count: u64,                 // Times this was reused
    pub source: ResolutionSource,       // Where this came from
}

// Resolution Source
pub enum ResolutionSource {
    Network,        // Fetched from registry
    Cache,          // Retrieved from local cache
    GRM,            // Retrieved from Global Resolution Memory
    Fallback,       // Used npm/bun fallback
}

// GRM Query
pub struct GRMQuery {
    pub package_spec: String,
    pub platform: Option<String>,
    pub node_version: Option<String>,
    pub freshness: Option<FreshnessRequirement>,
}

// Sync Configuration (user provides their own storage)
pub struct SyncConfig {
    pub enabled: bool,
    pub provider: SyncProvider,         // S3, GCS, Azure, Git, Rsync
    pub endpoint: Option<String>,        // Custom endpoint
    pub bucket: Option<String>,          // Bucket name
    pub prefix: Option<String>,          // Path prefix
    pub credentials: Option<Credentials>, // User provides these
}
```

### Key Properties
- **Local-First**: Works 100% without network
- **User-Owned Cloud**: You provide storage (S3, git, rsync)
- **Learning**: Each successful resolution improves future hits
- **Privacy**: No data sent anywhere by default
- **Cross-Platform**: Same resolution works across machines if synced

### Operations

```rust
impl GlobalResolutionMemory {
    // Query for cached resolution
    pub fn query(&self, q: &GRMQuery) -> Result<Option<ResolutionRecord>>;

    // Store new resolution
    pub fn store(&mut self, record: ResolutionRecord) -> Result<()>;

    // Invalidate stale entries
    pub fn prune(&mut self, max_age: Duration) -> Result<u64>;

    // Get resolution stats
    pub fn stats(&self) -> ResolutionStats;

    // Sync to/from user-provided storage
    pub fn sync_push(&self, config: &SyncConfig) -> Result<()>;
    pub fn sync_pull(&mut self, config: &SyncConfig) -> Result<u64>;

    // Merge remote GRM (for team sharing)
    pub fn merge(&mut self, remote: &GlobalResolutionMemory) -> Result<MergeResult>;
}
```

---

## 5. Instant Lock (iLock)

### Purpose
Pre-compute the entire dependency graph before touching the network. The lockfile IS the install plan.

### Architecture

```
.jhol.lock                    # Jhol's native lockfile
.jhol/
└── state/
    ├── ilock/               # Instant lock data
    │   ├── graph.bin        # Binary dependency graph
    │   └── manifest.bin    # Fast-load manifest
    └── profiles/            # Platform-specific locks
```

### Data Structures

```rust
// Instant Lock - complete install plan
pub struct InstantLock {
    pub version: u8,                    // Lock format version
    pub root_hash: String,              // MPT root hash
    pub platform: String,               // Target platform
    pub node_version: String,           // Target Node version
    pub packages: Vec<LockedPackage>,   // All packages to install
    pub dependency_graph: BinaryGraph,   // Compact binary representation
    pub checksum: String,               // Checksum of entire lock
    pub created_at: u64,                 // Creation timestamp
}

// Locked Package
pub struct LockedPackage {
    pub name: String,                    // Package name
    pub version: String,                // Resolved version
    pub content_hash: String,           // SHA256 of content
    pub integrity: String,               // Registry integrity
    pub url: String,                    // Tarball URL
    pub deps: Vec<String>,              // Dependency names
    pub optional_deps: Vec<String>,     // Optional deps
    pub peer_deps: Vec<PeerDep>,        // Peer deps
    pub build_metadata: BuildMetadata,  // Build-specific data
}

// Binary Graph - compact representation
pub struct BinaryGraph {
    pub nodes: Vec<GraphNode>,          // Packed node data
    pub edges: Vec<GraphEdge>,          // Packed edge data
    pub strings: StringTable,           // Shared string interning
}

// Graph Node (packed)
pub struct GraphNode {
    pub name_offset: u32,               // Offset in string table
    pub version_offset: u32,
    pub content_hash_offset: u32,
    pub dep_start: u32,                 // Start index in edges
    pub dep_count: u16,
    pub flags: u8,                      // optional, dev, peer
}
```

### Key Properties
- **Pre-Computed**: Full graph known before any network request
- **Binary Format**: 10x faster to parse than JSON
- **Verified**: Hash links graph to actual content
- **Parallel Fetch**: All tarballs can be fetched simultaneously
- **Offline Capable**: With GRM, can resolve 100% from cache

### Operations

```rust
impl InstantLock {
    // Create new ilock from package.json
    pub fn create(project: &Project) -> Result<Self>;

    // Load from file
    pub fn load(path: &Path) -> Result<Self>;

    // Save to file
    pub fn save(&self, path: &Path) -> Result<()>;

    // Verify lock matches package.json
    pub fn verify(&self, package_json: &PackageJson) -> Result<bool>;

    // Get packages to fetch (for parallel download)
    pub fn get_fetch_list(&self) -> Vec<FetchTask>;

    // Apply lock (link packages from store)
    pub fn apply(&self, store: &GlobalStore) -> Result<InstallResult>;

    // Convert to/from package-lock.json
    pub fn to_package_lock(&self) -> Result<PackageLock>;
    pub fn from_package_lock(lock: &PackageLock) -> Result<Self>;
}
```

---

## Integration Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                         JHOL INSTALL FLOW                            │
├──────────────────────────────────────────────────────────────────────┤
│                                                                       │
│  1. PARSE package.json                                                │
│         │                                                             │
│         ▼                                                             │
│  2. QUERY GRM for cached resolutions                                   │
│         │                                                             │
│         ├── Cache Hit → Use cached resolution                         │
│         │                                                             │
│         └── Cache Miss → Resolve from registry                        │
│         │                                                             │
│         ▼                                                             │
│  3. CREATE Instant Lock (pre-compute entire graph)                    │
│         │                                                             │
│         ▼                                                             │
│  4. STORE resolution in GRM (for future)                              │
│         │                                                             │
│         ▼                                                             │
│  5. FETCH packages (parallel, using content hashes)                   │
│         │                                                             │
│         ▼                                                             │
│  6. STORE in MPT (content-addressed, deduplicated)                     │
│         │                                                             │
│         ▼                                                             │
│  7. LINK packages (hardlinks from store to project)                  │
│         │                                                             │
│         ▼                                                             │
│  8. WRITE .jhol.lock (and package-lock.json for compatibility)       │
│         │                                                             │
│         ▼                                                             │
│  9. APPEND to Install Provenance Log                                  │
│         │                                                             │
│         ▼                                                             │
│  10. DONE                                                             │
│                                                                       │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Backwards Compatibility

| Feature | Implementation |
|---------|----------------|
| `package-lock.json` | Generated from iLock, fully compatible |
| `node_modules` | Optional (PnP or physical) |
| npm/yarn/bun compatibility | Same lockfile format |
| `.jhol.lock` | Native Jhol format |

---

## Zero node_modules Option

Jhol supports PnP-style resolution without physical `node_modules`:

```javascript
// .jhol-loader.js (generated)
module.paths = [
  // Custom paths that Jhol manages
  process.env.JHOL_VIRTUAL_STORE,
  ...module.paths
];

// Or use with Node.js --require flag:
// node --require ./jhol-loader.js app.js
```

For native module packages that MUST have physical files, Jhol falls back to selective physical installation.

---

## File Structure

```
crates/jhol-core/src/
├── mpt_store.rs              # Merkle Patricia Trie implementation
├── content_addressed.rs      # Content-addressed package storage
├── provenance_log.rs         # Install Provenance Log
├── grm/
│   ├── mod.rs               # GRM main module
│   ├── store.rs             # Resolution storage
│   ├── query.rs              # Query engine
│   └── sync.rs               # Optional sync to cloud
├── instant_lock.rs           # iLock format and operations
├── install.rs                # Modified to use new components
└── lib.rs                    # Updated exports
```

---

## Dependencies

No new external dependencies. Uses existing:
- `sha2` for SHA256
- `hex` for hex encoding
- `parking_lot` for synchronization
- `dashmap` for concurrent maps
- `serde` for serialization

---

## Implementation Order

1. **mpt_store.rs** - Foundation (MPT, node types, persistence)
2. **content_addressed.rs** - Package storage by content hash
3. **provenance_log.rs** - Append-only audit log
4. **grm/store.rs** - Resolution memory storage
5. **grm/query.rs** - Query engine
6. **grm/sync.rs** - Optional cloud sync
7. **instant_lock.rs** - Lockfile format
8. **install.rs** - Modified to use all components
9. **Tests and benchmarks**

---

## Security Considerations

- All content verified against stored SHA256 before use
- IPL chain prevents tampering with historical records
- GRM can be encrypted at rest (user provides key)
- Sync operations use user-provided credentials only
- No data exfiltrated without explicit opt-in

---

## Performance Targets

| Operation | Target | Current (baseline) |
|-----------|--------|-------------------|
| GRM lookup | < 1ms | N/A (new) |
| MPT insert | < 5ms | N/A |
| MPT lookup | < 1ms | N/A |
| iLock create | < 50ms | N/A |
| iLock apply | < 100ms | N/A |
| IPL append | < 5ms | N/A |

---

## References

- Merkle Patricia Trie: Ethereum, Git internal structures
- Certificate Transparency: RFC 6962
- Content Addressing: Git, IPFS, Nix
- Instant Lock: Derived from modern package manager lockfiles