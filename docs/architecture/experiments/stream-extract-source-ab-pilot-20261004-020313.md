# Source staged versus streamed extraction pilot

- Condition: registry cold; fresh projects and per-run Jhol cache; locked React/Vite fixture; harvest disabled; host/registry caches uncontrolled.
- Source release SHA-256: `fba6910a0ce8e3bfe986bf20e44186dcc0faaa06de7679af4bcbec79873f13d2`.
- Five randomized interleaved rounds per mode. All 10 passed route and scoped quality checks.

| Mode | Median | p95 | Median fetch/extract stage |
|---|---:|---:|---:|
| Staged | 918.94 ms | 1,171.46 ms | 868 ms |
| Streamed | 950.58 ms | 1,645.53 ms | 858 ms |

Streaming was 3.4% slower by median. Fetch/extract time moved only 10 ms, while staged mode releases its bounded download permit before extraction and can overlap work for different packages. Keep streaming opt-in. Pilot size and uncontrolled registry/OS caches do not support a performance claim. Detailed samples, fixture inputs, hashes, quality checks, and runner source are in the adjacent JSON file.
