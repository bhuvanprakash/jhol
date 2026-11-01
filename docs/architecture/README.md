# Architecture docs

- [Organization bootstrap seed](../../organization/README.md)
  — includes the future `.github` repository-root seed under `organization/github-repo/`.
- [JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md](./JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md)
  — organization, graph/adapters, phases A–D, acceptance gates.
- [PHASES_A_TO_D_COMPLETED.md](./PHASES_A_TO_D_COMPLETED.md)
  — what is done vs owner/deferred for Phases A–D.
- [PHASE_A_CHECKPOINT.md](./PHASE_A_CHECKPOINT.md)
  — evidence snapshot (test counts, gate status).
- [FOUNDATION_CRATES.md](./FOUNDATION_CRATES.md)
  — current foundation-crate APIs, invariants, tests, and limits.
- [blueprint-records/](./blueprint-records/)
  — append-only PASS/FAIL log (`history.jsonl`, `LATEST.md`). Re-run:
  `bash scripts/blueprint_record.sh`.

Schemas live in `/schemas`. Foundation crates (workspace members):

| Crate | Role |
|---|---|
| `jhol-graph` | Project graph + `jhol.project.lock` digests |
| `jhol-adapters` | JS / Python / Cargo discovery, planning, and guarded apply paths |
| `jhol-receipt` | Snapshots + execution receipts |
| `jhol-policy` | Scoped capability grants, profiles, deny-by-default decisions |
| `jhol-task` | Task / service topo + spawn cleanup |
| `jhol-env` | Host tools, versions, project pins, secret-safe child env |
| `jhol-diff` | Receipt, graph, project-lock, and project-state diffs |
| `jhol-ai` | Model-file pin / digest verify / offline prepare; no inference runtime |
| `jhol-db` | Migration plan, drift ledger, SQL risk scan, redacted TCP probe |
| `jhol-cloud` | Offline tenant/token/quota/routing control-plane library; no server |
| `jhol-remote` | Optional S3-compatible CAS client (not default CLI dep; live provider unverified) |
