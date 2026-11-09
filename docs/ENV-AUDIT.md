# Environment-variable audit

180 `JHOL_*` variables and 15 `JAGR_*` exist today. That number is a symptom, not
an achievement: each was added because one lane needed an escape hatch, and the
count is what "fix the default" was replaced with, one decision at a time.

This audit classifies them so the reduction is a plan rather than a mood.

---

## 1. Diagnostics (24) — keep, they change nothing

`JHOL_DEBUG_*` (12), `JHOL_TEST_*` (8), `JHOL_PROFILE_*`, `JHOL_HTTP_TIMING`,
`JHOL_VERBOSE`, `JHOL_JIT_DUMP`.

These print or dump; they cannot change a resolution or a tree. They are the reason
most of this project's defects were found at all (`JAGR_PROFILE_CHART=1` located the
141.6 ms mask assembly) and should stay.

**Obligation:** none. **Count impact:** none (they are not behavioural).

---

## 2. Escape hatches for a default that was once wrong (the real target)

Each of these exists because some lane's default behaviour was unsafe or
incomplete. Retiring one means making the safe behaviour the only behaviour, and
removing the flag — not renaming it.

| variable | why it exists | what retiring it requires |
|---|---|---|
| `JHOL_PRUNE_EXTRANEOUS` | pruning was unsafe while the lockfile could be incomplete | lockfile writer records everything it materialises (see `DEFECT-LOG.md` §5), then pruning becomes the default and `jhol verify --prune` stays as the explicit form |
| `JHOL_SINGLE_PASS` | the single-pass layout was a fix for a 273–445 s path | once the instance resolver handles every conflict case, the flat/single-pass path goes away with it |
| ~~`JHOL_NEST_RESOLVER`~~ | **retired** — see "Retired" below | — |
| `JHOL_NO_SELECTION_CACHE` | the selection cache needed an off switch while it was new | cache is verified on load; keep as a diagnostic |
| `JHOL_LICENSE_ALLOW_UNKNOWN` | a real policy needs an explicit exception | keep — the escape hatch *is* the feature |
| `JHOL_STRICT_PEER_DEPS` | peers are advisory by default (npm semantics) | keep — it selects between two defensible behaviours |
| `JHOL_NO_HARVEST`, `JHOL_NO_CACHE`, `JHOL_NO_DAEMON`, `JHOL_NO_PREFETCH`, `JHOL_NO_SPECULATE`, `JHOL_NO_FLOODGATE`, `JHOL_NO_HTTP2`, `JHOL_NO_PACKUMENT_TTL`, `JHOL_NO_TRANSITIVE_CACHE` | each disables an accelerator | retire one at a time by proving the accelerator is never wrong (the tree verifier is the tool), then delete both the accelerator's escape hatch and, where it is redundant, the accelerator |

---

## 3. Tuning (document, do not expand)

`JHOL_DOWNLOAD_CONCURRENCY` (default `(cpus×4).clamp(32,128)`),
`JHOL_CACHE_INSTALL_CONCURRENCY`, `JHOL_LOCK_TIMEOUT_SECS`, `JAGR_SEARCH_BUDGET`,
`JAGR_RECONCILE_STEPS`, `JHOL_JAGR_MAX_DEPS`, `JHOL_MAX_DISCOVERY_WORK`,
`JHOL_MAX_GRAPH_PACKAGES`, `JHOL_DAEMON_TIMEOUT_SECS`.

Measured positions: download concurrency is already at the practical ceiling (the
payload phase is registry-bound, not concurrency-bound), so these are for unusual
machines, not for making the common case fast.

---

## 4. Feature switches that are part of the interface (keep)

`JHOL_AS_OF`, `JHOL_ALLOW_LICENSE`, `JHOL_EXCLUDE_VULN`, `JHOL_OMIT_DEBUG`,
`JHOL_PM_STRICT`, `JHOL_RUNTIME_ENGINE` (unknown values are rejected),
`JHOL_ARENA_BYTES`, `JHOL_VM_HEAP_LIMIT`, `JHOL_VM_NO_GC`, `JHOL_JIT_ENABLE`.

Set by a flag or by a deliberate runtime choice; documented in
`CLI-REFERENCE.md` and the engine doc.

---

## The rule for new ones

> A new environment variable must name a behaviour that cannot be a default, and
> the commit that adds it must say which default it is avoiding and what would make
> that default safe.

Every variable in §2 that fails this test is debt; every one in §1 and §4 passes by
definition.

---

## Status

| class | count | action |
|---|---:|---|
| diagnostics | 24 | keep |
| escape hatches | ~40 | retire one per default that becomes safe |
| tuning | ~20 | document |
| interface | ~30 | keep |
| other/unclassified | rest | classify before adding more |

The honest summary: **nothing here is reduced yet.** The classification is the
prerequisite — retiring an escape hatch without fixing the default it guards is how
a project gets a variable named `JHOL_NO_UNSAFE_THING` and keeps the unsafe thing.

### Added: `JHOL_PREDICTIVE_PREFETCH`

Speculative prefetch of predicted packages during install. **Default off** — it
cost 367–453 ms on a six-package install whose cached path costs 50 ms, because it
fetches predicted packuments from the network before the requested ones. Set to
`1` to restore it. `jhol prefetch` uses the same predictor and is unaffected.

---

## Retired

Retirement means the variable is gone from the code, the safe behaviour is the only
behaviour, and the documentation says so. Renaming is not retiring.

| variable | retired | why it existed | why it could go |
|---|---|---|---|
| `JHOL_NEST_RESOLVER` | this session | the instance resolver was opt-in while it was unproven | it is the only *first* attempt now; the flat resolver stays as the automatic fallback, not as a mode. That resolver cannot represent a tree with two majors of one package, so an escape hatch to it was a way to be surprised, not a way to be safe. Verified afterwards: normal install (71 packages, `verify` clean), dated install (58 packages), and `JHOL_NEST_RESOLVER=0` now has no effect |

Two more are queued behind work rather than judgement: `JHOL_SINGLE_PASS` needs the
single-pass path deleted with it, and the `JHOL_NO_*` accelerator switches each need the
accelerator proven never wrong for a release before the switch goes.
