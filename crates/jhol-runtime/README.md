# jhol-runtime

A JavaScript / TypeScript / JSX runtime for Jhol, aimed at running JS **without
Node.js, Deno, or Bun** — an embeddable engine plus a Node-compatible standard
library, wired into the `jhol` CLI.

> **Status: experimental.** The runtime is part of the default `jhol` build, but
> treat it as in-progress. It is not a drop-in replacement for V8/Node yet.

## Engines

Two engines sit behind the `Engine` abstraction (`src/engine.rs`):

| Engine | Feature | Notes |
|--------|---------|-------|
| **QuickJS** (`quickjs-rusty`) | `native-engine` *(default)* | Lightweight, embeddable, no MSRV friction |
| **JavaScriptCore** (`javascriptcore`) | `jsc-engine` | Apple/WebKitGTK, includes JIT |

`mimalloc` (default) backs the global allocator for JS-heavy workloads.

## Features

From `Cargo.toml`:

```toml
default        = ["native-engine", "mimalloc"]
native-engine  = ["dep:quickjs-rusty"]   # QuickJS
jsc-engine     = ["dep:javascriptcore"]  # JavaScriptCore + JIT
mimalloc       = ["dep:mimalloc"]
swc-transpile  = ["dep:swc_core"]        # TypeScript / JSX
native-watcher = ["dep:notify"]          # OS file events (else mtime polling)
http-server    = ["dep:tiny_http"]
websocket      = ["dep:tungstenite"]
sqlite         = ["dep:rusqlite"]
```

At the workspace root these are exposed as `runtime` (default),
`runtime-server`, `runtime-websocket`, and `runtime-sqlite`.

## Node-compatible builtins

`src/modules/`: `console`, `fs`, `path`, `os`, `process`, `crypto`, `buffer`,
`events`, `http`, `timers`, `util`.

## Caching

- `bytecode_cache.rs` — cached compiled bytecode
- `transpile_cache.rs` / `transpile.rs` — TypeScript & JSX transforms
- `module_cache.rs` — module resolution cache
- `resolver.rs` — module resolution (Node-style)
- `tropical_graph.rs` — sparse module graph for load order
- `jit.rs`, `timer.rs`, `watch.rs` (mtime polling + optional `notify`)

## Usage

Via the CLI:

```bash
jhol run script.ts        # run a JS/TS file or package.json script
jhol eval "1 + 1"         # evaluate inline code
jhol build entry.ts       # bundle for browser / node / jhol
jhol run --watch app.ts   # hot reload (see watch.rs)
```

Advertised compatibility: `NODE_COMPAT_VERSION = "23.0.0"`.

## Building / testing

```bash
cargo build -p jhol-runtime
cargo test  -p jhol-runtime
cargo build -p jhol-runtime --features jsc-engine,swc-transpile,native-watcher
```

## See also

- `crates/jhol-runtime/examples/`
- ``crates/jhol-core/src/jgrid/README.md`` — instant materialization
- Root `README.md`, `CHANGELOG.md`
