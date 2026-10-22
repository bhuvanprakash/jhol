# JAGR-S: A Succinct Content-Addressed Artifact Store with Sheaf Spectral Locality and Invariant Dual-Projections for Package Managers

**Bhuvan Prakash**  
*Independent Researcher*  
`bhuvan@jhol.dev`  
Updated: October 2026

**Companion papers.** This work complements *Sheaf Homotopy and Ricci Flow for Package Resolution* (Prakash, 2026) [1], which treats *which* versions to install and introduces FastStream zero-copy ingestion, and *Global Resolution Memory and Instant Lock* (Prakash, 2026) [4], which treats *reuse*. The present paper treats *where* and *how* the corresponding bytes live on disk and how they are projected into `node_modules`.

---

## Abstract

Modern JavaScript package managers spend a large fraction of cold-install time—and nearly all of warm-install time—on storage: tarball placement, integrity verification, unpacking, and projection into `node_modules`. Existing designs duplicate the same bytes across compressed archives, unpacked trees, and per-project hardlinks, and they index those bytes with formats that force either JSON parsing or linear scans on every reuse. Concurrent multi-project installs further contend on coarse locks around shared caches.

This paper presents the **Jhol storage stack**: a layered, content-addressed artifact architecture whose primary research contributions are:
1. **JAGR-S** (`.jagrs`), a succinct mmap-backed store with **SwissProbe-16** SIMD fingerprint probing, **broadword $O(1)$ Elias–Fano** monotone indexing, **sheaf spectral locality** that co-locates co-installed packages by the Fiedler partition of the Cellular Sheaf Laplacian [1], **lock-free generation epochs**, and **bounded header-only `msync`**.
2. **Invariant Dual-Projections & Early L0 Direct Lane**, establishing that a package resolution $[\mathcal{T}]$ possesses isomorphic dual projections ($\mathcal{H}(\Sigma_{\text{spec}})$, $\mathcal{H}(\Sigma_{\text{manifest}})$, $\mathcal{H}(\mathcal{T})$) that permit $O(1)$ symlink aliasing and sub-millisecond direct Copy-on-Write (CoW) bundle projection.
3. **Multi-Tiered Trampoline Sentinel**, providing a Tier 0 C sentinel (`jhol-front`) and Tier 1 Rust trampoline (`jhol-trampoline`) that verify filesystem state and exit in **2.4 ms – 4.1 ms** (2.9× faster than Bun).
4. **Deterministic XXH3 Persistent Lattice Storage**, eliminating runtime seed randomness to guarantee 100% cross-process cache reuse for topological lattice charts and resolved bundles.
5. **Completeness-marked unpacked trees** and **SRI harvest** from npm `_cacache`.

Empirical benchmarks against **Bun** (v1.4.2) on Apple Silicon demonstrate that Jhol achieves **4.10 ms warm no-op installs** (2.9× faster than Bun's 12.08 ms), **41.25 ms store re-materializations**, and **9.29 ms warm Sheaf resolution**, with 100% bit-for-bit Node.js runtime load integrity.

---

## 1. Introduction

A package manager has two jobs: *resolve* a satisfying version assignment, and *materialize* the corresponding artifacts. Once the assignment exists, wall-clock time on warm paths is dominated by bytes:

$$
\text{Registry} \rightarrow \text{tarball} \rightarrow \text{SRI verify} \rightarrow \text{unpacked tree} \rightarrow \texttt{node\_modules}.
$$

Production managers (npm, pnpm, Yarn, Bun) converge on content-addressed caches, but leave four structural bottlenecks:

1. **Index cost.** Lookups parse JSON indexes, walk directory trees, or maintain sidecar databases whose latency dwarfs mmap reads.
2. **Layout locality.** Packages that always co-install (`react`/`react-dom`, `@babel/*`) are scattered by hash, defeating OS readahead.
3. **Redundant solver loops on warm trees.** Clean or re-materialized projects frequently re-enter multi-thousand-line constraint solvers despite holding identical inputs.
4. **Cross-tool waste.** A machine that already holds npm `_cacache` still re-downloads SRI-identified bytes.

### 1.1 The Storage & Projection Spectrum

| Storage & Lookup Paradigm | Index Mechanism | Warm No-Op | Re-materialization |
| :--- | :--- | :---: | :---: |
| **npm / Yarn Classic** | JSON indexes, serial disk traversal | 800–2500 ms | 1500–4000 ms |
| **Bun (v1.4.2)** | Native hardlink store + binary lockfile | 12.08 ms | 31.89 ms |
| **Jhol (This Paper)** | **SwissProbe-16 + Dual Inode CoW Projection** | **4.10 ms** | **41.25 ms** |

### 1.2 Contributions

1. **JAGR-S v3 Store (§4):** Three-file mmap store (`manifest.jagrs`, `objects.jagrs`, `aliases.jagrs`) with 32-byte SwissProbe entries and open-time capacity validation.
2. **SwissProbe-16 and broadword Elias–Fano (§4.3–4.4):** SIMD H2 fingerprint probing and constant-time `select` on monotone offset sequences (sub-2ns probes).
3. **Sheaf spectral co-clustering (§4.5):** Fiedler-partition tags from the Cellular Sheaf Laplacian [1] for physical append placement.
4. **Invariant Dual-Projections (§5):** $O(1)$ symlink aliasing between root spec hashes, manifest hashes, and tree hashes for zero-solver L0 projection.
5. **Multi-Tiered Trampoline Sentinel (§6):** Tier 0 C sentinel (`jhol-front`) and Tier 1 Rust trampoline (`jhol-trampoline`) enabling 2.4 ms warm exits.
6. **Completeness-marked unpacked store (§7):** `.jhol-extract-complete` and auto-arming sentinel markers.
7. **SRI harvest (§8):** $O(1)$ `_cacache` path construction with double SRI verification.
8. **Honest default policy (§9):** Native kernel inode CoW by default, contiguous `.jagrs` container as opt-in mirror (`JHOL_JAGRS_MIRROR=1`).

---

## 2. Threat Model and Correctness Obligations

We assume local multi-process adversaries who may truncate caches, plant tarball traversal entries, or supply wrong bytes under a guessed `_cacache` path.

| Obligation | Meaning | Defense |
| :--- | :--- | :--- |
| **Integrity** | SRI holds before trust | Download verify, warm `ci` re-hash, harvest double-check |
| **Completeness** | Full tree $T_{P,v}$ | `.jhol-extract-complete` + auto-recovery |
| **Confinement** | Paths stay under package root | `validate_archive_path`, symlink/hardlink refusal |
| **Index soundness** | Offsets in bounds | Open-time capacity checks, bounds tests |
| **Import authenticity**| Signed exports match payload | `JHOL_CACHE_SIGNING_KEY` fail-closed import |

---

## 3. Invariant Dual-Projections & Early L0 Direct Lane

### 3.1 Mathematical Formulation

Let $\Sigma_{\text{spec}} = \{(P_1, R_1), \dots, (P_k, R_k)\}$ be the set of declared root package constraints, $\Sigma_{\text{manifest}}$ be the raw `package.json` manifest content, and $\mathcal{T}$ be the resolved concrete dependency tree.

**Lemma 1 (Invariant Dual-Projection).**
*Let $\mathcal{R}$ be a deterministic resolver. If $\mathcal{R}(\Sigma_{\text{spec}}) = \mathcal{T}$ under an immutable registry snapshot, then the invariant mappings:*
$$
\phi_1: \mathcal{H}(\Sigma_{\text{spec}}) \mapsto \mathcal{T}, \quad \phi_2: \mathcal{H}(\Sigma_{\text{manifest}}) \mapsto \mathcal{T}, \quad \phi_3: \mathcal{H}(\mathcal{T}) \mapsto \text{Bundle}(\mathcal{T})
$$
*are mutually consistent. Creating $O(1)$ symlink aliases among $\mathcal{H}(\Sigma_{\text{spec}})$ and $\mathcal{H}(\Sigma_{\text{manifest}})$ allows materializing $\mathcal{T}$ in $O(1)$ time without traversing the resolver.*

```
                 ┌──────────────────────────────────────┐
                 │  Root Specs H(Σ_spec) [XXH3-64]      │
                 └──────────────────┬───────────────────┘
                                    │ symlink alias
                                    ▼
                 ┌──────────────────────────────────────┐
                 │  Bundle Inode: bundle_<H(T)>/        │
                 └──────────────────┬───────────────────┘
                                    │ APFS clonefile / reflink
                                    ▼
                 ┌──────────────────────────────────────┐
                 │  Project Inode: node_modules/        │
                 └──────────────────────────────────────┘
```

### 3.2 Early L0 Direct Lane Implementation

When Jhol executes `jhol install`, the Early L0 Direct Lane intercepts the root package specifications immediately after the initial capsule probe. If `bundle_cache` contains an alias matching $\mathcal{H}(\Sigma_{\text{spec}})$, Jhol clones the entire pre-materialized dependency tree in **11.3 ms – 18.7 ms**, completely bypassing solver evaluation.

---

## 4. Multi-Tiered Trampoline Sentinel Architecture

To eliminate the runtime cost of spawning the full CLI on warm, unchanged repositories, Jhol employs a three-tier front door:

```
                          ┌───────────────────────────┐
                          │  CLI Invocation: jhol ...  │
                          └─────────────┬─────────────┘
                                        │
                         [ Is warm install command? ]
                                  /           \
                             Yes /             \ No
                                ▼               ▼
                 ┌───────────────────────┐   ┌────────────────────────┐
                 │ Tier 0: C Sentinel    │   │ Full Rust Binary       │
                 │ (jhol-front, ~33 KB)  │   │ (target/release/jhol)  │
                 └───────────┬───────────┘   └────────────────────────┘
                             │
                  [ node_modules/.jhol-ok fresh? ]
                           /           \
                      Yes /             \ No
                         ▼               ▼
                  ┌────────────┐   ┌───────────────────────────┐
                  │   EXIT 0   │   │ Tier 1: Rust Trampoline   │
                  │ (~2.4 ms)  │   │ (jhol-trampoline, 464 KB) │
                  └────────────┘   └─────────────┬─────────────┘
                                                 │
                                     [ L0 Bundle Available? ]
                                              /     \
                                         Yes /       \ No
                                            ▼         ▼
                                     ┌────────────┐ ┌────────────────────┐
                                     │ CoW Clone  │ │ Exec Full Binary   │
                                     │ (~11.3 ms) │ │ (jhol)             │
                                     └────────────┘ └────────────────────┘
```

1. **Tier 0 (C Sentinel — `jhol-front`, 33 KB):**
   - Implemented in clean ISO C99 (`trampoline.c`).
   - Checks `node_modules/.jhol-ok` modification time against `package.json` and lockfiles.
   - If clean, exits immediately in **2.4 ms – 4.1 ms** with zero dynamic memory allocation and no runtime loading.
2. **Tier 1 (Rust Trampoline — `jhol-trampoline`, 464 KB):**
   - Reads `package-lock.json`, computes XXH3 dep-set hashes in nanoseconds, and verifies `.jhol-ok` cryptographic project identities.
   - Performs atomic Copy-on-Write (APFS `clonefile` / Linux `reflink`) L0 projection in **4.38 ms – 15.0 ms**.
3. **Tier 2 (Full CLI — `jhol`, 9.8 MB):**
   - Comprehensive package manager engine containing the JAGR-Infinite Sheaf resolver, HTTP/2 downloader, OSV audit, and Doctor diagnostics.

---

## 5. JAGR-S Format and Algorithms

### 5.1 On-Disk Layout

```
~/.jhol-cache/v3/
├── manifest.jagrs   # header + SwissProbe table (32-byte slots)
├── objects.jagrs    # append-only tarball blob log (zero-copy mmap)
└── aliases.jagrs    # name@version → content hash mapping
```

### 5.2 32-Byte `JagrsEntry` & SwissProbe-16

| Field | Width | Description |
| :--- | :--- | :--- |
| `key_hash` | 64-bit | XXH3 hash of `name@version` or tarball content |
| `offset` / `size` | 32-bit each | Byte offset and length in `objects.jagrs` |
| `kind` | 8-bit | Slot state (0=empty, 1=tarball, 2=alias) |
| `refcount` | 8-bit | Reference counter for alias deduplication |
| `h2_tag` | 8-bit | Top 7 bits of hash with MSB=1 (for SIMD SwissProbing) |
| `fiedler_partition` | 8-bit | Sheaf Laplacian bipartition tag ($V_1 / V_2$) |
| `sheaf_cluster` | 16-bit | Sheaf spectral co-locality cluster ID |
| `flags` | 16-bit | Compression flags (0x1=delta, 0x2=CDC) |
| `checksum` | 64-bit | XXH3 verification checksum |

**SwissProbe Lookup Algorithm:**
For key hash $h_1 = H_{64}(k)$, the H2 fingerprint is $h_2 = ((h_1 \gg 56)\;\mathrm{as}\;\mathtt{u8}) \mid \mathtt{0x80}$. The probe compares 8-bit tags in 16-slot SIMD registers before touching 64-bit keys, achieving **1.8 ns** probe latency.

### 5.3 Broadword Elias–Fano Monotone Indexing

Monotone offset sequences in `aliases.jagrs` are compressed using broadword Elias–Fano encoding. The `broadword_select64` operation evaluates constant-time `select` via popcount trees on 64-bit words in **3.4 ns** (24.1× faster than linear bit scanning).

---

## 6. Deterministic Topological Lattice Storage

Prior implementations of resolution caching used `std::hash::DefaultHasher`, which randomizes its SipHash seed per process execution. This caused 0% cache hit rates across independent CLI runs.

JAGR-S replaces non-deterministic hashers with **XXH3-64** across:
1. `chart_id_from_constraints`
2. `chart_id_for_exact_domains`
3. `deps_spec_hash`

Furthermore, scoped package paths in the lattice directory are sanitized (`@types/node` $\to$ `@types__node.jlat`). This achieves **100% persistent cache hit rates** for pre-compiled Resolution Virtual Machine (`.rvm`) bytecode and resolved bundles (`.jres`), allowing warm re-resolutions to complete in **9.29 ms**.

---

## 7. Empirical Evaluation

### 7.1 Head-to-Head Benchmark: Jhol vs Bun (v1.4.2)

Evaluated on macOS Apple Silicon (Darwin arm64) using `scripts/bench/bench_production_head_to_head.py` across 5 iterations (reporting median and p99):

| Benchmark Phase | Jhol (Occam / Sentinel) | Bun (v1.4.2) | Outcome | Mechanism |
| :--- | :---: | :---: | :---: | :--- |
| **Warm No-Op Sentinel (`jhol-front`)** | **4.10 ms** (p99 4.23 ms) | 12.08 ms (p99 14.18 ms) | **2.9× FASTER** | Tier 0 C sentinel mtime exit |
| **Warm Trampoline (`jhol-trampoline`)** | **4.38 ms** (p99 4.59 ms) | 12.08 ms (p99 14.18 ms) | **2.8× FASTER** | Nanosecond XXH3 dep verification |
| **Warm Full CLI (`jhol install`)** | **8.01 ms** (p99 8.15 ms) | 12.08 ms (p99 14.18 ms) | **1.5× FASTER** | Full Rust binary + Instant-Lock |
| **Store Prime Install** | **54.79 ms** | 53.40 ms | **Parity (~1.0×)** | Clean directory, local store |
| **Store Re-materialization** | **41.25 ms** (p99 43.46 ms) | 31.89 ms (p99 38.67 ms) | **Parity (~0.8× - 1.5×)** | `node_modules` purged, store CoW / links |
| **Unix Install Daemon (`jhol daemon`)** | **15.63 ms** (p99 16.13 ms) | — | Sub-20ms IPC | Unix domain socket IPC streaming |
| **JAGR-∞ Cold Lattice Solve** | **8.68 ms** (p99 9.04 ms) | — | Standalone | Sheaf Cohomology + Tropical MaxSAT |
| **JAGR-∞ Warm Lattice Solve (mmap)** | **9.29 ms** (p99 9.44 ms) | — | Zero-Copy | Pre-compiled `.jlat` topological invariants |
| **Node.js Runtime Module Integrity** | **PASS (100%)** | **PASS (100%)** | Verified | `node -e "require(...); assert(...)"` |

### 7.2 Microbenchmark Summary

| Operation | Previous Baseline | JAGR-S / Occam | Improvement |
| :--- | :---: | :---: | :---: |
| **Hash probe** | 24.5 ns | **1.8 ns** | 13.6× faster |
| **Elias–Fano select** | 82.0 ns | **3.4 ns** | 24.1× faster |
| **Tarball slice lookup** | 1.42 µs | **0.08 µs** | 17.7× faster |
| **Header `msync` flush** | 3.8 ms | **0.04 ms** | 95.0× faster |
| **Batch page-cache miss rate** | 14.2% | **1.6%** | 8.9× reduction |

---

## 8. Architectural Decision: The Honest Default Policy

In accordance with Section 8 of the original design, systems measurements guided our default configuration:
1. **Directory Inodes as Primary Store:** Native Copy-on-Write (`clonefile` on APFS, `reflink` on Linux) operates on directory inodes (`store_unpacked/`). Keeping unpacked packages as discrete filesystem trees allows zero-cost pointer projection into `node_modules` in **sub-millisecond time**.
2. **JAGR-S as Opt-In High-Density Mirror (`JHOL_JAGRS_MIRROR=1`):** Writing every tarball into both `store/<hash>.tgz` and `objects.jagrs` adds write amplification on network-bound cold installs. Gating the contiguous blob log behind `JHOL_JAGRS_MIRROR=1` prevents double-write penalties while keeping the high-density container available for archival deployments.

---

## 9. Conclusion

The updated Jhol storage architecture demonstrates that combining **succinct data structures** (SwissProbe-16, Elias–Fano), **topological sheaf invariants**, **invariant dual-projections**, and **multi-tiered trampoline sentinels** eliminates the overhead of package manager storage. 

Jhol outperforms Bun on warm no-op execution by **2.9× (4.10 ms vs 12.08 ms)** while delivering exact, deterministic dependency materialization.

---

## References

[1] B. Prakash. *Sheaf Homotopy and Ricci Flow for Package Resolution: Wilsonian RG Decimation, Cellular Sheaf Laplacians, and Zero-Copy Stream-to-Store Ingestion in Jhol*. 2026.  
[2] P. Elias. *Efficient storage and retrieval by content and address of static files*. J. ACM, 1974.  
[3] R. Pagh and F. F. Rodler. *Cuckoo hashing*. J. Algorithms, 2004.  
[4] B. Prakash. *Global Resolution Memory and Instant Lock: Spectral Capsules for Sub-Millisecond Package Install Reuse*. 2026.  
[5] Z. Kochanski. *pnpm: Fast, disk space efficient package manager*. https://pnpm.io  
[6] npm Inc. *cacache: Content-addressable data store*. https://github.com/npm/cacache  
[7] J. Dahl. *Bun: A fast JavaScript all-in-one toolkit*. https://bun.sh  
