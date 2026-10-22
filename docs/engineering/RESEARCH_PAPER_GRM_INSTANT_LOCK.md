# Global Resolution Memory and Instant Lock: Spectral Capsules for Sub-Millisecond Package Install Reuse

**Bhuvan Prakash**  
*Independent Researcher*  
`bhuvan@jhol.dev`  
September 2026

**Keywords:** global resolution memory, instant lock, dependency capsules, spectral stability, package-manager reuse, mmap install plans, install daemons, freshness calculus.

**Companion papers.** *Sheaf Homotopy and Ricci Flow for Package Resolution* [1] treats **decide**. *JAGR-S: A Succinct Content-Addressed Artifact Store* [2] treats **store**. This paper treats **reuse**: how a resolved tree becomes a reusable plan that skips resolution and network on subsequent installs.

---

## Abstract

After a package manager has resolved and materialized a dependency tree once, the dominant workload in day-to-day development is not cold resolution but **reuse**: unchanged projects, warm caches, repeated CI jobs, and editor-driven no-op installs. In practice, warm installs still re-parse JSON lockfiles, re-walk `node_modules`, re-hash manifests, and often re-enter the resolver even when the assignment is already known. Across npm, Yarn, Bun, and pnpm, there is no shared, local-first memory of *resolution outcomes*—only of tarball bytes and lockfile text.

This paper presents **Global Resolution Memory (GRM)** and **Instant Lock** as realized in Jhol. GRM is a lock-free, DashMap-sharded knowledge base of resolution records keyed by package spec and by dense ordinal keys $(P_{\mathrm{id}}\ll 32)\mid v_{\mathrm{ord}}$, with XXH3-accelerated spec queries. **Instant Lock** is the **JAGR Graph Capsule**: a versioned, `repr(C)` mmap file (`JHLCPL`) whose install path is `mmap → linear scan → hardlink`, with open-time soundness checks that reject corrupt or adversarial capsule bytes. A **Soul Path** Degree-Sequence Spectral Certificate (DSSC) classifies manifests as Stable / Marginal / Fragile / Degenerate and permits `can_skip_resolve` only when Stable ∧ $\hat\beta_1=0$. A Unix **install daemon** with thread-per-connection handling, read timeouts, and a process-wide `DAEMON_CWD_LOCK` serves warm attaches in tens of milliseconds while preserving cache isolation when `JHOL_CACHE_DIR` is set.

Empirical results on Apple Silicon (10 cores, darwin-arm64): unchanged 20-dependency projects reinstall in **7.5–11 ms** (Bun ≈ 7.8 ms); daemon-attached warm installs **30 ms** median versus **48 ms** direct (−38%); ~300-package warm-cache installs **76–127 ms** versus Bun **179–235 ms**. Freshness gates correctly invalidate on manifest change, deleted package directories, and explicit `install <pkg>` requests. We state soundness obligations for capsule open, fingerprint format, and daemon cwd serialization, and we clarify what GRM *does not* replace: strict `ci` still requires a verifiable lockfile with integrity.

---

## 1. Introduction

Cold install time in modern JavaScript package managers is bounded by registry physics [1]: abbreviated packuments, tarball bytes, TLS round trips, and CDN throttling dominate once the resolver itself is sound. Warm install time is bounded by *software*: how much work the manager invents when nothing changed.

The engineering log for Jhol documents years of cold-path optimization—HTTP/2 tarball multiplexing, manifest-first metadata, tarball prefetch overlap, store completeness markers—that brought large-project cold installs to parity with Bun. The remaining product gap for daily use is reuse latency: a developer who saves `package.json` unchanged and runs `jhol install` again should pay milliseconds, not a re-resolution tax.

**Table 1. What each reuse layer remembers.**

| Layer | Remembers | Typical cost if hit |
|-------|-----------|---------------------|
| OS page cache | File bytes | µs |
| Tarball / unpacked store [2] | Package contents by content hash | ms (link) |
| JSON lockfile | Version assignment + integrity | parse + walk |
| **GRM** | Past resolution records | sub-µs query |
| **Capsule (Instant Lock)** | Full binary install plan + spectral metadata | mmap + scan |
| **Daemon** | Warm process + status/fingerprint | connect + plan |

npm, Yarn, pnpm, and Bun each optimize some rows. None expose a first-class, local-first resolution memory that is simultaneously (a) lock-free under parallel installs, (b) ordinal-keyed for dense atlas reuse, and (c) paired with a spectral skip certificate and a zero-parse capsule plan.

### 1.1 Problem statement

Let $M$ be a project manifest and $L$ a lockfile. After an initial successful install, the manager should answer three questions cheaply:

1. **Skip resolve?** Has the root assignment changed in a way that could alter the resolved tree?
2. **Skip parse?** Can the install plan be read without JSON or lockfile parsing?
3. **Skip startup?** Can a warm process serve the request without respawning the CLI?

A correct system must fail closed when any answer is uncertain.

### 1.2 Contributions

1. **Lock-free GRM (§3):** DashMap shards, dense ordinal keys, XXH3 fast path, atomic telemetry; explicit statement of what GRM is *not*.
2. **Capsule Instant Lock (§4):** 64-byte `JHLCPL` layout, verified mmap open, zero-parse install path, explicit-request safety.
3. **Soul Path DSSC (§5):** Stable/Marginal/Fragile/Degenerate classification and `can_skip_resolve`.
4. **Install daemon (§6):** Thread-per-connection, `DAEMON_CWD_LOCK`, timeouts, `JHOL_CACHE_DIR` isolation, auto-attach defaults.
5. **Freshness calculus (§7):** Manifest hash, raw 8-byte fingerprint, `node_modules` entry count; gates that refuse to fire on explicit argv.
6. **Evaluation (§8):** Warm/no-op tables, ablations, correctness suite, and measurement protocol.

**Organization.** §2 model. §2.5 architecture. §3–7 design. §8 evaluation. §9 related work. §10 conclusion.

---

## 2.5 End-to-end reuse architecture

On a warm reinstall the CLI consults cheap pre-clap gates (status file, fingerprint, argv operands). If those miss, Soul Path classifies the manifest and may execute a capsule without JSON parsing. GRM accelerates any residual resolution; the store layer [2] supplies tarball bytes; the daemon optionally eliminates process startup.

**Design invariant:** each layer fails closed independently. A corrupt capsule cannot override a stale fingerprint; a warm daemon cannot ignore `JHOL_CACHE_DIR`; GRM hits never bypass integrity verification on `ci`.

```
CLI/argv → Freshness gates → Soul Path (Stable?) ──miss──→ GRM ──miss──→ Full resolve [1]
                │                    │
                └──── Capsule ───────┴──→ Store hardlink
                └── (optional) Daemon warm attach ──→ Capsule
```

---

## 2. Model

Let $\mathcal{P}$ be packages, $\mathcal{V}_P$ published versions, and $\mathcal{R}$ SemVer constraints. A project root specifies $M:\mathcal{P}\to\mathcal{R}$. A lockfile $L$ records an assignment $\sigma$ plus integrity metadata.

A **resolution record** is
$$
R = (\mathit{spec}, v, h_{\mathrm{content}}, \{h_{\mathrm{dep}}\}, \mathit{platform}, \mathit{node}, H(R)).
$$
GRM stores a multiset of such records keyed by spec and by dense ordinal keys.

A **capsule** $C$ is a binary encoding of a concrete install plan for one project state: package entries in install order, dependency edges, tarball hashes, string table, manifest hash, and spectral metadata $(\lambda_2,\hat\beta_1,\kappa)$. Magic `JHLCPL`, version 1.

**Freshness predicate.** Write $\mathrm{fresh}(M,L,C)$ when: (i) $H(M)$ matches the capsule header's `manifest_hash`; (ii) lockfile hash matches when $L$ exists; (iii) the on-disk `node_modules` entry count matches the status record; (iv) no explicit CLI package operand is absent from the capsule entry set.

**Reuse theorem (informal).** If $\mathrm{fresh}(M,L,C)$ and $\mathrm{Stable}(M)$ with $\hat\beta_1(C)=0$, then materializing from $C$ yields the same tree as re-resolving $M$ under identical registry snapshots—because $C$ was produced by a validated resolver [1] and freshness binds $C$ to $(M,L)$.

---

## 3. Global Resolution Memory

### 3.1 Architecture

```
LockFreeGRM
├── shards: DashMap<String, Vec<ResolutionRecord>>   # keyed by package_spec
├── fast_ordinal_shards: DashMap<u64, ResolutionRecord>
└── stats: { total_records, total_hits, misses }     # AtomicU64
```

Removing an outer `RwLock` (earlier design) eliminates the global serialization point: DashMap already shards internally.

### 3.2 Dense ordinal keys

Packages in the JAGR atlas carry integer ids; versions carry chart ordinals. Encode
$$
k = (P_{\mathrm{id}} \ll 32) \mid v_{\mathrm{ord}} \in \mathbb{N}_{64}.
$$
`store_ordinal` / `query_ordinal` are wait-free w.r.t. unrelated keys.

### 3.3 XXH3 fast path

For unfiltered `query(spec, None, None)`, Jhol hashes the spec with XXH3, probes `fast_ordinal_shards`, and verifies `record.package_spec == spec`. When platform or Node version filters are present, the implementation scans the spec shard and selects the best match by `(cached_at, hit_count)`.

**Table 2. GRM query semantics.**

| Call | Behavior |
|------|----------|
| `query_ordinal(k)` | Direct DashMap get; hit/miss atomic |
| `query(spec)` | XXH3 fast path, then equality verify |
| `query(spec, platform, node)` | Shard scan with filter; max cached entry |
| `store(record)` | Upsert in spec shard + XXH3 index |

**Global API.** `store_global`, `query_global`, `store_ordinal_global`, and `query_ordinal_global` wrap a `OnceLock`-initialized singleton shared by worker threads.

### 3.4 What GRM is not

GRM accelerates *repeated* resolution of the same specs across projects and sessions. It does **not** replace the lockfile as the CI source of truth. Strict `jhol ci` still requires a verifiable lockfile with integrity [1,2]. GRM also does not substitute for tarball presence.

---

## 4. Instant Lock = Capsule

### 4.1 On-disk layout (`JHLCPL`)

```
[ CapsuleHeader 64 B ]
[ PackageEntry × N   64 B each ]
[ StringTable        variable ]
[ DepEdge × M        8 B each ]
[ optional PBF trailer ]
```

Magic `JHLCPL\0\0`, version 1. Header fields include `manifest_hash`, `total_unpacked_bytes`, `spectral_gap`, `predicted_beta1`, `knot_fingerprint`. Capsule manifest hashing folds `LAYOUT_VERSION` so layout bugfixes invalidate stale capsules.

### 4.2 Open-time soundness

Prior bugs included `from_utf8_unchecked` and unchecked `from_raw_parts` on adversary-controlled offsets. Current open path:

1. Length ≥ header; magic/version check.
2. Checked arithmetic for section ends (no wrap on 32-bit).
3. UTF-8 validate string table once.
4. `verify()` ensures every entry offset in range.
5. Accessors use bounded `read_str` returning `""` on failure—never panic on corrupt input (`panic=abort` in release).

Permanent adversarial tests cover truncation, invalid UTF-8, corrupt offsets, absurd counts, and post-open mutation (TOCTOU).

### 4.3 Install path

Warm hit: `mmap → iterate entries by install_order → hardlink/copy from store by tarball hash → rebuild .bin`. No `serde_json` for the plan. Store completeness markers (`.jhol-extract-complete`) ensure link targets are full trees [2].

### 4.4 Explicit request safety

A capsule must not answer `jhol install foo@1.2.3` unless every requested base name appears in its entry set. Both `can_reuse_lockfile_for_requested` and `try_capsule_install` enforce this—closing a silent-drop class of bugs (E119–E125).

---

## 5. Soul Path and DSSC

Install is framed as a spectral decision tree:

```
classify(M) → StabilityClass
  Stable    → capsule execute (+ optional skip resolve)
  Marginal  → greedy / lockfile reuse
  Fragile   → full JAGR / expansion pipeline [1]
  Degenerate→ empty / trivial
```

**DSSC (Degree-Sequence Spectral Certificate).** From root deps alone (no full tropical stack):

- $n = |M|$
- tightness ratio = fraction of exact pins
- risk scores for `*`, `latest`, `^`/`~`, `||`
- proxy spectral gap from tightness; predicted $\beta_1$ from wide-range density

`can_skip_resolve` ⇔ `Stable` ∧ `predicted_beta1 == 0`.

After capsule execute, Soul Path performs **spectral verify**: every root dependency must exist and satisfy its declared range. Failure downgrades to full resolve.

---

## 6. Install Daemon

### 6.1 Role

A long-lived `jhol daemon --serve` process holds warm caches and answers install requests over a Unix domain socket (`0o600`). Auto-attach is default (`soul_path::auto_daemon_enabled`), with opt-outs `JHOL_AUTO_DAEMON=0`, `JHOL_NO_DAEMON=1`, and conservative `CI=1` behavior. If `JHOL_CACHE_DIR` is set, clients stay local so benchmarks and isolated caches are not silently served from the daemon's home cache (E84).

### 6.2 Concurrency and cwd

Handlers run **one thread per connection** (E66). The process-wide `DAEMON_CWD_LOCK` is held across `chdir` + install + restore—because install APIs are relative-path based and `chdir` is process-global. Without this lock, concurrent `--daemon` installs polluted `/tmp/node_modules` (E104).

Idle connections get a read timeout (`JHOL_DAEMON_READ_TIMEOUT_SECS`, default 30 s); active handlers are capped (`JHOL_DAEMON_MAX_CONNS`, default 64). Magic bytes are validated; `pkg_count` is clamped before allocation (E92).

### 6.3 Performance

| Mode | Median (20-dep warm) |
|------|----------------------|
| Direct install | 48 ms |
| Daemon attach | 30 ms (−38%) |
| No-op freshness hit | 7.5–11 ms |

---

## 7. Freshness Calculus

A reuse decision needs three bits of state:

1. **Manifest hash** $H(M)$ (and lockfile hash when present).
2. **Capsule / nm fingerprint** in compatible **binary** format (raw 8-byte hash—not hex text; mismatched writers caused permanent miss).
3. **Entry count** of `node_modules` in the status record (`JHLSTAT`, field `nm_entries`)—so deleting one package directory forces repair.

Gates: `lockfile_fingerprint_matches`, capsule warm bypass, `try_exit_before_clap` (must not fire when argv names packages).

---

## 8. Evaluation

### 8.1 Warm and no-op

| Scenario | Jhol | Bun (same machine) |
|----------|------|--------------------|
| 20-dep no-op reinstall | 7.5–11 ms | ≈ 7.8 ms |
| ~300-pkg warm cache | 76–127 ms | 179–235 ms |
| Daemon warm attach | 30 ms med | n/a |
| Direct warm (no daemon) | 48 ms med | — |
| Store-index warm tail | 16 ms med | 23 ms |

Protocol: fresh HOME/cache for cold; reused cache for warm; assert tree usability; kill daemons when measuring cold isolation; medians of ≥3–5 runs.

### 8.1b Warm-path decomposition

| Stage | Time (20-dep warm, representative) |
|-------|-------------------------------------|
| Process startup + clap | ≈ 4 ms |
| Status / fingerprint check | ≈ 1 ms |
| Capsule mmap + scan | ≈ 3 ms |
| Store hardlink + `.bin` | ≈ 6–8 ms |
| Lockfile tail (parallel SRI memo) | ≈ 12–15 ms |
| **Total (no-op path)** | **7.5–11 ms** when early exit |
| **Total (full warm)** | **16–48 ms** when layout tail runs |

The store-index optimization (index `store_unpacked` once per cache dir, then O(1) lookups) removed an O(entries × store) walk that previously dominated warm installs at scale.

### 8.2 Ablations

| Disable | Effect |
|---------|--------|
| Capsule missing | Fall back to lockfile layout / resolve |
| GRM empty | Resolve from registry; store records after |
| Daemon off | Direct path; no-op still ~8–11 ms via fingerprint |
| `JHOL_CACHE_DIR` set | Forces local path (isolation) |
| Status entry count omitted | Deleted package dir falsely “fresh” (fixed) |
| Explicit `install <pkg>` | Capsule/lockfile reuse refused unless entry present |

### 8.3 Correctness suite

Capsule adversarial tests (corrupt offsets, invalid UTF-8, truncation, post-open mutation). Freshness: manifest change, `add`, deleted dir, wipe, explicit install. Daemon: silent peer does not stall; concurrent projects do not cross-link. Integration: core lib / CLI / production gate. E145 nested-layout verifier for npm lockfile nested `debug` versions.

### 8.4 Measurement traps

1. Deleting only `store/` while a capsule exists is not a cold install.
2. `cmd | tail; echo $?` reports `tail`'s status, not `cmd`'s.
3. Running daemons break `JHOL_CACHE_DIR` isolation benchmarks.

---

## 9. Related Work

**Lockfiles.** npm / Yarn / pnpm / Bun lockfiles are the durable assignment; capsules are a *derived*, machine-local acceleration of the same plan.

**Build systems.** Bazel/Buck remote caches store action outputs; GRM stores *resolution* outputs—closer to Cargo's registry index cache than to CAS blobs [2].

**Daemonized tools.** `gradle --daemon`, `sbt`, language servers keep warm heaps; Jhol's daemon is specialized to install requests with explicit cwd serialization.

**Companion papers.** [1] resolution; [2] artifact storage and harvest.

---

## 10. Conclusion

**GRM** remembers resolutions; **capsules** remember install plans; **Soul Path** decides when remembering is safe; the **daemon** keeps the machinery warm. Together they bring unchanged-project installs to Bun parity (~8 ms) and make attached warm installs ~38% faster than direct process startup—without sacrificing the strict `ci` integrity model or explicit-request semantics.

### Limitations and future work

- Capsule format is project-local; sharing capsules across machines needs layout + libc/os tags in the header.
- DSSC is a root-level proxy; folding full sheaf $\lambda_2$ from [1] into the gate without losing the 8 ms budget remains open.
- Windows named-pipe daemon parity with the Unix socket path.

### Software availability

Open-source Jhol: `lock_free_grm.rs`, `grm/`, `capsule.rs`, `soul_path.rs`, daemon in `src/main.rs`.

---

## References

[1] B. Prakash. Sheaf Homotopy and Ricci Flow for Package Resolution. Jhol Research Series, Paper 1, 2026.  
[2] B. Prakash. JAGR-S: A Succinct Content-Addressed Artifact Store. Jhol Research Series, Paper 2, 2026.  
[3] N. Weizenbaum. PubGrub: Next-generation version solving. Dart Technical Reports, 2018.  
[4] npm CLI. package-lock.json documentation.  
[5] F. Mancinelli et al. Managing the evolution of a software configuration graph. ASE, 2006.  
[6] Y. Abate et al. Dependency solving is still hard, but we are getting better at it. ESEC/FSE, 2012.

---

## Acknowledgments

Engineering rounds E66, E84, E104, E119–E125, and E81 established the daemon, freshness, explicit-install, and auto-attach contracts that this paper formalizes.
