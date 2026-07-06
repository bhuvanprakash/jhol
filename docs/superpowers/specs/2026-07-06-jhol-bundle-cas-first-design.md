# Jhol Bundle — CAS-First Architecture Design

**Date:** 2026-07-06
**Status:** Approved
**Language:** Rust

---

## 1. Overview

Jhol Bundle is a cache-first, CAS-based JavaScript/TypeScript/JSX bundler built on Jhol's existing content-addressed storage infrastructure. Unlike conventional bundlers (esbuild, Rollup, Vite, Turbopack) that re-parse and re-transform every module on every build, Jhol Bundle deduplicates by content hash and reuses transformed output from the CAS store. The result is a bundler where identical files across projects and rebuilds are processed exactly once.

**Core principle:** The fastest code is code you never run twice.

---

## 2. Why CAS-First Is Different

Every existing bundler has a parse → graph → transform → emit pipeline. They differ in language (Go, Rust, JS), parallelism (serial, rayon, worker threads), and caching strategy (disk cache, memory cache). But all share one flaw: identical content is processed multiple times.

Jhol Bundle fixes this at the architecture level:

```
Conventional bundler:
  input.js → parse → transform → emit
  (every file, every build, every time)

Jhol Bundle:
  input.js → xxhash3 → CAS lookup
    ├── content already in CAS → skip parse/transform, emit from cache
    └── content not in CAS → parse, transform, store in CAS, emit
```

In a typical project, 60-80% of node_modules content is identical across installs or rebuilds. CAS-first means that content is parsed and transformed exactly once, ever.

---

## 3. Architecture

### 3.1 Layer 1 — Ingest & Hash (io_uring)

**Goal:** Read files from disk with minimum syscall overhead, compute content hash, deduplicate.

**Pipeline:**

```
File read (io_uring async read, mmap for hot files)
    ↓
xxHash3 hash (3.5 GB/s on modern hardware, non-cryptographic)
    ↓
CAS lookup (content hash as key)
    ├── HIT  → return cached ParseManifest
    └── MISS → enqueue for parsing
```

**Key design decisions:**

- `io_uring` for async file reads — avoids blocking worker threads on I/O. Falls back to `spawn_blocking` on non-Linux.
- mmap for files that are already in the OS page cache (common on rebuilds) — zero-copy read.
- xxHash3 (from `xxhash-rust` crate, already a dependency) — fastest non-cryptographic hash, no licensing issues.
- CAS keys are `content_hash = xxhash3(file_content)`. The content hash is the canonical identity of a module, not its path. This means `node_modules/react/index.js` and `node_modules/preact/index.js` with identical content are stored once.
- File metadata (path, mtime) stored separately from content. Rebuild invalidation uses mtime+size, not content re-hash.

**Data structure:** `HashMap<xxHash3, Arc<CachedModule>>` with `RwLock`. Sharded by hash prefix to reduce lock contention under parallel ingest.

### 3.2 Layer 2 — Parse & Transform (Rayon, lock-free)

**Goal:** Parse and transform unique content exactly once, store result in CAS.

**Pipeline:**

```
MISS queue (mpmc channel, bounded)
    ↓
Rayon parallel workers (one task per unique content hash)
    ├── Parse (swc via jhol's existing parser integration, or native regex for simple files)
    ├── Transform (TypeScript strip, JSX, env define replacement)
    └── Store ParseManifest + TransformedOutput in CAS
    ↓
Result: ParseManifest { ast_hash, exports, imports, deps }
         TransformedOutput { code, sourcemap_vlq }
```

**Key design decisions:**

- Uses Jhol's existing `rayon` dependency (already in workspace).
- Lock-free result emission: each worker writes to its own CAS shard, no shared mutation.
- Parse manifests are stored separately from transformed output. This means we can invalidate transforms (e.g., a new JSX pragma) without re-parsing.
- SwC is used for TypeScript/JSX parsing — the same way Jhol-core uses it. No new parser dependency.
- For simple `.js` files without TypeScript, we use a fast regex-based import/export parser to avoid swc overhead. This covers the majority of npm packages.
- Parallelism is bounded by CPU cores, not queue depth. Uses `par_iter()` with a semaphore for concurrency control.

### 3.3 Layer 3 — Graph & Emit (work-stealing queue, reflink)

**Goal:** Build module graph from parse manifests, emit output files.

**Pipeline:**

```
ParseManifests from CAS
    ↓
Graph builder (topological sort, cycle detection, externals resolution)
    ↓
Chunk partitioner (entry points → chunks, shared deps → async chunks)
    ↓
Emit workers (one per chunk, work-stealing queue)
    ├── Import/export rewriting
    ├── Output format (ESM / CJS / IIFE)
    └── File write (io_uring async write, reflink for same-FS copies)
```

**Key design decisions:**

- Uses Jhol's existing work-stealing `TaskQueue` (`crates/jhol-core/src/task_queue/`).
- Code splitting: modules reachable from multiple entry points become async chunks loaded via dynamic `import()`. This is the only form of splitting — no heuristic splitting, no magic naming.
- Output files are written with `io_uring` where available. For same-FS destinations, `reflink::reflink` (CoW clone) is used for zero-copy file duplication.
- The module graph is built from ParseManifests, not raw file content. This means graph construction is purely in-memory and fast — no I/O blocking.

### 3.4 CAS Schema

```
~/.jhol-cache/bundle-cas/
  ├── manifests/       # ParseManifest records (JSON)
  │   └── {xxhash3}.json
  ├── transformed/     # Transformed output (UTF-8 text)
  │   └── {xxhash3}.js
  ├── sourcemaps/      # VLQ sourcemaps
  │   └── {xxhash3}.map
  └── meta/            # Metadata (file path → content hash mapping)
      └── index.json   # Path → content hash for invalidation
```

- All keys are `xxhash3` of the original file content (not path).
- The `meta/index.json` maps source file paths to content hashes for cache invalidation. On rebuild, only files with changed mtime or size are re-hashed.
- On project clean, only `meta/index.json` is deleted. All parsed/transformed content persists across projects.

---

## 4. Module Graph

Jhol Bundle uses a **directed acyclic graph (DAG)** of ParseManifests. Each module node tracks:

```rust
struct ModuleNode {
    content_hash: u64,        // xxHash3 of original content
    ast_hash: u64,             // xxHash3 of parsed AST (for transform invalidation)
    path: Option<PathBuf>,     // None for virtual modules
    exports: Vec<ExportRecord>,
    imports: Vec<ImportRecord>,
    dependencies: Vec<u64>,    // content hashes of dependencies
    is_external: bool,
    is_entry: bool,
}
```

**Resolution order:**

1. Relative imports (`./foo`, `../foo`) — resolved against current file's directory, with extension search (`.js`, `.jsx`, `.ts`, `.tsx`, `.mts`, `.cts`, `.json`, `/index.js`).
2. Bare specifiers (`react`, `lodash`) — resolved via node_modules traversal from the importing file's directory upward.
3. Absolute imports (`/src/foo`) — resolved against project root.
4. Builtin modules (`fs`, `path`, `crypto`) — marked external automatically.

**Cycles:** Detected via DFS with color marking (white/gray/black). If a cycle is found, the bundler errors with the full cycle path. Cycles are not collapsed — they must be broken by the user.

---

## 5. Tree Shaking

Tree shaking in Jhol Bundle is **side-effect-based**, not export-based.

**Side effect categories:**

- **Pure** — modules that only export values and have no top-level side effects. Safe to tree shake.
- **Has side effects** — modules with top-level calls (`fetch()`, `console.log()`, `require()`), CSS/JSON imports, or missing `sideEffects: false` in package.json.
- **Unknown** — treated as having side effects (conservative).

**Algorithm:**

```
topological sort of the module DAG (leaves first)
    ↓
for each module (bottom-up):
    if module.exports is empty → mark all its deps as "potentially used"
    if module is pure AND no dependent uses its exports → skip (don't emit)
    if module has side effects → always emit
```

**Export usage tracking:** Entry modules mark all their exports as "used". Re-export chains are traced. Side-effect-only imports (`import 'polyfill'`) mark the target as "used" even if no named exports are consumed.

---

## 6. Code Splitting

Jhol Bundle's code splitting is **deterministic**, not heuristic.

**Split rules:**

1. Any module loaded by 2+ entry points → async chunk (loaded once, shared).
2. Any module reachable via `import()` (dynamic import) → async chunk.
3. Everything else → part of the importing entry's chunk.

**No:**
- Automatic vendor chunking
- heuristic chunk naming
- minSize/maxSize configuration

**The chunk graph is a DAG.** Circular dynamic imports are errors. Each chunk is emitted as a separate file with a hash-based name: `[entry-name].[content-hash].js`.

---

## 7. Supported Formats & Targets

### Formats

| Format | Description |
|--------|-------------|
| ESM (.mjs) | Native ES modules — import/export, no wrapper |
| CJS (.cjs) | CommonJS — `require()`, `module.exports` |
| IIFE (.js) | Immediately invoked — wraps in `(function(){})()` |

### Targets

| Target | Transforms |
|--------|-----------|
| browser | ES2020, no Node.js builtins |
| node | ES2022, all Node.js builtins external |
| jhol | ES2022, Jhol runtime polyfills |

### Minification

When `minify: true`, the transformed output is passed through swc's minifier (bundled via the `swc-backend` feature). Minification is skipped for non-JS files (CSS, JSON).

---

## 8. Watch Mode

Watch mode uses `notify` (already a dependency) to monitor entry files and their transitive dependencies.

**Invalidation strategy:**

```
File change detected (notify event)
    ↓
Re-hash changed file
    ↓
Compare against meta/index.json
    ├── Content hash unchanged → ignore (OS write, no change)
    └── Content hash changed →
        ├── Invalidate ParseManifest from CAS
        ├── Invalidate TransformedOutput from CAS
        ├── Re-parse + Re-transform only this file
        └── Re-run graph + emit (incremental)
```

**Incremental emit:** Only chunks containing the changed module (or its dependents) are re-emitted. Unaffected chunks are left untouched.

---

## 9. Sourcemaps

Sourcemaps use a **hand-rolled VLQ encoder** (no `sourcemap` crate dependency).

```
Original file → Parse → Transform → Emit
     ↑_____________VLQ offset tracking_____________|
```

Each transformation step records byte-offset deltas. Final VLQ encodes:
- `mappings` field: semicolon-separated line segments, comma-separated VLQ pairs
- `sources` field: relative paths from output file to source files
- `sourcesContent`: optionally inlined original source

**Options:**
- `none` — no sourcemap
- `inline` — VLQ appended as `//# sourceMappingURL=data:...` comment
- `external` — `.map` file written alongside output
- `both` — external file + inline comment

---

## 10. Error Handling

| Error | Behavior |
|-------|----------|
| Module not found | Error with path, suggestion (did you mean...) |
| Parse error | Error with line:column, code snippet, parser note |
| Cycle detected | Error with full cycle path |
| Transform error | Error with file, transform stage, reason |
| I/O error | Error with path, OS error code |
| CAS write error | Fall back to non-cached pipeline, warn |

**No silent failures.** Every error is surfaced with a file path and a code snippet.

---

## 11. Dependencies

Jhol Bundle reuses existing Jhol dependencies where possible:

- `rayon` — parallel parsing and transform
- `xxhash-rust` — content hashing (already in workspace)
- `swc_core` — TypeScript/JSX parsing and transform (already in jhol-bundle)
- `notify` — file watching (already in jhol-bundle)
- `reflink` — zero-copy file duplication (already in jhol-core)
- `io-uring` — async I/O (already in jhol-core, Linux only)

**No new heavy dependencies.** The CAS store and work-stealing queue already exist in jhol-core.

---

## 12. Performance Model

**Theoretical speed ceiling:**

For a project with `n` modules, `d` distinct content hashes:

| Phase | Conventional | Jhol Bundle |
|-------|-------------|-------------|
| Ingest | O(n) file reads | O(n) hash only (fast) |
| Parse | O(n) parse | O(d) parse (d ≤ n) |
| Transform | O(n) transform | O(d) transform (d ≤ n) |
| Emit | O(n) writes | O(changed) writes |
| **Total** | **O(n) all phases** | **O(d) parse+transform, O(changed) emit** |

**In practice:** For a typical `node_modules` project, d/n ≈ 0.2-0.4 (60-80% content deduplication). On rebuilds, changed modules are typically < 5% of total.

---

## 13. What's NOT Implemented (YAGNI)

- **Plugin system** — not at launch. The transform pipeline is fixed: TS strip → JSX → define → minify.
- **Multiple entry points with shared chunk optimization** — beyond the deterministic "shared by 2+ entries" rule.
- **WebAssembly target** — future work.
- **HMR** — this is a build tool, not a dev server.
- **Hot module replacement** — see above.
- **Custom loaders** — the extension → parser mapping is fixed: `.ts`→swc, `.js`→regex, `.css`→lightningcss (optional).

---

## 14. File Structure

```
crates/jhol-bundle/
  ├── Cargo.toml
  └── src/
      ├── lib.rs              # Public API: bundle(), bundle_for_target()
      ├── ingest.rs           # Layer 1: io_uring read, xxhash3, CAS lookup
      ├── parse.rs            # Layer 2: swc parsing, regex parsing, ParseManifest
      ├── transform.rs        # Layer 2: TypeScript strip, JSX, define replacement
      ├── cas.rs              # CAS store: manifest/transformed/sourcemap/meta
      ├── graph.rs            # Module graph, topological sort, cycle detection
      ├── tree_shake.rs       # Side-effect-based tree shaking
      ├── split.rs            # Chunk partitioner (entry → async → shared)
      ├── emit.rs             # Layer 3: import rewriting, format output, file write
      ├── sourcemap.rs        # VLQ encoder (hand-rolled)
      ├── watch.rs            # notify-based watch + incremental invalidation
      ├── config.rs           # BundleConfig (already exists)
      ├── errors.rs           # BundleError (already exists)
      └── plugins.rs          # (stub, not implemented at launch)
```

---

## 15. Open Questions

1. **ParseManifest storage format** — JSON (human-readable, slow) vs Bincode (fast, not human-readable). Recommendation: Bincode for CAS storage, JSON for debug output.
2. **CAS eviction** — when does the CAS grow unbounded? Recommendation: LRU with a configurable max size (default 2GB), evicted by access time.
3. **Sourcemap quality** — swc generates accurate sourcemaps. Hand-rolled VLQ encoder is correct but not optimized for smallest output. Acceptable for launch.