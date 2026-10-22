#!/usr/bin/env python3
"""Benchmark CI guardrails and artifact utilities (merged suite)."""
from __future__ import annotations

import sys


# ── regression: was check_benchmark_regression.py ──

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


def run_regression(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "guardrails.py", *argv]

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


# ── collect: was collect_baseline.py ──

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


def run_collect(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "guardrails.py", *argv]

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


# ── guardrails: was check_guardrails.py ──

import argparse
import json
import sys
from pathlib import Path


def load_json(path: Path) -> dict:
    if not path.exists():
        raise FileNotFoundError(f"file not found: {path}")
    return json.loads(path.read_text())


def run_guardrails(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "guardrails.py", *argv]

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


# ── fallback-trend: was check_fallback_trend.py ──

import argparse
import json
import sys
from pathlib import Path


def load_json(path: Path) -> dict:
    if not path.exists():
        raise FileNotFoundError(path)
    return json.loads(path.read_text())


def fallback_total(report: dict) -> int:
    return int(report.get("fallbackTelemetry", {}).get("data", {}).get("totalFallbacks", 0))


def fallback_reasons(report: dict) -> dict:
    d = report.get("fallbackTelemetry", {}).get("data", {}).get("reasons", {})
    return d if isinstance(d, dict) else {}


def run_fallback_trend(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "guardrails.py", *argv]

    parser = argparse.ArgumentParser(description="Fallback trend guardrail check")
    parser.add_argument("--current-report", required=True)
    parser.add_argument("--baseline-report", required=True)
    parser.add_argument("--config", default="benchmarks/fallback_trend_guardrails.json")
    args = parser.parse_args()

    try:
        current = load_json(Path(args.current_report))
        baseline = load_json(Path(args.baseline_report))
        config = load_json(Path(args.config))
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    current_total = fallback_total(current)
    baseline_total = fallback_total(baseline)
    delta = current_total - baseline_total
    current_reasons = fallback_reasons(current)

    failures: list[str] = []
    if current_total > int(config.get("maxTotalFallbacks", 0)):
        failures.append(
            f"current total {current_total} exceeds maxTotalFallbacks {config.get('maxTotalFallbacks')}"
        )
    if delta > int(config.get("maxIncreaseVsBaseline", 0)):
        failures.append(
            f"fallback increase {delta} exceeds maxIncreaseVsBaseline {config.get('maxIncreaseVsBaseline')}"
        )

    for reason, max_count in (config.get("maxReasonCount", {}) or {}).items():
        val = int(current_reasons.get(reason, 0))
        if val > int(max_count):
            failures.append(f"reason {reason} count {val} exceeds max {max_count}")

    print("Fallback trend check")
    print("====================")
    print(f"baseline_total={baseline_total}")
    print(f"current_total={current_total}")
    print(f"delta={delta}")

    if failures:
        print("\nFailures:")
        for failure in failures:
            print(f"- {failure}")
        return 1

    print("\nAll fallback trend guardrails satisfied.")
    return 0


# ── clean-artifacts: was clean_benchmark_artifacts.py ──

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


def run_clean_artifacts(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "guardrails.py", *argv]

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


# ── dashboard: was performance_dashboard.py ──

"""
Jhol Performance Dashboard Generator

Generates HTML dashboard from benchmark results for performance monitoring
and trend analysis.

Usage:
    python3 scripts/performance_dashboard.py --input benchmark-results.json --output dashboard.html
"""

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Any, Optional


def load_benchmark_results(path: str) -> Dict[str, Any]:
    """Load benchmark results from JSON file."""
    with open(path, 'r') as f:
        return json.load(f)


def calculate_statistics(results: List[Dict[str, Any]]) -> Dict[str, float]:
    """Calculate statistics from benchmark results."""
    if not results:
        return {}
    
    times = [r.get('duration_ms', 0) for r in results]
    return {
        'min': min(times),
        'max': max(times),
        'avg': sum(times) / len(times),
        'median': sorted(times)[len(times) // 2],
        'stddev': calculate_stddev(times),
    }


def calculate_stddev(values: List[float]) -> float:
    """Calculate standard deviation."""
    if len(values) < 2:
        return 0.0
    
    avg = sum(values) / len(values)
    variance = sum((x - avg) ** 2 for x in values) / len(values)
    return variance ** 0.5


def generate_trend_chart_data(results: List[Dict[str, Any]]) -> str:
    """Generate Chart.js data for trend chart."""
    labels = []
    data_points = []
    
    for i, result in enumerate(results):
        timestamp = result.get('timestamp', f'Run {i+1}')
        duration = result.get('duration_ms', 0)
        labels.append(timestamp)
        data_points.append(duration)
    
    return f"""
    {{
        labels: {json.dumps(labels)},
        datasets: [{{
            label: 'Install Duration (ms)',
            data: {json.dumps(data_points)},
            borderColor: 'rgb(75, 192, 192)',
            tension: 0.1,
            fill: true,
            backgroundColor: 'rgba(75, 192, 192, 0.2)'
        }}]
    }}
    """


def generate_comparison_table(current: Dict[str, Any], baseline: Optional[Dict[str, Any]]) -> str:
    """Generate HTML table comparing current vs baseline."""
    if not baseline:
        return "<p>No baseline data available for comparison.</p>"
    
    rows = []
    metrics = ['cold_install_ms', 'warm_install_ms', 'resolution_time_ms', 'cache_lookup_ms']
    
    for metric in metrics:
        current_val = current.get(metric, 0)
        baseline_val = baseline.get(metric, 0)
        
        if baseline_val > 0:
            change = ((current_val - baseline_val) / baseline_val) * 100
            change_class = 'improvement' if change < 0 else 'regression'
            change_str = f"{change:+.1f}%"
        else:
            change_class = ''
            change_str = 'N/A'
        
        rows.append(f"""
        <tr>
            <td>{metric.replace('_', ' ').title()}</td>
            <td>{current_val:.0f} ms</td>
            <td>{baseline_val:.0f} ms</td>
            <td class="{change_class}">{change_str}</td>
        </tr>
        """)
    
    return f"""
    <table>
        <thead>
            <tr>
                <th>Metric</th>
                <th>Current</th>
                <th>Baseline</th>
                <th>Change</th>
            </tr>
        </thead>
        <tbody>
            {''.join(rows)}
        </tbody>
    </table>
    """


def generate_dashboard_html(
    results: Dict[str, Any],
    baseline: Optional[Dict[str, Any]] = None,
    output_path: str = 'dashboard.html'
) -> None:
    """Generate complete HTML dashboard."""
    
    timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    version = results.get('version', 'unknown')
    
    # Extract scenario results
    scenarios = results.get('scenarios', [])
    overall_stats = results.get('overall_stats', {})
    
    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Jhol Performance Dashboard</title>
    <script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
    <style>
        :root {{
            --primary: #2563eb;
            --success: #16a34a;
            --warning: #ca8a04;
            --danger: #dc2626;
            --bg: #f8fafc;
            --card-bg: #ffffff;
            --text: #1e293b;
            --text-muted: #64748b;
        }}
        
        * {{
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }}
        
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: var(--bg);
            color: var(--text);
            line-height: 1.6;
            padding: 2rem;
        }}
        
        .container {{
            max-width: 1400px;
            margin: 0 auto;
        }}
        
        header {{
            margin-bottom: 2rem;
        }}
        
        h1 {{
            font-size: 2rem;
            font-weight: 700;
            color: var(--primary);
            margin-bottom: 0.5rem;
        }}
        
        .subtitle {{
            color: var(--text-muted);
            font-size: 0.875rem;
        }}
        
        .grid {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
            gap: 1.5rem;
            margin-bottom: 2rem;
        }}
        
        .card {{
            background: var(--card-bg);
            border-radius: 0.5rem;
            padding: 1.5rem;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }}
        
        .card h2 {{
            font-size: 1.25rem;
            margin-bottom: 1rem;
            color: var(--text);
        }}
        
        .stat {{
            display: flex;
            justify-content: space-between;
            padding: 0.75rem 0;
            border-bottom: 1px solid #e2e8f0;
        }}
        
        .stat:last-child {{
            border-bottom: none;
        }}
        
        .stat-label {{
            color: var(--text-muted);
        }}
        
        .stat-value {{
            font-weight: 600;
        }}
        
        .improvement {{
            color: var(--success);
        }}
        
        .regression {{
            color: var(--danger);
        }}
        
        table {{
            width: 100%;
            border-collapse: collapse;
            margin-top: 1rem;
        }}
        
        th, td {{
            padding: 0.75rem;
            text-align: left;
            border-bottom: 1px solid #e2e8f0;
        }}
        
        th {{
            background: #f1f5f9;
            font-weight: 600;
        }}
        
        .chart-container {{
            position: relative;
            height: 400px;
            margin-top: 1rem;
        }}
        
        .summary-grid {{
            display: grid;
            grid-template-columns: repeat(4, 1fr);
            gap: 1rem;
            margin-bottom: 2rem;
        }}
        
        .summary-card {{
            background: var(--card-bg);
            padding: 1.5rem;
            border-radius: 0.5rem;
            text-align: center;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }}
        
        .summary-value {{
            font-size: 2rem;
            font-weight: 700;
            color: var(--primary);
        }}
        
        .summary-label {{
            color: var(--text-muted);
            font-size: 0.875rem;
            margin-top: 0.5rem;
        }}
        
        footer {{
            margin-top: 3rem;
            text-align: center;
            color: var(--text-muted);
            font-size: 0.875rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>🚀 Jhol Performance Dashboard</h1>
            <p class="subtitle">Generated: {timestamp} | Version: {version}</p>
        </header>
        
        <div class="summary-grid">
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('total_scenarios', len(scenarios))}</div>
                <div class="summary-label">Total Scenarios</div>
            </div>
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('jagr_success_rate', 'N/A')}</div>
                <div class="summary-label">JAGR Success Rate</div>
            </div>
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('average_speedup', 'N/A')}</div>
                <div class="summary-label">Average Speedup</div>
            </div>
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('total_time_jagr_ms', 'N/A')} ms</div>
                <div class="summary-label">Total JAGR Time</div>
            </div>
        </div>
        
        <div class="grid">
            <div class="card">
                <h2>📊 Performance Trends</h2>
                <div class="chart-container">
                    <canvas id="trendChart"></canvas>
                </div>
            </div>
            
            <div class="card">
                <h2>📈 Scenario Comparison</h2>
                {generate_comparison_table(results, baseline)}
            </div>
        </div>
        
        <div class="card">
            <h2>📋 Detailed Results</h2>
            <table>
                <thead>
                    <tr>
                        <th>Scenario</th>
                        <th>Status</th>
                        <th>JAGR Time (ms)</th>
                        <th>Legacy Time (ms)</th>
                        <th>Speedup</th>
                    </tr>
                </thead>
                <tbody>
                    {generate_scenario_rows(scenarios)}
                </tbody>
            </table>
        </div>
        
        <footer>
            <p>Jhol Performance Dashboard | Built with ❤️ for better package management</p>
        </footer>
    </div>
    
    <script>
        // Trend Chart
        const trendCtx = document.getElementById('trendChart').getContext('2d');
        new Chart(trendCtx, {{
            type: 'line',
            data: {generate_trend_chart_data(scenarios)},
            options: {{
                responsive: true,
                maintainAspectRatio: false,
                plugins: {{
                    legend: {{
                        display: true,
                        position: 'top'
                    }}
                }},
                scales: {{
                    y: {{
                        beginAtZero: true,
                        title: {{
                            display: true,
                            text: 'Duration (ms)'
                        }}
                    }},
                    x: {{
                        title: {{
                            display: true,
                            text: 'Test Run'
                        }}
                    }}
                }}
            }}
        }});
    </script>
</body>
</html>
"""
    
    with open(output_path, 'w') as f:
        f.write(html)
    
    print(f"Dashboard generated: {output_path}")


def generate_scenario_rows(scenarios: List[Dict[str, Any]]) -> str:
    """Generate HTML table rows for scenarios."""
    rows = []
    
    for scenario in scenarios:
        name = scenario.get('name', 'Unknown')
        status = scenario.get('status', 'unknown')
        jagr_time = scenario.get('jagr_time_ms', 0)
        legacy_time = scenario.get('legacy_time_ms', 0)
        
        if legacy_time > 0 and jagr_time > 0:
            speedup = legacy_time / jagr_time
            speedup_str = f"{speedup:.2f}x"
        else:
            speedup_str = 'N/A'
        
        status_class = 'improvement' if status == 'success' else 'regression'
        
        rows.append(f"""
        <tr>
            <td>{name}</td>
            <td class="{status_class}">{status}</td>
            <td>{jagr_time:.0f}</td>
            <td>{legacy_time:.0f}</td>
            <td>{speedup_str}</td>
        </tr>
        """)
    
    return ''.join(rows)


def run_dashboard(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "guardrails.py", *argv]

    parser = argparse.ArgumentParser(description='Generate Jhol Performance Dashboard')
    parser.add_argument('--input', '-i', required=True, help='Input benchmark results JSON')
    parser.add_argument('--baseline', '-b', help='Baseline results for comparison')
    parser.add_argument('--output', '-o', default='dashboard.html', help='Output HTML file')
    
    args = parser.parse_args()
    
    results = load_benchmark_results(args.input)
    baseline = None
    
    if args.baseline:
        try:
            baseline = load_benchmark_results(args.baseline)
        except FileNotFoundError:
            print(f"Warning: Baseline file not found: {args.baseline}")
    
    generate_dashboard_html(results, baseline, args.output)


_MODES = {
    "regression": run_regression,
    "collect": run_collect,
    "guardrails": run_guardrails,
    "fallback-trend": run_fallback_trend,
    "clean-artifacts": run_clean_artifacts,
    "dashboard": run_dashboard,
}

_HELP = """
Modes: regression | collect | guardrails | fallback-trend | clean-artifacts | dashboard
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
