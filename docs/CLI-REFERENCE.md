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
| ~~`JHOL_NEST_RESOLVER=0`~~ | retired: the instance resolver is the only first attempt |
| `JHOL_STRICT_PEER_DEPS=1` | peer mismatches become failures |
| `JAGR_SEARCH_BUDGET` | resolver step cap |
| `JHOL_VERBOSE=1` | diagnostics on stderr |
| `JAGR_PROFILE_CHART=1`, `JHOL_PROFILE_INSTALL=1` | phase timings |

Full list: `grep -rhoE 'JHOL_[A-Z0-9_]+' crates src | sort -u` (179 today).
That number is a symptom, not an achievement.

## The front door

`jhol --help` states what the tool is — *verified installs for npm packages, Python wheels and
model weights* — and then groups the surface by intent rather than by history: **Every day**,
**Trust the tree**, **Choose what goes in**, **Artifacts**, **Reproduce and prove**, **Engine
and tools**.

Sixteen commands from an earlier blueprint era (`plan`, `diff`, `sync`, `snapshot`, `restore`,
`db`, `dev`, `jagr`, `audit-jagr`, `betti-history`, `jagr-impact`, `pack`, `publish`, `login`,
`cdn`, `view`) are **hidden by default**, because a command list is a promise and those
sixteen were a different project. They still run; `JHOL_BLUEPRINT=1 jhol --help` shows them.

### `jhol status [--json]`

One screen of truth, and the only implementation of it. The TUI dashboard and `jhol gui --web`
render the same `jhol_core::status::Status` struct; nothing recomputes the numbers.

```
demo 1.0.0

  tree       match — 71 packages
  lockfile   matches package.json
  policy     as-of 2023-06-01 · licences MIT,ISC
  axes       macos/arm64 libc unknown · python - · cuda none
  proof      snapshot ready · attested 71 packages, 0 models
```

`--json` is the contract for editors, dashboards and agents.

### `jhol gui` and `jhol gui --web`

`jhol gui` is the terminal dashboard (ratatui), five tabs:

| tab | shows |
|---|---|
| **1. Status** | the `jhol status` struct — tree verdict and reason, lockfile drift, policy, axes, artifacts, proof |
| **2. Packages** | declared dependencies, direct and dev |
| **3. Cache** | tarballs, bytes, contents |
| **4. Health** | `doctor` scan results |
| **5. Actions** | the five whitelisted invocations; **Enter** runs the selected one and shows its output |

The dashboard no longer collects its own numbers: tab 1 renders `jhol_core::status::Status`,
the same struct the CLI and web page use. The old JAGR-invariants tab is gone — it displayed a
secondary analysis in a place where the verdict belongs.

`jhol gui --web` serves the same data as a single page on `127.0.0.1:<random>` and opens the
browser: no webview dependency, no second implementation, `/status.json` and `/action` behind
it. A unit test asserts the TUI's five actions are byte-identical to the web surface's, so the
two cannot drift apart silently.

### The write contract: `POST /action`

The web view is not read-only. It exposes exactly **five** actions, and every one is a
whitelisted CLI invocation — the same binary a user would run, so verification, policy and
exit codes are the ones already tested:

| action | runs |
|---|---|
| `verify` | `jhol verify` |
| `prune` | `jhol verify --prune` |
| `install` | `jhol install` |
| `ci` | `jhol ci` |
| `policy` | `jhol install --exclude-vuln` |

There is no shell and no arbitrary command: an unknown action is refused with exit 2. Requests
must be `POST` and must carry the session token embedded in the URL that was opened — a random
web page cannot drive installs on `127.0.0.1`, even though it can reach the port.

The reply is JSON: `{ok, code, stdout, stderr}` with the last 8 KB of each stream, because the
useful part of an install log is at the end.

Verified: `verify` → `code 0, "OK: 71 packages match the lockfile"`; `prune` and `install`
succeed; a bad token returns `403 bad token`; `{"action":"rm -rf /"}` returns
`code 2, unknown action`.

## Inspecting weights: `jhol inspect`

```
jhol inspect models/llama-3-8b            # a directory
jhol inspect ./model.safetensors --json   # one file
jhol inspect llama-tiny                   # a name pinned in .jhol/models.lock.json
jhol inspect hf://openai-community/gpt2/model.safetensors   # remote, header only
jhol inspect hf://meta-llama/Llama-3-8B/model.safetensors@5f2bc1d  # a pinned revision
jhol inspect hf://openai-community/gpt2/model.safetensors --top 8  # heaviest tensors
```

`.gguf` files are read too — magic, metadata table and tensor descriptors — reporting
`format gguf`, `general.architecture`, the quantisation mix (`Q8_0×254 F32×145`), parameter
count, and a resident estimate from GGML packet sizes. The probe starts at 128 KB and doubles
by fetching only the delta, because a GGUF header carries the tokenizer vocabulary (16 MB for
Qwen3's 151k tokens, out of a 9.4 GB file).

A **directory** is read through every shard (`model-0000N-of-0000M.safetensors`) and reports
`config.json` / `tokenizer.json` / `tokenizer.model` / `generation_config.json` presence. Reading
one shard under-reports parameters by the shard count.

`--top N` lists the N heaviest tensors (default 5, `0` hides them), because the answer to
"what is inside?" is usually "one big matrix and a lot of small ones".

A remote target costs one HTTP `Range` request for the first 128 KB (a second only if the
header is larger). Measured on the 548 MB GPT-2 checkpoint: **14,291 bytes read, 1 request,
1.2 s** — network-bound, not size-bound.

A safetensors header maps every tensor to a byte range, so the *structure* of a checkpoint is
readable from a few kilobytes:

```
/tmp/MW/huge.safetensors
  400 tensors · 3.36 B params · 6710886400 bytes of tensor data
  dtypes     BF16×400
  resident   7.5 GB — no GPU memory detected, fit unknown
  header     40153 bytes read (0.0006% of the file)
```

`resident` is tensor bytes plus 20% for the allocator and the object graph — deliberately
generous, because an under-quote here is an out-of-memory at the worst moment. Nothing is
downloaded and nothing is loaded. When a GPU is detected the last line states the comparison
(`fits` / `does not fit`); when it is not, the verdict is *unknown*, never a pass.

Measured: **5–6 ms** for both a 419 MB and a 6.7 GB file (the cost is the header).

## Promoting a fork: `jhol env promote <name>`

Makes a fork the project's tree with **two renames and no copying**: the current `node_modules`
moves aside, the fork's moves in, identity files are copied back, and the replaced tree is kept
until the new one is renamed into place — a failed promote leaves the project exactly as it
was. Then it verifies.

```
promoted trial2 in 37 ms — tree matches the lockfile
```

## The ML dashboard: `jhol gui` and `jhol gui --web`

See `docs/GUI-ML.md`: seven tabs / six web sections, all rendered from the same status struct,
with an artifact fitness table, a byte budget that states its throughput assumption, a proof
tab, streaming actions, and header-only weight inspection.

## Watching: `jhol status --watch [--interval MS] [--json]`

```
fp 1.0.0 · tree match · 71 pkgs · lock ok · 0.0 MB missing (~0 s) · artifacts 3 unfit 1 · unknown 1 · forks 1
```

Rewrites one line on a terminal and prints one line per change when piped, so
`jhol status --watch | grep -v 'tree match'` is an alarm. A tick compares
`jhol_core::status::fingerprint` first and **collects nothing when nothing changed** —
~0.2 ms of CPU per idle tick against 7–8 ms for a one-shot run.

## `/plan?action=…`

Returns `{action, writes, summary}` — the same sentence the TUI shows before it runs an
action, from the same function (`jhol_core::status::action_preview`). `writes: false` means the
action needs no confirmation.
