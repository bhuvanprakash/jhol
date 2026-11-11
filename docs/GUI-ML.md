# The AI/ML surface: TUI and web dashboard

A package manager for ML work answers four questions, and jhol answers them on one screen
each. Everything below renders **one** struct — `jhol_core::status::Status` — collected by
`status::collect` and enriched once, in `src/status_view.rs`, with weight headers. Nothing in
either surface computes its own numbers.

| question | tab | web section |
|---|---|---|
| will it run on *this* box? | 2 Artifacts | `artifacts` |
| what will it cost? | 3 Budget | `budget` |
| what am I getting? | 2 Artifacts (detail pane) | `artifacts` + inspect box |
| can I prove it later? | 4 Proof | `proof` |

## TUI — `jhol gui`

```
 1. Status     tree verdict and reason, lockfile drift, policy, axes, proof
 2. Artifacts  one row per wheel and weight, marked ✓ ✗ ? with the reason; Enter describes
 3. Budget     missing/cached bytes, an ETA and the throughput it assumes, plus forks
 4. Proof      snapshot, attestation counts, the agent tool catalog
 5. Actions    verify · verify --prune · install · ci · policy — Enter runs the selection
 6. Packages   declared dependencies
 7. Health     doctor results
```

`?` means **unknown**, never a pass: a weight file whose resident size cannot be compared to a
GPU (`gpu_mem_mb` absent) reports `?` and says why. A wheel with no platform tag reports `?`
too.

Artifacts and Actions are the only tabs with new logic; the rest render fields.

## Web — `jhol gui --web`

One page on `127.0.0.1:<random>`; `std::net` and a page, no webview dependency.

| endpoint | method | does |
|---|---|---|
| `/` | GET | the page |
| `/status.json` | GET | the enriched status struct |
| `/inspect?path=…` | GET | header-only weight description, in-process |
| `/action?t=…` | POST | JSON reply `{ok, code, stdout, stderr}` |
| `/action?t=…&stream=1` | POST | chunked text, written as the child emits it |

Actions are a **whitelist of CLI invocations** — `verify`, `verify --prune`, `install`, `ci`,
`install --exclude-vuln`, `env fork <name>`, `env promote <name>` — never a shell. Fork names
are validated (`[A-Za-z0-9._-]`, no leading dash, no `..`), and an unknown action returns exit
code 2. Every mutating request needs the session token from the URL that was opened, so a page
on another origin cannot drive installs on loopback.

The inspect box takes a path, a directory, or a pinned model name and reads **only the
header**.

## Benchmarks (Apple arm64, `/usr/bin/env TIMEFORMAT=%3R`, five runs, seconds)

| | median |
|---|---|
| `jhol status` on a 71-package project | **8–10 ms** |
| `jhol inspect` 419 MB, 200 tensors | **5–6 ms** |
| `jhol inspect` 6.7 GB, 400 tensors | **6 ms** |

The last two are the point: reading 40 KB of a 6.7 GB checkpoint costs the same as reading a
419 MB one, because the cost is the header and not the weights. A 3.36 B-parameter model is
described — parameters, dtypes, 7.5 GB resident estimate — in 6 ms with nothing downloaded.

`jhol env promote` (two renames, no copying) measured **35–40 ms** on a 71-package tree, and
the tree verified against the lockfile immediately after.

## What is checked, not just written

- `status_view::tests` — a weight is measured from its header and compared to the host; no GPU
  means `?`, never `true`; a weight too big for the host is `false` **with the reason**; a
  weight path resolves against its model directory exactly once (the double-prefix defect).
- `status::tests` — a missing weight file is a plan item, not a pass; every agent-catalog tool
  name exists in the MCP server source.
- `tui::action_tests` — the TUI's five actions are byte-identical to the web surface's five.

## Deliberate limits

- **No auto-download.** Inspect never fetches; actions run only when asked, and the streaming
  panel is the only thing between a click and a multi-gigabyte fetch.
- **No chart decoration.** Bytes and seconds are printed with the assumption that produced
  them (`at 5 MB/s`), because a number without its assumption is a guess in a suit.
- **GPU memory is probed, not assumed.** No GPU → `?` everywhere, and the reason says so.

## Remote inspection: describe a checkpoint you have not downloaded

```
jhol inspect hf://openai-community/gpt2/model.safetensors
jhol inspect hf://meta-llama/Llama-3-8B/model.safetensors@5f2bc1d   # pinned revision
```

A safetensors file starts with `[8-byte length][JSON header]`, so the *structure* of a remote
checkpoint is behind one HTTP `Range` request for the first 128 KB — the common case is a
single request, and an unusually large header costs a second one for the remainder.

Measured on the public 548 MB GPT-2 checkpoint:

| | |
|---|---|
| bytes read | **14,291** (0.0026 % of the file) |
| range requests | **1** |
| wall time | 1.2 s, network-bound |
| reported | 160 tensors · 137.0 M params · F32×160 · 0.6 GB resident |

Local files go through the same path minus the fetch, and the web inspect box accepts
`hf://` too — the same function, `status_view::inspect_target`.

## npm native packages in the Fitness table

The table used to hold wheels and weights only. Native npm packages declare where they may
run, and they are the only npm packages that can fail for *this* machine:

```
 ✓ package @esbuild/darwin-arm64 0.20.0
      os darwin (host darwin)
      cpu arm64 (host arm64)
 ✗ package @esbuild/linux-x64 0.20.0
      os linux (host darwin)
      cpu x64 (host arm64)
```

Rows come from `package-lock.json` entries that declare `os`, `cpu` or `libc`. A pure-JS
package gets **no row**: 71 packages that all fit is noise, and noise is how a warning stops
being read. The host is translated into npm's dialect (`macos`→`darwin`, `x86_64`→`x64`), with
a test for the translation and one for the filtering.

## Live progress in the TUI

Actions used to run to completion behind a spinner. They now stream: the child's stdout is
read line by line on a worker thread, sent over a channel, and appended to the Actions pane on
every frame, so a download reports progress in the terminal the same way the browser does. The
exit line (`[exit 0]`) is appended by the same loop that reads the output, so what you see is
what ran.

## The MCP bridge: one surface, two drivers

| endpoint | does |
|---|---|
| `GET /tools.json` | `tools/list` from the real MCP server, in-process |
| `POST /mcp?t=…` | any JSON-RPC request — `tools/list`, `tools/call`, notifications |

The bridge calls `jhol_core::mcp::handle_mcp_request` directly. There is no second server and
no second tool list: a test asserts the catalog and the MCP server answer **the same** tools in
both directions, after this work found two tools the dashboard was hiding
(`jhol_plan_readonly`, `jhol_sync_lock`).

Verified over HTTP: `tools/list` → 12 tools; `tools/call jhol_axes` → the host axes JSON; a bad
token → `-32001`; malformed JSON → `-32700`; a notification → no response, per JSON-RPC.

## Conditional polling: an idle dashboard costs nothing

A dashboard polls once a second. Collecting a status walks `node_modules`, reads three
lockfiles and enriches weights; a fingerprint of the seven files a status depends on
(`package.json`, the lockfiles, `node_modules`, `.jhol/envs` mtimes and sizes) costs
microseconds. When the fingerprint matches the request's `If-None-Match`, the server answers
**304 and collects nothing at all**.

Measured over 20 polls of a 71-package project, curl included:

| | per poll |
|---|---|
| full `200` (collect + enrich) | **15.2 ms** |
| `304` (fingerprint only) | **6.7 ms** |

The 6.7 ms is curl process startup — the server's share of that is under a millisecond.
Verified that the fingerprint is honest: creating a fork changes `.jhol/envs` mtime and the
next revalidation returns `200`, not a stale `304`.

## Look before you leap

Anything that writes is armed on the first `Enter` (TUI, 20 s window) or click (web, 10 s),
with the plan it was armed against shown in the output pane:

```
install — from package.json
  nothing to download; this re-materialises from the lockfile

  Enter again within 20 s to run · anything else cancels
```

Read-only actions (`verify`, `verify --prune`) run immediately: confirmation on a question is
friction, confirmation on a download is care. The rule is the plan's bytes, not a policy list.

## Heaviest tensors

`jhol inspect --top N` (default 5) and the detail pane name where the bytes actually are:

```
  heaviest tensors
    transformer.h.11.attn.c_attn.weight         6.4 MB
    transformer.h.11.mlp.c_fc.weight            6.3 MB
```

Same data in `--json` (`largest`), the TUI artifact detail pane, and the web inspect box.

## One walk, not two

`status` used to walk the tree twice — once to verify it, once to count packages, the second
counter calling `std::env::var` per entry. `jhol_treecheck::inspect_tree` now returns the
verdict **and** the count from one walk. Collection dropped from ~5 ms to ~4 ms on 71
packages and the count is the same number the verifier saw, not a second opinion.

## Sharded models are read as a whole

Real checkpoints are sharded: `model-00001-of-00008.safetensors`… Describing one shard and
calling that the model under-reports parameters by the **shard count** — the kind of wrong
that looks right. `jhol_ai::safetensors::summarise_dir` reads every shard, sums parameters and
dtype counts, and reports which loader sidecars are present:

```
models/sharded
  3 tensors · 2000 params · 4000 bytes of tensor data across 2 shards
  dtypes     BF16×3
  sidecars   config.json ✓  tokenizer.json ✓  tokenizer.model ✗  generation_config.json ✗
```

Fixture: two 1000-parameter shards reported as **2000** parameters; the single-shard reading
(1500) was the bug this replaced. The dashboard's enricher uses the same function, so the
Artifacts tab and `jhol inspect` cannot disagree about a model's size.

## One description of one download

The confirmation sentence is produced by `jhol_core::status::action_preview`, and the TUI and
the web page both print **that**. The web asks the server (`GET /plan?action=…`); the TUI calls
the function directly. Before this, each surface composed its own sentence — the exact failure
mode that later becomes "the terminal said 40 MB and the browser said 3 MB".

```
verify  → reads 71 packages and compares versions with the lockfile — no writes
prune   → nothing to prune; the tree holds only authorised packages
install → nothing to download; this re-materialises from the lockfile
ci      → …; ci refuses to write a lockfile that would drift
policy  → …; policy in force: as-of 2023-06-01 · licences MIT,ISC
```

Three tests pin it: reads never ask for confirmation, writes always do, and the estimate names
its throughput and scales with the plan.

## `jhol status --watch`: refresh for the price of a fingerprint

One process, one line, rewritten in place on a terminal and printed per change when piped.
Each tick compares `jhol_core::status::fingerprint` — the same tag the dashboard's `ETag` uses —
and **skips collection entirely when nothing changed**.

| | per refresh |
|---|---|
| `--watch`, nothing changed | **~0.2 ms of CPU** (user 0.00 s, sys 0.02 s over ~100 ticks) |
| `jhol status` one-shot | 7–8 ms wall, ~18 ms CPU per run |

```
fp 1.0.0 · tree match · 71 pkgs · lock ok · 0.0 MB missing (~0 s) · artifacts 0 unfit 0 · unknown 0 · forks 1
```

`touch package.json` mid-watch produces exactly one new line — the change is detected, not
assumed.

## Filtering, because a list of 71 things is not a dashboard

- **TUI**: `u` on the Artifacts tab shows only what does not fit; rows are ordered
  failures → unknowns → fits, because the question is "what is wrong", not "what is here".
- **Web**: a filter box (name, kind, or reason) and an "only what does not fit" checkbox; the
  table carries `data-fit` so filtering is a CSS toggle, not a re-render.
