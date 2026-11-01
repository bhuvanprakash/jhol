Benchmarking 6 workloads (warmup=3, runs=10)...

Running array.js ...
Running fib.js ...
Running json.js ...
Running loop.js ...
Running objects.js ...
Running strings.js ...

| benchmark | jhol avg (ms) | jhol median | bun avg (ms) | bun median | node avg (ms) | node median | jhol/bun | jhol/node |
|-----------|---------------|-------------|--------------|------------|---------------|-------------|----------|-----------|
| array.js  |        57.224 |      56.730 |       15.467 |     15.494 |        35.648 |      35.838 |     3.66 |      1.58 |
| fib.js    |       763.648 |     743.300 |       45.836 |     45.516 |       104.083 |     101.328 |    16.33 |      7.34 |
| json.js   |       338.767 |     328.398 |       35.693 |     35.897 |        63.546 |      63.387 |     9.15 |      5.18 |
| loop.js   |       218.495 |     214.782 |       16.120 |     16.032 |        80.313 |      78.655 |    13.40 |      2.73 |
| objects.js |       308.150 |     306.369 |      445.375 |    426.151 |       770.408 |     761.507 |     0.72 |      0.40 |
| strings.js |         9.284 |       8.865 |        9.611 |      9.637 |        28.201 |      28.061 |     0.92 |      0.32 |

Raw JSON:
[
  {
    "runtime": "jhol",
    "file": "array.js",
    "runs": 10,
    "avg_ms": 57.224,
    "median_ms": 56.73,
    "min_ms": 56.224,
    "max_ms": 59.245
  },
  {
    "runtime": "jhol",
    "file": "fib.js",
    "runs": 10,
    "avg_ms": 763.648,
    "median_ms": 743.3,
    "min_ms": 670.713,
    "max_ms": 859.882
  },
  {
    "runtime": "jhol",
    "file": "json.js",
    "runs": 10,
    "avg_ms": 338.767,
    "median_ms": 328.398,
    "min_ms": 298.516,
    "max_ms": 404.778
  },
  {
    "runtime": "jhol",
    "file": "loop.js",
    "runs": 10,
    "avg_ms": 218.495,
    "median_ms": 214.782,
    "min_ms": 210.25,
    "max_ms": 235.779
  },
  {
    "runtime": "jhol",
    "file": "objects.js",
    "runs": 10,
    "avg_ms": 308.15,
    "median_ms": 306.369,
    "min_ms": 282.082,
    "max_ms": 329.685
  },
  {
    "runtime": "jhol",
    "file": "strings.js",
    "runs": 10,
    "avg_ms": 9.284,
    "median_ms": 8.865,
    "min_ms": 8.728,
    "max_ms": 10.6
  },
  {
    "runtime": "bun",
    "file": "array.js",
    "runs": 10,
    "avg_ms": 15.466962501523085,
    "median_ms": 15.493666986003518,
    "min_ms": 14.674500009277835,
    "max_ms": 16.398999985540286
  },
  {
    "runtime": "bun",
    "file": "fib.js",
    "runs": 10,
    "avg_ms": 45.83647509862203,
    "median_ms": 45.51631250069477,
    "min_ms": 44.48125002090819,
    "max_ms": 48.104209010489285
  },
  {
    "runtime": "bun",
    "file": "json.js",
    "runs": 10,
    "avg_ms": 35.692679102066904,
    "median_ms": 35.897062509320676,
    "min_ms": 34.48387500247918,
    "max_ms": 37.03808301361278
  },
  {
    "runtime": "bun",
    "file": "loop.js",
    "runs": 10,
    "avg_ms": 16.1198043031618,
    "median_ms": 16.032375002396293,
    "min_ms": 15.773375023854896,
    "max_ms": 16.943082999205217
  },
  {
    "runtime": "bun",
    "file": "objects.js",
    "runs": 10,
    "avg_ms": 445.3749832959147,
    "median_ms": 426.15120849222876,
    "min_ms": 402.0724580041133,
    "max_ms": 545.1524159871042
  },
  {
    "runtime": "bun",
    "file": "strings.js",
    "runs": 10,
    "avg_ms": 9.610737505136058,
    "median_ms": 9.636562506784685,
    "min_ms": 9.343665995402262,
    "max_ms": 9.881750011118129
  },
  {
    "runtime": "node",
    "file": "array.js",
    "runs": 10,
    "avg_ms": 35.64844138745684,
    "median_ms": 35.83831248397473,
    "min_ms": 32.65045798616484,
    "max_ms": 39.05041600228287
  },
  {
    "runtime": "node",
    "file": "fib.js",
    "runs": 10,
    "avg_ms": 104.08336269611027,
    "median_ms": 101.3277294987347,
    "min_ms": 86.11304199439473,
    "max_ms": 146.5453749988228
  },
  {
    "runtime": "node",
    "file": "json.js",
    "runs": 10,
    "avg_ms": 63.5457208059961,
    "median_ms": 63.386625493876636,
    "min_ms": 61.66833301540464,
    "max_ms": 66.11762501415797
  },
  {
    "runtime": "node",
    "file": "loop.js",
    "runs": 10,
    "avg_ms": 80.31299590365961,
    "median_ms": 78.65493799909018,
    "min_ms": 76.89220801694319,
    "max_ms": 85.70141700329259
  },
  {
    "runtime": "node",
    "file": "objects.js",
    "runs": 10,
    "avg_ms": 770.4077375936322,
    "median_ms": 761.5067289880244,
    "min_ms": 744.509707990801,
    "max_ms": 826.2548749917187
  },
  {
    "runtime": "node",
    "file": "strings.js",
    "runs": 10,
    "avg_ms": 28.2005375076551,
    "median_ms": 28.06104150658939,
    "min_ms": 27.052625024225563,
    "max_ms": 29.58683401811868
  }
]
