#!/usr/bin/env python3
from __future__ import annotations

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


def main() -> int:
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


if __name__ == "__main__":
    raise SystemExit(main())
