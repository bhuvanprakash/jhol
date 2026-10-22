# Jhol Cloud

Private, independently deployed service for optional Jhol remote-cache
capabilities. Local Jhol workflows must remain usable without an account or
cloud connection.

Initial scope is authenticated remote content and action-cache metadata over a
versioned protocol. Remote task execution is out of scope until a separate
worker isolation and trust design passes review.

## Required boundaries

- Verify content digests after transport; reject mismatches before cache publish.
- Isolate authorization, quotas, metadata, and object access by tenant.
- Never treat a matching action key alone as permission to reuse another
  tenant's result.
- Keep credentials out of receipts, logs, URLs, and cache keys.
- Make upload, retention, billing, and execution distinct user-visible actions.
- Exercise throttling, disconnects, retries, and cache-poisoning defenses before
  production use.

The monorepo's `schemas/remote-cas-v1.schema.json` is the initial wire-contract
reference. This README records intended boundaries; it does not claim a cloud
service or live S3/Arenvara backend exists.

See [`CONTROL_PLANE_DESIGN.md`](CONTROL_PLANE_DESIGN.md) for the proposed
tenant, quota, usage, and diagnostics boundary and its pre-deployment gates.
