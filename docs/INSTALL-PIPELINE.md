# The install pipeline

Everything between `jhol install` and a directory tree, in the order a decision is
actually made. Source: `src/main.rs` (dispatch + lanes), `crates/jhol-core/src/install.rs`
(~8.5k lines of lanes and materialisation), `content.rs` (store), `origin.rs`
(metadata cache), `lockfile_write.rs` (tree planning).

---

## 0. One freshness authority, before any lane

```
main()
  └─ plain_install_fully_settled()          // plain `install`, no package args
        └─ tree_state::verify_tree / reconcile
              Match      -> print "Up to date (N packages)" and exit
              Extraneous -> warn, keep the tree, exit (unless JHOL_PRUNEEXTRANEOUS)
              otherwise  -> continue into the real install
```

Why it exists: five separate components used to decide "already up to date" with
five kinds of evidence — a `.jhol-ok` mtime, a prepared-tree proof, a bundle
projection, `lockfile_fingerprint_matches()`, a daemon verdict — plus `nm_fresh()`
inside the installer. A fingerprint can prove nothing changed *around* the tree; none
could prove the tree is what the lockfile authorises, so a package.json edited
inside a package and a directory the lockfile never mentioned both survived.

`verify_tree` answers exactly one question — for every package the lockfile
authorises, is it on disk at the locked version, and is everything on disk
authorised? — reading one small `package.json` per package and no archives
(`crates/jhol-treecheck`, 12 tests).

`lockfile_fingerprint_matches()` now includes that verdict, so every caller of the
old predicate inherits the authority instead of re-deriving it.

---

## 1. The project lock

`project_lock::ProjectLock::acquire(".")` — `flock` on `.jhol/install.lock`,
held for the life of the process for commands that write the tree
(`install`, `i`, `ci`, `add`, `remove`, `update`, `dedupe`, `prune`); read-only
commands never take it, and warm installs return before it because they write
nothing.

* `flock`, not a pid file: ownership is the open file description, so there is
  nothing to read and nothing to race; the kernel releases it on any exit path,
  including `std::process::exit` (which the CLI uses widely and which bypasses
  `Drop`).
* the pid written inside is diagnostics only — it tells a waiting user who they are
  waiting for;
* re-entrant within a process, because a lifecycle script that runs `jhol install`
  would otherwise deadlock against itself.

---

## 2. Lanes

| lane | gate | what it does |
|---|---|---|
| 0 warm bypass | `can_use_instant_lock_proof` + verify | skip everything; the tree verdict is re-checked first |
| capsule | `can_reuse_prepared_layout` (empty pkgs or `from_lockfile`) | mmap a prepared tree, verify, project |
| L0 projection | bundle cache present | clonefile a cached bundle into `node_modules` |
| fresh replay | manifest hash matches a published bundle | replay it |
| traditional | everything else | expand → resolve → download → extract |

Every lane's success is then checked by `reconcile_tree` before the summary prints,
and the summary says `Repaired (...)` when something was fixed rather than
`Up to date`. Lanes may save work; they may not decide truth.

---

## 3. Resolution path

```
roots = manifest deps (+ requested packages, + workspace links)
flat expansion (expand_with_transitive_dependencies)
   ├─ no conflict  -> install flat
   └─ conflict or --as-of/--allow-license
        single-pass layout (reuses flat picks)   [disabled under a policy]
          └─ rejected by the completeness guard
               -> resolve_full_tree_nested      <- instances, multi-major
                     └─ on error: resolve_full_tree (previous path)
```

`--as-of` and `--allow-license` force the instance resolver and forbid the
single-pass shortcut: both are resolution inputs the flat path does not implement,
and a dated install that quietly resolved to today would be worse than one that
refuses.

See `RESOLVER-JAGR.md` for the resolver itself.

---

## 4. Metadata

* **abbreviated** packuments (`application/vnd.npm.install-v1+json`) are what the
  cache stores and what normal installs use; they omit `time` and per-version
  `license`.
* **full** packuments are fetched only when a feature needs them
  (`registry::fetch_metadata_full`), which is why dated and license-policy installs
  are slower.
* `prune_packument_in_place` keeps `name`, `versions`, `dist-tags` and drops the
  rest. `prune_packument_in_place_keeping` retains `time` and per-version `license`
  while those features are active — forgetting this broke `--as-of` twice and
  license policy once.
* the binary index (`jagr_index`) is written beside each cached packument and read
  in preference to gunzip+JSON: 426 ms → 15.2 ms for 344 packages.

---

## 5. Download and store

* Concurrency is `(cpus × 4).clamp(32, 128)` (`JHOL_DOWNLOAD_CONCURRENCY`), through
  a persistent worker pool; the HTTP client is a pooled `ureq::Agent`. There is no
  further parallelism available — the payload phase is registry-bound.
* Content is addressed by SRI hash: `store/<hash>.tgz` with a `.sri` sidecar, and
  `store_unpacked/<shard>/<hash>/` for extracted trees.
* **Completeness rule** (`store_extract_complete`): a completion marker plus a
  manifest that *exists and is non-empty*. A manifest that exists but is empty is
  corruption — a zero-byte `package.json` with a marker made every install re-clone
  the same corruption, which is what kept a jest tree permanently "needing repair".
  A manifest that is *absent* falls back to the marker, because a tarball without a
  root `package.json` is legitimate.

---

## 6. Materialisation

* directories are created first (scope dirs included), then extraction runs in
  parallel;
* store-link (clonefile/CoW) is tried before any copy;
* tree paths come from the resolver's placement (see `RESOLVER-JAGR.md` §4);
* materialising an existing 266-package tree from the store measured 379 ms.

---

## 7. After the tree

| step | source |
|---|---|
| tree verification / repair / prune | `crates/jhol-treecheck` |
| lockfile write (npm v3-shaped) | `lockfile.rs`, `lockfile_write.rs` |
| lifecycle scripts (**not run** without consent) | `lifecycle.rs` |
| Script Receipts: SHA-256 over name@version + exact script text + tarball SRI | `.jhol/script-receipts.json` |
| patches re-applied after every lane | `patch.rs` |
| `engines.node` enforcement (warn, or fail with `--engine-strict`) | `engines.rs` |
| omit filtering (`--omit=dev|optional|peer`) | `omit.rs` |

Scripts are opt-in: an install that skips them prints what was skipped, and an
approved script that fails fails the install.

---

## 8. Failure modes, and what you see

| situation | behaviour |
|---|---|
| tree disagrees with the lockfile | fast lane refused, real install repairs, summary says `Repaired` |
| package the lockfile never authorised | warning naming it; pruned only with `JHOL_PRUNEEXTRANEOUS=1` |
| lockfile that omits packages it installed | reported as unverified, never pruned against |
| another install running | waits, naming the pid; `JHOL_LOCK_TIMEOUT_SECS` bounds it |
| corrupt store entry | detected by the completeness rule and re-extracted |
| dated install with no publish dates | exit 1, zero packages installed, reason printed |
| license policy violation | exit 1, names the package and range |
| metadata missing | fetch and retry (`MissingPackument` is a request, not a failure) |

---

## 9. Numbers

| phase | value |
|---|---|
| chart assembly (275 packuments) | 11.4 ms |
| cache view via index (344 packuments) | 15.2 ms |
| metadata prefetch, bounded (400 packuments) | 9.8–12.3 s |
| tarball downloads (266 packages) | 19–54 s (registry-bound) |
| materialise from store (266 packages) | 379 ms |
| warm install, 266-package tree | 18 ms |
| dated install | 21–24 s |

## Predictive prefetch is opt-in, because measurement said so

The install loop used to call the spectral predictor synchronously, before the
packages the user asked for, and `DaemonCache::prefetch` goes to the network when
the predicted packument is not cached.

Measured on a six-package install with a warm cache:

| | wall time |
|---|---:|
| online, prefetch on (old default) | 367–453 ms |
| `--offline`, i.e. prefetch skipped | 50 ms |
| online, prefetch off (new default) | 38–110 ms |
| online, `JHOL_PREDICTIVE_PREFETCH=1` | 40 ms (this time, cache warm) |

Speculation cost seven times the transaction it decorated. Nothing was deleted:
the predictor still exists, still powers `jhol prefetch`, and one variable restores
the old behaviour. The rule it produced is the one that keeps recurring — **a
speculative path may save work; it may not charge the user for work they did not
ask for.**

Express, 71 packages, warm cache, `node_modules` wiped: 49 ms and 38 ms across two
runs, and `jhol verify` reports the tree matches the lockfile.

## A version document is a projection, not a new fact

Adding one dependency to a six-package tree made **seven** requests to
`/<pkg>/<version>`, ~350 ms each, sequentially — for packages whose packuments were
already on disk. `fetch_manifest` went straight to the network for every selector.

A `/<pkg>/<version>` document is a projection of the packument we stored, so it is
now answered from the cached packument when one exists (`latest` resolved through
`dist-tags`), and the network is used only for a package never seen before.

Two related fixes in the same measurement loop:

* **duplicate URLs in one process.** The same package was fetched twice in a single
  install (2 × 540 ms for a 2.5 KB document). `http_client::get` now memoises bodies
  for 30 s — bounded by age, by body size (4 MB) and by entry count (512), so it
  cannot become a stale cache or an unbounded one.
* **speculative prefetch is opt-in** (see above): 367–453 ms of a 50 ms install.

| adding `lodash@4.17.21` to a warm six-package tree | wall time | network |
|---|---:|---:|
| before | 3.13 s | 7 requests |
| after | **0.77 s** | **0** |

`jhol verify` reports 7 packages matching the lockfile.

## Adding one package to an existing tree: where 6.3 s actually goes

`jhol install lodash@4.17.21` on a project that already has 71 packages took **6.3 s
with zero HTTP requests**. Profile (`JHOL_PROFILE_INSTALL=1`), same run:

| stage | time |
|---|---:|
| `nested_resolve` (whole function) | 6190 ms |
| — prefetch + parse of the frontier (196 packuments) | **≈4700 ms** |
| — `nest_solve` (the resolver itself) | 1417 ms |
| — `nest_paths` | 0 ms |
| — `nest_snapshots` (72 packages) | 14 ms |
| `lockfile_layout` (writing the tree) | 60 ms |

So the resolver is 1.4 s and the tree write is 60 ms; the rest is loading packuments.
The frontier `missing_packages_bounded` returns is the transitive *name* closure — 196
documents when the tree needs 72 — and every one is read from cache and parsed into a
`serde_json::Value` before the resolver runs.

Two comparisons that keep this honest: the same 73-package tree materialises in **52 ms**
from an existing lockfile (`jhol install` with `node_modules` wiped), and
`--lockfile-only` for the same add is **0.08 s** because it never enters this path.

The fix was in the frontier itself, not in parallelism — nothing here touches the
network. `missing_packages_bounded` walked the dependencies of **every published
version** of every reachable package. The resolver picks the *newest* satisfying
release, so the older releases' dependencies are mostly never needed. It now follows the
newest release only, and when a range does force an older one the resolver's
`needs_fetch` names it.

| adding `lodash` to a 71-package project | before | after |
|---|---:|---:|
| documents loaded into `world` | 196 | **75** (tree: 72) |
| `nested_resolve` | 6190 ms | **732–1281 ms** |
| total | 6.3 s | **0.80–1.37 s** |

A three-dependency manifest (express + vite + lodash, 83 packages) installs in 358 ms and
`verify` is clean. Fewer documents loaded also means fewer wrong guesses: the frontier is
now within 4% of the tree size instead of 2.7× it.

Kept on the record: two attempts that did **not** work. Capping the fetch budget at 96
helped small adds but would have made a large cold install fetch the remainder one request
at a time; adding a resolve attempt after each 48-name slice made it worse (286 documents,
7.1 s) because the over-approximating frontier kept feeding names the tree did not need.
Fixing the frontier fixed both.

## One missing name per resolve pass

The fetch-and-retry loop exists because the resolver reports a missing package instead of
failing, which is the right contract — but it fetched **one** package per pass and then
re-resolved everything. Measured on a one-package add: **24 solve passes**, 3.4 s of the
5.1 s install, each pass re-narrowing a 75-package chart.

It now fetches the current frontier (the same bounded helper the prefetch uses) in parallel
batches, so a pass adds everything the resolver can currently name. Ten passes remain, and
they are the interesting ones: a package whose *older* release declares a dependency the
newest release dropped is not in the frontier, so it is discovered one layer at a time.
That is correct behaviour, not waste.

| add `lodash` to a 71-package tree | before | after |
|---|---:|---:|
| solve passes | 24 | **10** |
| `nest_solve` | 3425 ms | **443–705 ms** |
| total | 5.09 s | **1.16–2.16 s** |

What is left is per-solve cost, not solve count: `resolve_with_options` builds its
per-package view (sorted versions, per-version dependency maps) fresh on every call, so ten
passes rebuild the same 75 views ten times. The lever is a resolver object that owns that
cache for the duration of a fetch-and-retry loop — a structural change, and the next
measurable target here.

### Two corrections to the numbers above

**Machine state dominates this path.** The identical binary measured `nest_solve` at
**143 ms per solve** in one session and **20 ms per solve** in another: the add fell from
1.16–2.16 s to **0.484 s** with no code change. Any single reading here is worthless
without repeats; the medians above are the useful part and the ranges are honest.

**A layer loop in the retry branch did not help and was reverted.** Fetching the frontier
again after inserting documents — so that each newly visible dependency is loaded in the
same pass — left the solve count at **10**. The reason is now clear: those ten packages
are needed by *older* releases, and the frontier follows the newest release only, so it
cannot name them no matter how often it is asked. Anything that fixes it must either walk
all versions (196 documents, ~4.7 s of parsing — worse than the ten solves) or cache the
resolver's per-package view, which is the lever named above.

## Warm install: the honest scoreboard

express@4.18.2 (71 packages), clean cache, order alternated between the two tools, package
count taken after every run:

| scenario | jhol | bun 1.4.4 | |
|---|---:|---:|---|
| no-op (tree present, nothing to do) | **22 ms** | 21 ms | parity |
| `node_modules` wiped, rebuilt | **39–45 ms** | 34–38 ms | parity, within ~10% |
| cold, fresh cache | **0.83–1.13 s** | 1.49–2.01 s | 1.8× ahead |

Three numbers from this session had to be corrected before the table above was true:

* **38–49 ms warm** — measured when a corrupt store entry made installs produce 56 of 71
  packages (see `DEFECT-LOG.md` §14). It was timing less work.
* **105–130 ms warm** — the same damaged cache, being *repaired* on every run.
* **25–27 ms warm** — real, but only with nothing else running: the store's pages were in
  RAM. Interleaving with bun evicts them, and an order-controlled A/B shows 52–60 ms.
  A benchmark that only measures the hot case is measuring the page cache.

Where the remaining ~20 ms is: per-package materialisation, ~0.4 ms for each of 71 packages
(a `clonefile` plus `materialize::finish`), against bun's ~0.25 ms. Closing that means
making the per-package step cheaper — fewer syscalls, batched marker writes, and folding the
post-install verification into the pass that already touches every file — not changing the
strategy. A whole-tree `clonefile` was measured and is **slower** (`cp -cR` of 737 files:
114 ms), so that door is closed with evidence.

### How the warm rebuild got to parity

Two measurements, one of which corrected the other.

**A recursive `clonefile` is 12–15 ms; `cp -cR` is 114 ms.** I had rejected a whole-tree
snapshot because `cp -cR` on this tree took 114 ms — but `cp -c` clones *per file*, while
`libc::clonefile` on a **directory** is a single kernel call that snapshots the whole tree.
`examples/clone_probe.rs` measures the right one. The idea was not wrong; the reference was.

So a verified tree is kept at `.jhol/prepared`, keyed by a hash of the manifest and
lockfile, and restored with one clone when `node_modules` is absent:

* the snapshot is only written after `verify_tree` reports `Match`, so it never pins an
  unverified tree;
* it is only used when the fingerprint matches and `node_modules` is **absent**;
* the clone is **verified again** before anything is reported, and deleted if it fails.

**Verification was the other half.** `jhol verify` on this tree cost 41 ms — more than bun's
entire install — because the freshness report verified, then called `reconcile_tree`, which
verified again. One walk when the tree already matches.

| warm rebuild of 71 packages | before | after |
|---|---:|---:|
| per-package projection | 52–60 ms | — |
| snapshot clone + one verify walk | — | **39–45 ms** |
| bun 1.4.4 | 34–38 ms | 34–38 ms |

Remaining ~5 ms is the verification walk itself; the next lever is making that walk cheaper
(it reads 71 manifests and stats 140 directories), not the clone.

### Where a 40 ms warm install actually goes

Measured in-process, not guessed (`examples/verify_probe.rs`, `examples/clone_probe.rs`):

| piece | cost |
|---|---:|
| process start (`jhol --version`) | **~20 ms** |
| `verify_tree` on 71 packages | **1.6 ms/call** |
| recursive `clonefile` of 738 files | **12–15 ms** |
| rest of the install work | ~2 ms |

So the warm install is **startup + one clone**, and verification — which I had blamed at
41 ms — is 1.6 ms. The 41 ms was the whole freshness report (config, lockfile parse, two
walks, summary), not the tree check.

Consequences, in order of size: the way to beat bun warm is to cut **startup**, not the
installer. `jhol --version` is ~20 ms against bun's ~17 ms, so there is little headroom there
either; both tools are dominated by process start at this tree size. The clone (12–15 ms for
738 files) is at the filesystem's floor for metadata copy — `cp -cR` needs 114 ms for the
same tree.

What this rules out, with numbers: a hardlink farm per file (≈0.4 ms/package, worse), a
whole-tree `cp -c` (114 ms), and further work on `verify_tree` (1.6 ms).

### The final breakdown, measured inside the process

`JHOL_PROFILE_STARTUP=1` prints the fast path's own numbers; the rest is process start.

| piece | cost | note |
|---|---:|---|
| process start → `main` | **~18 ms** | bun ~17 ms; stripping the binary changes nothing (already stripped) |
| config load | **0.04 ms** | not the cost it looked like |
| snapshot clone | **18–21 ms** | 738 files; `cp -cR` needs 114 ms, a per-file hardlink farm ~0.4 ms/package |
| freshness report (verify) | **1.6–2.3 ms** | |
| total in the fast path | **~20–24 ms** | |

That is the whole warm install: process start plus one clone plus a 2 ms verification.
Beating bun here would mean beating the OS at cloning a 738-file tree or beating process
start — and both tools sit within a millisecond of each other on startup.

What was *not* worth doing, each measured first: stripping (no change), a `cp -c` tree copy
(114 ms), a per-file hardlink farm (0.4 ms/package, and it shares inodes — the model-store
trap), symlinking node_modules to the snapshot (no isolation), and further work on
`verify_tree` (1.6 ms).

### The floor, and why the remaining gap is not ours

Two more measurements that close the question rather than open it.

**Process start is the platform, not the binary.** A trivial `fn main(){println!()}` compiled
with `rustc -O` takes **17–18 ms** on this machine — the same as our 17 MB binary and the same
as `bun --version` (~17 ms). So a warm install's first 18 ms are macOS's, and no amount of
work on our side changes them.

**clonefile does not parallelise usefully.** Cloning one recursive tree of 73 entries:
**9–14 ms**; cloning the same entries concurrently across 10 threads: **10–14 ms**. The
metadata copy is already efficient per call; splitting it adds thread overhead without
overlapping the work.

Which means the whole warm install — `39–45 ms` — is:

```
17-18 ms   process start  (platform floor; identical for a hello-world binary)
19 ms      one clonefile  (kernel's cost for 738 file metadata entries)
2 ms       verification   (our own work, and the only part we control)
```

Against bun's 34–38 ms that is parity, and the honest statement is that both tools are
platform-bound at this tree size. The remaining lever is not an optimisation; it is a smaller
tree or a faster filesystem.

### Correction: the harness was inflating both sides

Every timing reported earlier in this document used `python3 -c 'time.time()'` **after** the
command, so python's own startup (~18 ms) was counted inside the delta. Re-measured with
bash's `time` builtin — no subprocess — the true numbers are:

| | jhol | bun 1.4.4 | |
|---|---:|---:|---|
| no-op warm | **8 ms** | 17 ms | ahead |
| warm rebuild, 71 packages | **26–29 ms** | 20–22 ms | ~1.3× behind |
| `--version` | 6 ms | 7 ms | parity |
| hello-world `rustc -O` | 8 ms | — | the platform floor |

So startup is ~6–8 ms for both tools (the "18 ms" was the harness), and a warm rebuild is
clone-dominated rather than startup-dominated.

### The snapshot was never used, and why

`save_prepared_snapshot` was called inside `install_package`, but the CLI rewrites
`package-lock.json` after that returns — so the recorded fingerprint never matched and
`try_prepared_clone` declined every time (`taken=false`). Saving at the end of the CLI
install, after every write, made the fast path real: warm rebuilds went from ~40 ms (normal
projection path) to **26–29 ms**, with `taken=true` on every run.

This is the same defect class as the rest of this document: *a cache keyed on inputs must be
keyed on their final value*. The snapshot existed, was correct, and was unreachable.
