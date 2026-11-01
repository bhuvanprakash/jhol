# Cold HTTP and projection diagnostic

Three randomized interleaved rounds per mode on the same locked React/Vite fixture. Fresh projects and per-run Jhol caches; harvest disabled; registry and OS caches uncontrolled. All six runs passed route and scoped identity/CJS/ESM/lock checks.

| Mode | Median install | Median summed TTFB | Median summed body read | Median project/consume work |
|---|---:|---:|---:|---:|
| Staged | 962.70 ms | 1,474 ms | 1,375 ms | 169 ms |
| Streamed | 927.88 ms | 1,540 ms | 1,307 ms | 1,725 ms |

Sixteen HTTP/2 requests occurred per run. Per-request durations overlap, so the sums are diagnostics rather than portions of wall time. Streamed consume duration includes reading the body while projection runs; staged projection duration begins after the body has been written to a private archive. Do not compare these two projection sums as equivalent standalone CPU costs.

This three-round result weakly favors streaming by median but conflicts with the five-round pilot (which favored staged and showed much worse streamed p95). No performance conclusion is justified. `JHOL_STREAM_EXTRACT=1` remains opt-in. The measurement only covers this fixture and host.
