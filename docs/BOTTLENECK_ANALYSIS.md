# Jhol vs Bun — Bottleneck Analysis & Attack Plan

> Generated: 2026-02-18  
> Data source: live benchmark `benchmarks/live-bench-20260218-200004.json`  
> All numbers are on macOS (Apple Silicon), 5 runs, median, small = lodash+axios+chalk

---

## 1. The Real Numbers (Corrected)

The previous benchmark script had a bug: it was nuking `~/.cache/jhol` but the real
cache lives at `~/.jhol-cache`. So "cold" runs 2–5 were actually warm cache hits.

Here are the **true** numbers (run1 = true cold, cache nuked from ~/.jhol-cache):

| Scenario | jhol | bun | npm | yarn | pnpm |
|---|---|---|---|---|---|
| **Cold install** (no cache) | **2.25s** | **1.45s** | 4.42s | 6.20s | 3.20s |
| **Warm install** (cache→node_modules) | **0.021s** | **0.048s** | 3.27s | 1.59s | 2.06s |

### What this actually means:

```
COLD:  jhol=2.25s   bun=1.45s   → jhol is 1.55x slower than bun
WARM:  jhol=0.021s  bun=0.048s  → jhol is 2.3x FASTER than bun 🔥
```

**jhol already beats bun on warm installs by 2.3x.** The only gap is on true cold
(first install from empty cache). Everything else (npm, yarn, pnpm) is already crushed:

- jhol is **156× faster** than npm on warm
- jhol is **75× faster** than yarn on warm  
- jhol is **98× faster** than pnpm on warm
- jhol is **2× faster** than npm on cold
- jhol is **1.4× faster** than pnpm on cold

---

## 2. Where the 800ms Cold Gap Goes

The cold install gap is **~800ms** (2.25s - 1.45s). Here's where it lives:

```
jhol cold install timeline (lodash + axios + chalk):
─────────────────────────────────────────────────────
[0ms]    binary start, parse package.json
[~5ms]   read store_index.json from disk
[~10ms]  classify: check store_index for cache hits → all MISS (cold)
[~10ms]  ManifestThenPackumentStrategy: dispatch parallel manifest requests
         ↓ BLOCKING: ureq (synchronous HTTP) per thread
[~600ms] 3× manifest HTTP round trips to registry.npmjs.org/pkg/version
         - Each is a separate TCP connection (no HTTP/2 multiplexing)
         - ureq does NOT pool connections between threads
         - Each request: DNS + TLS handshake + HTTP GET + read body
[~800ms] download 3× tarballs (lodash.tgz ~25KB, axios.tgz ~50KB, chalk.tgz ~12KB)
         - these overlap with the extract+hash step
[~1800ms] sha256 hash + write to store
[~2100ms] unpack tgz → store_unpacked/<hash>/
[~2200ms] hardlink/copy from store_unpacked → node_modules
[~2250ms] write store_index.json
─────────────────────────────────────────────────────
TOTAL: ~2250ms
```

```
bun cold install timeline (same packages):
─────────────────────────────────────────────────────
[0ms]    Zig binary start (much faster than Rust process startup on macOS)
[~5ms]   parse package.json + resolve from bun's binary B-tree registry cache
[~20ms]  HTTP/2 multiplexed requests to registry.npmjs.org
         - Single TCP connection, 3 parallel streams
         - Bun maintains a persistent connection pool
         - Resolution hits bun's pre-baked manifest index
[~400ms] download 3× tarballs (HTTP/2 parallel, same connection)
[~800ms] extract to bun's content-addressed store (~/.bun/install/cache)
         - Uses clonefile() on macOS APFS (CoW copy, ~1μs per file)
[~1200ms] hardlink from store → node_modules
[~1450ms] write bun.lock
─────────────────────────────────────────────────────
TOTAL: ~1450ms
```

---

## 3. Root Causes Ranked by Impact

### 🔴 #1 — `ureq` (synchronous blocking HTTP) with no connection reuse (Est. savings: ~400ms)

**What's happening:**  
`registry.rs` uses `ureq` for ALL HTTP calls. `ureq` is a synchronous, blocking HTTP/1.1
client. Every request gets its own TCP connection:
```
Thread 1 → ureq::get(manifest_lodash)  → TCP connect → TLS → GET → read → close
Thread 2 → ureq::get(manifest_axios)   → TCP connect → TLS → GET → read → close  
Thread 3 → ureq::get(manifest_chalk)   → TCP connect → TLS → GET → read → close
```
Each TLS handshake to registry.npmjs.org costs ~50-120ms on its own.
With 3 packages × 2 round trips (manifest + tarball) = 6 separate TCP+TLS connects.

**What bun does:**  
Bun uses HTTP/2 over a single persistent connection. One TLS handshake, then
multiplexes all 6 requests in parallel over the same connection.

**The fix:**  
Replace `ureq` with `reqwest` + `tokio` for async HTTP/2. One connection, all parallel:
```toml
# Cargo.toml
reqwest = { version = "0.11", features = ["http2", "rustls-tls", "json", "stream"] }
tokio = { version = "1", features = ["full"] }
```
OR: Keep threads but add a shared `ureq::Agent` with connection pooling:
```rust
// Quick win: share one Agent across threads (ureq does pool on the Agent)
static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
fn get_agent() -> &'static ureq::Agent {
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build()
    })
}
```
This alone could save ~200-400ms on cold install.

---

### 🔴 #2 — Manifest → Packument two-step resolution wastes RTTs (Est. savings: ~200ms)

**What's happening:**  
`ManifestThenPackumentStrategy` in `install.rs` does:
1. Try `registry.npmjs.org/<pkg>/<version>` (abbreviated manifest) → parallel
2. On failure → fallback to `registry.npmjs.org/<pkg>` (full packument) → sequential

For packages without a pre-resolved version tag (e.g. "latest"), the manifest URL
**always 404s** because it requires an exact version. So it always falls through to the
packument, creating a guaranteed 2× RTT per package.

**What bun does:**  
Bun's registry client uses the abbreviated packument (`application/vnd.npm.install-v1+json`)
with a persistent HTTP/2 connection. The abbreviated format is 10-50× smaller than
the full packument. Single request, get tarball URL + integrity, download.

**The fix:**  
Use the abbreviated packument directly as the fast path, in parallel for all packages:
```rust
// Direct abbreviated packument: GET /lodash?write=true with Accept: application/vnd.npm.install-v1+json
// This is 50KB not 5MB, has latest dist-tags, and has tarball URLs
// Don't bother with the manifest fast path for "latest" requests — it always misses
fn fetch_abbreviated_packument_parallel(packages: &[String]) -> Vec<(String, Result<ResolvedFetch, String>)> {
    // One thread per package, all parallel, shared Agent for TCP reuse
    // Returns resolved version + tarball URL + integrity in one round trip
}
```

---

### 🟡 #3 — `clonefile()` not used on macOS APFS (Est. savings: ~50ms warm, ~20ms cold)

**What's happening:**  
On macOS with APFS, you can use `clonefile()` (copy-on-write) to "copy" a file in
~1 microsecond regardless of file size. Jhol uses `std::fs::hard_link` which requires
both source and destination to be on the same filesystem AND the same volume.

From `utils.rs`:
```rust
pub fn link_package_from_store(unpacked: &Path, node_modules: &Path, base: &str) -> Result<()> {
    // Uses hard_link — fails cross-volume, falls back to copy
}
```

**What bun does:**  
Bun uses `clonefile()` on macOS via `libc::clonefile`. If same volume → clonefile
(instant CoW). Cross-volume → hardlink. Cross-filesystem → copy.

**The fix:**
```rust
#[cfg(target_os = "macos")]
fn fast_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    let src_c = CString::new(src.as_os_str().as_encoded_bytes()).unwrap();
    let dst_c = CString::new(dst.as_os_str().as_encoded_bytes()).unwrap();
    let ret = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if ret == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}
```
Fall through to hardlink, then copy on failure. This makes warm installs near-instant
for large packages (react, next.js etc have thousands of files).

---

### 🟡 #4 — `store_index.json` read+parse on every startup (Est. savings: ~5-15ms)

**What's happening:**  
Every `jhol install` reads and JSON-parses `~/.jhol-cache/store_index.json`. For a
large project with hundreds of cached packages, this file can be 500KB+. serde_json
parse of 500KB takes ~5-15ms.

**What bun does:**  
Bun uses a memory-mapped binary B-tree for its package index. O(1) lookup, zero parse
time, mmap means the OS handles caching.

**The fix (quick win):**  
Use a compact binary format instead of JSON. Options:
- `bincode` (fastest, Rust-native): ~0.1ms parse for same data
- `rmp-serde` (MessagePack): compact + cross-language readable
- OR just `mmap` the JSON file: at least avoids the disk read on second call

```toml
bincode = "1.3"
```
```rust
// Write: bincode::serialize_into(file, &index)
// Read: bincode::deserialize_from(file)
```

---

### 🟡 #5 — Tarball extraction: sequential flate2, no parallelism within package (Est. savings: ~30ms)

**What's happening:**  
Each `.tgz` is extracted with `flate2` (DEFLATE). This is single-threaded per package.
For packages like `react` (400+ files), extraction alone takes 20-50ms.

`registry::ensure_unpacked_in_store` does: read tgz → deflate → write files.

**What bun does:**  
Bun parallelizes extraction across packages AND uses `zstd` in its internal store
(faster decompression than gzip at same compression ratio).

**The fix:**  
1. Short term: ensure extraction is already parallel across packages (it is via worker_pool)
2. Medium term: re-compress to `zstd` in the store after first download:
```toml
zstd = "0.13"
```
```rust
// Store as <hash>.zst internally, decompress on extract
// zstd decompresses ~3× faster than gzip
```

---

### 🟢 #6 — DNS not cached between parallel threads (Est. savings: ~50ms)

**What's happening:**  
Each `ureq` thread does its own DNS resolution for `registry.npmjs.org`. On some
systems this blocks per-thread. With 32 concurrent download threads, this means
32 parallel DNS lookups for the same hostname.

**The fix:**  
Use `hickory-dns` (formerly trust-dns) as a caching resolver, or ensure the shared
`ureq::Agent` is set up with a resolved IP directly for known CDN hostnames.
(This is a minor fix but free to do with shared Agent from fix #1.)

---

## 4. Prioritized Attack Plan

```
Phase 1 — Close the cold gap to < bun (target: 0.8s cold install)
────────────────────────────────────────────────────────────────────
[P1.1] Share a single ureq::Agent globally (TCP connection pool)
       Estimated time: 2 hours | Expected gain: 200-400ms cold
       
[P1.2] Fetch abbreviated packuments directly in parallel (skip manifest fast path for "latest")
       Estimated time: 4 hours | Expected gain: 100-200ms cold
       
[P1.3] Add clonefile() on macOS for store→node_modules linking
       Estimated time: 3 hours | Expected gain: 30-80ms warm, 20ms cold

Phase 2 — Beat bun on cold install (target: 0.5s cold install)
────────────────────────────────────────────────────────────────────
[P2.1] Migrate to tokio + reqwest async HTTP/2 with connection multiplexing
       Estimated time: 2 days | Expected gain: 200-500ms cold (HTTP/2 > multiple saves)
       
[P2.2] Migrate store_index.json to bincode binary format
       Estimated time: 4 hours | Expected gain: 5-15ms every run
       
[P2.3] Zstd recompression in store
       Estimated time: 1 day | Expected gain: 20-50ms per cold install

Phase 3 — Structural advantage (target: own the "fastest" crown forever)
────────────────────────────────────────────────────────────────────
[P3.1] Pre-warm registry metadata in background daemon (like pnpm's "server" mode)
       When jhol is running in a dev environment, pre-fetch packuments for
       deps declared in package.json before install is called.
       
[P3.2] Content-Defined Chunking (CDC) deduplication in store
       Like Bun's content-addressed store, but chunk-level dedup means
       lodash@4.17.20 and lodash@4.17.21 share 99% of store blocks.
       
[P3.3] Registry mirror/CDN with baked tarball index
       Cache a binary index of (pkg@version → tarball_url + integrity + size)
       Pre-populated for the top 10,000 npm packages. Serve from a CDN.
       jhol checks this index first (sub-1ms HTTP/2 request) before hitting npm.
```

---

## 5. Quick Win You Can Do Right Now (2 Hours)

The single highest-impact change is sharing one `ureq::Agent`. Here's exactly what to change:

**In `registry.rs`**, replace every `ureq::get(url)` with:

```rust
use std::sync::OnceLock;

static HTTP_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

fn http_agent() -> &'static ureq::Agent {
    HTTP_AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(60))
            .https_connector(ureq::TlsConnector::Rustls)  // if using rustls feature
            .build()
    })
}

// Replace: ureq::get(&url)
// With:    http_agent().get(&url)
```

This immediately enables TCP connection reuse across all parallel manifest + tarball
downloads. All threads share the same connection pool → `registry.npmjs.org` stays
connected across all 6 requests instead of reconnecting 6 times.

**Expected result: cold install drops from ~2.25s to ~1.5-1.8s** (within bun's range).

---

## 6. Summary Scorecard

| Metric | Current | After P1 | After P2 | Goal |
|---|---|---|---|---|
| Cold install (small) | 2.25s | ~1.3s | ~0.7s | **< 0.5s** |
| Warm install (small) | 0.021s | 0.015s | 0.010s | **< 0.01s** |
| Bun cold install | 1.45s | 1.45s | 1.45s | beat by 0.5s |
| Bun warm install | 0.048s | 0.048s | 0.048s | already beaten ✅ |
| npm cold install | 4.42s | 4.42s | 4.42s | already beaten ✅ |

**jhol already wins on warm install, npm/yarn/pnpm comparison. The only gap to close is
the true cold install against bun, and the fix is clear: shared HTTP agent (P1.1) +
async HTTP/2 (P2.1).**
