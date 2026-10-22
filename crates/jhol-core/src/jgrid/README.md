# jhol-core::jgrid

**JGrid** — the Jhol *instant materialization* pipeline. A content-addressed,
memory-mappable, clone-ready package storage format that replaces `.tgz`
tarballs as the canonical cache artifact, enabling sub-millisecond
`node_modules` materialization.

> **Status:** production-wired for L0 + L1-store. Heavy L2 `store.jgrid` ingest
> remains opt-in (`JHOL_JGRID_INGEST=1`).

## Four-tier materialization (world-class CAS path)

Inspired by pnpm CAFS, LPM virtual stores, NativeLink CAS, and conda/APFS
clonefile practice: keep **immutable content-addressed package trees**, then
materialize with **whole-directory COW clones** — not per-file walks from a
giant mmap blob on the hot path.

| Tier | What it is | When it fires |
|------|------------|---------------|
| **L0** | Graph-keyed whole-tree `node_modules` bundle (`dep_set_hash`) | After any successful install (default); clonefile once on hit |
| **L1-store** | Parallel `clonefile` from `store_unpacked/<shard>/<hash>/` | Every package already complete in the CAS unpacked store |
| **L1** | Pre-assembled per-package trees from JGrid manifests | When `store.jgrid` has manifests + tree cache |
| **L2** | Content blocks — mmap'd, content-addressed file data | Fallback when L0/L1 miss and the store is ingested |

Order: light L0 → L1-store → verified L0 → L1 → L2. Route markers under
`JHOL_JGRID_ROUTE=1`: `[JGRID] L0`, `[JGRID] L1-store`, `[JGRID] L0-publish`.

## Env knobs

| Var | Default | Effect |
|-----|---------|--------|
| `JHOL_JGRID_L0` | on | Publish durable L0 after install (`0` disables) |
| `JHOL_JGRID` | auto | Force attempt even without cache artifacts |
| `JHOL_JGRID_INGEST` | off | Heavy L2 ingest into `store.jgrid` |
| `JHOL_JGRID_ROUTE` | off | Print which tier materialized |

## Format (`src/format/`)

A `.jgrid` file is a single self-describing binary:

- **Content blocks** — deduplicated byte buffers addressed by SHA-256
- **Swiss Table** — O(1) SHA-256 → (offset, size) via a 16-bit-control-word /
  56-bit-slot SIMD probe table (`swiss_table.rs`, `perfect_hash.rs`)
- **Package manifests** — per-package path → content-hash maps (`manifest.rs`)
- **String table** — interned path strings shared across manifests
- **Tree DAG** — optional serialized directory tree (`tree_dag.rs`)
- **Header** — 256-byte header with absolute offsets (`header.rs`)

Designed for **mmap access** (no parse at read time), **zero-copy reads**
(`&[u8]` slices into the mapping), and **O(1) lookup** (~2–6 ns).

Full specification: [`spec.md`](./spec.md). Design/rollout: [`PLAN.md`](./PLAN.md).

## Modules

```
ingester/    from_tarball.rs, tree_builder.rs
materialize/ cloner.rs, installer.rs, store_bridge.rs
store/       grid_store.rs, tree_cache.rs,
             bundle_cache.rs, builder.rs, gc.rs
format/      header, manifest, swiss_table, tree_dag,
             string_table, perfect_hash
```

## Usage

```rust,ignore
use jhol_core::jgrid::{JGridPipeline, ResolvedPackage};

let pipeline = JGridPipeline::new("~/.jhol-cache/grid")?;
let packages = vec![ResolvedPackage { name: "lodash".into(), version: "4.17.21".into() }];

if pipeline.materialize(&packages, "./node_modules")? {
    println!("instant install");
}
```
