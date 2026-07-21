#!/usr/bin/env python3
"""Expand a pinned package list to a larger set using dependency graph crawl."""

from __future__ import annotations

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


def main() -> int:
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


if __name__ == "__main__":
    raise SystemExit(main())
