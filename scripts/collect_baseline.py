#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
from datetime import datetime, timezone
from pathlib import Path


def read_json(path: Path):
    if not path.exists():
         return None
    try:
        return json.loads(path.read_text())
    except Exception:
        return None


def select_benchmark_workload(benchmark: dict, workload: str) -> dict:
    if not isinstance(benchmark, dict):
        return {}

    workloads = benchmark.get("workloads")
    if isinstance(workloads, list) and workloads:
        if workload:
            for item in workloads:
                if isinstance(item, dict) and item.get("name") == workload:
                    return item
            return {}
        first = workloads[0]
        return first if isinstance(first, dict) else {}

    return benchmark


def choose_metric_map(benchmark_entry: dict, metric_source: str) -> tuple[str, dict]:
    if not isinstance(benchmark_entry, dict):
        return metric_source, {}
    data = benchmark_entry.get(metric_source)
    if isinstance(data, dict):
        return metric_source, data
    fallback = benchmark_entry.get("averages")
    if isinstance(fallback, dict):
        return "averages", fallback
    return metric_source, {}


def collect_fixture_compatibility(fixtures_dir: Path) -> dict:
    result = {
        "fixturesDir": str(fixtures_dir),
        "fixtureCount": 0,
        "validPackageJsonCount": 0,
        "fixtures": [],
    }
    if not fixtures_dir.exists():
        return result

    for item in sorted(fixtures_dir.iterdir()):
        if not item.is_dir():
            continue
        pkg = item / "package.json"
        valid = False
        if pkg.exists():
            try:
                json.loads(pkg.read_text())
                valid = True
            except Exception:
                valid = False

        result["fixtureCount"] += 1
        result["validPackageJsonCount"] += 1 if valid else 0
        result["fixtures"].append(
            {
                "name": item.name,
                "path": str(item),
                "packageJsonPresent": pkg.exists(),
                "packageJsonValid": valid,
            }
        )

    return result


def detect_cache_dir() -> Path:
    env_dir = os.environ.get("JHOL_CACHE_DIR")
    if env_dir:
        return Path(env_dir)
    home = Path.home()
    return home / ".jhol-cache"


def main() -> int:
    parser = argparse.ArgumentParser(description="Collect Week-1 baseline metrics artifact")
    parser.add_argument(
        "--benchmark-json",
        default="benchmark-results.json",
        help="Path to benchmark JSON produced by scripts/benchmark.py",
    )
    parser.add_argument(
        "--fixtures-dir",
        default="tests/fixtures",
        help="Fixtures directory used for compatibility baseline",
    )
    parser.add_argument(
        "--out",
        default="week1-baseline-report.json",
        help="Output report JSON path",
    )
    parser.add_argument(
        "--workload",
        default="",
        help="Optional workload name for multi-workload benchmark JSON",
    )
    parser.add_argument(
        "--metric-source",
        choices=["medians", "averages"],
        default="medians",
        help="Benchmark metric map to surface into KPI report (default: medians)",
    )
    args = parser.parse_args()

    benchmark_path = Path(args.benchmark_json)
    fixtures_dir = Path(args.fixtures_dir)
    out_path = Path(args.out)

    benchmark_json = read_json(benchmark_path) or {}
    benchmark_entry = select_benchmark_workload(benchmark_json, args.workload)
    metric_source, metrics = choose_metric_map(benchmark_entry, args.metric_source)
    fallback_path = detect_cache_dir() / "fallback_telemetry.json"
    fallback_json = read_json(fallback_path) or {
        "totalFallbacks": 0,
        "reasons": {},
        "byPackage": {},
    }

    compatibility = collect_fixture_compatibility(fixtures_dir)

    report = {
        "schemaVersion": "1",
        "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
        "gitSha": os.environ.get("GITHUB_SHA", "local"),
        "kpis": {
            "resolverParityPassRate": None,
            "fallbackRate": None,
            "enterpriseConfigPassRate": None,
            "installReliability": None,
            "performanceTrend": None,
        },
        "benchmark": {
            "source": str(benchmark_path),
            "suite": benchmark_json.get("suite"),
            "repeats": benchmark_json.get("repeats"),
            "metricSource": metric_source,
            "workload": benchmark_entry.get("name") or benchmark_json.get("workload"),
            "workloadsAvailable": [
                w.get("name")
                for w in benchmark_json.get("workloads", [])
                if isinstance(w, dict) and w.get("name")
            ],
            "metrics": metrics,
            "averages": benchmark_entry.get("averages", benchmark_json.get("averages", {})),
            "medians": benchmark_entry.get("medians", benchmark_json.get("medians", {})),
            "stats": benchmark_entry.get("stats", benchmark_json.get("stats", {})),
            "packages": benchmark_entry.get("packages", benchmark_json.get("packages", [])),
        },
        "compatibility": compatibility,
        "fallbackTelemetry": {
            "source": str(fallback_path),
            "data": fallback_json,
        },
    }

    out_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Wrote baseline report: {out_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
