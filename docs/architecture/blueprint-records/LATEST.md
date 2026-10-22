# Blueprint record — 20260930T022752Z-Darwin-arm64 (BRUTAL)

- **status:** OK
- **host:** Darwin-arm64
- **utc:** 20260930T022752Z
- **suite:** `scripts/blueprint_record.sh` + extra stress (plan×2, sync×3, snapshot×3, tamper, clippy `-D warnings`, verify.sh, remote fail-closed)
- **totals (recorded checks):** pass=30 fail=0 skip=0 total=30
- **stress extras:** all expected behaviours confirmed (see below)
- **history:** `docs/architecture/blueprint-records/history.jsonl`

## Recorded checks (PASS/FAIL)

| Phase | Id | Status | Duration | Detail |
|-------|----|--------|----------|--------|
| A | `build_bin` | **PASS** | 15003ms | cargo build --bin jhol |
| A | `bin_exists` | **PASS** | 0ms | target/debug/jhol |
| A | `foundation_lib_tests` | **PASS** | 7553ms | foundation crate --lib tests |
| A | `core_lib_tests` | **PASS** | 2315ms | jhol-core --lib |
| A | `compare_versions` | **PASS** | 3724ms | npm/Bun/Jhol correctness harness |
| A | `cli_help_plan` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_help_diff` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_help_sync` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_help_snapshot` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_help_restore` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_help_db` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_help_ai` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_help_dev` | **PASS** | 0ms | listed in jhol --help |
| A | `cli_db_subcommands` | **PASS** | 0ms | db plan + probe in help |
| A | `cli_ai_subcommands` | **PASS** | 0ms | ai verify + prepare in help |
| B | `plan_json` | **PASS** | 1306ms | jhol plan --json shows javascript |
| B | `sync_project_lock` | **PASS** | 196ms | jhol sync writes jhol.project.lock |
| B | `project_lock_format` | **PASS** | 114ms | project.lock format_version 1 |
| C | `snapshot_create` | **PASS** | 238ms | snapshot create returns id |
| C | `restore_plan` | **PASS** | 324ms | restore --plan ok |
| C | `restore_databases_refused` | **PASS** | 298ms | restore --databases must fail |
| C | `db_plan` | **PASS** | 197ms | db plan finds 001_init |
| C | `db_probe` | **PASS** | 100ms | db probe json |
| C | `ai_verify` | **PASS** | 168ms | ai verify model pin |
| C | `ai_prepare` | **PASS** | 131ms | ai prepare offline |
| C | `dev_boot_order` | **PASS** | 94ms | dev --json boot_order |
| C | `dev_spawn_cleanup` | **PASS** | 2352ms | dev --spawn hold 1s |
| D | `remote_crate_tests` | **PASS** | 1887ms | jhol-remote --lib |
| D | `remote_schema` | **PASS** | 186ms | remote-cas-v1 schema file |
| D | `remote_not_in_cli` | **PASS** | 0ms | CLI does not link jhol-remote |

## Extra stress (brutal) — results

| Check | Expected | Observed | Verdict |
|-------|----------|----------|---------|
| `plan` twice → same `kind` | stable | `plan_stable True` | **PASS** |
| `sync` ×3 | exit 0 | `sync_x3:0` | **PASS** |
| `snapshot create` ×3 | 3 distinct ids | 3 UUIDs minted | **PASS** |
| `restore --apply --databases` | refuse (non-zero) | exit **1**; msg: `database restore is not supported; refuse rather than guess` | **PASS** (fail-closed) |
| AI pin tamper (`weights.bin` mutated) | verify fails | exit **1**; digest mismatch expected≠got | **PASS** (integrity) |
| AI pin restored | verify ok | `ai_restore_ok:0` | **PASS** |
| Action cache dir after sync | `.jhol/action-cache` exists | present + cache key file | **PASS** |
| `jhol.project.lock` | exists | `lock_ok` | **PASS** |
| Foundation `clippy -D warnings` | exit 0 | `CLIPPY_EXIT:0` | **PASS** |
| `examples/blueprint/verify.sh` | exit 0 | `verify: plan+lock+snapshot+db+ai+dev+spawn ok` | **PASS** |
| `jhol-remote` unit tests | 3 pass | tenant_mismatch / unconfigured / tampered payload | **PASS** |

### Stress error messages (proof of fail-closed)

```
restore --databases:
Error: restore apply: restore blocked: database restore is not supported; refuse rather than guess

ai verify (tampered weights):
Error: ai verify: digest mismatch for models/demo/weights.bin:
  expected bf6875a563be64dafa0c8e16f4b6093f55e15ba38f5c7a8844eaa61141dc805e,
  got     924c6e9b8777782b473ea17f32c5e6615c949a50d99756de04d48cc0c66f50d3
```

## How to re-run

```sh
bash scripts/blueprint_record.sh
# then optional stress block in agent/CI as needed
```
