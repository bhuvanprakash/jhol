# Jhol Cloud control-plane design (proposal)

**Status:** design only. No hosted Jhol Cloud service, tenant database, token
issuer, or live object-store integration exists in this checkout.

## Purpose

Jhol Cloud is an optional service for verified remote content and, later,
action-cache metadata. The local package manager and project workflows remain
usable without an account or cloud connection. The control plane authorizes
requests and accounts for use; object storage holds immutable bytes. A matching
digest or action key is never authorization to read another tenant's data.

## First service boundary

```text
Jhol client
   |
   | TLS + tenant credential; digest and declared size
   v
API edge -> tenant authentication -> authorization + atomic quota reservation
                                        |                     |
                                        v                     v
                                usage event ledger       tenant metadata
                                        |
                                        v
                         private staging object storage
                                        |
                    streamed size + SHA-256 verification
                                        |
                     publish tenant-scoped CAS object
```

The client-supplied tenant identifier is a selector only. The authenticated
credential determines the tenant; mismatches fail closed. Object keys include
the server-derived tenant namespace and the validated lowercase SHA-256 digest.
The service verifies the payload while receiving it and before publishing it.

## Proposed API responsibilities

The exact paths and wire fields still need protocol review. The first version
should expose only these operations:

- **Limits:** return the authenticated tenant's current byte and request limits
  plus the accounting window. Do not expose other tenants or internal billing
  records.
- **Content upload:** reserve quota before accepting bytes, enforce declared and
  observed size limits, stream to private staging, verify SHA-256, then publish
  atomically. Repeated uploads of the same tenant/digest are idempotent.
- **Content fetch:** require tenant authorization for every request, enforce
  egress limits, and verify stored object size/digest before returning it.
- **Usage:** account uploaded bytes, downloaded bytes, stored bytes, and request
  counts from server observations. Retries use an idempotency key so a client
  retry does not double-charge one completed operation.
- **Diagnostics:** separate unauthenticated liveness from readiness. Detailed
  tenant diagnostics require authentication and redact tokens, URLs, object
  keys, and customer content.

Action-cache metadata is a later protocol slice. A cache record must bind the
tenant, action key, declared input digests, tool identity, output digests, and
policy result. Secrets, database mutations, nondeterministic work, and actions
with undeclared inputs are not cacheable by default.

## Authentication and tenant isolation

- Use revocable tenant-scoped credentials with a key identifier. Store a
  verifier, not the plaintext secret; support rotation and revocation.
- Derive the tenant from the authenticated principal. Never trust a tenant ID
  from a path, query string, object header, or action key by itself.
- Apply authorization at every content and metadata read/write boundary,
  including internal object-store calls and background cleanup jobs.
- Return a non-enumerating not-found response for an object outside the caller's
  tenant. Keep authorization failures free of object-existence details.
- Redact credentials and signed headers from logs, traces, metrics, URLs, and
  error reports. Do not put them in receipts or cache keys.

## Quota and usage consistency

Uploads reserve the maximum authorized byte count atomically before streaming.
The reservation is finalized from observed bytes after digest verification and
released on failure or expiry. Concurrent uploads cannot each pass a quota
check against the same stale counter. Egress reservations are checked before
returning content. Retries and idempotency records must make accounting
repeatable across disconnects.

Usage records are append-only operational evidence. Corrected counters should
be represented by compensating entries rather than rewriting the event trail.
Retention periods, tenant deletion, and any billing conversion remain explicit
product decisions; the service must not infer billing policy from byte counts.

## Failure behavior

- Reject malformed tenant credentials, digest paths, content lengths, oversized
  objects, quota exhaustion, and digest mismatches before publication.
- Do not convert storage/authentication outages into cache misses that claim a
  successful remote lookup; return a typed retryable or permanent error.
- Bound upload/download duration, body size, retry count, and response memory.
- On an interrupted upload, discard or expire uncommitted staging objects and
  release quota reservations.
- Keep local cache contents unchanged when a remote fetch fails validation.

## Deployment boundaries

The first control plane should not execute customer tasks. Remote execution
requires a separate worker design covering process/container isolation,
network egress, secrets, cancellation, resource limits, artifact exchange, and
cross-tenant tests. It remains out of scope for the initial cache service.

Provider choice, service visibility, credential enrollment, deployment region,
retention, quota tiers, and billing behavior are open owner decisions. The local
`jhol-remote` client and `schemas/remote-cas-v1.schema.json` do not prove that
this control plane or any live AWS S3/Arenvara deployment exists.

## Acceptance gates before a hosted deployment

1. Authenticate and revoke credentials; verify tenant identity cannot be
   overridden by request data.
2. Prove cross-tenant GET/HEAD, upload, metadata lookup, and cleanup all fail
   closed, including guessed digest/action keys.
3. Race concurrent uploads at quota boundaries and verify exactly-once usage
   accounting across retries and disconnects.
4. Inject 429s, timeouts, mid-body disconnects, corrupt stored objects, and
   database/object-store outages; prove no poisoned object is published.
5. Verify tenant deletion, retention cleanup, and usage history without
   touching another tenant's objects or records.
6. Run provider compatibility tests against isolated AWS and Arenvara test
   accounts before advertising either backend.
7. Keep remote task execution disabled until its separate isolation review
   passes.
