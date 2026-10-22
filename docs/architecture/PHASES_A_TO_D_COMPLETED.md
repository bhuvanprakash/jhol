# Phases A–D — implementation status and recorded evidence

Status as of **30 Sep 2026** (darwin-aarch64).  
Latest measured run: **`20260930T022752Z-Darwin-arm64`** — **30/30 PASS**, plus stress extras all PASS.  
Authoritative checklist: `JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md` §18.  
Evidence: `docs/architecture/blueprint-records/LATEST.md` + `history.jsonl`.

After that recorded gate, `cargo test -p jhol-remote` passed **9/9** with
loopback transport, retry, and signature-vector coverage. This targeted run is separate
from the recorded 30/30 and does not establish live provider compatibility.

An earlier default-feature workspace run on 2026-09-30 passed **645 tests, 0
failed, 12 ignored** and emitted existing compiler warnings. After the adapter
follow-up, `scripts/blueprint_gate.sh` passed again: 462 core library tests
passed, 2 were ignored, and foundation tests plus the offline polyglot fixture
passed. The recorded blueprint gate remains its own 30-check evidence run.

---

## Brutal test summary

| Layer | Result |
|-------|--------|
| Recorded suite (`blueprint_record.sh`) | **pass=30 fail=0 skip=0** |
| Plan idempotence | stable across 2 calls |
| Sync hammer | 3× exit 0; lock + `.jhol/action-cache` present |
| Snapshot hammer | 3 distinct snapshot ids |
| Restore DB auto-apply | **refused** (exit 1, explicit error) |
| AI integrity | tamper → digest mismatch; restore → verify ok |
| Clippy foundation `-D warnings` | exit 0 |
| `examples/blueprint/verify.sh` | exit 0 |
| `jhol-remote` fail-closed units | 3/3 pass |

**Failures this run:** none.

---

## Phase A — Preserve / consolidate (gates)

**Completed and proven**

| Item | Evidence |
|------|----------|
| Core lib suite | `cargo test -p jhol-core --lib` PASS in record (~2.3s) |
| Foundation crate libs | graph/adapters/receipt/policy/task/env/diff/ai/db/remote PASS (~7.5s) |
| npm/Bun/Jhol harness | `tests/compatibility/compare_versions.sh` PASS (~3.7s) |
| CLI help ↔ dispatch | plan/diff/sync/snapshot/restore/db/ai/dev all in `--help`; db plan+probe; ai verify+prepare |
| Clippy warn gate | foundation crates `clippy --lib -D warnings` EXIT 0 |
| Build | `cargo build --bin jhol` → `target/debug/jhol` |

**Not completed (owner-only)**

| Item | Status |
|------|--------|
| Create GitHub org + transfer + five-repo split | Open — needs owner credentials |

---

## Phase B — Graph / adapters / plan / lock / cache (implemented local slice)

**Verified paths**

| Item | Evidence |
|------|----------|
| `jhol plan --json` | Shows javascript adapter; kind stable across repeats |
| `jhol sync` | Writes `jhol.project.lock` with `format_version: 1` and native `package-lock.json` digest |
| Action cache | `examples/blueprint/.jhol/action-cache/` populated after sync |
| Adapter discovery and plan | `jhol plan --json` reports concrete JS, uv, and Cargo plans with project roots, output paths, capability requirements, and limitations |
| Adapter apply | JS runs Jhol's native install with scripts disabled; Python invokes uv; Rust invokes `cargo build --workspace`; all use shell-free argument arrays and verified output digests |
| Apply policy | Every plan is authorized and native tools are preflighted before the first write; Python/Cargo build execution needs `--allow-build-scripts`; `--offline` disables network permission and reaches all tools |
| Polyglot example | `examples/blueprint/verify.sh` EXIT 0; a temporary no-dependency fixture applied JS/Python/Rust offline, verified outputs, then the broader example checks passed |
| Lock invalidation | Graph crate detects modified, added, and removed native locks; the CLI fixture adds/removes `requirements.txt`, checks lock contents and a changed cache key; cached actions are not replayed |

**Open gate:** real third-party Python/Rust installs and runs across supported platforms. The current fixture validates offline orchestration without exercising external registries or wheels.

---

## Phase C — Snapshot / AI / DB / tasks / demo

**Completed and proven**

| Item | Evidence |
|------|----------|
| Snapshot create/list path | 3 sequential creates → 3 UUIDs |
| Restore `--plan` | PASS |
| Restore `--apply --databases` | **FAIL-CLOSED** — blocked with clear error |
| `jhol db plan` | Finds `001_init` migration |
| `jhol db probe` | JSON includes `database_url_set` |
| `jhol ai verify` / `prepare` | PASS; tamper detection proven |
| `jhol dev --json` | `boot_order` present |
| `jhol dev --spawn --hold-secs 1` | PASS (~2.3s) — spawn + Drop cleanup |
| Polyglot demo markers | JS/Python/Rust + migrations + model pin in example |

**Intentionally incomplete**

| Item | Note |
|------|------|
| Full production ML inference stack | Pin/verify/prepare only — not a serving stack |

---

## Phase D — Optional remote CAS

**Implemented locally and tested; live service remains unverified**

| Item | Evidence |
|------|----------|
| `schemas/remote-cas-v1.schema.json` | File present |
| `crates/jhol-remote` | 9/9 targeted tests: tenant mismatch, unconfigured refusal, tamper rejection, AWS SigV4 example, loopback PUT/GET, 429 and disconnect retries, invalid endpoint/path refusal |
| Local-first | `jhol-remote` **not** imported in `src/main.rs` |
| S3-compatible transport | Path-style SigV4 GET/PUT; tenant-scoped digest key; 512 MiB bound; redirects disabled |

**Still open (needs live service and owner decisions)**

| Item | Note |
|------|------|
| Live AWS S3 + Arenvara backends | Protocol compatibility is implemented; provider setup and behavior are not verified |
| Live throttle / poison / disconnect suite | Needs real endpoint |
| `jhol-cloud` control plane | Not started |
| Remote task execution | Blocked on trust review |

---

## How to re-prove

```sh
bash scripts/blueprint_record.sh   # → updates LATEST.md + history.jsonl
bash scripts/blueprint_gate.sh     # one-shot green/red
JHOL_BIN=target/debug/jhol bash examples/blueprint/verify.sh
```

---

## Bottom line

Phase **B** has working graph/lock/discovery and JS/Python/Rust apply slices on this host. Phase **C** capabilities and local **D transport** have recorded local evidence.  
Phase **A** gates are green except **org transfer** (owner).  
Live remote/cloud services, org transfer, and full inference remain open; local mock tests do not close live-service gates.
