# Optional remote CAS client

`jhol-remote` provides an S3-compatible content-addressed transport. It is not
a default CLI dependency; local installs continue to use local stores only.
The client uses path-style objects:

```text
<endpoint>/<bucket>/tenants/<tenant>/sha256/<lowercase-sha256>
```

Uploads verify the supplied digest before sending. Downloads have a 512 MiB
limit, check any declared size, and verify SHA-256 before returning bytes. A
caller must publish returned data to its local cache only after `fetch` returns
successfully. Tenant path prefixes aid routing; service authorization must
enforce tenant isolation independently.

## Configuration

| Variable | Purpose |
|---|---|
| `JHOL_REMOTE_CAS_URL` | S3-compatible endpoint; HTTPS required, except HTTP loopback for local development. |
| `JHOL_REMOTE_TENANT` | Tenant namespace; restricted to letters, digits, `.`, `_`, and `-`. |
| `JHOL_REMOTE_S3_BUCKET` | Bucket name. |
| `JHOL_REMOTE_S3_REGION` | SigV4 region. |
| `JHOL_REMOTE_S3_ACCESS_KEY_ID` | Access key ID. |
| `JHOL_REMOTE_S3_SECRET_ACCESS_KEY` | Secret access key. |
| `JHOL_REMOTE_S3_SESSION_TOKEN` | Optional temporary credential token. |

Credentials stay in environment-backed client fields. Do not put secrets in the
endpoint URL, wire objects, receipts, or logs. Redirects are disabled so signed
authorization headers cannot be forwarded to another host. Requests use a
30-second timeout and bounded retries for throttling, server errors, and
transport failures, including interrupted response bodies. Large-object multipart transfer and live Arenvara/provider
verification remain unimplemented.
