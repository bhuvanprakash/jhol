# Getting started with Jhol

Jhol is a package manager that’s built for speed and works great offline. It uses your existing `package.json` and lockfile, and it prefers **Bun** when you have it installed—otherwise it falls back to npm. Here’s how to get going.

---

## Install Jhol

**Linux & macOS**

```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

That pulls the repo and builds it. Once it’s done, you can run `jhol` from anywhere if it’s on your PATH. If not:

```sh
jhol global-install
```

**Windows**

1. Install Rust from [rustup.rs](https://rustup.rs) if you don’t have it.
2. Clone and build:
   ```powershell
   git clone https://github.com/bhuvanprakash/jhol.git
   cd jhol
   cargo build --release
   ```
3. Put the binary somewhere in your PATH (e.g. `C:\Program Files\Jhol\`) and add that folder to your system PATH.
4. Or run `install_jhol.bat` as Administrator—it’s in the repo.

Check that it worked:

```sh
jhol --version
```

---

## Using Jhol

Think of it like npm or yarn, but cache-first and with a few extras.

### Install packages

```sh
jhol install axios
```

Jhol will look in its cache first. If it finds the package there, it installs from there. If not, it fetches from the registry (via Bun or npm) and then caches it for next time.

Want to install everything from your `package.json`? Just run:

```sh
jhol install
```

No package names needed—it reads your dependencies and lockfile and installs from that.

You can pin versions too:

```sh
jhol install react@18.0.0 lodash@4.17.21
```

### Check and fix outdated dependencies

```sh
jhol doctor
```

That lists what’s outdated. To actually update them:

```sh
jhol doctor --fix
```

### Security audit

```sh
jhol audit
```

Shows known vulnerabilities. To try to fix them automatically:

```sh
jhol audit --fix
```

### Generate an SBOM

If you need a software bill of materials (e.g. for compliance or tooling):

```sh
jhol sbom
```

Prints CycloneDX-style JSON. To write it to a file:

```sh
jhol sbom -o sbom.json
```

---

## How Jhol behaves

1. **Cache first** – If the package (and version) is already in the cache, it installs from there. No network.
2. **Then the registry** – If it’s not cached, it uses Bun (or npm) to fetch and install, then stores it in the cache.
3. **Offline** – With `--offline` (or `JHOL_OFFLINE=1`), it *only* uses the cache. If something’s missing, it fails and tells you what’s missing. Handy for air-gapped or flaky networks.
4. **Lockfile** – It respects `package-lock.json` and `bun.lock`. Use `--frozen` if you want it to refuse to run when the lockfile is out of sync with `package.json`.

---

## Cache and bundles

**See what’s cached**

```sh
jhol cache list
jhol cache size
```

**Prune old stuff**

```sh
jhol cache prune
```

Keep only the 50 most recently used tarballs:

```sh
jhol cache prune --keep 50
```

**Export deps for another machine (e.g. offline)**

From a project that already has its deps installed (or at least resolved):

```sh
jhol cache export ./jhol-bundle
```

That copies everything your project needs into `./jhol-bundle` (plus a small manifest). On the other machine:

```sh
jhol cache import ./jhol-bundle
```

Then you can run `jhol install --offline` there.

**Nuke the cache**

```sh
jhol cache clean
```

---

## Workspaces

If your repo uses npm/Bun workspaces (the `workspaces` field in the root `package.json`), you can run install, doctor, or audit in all of them at once:

```sh
jhol install --all-workspaces
jhol doctor --fix --all-workspaces
jhol audit --all-workspaces
```

Jhol finds the workspace roots and runs the command in each. One command, whole monorepo.

---

## Config and env

- **Cache location:** Set `JHOL_CACHE_DIR` if you don’t want the default (`~/.jhol-cache` or `%USERPROFILE%\.jhol-cache`).
- **Quieter output:** `JHOL_LOG=quiet` or use `-q` / `--quiet` on the command.
- **Optional config file:** Put a `.jholrc` (JSON) in your project root or home dir. You can set things like `"backend": "bun"`, `"cacheDir": "/path/to/cache"`, `"offline": false`, `"frozen": false`. CLI flags still override this.

---

## Where to go from here

- **README.md** – Full command list and options.
- **Documentation/main.md** – Longer guide with troubleshooting and FAQs.
- **GitHub** – [github.com/bhuvanprakash/jhol](https://github.com/bhuvanprakash/jhol) for issues and contributions.

Once you’re set up, `jhol install` and `jhol doctor --fix` will get you most of the way. The rest is there when you need it.
