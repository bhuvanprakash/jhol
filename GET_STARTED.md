# Getting Started with Jhol

Jhol is a cache-first package manager for JavaScript projects. It works with your existing `package.json` and lockfiles, and supports native install, doctor, and audit workflows.

---

## 1) Install Jhol

### Linux / macOS

```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

### Windows

1. Install Rust from [rustup.rs](https://rustup.rs).
2. Clone and build:

```powershell
git clone https://github.com/bhuvanprakash/jhol.git
cd jhol
cargo build --release
```

3. Add the built binary to PATH, or run `install_jhol.bat` as Administrator.

Verify installation:

```sh
jhol --version
```

---

## 2) Install dependencies

Install one package:

```sh
jhol install axios
```

Install from your project `package.json`:

```sh
jhol install
```

Use exact versions when needed:

```sh
jhol install react@18.0.0 lodash@4.17.21
```

---

## 3) Keep dependencies healthy

Check outdated packages:

```sh
jhol doctor
```

Update outdated packages:

```sh
jhol doctor --fix
```

Audit vulnerabilities:

```sh
jhol audit
jhol audit --fix
```

Generate SBOM:

```sh
jhol sbom -o sbom.json
```

---

## 4) Work offline

Install using cache only:

```sh
jhol install --offline
```

You can also set:

```sh
JHOL_OFFLINE=1
```

For CI or reproducible builds:

```sh
jhol install --frozen
# or
jhol ci
```

---

## 5) Manage cache

```sh
jhol cache list
jhol cache size
jhol cache prune --keep 50
```

Export/import cache bundles for air-gapped or offline environments:

```sh
jhol cache export ./jhol-bundle
jhol cache import ./jhol-bundle
```

---

## 6) Workspaces

Run commands across all workspaces in monorepos:

```sh
jhol install --all-workspaces
jhol doctor --fix --all-workspaces
jhol audit --all-workspaces
```

---

## 7) Configuration quick reference

- `JHOL_CACHE_DIR`: override cache location
- `JHOL_LOG=quiet`: reduce log output
- `JHOL_OFFLINE=1`: force offline mode
- `.jholrc` (JSON): set defaults (`backend`, `cacheDir`, `offline`, `frozen`)

---

## Next steps

- Read [README.md](./README.md) for full command coverage.
- Read [Documentation/main.md](./Documentation/main.md) for deeper guidance.
