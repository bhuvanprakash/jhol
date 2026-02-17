#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def baseline_metrics(payload: dict) -> dict:
    if not isinstance(payload, dict):
        return {}
    maybe_metrics = payload.get("metrics")
    if isinstance(maybe_metrics, dict):
        return maybe_metrics
    # Backward compatible: baseline itself is a metric map.
    return {k: v for k, v in payload.items() if isinstance(v, (int, float))}


def select_workload(results: dict, workload: str) -> dict:
    workloads = results.get("workloads")
    if isinstance(workloads, list) and workloads:
        if workload:
            for item in workloads:
                if isinstance(item, dict) and item.get("name") == workload:
                    return item
            raise RuntimeError(f"workload '{workload}' not found in results")
        first = workloads[0]
        if isinstance(first, dict):
            return first

    if workload:
        raise RuntimeError(
            f"results has no workloads array; cannot select workload '{workload}'"
        )

    return results


def metric_map(results: dict, metric_source: str, workload: str) -> tuple[str, dict]:
    selected = select_workload(results, workload)
    metrics = selected.get(metric_source)
    if not isinstance(metrics, dict):
        # Fallback for compatibility with older files.
        fallback = selected.get("averages")
        if isinstance(fallback, dict):
            return "averages", fallback
        return metric_source, {}
    return metric_source, metrics


def main() -> int:
    parser = argparse.ArgumentParser(description="Check benchmark regressions")
    parser.add_argument("--baseline", required=True, help="Path to baseline JSON")
    parser.add_argument("--results", required=True, help="Path to benchmark results JSON")
    parser.add_argument(
        "--metric-source",
        choices=["medians", "averages"],
        default="medians",
        help="Metric source from benchmark JSON (default: medians)",
    )
    parser.add_argument(
        "--workload",
        default="",
        help="Optional workload name for multi-workload benchmark JSON",
    )
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

    try:
        baseline = json.loads(baseline_path.read_text())
        results = json.loads(results_path.read_text())
    except Exception as exc:
        print(f"error: failed to parse JSON: {exc}", file=sys.stderr)
        return 2

    baseline_map = baseline_metrics(baseline)
    try:
        active_source, current_map = metric_map(results, args.metric_source, args.workload)
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if not baseline_map:
        print("error: baseline metrics are empty", file=sys.stderr)
        return 2
    if not current_map:
        print("error: current benchmark metrics are empty", file=sys.stderr)
        return 2

    failures = []

    print("Benchmark regression check")
    print("=" * 30)
    print(f"metric_source={active_source}")
    if args.workload:
        print(f"workload={args.workload}")
    for key, base in baseline_map.items():
        current = current_map.get(key)
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
