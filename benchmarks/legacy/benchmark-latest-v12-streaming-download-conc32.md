# Jhol benchmark summary

Packages: `lodash, axios, chalk`

| Metric | Average (s) | Median (s) | Runs |
|---|---:|---:|---|
| jhol_cold_install | 1.005 | 0.777 | 1.944, 0.777, 0.775, 0.747, 0.780 |
| jhol_warm_install | 0.018 | 0.018 | 0.022, 0.016, 0.018, 0.020, 0.016 |
| jhol_offline_install | 0.020 | 0.020 | 0.017, 0.019, 0.020, 0.023, 0.021 |
| npm_cold_install | 2.040 | 2.063 | 2.279, 1.667, 1.818, 2.063, 2.376 |
| npm_warm_install | 1.672 | 1.616 | 1.539, 1.554, 1.664, 1.989, 1.616 |
| yarn_cold_install | 1.867 | 1.792 | 2.165, 1.923, 1.792, 1.772, 1.683 |
| yarn_warm_install | 0.991 | 0.997 | 0.997, 1.023, 0.975, 1.009, 0.951 |
| pnpm_cold_install | 2.309 | 2.361 | 2.564, 2.157, 2.494, 1.970, 2.361 |
| pnpm_warm_install | 1.505 | 1.470 | 1.663, 1.517, 1.470, 1.465, 1.409 |
| bun_cold_install | 0.055 | 0.042 | 0.110, 0.040, 0.044, 0.037, 0.042 |
| bun_warm_install | 0.044 | 0.042 | 0.041, 0.053, 0.042, 0.040, 0.045 |
