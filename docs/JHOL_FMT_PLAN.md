# jhol fmt — the cache-first formatter

**Status:** Phase 1 engines landed (JS/TS/JSX/TSX/JSON/CSS + CLI). This plan is the
"beat bun fmt" push — and it is deliberately *not* a bun/prettier/biome clone.

## Why jhol fmt is a different animal

bun fmt = one binary, one parser, Prettier-compatible output, parallel files,
**no persistent cache** (every run re-parses everything). Prettier = Node
process, JS printer. Biome = Rust, one parser+printer. All three share the same
model: *parse → print → write, per file, from scratch, every run.*

jhol fmt breaks that model on purpose, using ideas no other formatter has:

1. **One parse, many tools.** `jhol build` already parses every JS/TS file into
   jhol's content-addressed store (CAS). `jhol fmt` reuses those parse results
   instead of re-parsing. A project that runs `jhol build && jhol fmt` pays for
   parsing once. No other toolchain shares parse state between tools.
2. **Cross-project memoization.** Formatted output is a pure function of
   (content hash, engine version, config digest). That mapping is memoized in
   the *global* jhol cache, so the same dependency file formatted in ten
   projects is formatted once, ever. bun fmt's cache is per-run at best.
3. **Diff-minimal formatting.** Never rewrite a line that doesn't need it.
   Output is idempotent *and* stable: `fmt(fmt(x)) == fmt(x)` and the diff
   touches only lines the engine must change. This is a correctness guarantee
   the others don't make (Prettier reflows aggressively).
4. **Range formatting** (`--range start:end`) — format only an edited region,
   for format-on-save without whole-file churn.
5. **Daemon mode** — a persistent formatter behind jhol's existing daemon
   socket: sub-millisecond format-on-save, zero process startup.
6. **Formatting receipts** — jhol's receipt system records *what* changed and
   *why* (engine, config, hash), so formatting is auditable and diffable
   (`jhol diff` on receipts).
7. **Polyglot by routing, not by reimplementing** — jhol's adapter registry
   dispatches languages with mature native tools (rustfmt, gofmt, ruff) instead
   of growing a 20-language parser.

Spec compatibility (Prettier option names, `.prettierrc` read) is a *service*
to users, not an architecture we copied.

## Architecture

```
jhol fmt
├── discovery    walkdir + built-in ignores + .prettierignore (globset)
├── config       CLI > .jholrc [fmt] > .prettierrc(.json) > spec defaults
├── engines      swc (JS/TS) · serde_json (JSON) · lightningcss (CSS)
│                · external dispatch (rustfmt/gofmt/ruff) · CAS reuse (P2)
├── memo         .jhol/fmt-cache.json (per-project) + global CAS (P2)
└── io           rayon parallel · atomic temp+rename · receipts · exit codes
                 0 clean / 1 needs-formatting / 2 error
```

## Phases

**Phase 1 — core (this change)**
- [x] JS/TS/JSX/TSX via swc; JSON via serde_json; CSS/SCSS/Less via lightningcss
- [x] `jhol fmt [path]...`, `--check`, `--list-different`, `--stdin-filepath`
- [ ] `FmtConfig` (printWidth, tabWidth, useTabs, semi, singleQuote,
      trailingComma) from `.jholrc`/`.prettierrc`/CLI; tabWidth+useTabs applied
- [ ] Cache-first skip (`.jhol/fmt-cache.json`, engine+config digest keyed)
- [ ] Built-in ignores + `.prettierignore`; minified/lockfile skip
- [ ] External dispatch: rustfmt / gofmt / ruff format
- [ ] `--init`, `--no-cache`, atomic writes, exit codes 0/1/2
- [ ] Benchmarks

**Phase 2 — the unique wins**
- [ ] CAS parse reuse: share `jhol build`'s parse manifests with `jhol fmt`
- [ ] Global cross-project memoization of (hash → formatted) pairs
- [ ] Diff-minimal printer pass (line-stability guarantee)
- [ ] Full option application: semi, singleQuote, trailingComma, printWidth wrap
- [ ] Import sorting; `package.json` key sorting (conventional order)

**Phase 3 — workflows**
- [ ] `--range start:end` for editors; daemon format-on-save over jhol socket
- [ ] Formatting receipts (`jhol diff` integration)
- [ ] `--staged` / `--changed <ref>`; `bunfig.toml [fmt]` read (migration)
- [ ] TOML / YAML / Markdown code blocks via adapter registry

**Phase 4 — measurement**
- [ ] Criterion benches: cold, warm (cached), large monorepo, daemon p50/p99
- [ ] CI matrix vs prettier / biome / bun fmt (once it ships in stable Bun)

## Honest perf strategy

| Axis | bun fmt | jhol fmt |
|---|---|---|
| Process model | in-process Zig | in-process Rust (swc) |
| Parallelism | per-core threads | rayon work-stealing |
| Cold run | fast | fast (same class) |
| **Repeat runs** | full re-parse | **hash skip → ~zero work** |
| **Build+fmt** | two parses | **one parse (CAS reuse)** |

We don't claim 320x. We win the axes competitors leave on the table:
repeat-run cost and shared parse work. That is the whole point.

## Non-goals

- Not a clone: no copied source, no copied architecture. Option *names* follow
  the Prettier spec because they are the de-facto standard, nothing more.
- No Node.js runtime, no npx, no network, ever.
- No 20-language mega-parser. Route to native tools instead.
