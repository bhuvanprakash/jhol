# Release criteria

This is the checklist that decides whether a release is published. It is deliberately mechanical:
each line is something that can be run, not something that can be claimed.

## What a release means

| | rule |
|---|---|
| version | the CLI surface, the lockfile schema and the status contract are stable from here |
| semver | after the first stable tag: new flags may be added; existing flags, exit codes and lockfile fields may not change meaning |
| platforms | a platform is supported only when its **scorecard column is green** (`docs/COMPATIBILITY.md`) |
| artifact kinds | npm packages, Python wheels, model weights — all three are first class and all three appear in `status` |
| knobs | no new `JHOL_*` variable may be added before the release; the audit below is part of the release |

## Definition of done

### Correctness

- [ ] `sh scripts/verify-all.sh` green (installer gate, engine gate, CLI contracts, workspace tests)
- [ ] `jhol-core` tests green, ignored tests each carry their reason in the test body
- [ ] JholVM corpus node-exact (currently 55/55)
- [ ] brute-force resolver completeness test enabled and green
- [ ] an interrupted install (`kill -9` mid-install) leaves either a verifiable tree or none —
      never a half tree that `verify` calls `match`

### Compatibility (the gate that matters)

- [ ] scorecard: **all ten rows PASS or SKIP-with-reason** on macOS arm64
- [ ] scorecard run on Linux x86_64; every row PASS or SKIP-with-reason
- [ ] Windows x86_64: either green, or explicitly listed as unsupported in README with the reason
- [ ] no row removed from the scorecard during the release

### Performance (measured, with its assumptions)

- [ ] warm install ≤ 20 ms on the 71-package fixture (currently 8–19 ms)
- [ ] cold install within 15 % of `pnpm` on the same scenarios, same tree — npm-compatible semantics
- [ ] `jhol status` warm ≤ 10 ms; `--watch` idle tick ≤ 1 ms of CPU
- [ ] every performance claim in README names its scenario and its date

### Robustness

- [ ] disk full (`ENOSPC`) during install: exits non-zero, names the cause, `verify` does not lie
- [ ] concurrent installs in one project serialise on the project lock, second waits or fails clearly
- [ ] corrupt cache entry: detected by digest, repaired or reported, never used
- [ ] one unpinned flake policy: no test may be flaky by scheduling — fixes are in the harness
      (`docs/DEFECT-LOG.md`, 2026-11-11 entry)

### Surface

- [ ] `jhol --help` describes the tool truthfully; hidden blueprint commands stay hidden
- [ ] every error message names the thing that failed and the next command to run
- [ ] `status`, `verify`, `why`, `attest`, `fit` all have `--json` and a stable field set
- [ ] no screen shows `—` or `NaN` where a value is missing; unknowns are labelled as unknowns

### Documentation

- [ ] README claims match the scorecard and the platform matrix — nothing beyond it
- [ ] `docs/` has: RESOLVER-JAGR, INSTALL-PIPELINE, OPERATIONS, PRODUCTION-READINESS,
      COMPATIBILITY, ENGINE-JHOLVM, ENGINE-GAPS, GUI-ML, CLI-REFERENCE, DEFECT-LOG
- [ ] an "unproven" list exists and README does not contradict it

## Knob audit (release blocker)

`JHOL_*` is a support surface. Before 2.0.0:

1. List every knob with one line: what it changes, who needs it, what test proves it.
2. Any knob the scorecard never touches is **deleted** (not documented).
3. Any knob that switches to a strictly slower path is a preference — delete it (rule from
   `docs/OPERATIONS.md`).
4. Document the survivors in `docs/ENV-AUDIT.md` with a one-line justification each.

## Platform claim, stated honestly at publish time

- macOS arm64: supported.
- Linux: supported only if the scorecard ran and is green; otherwise "not supported yet" in the
  README's first screen, not a footnote.
- Windows: same rule.

## Release runbook

```sh
sh scripts/verify-all.sh                 # must be green
sh scripts/scorecard.sh                  # must be all PASS/SKIP
cargo build --release
./target/release/jhol --version          # must print 2.0.0
sh scripts/package-release.sh            # tarball + sha256 + size
git tag -a vX.Y.Z -m "jhol X.Y.Z"
# publishing to a registry is a separate, deliberate step
```

## The list we publish beside the release

- Linux/Windows status, verbatim from the platform matrix.
- Scorecard table, including every failing row at the time of the tag.
- Known engine gaps (BigInt, `eval`, regex lookaround/backrefs, mapped `arguments`).
- Performance numbers with their scenarios and dates.
- The number of surviving knobs.

A release that lists its own gaps is worth more than one that hides them.
