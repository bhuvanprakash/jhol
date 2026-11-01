# Jhol benchmark summary

Suite: `small`
Repeats: `3`

## Workload: `small`

Packages: `lodash, axios, chalk`

| Metric | Average (s) | Median (s) | MAD (s) | P95 (s) | Outliers | Runs |
|---|---:|---:|---:|---:|---:|---|
| jhol_cold_install | 0.708 | 0.431 | 0.010 | 1.189 | 0 | 1.273, 0.420, 0.431 |
| jhol_offline_install | 0.074 | 0.074 | 0.001 | 0.075 | 0 | 0.075, 0.073, 0.074 |
| jhol_warm_install | 0.093 | 0.099 | 0.007 | 0.106 | 0 | 0.106, 0.073, 0.099 |

