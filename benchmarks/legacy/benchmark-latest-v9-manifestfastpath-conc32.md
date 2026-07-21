# Jhol benchmark summary

Packages: `lodash, axios, chalk`

| Metric | Average (s) | Median (s) | Runs |
|---|---:|---:|---|
| jhol_cold_install | 0.855 | 0.525 | 2.129, 0.525, 0.505, 0.488, 0.628 |
| jhol_warm_install | 0.016 | 0.016 | 0.016, 0.016, 0.016, 0.016, 0.016 |
| jhol_offline_install | 0.013 | 0.014 | 0.012, 0.012, 0.014, 0.015, 0.014 |
| npm_cold_install | 2.317 | 2.431 | 1.561, 2.708, 2.539, 2.431, 2.345 |
| npm_warm_install | 2.081 | 2.142 | 2.335, 2.261, 1.964, 2.142, 1.702 |
| yarn_cold_install | 2.141 | 2.214 | 2.214, 1.814, 1.836, 2.421, 2.422 |
| yarn_warm_install | 1.221 | 1.060 | 1.901, 1.112, 1.007, 1.060, 1.023 |
| pnpm_cold_install | 1.796 | 1.706 | 2.161, 1.675, 1.706, 1.815, 1.625 |
| pnpm_warm_install | 1.526 | 1.488 | 1.403, 1.755, 1.488, 1.531, 1.452 |
| bun_cold_install | 0.108 | 0.041 | 0.376, 0.035, 0.041, 0.039, 0.047 |
| bun_warm_install | 0.047 | 0.046 | 0.046, 0.052, 0.044, 0.045, 0.048 |
