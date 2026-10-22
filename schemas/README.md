# Schemas (v1)

JSON Schema drafts for project / lock / receipt / adapter protocol. Consumers
should treat `format_version` / `schema_version` as part of the identity.

| File | Purpose |
|---|---|
| `jhol-project-v1.schema.json` | Optional `jhol.toml` / project declaration |
| `graph-lock-v1.schema.json` | Cross-language lock that *references* native locks |
| `execution-receipt-v1.schema.json` | Portable plan/sync evidence |
| `adapter-protocol-v1.schema.json` | Adapter plan / apply / verify shapes |

Native `package-lock.json` / `uv.lock` / `Cargo.lock` remain authoritative.
