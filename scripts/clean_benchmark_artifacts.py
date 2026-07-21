#!/usr/bin/env python3
from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


def is_tracked(path: Path) -> bool:
    proc = subprocess.run(
        ["git", "ls-files", "--error-unmatch", str(path)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return proc.returncode == 0


def should_keep(path: Path, keep_exact: set[str], keep_prefixes: list[str]) -> bool:
    name = path.name
    if name in keep_exact:
        return True
    for prefix in keep_prefixes:
        if name.startswith(prefix):
            return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Clean generated benchmark/report artifacts while preserving canonical files"
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="Delete matched files. Without this flag, print planned deletions only.",
    )
    parser.add_argument(
        "--include-reports",
        action="store_true",
        help="Also remove generated markdown reports like JHOL_*.md.",
    )
    parser.add_argument(
        "--keep",
        action="append",
        default=[],
        help="Filename prefix to keep (can be passed multiple times).",
    )
    args = parser.parse_args()

    keep_exact = {
        "benchmark-comprehensive-small.json",
        "benchmark-comprehensive-small.md",
        "benchmark-comprehensive-medium.json",
        "benchmark-comprehensive-medium.md",
        "benchmark-after-idea1-small.json",
        "benchmark-after-idea1-small.md",
    }
    keep_prefixes = set(args.keep)

    root = Path.cwd()
    candidates: list[Path] = []

    for pattern in ["benchmark-*.json", "benchmark-*.md"]:
        candidates.extend(sorted(root.glob(pattern)))

    if args.include_reports:
        for pattern in [
            "JHOL_*.md",
            "PERFORMANCE_*.md",
            "OPTIMIZATION_*.md",
            "NEXT_STEPS_*.md",
            "LIGHTNING_FAST_*.md",
            "PUBGRUB_*.md",
        ]:
            candidates.extend(sorted(root.glob(pattern)))

    delete_list: list[Path] = []
    for path in candidates:
        if should_keep(path, keep_exact, list(keep_prefixes)):
            continue
        if is_tracked(path):
            continue
        delete_list.append(path)

    if not delete_list:
        print("No generated artifacts to clean.")
        return 0

    action = "Deleting" if args.apply else "Would delete"
    print(f"{action} {len(delete_list)} file(s):")
    for path in delete_list:
        print(f" - {path}")

    if args.apply:
        for path in delete_list:
            path.unlink(missing_ok=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
