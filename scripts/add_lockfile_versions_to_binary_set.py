#!/usr/bin/env python3
"""Add lockfile-pinned name@version pairs to binary-packages/ (jhol's own bundled store).

Archive format (matches crates/jhol-core/src/registry.rs decode_binary_package_archive):
  "JHOL" + u32le(1) + u16 name_len + u16 ver_len + sha256(tgz)[32B]
  + u32 original_size + u32 compressed_size + name + ver + gzip(tgz)
index.json: "name@version" -> sha256(tgz) hex
"""
import gzip
import hashlib
import json
import platform
import struct
import sys
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "binary-packages"

# Current-platform filter: skip optional variants built for other OS/CPU.
_CUR_OS = {"darwin": "darwin", "linux": "linux", "windows": "win32"}.get(platform.system().lower())
_CUR_CPU = {"x86_64": "x64", "amd64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(platform.machine().lower())

_packument_cache: dict[str, dict] = {}


def packument(name: str) -> dict:
    if name not in _packument_cache:
        url = f"https://registry.npmjs.org/{urllib.parse.quote(name, safe='')}"
        req = urllib.request.Request(url, headers={"accept": "application/vnd.npm.install-v1+json"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                _packument_cache[name] = json.loads(r.read())
        except Exception:  # noqa: BLE001 - missing packument = download and let jhol verify
            _packument_cache[name] = {}
    return _packument_cache[name]


def platform_mismatch(name: str, ver: str) -> bool:
    v = packument(name).get("versions", {}).get(ver) or {}
    if _CUR_OS and "os" in v:
        if _CUR_OS not in [str(o).lstrip("!") for o in v["os"]] and not any(
            str(o).startswith("!") for o in v["os"]
        ):
            return True
    if _CUR_CPU and "cpu" in v:
        if _CUR_CPU not in [str(c).lstrip("!") for c in v["cpu"]] and not any(
            str(c).startswith("!") for c in v["cpu"]
        ):
            return True
    return False


def fetch(name: str, ver: str) -> bytes | None:
    base = name.split("/")[-1]
    url = f"https://registry.npmjs.org/{urllib.parse.quote(name, safe='')}/-/{base}-{ver}.tgz"
    try:
        with urllib.request.urlopen(url, timeout=30) as r:
            return r.read()
    except Exception as e:  # noqa: BLE001 - report and skip, never abort the set
        print(f"skip {name}@{ver}: {e}", file=sys.stderr)
        return None


def main() -> int:
    lock_path = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO / "bench_cold" / "package-lock.json"
    lock = json.loads(lock_path.read_text())
    idx = json.loads((BIN / "index.json").read_text())

    wanted = {}
    for key, meta in lock.get("packages", {}).items():
        if not key or "version" not in meta:
            continue
        name = key.split("node_modules/")[-1]
        ver = str(meta["version"])
        wkey = f"{name}@{ver}"
        if wkey not in idx:
            wanted[wkey] = (name, ver)

    # Drop variants built for other platforms before downloading (E21-style).
    skipped_platform = 0
    to_fetch = {}
    for wkey, (name, ver) in wanted.items():
        if platform_mismatch(name, ver):
            skipped_platform += 1
        else:
            to_fetch[wkey] = (name, ver)
    wanted = to_fetch

    if not wanted:
        print("index already covers lockfile — nothing to do")
        return 0

    print(f"fetching {len(wanted)} packages ({skipped_platform} skipped as other-platform)...")
    with ThreadPoolExecutor(max_workers=16) as ex:
        results = list(ex.map(lambda kv: (kv[0], kv[1], fetch(*kv[1])), wanted.items()))

    added = 0
    for wkey, (name, ver), tgz in results:
        if not tgz:
            continue
        digest = hashlib.sha256(tgz).digest()
        comp = gzip.compress(tgz, 6)
        arch = (
            b"JHOL"
            + struct.pack("<I", 1)
            + struct.pack("<HH", len(name.encode()), len(ver.encode()))
            + digest
            + struct.pack("<II", len(tgz), len(comp))
            + name.encode()
            + ver.encode()
            + comp
        )
        (BIN / f"{digest.hex()}.jhol").write_bytes(arch)
        idx[wkey] = digest.hex()
        added += 1

    idx_sorted = dict(sorted(idx.items()))
    (BIN / "index.json").write_text(json.dumps(idx_sorted, indent=2) + "\n")
    print(f"added {added} packages; index now {len(idx_sorted)} entries")
    # One command = complete store curation. deps.json + meta.json must cover
    # every new archive: without them the metadata synth refuses the package
    # (safe, but network-bound) and peer/platform truth lags behind — the exact
    # hidden-issue class from OPTIMIZATION_TIER1_3_6_8 Appendix 13.
    import subprocess as sp
    sp.run(
        [sys.executable, str(Path(__file__).resolve().parent / "enrich_binary_store_deps.py")],
        check=False,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
