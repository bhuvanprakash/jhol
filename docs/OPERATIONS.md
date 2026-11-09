# Operations — caches, gates, measurements, troubleshooting

## Workspace map

| crate | lines | role |
|---|---:|---|
| `jhol` (`src/`) | CLI | dispatch, lanes, subcommands |
| `jhol-core` | 94 659 | installer, resolver, registry, store, lockfile, policies |
| `jhol-vm` | 31 899 | the JavaScript engine (lexer → JIT) |
| `jhol-bundle` | 8 797 | bundler |
| `jhol-runtime` | 6 790 | runtime helpers (daemon, transports) |
| `jhol-trampoline` | 1 291 | thin warm front door |
| `jhol-treecheck` | 640 | the tree-vs-lockfile verdict |
| `jhol-adapters`, `jhol-ai`, `jhol-cloud`, `jhol-db`, `jhol-diff`, `jhol-env`, `jhol-graph`, `jhol-policy`, `jhol-receipt`, `jhol-remote`, `jhol-task` | 500–1 500 each | supporting subsystems |

---

## Cache layout (`$JHOL_CACHE_DIR`, default `~/.jhol-cache`)

```
packuments/<sha256>.json        gzipped HTTP bodies (abbreviated)
packuments/<sha256>.json.idx    resolver's view of the same (see RESOLVER-JAGR.md §6)
store/<sri>.tgz (+ .sri)        content-addressed tarballs
store_unpacked/<shard>/<sri>/   extracted trees, completeness-marked
grid/bundles/<hash>             prepared trees for L0 projection
store_index.json                reachability index used by `cache gc`
lockfiles/, expansion_cache/, daemon-cache/, logs.txt, markov.chain
```

Invariants:

1. bytes are never returned unless they match the requested SRI;
2. a tarball and its `.sri` are published together — a sidecar without its tarball is
   absent, not a hit;
3. an unpacked entry counts as present only when the marker **and** a non-empty
   `package.json` agree (see `DEFECT-LOG.md` §5);
4. write-through never fails an install: a cache that cannot be written is a slower
   install, not a broken one.

---

## Gates

```bash
sh crates/jhol-core/verify.sh     # build, unit tests, adversarial corpus,
                                  # E2E offline install, byte-identical determinism,
                                  # gc safety
sh crates/jhol-vm/verify.sh       # engine corpus, unit tests, jit/interpreter agreement

cargo test -p jhol-core --lib     # 678 tests, 7 ignored (each with its reason)
cargo test -p jhol-treecheck --lib
cargo build --release --features jholvm
```

`crates/jhol-core/src/adversarial_install.rs` builds hostile tarballs **on the raw
wire** (13 tests): the `tar` crate refuses hostile paths itself, so fixtures made
through it prove nothing.

---

## Measuring

Absolute numbers move 2–3× with machine load, so:

```bash
# resolver phases
JAGR_PROFILE_CHART=1 cargo run -q -p jhol-core --example resolve_bench -- /path/to/packuments jest

# install phases
JHOL_PROFILE_INSTALL=1 JHOL_VERBOSE=1 jhol install
JHOL_HTTP_TIMING=1 jhol install
```

Rules that came from being wrong:

* **Kill the daemon first.** It can serve a stale verdict:
  `pkill -9 -f 'jhol-bin daemon'`.
* **`stat` the binary.** A build killed by `timeout` leaves the old binary, so
  "0 errors" may describe code that was never compiled.
* **Interleave A/B.** Run both tools in the same seconds; never compare today's
  number with yesterday's.
* **Report phases, not totals.** Cold `jest` moved 31 s → 64 s between runs of the
  same manifest with only the network changing.
* **Measure the phase you are optimising.** Two confident hypotheses (version
  parsing, JSON extraction) were both wrong; the third measurement found the actual
  141.6 ms.

---

## Troubleshooting

| symptom | first thing to check |
|---|---|
| "Up to date" but something is wrong | `jhol verify`; then `jhol why <pkg>` |
| a range cannot be installed | `jhol why-not <pkg>@<range>` |
| repeated `Repaired` on warm runs | `jhol verify --json`; suspect a corrupt store entry (`store_extract_complete`) |
| install waits | another install holds `.jhol/install.lock`; the message names the pid |
| install refuses on a date | full metadata unavailable; see `RESOLVER-JAGR.md` §5 |
| measurements contradict the filesystem | kill the daemon; `stat` the binary |
| `No space left on device` | free `target/debug` (regenerable, 12 GB) before building |
| a flag seems ignored | it may be declared in the dead clap builder — `DEFECT-LOG.md` §6 |
| engine prints a JSException dump | an interpreted string would be better; report the shape |

---

## Landmines (each cost real time)

1. ~~**Two clap builders**~~ — **resolved**: one `cli_command()`, used by dispatch
   and by completion generation. It cost five fixes (including a 100 % `ci` crash)
   before it was worth doing; do not reintroduce a second tree for a "small" reason.
2. **Daemon staleness**: it holds verdicts across binary upgrades.
3. **A `timeout`-killed build leaves the old binary.**
4. **`$!` inside a multi-line bash block** resolves oddly; capture PIDs in a subshell
   writing to a file when profiling.
5. **`utils::log` is silenced on fast lanes**; security summaries must go to stderr
   directly.
6. **The `tar` crate refuses hostile paths**, so adversarial fixtures must be raw
   wire bytes.
7. **`build_full_chart` used to leave names empty** — fixed, but callers exist in
   tests that still fill them; the builder is now authoritative.
8. **Pruning is opt-in** (`JHOL_PRUNE_EXTRANEOUS=1`) because a lockfile can be an
   incomplete record; `jhol verify --prune` is the explicit request.

## One entry point: `scripts/verify-all.sh`

CI ran `cargo test`, which builds **default features only** and never runs the two
quality gates. Measured with `--all-features`: `jhol-core` runs **747** library tests
instead of 721 — **26 tests nothing was running** — and the model/py/engine features
were only ever compiled by hand.

```sh
sh scripts/verify-all.sh                 # everything, one round
ROUNDS=5 sh scripts/verify-all.sh        # flake hunt: five rounds
VERIFY_FAST=1 sh scripts/verify-all.sh   # skip the all-features build
```

It runs `cargo test --workspace --all-features`, the installer gate, the engine gate
(skipped with a notice when `node` is absent — it is the corpus oracle), and with
`ROUNDS>1` re-runs the four library suites and names the first disagreement. CI gained
an `all-features` job with the same command.

### Disk: the all-features build needs room

The first attempt at this failed with
`failed to write query cache …: No space left on device`. `--all-features` recompiles
every crate with extra code paths, and `target/debug` alone reached **7.6 GB**. Check
before a big build:

```sh
df -h / | tail -1        # want > 3 GB free
rm -rf target/debug      # always regenerable; never delete target/release blindly
```

Freeing `target/debug` and stale `/tmp` fixture directories took free space from
194 MB to 7.5 GB and the build then passed.
