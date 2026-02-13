#!/usr/bin/env python3
"""Simple benchmark regression checker.

Compares benchmark JSON output from scripts/benchmark.py against a baseline.
Fails if any metric exceeds baseline by threshold percent.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description="Check benchmark regressions")
    parser.add_argument("--baseline", required=True, help="Path to baseline JSON")
    parser.add_argument("--results", required=True, help="Path to benchmark results JSON")
    parser.add_argument(
        "--threshold",
        type=float,
        default=0.25,
        help="Allowed slowdown ratio (default: 0.25 = 25%%)",
    )
    args = parser.parse_args()

    baseline_path = Path(args.baseline)
    results_path = Path(args.results)

    if not baseline_path.exists():
        print(f"error: baseline file not found: {baseline_path}", file=sys.stderr)
        return 2
    if not results_path.exists():
        print(f"error: results file not found: {results_path}", file=sys.stderr)
        return 2

    baseline = json.loads(baseline_path.read_text())
    results = json.loads(results_path.read_text())
    averages = results.get("averages", {})

    failures = []

    print("Benchmark regression check")
    print("=" * 30)
    for key, base in baseline.items():
        current = averages.get(key)
        if current is None:
            failures.append(f"Missing metric in results: {key}")
            continue
        limit = base * (1.0 + args.threshold)
        status = "OK"
        if current > limit:
            status = "REGRESSION"
            failures.append(
                f"{key}: current={current:.3f}s > allowed={limit:.3f}s (baseline={base:.3f}s, threshold={args.threshold:.0%})"
            )
        print(
            f"{key:22} baseline={base:7.3f}s current={current:7.3f}s allowed={limit:7.3f}s -> {status}"
        )

    if failures:
        print("\nRegression check failed:")
        for f in failures:
            print(f"- {f}")
        return 1

    print("\nNo regressions detected.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
