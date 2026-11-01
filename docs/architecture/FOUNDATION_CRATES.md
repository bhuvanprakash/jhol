# Foundation crates — current implementation

Status snapshot: 2026-10-24. These crates are small contracts, not a second
package manager. The CLI consumes most of them; `jhol-cloud` and `jhol-remote`
are workspace members with no root dependency, and `jhol-cloud` has no internal
imports at all.

| Crate | Role | Targeted tests |
|---|---|---:|
| `jhol-graph` | Typed cross-ecosystem graph and `jhol.project.lock` digests | 8 |
| `jhol-adapters` | JS, Python/uv, and Rust/Cargo discovery, plans, guarded apply | 6 |
| `jhol-policy` | Scoped capability grants, deny-by-default decisions | 10 |
| `jhol-receipt` | Execution receipts and guarded snapshots | 5 |
| `jhol-task` | Task/service ordering, readiness, child cleanup | 8 |
| `jhol-env` | Host tools, versions, pins, secret-safe child environments | 8 |
| `jhol-diff` | Receipt, graph, project-lock, and project-state diffs | 13 |
| `jhol-ai` | Content-addressed model/dataset file pins | 8 |
| `jhol-db` | Migration planning, drift ledger, SQL risk scan, URL redaction | 10 |
| `jhol-cloud` | Offline control-plane domain model and protocol state machine | 12 |
| `jhol-remote` | Optional S3-compatible CAS transport | 9 |

All 97 counts in the table were re-run on 2026-10-24 in two scoped batches,
with 0 failures: the five rebuilt crates plus policy consumers (67), and
`jhol-graph`, `jhol-ai`, `jhol-receipt`, `jhol-remote` (30). This is not a full
workspace re-baseline.

## Shared invariants

- No `unsafe` code (`#![forbid(unsafe_code)]` in every crate).
- No hidden service: environment, database, and cloud crates do not open network
  connections as part of discovery or planning.
- Deny by default. A plan is not consent; side effects need an explicit grant.
- Deterministic serialization. New fields are additive and `serde(default)` so
  older JSON documents still parse.
- Content addressing is SHA-256 hex over bytes, never over re-serialized objects,
  unless the function name says otherwise.
- Secrets are redacted before a report or URL leaves the crate.

## `jhol-env`

`find_tool` resolves an executable on `PATH` without a shell. `probe_tool` runs
only the version command and records path, raw output, and a parsed version.
`probe_host` probes the default set (`node`, `npm`, `bun`, `python3`, `uv`,
`cargo`, `rustc`).

`Version` parses dotted runs from banners such as `v20.11.0`,
`Python 3.12.1`, and `rustc 1.80.0 (hash)`. `VersionConstraint` supports
`*`, `20.x`, `20.10`, `>=`, `>`, `<=`, `<`, `=`, `^`, `~`, and AND clauses.
`check_requirements` reports required vs optional misses without panicking.

`capture_env` redacts secret-looking names. `child_env` returns a sorted,
minimal environment containing `PATH` and safe base variables plus explicit
allow-list entries. `read_version_pins` reads `.nvmrc`, `.node-version`,
`.python-version`, `rust-toolchain(.toml)`, and `package.json` `engines`.
`check_version_pins` compares those pins with installed tools.

Limit: discovery is observational. It does not install, upgrade, or isolate a
toolchain.

## `jhol-policy`

`Capability` enumerates the eight side-effect classes. `Rule` narrows a
capability to a resource with `*`/`?` glob matching. `Policy` keeps session
grants, explicit denies, and rules. Deny wins; the most specific matching rule
wins; a scoped allow rule narrows an otherwise global grant.

Profiles: `deny_all`, `read_only`, `local_development`, `ci`, and `untrusted`.
`explain` returns the decision and matched rule without performing work.
`check_scoped` gates a concrete path, registry host, or command. `merge` unions
grants while preserving denies. Policies round-trip through JSON, including the
legacy `{"allowed":[...]}` shape.

Limits: this is an authorization gate, not an OS sandbox. Approved child tools
still run with the invoking user's permissions.

## `jhol-diff`

`diff_receipts` compares inputs, outputs, plan digest, status, and side effects.
It now reports removed inputs and outputs instead of silently ignoring them.
`ChangeClass` separates `Unchanged`, cacheable classes, `RequiresWork`,
`Blocked`, `PolicyDenied`, `Added`, and `Removed`. Added/removed entries are
work.

`diff_graphs` compares typed graph nodes and edges. `diff_project_locks`
compares config digest, native lock digests, adapters, and permitted outputs.
`diff_state` compares manifest/lock text and installed-tree presence, with
package-level changes from npm lockfile v1/v2/v3. `lock_package_versions` never
panics on invalid JSON; callers get a structured `StateReport` with notes.

Limit: this is not a full incremental-build engine. Graph analysis is structural;
it does not replay actions.

## `jhol-db`

`plan_migrations` discovers `*.sql`, hashes bytes, and orders files by numeric
prefix (`2` before `10`), with duplicate-order notes. `write_ledger` records a
reviewed order at `.jhol/db-migrations.json`; `verify_migrations` reports added,
removed, modified, and reordered migrations. A missing ledger is reported as
drift, never as clean.

`analyze_migration` / `scan_migrations` strip comments and string literals, count
statements, and flag destructive statements (`DROP`, `TRUNCATE`, `DELETE` or
`UPDATE` without `WHERE`, `ALTER ... DROP`) and non-transactional statements
(`CREATE INDEX CONCURRENTLY`, `VACUUM`, `CREATE DATABASE`, `ALTER TYPE ... ADD
VALUE`).

`parse_database_url` understands PostgreSQL, MySQL/MariaDB, Redis, SQLite, and
bare `host:port`. `redact_database_url` removes passwords and secret query
parameters. `probe_database_url` performs a TCP connect only; SQLite is checked
as a file target and never opened. No SQL is executed anywhere in this crate.

Limit: no migration apply, rollback, or schema-state comparison exists.

## `jhol-cloud`

`ControlPlane` is an in-memory, library-only model for a future hosted service:

- `Tenant` with `PlanTier`, quotas, status, labels, and creation time.
- `ApiToken` with scoped grants (`RemoteRead`, `RemoteWrite`, `UsageRead`,
  `Admin`), expiry, revocation, SHA-256 digest, and constant-time verification.
  The plaintext token is returned exactly once.
- `UsageLedger` with tenant totals, trailing-minute request quotas, trailing-day
  upload quotas, and idempotency keys.
- `StorageRouter` producing deterministic tenant/digest-scoped CAS keys.
- Bounded audit trail and a `Request`/`Response` state machine for
  `/v1/health`, `/v1/tenants`, `/v1/tokens`, `/v1/auth/check`, `/v1/usage`, and
  `/v1/route`.

Security limits (documented in the crate): token material uses OS-seeded
`RandomState` plus time and a counter, which is adequate for local tests but must
be replaced with a CSPRNG in a deployed service. Transport security, persistence,
live authorization, and provider integration are out of scope. The crate has no
`jhol-*` dependencies by design.

## Verification

```sh
CARGO_BUILD_JOBS=2 cargo test -p jhol-env -p jhol-policy -p jhol-diff \
  -p jhol-db -p jhol-cloud -p jhol-adapters -p jhol-task
CARGO_BUILD_JOBS=2 cargo test -p jhol-graph -p jhol-ai -p jhol-receipt \
  -p jhol-remote
CARGO_BUILD_JOBS=2 cargo check -p jhol --no-default-features
```

The root check validates CLI consumers of `jhol-env`, `jhol-diff`, and
`jhol-db` without paying for the default runtime/bundler feature build.
