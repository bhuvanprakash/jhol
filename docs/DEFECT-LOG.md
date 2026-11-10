# Defect log — root causes, not anecdotes

Every entry: what was observed, what the actual cause was, how it was caught, and the
rule it produced. Ordered by how much it taught.

---

## 1. `VersionBitmap::intersect` compared bitmaps with different bases

**Observed.** The resolver reported "no version of p1 satisfies every dependent" for
a two-package world where a solution plainly existed.

**Cause.** Words are offsets from `ordinal_lo`. `intersect` ANDed them positionally.
`from_interval({1..1})` anchors at ordinal 1; intersecting it with a domain anchored
at 0 selected ordinal **0** — a version the range excludes.

**Caught by.** `completeness_against_brute_force`, written as a proof obligation in
the same session.

**Status.** Fix written and verified (base-aware `intersect`/`union`), **held back**:
with correct intersections, `jest` stops resolving, because its tree needs several
majors of `chalk`/`semver` and the flat model requires one version per package. The
old bug had been *accepting* incompatible versions and the nested layout then
produced a plausible tree. The test stays `#[ignore]`d with this history inline.

**Rule.** A set operation on positioned data must assert the position, or it is not
the operation it claims to be.

---

## 2. Five authorities decided "already up to date"

**Observed.** A `package.json` edited on disk to `9.9.9` survived `jhol install`
("Up to date"); a directory the lockfile never authorised was kept forever; yet a
*deleted* package was repaired. Inconsistent.

**Cause.** Five independent predicates, each with different evidence: trampoline
`.jhol-ok` mtime, prepared-tree proof, bundle projection,
`lockfile_fingerprint_matches()`, daemon verdict — plus `nm_fresh()` inside the
installer. A fingerprint proves nothing changed *around* the tree, not that the tree
is what the lockfile authorises.

**Caught by.** Three hand-written reproductions on one project.

**Fix.** `jhol-treecheck` (`Match | Missing | Wrong | Extraneous | Unverified`) as the
single predicate, consulted at the entry point and at the convergence point; the
verdict moved *inside* `lockfile_fingerprint_matches` so every caller inherits it.

**Rule.** A fast path may save work; it may not decide truth. Put the authority at
the chokepoint, not at each call site — gating call sites took four attempts.

---

## 3. The verifier's own walker descended into package internals

**Observed.** A chart/index equality test failed, and pruning was about to delete
`node_modules/a/lib` as an "unauthorised package".

**Cause.** The directory walker queued *package* directories and treated their
children as packages instead of descending only into their `node_modules`.

**Caught by.** The nested/scoped layout test, before any real tree was touched.

**Rule.** A pruner must be tested for what it refuses to delete.

---

## 4. A project lock that raced against itself

**Observed.** Two concurrent installs interleaved and produced a tree matching
neither.

**Cause (first version).** `create_new` then write the pid. Between the two, a
second process read an **empty** file, called the content garbage, treated the lock
as stale, removed it and proceeded — the exact bug the lock existed to prevent.

**Cause (second).** `Drop` never ran because the CLI exits through
`std::process::exit`, so a lock leaked after every install.

**Fix.** `flock`: ownership is the open file description, released by the kernel on
any exit path; the pid inside is diagnostics only.

**Rule.** Prefer the mechanism the OS already gets right. A lock implemented as a
file with content is two mechanisms, and the protocol had a hole.

---

## 5. Zero-byte manifest counted as a complete extract

**Observed.** Warm `jest` installs reported `Added 1 packages` forever.

**Cause.** `store_extract_complete` returned early on the completion marker, so a
finished extract with an unreadable `package.json` (0 bytes) counted as installed.
Every install "repaired" it by re-cloning the same zero bytes.

**Fix.** A manifest that *exists and is empty* is corruption; a manifest that is
*absent* is governed by the marker (my first version required a non-empty manifest
unconditionally and broke two store tests plus the installer gate — extracting a
tarball with no root `package.json` is legitimate).

**Rule.** "Repair" that does not converge is a symptom of corrupted input, not of a
missing step.

---

## 6. `jhol ci` panicked on every invocation

**Observed.** `arg 'engine-strict''s ArgAction should be one of SetTrue, SetFalse`.

**Cause.** The `ci` subcommand never declared the flag; its handler read it. The
two-clap-builder hazard again.

**Fix.** `flag(&m, name)` treats an undeclared flag as *off* (117 call sites), plus a
scripted audit of every arm's read-ids against its declarations.

**Rule.** Reading an option that may not exist is undefined behaviour, not "false".

---

## 7. `--as-of` and license policy were broken by our own pruning

**Observed.** `--as-of` reported "no publish dates" for metadata that arrived
complete (322 `time` entries). License policy saw every package as unlicensed.

**Cause.** `prune_packument_in_place` keeps only `name`, `versions`, `dist-tags`.
The `--as-of` path pruned **twice** — the second call used the install-only key set
and undid the keeping variant. License data was stripped by the per-version
whitelist.

**Fix.** `prune_packument_in_place_keeping(top, …)` retaining `time` and
`license`, and exactly one prune per fetched document.

**Rule.** When data "looks missing", check what upstream deleted before blaming the
source. Three separate features were broken this way.

---

## 8. An unbounded prefetch hung a cold install

**Observed.** Cold `jest` install hit a 600 s timeout with nothing installed.

**Cause.** The frontier followed `peerDependencies` *and* every published version of
every package, so it never closed.

**Fix.** `missing_packages_bounded`: no peers, hard name cap, capped rounds, 20 s
ceiling, sequential fallback. Three tests pin the bound.

**Rule.** An optimisation must be unable to be unbounded. A hang is worse than slow.

---

## 9. Replay lane answered a question it was not asked

**Observed.** `jhol install zod` in a directory with a matching manifest replayed
`express`'s 68-package tree.

**Cause.** The dependency-manifest cache key hashed only `package.json` + platform,
then published a manifest-keyed alias even when the request named packages.

**Fix.** Publish manifest-keyed aliases only when the request has no packages; key
version bumped v1→v2 in both `jhol-core` and `jhol-trampoline` (they must match
byte-for-byte).

**Rule.** A cache key must include every input that changes the answer.

---

## 10. Transitive `npm:` aliases went to the registry under the alias name

**Observed.** `pretty-format`'s `"@jest/react-is-18": "npm:react-is@^19"` produced
HTTP 404 and stopped a jest tree.

**Cause.** The alias name was treated as the package name in four resolution stages.

**Fix.** Expand `npm:` aliases at each stage; `Instance::installed_as` records the
tree name while resolution uses the real package; a post-install pass materialises
the alias links.

**Rule.** An alias is two names, and code must say which one it means.

---

## 11. Graph-edge diseases fixed together

* **`i < 4096` cliff** — a dense-index fast path gated on an arbitrary constant; on
  a 200k array 98 % of accesses took the slow path. Guard is now `i < len`.
* **Sparse precedence** — writes above `SPARSE_FROM` landed in a sparse map while
  reads consulted the dense array first, so recent writes were invisible.
* **Object numeric keys** — a fused literal stored `"5000"` while computed access
  produced `Idx(5000)`; fixed with `key_for_receiver`, scoped to objects after it
  broke `charCodeAt`.
* **`INLINE_PROPS = 4`** — a 5-property object heap-spilled; 6 fixed a 1.2× deficit.
* **`var` in loop closures** — the loop-cell pre-pass ignored `var` initialisers
  while the body cellified them, so a fused test compared a cell pointer as a number
  and the loop exited once.
* **JIT nested literals** — escape analysis checked only three fields while
  `NewObjectLit` consumes a value window; `jt4` summed 241 080 instead of 343 200.

**Rule.** Every fused/optimised path needs a second, independent way to observe the
same fact (corpus vs interpreter, index vs JSON, verifier vs tree).

---

## 12. `cp -r` of untracked work is not a review

Two of my patches silently no-opped because the anchor text did not match, and I read
a benchmark instead of verifying the edit landed — once reporting a number produced
by the *previous* binary. A `timeout`-killed build leaves the old binary in place, so
"0 errors" can describe code that was never compiled.

**Rule.** Verify the patch applied before believing the measurement. `stat` the
binary when a build might have been killed.

---

## 13. The verifier demanded packages that cannot exist here

**Observed.** `vite` installed cleanly and then failed verification:
`node_modules/@esbuild/aix-ppc64 is missing`. `jhol ci` also refused the tree with
"integrity entries missing".

**Cause.** esbuild publishes one package per target; the lockfile records all 21 and
each carries `os`/`cpu`. `verify_tree` required every lockfile entry to be on disk,
so 20 packages that cannot exist on darwin/arm64 were reported missing. The same
entries have no `integrity` (nothing was ever downloaded), so the strict-lockfile
gate tripped too.

**Fix.** `entry_platform_applies` (npm semantics: allow-list, `!` deny-list,
`libc` only on Linux) excludes inapplicable entries from what must exist and from
the integrity requirement; the writer now falls back to the packument's hash when a
manifest names a tarball without one.

**Caught by.** A ten-manifest sweep (install → verify → idempotency → `ci` →
verify), not by unit tests. That is the argument for the sweep existing.

**Rule.** A correctness check must know what is *legitimately absent* before it
calls anything missing.

---

## Tests born from these

| defect | test |
|---|---|
| 1 | `completeness_against_brute_force` (ignored, reason inline) |
| 2 | `jhol-treecheck` suites; tamper → repair manual sequence |
| 3 | `nested_and_scoped_entries_compare`, `prune_removes_only_unauthorised` |
| 4 | `project_lock` tests (re-entrancy, timeout names holder) |
| 5 | `store_extract_guard_tests` (empty / repaired / absent manifest) |
| 6 | `compiled_spec`-style audit + `flag()` |
| 7 | `prune_keeping_tests`, `as_of_tests`, `license_policy_tests` |
| 8 | `bounded_frontier_tests` |
| 9 | manifest-keyed alias tests |
| 10 | `jagr_nest` alias cases |
| 11 | `crates/jhol-vm/tests/js/*` (44 pairs), `adv/*` |

## 14. A store entry with a completion marker and nothing else

**Symptom.** A warm-cache install of `express@4.18.2` reported `Added 56 packages`, left
`node_modules/express` as an **empty directory**, and the lockfile listed 71 packages. The
tree was missing 14 of them, including `express` itself. Cold cache: 71, correct. So every
fast path was answering "installed" for content that did not exist.

**Root cause.** `store_extract_complete` accepted a directory that contained the completion
marker, and the corrupt entry for `express` contained the marker and **one file** — the
marker. The extract had been truncated after the marker was written. `project_package`
then linked that empty directory, counted the package as installed, and moved on.

**Fix.** A marker over a directory with no other entry is corruption, not "a tarball
without a manifest". That case is now rejected, the entry re-extracted, and the tree
converges (56 → 68 → 71 across two runs, then `Up to date`).

**Rule.** *A completion marker proves a write finished; it does not prove the write
produced anything.* Every "already done" test needs a content check beside it, and the
cheapest one — "is there anything here at all" — is the one that was missing.

