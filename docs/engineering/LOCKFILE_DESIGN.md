# Lockfile Format Design

This document describes the lockfile format and design choices used by Jhol, and the guarantee that “install from this lockfile produces the same tree and resolvable modules.”

## Supported formats

- **.jhol.lock**: Native Jhol lock format. Stores the canonical instance-aware graph and is preferred by native install/reuse paths.
- **package-lock.json** (npm lockfile v2/v3): Primary format. Uses a `packages` object with keys for the root and each dependency (e.g. `""` for root, `node_modules/<pkg>` or plain package keys).
- **bun.lock**: Read for compatibility; Jhol can install from it and convert to `package-lock.json` via `import-lock`.

## What is stored

- **Version**: Exact resolved version (e.g. `1.2.3`).
- **Resolved**: Tarball URL (e.g. `https://registry.npmjs.org/.../-/....tgz`).
- **Integrity**: Optional SRI hash for verification.
- **Dependencies**: Map of dependency name to version for that package.
- **Instance placement**: For `.jhol.lock`, the canonical install slot/path for each resolved package instance.
- **Optional / dev**: Metadata for optional and dev dependencies where the format supports it.

Only *intrinsic* data (name, version, source, checksum, dependency set) are stored so that diffs stay meaningful and merge-friendly (see “Lockfile Format Design and Tradeoffs”, Nesbitt 2026).

## Same-tree guarantee

“Install from this lockfile” means:

- The same set of packages at the same resolved versions is installed.
- `node_modules` layout and `.bin` links are produced so that `require()` and `import` resolve to the same files as when the lockfile was generated.
- When integrity is present, tarballs are verified; mismatch causes install to fail.

“Same” is defined as: same logical package names and versions; same `resolved` URLs when used; same integrity when present. Optional and platform-specific dependencies may differ by environment; the guarantee holds for the current platform and optionality policy.

## Edge cases

- **Optional dependencies**: May be omitted from the tree on some installs; the lockfile can still list them.
- **Platform-specific**: Optional deps and `os`/`cpu` filters can change what is installed; the lockfile records what was chosen for the generating environment.
- **Bun lockfile**: No integrity in `bun.lock`; integrity is filled when converting to or writing `package-lock.json`.
- **Jhol lockfile**: `.jhol.lock` is native and instance-aware; `package-lock.json` remains a compatibility projection.

## References

- Lockfile Format Design and Tradeoffs (Nesbitt, 2026).
- PEP 751 (pylock.toml) for comparison with Python’s lockfile standard.
- npm package-lock.json documentation.
