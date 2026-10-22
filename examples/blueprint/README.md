# Blueprint polyglot example

Small JS + Python + Rust tree showing Jhol's native JavaScript manager working
alongside each ecosystem's own tool: uv for Python and Cargo for Rust.

`models/demo/` remains a tiny integrity-only fixture. `models/linear-demo/` is
a usable, Jhol-owned two-feature linear classifier: `jhol.linear-classifier.v1`
format, `jhol-linear-cpu-v1` runtime, and a checked-in digest-pinned model.
Inference loads bounded numeric weights as data; it runs no model code and needs
no external model package or network access.

```sh
# from repo root, after `cargo build --bin jhol`
./target/debug/jhol plan --path examples/blueprint
./target/debug/jhol plan --path examples/blueprint --json
./target/debug/jhol ai infer --path examples/blueprint \
  --manifest models/linear-demo.jhol-model.json \
  --config models/linear-demo/config.json --input=-2,1
```

The model uses 256 KiB artifact and manifest caps, a 16 KiB config cap, at most 64 input
features, and at most 16 classes. `./verify.sh` runs the prediction twice and
checks the stable `large` result. This demonstrates actual local inference for
the small linear format; it does not claim transformer or LLM support.

`jhol.toml` declares intent; native locks remain authoritative. No `sync` / install
side effects run from `plan`.

`jhol dev` reads explicit `[[service]]` declarations from the same file.
`--probe` checks their readiness endpoints; `--spawn` runs only declared
argument arrays. The disposable demo listeners require `python3` and free
loopback ports 15432, 18000, and 18080. They are fixtures, not databases or
application servers.

`sync --apply` plans all detected adapters first, checks policy for every plan,
then applies them in order. Python build backends and Cargo build scripts require
`--allow-build-scripts`; `--offline` is passed to Jhol, uv, and Cargo. The
verification script uses a temporary, dependency-free copy to exercise the full
offline flow without changing this example's manifests or generated environment.

Verification script: `./verify.sh` (expects a `jhol` binary on `PATH` or
`JHOL_BIN`).

Optional network-backed dependency check: `python3 verify_online.py`. It uses a
temporary directory and pinned `ms`, `idna`, and `itoa` dependencies to exercise
real registry downloads, generated native locks, adapter receipts, and the
compiled Rust/Python outputs. It requires `jhol`, `uv`, and Cargo. Cargo build
scripts and Python build backends are explicitly approved for this isolated
fixture by the command; do not point it at a real project. CI is configured to
run this check on Ubuntu, macOS, and Windows.
