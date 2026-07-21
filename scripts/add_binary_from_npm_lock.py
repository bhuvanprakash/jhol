#!/usr/bin/env python3
"""Add binary package artifacts for exact versions from npm package-lock closure.

This script creates a temporary npm project, generates package-lock.json for the
requested seed packages, then builds missing JHOL binary artifacts for compatible
(os/cpu) package entries in that lockfile.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import platform
import shutil
import struct
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path
from typing import Dict, Iterable, Optional, Tuple

BINARY_MAGIC = b"JHOL"
BINARY_VERSION = 1
USER_AGENT = "jhol-lock-builder/1.0"


def compute_hash(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def build_binary_package(package: str, version: str, tarball_data: bytes) -> bytes:
    content_hash = compute_hash(tarball_data)

    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode="wb", compresslevel=6) as fh:
        fh.write(tarball_data)
    compressed_data = compressed.getvalue()

    package_bytes = package.encode("utf-8")
    version_bytes = version.encode("utf-8")

    header = struct.pack(
        "<4sIHH32sII",
        BINARY_MAGIC,
        BINARY_VERSION,
        len(package_bytes),
        len(version_bytes),
        bytes.fromhex(content_hash),
        len(tarball_data),
        len(compressed_data),
    )
    return header + package_bytes + version_bytes + compressed_data


def verify_binary_archive(binary_data: bytes) -> None:
    if len(binary_data) < 52:
        raise RuntimeError("binary package too small")
    if binary_data[0:4] != BINARY_MAGIC:
        raise RuntimeError("invalid binary magic")

    package_name_len = int.from_bytes(binary_data[8:10], "little")
    version_len = int.from_bytes(binary_data[10:12], "little")
    header_hash = binary_data[12:44]
    original_size = int.from_bytes(binary_data[44:48], "little")
    compressed_size = int.from_bytes(binary_data[48:52], "little")

    compressed_start = 52 + package_name_len + version_len
    compressed_end = compressed_start + compressed_size
    if compressed_end > len(binary_data):
        raise RuntimeError("truncated binary payload")

    with gzip.GzipFile(fileobj=io.BytesIO(binary_data[compressed_start:compressed_end]), mode="rb") as fh:
        tarball = fh.read()

    if len(tarball) != original_size:
        raise RuntimeError("size mismatch")
    if hashlib.sha256(tarball).digest() != header_hash:
        raise RuntimeError("content hash mismatch")

    with tarfile.open(fileobj=io.BytesIO(tarball), mode="r:gz") as tf:
        _ = tf.getmembers()


def npm_os_name() -> str:
    sys_name = platform.system().lower()
    if sys_name == "darwin":
        return "darwin"
    if sys_name == "linux":
        return "linux"
    if sys_name.startswith("win"):
        return "win32"
    return sys_name


def npm_cpu_name() -> str:
    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64"}:
        return "x64"
    if machine in {"aarch64", "arm64"}:
        return "arm64"
    return machine


def field_allows_current(value: object, current: str) -> bool:
    if value is None:
        return True
    if isinstance(value, str):
        values = [value]
    elif isinstance(value, list):
        values = [str(v) for v in value]
    else:
        return True

    positives = []
    for token in values:
        token = token.strip()
        if not token:
            continue
        if token.startswith("!"):
            if token[1:] == current:
                return False
        else:
            positives.append(token)

    return not positives or current in positives


def package_name_from_lock_path(path: str, entry: Dict[str, object]) -> Optional[str]:
    name = entry.get("name")
    if isinstance(name, str) and name:
        return name

    parts = [p for p in path.split("/") if p]
    if "node_modules" not in parts:
        return None
    idx = len(parts) - 1 - parts[::-1].index("node_modules")
    seg = parts[idx + 1 :]
    if not seg:
        return None
    if seg[0].startswith("@") and len(seg) >= 2:
        return f"{seg[0]}/{seg[1]}"
    return seg[0]


def fetch_url(url: str, timeout: int = 180, retries: int = 4) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    last_err: Optional[Exception] = None
    for _ in range(max(1, retries)):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.read()
        except Exception as exc:  # noqa: BLE001
            last_err = exc
            continue
    raise RuntimeError(f"request failed for {url}: {last_err}")


def run_npm_lock(seed_specs: Iterable[str]) -> Dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="jhol-lock-build-") as td:
        root = Path(td)
        subprocess.run(["npm", "init", "-y"], cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        cmd = ["npm", "install", *seed_specs, "--package-lock-only", "--ignore-scripts"]
        subprocess.run(cmd, cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        lock_path = root / "package-lock.json"
        return json.loads(lock_path.read_text())


def main() -> int:
    parser = argparse.ArgumentParser(description="Add binary artifacts from npm lockfile closure")
    parser.add_argument("packages", nargs="+", help="Seed packages/specs for npm lockfile")
    parser.add_argument("--binary-dir", default="binary-packages", help="Binary packages directory")
    args = parser.parse_args()

    binary_dir = Path(args.binary_dir)
    binary_dir.mkdir(parents=True, exist_ok=True)

    index_path = binary_dir / "index.json"
    deps_path = binary_dir / "deps.json"

    index: Dict[str, str] = {}
    if index_path.exists():
        index = {str(k): str(v) for k, v in json.loads(index_path.read_text()).items()}

    deps_map: Dict[str, Dict[str, str]] = {}
    if deps_path.exists():
        raw = json.loads(deps_path.read_text())
        deps_map = {
            str(k): {str(dk): str(dv) for dk, dv in v.items()}
            for k, v in raw.items()
            if isinstance(v, dict)
        }

    lock = run_npm_lock(args.packages)
    packages_obj = lock.get("packages", {})
    if not isinstance(packages_obj, dict):
        raise RuntimeError("invalid package-lock: missing packages map")

    current_os = npm_os_name()
    current_cpu = npm_cpu_name()

    added = 0
    skipped = 0

    for path, entry_obj in sorted(packages_obj.items(), key=lambda kv: kv[0]):
        if not isinstance(entry_obj, dict):
            continue
        if path == "":
            continue

        name = package_name_from_lock_path(path, entry_obj)
        version = entry_obj.get("version")
        resolved = entry_obj.get("resolved")
        if not isinstance(name, str) or not name:
            continue
        if not isinstance(version, str) or not version:
            continue
        if not isinstance(resolved, str) or not resolved.startswith("http"):
            continue

        if not field_allows_current(entry_obj.get("os"), current_os):
            continue
        if not field_allows_current(entry_obj.get("cpu"), current_cpu):
            continue

        spec = f"{name}@{version}"
        if spec in index:
            skipped += 1
        else:
            tarball = fetch_url(resolved)
            binary = build_binary_package(name, version, tarball)
            verify_binary_archive(binary)

            content_hash = compute_hash(tarball)
            artifact = binary_dir / f"{content_hash}.jhol"
            if not artifact.exists():
                artifact.write_bytes(binary)
            index[spec] = content_hash
            added += 1

        dep_obj = entry_obj.get("dependencies")
        if isinstance(dep_obj, dict):
            deps_map[spec] = {str(k): str(v) for k, v in dep_obj.items()}
        else:
            deps_map.setdefault(spec, {})

    index_sorted = dict(sorted(index.items()))
    deps_sorted = dict(sorted(deps_map.items()))

    index_path.write_text(json.dumps(index_sorted, indent=2) + "\n")
    deps_path.write_text(json.dumps(deps_sorted, indent=2) + "\n")

    print(f"added={added} skipped={skipped} total_index={len(index_sorted)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
