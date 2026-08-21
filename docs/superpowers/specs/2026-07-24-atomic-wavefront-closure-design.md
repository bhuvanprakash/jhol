# Atomic Wavefront Closure Design

## Objective

Make Jhol's first install and cache-backed reinstall as fast as possible without
embedded package bytes, daemon dependence, incomplete dependency trees, partial
package extraction, weaker integrity checks, or altered semver behavior.

## Measured Baseline

The July 24 PRW baseline reported 391 ms cold and 45 ms warm for the brutal
`simple` fixture. Those numbers are not valid performance evidence:

- `package-lock.json` contains only 11 direct packages and no transitive graph.
- Dynamic imports fail for express, chalk, axios, react-dom, dayjs, and
  typescript.
- The benchmark considers the install successful when any one of
  express/react/lodash exists and suppresses command failures.
- Runtime still checks `binary-packages/` through `load_binary_tarball` even
  though embedded tarballs were removed.
- A full axios tarball produced a reusable unpacked directory containing only
  `package.json`, showing that incomplete stores can be accepted as complete.

Performance work must therefore begin with a complete, executable dependency
closure and atomic store invariants.

## Architecture

### 1. Fixed-Point Wavefront Resolver

Replace the current multi-pass expansion with a wavefront loop:

1. Seed constraints and required/optional status from direct dependencies.
2. Select all package names whose current constraints have not been resolved.
3. Resolve local lattice/metadata hits without network.
4. Fetch missing packuments concurrently with the existing bounded HTTP path.
5. Resolve each package deterministically against all known constraints.
6. Read required and platform-compatible optional dependencies from the chosen
   versions and merge their constraints into the next wave.
7. Repeat until no package selection or constraint changes.

The output is the complete exact `name@version` closure. A package may be
downloaded only after its current constraint set selects a version. If a later
constraint changes that selection, the stale result is discarded before
materialization.

The first implementation keeps resolution waves and download/extraction as two
bounded parallel phases. Streaming overlap is deferred until the fixed-point
resolver is proven correct.

### 2. No Binary Package Runtime Path

Remove runtime reads from `binary-packages/`:

- no `load_binary_tarball` preference in download functions;
- no binary version or dependency index resolution;
- no embedded tarball or bundled decode cache;
- retain only the small resolution seed when its version satisfies every known
  constraint.

All package content comes from the registry or the persistent store/CAS.

### 3. Atomic Complete Store

`store_unpacked/<shard>/<hash>` must never represent partial extraction.

For each new tarball:

1. Verify expected SRI before trusting content.
2. Decompress and extract into a unique sibling temporary directory.
3. Reject unsafe paths and unsupported entries using existing tar extraction
   protections.
4. Verify `package.json` exists and its name/version match the resolved package.
5. Write a completion marker containing the content hash and package identity.
6. Atomically rename the temporary directory into the final sharded path.

Existing directories without a valid completion marker are rebuilt from the
tarball. Failed extraction removes its temporary directory.

### 4. Cold Materialization

After resolution, package downloads run through the existing worker pool at the
bounded download concurrency. Each worker verifies, atomically unpacks, and
records the store index entry. Once all required packages are ready, Jhol links
the complete flattened graph into `node_modules` and writes the complete
lockfile. Required-package failures fail the install; optional unsupported
packages may be omitted according to existing platform rules.

### 5. Warm Materialization

Warm install reads the complete lockfile/capsule graph and store index once.
Every package is projected with a whole-directory link when possible. There are
no packument requests, tarball reads, decompressions, or lifecycle scripts when
scripts are disabled. Missing or invalid store entries fail frozen/offline mode
and trigger the normal registry path otherwise.

## Correctness Invariants

- Every required dependency reachable from every installed package is present.
- Every selected version satisfies all accumulated constraints.
- Platform filtering preserves existing required/optional semantics.
- Registry SRI verification remains mandatory when integrity is provided.
- Package contents are complete by default; no selective removal of runtime
  files. Explicit minimal extraction remains opt-in only.
- A successful lockfile describes the same complete graph materialized in
  `node_modules`.
- Required failures never become successful benchmark results.

## Verification

### Automated Regression Tests

Write failing tests before production changes:

- express installs and can be imported with all transitives;
- axios's runtime entry points exist after cold and warm materialization;
- react-dom can import scheduler;
- the lockfile contains transitive packages and dependency edges;
- incomplete `store_unpacked` directories are rebuilt atomically;
- runtime install does not read `binary-packages/`;
- seeded versions are used only when they satisfy the full constraint set.

### Benchmark Evidence

Repair `brutal_v2.sh` before trusting its output:

- fix the invalid `binary` declaration;
- do not suppress install failures;
- require all direct packages and representative transitive imports;
- verify lockfile and node_modules closure counts;
- report medians only for successful runs.

Run release benchmarks with fresh cache for cold and new project/same cache for
warm. Report wall time, success count, resolved package count, materialized file
count following links, binary size, and any remaining network variance.

## Expected Outcome

The first correct implementation may benchmark slower than the invalid 391 ms
baseline because it performs the missing work. The optimization target is the
fastest complete install, not the smallest timer. Once correctness is restored,
profiling determines whether metadata waves, download latency, atomic extraction,
or warm linking is the next dominant cost.
