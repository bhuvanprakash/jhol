# JHOL - Project Context

## Project Overview

**JHOL** is a **Rust-powered, cache-first package manager** for JavaScript/Node.js projects. It provides native install, doctor, and audit capabilities without requiring npm/Bun at runtime, with offline support and deterministic CI workflows.

### Key Features
- 🦀 **Native Rust Implementation** - No Node.js/Bun runtime required for core operations
- 📦 **Cache-First Architecture** - Global caches for resolution, tarballs, and extracted packages
- 🛜 **Offline-Ready** - Install from cache with `--offline` flag
- 🔒 **Deterministic CI** - Reproducible installs with `--frozen` or `jhol ci`
- ⚡ **Performance Optimized** - 40-60% faster cache operations, 90x faster resolution with caching
- 🧩 **Fallback Support** - Delegate to Bun/npm for compatibility edge cases

### Architecture
- **Workspace Structure**: Main binary (`src/main.rs`) + core library (`crates/jhol-core/`)
- **Core Components**:
  - `install.rs` - Package installation logic with parallel hardlinking
  - `registry.rs` - npm registry client with DashMap caching
  - `global_*.rs` - Global cache stores (resolution, tarball, extracted)
  - `lockfile.rs` - Lockfile parsing and generation
  - `doctor.rs` / `audit.rs` - Dependency health and security checks

### Latest Performance (March 2026)
| Metric | JHOL | Bun | Status |
|--------|------|-----|--------|
| Cold Install | 0.048s | 0.022s | Within 2.2x |
| Warm Install | **0.014s** | 0.020s | **1.4x FASTER** |
| Offline Install | **0.014s** | N/A | **Only JHOL supports** |

### Key Optimizations Implemented
1. **DashMap** - Lock-free concurrent metadata cache (registry.rs)
2. **xxHash** - 15x faster internal hashing (11 files)
3. **Global Caches** - 90x faster resolution, zero network for cached packages
4. **Parallel Hardlinking** - 3x faster for 5+ packages
5. **Connection Pooling** - Reduced TCP overhead
6. **DNS Caching** - Faster repeated requests

---

## Building and Running

### Prerequisites
- Rust stable (edition 2021)
- Python 3 (for benchmarking scripts)

### Build Commands

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Check without building
cargo check
```

### Running JHOL

```bash
# Install packages
./target/release/jhol install lodash
./target/release/jhol install react react-dom
./target/release/jhol install  # From package.json

# Maintenance
./target/release/jhol doctor
./target/release/jhol doctor --fix
./target/release/jhol audit
./target/release/jhol audit --fix

# Cache management
./target/release/jhol cache list
./target/release/jhol cache prune
./target/release/jhol cache clean

# Offline mode
./target/release/jhol install --offline
```

### Benchmarking

```bash
# Scientific benchmark suite (recommended)
python3 scripts/benchmark_scientific_v2.py \
  --suite small \
  --repeats 5 \
  --jhol-bin target/release/jhol \
  --json-out results.json \
  --markdown-out results.md

# Offline benchmark (no network variance)
python3 scripts/benchmark_offline.py \
  --suite small \
  --repeats 5 \
  --jhol-bin target/release/jhol
```

### Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `JHOL_CACHE_DIR` | Override cache directory | `~/.cache/jhol` |
| `JHOL_OFFLINE` | Force offline mode | `0` |
| `JHOL_QUIET` | Minimal output | `0` |
| `JHOL_PROFILE_INSTALL` | Enable timing logs | `0` |
| `JHOL_NETWORK_CONCURRENCY` | Network concurrency limit | Auto (CPU * 2) |
| `JHOL_MIN_RELEASE_AGE_SECONDS` | Ignore new package versions | None |

---

## Development Conventions

### Code Style
- **Rust Edition**: 2021
- **Formatting**: Standard Rust formatting (`cargo fmt`)
- **Error Handling**: `Result<T, String>` or custom `JholError` enum
- **Logging**: `utils::log()` for user messages, `eprintln!()` for timing logs

### Testing Practices
- **Unit Tests**: In-module `#[cfg(test)]` modules
- **Integration Tests**: `tests/` directory with fixtures
- **Benchmark Tests**: Python scripts in `scripts/` directory
- **Test Command**: `cargo test` (runs unit + integration tests)

### Optimization Guidelines
1. **Profile First** - Use `JHOL_PROFILE_INSTALL=1` to identify bottlenecks
2. **Global Caches** - Prefer global caches over per-project caching
3. **Lock-Free Data Structures** - Use `DashMap` for concurrent HashMaps
4. **xxHash for Internal** - Use `xxh3_64` for non-cryptographic hashing
5. **Keep SHA256 for Integrity** - npm registry requires SHA256/384/512 for SRI

### Key Architecture Decisions

#### Why DashMap Over Mutex/RwLock?
- **Workload**: Concurrent HashMap with reads AND writes
- **DashMap**: Lock-free reads, sharded writes, no global lock
- **Mutex**: Blocks ALL threads (too restrictive)
- **RwLock**: Atomic overhead, 34% slower for write-heavy workloads

#### Why xxHash for Internal Hashing?
- **15x faster** than SHA256 for cache keys
- **Perfect for**: Internal deduplication, cache keys, signatures
- **Keep SHA256 for**: npm integrity verification (registry requirement)

#### Global Cache Architecture
```
~/.cache/jhol-resolution-cache/  # Resolved dependency graphs
~/.cache/jhol-tarball-store/     # Downloaded tarballs (content-addressable)
~/.cache/jhol-extracted-store/   # Pre-extracted packages (ready to hardlink)
```

### Contribution Guidelines
1. **Fork and Branch**: Create feature branch from main
2. **Make Changes**: Follow existing code style
3. **Test**: Run `cargo test` and benchmark scripts
4. **Commit**: Descriptive commit messages
5. **Pull Request**: Clear description of changes

### Project Structure
```
jhol/
├── Cargo.toml              # Workspace + main binary config
├── src/main.rs             # CLI entrypoint
├── crates/jhol-core/       # Core library
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Library exports
│       ├── install.rs      # Installation logic
│       ├── registry.rs     # npm registry client (DashMap)
│       ├── global_*.rs     # Global cache stores
│       ├── lockfile.rs     # Lockfile handling
│       └── ...
├── scripts/
│   ├── benchmark_scientific_v2.py  # Main benchmark suite
│   └── benchmark_offline.py        # Offline benchmark
├── tests/                  # Integration tests
├── test-project/           # Test fixtures
└── Documentation/          # User documentation
```

### Recent Changes (March 2026)
- **DashMap Integration**: Lock-free concurrent metadata cache in `registry.rs`
- **xxHash Optimization**: 11 files updated for 15x faster internal hashing
- **Parallel Hardlinking**: New `parallel_hardlink.rs` module
- **Global Caches**: Resolution, tarball, and extracted package stores
- **Scientific Benchmarking**: Proper cache clearing, statistical analysis

### Known Limitations
- **Cold Install Gap**: Still 2.2x slower than Bun (0.048s vs 0.022s)
- **Write-Heavy Caches**: Mutex preferred over RwLock for write-heavy workloads
- **Network Dependency**: Cold installs still require network (global caches help)

### Performance Targets
- **Cold Install**: < 0.030s (currently 0.048s)
- **Warm Install**: < 0.015s (currently 0.014s ✅)
- **Variance**: < 5% (currently < 10%)

---

## Quick Reference

### Common Commands
```bash
# Build
cargo build --release

# Test
cargo test

# Install package
jhol install <package>

# Install from package.json
jhol install

# Check dependencies
jhol doctor

# Audit security
jhol audit

# Cache management
jhol cache prune
jhol cache clean

# Offline install
jhol install --offline
```

### Key Files to Know
- `src/main.rs` - CLI entrypoint
- `crates/jhol-core/src/lib.rs` - Core library exports
- `crates/jhol-core/src/install.rs` - Installation logic
- `crates/jhol-core/src/registry.rs` - Registry client with DashMap
- `scripts/benchmark_scientific_v2.py` - Benchmark suite

### Debugging Tips
- Enable timing logs: `JHOL_PROFILE_INSTALL=1`
- Check cache: `ls -la ~/.cache/jhol-*`
- Clear cache: `jhol cache clean`
- Verbose output: Remove `-q` flag from commands
