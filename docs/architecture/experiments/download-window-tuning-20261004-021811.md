# Tarball download-window tuning

Thirty randomized rounds compared `JHOL_DOWNLOAD_WINDOW=4` (current default) with `=8` on the same locked React/Vite fixture. Each run used a fresh project and Jhol cache, disabled harvest, and verified route plus install quality. Registry/OS caches remained uncontrolled. Jhol release SHA-256: `6739eb1f845bc1e3187adda65408b224e7345ce150d7bafc2c77da0530202daf`.

| Window | Valid samples | Median | p95 | Downloaded bytes/run |
|---:|---:|---:|---:|---:|
| 4 | 30/30 | 1,000.65 ms | 2,392.23 ms | 7,610,763 |
| 8 | 29/30 | 959.67 ms | 2,808.89 ms | 7,610,763 |

For the 29 rounds where both samples passed, paired median ratio (8/4) was recorded with bootstrap 95% CI in JSON. The interval crosses 1; the median difference is not a stable win. Window 8 also had the worse p95 and one failed install. Its error was `failed to unpack package/bin/esbuild ... (details: download_failed)`; route ran, but the install and follow-up consumer checks failed. Preserve this as a quality failure.

Do not change the default. The three-round sweep that favored 8 was not replicated robustly. Investigate the failed unpack path before considering a higher window. No Bun comparison or 4× claim is made.
