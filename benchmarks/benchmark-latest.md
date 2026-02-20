# Jhol benchmark summary

Packages: `lodash, axios, chalk`

| Metric | Average (s) | Median (s) | Runs |
|---|---:|---:|---|
| jhol_cold_install | 0.798 | 0.723 | 1.123, 0.723, 0.749, 0.689, 0.704 |
| jhol_warm_install | 0.016 | 0.017 | 0.014, 0.017, 0.017, 0.016, 0.017 |
| jhol_offline_install | 0.016 | 0.017 | 0.017, 0.017, 0.017, 0.015, 0.015 |
| npm_cold_install | 1.838 | 1.629 | 3.443, 1.319, 1.630, 1.629, 1.169 |
| npm_warm_install | 1.336 | 1.299 | 1.299, 1.231, 1.519, 1.096, 1.534 |
| yarn_cold_install | 1.869 | 1.811 | 2.300, 1.811, 1.661, 1.720, 1.856 |
| yarn_warm_install | 0.932 | 0.904 | 0.833, 1.164, 0.909, 0.852, 0.904 |
| pnpm_cold_install | 2.089 | 2.025 | 2.384, 2.132, 1.927, 2.025, 1.980 |
| pnpm_warm_install | 1.462 | 1.479 | 1.473, 1.518, 1.487, 1.354, 1.479 |
| bun_cold_install | 0.188 | 0.047 | 0.723, 0.039, 0.047, 0.037, 0.094 |
| bun_warm_install | 0.050 | 0.045 | 0.045, 0.044, 0.062, 0.044, 0.055 |
