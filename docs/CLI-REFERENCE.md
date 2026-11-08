# CLI reference

Exit codes: **0** success · **1** failure (including "tree does not match", policy
refusal, dated install that cannot be honoured) · **127** broken install layout
(the thin front door could not find the full CLI).

---

## Install family

### `jhol install [PACKAGES...]`

| flag | effect |
|---|---|
| `--as-of <DATE>` | resolve as of a date (`YYYY-MM-DD` or RFC3339); needs full metadata |
| `--allow-license <LIST>` | only versions under these licenses (comma separated) |
| `--license-allow-unknown` | with the above, also allow versions declaring no license |
| `--exclude-vuln` | do not resolve releases with known advisories (queries OSV, bans them, re-resolves) |
| `--omit=dev,optional,peer` | leave those classes out (also `--prod`, `--include-dev`) |
| `--save`, `--save-dev/-D`, `--save-peer`, `--save-optional/-O`, `--save-exact/-E` | persist the request |
| `--filter/-F <PATTERN>` | workspace selection |
| `--frozen-lockfile` | fail if the lockfile would change |
| `--engine-strict` | fail on `engines.node` mismatch instead of warning |
| `--offline` | cache only |
| `--no-scripts`, `--native-only` | skip lifecycle scripts / native builds |
| `-q/--quiet` | summary only |

A plain `jhol install` with nothing to do prints `Up to date (N packages)` and
exits; an install that fixed something prints `Repaired (N packages)`.

### `jhol ci`
Clean, deterministic install: wipes the tree, requires a lockfile, then installs
exactly it. Reports pending script approvals.

### `jhol add` / `jhol remove` / `jhol update` / `jhol dedupe`
Mutate the manifest and re-resolve. `dedupe` reports packages installed at more than
one version (`--json`); it does not silently collapse them — collapsing duplicates
is a resolution decision.

---

## Verification and explanation

### `jhol verify [--json] [--prune] [-q]`
Exit **0** only when `node_modules` is exactly what the lockfile authorises.
Anything else names the difference and exits 1. `--prune` removes unauthorised
directories then re-verifies. A missing lockfile prints `not verified` and exits 0
(there is nothing to disagree with).

### `jhol why <PACKAGE>`
Dependency chains, then the lockfile derivation: every installed copy, its version,
its path, and the range each dependent demanded. Offline. `--jagr` switches to the
deep resolver view.

### `jhol why-not <PACKAGE@RANGE>`
Why a range cannot be used, and what would allow it. Three outcomes kept distinct:
already installed (answered with its path), published-but-forbidden (**minimal
unsat core**), nothing published in range. Offline, from cached metadata.

```
$ jhol why-not chalk@^3
the only versions matching it are: 3.0.0
and these requirements forbid every one of them:
  jest-watcher@29.7.0 needs chalk ^4.0.0
```

---

## Cache

| command | effect |
|---|---|
| `jhol cache list` | cached packages |
| `jhol cache size` | bytes and count |
| `jhol cache gc` | reachability sweep over every store root (`--dry-run`) |
| `jhol cache clean` | remove cached tarballs; reports each failure with its reason and exits 1 on partial failure; works even when the cache cannot be initialised |
| `jhol cache export <dir>` / `import` | move a cache between machines |
| `jhol prefetch` | fetch a lockfile's packages ahead of time |

---

## Registry

`jhol view <pkg>`, `jhol search <term>`, `jhol audit`, `jhol outdated`,
`jhol doctor`, `jhol pack`, `jhol publish`, `jhol login`, `jhol import-lock`.

`pack` is deterministic (sorted entries, epoch mtimes, hand-built gzip).
`publish`/`login` speak the CouchDB packument protocol with bearer auth; tokens are
written to `.npmrc` with mode 0600.

---

## Packages and scripts

| command | effect |
|---|---|
| `jhol approve-scripts`, `revoke-scripts` | manage Script Receipts (consent bound to name@version + script text + tarball SRI) |
| `jhol patch` | extract → edit → `--commit` stores `patches/<pkg>+<ver>.patch`; `--list`, `--rollback`; re-applied after every install lane |
| `jhol global install/list/path/remove` | global prefix with POSIX bin shims |
| `jhol run`, `eval`, `exec` | run scripts / evaluate code / run a bin from `node_modules/.bin` |
| `jhol fmt` | format (comment-preserving) |
| `jhol build` | bundle via `jhol-bundle` |

---

## Environments, attestation, replay

| command | effect |
|---|---|
| `jhol env fork <name>` | copy-on-write fork of the installed tree into `.jhol/envs/<name>` (isolated writes, shared blocks) |
| `jhol env list` / `env diff a b` / `env rm <name>` | manage and compare forks |
| `jhol attest` | write `.jhol/attestation.json`: manifest, lockfile, packages, models, host axes — deterministic |
| `jhol attest --verify` | re-derive and compare; exit 1 with the reason on any change |
| `jhol replay <spec.json> [--execute]` | reproduce a published environment (deps as of a date, plus models); refuses to edit the manifest silently |

## Hardware axes and wheels

| command | effect |
|---|---|
| `jhol axes` | what jhol thinks this machine is: os, arch, libc, python, driver, sm, GPU memory (`--json`) |
| `jhol wheels <pkg>[@<ver>]` | every published artifact of a PyPI release classified against this host, with a reason for each rejection; exits non-zero if nothing fits (`--python`, `--json`) |
| `jhol py install <pkg>[@<ver>]` | resolve **and** install wheels for this machine into `.jhol/py/site-packages`, cached by content, recorded per-file (`--python`, `--pre`, `--json`); `jhol py list` |
| `jhol pysolve <pkg>[@<ver>]` | resolve a full Python dependency graph to artifacts that run here: markers evaluated, pre-releases excluded, wheels filtered by ABI (`--python`, `--pre`, `--allow-sdist`, `--json`) |

## Models (AI-first)

| command | effect |
|---|---|
| `jhol ai inspect --source hf://owner/repo --file model.safetensors` | describe a checkpoint from its header over a byte range (no weights fetched); `--path` for a local file, `--json` |
| `jhol ai pull --source hf://owner/repo --file …` | fetch into the shared model store and hard-link into `models/<repo>`; writes a commit-pinned manifest; `--revision`, `--dest`, `--max-bytes` |
| `jhol ai verify --path models/<repo> --manifest <repo>.jhol-model.json` | per-file SHA-256 verification of a pulled model |
| `jhol ai pin`, `pin-remote`, `prepare`, `infer` | manifest authoring, resumable prepare, tiny linear inference |

## Engines and runtime

`jhol install` can run scripts on JholVM with `JHOL_RUNTIME_ENGINE=jholvm`; `jhol
eval`, `jhol run` and `jhol exec` honour the same variable. See `ENGINE-JHOLVM.md`.

---

## Daemon and completion

`jhol daemon --serve|--stop`, `jhol completions <shell>`, `jhol git-hook`.

The daemon is an accelerator, not an authority: it can serve a stale verdict, so
`pkill -9 -f 'jhol-bin daemon'` is the first step of any measurement or any report
that contradicts the filesystem.

---

## Environment (most-used)

| variable | effect |
|---|---|
| `JHOL_CACHE_DIR` | cache root (tests and benchmarks pin it) |
| `JHOL_RUNTIME_ENGINE` | `jholvm` / `jsc` / `qjs` (unknown values are rejected) |
| `JHOL_DOWNLOAD_CONCURRENCY` | download parallelism (default `(cpus×4).clamp(32,128)`) |
| `JHOL_PRUNE_EXTRANEOUS=1` | allow pruning unauthorised directories |
| `JHOL_AS_OF`, `JHOL_ALLOW_LICENSE`, `JHOL_LICENSE_ALLOW_UNKNOWN` | set by the corresponding flags |
| `JHOL_NEST_RESOLVER=0` | use the previous nested resolver |
| `JHOL_STRICT_PEER_DEPS=1` | peer mismatches become failures |
| `JAGR_SEARCH_BUDGET` | resolver step cap |
| `JHOL_VERBOSE=1` | diagnostics on stderr |
| `JAGR_PROFILE_CHART=1`, `JHOL_PROFILE_INSTALL=1` | phase timings |

Full list: `grep -rhoE 'JHOL_[A-Z0-9_]+' crates src | sort -u` (179 today).
That number is a symptom, not an achievement.
