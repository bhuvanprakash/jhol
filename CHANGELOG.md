# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Changed
- Improved install/prefetch performance with registry packument ETag caching and parallel prefetch/download flow.
- Added stronger cache safety checks when reading packuments (fallback on empty/invalid cache bodies).
- Added stricter backend fallback controls: `--no-scripts`/`--scripts` behavior, script allowlist support via `JHOL_SCRIPT_ALLOWLIST`, and safer defaults.
- Expanded native lockfile fidelity with peer dependency metadata fields.
- Added `jhol ci` command for strict lockfile installs and `jhol audit --gate` for CI vulnerability gating.
- Standardized JSON success payloads for `install`, `doctor`, `audit`, and `ci` commands.