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

## Next slices, in order

| slice | what it adds | acceptance |
|---|---|---|
| P1b | models in the project lockfile (not only a side manifest) | `jhol verify` covers npm + model files in one command |
| P1c | streaming + resumable pulls (current cap: one file ≤ `--max-bytes`, buffered) | interrupt at 60 %, resume, digest still matches |
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
