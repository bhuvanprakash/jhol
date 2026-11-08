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
