# Jhol benchmark summary

Suite: `medium`
Repeats: `5`

## Workload: `medium`

Packages: `react, next, typescript, express`

| Metric | Average (s) | Median (s) | MAD (s) | P95 (s) | Outliers | Runs |
|---|---:|---:|---:|---:|---:|---|
| jhol_cold_install | 6.073 | 5.995 | 0.024 | 6.327 | 1 | 6.404, 6.019, 5.964, 5.983, 5.995 |
| jhol_offline_install | 0.097 | 0.107 | 0.004 | 0.111 | 0 | 0.107, 0.083, 0.111, 0.109, 0.076 |
| jhol_warm_install | 0.175 | 0.184 | 0.072 | 0.259 | 0 | 0.231, 0.184, 0.266, 0.081, 0.112 |

