# Do NOT push these online — and do NOT add them to `.gitignore`

**Purpose:** This file is the **only** record for paths and artifacts that must stay off GitHub / public remotes.  
**Rule:** Do **not** hide these behind `.gitignore` entries for this list — keep this manifest visible so every push is checked against it.

Last updated: 2026-09-18 (after root reorg + snapshot backups).

---

## Before every `git push`

1. Run `git status` and scan **untracked** + **staged** paths against the tables below.
2. Run `git diff --cached --name-only` — no secrets, no local home paths, no tool DBs.
3. If anything below appears staged, **unstage it** (`git reset HEAD -- <path>`) and leave it local.
4. Snapshots live **outside** the repo — they are never pushed.

Quick scan:

```sh
git status --short
git diff --cached --name-only | rg -i 'reasonix|graft/\.cache|graphify-out|\.code-review-graph|\.codebase-memory|benchmarks/(results|profiles)|\.jhol|\.kilo/|\.npmrc|\.jholrc|token|secret|password' || true
```

---

## 1. Secrets and credentials (never commit, never push)

| Item | Typical location | Why |
|------|------------------|-----|
| Registry auth tokens | `~/.jholrc`, `~/.npmrc`, project `.npmrc`, `.jholrc` | `_authToken`, `NODE_AUTH_TOKEN`, `NPM_TOKEN`, `${ENV_VAR}` expansions |
| Cache signing key | env `JHOL_CACHE_SIGNING_KEY` | Signs cache exports |
| Enterprise SSO / proxy passwords | env `JHOL_*`, `.jholrc` `authTokens` | Private registry access |
| CA / client certs with private keys | `SSL_CERT_FILE`, `JHOL_CA_CERT_PATH` | TLS identity |
| Any `.env` with real values | `.env`, `.env.local` | API keys, tokens |
| Generated cache export signatures | `cache export` output dirs you signed locally | Tamper-evident blobs tied to your key |

**If a token ever landed in a committed file:** rotate the token, then rewrite history or purge before pushing — do not push and hope.

---

## 2. Machine-local identity (paths leak your username & layout)

| Path / file | Notes |
|-------------|--------|
| `reasonix.toml` | Contains **absolute paths** under `/Users/bhuvanprakash/...` and long personal bash allowlists. **Do not push.** |
| `benchmarks/profiles/*.txt` | Install profiler output; may embed cwd and cache paths. |
| `benchmarks/results/json/*.json` | One-off bench runs; stderr tails, timings, local experiment labels. |
| `graft/.cache/session/*.json` | Graft session state + `telemetry-repo-id.json`. |
| `graphify-out/` | Generated graph DB + detect JSON with **full absolute file paths**. |
| `.code-review-graph/graph.db` | ~46 MB local knowledge graph. |
| `.codebase-memory/graph.db.zst` | Compressed local memory graph. |
| `docs/archive/REORG_MANIFEST.json` | Fine to push (only relative moves) — listed here only so you do not confuse it with bench JSON. |

---

## 3. Tooling / agent state inside the repo (stay local)

| Path | Notes |
|------|--------|
| `.jhol/` | Per-project jhol metadata. |
| `.jhol.capsule` | Zero-parse install capsule (project-specific). |
| `.kilo/` | Kilo worktrees and agent metadata (includes full repo copies under `worktrees/`). |
| `.worktrees/` | Git worktrees with uncommitted experiment branches. |
| `.opencode/` | Local OpenCode plans and session state. |
| `.reasonix/` | Reasonix autoresearch state. |
| `.commandcode/` | Local command-code cache. |
| `.slim/deepwork/` | Deepwork session notes. |
| `graft/.cache/` | Graft indexer cache (not the `graft/` index nodes themselves — review before push). |
| `target/` | Rust build output (large; rebuild anywhere). |
| `node_modules/` | If created at repo root during smoke tests. |
| Root `package.json` / `package-lock.json` | Local npm smoke only (not fixture `tests/fixtures/**`). |

---

## 4. Outside the repository (never in git — backups)

| Path | Notes |
|------|--------|
| `~/.jhol-tree-backups/` | `scripts/snapshot_tree.sh` snapshots (source + docs). |
| `~/.jhol-tree-backups/*-root-artifacts/` | Copy of root bench/docs before reorg (144 files, 2026-09-17). |
| `~/.jhol-cache/` | Default jhol package cache (tarballs, store, packuments). |
| `~/.cargo/registry/` | Downloaded crates (not this repo). |
| `~/Library/Caches/` npm/bun/pnpm stores | Harvest sources for E65; huge and machine-specific. |
| Cursor agent stores | `~/Library/Application Support/Cursor/AgentStores/...` |
| Agent transcripts | `~/.cursor/projects/.../agent-transcripts/` |

Restore tree snapshot:

```sh
./scripts/snapshot_tree.sh --restore ~/.jhol-tree-backups/20260917T224536Z
```

---

## 5. Large or binary blobs (do not push unless intentionally releasing)

| Path | Notes |
|------|--------|
| `binary-packages/` | Pre-packed tarballs + `.unpacked/` (gitignored today; still **do not push** if ignore is removed). |
| `benchmarks/macos-arm64/*.json` | Dated machine benchmarks — OK in repo only if you **want** them public; otherwise keep local. |
| `.code-review-graph/graph.db` | See §2. |
| `.codebase-memory/graph.db.zst` | See §2. |

---

## 6. Session / research logs (policy choice — default: do not push)

These are **not secrets**, but they are huge, noisy, and embed session history. Default: **keep local** unless you deliberately publish them.

| File | Size (approx.) | Notes |
|------|----------------|--------|
| `AGENTS.md` | ~290 KB | Agent work log; references internal paths and session detail. |
| `INNOVATION_LOG.md` | ~350 KB | Adopted/rejected experiment ledger. |
| `CHANGELOG.md` | Large | User-facing; usually **OK to push** after editing — unlike the two above. |

If you push code without these, trim `AGENTS.md` / `INNOVATION_LOG.md` from the commit or maintain a public branch without them.

---

## 7. What `.gitignore` already covers (do not duplicate here)

The following are **already** in `.gitignore`. **Do not add duplicate lines** for the items in §1–§4 above — this file is the record instead.

- `target/`, `Cargo.lock`, `binary-packages/`, `.worktrees/`, `.jhol/`, `.jhol.capsule`
- Root bench JSON/markdown patterns (`/benchmark-*.json`, etc.)
- `node_modules/`, root `package.json` / `package-lock.json`

**Important:** Items listed in §2–§4 that are **not** in `.gitignore` (e.g. `reasonix.toml`, `graft/.cache/`, `graphify-out/`, `benchmarks/results/`) rely on **you** reading this file before push — that is intentional per project policy.

---

## 8. Safe to push (reference)

| Path | Role |
|------|------|
| `src/`, `crates/`, `tests/`, `scripts/` | Product source |
| `docs/user/`, `docs/engineering/`, `docs/plans/` | Documentation (after reorg) |
| `docs/archive/2025-optimization/` | Historical reports (public OK if you want them) |
| `README.md`, `GET_STARTED.md`, `SECURITY.md`, `CONTRIBUTING.md` | User docs |
| `install.sh`, `install_jhol.bat` | Installers |
| `benchmarks/*.rs`, `scripts/bench/` | Bench **code** (not `benchmarks/results/` JSON) |
| `Documentation/README.md` | Redirect stub after doc merge |

---

## 9. Recovery / no data loss

- Full tree snapshot: `~/.jhol-tree-backups/20260917T224536Z` (1,249 files).
- Root artifact backup: `~/.jhol-tree-backups/20260917T224536Z-root-artifacts` (144 files).
- Move map after reorg: `docs/archive/REORG_MANIFEST.json` (141 moves; all destinations verified).

---

## 10. Owner rules (reminder)

- **No `git commit` / `git push`** unless the owner explicitly allows it in that session (`AGENTS.md` hard rule).
- This file **must stay in the repo** and **must not** be added to `.gitignore`.
- When in doubt: **do not push** — ask or keep local.
