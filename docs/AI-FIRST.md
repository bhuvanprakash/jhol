# AI-first: models as first-class artifacts

Status of the plan, honestly. This document tracks what exists, what is measured, and
what remains — the same discipline as `PRODUCTION-READINESS.md`.

## The thesis

> An ML environment is not a list of packages. It is npm packages, Python wheels,
> toolchain constraints, model weights and datasets that must be reproducible
> together on hardware the user does not control.

**One graph, one lockfile, one proof — over packages, wheels, models and data.**

## What the ecosystem exposes (verified, not assumed)

| fact | consequence |
|---|---|
| safetensors = `[u64 LE header length][JSON header][data]`, header maps each tensor to a byte range | a model's structure is readable in kilobytes — **before any download** |
| HF resolve returns `ETag`, `X-Linked-Etag`, `X-Repo-Commit`, `Accept-Ranges` | per-file digests, byte ranges, and branch→commit pinning without a hub of our own |
| PyPI wheel filenames encode the full ABI stack (`cp310-abi3-manylinux_2_17_aarch64`) | hardware/ABI compatibility is derivable offline, as a mask |

## Shipped

### Structure without weights — `jhol ai inspect`

```
$ jhol ai inspect --source hf://hf-internal-testing/tiny-random-bert --file model.safetensors
100 tensors, 126,851 parameters, header 10752 bytes (data starts at 10760)
dtypes: F32 x99, I64 x1
```

Measured: two byte-range requests (**10,760 bytes**) and 1.16 s for a 520 KB file —
and the same 10,760 bytes for a 7 GB checkpoint. Implemented as
`jhol_ai::safetensors` (pure parser + `parse_file`), with `header_len` sanity caps so a
hostile header cannot choose our allocation size.

### Dedupe that cannot be poisoned — `jhol ai pull` + model CAS

```
$ jhol ai pull --source hf://owner/repo --file model.safetensors
1 file(s), 1 downloaded, 520212 bytes linked from ~/.jhol-cache/model_store
manifest: models/<repo>/<repo>.jhol-model.json
```

Measured on two projects pulling the same checkpoint:

| property | result |
|---|---|
| disk for two projects | **one copy** (same inode), 520,212 bytes |
| store entries | 1 |
| entry mode | `0444` |
| in-place tamper | **blocked by the filesystem** |
| digest mismatch | caught by `ai verify` |

The design detail that matters: a project copy and the store entry **share an inode**, so
an in-place edit rewrites the stored bytes for every project. Entries are therefore
written read-only, `put` verifies an existing entry before trusting it (and repairs it),
and `materialise` refuses to hand out bytes that disagree with their own name. My first
version had none of that, and a "repair" happily re-linked corrupted bytes.

Manifests pin a **commit**, never a branch: `main` is resolved once through
`X-Repo-Commit` and recorded, because a pin that moves is not a pin.

### Models under `jhol verify` — one command, packages *and* weights

`jhol ai pull` now writes `.jhol/models.lock.json` (commit + per-file SHA-256 + size),
and `jhol verify` checks it together with the tree:

```
$ jhol verify
OK: models match the lock (no package lockfile)

$ # after editing a checkpoint in place
$ jhol verify ; echo $?
NOT OK: tiny-random-bert: models/.../model.safetensors is 2f892aa16f8e but the lock pins 965f02b6a7e5
  run `jhol ai pull ...` to restore the pinned model files
1
```

`package-lock.json` stays npm's file; the model lock lives beside it in `.jhol/`, so a
project is still installable by npm while jhol holds it to a stronger standard. The
verifier's shape is deliberately the same as the tree verifier: compare disk with what
the lock authorises, name the difference, exit non-zero.

### Streaming pulls with resume

Downloads stream to disk with hashing in one pass — a 7 GB checkpoint never enters
memory — and resume by range. Proven end to end rather than asserted:

```
$ jhol ai pull … --max-bytes 100000     # interrupted
  partial bytes: 98304
$ jhol ai pull …                        # resumed via Range/206
1 file(s), 1 downloaded, 520212 bytes
$ jhol verify
OK: models match the lock
```

A server that ignores the range (`200` instead of `206`) restarts the file rather than
appending a duplicate: a silently corrupted checkpoint that hashes wrong is the worst
possible outcome of a "resume".

## Next slices, in order

| slice | what it adds | acceptance |
|---|---|---|
| ~~P1b~~ | models in the project lockfile | **done** — `jhol verify` covers npm + model files |
| ~~P1c~~ | streaming + resumable pulls | **done** — interrupted at 98,304 B, resumed, digest verifies |
| P0 | one `Artifact` shape across npm, wheels, models | chart/verify tests unchanged, one verifier entry point |
| P3 | hardware/ABI axes in resolution (wheel tags + device detection) | 3 hardware profiles resolve differently and each verifies |
| P4 | `why-not --axis`: the minimal fix for a CUDA/torch/xformers conflict | one-line fix for the canonical ML conflict |
| P5 | `env fork` (CoW), bisect across forks | 6 GB environment forked in <2 s, ~0 extra bytes |
| P6 | determinism axes + `attest` (in-toto-shaped, offline verifiable) | attestation verifies on a clean offline machine |
| P7 | `replay paper.yaml`, MCP tools with cost estimates | a paper's environment reproduces; an agent fixes a broken env via MCP only |

## Non-goals

Not a hub, not a Python solver (delegate to uv/pip through the adapter and represent the
result), not a training framework. No copying: the moat is the node shape and bitmap
policy memory, not another tool's CLI.
