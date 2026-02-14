#!/usr/bin/env python3
"""Generate an initial resolver parity fixture report.

Week-2 implementation target:
- track fixture corpus growth
- verify edge-case coverage categories exist
- ensure each fixture has a snapshot descriptor
- emit machine-readable pass-rate report
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path


EDGE_TYPES = {
    "peer": lambda pj: bool(pj.get("peerDependencies")),
    "optional": lambda pj: bool(pj.get("optionalDependencies")),
    "overrides": lambda pj: bool(pj.get("overrides")),
    "workspaces": lambda pj: bool(pj.get("workspaces")),
}


def build_actual_graph(package_json: dict) -> dict:
    root = {
        "name": package_json.get("name", ""),
        "version": package_json.get("version", ""),
    }
    edges = []
    for dep_type in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ]:
        deps = package_json.get(dep_type, {})
        if isinstance(deps, dict):
            for dep_name, dep_spec in sorted(deps.items()):
                edges.append(
                    {
                        "from": "$root",
                        "to": str(dep_name),
                        "spec": str(dep_spec),
                        "type": dep_type,
                    }
                )

    overrides = package_json.get("overrides", {})
    if not isinstance(overrides, dict):
        overrides = {}
    workspaces = package_json.get("workspaces", [])
    if isinstance(workspaces, str):
        workspaces = [workspaces]
    if not isinstance(workspaces, list):
        workspaces = []

    return {
        "root": root,
        "edges": edges,
        "overrides": {k: str(v) for k, v in sorted(overrides.items())},
        "workspaces": sorted([str(w) for w in workspaces]),
    }


def normalize_graph(graph: dict) -> dict:
    root = graph.get("root", {}) if isinstance(graph, dict) else {}
    edges = graph.get("edges", []) if isinstance(graph, dict) else []
    overrides = graph.get("overrides", {}) if isinstance(graph, dict) else {}
    workspaces = graph.get("workspaces", []) if isinstance(graph, dict) else []

    normalized_edges = []
    if isinstance(edges, list):
        for edge in edges:
            if not isinstance(edge, dict):
                continue
            normalized_edges.append(
                {
                    "from": str(edge.get("from", "$root")),
                    "to": str(edge.get("to", "")),
                    "spec": str(edge.get("spec", "")),
                    "type": str(edge.get("type", "dependencies")),
                }
            )
    normalized_edges.sort(key=lambda e: (e["from"], e["to"], e["type"], e["spec"]))

    normalized_overrides = {}
    if isinstance(overrides, dict):
        normalized_overrides = {str(k): str(v) for k, v in sorted(overrides.items())}

    normalized_workspaces = []
    if isinstance(workspaces, list):
        normalized_workspaces = sorted([str(w) for w in workspaces])

    return {
        "root": {
            "name": str(root.get("name", "")),
            "version": str(root.get("version", "")),
        },
        "edges": normalized_edges,
        "overrides": normalized_overrides,
        "workspaces": normalized_workspaces,
    }


def semantic_diff(expected_graph: dict | None, actual_graph: dict) -> dict:
    if not expected_graph or not isinstance(expected_graph, dict):
        return {
            "matches": False,
            "reason": "missing expectedGraph in snapshot",
            "root": {},
            "edges": {"missing": [], "extra": []},
            "overrides": {"missing": {}, "extra": {}},
            "workspaces": {"missing": [], "extra": []},
        }

    expected = normalize_graph(expected_graph)
    actual = normalize_graph(actual_graph)

    root_diff = {
        "expected": expected["root"],
        "actual": actual["root"],
        "matches": expected["root"] == actual["root"],
    }

    exp_edges = {
        (e["from"], e["to"], e["type"], e["spec"]): e for e in expected["edges"]
    }
    act_edges = {
        (e["from"], e["to"], e["type"], e["spec"]): e for e in actual["edges"]
    }
    missing_edges = [exp_edges[k] for k in sorted(exp_edges.keys()) if k not in act_edges]
    extra_edges = [act_edges[k] for k in sorted(act_edges.keys()) if k not in exp_edges]

    missing_overrides = {
        k: v
        for k, v in expected["overrides"].items()
        if actual["overrides"].get(k) != v
    }
    extra_overrides = {
        k: v
        for k, v in actual["overrides"].items()
        if expected["overrides"].get(k) != v
    }

    exp_ws = set(expected["workspaces"])
    act_ws = set(actual["workspaces"])
    missing_workspaces = sorted(list(exp_ws - act_ws))
    extra_workspaces = sorted(list(act_ws - exp_ws))

    matches = (
        root_diff["matches"]
        and not missing_edges
        and not extra_edges
        and not missing_overrides
        and not extra_overrides
        and not missing_workspaces
        and not extra_workspaces
    )

    return {
        "matches": matches,
        "reason": None if matches else "semantic graph mismatch",
        "root": root_diff,
        "edges": {"missing": missing_edges, "extra": extra_edges},
        "overrides": {"missing": missing_overrides, "extra": extra_overrides},
        "workspaces": {
            "missing": missing_workspaces,
            "extra": extra_workspaces,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Resolver fixture parity report")
    parser.add_argument("--fixtures-dir", default="tests/fixtures")
    parser.add_argument("--snapshots-dir", default="tests/resolver-snapshots")
    parser.add_argument("--out", default="resolver-parity-report.json")
    parser.add_argument("--min-pass-rate", type=float, default=1.0)
    parser.add_argument(
        "--config",
        default="benchmarks/resolver_parity_guardrails.json",
        help="Optional guardrail config JSON",
    )
    args = parser.parse_args()

    fixtures_dir = Path(args.fixtures_dir)
    snapshots_dir = Path(args.snapshots_dir)
    out_path = Path(args.out)

    guardrails = {
        "minPassRate": args.min_pass_rate,
        "minFixtureCount": 0,
        "requiredEdgeTypes": list(EDGE_TYPES.keys()),
    }
    config_path = Path(args.config)
    if config_path.exists():
        try:
            loaded = json.loads(config_path.read_text())
            if isinstance(loaded, dict):
                guardrails.update({k: v for k, v in loaded.items() if v is not None})
        except Exception:
            pass

    failures: list[str] = []
    fixtures = []
    coverage_counts = {k: 0 for k in EDGE_TYPES.keys()}
    semantic_match_count = 0

    if not fixtures_dir.exists():
        print(f"error: fixtures dir does not exist: {fixtures_dir}", file=sys.stderr)
        return 2

    for item in sorted(fixtures_dir.iterdir()):
        if not item.is_dir():
            continue

        fixture_name = item.name
        package_json_path = item / "package.json"
        snapshot_path = snapshots_dir / f"{fixture_name}.json"

        package_json_valid = False
        package_json = {}
        if package_json_path.exists():
            try:
                package_json = json.loads(package_json_path.read_text())
                package_json_valid = True
            except Exception:
                package_json_valid = False

        edge_types = []
        if package_json_valid:
            for edge_type, detector in EDGE_TYPES.items():
                if detector(package_json):
                    edge_types.append(edge_type)
                    coverage_counts[edge_type] += 1

        snapshot_exists = snapshot_path.exists()
        snapshot_valid = False
        snapshot_json = {}
        if snapshot_exists:
            try:
                snapshot_json = json.loads(snapshot_path.read_text())
                snapshot_valid = isinstance(snapshot_json, dict)
            except Exception:
                snapshot_valid = False

        actual_graph = build_actual_graph(package_json) if package_json_valid else {}
        diff = (
            semantic_diff(snapshot_json.get("expectedGraph"), actual_graph)
            if package_json_valid and snapshot_exists and snapshot_valid
            else {
                "matches": False,
                "reason": "snapshot missing/invalid or package.json invalid",
                "root": {},
                "edges": {"missing": [], "extra": []},
                "overrides": {"missing": {}, "extra": {}},
                "workspaces": {"missing": [], "extra": []},
            }
        )
        fixture_pass = package_json_valid and snapshot_exists and snapshot_valid and diff["matches"]
        if diff.get("matches"):
            semantic_match_count += 1

        if not fixture_pass:
            if not package_json_valid:
                failures.append(f"{fixture_name}: invalid package.json")
            if not snapshot_exists:
                failures.append(f"{fixture_name}: missing snapshot {snapshot_path}")
            if snapshot_exists and not snapshot_valid:
                failures.append(f"{fixture_name}: invalid snapshot JSON {snapshot_path}")
            if package_json_valid and snapshot_exists and snapshot_valid and not diff["matches"]:
                failures.append(f"{fixture_name}: semantic snapshot mismatch")

        fixtures.append(
            {
                "name": fixture_name,
                "path": str(item),
                "packageJsonPath": str(package_json_path),
                "snapshotPath": str(snapshot_path),
                "packageJsonValid": package_json_valid,
                "snapshotPresent": snapshot_exists,
                "snapshotValid": snapshot_valid,
                "edgeTypes": edge_types,
                "actualGraph": actual_graph,
                "semanticDiff": diff,
                "pass": fixture_pass,
            }
        )

    total = len(fixtures)
    passed = len([f for f in fixtures if f["pass"]])
    pass_rate = (passed / total) if total else 0.0

    required_edge_types = [
        edge_type
        for edge_type in guardrails.get("requiredEdgeTypes", list(EDGE_TYPES.keys()))
        if edge_type in EDGE_TYPES
    ]
    missing_edge_coverage = [
        edge_type for edge_type in required_edge_types if coverage_counts.get(edge_type, 0) == 0
    ]
    if missing_edge_coverage:
        failures.append(
            "missing edge coverage categories: " + ", ".join(missing_edge_coverage)
        )

    min_fixture_count = int(guardrails.get("minFixtureCount", 0))
    if total < min_fixture_count:
        failures.append(f"fixture count {total} below minFixtureCount {min_fixture_count}")

    min_pass_rate = float(guardrails.get("minPassRate", args.min_pass_rate))
    if pass_rate < min_pass_rate:
        failures.append(
            f"pass rate {pass_rate:.2%} below threshold {min_pass_rate:.2%}"
        )

    report = {
        "schemaVersion": "1",
        "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
        "fixturesDir": str(fixtures_dir),
        "snapshotsDir": str(snapshots_dir),
        "totals": {
            "fixtureCount": total,
            "passed": passed,
            "failed": total - passed,
            "passRate": pass_rate,
        },
        "coverage": {
            "edgeTypeCounts": coverage_counts,
            "missingEdgeCoverage": missing_edge_coverage,
        },
        "semantic": {
            "matched": semantic_match_count,
            "mismatched": total - semantic_match_count,
            "matchRate": (semantic_match_count / total) if total else 0.0,
        },
        "fixtures": fixtures,
        "guardrails": guardrails,
        "failures": failures,
        "status": "pass" if not failures else "fail",
    }

    out_path.write_text(json.dumps(report, indent=2) + "\n")

    print("Resolver fixture parity report")
    print("=" * 31)
    print(f"fixtures={total} passed={passed} pass_rate={pass_rate:.2%}")
    print(f"edge_coverage={coverage_counts}")
    print(f"report={out_path}")

    if failures:
        print("\nFailures:")
        for failure in failures:
            print(f"- {failure}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
