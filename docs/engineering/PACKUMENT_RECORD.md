# E60 — typed packument record: field contract, what was kept, what was rejected

Round 4 (2026-10-03), owner-authorized. Written by the orchestrator after
workstream R1 stopped with the measurement done and the document unwritten; R1's
own evidence is appended under `## R1` in
`.agents/work/tech-debt/production-ready-2026-10-03/progress.md`.

Baseline this round: `cargo check --workspace --all-targets` exit 0,
`cargo test --workspace` 704 passed / 0 failed / 12 ignored.

**Outcome in one line:** the partial migration landed and measured **−79.3 MB
(−30.3 %) peak RSS** with byte-identical lockfile output on every fixture; the
full migration is blocked on one signature in `layout.rs` and is recorded here as
a scoped follow-up, not attempted halfway.

## 1. The problem, measured

`serde_json::Value` costs one enum slot plus a heap allocation per JSON node.
Measured this round on a hermetic `jagr-instance-bench` run (5 attempts,
deterministic lockfile):

| Quantity | Value |
|---|---|
| Packument bytes on disk | 22.4 MB (69 packuments, 18,189 versions) |
| `Value`-tree proxy for those bytes | **257.8 MB (11.5×)** |
| Largest single packument | 8.3 MB raw → **118 MB** as a `Value` tree |
| E60 typed-record ceiling | 3.2× (≈72 MB for the same set) |
| Baseline peak RSS, `install --lockfile-only`, median of 5 | **261,767,168 B** |

The proxy is an order-of-magnitude model (per-node overhead), not an allocator
measurement; `measure_metadata_ceiling.py` says so in its own header. It is
consistent with the real peak, which is why it was used to scope the work.

## 2. Field contract — 19 keys

Enumerated independently by the orchestrator before R1's record was reviewed
(`orchestrator-field-list.txt`), then reconciled. Every key is carried; nothing
the resolver can see was narrowed.

| key (reads) | carried as | note |
|---|---|---|
| `versions` (46) | `Option<BTreeMap<String, VersionRecord>>` | `Option` preserves absent-vs-empty, which `has_versions()`' historical `.unwrap_or(true)` depends on |
| `_vortex_manifest_synth` (7) | `bool` + `de_marker` | presence semantics: any JSON value, **including `null`**, means true — matching `Value::get(k).is_some()` |
| `_bundled_index_synth` (7) | `bool` + `de_marker` | same; `is_synth()` = `m1 \|\| m2` |
| `dependencies` (10) | `SpecMap` (`BTreeMap<String, Option<String>>`) | `None` = value was not a string, so `unwrap_or("*")` sites keep their own handling |
| `peerDependencies` (8) | `SpecMap` | non-string peers still skipped by the caller, not by the parser |
| `optionalDependencies` (2) | `SpecMap` | |
| `peerDependenciesMeta` (5) + nested `optional` (10) | `BTreeMap<String, Value>` verbatim + `peer_optional()` | verbatim because the lockfile clones these values → byte parity |
| `dist` (8) → `tarball` (3), `integrity` (6) | `DistRecord { tarball, integrity, shasum }` | resolver takes `resolved` from `lockfile::tarball_url_from_registry`, not `dist.tarball`; both carried |
| `dist-tags` (4) → `latest` (2) | `dist_tags` + `latest_tag()` | used for `lattice_cache::remember(pkg,"latest",…)` |
| `version` (7), `name` (4) | `Spec` | |

Not packument keys — **lockfile** keys, read from `package-lock.json` `Value`s
that deliberately stay `Value`: `packages` (4), `resolved` (2), plus
`version`/`dependencies`/`peerDependencies` hits inside lockfile readers. No
lockfile reader was migrated.

Unknown keys are ignored — that is where the memory win comes from — so every
field the consumers read is carried explicitly. Every field is
`#[serde(default)]` with tolerant deserializers: a mistyped value degrades
instead of failing the parse, so a malformed packument cannot silently turn into
a dropped package.

## 3. What was kept

1. **`crates/jhol-core/src/packument.rs`** (new, ~529 lines, 5 unit tests) —
   `PackumentRecord` / `VersionRecord` / `DistRecord` / `Spec`, `is_synth()`,
   `latest_tag()`, `satisfies_all()`, `highest_satisfying()`, `newest_version()`,
   `retain_versions()`.
2. **The retained-clone removal, which is where the win came from.**
   `registry::cache_packument` used to store a pruned full clone of every fetched
   packument in an unbounded, never-evicted global `RwLock<HashMap<String, Value>>`
   (`packument_read_cache`) *in addition to* `fetch_dep_tree`'s own map — two
   copies of the whole metadata set. `cache_packument` is now a no-op and
   `get_cached_packument` parses on demand from the byte-level LRU the fetch path
   already fills. Its only readers are `registry.rs:1216` and `:1651`, neither on
   the `--lockfile-only` path.
3. **`newest_version` fixed to a total order** — see §5.

## 4. Measured result

All runs: hermetic fixture copy (never in-place), fresh `JHOL_CACHE_DIR` per run,
no `package-lock.json` in the copy, `install --lockfile-only`, `/usr/bin/time -l`.
Runner: `.agents/work/performance/duplication/parity_run.sh`.

| Peak RSS, median of 5 | Bytes | Range |
|---|---|---|
| before (`/tmp/jhol-before`, sha256 `dd1c5848…`) | 261,767,168 | 257.2–282.1 MB |
| after (final tree, sha256 `968e32cd…`) | **182,468,608** | 170.5–197.6 MB |
| **delta** | **−79,298,560 (−30.3 %)** | distributions do not overlap |

Work asserted in every run: 78 lockfile packages, identical to the before-run.

**Parity gate — PASS.** Lockfiles byte-identical before vs after on all eight
cases: `bench-1..5` (23,570 B each), `react-app` (64 entries), `peer-deps-app`
(4), `workspace-app` (2). This is what makes the win a "same answer, less memory"
result rather than a behaviour change.

## 5. Two bugs the record work surfaced — parity caught neither

Both were resolution-relevant rule changes hiding inside a "pure memory" refactor.
This is the part to remember about E60.

1. **`_vortex_manifest_synth` / `_bundled_index_synth` would have been dropped.**
   A record that ignores unknown keys silently loses the two markers the synth
   path keys off (`fetch_metadata_full_packument`'s reject-synth-cached branch,
   and the BFS escalation when a synth-marked meta cannot satisfy a constraint).
   The symptom would have been "resolved the wrong version from a stale synth",
   not a crash. R1 carried them with `de_marker` presence semantics plus a
   round-trip unit test including `"_bundled_index_synth":null`.
   **Parity could not have caught this: 0 of 5 fixture stdouts mention synth.**
2. **`newest_version` used a non-total comparator** (mixed semver/textual
   `max_by`), and `max_by`'s single pass settled on `"not-semver"` for
   `["1.0.0","1.5.0","2.0.0","not-semver"]` — a version that cannot be fetched.
   Fixed in `packument.rs:193` to filter-to-parseable then max, falling back to
   textual order only when nothing parses; the parity with
   `jagr_infinite::collect_dep_names_for`'s `newest` closure is preserved for
   every input that closure actually sees. It had exactly one caller — its own
   test — so no production behaviour changed, but the public accessor's contract
   was wrong.

## 6. Rejected, deferred, and the unblock request

- **Full `jagr_infinite` / `lockfile_write` migration — deferred with a blocker.**
  Migrating `fetch_one_packument` forces
  `HashMap<String, PackumentRecord>` through `fetch_dep_tree` → `build_full_chart`
  → `reconcile_versions` → `finalize_resolution` → **`layout::build_node_layout`
  (`layout.rs:62-68`), whose parameter is `&HashMap<String, serde_json::Value>`**.
  Its accessors defeat a partial migration: `highest_satisfying` (`layout.rs:33`)
  needs every version key; `layout.rs:180-186` reads `versions[v].dist.tarball`
  and `.dist.integrity` for versions layout itself chooses; `declared_edges`
  (`layout.rs:230-247`) needs deps/peers/peerMeta for arbitrary versions — and
  `build_node_layout_with`'s accessor is `Fn(&str) -> Option<&'a Value>`, so
  records cannot be substituted and Values cannot be built lazily (the reference
  must outlive the call). Keeping a record map **and** a Value map alive together
  measures as a regression by construction — that is E67's +3 %/+26 % mechanism,
  not a hypothesis.
  **Ask for a follow-up round:** let `layout.rs` take the record (or gain a
  trait-based accessor), then migrate the remaining consumers.
- **`lockfile_write.rs` left on the `Value` path deliberately** — its packument
  reads serve `resolve_full_tree_jagr3` / `pubgrub_v2`, which W2 documented as
  unreachable from the CLI (`resolve_full_tree` routes every `--resolver` value to
  JAGR-∞). Migrating unreachable code adds risk for a win nobody can measure.
- **Unproven, flagged:** that the byte-level LRU is filled on every path that
  previously called `cache_packument`. Parity says the eight fixtures behave
  identically; it does not prove the general case. If a future report shows a
  packument re-fetch or a resolution difference, check this first.

## 7. Verification commands

```sh
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2
cargo check --workspace --all-targets     # exit 0
cargo test --workspace                    # 704 passed / 0 failed / 12 ignored (baseline)
cargo test -p jhol-core --lib             # 506 passed / 0 failed / 2 ignored (+5 new tests)
bash .agents/work/tech-debt/production-ready-2026-10-03/verify-no-loss.sh
```
