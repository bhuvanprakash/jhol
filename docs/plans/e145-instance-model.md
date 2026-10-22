# E145 — instance-aware dependency linking

## The design rule

> A lockfile stores **instance placement** — the slot for each resolved package *instance*
> (`Documentation/LOCKFILE_DESIGN.md`). So an installer's unit is the **instance** (name, version, slot),
> never the package *name*.

Three consequences, which together fix the nested-version bug:

1. **Identity is (name, version, slot).** Anything keyed by name alone cannot represent a tree where
   `debug` exists at both 4.4.3 (nested) and 2.6.9 (hoisted).
2. **The store is the authority for which versions exist** — the project tree is only a projection of it.
   A version that is in the store but not hoisted is still a valid link target, and a cached tarball
   without an unpacked directory does not exist yet (hence `ensure_store_unpacked_for_tarball`).
3. **Links are chosen per dependent, by range**, preferring a version that already satisfies the
   declaration (that is what preserves a nested copy) and never linking a version that violates it.
   When nothing satisfies, fall back to the best available and say so under `JHOL_DEBUG_LAYOUT` —
   a missing link is a hard runtime failure, a wrong one is silent, so the fallback is deliberate.

## Pipeline ordering (the part that took ten rounds)

The linking pass **collects the tree it walks**, so it must run *after* materialization completes:
run mid-materialization and nested copies (created by the pass itself) are invisible to it. That single
ordering fact was the last root cause of E145.

## Harness (DRY)

| file | purpose |
|---|---|
| `scripts/e145/model_texts.py` | the model source texts, in the repo |
| `scripts/e145/apply.py` | apply (`--revert` to undo), idempotent |
| `JHOL_DEBUG_LAYOUT=1` | logs `[inst]` accept/reject per entry, `[collect]` summary, `[extract]` branch, `[link]` decisions |

## Verification (must all hold)

```
agent-base → debug        4.4.3   (npm's lockfile requires it)
https-proxy-agent → debug 4.4.3
runtime load of all 19 deps        19/19   (axios included)
jhol install (own resolution)      4.4.3
copy mode                          all real deps load
```
