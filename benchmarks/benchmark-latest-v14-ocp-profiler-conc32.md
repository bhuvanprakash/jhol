# Jhol benchmark summary

Packages: `lodash, axios, chalk`

| Metric | Average (s) | Median (s) | Runs |
|---|---:|---:|---|
| jhol_cold_install | 1.125 | 0.849 | 2.148, 0.849, 0.839, 0.652, 1.135 |
| jhol_warm_install | 0.025 | 0.025 | 0.023, 0.025, 0.029, 0.026, 0.024 |
| jhol_offline_install | 0.031 | 0.030 | 0.030, 0.032, 0.026, 0.030, 0.035 |
| npm_cold_install | 2.705 | 2.729 | 3.438, 2.319, 2.085, 2.955, 2.729 |
| npm_warm_install | 2.222 | 2.386 | 2.386, 2.577, 1.613, 2.135, 2.398 |
| yarn_cold_install | 2.275 | 2.323 | 2.382, 2.381, 2.281, 2.323, 2.009 |
| yarn_warm_install | 1.479 | 1.355 | 1.955, 1.355, 1.227, 1.314, 1.543 |
| pnpm_cold_install | 5.406 | 4.717 | 8.667, 4.060, 4.717, 2.443, 7.143 |
| pnpm_warm_install | 2.263 | 2.019 | 1.988, 3.156, 2.019, 1.961, 2.190 |
| bun_cold_install | 0.190 | 0.070 | 0.679, 0.070, 0.064, 0.074, 0.065 |
| bun_warm_install | 0.064 | 0.066 | 0.066, 0.053, 0.060, 0.070, 0.069 |
