#!/usr/bin/env python3
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


def main() -> int:
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


if __name__ == "__main__":
    raise SystemExit(main())
