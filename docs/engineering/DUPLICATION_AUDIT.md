# Duplication audit — measured, root cause

Workstream W1, production-readiness brief 2026-10-03. Baseline: `cargo check
--workspace --all-targets` exit 0, `cargo test --workspace` 704 passed / 0 failed
/ 12 ignored.

Method: measure the ceiling read-only first, fix only the smallest diff that removes
a duplication at its root, record every rejection with its numbers. Evidence
commands are quoted verbatim and are read-only.

Tooling left behind (runnable checks, no dependencies):

| script | answers |
|---|---|
| `.agents/work/performance/duplication/scan_duplication.py` | copy-pasted `fn` bodies, redundant bytes |
| `.agents/work/performance/duplication/measure_metadata_ceiling.py` | retained vs transient packument bytes |
| `.agents/work/performance/duplication/dedupe_paths.py` | the store-layout dedupe applied below |

## Verdicts

| # | location | kind | measured before → after | action | evidence command | restore |
|---|---|---|---|---|---|---|
| 1 | `crates/jhol-core/src/registry.rs`, `crates/jhol-core/src/utils.rs` | second source of truth for the store layout (`store`, `store_unpacked`, `<hash>.tgz`, `<hash>.tgz.sri`) | registry **14 → 0** store-layout literals, utils **10 → 0** (broad `\.tgz` count incl. temp names: registry 26 → 2, utils 18 → 8) | **fixed** | `grep -cE 'join\("store"\)\|join\("store_unpacked"\)\|store_dir\(\)\.join\(format!\("\{\}\.tgz' crates/jhol-core/src/{registry,utils}.rs{,.bak-1790981641}` | `cp crates/jhol-core/src/registry.rs.bak-1790981641 crates/jhol-core/src/registry.rs` (same for `utils.rs`, `content.rs`) |
| 2 | shard rule `if hash.len() >= 2 { &hash[..2] } else { "xx" }` | copy-pasted layout rule | 10 copies (registry 8, utils 1, content 1) → 1 (content.rs:76) | **fixed** | `grep -rc 'let shard = if hash.len() >= 2' crates/jhol-core/src/` | as row 1 |
| 3 | `jagr/tropical_sheaf.rs:979 check_snf` ≡ `jagr/simplicial_normal_form.rs:19 check_snf` | same 2-line accessor body, same name | 121 B ×2 | **rejected** | `python3 .agents/work/performance/duplication/scan_duplication.py crates src` | — |
| 4 | `jagr/core_guided_solver.rs:218 formula` ≡ `jagr/core_guided_lp.rs:573 formula` | identical accessor on two types | 54 B ×2 | **rejected** | same as row 3 | — |
| 5 | `jagr_infinite.rs` metadata internals | retained duplication behind the ~12× peak | 22.3 MB packument JSON → 22.2 MB retained (0.99×); `serde_json::Value` tree **279.8 MB = 12.5×** | **deferred**, ceiling measured | `python3 .agents/work/performance/duplication/measure_metadata_ceiling.py` | no change made |
| 6 | `jagr_infinite.rs:2614` `Value::Chart(chart.clone())` | whole-version-space clone | E57: large-test 339→340 MB, next-test 1058→1053 MB (median 5) | **rejected — closed, cited** | `grep -n 'let shard' AGENTS.md` (round 46) | E57 was reverted in its own round |
| 7 | `fetch_dep_tree` retains every packument | retention for the whole resolution | E53 (kept, corrected to noise), E61 prune-in-place −20 % next-test (kept), E62/E67 rejected | **closed, cited** | AGENTS.md rounds 42, 50, 53 | — |
| 8 | `docs/` authored duplicates | same file content twice | 59 duplicate pairs (`archive/2025-optimization` ↔ `archive/reports`, `archive/reports` ↔ `plans`) | **deferred** — outside W1 ownership; preservation rule forbids deleting | `find docs -type f -size +1k \| grep -v node_modules \| xargs md5 -r \| sort \| awk '{if($1==p)n++; p=$1} END{print n}'` | — |
| 9 | `install.rs` 31, `jagrs.rs` 9, `capsule.rs` 4, `daemon_cache.rs` 1, `soul_path.rs` 4, `lockfile_write.rs` 3 | same store-layout literals, outside W1 ownership | 52 literal sites | **request** (see below) | `grep -rn 'join("store")\|join("store_unpacked")' crates src --include=*.rs \| awk -F: '{print $1}' \| sort \| uniq -c` | — |
| 10 | whole workspace | copy-pasted `fn` bodies ≥ 8 normalised lines | 18 groups / 43 sites / 3,048 redundant bytes = **0.16 %** of 1,898,582 body bytes | **rejected** | `python3 .agents/work/performance/duplication/scan_duplication.py crates src` | — |

## What was fixed

`content.rs` already declares the layout ("One place defines the on-disk shape.
Every reader and writer goes through these four functions"). `utils.rs` and
`registry.rs` violated that contract — this is the same defect class as the
original `content-origin` bug (a read root list and a write root list that drift
apart), and it is what the brief's priority 2 asks for.

- `content.rs`: added `unpacked_root(root)`; `unpacked_dir` is now
  `unpacked_root(root).join(shard).join(hash)`. The shard rule exists once.
- `utils.rs`: `store_dir()`, `store_unpacked_dir()`, `sharded_unpacked_path()` are
  thin wrappers over `content::*`. The ambient-cache path stays, the layout does not.
- `registry.rs`: every `cache_dir.join("store")`, `.join(format!("{}.tgz", hash))`,
  `.tgz.sri` and `store_unpacked/<shard>/<hash>` now calls
  `content::{store_dir,tarball_path,sri_sidecar_path,unpacked_dir}`; the seven
  duplicated shard computations are gone. Public behaviour and on-disk paths are
  byte-identical; four local `let store_dir` bindings remain only where the
  directory itself is created.

## Deferred: the JAGR ~12× peak (row 5)

Numbers measured today on the live cache (274 packuments, 17,344 version entries):

```
raw JSON on disk        :     22.3 MB
15-key pruned projection:     22.2 MB  (0.99x raw)
Value-tree proxy (parse):    279.8 MB  (12.5x raw)
```

The stored bytes are already npm-abbreviated, so **retained** duplication is
already taken (E61 prunes the value tree in place, −20 % next-test RSS; E55 removed
the by-value atlas clone, −19 %/−26 %). The remaining factor is the exponent in
`serde_json::Value` itself: one 32-byte enum slot plus a heap allocation per JSON
node, built transiently before the prune. That matches the ledger's conclusion
(E53/E55/E56/E57/E59/E61/E62/E67) exactly, and it is pinned to 12.5× here by a
second, independent method.

The only fix that removes it is to never build the `Value` tree — parse into a
typed record and let consumers take that record. E60 measured its ceiling at
**3.2×** (85.4 MB → 27.0 MB on 82 packuments); E67 implemented the half-measure
(typed record converted back to a pruned `Value`) and measured **+3 %/+26 %**, so
it was reverted. The winning variant is cross-cutting: `fetch_dep_tree` → atlas →
`reconcile_versions` → `lockfile_write` all consume packuments. That is a
multi-round refactor with a real regression surface, not a one-shot inside W1, so
it is deferred with the ceiling on record rather than half-built. The proxy number
is an order-of-magnitude model (per-node overhead), not a Rust allocation
measurement; labelled as such in the script.

## Requests (files outside W1 ownership — not edited)

1. 52 store-layout literals remain in `install.rs`, `jagrs.rs`, `capsule.rs`,
   `daemon_cache.rs`, `soul_path.rs`, `lockfile_write.rs`. Route them through
   `content::*` in the same way; no behaviour change, and it finishes the
   single-origin contract. `install.rs` is the largest (31 sites).
2. 59 duplicated authored doc pairs. Keep both by the preservation rule, but the
   pair list is machine-derivable; a MANIFEST-style record would let the owner
   pick a canonical copy later.
3. Cross-crate copy-paste the scanner found and W1 does not own:
   `jhol-runtime/src/{bytecode_cache,transpile_cache}.rs cache_dir` (312 B),
   `jhol-adapters/src/{js,rust_lang,python}.rs {apply,verify,capabilities}`
   (≈413 B), `jhol-remote/src/lib.rs` ≡ `jhol-graph/src/project_lock.rs`
   `digest_bytes` (115 B). All trivial; recorded so nobody re-measures.

## Verification

```sh
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets   # exit 0
CARGO_BUILD_JOBS=2 cargo test -p jhol-core --lib
CARGO_BUILD_JOBS=2 cargo test --test commands
cargo fmt --all
```
