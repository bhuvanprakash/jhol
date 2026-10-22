# Jhol benchmarks

Independent home for reproducible Jhol performance and compatibility evidence.

## Rules for a result

- Pin Jhol, fixture inputs, and competitor versions.
- Record OS, CPU, memory, network conditions, and cache state.
- Separate no-op, warm cache, offline, and fresh registry workloads.
- Check installed output correctness before recording a timing.
- Keep failed runs and raw measurements; publish method and exclusions with summaries.
- Do not turn one machine or workload into a universal speed claim.

Benchmark tools may compare public package managers for interoperability and
performance research. Their identity, implementation, and presentation remain
their own; this repository reports measured behavior, not borrowed product
claims.
