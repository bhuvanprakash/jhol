# Jhol benchmark summary

Packages: `lodash, axios, chalk`

| Metric | Average (s) | Median (s) | Runs |
|---|---:|---:|---|
| jhol_cold_install | 0.841 | 0.851 | 0.851, 0.819, 0.817, 0.859, 0.857 |
| jhol_warm_install | 0.022 | 0.021 | 0.021, 0.017, 0.021, 0.024, 0.024 |
| jhol_offline_install | 0.021 | 0.019 | 0.024, 0.018, 0.026, 0.019, 0.019 |
| npm_cold_install | 2.318 | 2.428 | 2.428, 2.458, 1.874, 2.318, 2.513 |
| npm_warm_install | 2.433 | 2.018 | 1.997, 3.907, 2.524, 2.018, 1.721 |
| yarn_cold_install | 2.099 | 1.992 | 1.970, 2.268, 2.640, 1.624, 1.992 |
| yarn_warm_install | 1.161 | 1.119 | 1.043, 1.119, 1.584, 1.193, 0.865 |
| pnpm_cold_install | 2.001 | 1.910 | 1.910, 1.868, 2.167, 2.192, 1.867 |
| pnpm_warm_install | 1.446 | 1.411 | 1.484, 1.386, 1.411, 1.310, 1.639 |
| bun_cold_install | 0.061 | 0.044 | 0.124, 0.044, 0.039, 0.044, 0.052 |
| bun_warm_install | 0.043 | 0.043 | 0.041, 0.043, 0.043, 0.041, 0.046 |
