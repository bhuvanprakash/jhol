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
