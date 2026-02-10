# Jhol

**Fast, offline-friendly package manager** with local caching and npm fallback. Use your existing `package.json`; Jhol caches tarballs and falls back to the registry when needed.

---

## Why Jhol?

- **Fast installs** – Caches package tarballs so repeat installs skip the registry when possible.
- **Offline-friendly** – Install previously cached packages without a network.
- **npm-compatible** – Uses npm under the hood; works with existing `package.json` and lockfiles.
- **Doctor** – `jhol doctor` and `jhol doctor --fix` to check and update outdated dependencies.

## Installation

**From crates.io (recommended):**
```sh
cargo install jhol
```

**From source:**
```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

**Prebuilt binaries (Linux & Windows):**  
Download `jhol-linux` or `jhol-windows.exe` from [Releases](https://github.com/bhuvanprakash/jhol/releases).  
- Linux: `chmod +x jhol-linux` then move to your PATH if you like.  
- Windows: run `jhol-windows.exe` or add it to your PATH.

**Add to PATH (any install method):**
```sh
jhol global-install
```

## Quick start

```sh
jhol install lodash          # Install one package (uses cache when available)
jhol install react react-dom # Install multiple
jhol doctor                  # Check outdated deps
jhol doctor --fix            # Update outdated deps
```

## Commands

| Command | Description |
|--------|-------------|
| `jhol install <pkg> [pkgs...]` | Install packages; uses cache when available |
| `jhol install --no-cache <pkg>` | Ignore cache and fetch from registry |
| `jhol doctor` | List outdated dependencies |
| `jhol doctor --fix` | Update outdated dependencies |
| `jhol cache list` | List cached packages |
| `jhol cache clean` | Remove cached tarballs |
| `jhol global-install` | Install jhol binary to PATH |

Use `-q` / `--quiet` for less output.

## Configuration

| Env / flag | Effect |
|------------|--------|
| `JHOL_CACHE_DIR` | Override cache directory (default: `~/.jhol-cache` on Unix, `%USERPROFILE%\.jhol-cache` on Windows) |
| `JHOL_LOG=quiet` or `-q` | Quieter logging |

## Links

- **Crate:** [crates.io/crates/jhol](https://crates.io/crates/jhol)
- **Releases:** [GitHub Releases](https://github.com/bhuvanprakash/jhol/releases)

## License

Jhol is licensed under the [Jhol License](LICENSE) (personal and non-commercial use). For other use, contact bhuvanstark6@gmail.com.
