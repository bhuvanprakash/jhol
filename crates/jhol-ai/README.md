# jhol-ai

Jhol model files stay ordinary project files with exact SHA-256 pins. Remote
preparation supports public Hugging Face files pinned to a 40-character commit;
the optional `infer` path runs a small Jhol-owned linear classifier on a bounded
CPU runtime. It accepts numeric feature vectors and does not execute model code.

- `jhol ai pin` hashes files already present locally.
- `jhol ai pin-remote` records trusted file hashes and byte sizes without network
  access.
- `jhol ai prepare` verifies cached files offline by default.
- `jhol ai prepare --allow-network` fetches missing pinned files over HTTPS,
  resumes partial files, enforces `--max-bytes`, verifies SHA-256 before
  publishing, removes completed partials, and prints a deterministic download
  receipt with byte counts and cleaned partial paths.
- `jhol ai infer --config PATH --input 1,2` verifies the complete local
  manifest before running the `jhol.linear-classifier.v1` format on
  `jhol-linear-cpu-v1`. The runtime caps the artifact at 256 KiB, config at
  16 KiB, manifest at 256 KiB, features at 64, and classes at 16. This small
  classifier is a real local inference example; it does not claim transformer
  or LLM support.

The config schema is `schemas/jhol-linear-model-v1.schema.json`. The weights
file starts with the eight-byte `JHLIN001` header, followed by little-endian
`f32` values: a row-major class-by-feature matrix, then one bias per class.
Jhol reads verified weights as data and performs the linear scores plus stable
softmax itself.

Use `schemas/jhol-model-v1.schema.json` for manifest shape. Never put access
tokens in a manifest. Current remote fetch supports public Hugging Face files;
private repositories and general-purpose model runtimes remain out of scope.
