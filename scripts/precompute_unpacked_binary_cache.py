#!/usr/bin/env python3
"""Precompute unpacked trees for committed binary packages.

Creates binary-packages/.unpacked/<hash>/... where content mirrors extracted package
contents without the top-level tarball `package/` directory.
"""

import gzip
import hashlib
import io
import json
import tarfile
from pathlib import Path

BINARY_DIR = Path("binary-packages")
INDEX_PATH = BINARY_DIR / "index.json"
UNPACKED_ROOT = BINARY_DIR / ".unpacked"


def decode_jhol(archive: bytes) -> bytes:
    if len(archive) < 52 or archive[:4] != b"JHOL":
        raise RuntimeError("invalid binary package header")

    package_name_len = int.from_bytes(archive[8:10], "little")
    version_len = int.from_bytes(archive[10:12], "little")
    expected_hash = archive[12:44]
    original_size = int.from_bytes(archive[44:48], "little")
    compressed_size = int.from_bytes(archive[48:52], "little")

    payload_start = 52 + package_name_len + version_len
    payload_end = payload_start + compressed_size
    if payload_end > len(archive):
        raise RuntimeError("truncated binary package")

    tarball = gzip.decompress(archive[payload_start:payload_end])
    if len(tarball) != original_size:
        raise RuntimeError("size mismatch")

    digest = hashlib.sha256(tarball).digest()
    if digest != expected_hash:
        raise RuntimeError("hash mismatch")

    return tarball


def extract_tarball(tarball_bytes: bytes, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(tarball_bytes), mode="r:gz") as tf:
        for member in tf.getmembers():
            if not member.name:
                continue
            parts = [p for p in member.name.split("/") if p]
            if not parts or parts[0] != "package":
                continue
            rel_parts = parts[1:]
            if not rel_parts:
                continue
            target = destination.joinpath(*rel_parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if member.issym() or member.islnk():
                # Keep extraction deterministic/safe; skip links.
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            src = tf.extractfile(member)
            if src is None:
                continue
            with target.open("wb") as out:
                out.write(src.read())


def main() -> int:
    if not INDEX_PATH.exists():
        raise RuntimeError(f"missing {INDEX_PATH}")

    index = json.loads(INDEX_PATH.read_text())
    if not isinstance(index, dict):
        raise RuntimeError("index.json must be an object")

    hashes = sorted(set(str(v) for v in index.values()))
    UNPACKED_ROOT.mkdir(parents=True, exist_ok=True)

    built = 0
    skipped = 0

    for i, digest in enumerate(hashes, start=1):
        out_dir = UNPACKED_ROOT / digest
        if out_dir.exists():
            skipped += 1
            continue

        archive_path = BINARY_DIR / f"{digest}.jhol"
        if not archive_path.exists():
            raise RuntimeError(f"missing artifact {archive_path}")

        print(f"[{i:03d}/{len(hashes):03d}] {digest}")
        archive = archive_path.read_bytes()
        tarball = decode_jhol(archive)

        tmp_dir = UNPACKED_ROOT / f"{digest}.tmp"
        if tmp_dir.exists():
            for child in tmp_dir.iterdir():
                if child.is_dir():
                    import shutil

                    shutil.rmtree(child, ignore_errors=True)
                else:
                    child.unlink(missing_ok=True)
        tmp_dir.mkdir(parents=True, exist_ok=True)

        extract_tarball(tarball, tmp_dir)
        tmp_dir.rename(out_dir)
        built += 1

    print(f"Ready unpacked cache at {UNPACKED_ROOT} (built={built}, skipped={skipped})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
