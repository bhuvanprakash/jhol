# Jhol – Fast, Offline-Friendly Package Manager

A fast, offline-friendly npm alternative with local caching and doctor. Use your existing `package.json`; Jhol caches tarballs and falls back to the registry when needed.

### Why Jhol?
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
Download `jhol-linux` or `jhol-windows.exe` from [Releases](https://github.com/bhuvanprakash/jhol/releases). Make the binary executable (Linux: `chmod +x jhol-linux`) and optionally move it to your PATH.

Optional: install the binary to your PATH (e.g. `/usr/local/bin`) so you can run `jhol` from anywhere:

```sh
jhol global-install
```

## Usage

```sh
jhol install <package> [packages...]   # Install packages (uses cache when available)
jhol install --no-cache <package>      # Ignore cache and fetch from registry
jhol doctor                             # Check for outdated dependencies
jhol doctor --fix                      # Update outdated dependencies
jhol cache list                        # List cached packages
jhol cache clean                       # Remove cached tarballs
jhol global-install                    # Install jhol binary to PATH
```

## Configuration

- **Cache directory:** Set `JHOL_CACHE_DIR` to override the default (`~/.jhol-cache` on Unix, `%USERPROFILE%\.jhol-cache` on Windows).
- **Quieter output:** Set `JHOL_LOG=quiet` or use `-q` / `--quiet` with `install` and `doctor`.

## License

This software is licensed under the JHOL FREE LICENSE (Proprietary, Non-Commercial): free for personal and educational use; not for commercial use, redistribution, or modification.

For licensing inquiries: bhuvanstark6@gmail.com
