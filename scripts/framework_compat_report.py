#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path


def load_json(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text())
    except Exception:
        return {}


def main() -> int:
    parser = argparse.ArgumentParser(description="Framework compatibility matrix report")
    parser.add_argument("--fixtures-dir", default="tests/fixtures")
    parser.add_argument("--matrix", default="benchmarks/framework_matrix.json")
    parser.add_argument("--config", default="benchmarks/framework_guardrails.json")
    parser.add_argument("--out", default="framework-compat-report.json")
    args = parser.parse_args()

    fixtures_dir = Path(args.fixtures_dir)
    matrix = load_json(Path(args.matrix))
    guardrails = {"minPassRate": 1.0, "minFrameworkCount": 1}
    guardrails.update(load_json(Path(args.config)))

    frameworks = matrix.get("frameworks", [])
    rows = []
    failures: list[str] = []

    for fw in frameworks:
        name = fw.get("name", "unknown")
        fixture = fw.get("fixture", "")
        pkg_path = fixtures_dir / fixture / "package.json"
        parsed = {}
        valid_json = False
        try:
            parsed = json.loads(pkg_path.read_text())
            valid_json = True
        except Exception:
            valid_json = False

        deps = parsed.get("dependencies", {}) if isinstance(parsed, dict) else {}
        dev_deps = parsed.get("devDependencies", {}) if isinstance(parsed, dict) else {}
        workspaces = parsed.get("workspaces") if isinstance(parsed, dict) else None

        req_deps = fw.get("requiredDependencies", [])
        missing_deps = [d for d in req_deps if d not in deps and d not in dev_deps]
        needs_workspaces = bool(fw.get("requiredWorkspaces", False))
        workspace_ok = (not needs_workspaces) or bool(workspaces)

        passed = valid_json and not missing_deps and workspace_ok
        if not passed:
            if not valid_json:
                failures.append(f"{name}: invalid package.json ({pkg_path})")
            if missing_deps:
                failures.append(f"{name}: missing deps {missing_deps}")
            if needs_workspaces and not workspace_ok:
                failures.append(f"{name}: missing required workspaces field")

        rows.append(
            {
                "framework": name,
                "fixture": fixture,
                "packageJson": str(pkg_path),
                "validPackageJson": valid_json,
                "requiredDependencies": req_deps,
                "missingDependencies": missing_deps,
                "requiredWorkspaces": needs_workspaces,
                "workspacePresent": bool(workspaces),
                "pass": passed,
            }
        )

    total = len(rows)
    passed = len([r for r in rows if r["pass"]])
    pass_rate = (passed / total) if total else 0.0

    if total < int(guardrails.get("minFrameworkCount", 1)):
        failures.append(
            f"framework count {total} below minFrameworkCount {guardrails.get('minFrameworkCount')}"
        )
    min_pass_rate = float(guardrails.get("minPassRate", 1.0))
    if pass_rate < min_pass_rate:
        failures.append(f"pass rate {pass_rate:.2%} below threshold {min_pass_rate:.2%}")

    report = {
        "schemaVersion": "1",
        "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
        "fixturesDir": str(fixtures_dir),
        "matrix": str(args.matrix),
        "guardrails": guardrails,
        "totals": {
            "frameworkCount": total,
            "passed": passed,
            "failed": total - passed,
            "passRate": pass_rate,
        },
        "rows": rows,
        "failures": failures,
        "status": "pass" if not failures else "fail",
    }

    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    print(f"frameworks={total} passed={passed} pass_rate={pass_rate:.2%}")
    print(f"report={args.out}")
    if failures:
        print("Failures:")
        for f in failures:
            print(f"- {f}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
