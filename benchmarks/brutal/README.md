# Brutal Benchmark — jhol vs bun vs npm vs pnpm

Independent-lab-style. 12 metrics. No tricks. No cherry-pick.

## Projects

- `bench-simple/` — 11 direct deps (express, uuid, lodash, etc.), ~90 total
- `bench-react/` — create-react-app shape (react, react-dom, react-scripts), ~1500 total
- `bench-next/` — Next.js project shape, ~2500 total
- `bench-monorepo/` — 3 workspaces each with 10 deps
- `bench-heavy/` — 1 dep with 1000 deps

## Metrics (12)

| # | Metric | Unit | How |
|---|--------|------|-----|
| 1 | Cold install time | ms (real) | wall clock, 3 runs, median |
| 2 | Warm install time | ms (real) | rm node_modules only, keep cache |
| 3 | Fingerprint/skip time | ms (real) | no changes, just validate |
| 4 | Peak RSS | KB | `/usr/bin/time -l` |
| 5 | Disk usage (project + cache) | KB | `du -sk` |
| 6 | CPU usage | % | total from time |
| 7 | Binary size | bytes | `stat -f%z` |
| 8 | Install reliability | % | 10x runs, success/10 |
| 9 | Offline install | success? | no network, use cache |
| 10 | Lockfile write time | ms | time to write package-lock.json |
| 11 | Network bytes | bytes | `nstat` diff (macOS) |
| 12 | Exit code correctness | bool | 0 success, non-0 failure |

## Usage

```bash
# All 5 projects, all tools
bash bench.sh

# Single project
bash bench.sh simple

# Custom
TOOLS="jhol bun" PROJECTS="simple next" bash bench.sh
```
