# Phase A–D checkpoint (evidence only — not a release claim)

Recorded: 30 Sep 2026, darwin-aarch64.

**Source state (read-only inventory):** base `main` commit `5bfc48db`; working
tree contains existing tracked edits and untracked files. Rust `1.98.1`
(`aarch64-apple-darwin`), Cargo `1.98.1`; root default features are
`runtime`, `bundle`, and `http-reqwest`. No Git write operation was performed.

**Local gate (re-run):** `scripts/blueprint_gate.sh` → `blueprint_gate: OK`

**Full workspace snapshot (2026-09-30):** `cargo test --workspace` →
**645 passed / 0 failed / 12 ignored**. This aggregate predates the latest
adapter follow-up. Subsequent focused checks passed 6 adapter tests and the
real-dependency JS/Python/Rust smoke fixture on macOS arm64. Existing compiler
warnings remain; neither result verifies optional features or other platforms.

## Objective coverage (agent-implementable)

| Objective item | Evidence |
|---|---|
| Phase A preserve/consolidate gates | Checkboxes §18 A (except org transfer); clippy `-D warnings` on foundation; compare_versions harness; layout/audit tests |
| Phase B graph/adapters/plan/diff/sync/project.lock/action-cache | Crates + CLI wired; verify.sh exercise |
| Phase C snapshot/receipt, local and remote AI artifact pin/prepare/infer, DB plan, declared service spawn, polyglot demo | Crates + `jhol snapshot/restore/ai/db/dev`; examples/blueprint; bounded public Hugging Face fetch and local Jhol CPU inference |
| Phase D remote CAS client | `jhol-remote` + `schemas/remote-cas-v1.schema.json`; local transport tests pass; live providers and cloud control plane remain open |
| Earlier foundation-gate snapshot | core lib 460/0/2; workspace lib+tests **633/0/11** EXIT 0; superseded by the later snapshots above and below |
| Latest focused follow-up | core lib 462/0/2; adapter lib 6/0/0; online third-party fixture passed on macOS arm64 |
| examples/blueprint verify | pass (in blueprint_gate) |
| CLI help ↔ dispatch | plan/diff/sync/snapshot/restore/db/ai/dev |
| CHANGELOG | `[Unreleased]` Added Phases A–D |
| Workspace shape / no commit | 9 Phase B/C foundation crates plus optional `jhol-remote`; 14 workspace members including root, core, runtime, and bundle; working tree uncommitted |

## Live GitHub state checked read-only (2026-09-30)

`gh auth status` showed the signed-in account `bhuvanprakash`; `gh api`
confirmed `bhuvanprakash/jhol` is private, on `main`, and the account has admin
permission. The local `origin` remains `https://github.com/bhuvanprakash/jhol`.
No organization handle has been chosen; public profile 404s for suggested
handles are not proof they can be used during organization creation. No Git
write operation or remote change was performed.

## Open work in the full blueprint objective

1. GitHub organization creation and remote-repository transfer; the local `.github` repository-root seed and four repository README/contribution/security drafts are prepared.
2. Cross-platform hosted runs for the online adapter smoke check; the Linux/macOS/Windows matrix is configured but has not produced results here.
3. Live AWS S3/Arenvara verification, the `jhol-cloud` control plane, and remote task execution design/implementation.

## Declared-service follow-up (2026-09-30)

The `jhol dev` example now reads only explicit `[[service]]` commands from the
schema-1 `jhol.toml`; it does not guess PostgreSQL, Python, or Node commands.
`--spawn` is required to launch them. Commands remain argument arrays, service
working directories must resolve inside the project, and process cleanup is
verified after the sample hold period.

- `cargo test -p jhol-task`: **8 passed / 0 failed**.
- `cargo build --bin jhol`: passed; existing compiler warnings remain.
- `jhol dev --path examples/blueprint --json`: planned `db`, `api`, `web` in
  dependency order without launching processes.
- `jhol dev --path examples/blueprint --spawn --hold-secs 1 --json`: spawned
  the declared sample commands and exited successfully; all three readiness
  ports were free after cleanup.
- An empty project without `jhol.toml` failed closed and launched no process.
- `git diff --check` and TOML/JSON parsing passed.

The full `scripts/blueprint_gate.sh` was not rerun for this follow-up because
its example verifier updates persistent fixture artifacts; the focused checks
above exercised the changed service behavior without rewriting those files.
The `organization/repositories/jhol-cloud/CONTROL_PLANE_DESIGN.md` file is a
design proposal only; it does not implement or deploy a cloud service.


## Model artifact acquisition follow-up (2026-09-30)

`jhol ai pin-remote` writes a schema-1 manifest for public Hugging Face files
using an exact commit revision and caller-supplied SHA-256 and byte sizes; it
performs no network access. `jhol ai prepare` remains offline by default. With
`--allow-network`, it streams and resumes into bounded partial files, enforces
the configured byte ceiling, checks exact size and SHA-256 before publishing,
and reports cleaned partial paths and downloaded bytes. Interrupted downloads
remain resumable. No live Hugging Face request or model inference was claimed.

- `cargo test -p jhol-ai`: **4 passed / 0 failed**.
- `cargo check -p jhol-core --features http-reqwest -j 1`: passed.
- Bounded downloader tests with `http-reqwest`: **2 passed / 0 failed**.
- `cargo check --bin jhol -j 1`: passed.
- Release CLI smoke flow: local pin/verify, remote manifest creation, offline
  refusal, and byte-cap refusal passed.
- These focused results do not establish live-provider availability, private
  model support, or inference reproducibility.


## Local inference follow-up (2026-09-30)

The polyglot example now pins a Jhol-owned two-feature linear classifier and
runs it through `jhol-linear-cpu-v1` using `jhol ai infer`. The runtime verifies
and retains the exact manifest-pinned bytes before inference, executes no model
code, and enforces a 256 KiB artifact and manifest, 16 KiB config, 64-feature and
16-class limit. Its `JHLIN001` weights are a versioned little-endian linear format. The
example verifier runs the same input twice and checks the same `large` label.
This establishes only the checked-in linear model/runtime pair, not transformer
execution, accelerator support, or broad model compatibility.

- `cargo test -p jhol-ai -j 1`: **7 passed / 0 failed**.
- `cargo build --bin jhol -j 1`: passed; existing workspace warnings remain.
- CLI smoke: same input produced the same `large` prediction twice; JSON model/runtime identity and probability sum passed; wrong dimensions and 65-feature input failed closed.
- `bash -n` for the example and blueprint gate scripts, model manifest digest/size checks, schema JSON parse, and `git diff --check`: passed.
- The full example verifier was not rerun because it applies sync to the persistent example tree; the new inference command was run directly against the built CLI.
