#!/usr/bin/env python3
"""Binary package cache tooling (merged suite)."""
from __future__ import annotations

import sys


# ── build: was build_binary_packages.py ──

"""Build deterministic JHOL binary package artifacts for a pinned package set.

Supports resumable builds with a partial index file and parallel workers.
"""

import argparse
import gzip
import hashlib
import io
import json
import struct
import tarfile
import threading
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Dict, List, Optional, Tuple

BINARY_MAGIC = b"JHOL"
BINARY_VERSION = 1
DEFAULT_DATA_FILE = Path("crates/jhol-core/data/binary-packages-top1000.json")
OUTPUT_DIR = Path("binary-packages")
DEFAULT_PARTIAL_INDEX = OUTPUT_DIR / "index.partial.json"


def compute_hash(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_json_map(path: Path) -> Dict[str, str]:
    if not path.exists():
        return {}
    try:
        raw = json.loads(path.read_text())
    except Exception:
        return {}
    if not isinstance(raw, dict):
        return {}
    return {str(k): str(v) for k, v in raw.items()}


def load_top_packages(data_file: Path, expected_count: Optional[int]) -> List[str]:
    packages = json.loads(data_file.read_text())
    if not isinstance(packages, list):
        raise RuntimeError(f"{data_file} must contain a JSON list")
    unique = list(dict.fromkeys(str(p).strip() for p in packages if str(p).strip()))
    if expected_count is not None and len(unique) != expected_count:
        raise RuntimeError(
            f"Expected exactly {expected_count} packages, found {len(unique)} in {data_file}"
        )
    return sorted(unique)


def fetch_url(url: str, timeout: int = 20, retries: int = 3) -> bytes:
    last_err: Optional[Exception] = None
    req = urllib.request.Request(url, headers={"User-Agent": "jhol-builder/1.0"})
    for _ in range(retries):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.read()
        except Exception as exc:  # noqa: BLE001
            last_err = exc
    raise RuntimeError(f"request failed for {url}: {last_err}")


def fetch_package_metadata(package: str) -> dict:
    encoded = package.replace("/", "%2F")
    payload = fetch_url(f"https://registry.npmjs.org/{encoded}")
    return json.loads(payload)


def download_tarball(url: str) -> bytes:
    return fetch_url(url, timeout=60)


def build_binary_package(package: str, version: str, tarball_data: bytes) -> bytes:
    content_hash = compute_hash(tarball_data)

    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode="wb", compresslevel=6) as f:
        f.write(tarball_data)
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


def decode_binary_package_archive(data: bytes) -> bytes:
    if len(data) < 52:
        raise RuntimeError("binary package too small")
    if data[0:4] != BINARY_MAGIC:
        raise RuntimeError("invalid magic")

    package_name_len = int.from_bytes(data[8:10], "little")
    version_len = int.from_bytes(data[10:12], "little")
    header_hash = data[12:44]
    original_size = int.from_bytes(data[44:48], "little")
    compressed_size = int.from_bytes(data[48:52], "little")

    compressed_start = 52 + package_name_len + version_len
    compressed_end = compressed_start + compressed_size
    if compressed_end > len(data):
        raise RuntimeError("truncated payload")

    with gzip.GzipFile(fileobj=io.BytesIO(data[compressed_start:compressed_end]), mode="rb") as f:
        tarball = f.read()

    if len(tarball) != original_size:
        raise RuntimeError("size mismatch")

    digest = hashlib.sha256(tarball).digest()
    if digest != header_hash:
        raise RuntimeError("hash mismatch")

    with tarfile.open(fileobj=io.BytesIO(tarball), mode="r:gz") as tar:
        _ = tar.getmembers()

    return tarball


def process_package(package: str) -> Tuple[str, str, str]:
    metadata = fetch_package_metadata(package)
    latest = metadata.get("dist-tags", {}).get("latest")
    if not latest:
        raise RuntimeError("missing dist-tags.latest")

    version_data = metadata.get("versions", {}).get(latest, {})
    tarball_url = version_data.get("dist", {}).get("tarball")
    if not tarball_url:
        raise RuntimeError("missing tarball url")

    tarball_data = download_tarball(tarball_url)
    binary_data = build_binary_package(package, latest, tarball_data)
    _ = decode_binary_package_archive(binary_data)

    content_hash = compute_hash(tarball_data)
    out_file = OUTPUT_DIR / f"{content_hash}.jhol"
    if not out_file.exists():
        out_file.write_bytes(binary_data)

    spec = f"{package}@{latest}"
    return spec, content_hash, package


def run_build(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "binary_packages.py", *argv]

    parser = argparse.ArgumentParser(description="Build binary package artifacts")
    parser.add_argument("--data-file", default=str(DEFAULT_DATA_FILE))
    parser.add_argument("--expected-count", type=int, default=None)
    parser.add_argument("--workers", type=int, default=12)
    parser.add_argument("--partial-index", default=str(DEFAULT_PARTIAL_INDEX))
    args = parser.parse_args()

    data_file = Path(args.data_file)
    target_packages = load_top_packages(data_file, args.expected_count)
    target_count = len(target_packages)

    OUTPUT_DIR.mkdir(exist_ok=True)
    partial_index_path = Path(args.partial_index)

    index: Dict[str, str] = {}
    index.update(load_json_map(OUTPUT_DIR / "index.json"))
    index.update(load_json_map(partial_index_path))

    pending = [p for p in target_packages if not any(k.startswith(f"{p}@") for k in index.keys())]
    print(f"total={target_count} cached={target_count-len(pending)} pending={len(pending)} workers={args.workers}")

    lock = threading.Lock()
    failed = 0
    completed = 0

    if pending:
        with ThreadPoolExecutor(max_workers=max(1, args.workers)) as pool:
            future_map = {pool.submit(process_package, pkg): pkg for pkg in pending}
            for fut in as_completed(future_map):
                pkg = future_map[fut]
                try:
                    spec, digest, _ = fut.result()
                    with lock:
                        index[spec] = digest
                        completed += 1
                        if completed % 25 == 0:
                            partial_index_path.write_text(json.dumps(dict(sorted(index.items())), indent=2) + "\n")
                        print(f"ok [{completed}/{len(pending)}] {spec}")
                except Exception as exc:  # noqa: BLE001
                    failed += 1
                    print(f"fail {pkg}: {exc}")

    # Ensure one entry per package (latest) in final index and deterministically sorted.
    per_package: Dict[str, Tuple[str, str]] = {}
    for spec, digest in index.items():
        if "@" not in spec:
            continue
        pkg, ver = spec.rsplit("@", 1)
        per_package[pkg] = (ver, digest)

    missing = [pkg for pkg in target_packages if pkg not in per_package]
    if missing:
        raise RuntimeError(f"missing {len(missing)} packages after build (examples: {', '.join(missing[:10])})")

    final_index: Dict[str, str] = {}
    for pkg in sorted(target_packages):
        ver, digest = per_package[pkg]
        final_index[f"{pkg}@{ver}"] = digest

    if failed != 0:
        raise RuntimeError(f"build incomplete: failed={failed}")

    (OUTPUT_DIR / "index.json").write_text(json.dumps(final_index, indent=2) + "\n")
    if partial_index_path.exists():
        partial_index_path.unlink()

    print(f"Built {len(final_index)} binary packages -> {OUTPUT_DIR}")
    return 0


# ── bundle: was build_binary_bundle.py ──

def run_bundle(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "binary_packages.py", *argv]
    import json
    from pathlib import Path

    root = Path('binary-packages')
    index_path = root / 'index.json'
    if not index_path.exists():
        raise SystemExit('binary-packages/index.json missing')

    index = json.loads(index_path.read_text())
    bundle_path = root / 'bundle.bin'
    bundle_index_path = root / 'bundle-index.json'

    bundle_index = {}
    offset = 0
    with bundle_path.open('wb') as out:
        # deterministic order by hash for stable bundle layout
        hashes = sorted(set(index.values()))
        for h in hashes:
            src = root / f'{h}.jhol'
            if not src.exists():
                continue
            data = src.read_bytes()
            out.write(data)
            bundle_index[h] = {'offset': offset, 'len': len(data)}
            offset += len(data)

    bundle_index_path.write_text(json.dumps(bundle_index, indent=2) + '\n')
    print(f'wrote {bundle_path} ({offset} bytes), entries={len(bundle_index)}')
    return 0


# ── add-from-lock: was add_binary_from_npm_lock.py ──

"""Add binary package artifacts for exact versions from npm package-lock closure.

This script creates a temporary npm project, generates package-lock.json for the
requested seed packages, then builds missing JHOL binary artifacts for compatible
(os/cpu) package entries in that lockfile.
"""


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


def run_add_from_lock(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "binary_packages.py", *argv]

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


# ── expand-set: was expand_binary_package_set.py ──

"""Expand a pinned package list to a larger set using dependency graph crawl."""


import argparse
import json
import urllib.parse
import urllib.request
from collections import deque
from pathlib import Path


def fetch_metadata(package: str, timeout: int = 20) -> dict:
    encoded = urllib.parse.quote(package, safe="")
    url = f"https://registry.npmjs.org/{encoded}"
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        return json.loads(resp.read())


def latest_version(meta: dict) -> str | None:
    return meta.get("dist-tags", {}).get("latest")


def latest_deps(meta: dict) -> list[str]:
    ver = latest_version(meta)
    if not ver:
        return []
    version_obj = meta.get("versions", {}).get(ver, {})
    deps = version_obj.get("dependencies", {}) or {}
    return sorted(str(k) for k in deps.keys())


def load_seed(path: Path) -> list[str]:
    data = json.loads(path.read_text())
    if not isinstance(data, list):
        raise RuntimeError(f"{path} must contain a JSON array")
    return [str(x).strip() for x in data if str(x).strip()]


def run_expand_set(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "binary_packages.py", *argv]

    parser = argparse.ArgumentParser(description="Expand binary package set")
    parser.add_argument("--seed", default="crates/jhol-core/data/binary-packages-top100.json")
    parser.add_argument("--out", default="crates/jhol-core/data/binary-packages-top1000.json")
    parser.add_argument("--target", type=int, default=1000)
    args = parser.parse_args()

    seed_path = Path(args.seed)
    out_path = Path(args.out)
    target = int(args.target)
    if target < 1:
        raise RuntimeError("--target must be >= 1")

    seed = load_seed(seed_path)
    if not seed:
        raise RuntimeError("seed list is empty")

    chosen: list[str] = []
    seen: set[str] = set()
    queue: deque[str] = deque(seed)

    while queue and len(chosen) < target:
        pkg = queue.popleft()
        if pkg in seen:
            continue
        seen.add(pkg)
        chosen.append(pkg)
        try:
            meta = fetch_metadata(pkg)
        except Exception:
            continue
        for dep in latest_deps(meta):
            if dep not in seen:
                queue.append(dep)

    if len(chosen) < target:
        raise RuntimeError(f"only collected {len(chosen)} packages (target={target})")

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(chosen[:target], indent=2) + "\n")
    print(f"wrote {target} packages to {out_path}")
    return 0


# ── update-profiles: was update_binary_profiles.py ──

"""Generate binary package graph profiles from npm lockfile closures.

Profiles help bypass dependency expansion for common seed sets.
"""


import argparse
import json
import platform
import subprocess
import tempfile
from pathlib import Path
from typing import Dict, List, Optional, Tuple


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


def build_lockfile(seed_specs: List[str]) -> Dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="jhol-profile-") as td:
        root = Path(td)
        subprocess.run(["npm", "init", "-y"], cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(
            ["npm", "install", *seed_specs, "--package-lock-only", "--ignore-scripts"],
            cwd=root,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return json.loads((root / "package-lock.json").read_text())


def profile_key(seed_specs: List[str]) -> str:
    names = []
    for spec in seed_specs:
        if spec.startswith("@"):
            idx = spec.rfind("@")
            if idx > 0:
                names.append(spec[:idx])
            else:
                names.append(spec)
        else:
            if "@" in spec:
                names.append(spec.split("@", 1)[0])
            else:
                names.append(spec)
    names = sorted(set(names))
    return ",".join(names)


def closure_specs(seed_specs: List[str]) -> List[str]:
    lock = build_lockfile(seed_specs)
    packages_obj = lock.get("packages", {})
    if not isinstance(packages_obj, dict):
        return []

    current_os = npm_os_name()
    current_cpu = npm_cpu_name()

    specs: List[str] = []
    for path, entry_obj in packages_obj.items():
        if path == "" or not isinstance(entry_obj, dict):
            continue
        name = package_name_from_lock_path(path, entry_obj)
        version = entry_obj.get("version")
        if not isinstance(name, str) or not name:
            continue
        if not isinstance(version, str) or not version:
            continue
        if not field_allows_current(entry_obj.get("os"), current_os):
            continue
        if not field_allows_current(entry_obj.get("cpu"), current_cpu):
            continue
        specs.append(f"{name}@{version}")

    return sorted(set(specs))


def run_update_profiles(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "binary_packages.py", *argv]

    parser = argparse.ArgumentParser(description="Update binary profiles")
    parser.add_argument("--binary-dir", default="binary-packages")
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        help="Comma-separated seed specs (example: lodash,axios,chalk)",
    )
    args = parser.parse_args()

    binary_dir = Path(args.binary_dir)
    profiles_path = binary_dir / "profiles.json"

    profiles: Dict[str, List[str]] = {}
    if profiles_path.exists():
        raw = json.loads(profiles_path.read_text())
        if isinstance(raw, dict):
            profiles = {
                str(k): sorted(set(str(x) for x in v if isinstance(x, str)))
                for k, v in raw.items()
                if isinstance(v, list)
            }

    entries = args.profile or [
        "lodash,axios,chalk",
        "react,next,typescript,express",
    ]

    for raw in entries:
        seed_specs = [p.strip() for p in raw.split(",") if p.strip()]
        if not seed_specs:
            continue
        key = profile_key(seed_specs)
        specs = closure_specs(seed_specs)
        profiles[key] = specs
        print(f"profile {key}: {len(specs)} specs")

    profiles_path.write_text(json.dumps(dict(sorted(profiles.items())), indent=2) + "\n")
    print(f"wrote {profiles_path}")
    return 0


# ── precompute-unpacked: was precompute_unpacked_binary_cache.py ──

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


def run_precompute_unpacked(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "binary_packages.py", *argv]
    if argv and argv[0] in ("-h", "--help", "help"):
        print(
            "precompute_unpacked_binary_cache.py — unpack binary-packages/*.jhol into .unpacked/\n"
            "No CLI flags (forwards to binary_packages.py precompute-unpacked)."
        )
        return 0

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


_MODES = {
    "build": run_build,
    "bundle": run_bundle,
    "add-from-lock": run_add_from_lock,
    "expand-set": run_expand_set,
    "update-profiles": run_update_profiles,
    "precompute-unpacked": run_precompute_unpacked,
}

_HELP = """
Modes: build | bundle | add-from-lock | expand-set | update-profiles | precompute-unpacked
"""


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0] in ("-h", "--help", "help"):
        print(_HELP.strip())
        return 0
    mode = argv[0]
    if mode not in _MODES:
        raise SystemExit(f"unknown mode {mode!r}; choose: {', '.join(_MODES)}\n{_HELP}")
    rc = _MODES[mode](argv[1:])
    return int(rc or 0)


if __name__ == "__main__":
    raise SystemExit(main())
