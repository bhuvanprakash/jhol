# Compatibility scorecard

**What this is.** A local harness that installs the same project twice — once with `npm`, once
with `jhol` — and compares **what a program can observe**: the set of `name@version` pairs in
`node_modules`, the bin links, whether lifecycle scripts ran, the file count, and how failures are
described. It produces one table, and that table is the product's honest position.

**What this is not.** It is not a compatibility shim, and it is not a copy of anything. We never
read npm's, pnpm's, yarn's or bun's source, and we never port their algorithms. Our resolver is
JAGR-∞ and our tree verifier is ours. What we take from other tools is **behaviour**, the same way
a browser takes HTML semantics from the spec: the observable contract is the test, the
implementation stays ours.

> Rule: if a fix can only be expressed by copying a competitor's internal design, the fix is wrong.
> Re-derive it from our own invariants and prove it with a scorecard row.

## Why these comparisons

| observed | why it decides trust |
|---|---|
| `name@version` set (sorted) | a tree that resolves differently is a different program; nothing else matters if this is wrong |
| bin links in `node_modules/.bin` | a missing bin is a broken `npm run`, and it fails at *run* time, not install time |
| lifecycle scripts (`preinstall`, `install`, `postinstall`) | native packages build here; wrong order or missing env breaks silently |
| file count (± tolerance) | catches "installed the right versions, half the files" |
| exit code + first error line | a failure that names nothing is worse than a wrong version |
| wall time (informational) | never a pass/fail criterion until semantics match |

## The scenarios, and the hard thing each isolates

| # | scenario | isolates |
|---|---|---|
| 1 | `express@4.18.2` | the boring baseline: one range, no peers, no scripts |
| 2 | `jest@29` | a large graph with duplicate versions of the same package at different depths |
| 3 | `esbuild@0.20.0` | optional platform packages: the wrong host package must not be required |
| 4 | `sharp@0.33` | native prebuilt binary, `os`/`cpu` gating, `install` script |
| 5 | `react@18` + `react-dom@18` + `@testing-library/react@14` | peer dependencies and their resolution |
| 6 | `chalk@5` + `execa@8` | ESM-only packages: `exports`, no `main` fallback |
| 7 | workspace monorepo (2 members, `workspace:*`) | workspace links, not registry fetches |
| 8 | `overrides` on a transitive `lodash` | a manifest field that must beat the resolver's preference |
| 9 | `typescript@5` | bin links must exist and point at the right file |
| 10 | `@babel/core@7` + `@babel/preset-env@7` | scoped packages and a deep dependency graph |

Ten is deliberately small. A row is added **when a real failure needs a home**, not to look
thorough.

## Method

1. **Isolation.** One temporary directory per run, per tool. Same `package.json` bytes, same
   registry (`https://registry.npmjs.org` unless `JHOL_REGISTRY` is set).
2. **Determinism first.** Pass 1 runs `--ignore-scripts`: this compares *resolution and layout*
   only, so a flaky native build cannot mask a resolver bug.
3. **Manifest.** For every `node_modules/**/package.json` at depth ≤ 3: `name@version`, sorted.
   Plus `node_modules/.bin` entry names, plus total file count.
4. **Diff.** First ten differing lines, always with the npm side and the jhol side on separate
   lines so the difference is readable without a tool.
5. **Timing.** `bash time` builtin, one cold and one warm run, reported but **not** gating.
6. **Cleanup.** Each scenario's directories are deleted after measurement, so a ten-scenario run
   never needs more than ~1 GB.

## Failure taxonomy

Every failure is classified, because the class decides the fix:

| class | meaning | where the fix belongs |
|---|---|---|
| `RESOLVE` | a different version was chosen | resolver (JAGR-∞), never the installer |
| `LAYOUT` | same versions, different shape (`node_modules` placement) | materialiser |
| `FILES` | same versions, missing or extra files | extraction / integrity |
| `BINS` | `.bin` links missing or wrong target | link step |
| `SCRIPTS` | lifecycle script missing, wrong order, wrong env | install pipeline |
| `GATE` | `os`/`cpu`/`libc`/`engines` handled differently | platform gating |
| `MESSAGE` | failure text names nothing actionable | error surface |
| `TOOL` | npm or jhol crashed/timed out | infrastructure, re-run |

## Scorecard format (published as-is, including failures)

```
scenario                  tree   bins  scripts  files   exit   verdict
express-4.18.2            ✓      ✓     n/a      71/71   ✓      PASS
jest-29                   ✗      —     —        —       —      RESOLVE  (+2 / -1)
```

A row may be `SKIP` only with a reason in the notes column. No row is deleted because it fails.

## How to run it (local, no CI yet)

```sh
sh scripts/scorecard.sh              # all ten scenarios
sh scripts/scorecard.sh --quick      # the first four: fast signal while developing
sh scripts/scorecard.sh --scenario 3 # one row, for a single fix
```

Requires `npm` on `PATH` (the oracle) and a release build of `jhol`. Output goes to
`docs/scorecard/YYYY-MM-DD.md`, generated, never hand-edited. Each run prints the table to stdout
and exits non-zero if any row is not `PASS` or `SKIP`.

## Platform matrix

The scorecard runs per platform, and a platform is only listed as supported when its column is
green:

| platform | scorecard | status |
|---|---|---|
| macOS arm64 | partial (see `docs/scorecard/`) | supported, early adopters |
| Linux x86_64 | never run | **not supported yet** |
| Windows x86_64 | never run | **not supported yet** |

Running it on Linux needs nothing more than Docker:

```sh
docker run --rm -v "$PWD:/w" -w /w rust:1 bash -lc \
  'cargo build --release && sh scripts/scorecard.sh'
```

No CI account, no push: the harness is the same command everywhere.

## Known limitations of the current pass

Recorded so they are not mistaken for "no problem":

1. **Layout is invisible.** The manifest is a *set* of `name@version`; two nested copies and one
   flattened copy compare equal. Depth must be added before claiming layout compatibility.
2. **Extra files pass.** The file check fails only when jhol has less than 95 % of npm's count, so
   a row can pass while shipping more files than the oracle.
3. **Timing is informational.** Cold installs differ in what they fetch; the column stays out of
   the verdict until semantics match.
4. **Scripts are not compared in pass 1.** `--ignore-scripts` is deliberate: resolution first.
   Pass 2 runs with scripts on the scenarios that build, and compares presence and order.

First results and the verified root cause of the `express` divergence:
`docs/scorecard/NOTES-2026-11-12.md`.
