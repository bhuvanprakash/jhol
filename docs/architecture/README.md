# Architecture docs

- [Organization bootstrap seed](../../organization/README.md)
  — includes the future `.github` repository-root seed under `organization/github-repo/`.
- [JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md](./JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md)
  — organization, graph/adapters, phases A–D, acceptance gates.
- [PHASES_A_TO_D_COMPLETED.md](./PHASES_A_TO_D_COMPLETED.md)
  — what is done vs owner/deferred for Phases A–D.
- [PHASE_A_CHECKPOINT.md](./PHASE_A_CHECKPOINT.md)
  — evidence snapshot (test counts, gate status).
- [blueprint-records/](./blueprint-records/)
  — append-only PASS/FAIL log (`history.jsonl`, `LATEST.md`). Re-run:
  `bash scripts/blueprint_record.sh`.

Schemas live in `/schemas`. Foundation crates (workspace members):

| Crate | Role |
|---|---|
| `jhol-graph` | Project graph + `jhol.project.lock` digests |
| `jhol-adapters` | JS / Python / Cargo discovery, planning, and guarded apply paths |
| `jhol-receipt` | Snapshots + execution receipts |
| `jhol-policy` | Capability gates (sync --apply, MCP) |
| `jhol-task` | Task / service topo + spawn cleanup |
| `jhol-env` | Host toolchain probe |
| `jhol-diff` | Receipt diff |
| `jhol-ai` | Model-file pin / digest verify / offline prepare; no inference runtime |
| `jhol-db` | Migration plan + optional TCP probe |
| `jhol-remote` | Optional S3-compatible CAS client (not default CLI dep; live provider unverified) |
