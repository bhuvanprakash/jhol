#!/usr/bin/env python3
"""Enrich binary-packages/deps.json from the .jhol archives themselves.

The store already carries every package's real package.json (that is what the
tarball path serves). Metadata synthesis needs the same truth: name@version ->
dependencies, extracted from bytes already on disk. No network.
"""
import gzip, io, json, struct, sys, tarfile
from pathlib import Path

BIN = Path(__file__).resolve().parent.parent / "binary-packages"

def meta_from_archive(path: Path):
    """Peer + platform truth from the archive's own package.json: the fields the
    network packument carries and a metadata synth must not silently drop."""
    data = path.read_bytes()
    if len(data) < 52 or data[:4] != b"JHOL":
        return None
    nlen = struct.unpack("<H", data[8:10])[0]
    vlen = struct.unpack("<H", data[10:12])[0]
    csz = struct.unpack("<I", data[48:52])[0]
    start = 52 + nlen + vlen
    tgz = gzip.decompress(data[start:start + csz])
    with tarfile.open(fileobj=io.BytesIO(tgz), mode="r:gz") as tf:
        for m in tf.getmembers():
            if m.name in ("package/package.json", "package.json"):
                pj = json.load(tf.extractfile(m))
                meta = {}
                for k in ("peerDependencies", "peerDependenciesMeta", "optionalDependencies",
                          "os", "cpu", "libc", "engines"):
                    if pj.get(k):
                        meta[k] = pj[k]
                return meta
    return None


def deps_from_archive(path: Path):
    data = path.read_bytes()
    if len(data) < 52 or data[:4] != b"JHOL":
        return None
    nlen = struct.unpack("<H", data[8:10])[0]
    vlen = struct.unpack("<H", data[10:12])[0]
    csz = struct.unpack("<I", data[48:52])[0]
    start = 52 + nlen + vlen
    tgz = gzip.decompress(data[start:start + csz])
    with tarfile.open(fileobj=io.BytesIO(tgz), mode="r:gz") as tf:
        for m in tf.getmembers():
            if m.name in ("package/package.json", "package.json"):
                pj = json.load(tf.extractfile(m))
                out = {}
                out.update(pj.get("dependencies") or {})
                for k, v in (pj.get("optionalDependencies") or {}).items():
                    out.setdefault(k, v)
                return out
    return None

def main() -> int:
    idx = json.loads((BIN / "index.json").read_text())
    deps_path = BIN / "deps.json"
    try:
        deps = json.loads(deps_path.read_text())
    except Exception:
        deps = {}
    missing = [k for k in idx if k not in deps]
    filled = 0
    for k in missing:
        arch = BIN / f"{idx[k]}.jhol"
        if not arch.exists():
            continue
        d = deps_from_archive(arch)
        if d is not None:
            deps[k] = d          # {} = known zero-dep package, distinct from unknown
            filled += 1
    deps_path.write_text(json.dumps(dict(sorted(deps.items())), indent=2) + "\n")
    print(f"deps.json: {len(deps)} entries, filled {filled} from archives")

    meta_path = BIN / "meta.json"
    try:
        metas = json.loads(meta_path.read_text())
    except Exception:
        metas = {}
    mfilled = 0
    for k in idx:
        if k in metas:
            continue
        arch = BIN / f"{idx[k]}.jhol"
        if not arch.exists():
            continue
        m = meta_from_archive(arch)
        if m:
            metas[k] = m
            mfilled += 1
        else:
            metas[k] = {}     # known-empty: no peers, no platform pins
    meta_path.write_text(json.dumps(dict(sorted(metas.items())), indent=2) + "\n")
    with_peers = sum(1 for v in metas.values() if v.get("peerDependencies"))
    print(f"meta.json: {len(metas)} entries, filled {mfilled}, {with_peers} carry peers")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
