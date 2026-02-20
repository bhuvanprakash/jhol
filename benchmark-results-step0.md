# Jhol benchmark summary

Suite: `small`
Repeats: `5`

## Workload: `small`

Packages: `lodash, axios, chalk`

| Metric | Average (s) | Median (s) | MAD (s) | P95 (s) | Outliers | Runs |
|---|---:|---:|---:|---:|---:|---|
| jhol_cold_install | 0.815 | 0.749 | 0.018 | 1.062 | 1 | 1.136, 0.696, 0.749, 0.765, 0.731 |
| jhol_offline_install | 0.015 | 0.015 | 0.001 | 0.017 | 0 | 0.015, 0.014, 0.016, 0.017, 0.014 |
| jhol_warm_install | 0.016 | 0.016 | 0.001 | 0.017 | 0 | 0.015, 0.017, 0.015, 0.016, 0.016 |

