#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def load_json(path: Path) -> dict:
    if not path.exists():
        raise FileNotFoundError(f"file not found: {path}")
    return json.loads(path.read_text())


def main() -> int:
    parser = argparse.ArgumentParser(description="Check Week-1 KPI guardrails")
    parser.add_argument("--report", required=True, help="Path to week1 baseline report JSON")
    parser.add_argument(
        "--config",
        default="benchmarks/week1_guardrails.json",
        help="Guardrail config JSON (default: benchmarks/week1_guardrails.json)",
    )
    args = parser.parse_args()

    try:
        report = load_json(Path(args.report))
        config = load_json(Path(args.config))
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    failures: list[str] = []

    fallback_total = (
        report.get("fallbackTelemetry", {})
        .get("data", {})
        .get("totalFallbacks", 0)
    )
    fallback_max = config.get("max_total_fallbacks", 0)
    if fallback_total > fallback_max:
        failures.append(
            f"fallback total {fallback_total} exceeds max_total_fallbacks {fallback_max}"
        )

    compatibility = report.get("compatibility", {})
    fixture_count = compatibility.get("fixtureCount", 0)
    valid_count = compatibility.get("validPackageJsonCount", 0)
    ratio = (valid_count / fixture_count) if fixture_count else 1.0
    min_ratio = float(config.get("min_fixture_valid_ratio", 1.0))
    if ratio < min_ratio:
        failures.append(
            f"fixture valid ratio {ratio:.2%} below min_fixture_valid_ratio {min_ratio:.2%}"
        )

    benchmark = report.get("benchmark", {})
    metric_source = benchmark.get("metricSource", "averages")
    metrics = benchmark.get("metrics", {})
    if not isinstance(metrics, dict) or not metrics:
        metrics = benchmark.get("averages", {})
        metric_source = "averages"
    required_metrics = config.get(
        "required_benchmark_metrics",
        ["jhol_cold_install", "jhol_warm_install", "jhol_offline_install"],
    )
    for metric in required_metrics:
        if metric not in metrics:
            failures.append(f"missing benchmark metric: {metric}")

    print("Week-1 guardrail check")
    print("=" * 24)
    print(f"fallback_total={fallback_total} (max={fallback_max})")
    print(
        f"fixture_valid_ratio={ratio:.2%} (valid={valid_count}, total={fixture_count}, min={min_ratio:.2%})"
    )
    print(f"benchmark_metric_source={metric_source}")
    print(f"required_metrics={required_metrics}")

    if failures:
        print("\nGuardrail check failed:")
        for f in failures:
            print(f"- {f}")
        return 1

    print("\nAll Week-1 guardrails satisfied.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
