# Project Fingerprint and Lockfile Reuse Design

## Objective

Reduce Jhol no-op and lockfile-only latency without weakening dependency resolution, integrity checks, offline behavior, lifecycle policy, or the four-layer JAGR resolver. Preserve the current warm-install advantage over Bun and reject any repeatable performance regression.

## Measured Baseline

The current release binary was compared with Bun 1.3.14 using the repository's six-case local harness and three-run medians:

| Case | Jhol | Bun |
|---|---:|---:|
| Warm install | 251 ms | 424 ms |
| No-op install | 275 ms | 176 ms |
| Offline install | 238 ms | N/A |
| Lockfile-only | 1489 ms | 501 ms |
| Cached single-package add | 565 ms | 315 ms |
| Repeat-warm install | 598 ms | 525 ms |

The benchmark fixture must remain identical before and after. Final reporting will use repeated medians and identify outliers rather than selecting a favorable single run.

## Problem

Jhol stores one `~/.jhol/global_fingerprint.json` value containing only the most recently installed lockfile hash. Different projects overwrite that value. The no-op check can therefore miss valid projects, and it does not include `package.json`, so manifest changes are not part of the identity.

The lockfile-only command always calls `resolve_full_tree` and rewrites `package-lock.json`, even when an existing complete lockfile already represents the current root manifest. This repeats registry and resolution work without changing output.

## Design

### Project-scoped install fingerprint

Create a deterministic project identity from the canonical project directory. Store its fingerprint record under Jhol's cache/home namespace using that identity rather than one process-global filename.

The record contains:

- a format/version marker;
- the xxh3 hash of `package.json` bytes;
- the xxh3 hash of `package-lock.json` bytes.

The no-op path returns early only when:

1. `node_modules` exists;
2. both current files exist and hash successfully;
3. the project-scoped record parses and has the supported format;
4. both hashes match.

Missing, malformed, legacy, or mismatched records are cache misses and execute the normal install. No error is hidden and no dependency validation is skipped after a miss. The legacy global fingerprint is not trusted for a hit.

The record is written only after a successful install and lockfile write. Its parent directory is created as needed. A temporary file plus rename is preferred so interruption cannot leave a trusted partial record.

### Validated lockfile-only reuse

Before calling `resolve_full_tree`, inspect the existing `package-lock.json`. Reuse it only when all of these conditions hold:

1. the lockfile is readable and structurally valid;
2. required integrity information is complete under Jhol's existing integrity rule;
3. the root dependency sections represented by the lockfile exactly match the relevant manifest dependency sections;
4. every root dependency has a resolved lockfile entry compatible with its requested range.

When reusable, `install_lockfile_only` returns success without registry access or rewriting identical bytes. Any uncertainty falls back to the existing full native resolution and writer. This is an idempotence optimization, not a new resolver and not a relaxed validity rule.

Dependencies, optional dependencies, dev dependencies, peer constraints, overrides, and resolutions are included wherever the current lockfile writer represents them. Unsupported or ambiguous metadata forces full resolution.

## Preserved Jhol Identity

- JAGR Layers 0, 3, 2, and 1 remain unchanged.
- Native resolution remains authoritative after any mismatch.
- No Bun/npm algorithm, data format, or cache strategy is introduced.
- Offline mode, integrity verification, deterministic lockfile writing, and lifecycle-script policy remain unchanged.
- The optimization is Jhol's cache-first principle applied to project identity and mathematical work avoidance.

## Testing

Tests are written and observed failing before production changes.

Fingerprint tests cover:

- two projects cannot overwrite each other's valid fingerprints;
- a `package.json` change invalidates the hit;
- a lockfile change invalidates the hit;
- malformed and legacy records miss safely;
- a valid matching project returns a hit;
- absence of `node_modules` prevents a hit.

Lockfile-only tests cover:

- an unchanged complete lockfile is reused without rewriting it;
- changed root dependencies force full resolution;
- missing integrity forces full resolution;
- incompatible root versions force full resolution;
- malformed lockfiles force full resolution;
- valid fallback output remains equivalent to the current writer.

Run the focused tests, the complete `jhol-core` library suite, CLI integration tests relevant to install/lockfile-only, and formatting checks.

## Performance Acceptance

Build the same release profile and rerun the same local fixture against Bun 1.3.14. Report raw samples and medians for Jhol-before, Jhol-after, and Bun.

Acceptance rules:

- no repeatable regression greater than 3% in warm install, offline install, or another current Jhol-winning case;
- no correctness, installed-tree, lockfile, integrity, or resolver-output regression;
- no-op and lockfile-only should improve materially; otherwise remove the ineffective optimization;
- noisy cases are rerun with at least seven samples and compared by median;
- any optimization responsible for a failed guardrail is reverted independently.

## Out of Scope

- daemon-only benchmark advantages;
- changing JAGR mathematics or layer ordering;
- replacing npm-compatible lockfiles with a new format;
- broad CAS, registry, downloader, or extraction rewrites;
- weakening validation to manufacture benchmark wins.
