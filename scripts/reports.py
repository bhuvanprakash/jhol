#!/usr/bin/env python3
"""Engineering reports (merged suite)."""
from __future__ import annotations

import sys


# ── enterprise-npmrc: was enterprise_npmrc_report.py ──

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path


def parse_npmrc(contents: str) -> dict:
    out = {
        "registry": None,
        "scopedRegistries": {},
        "hasAuthToken": False,
        "proxy": None,
        "httpsProxy": None,
        "strictSsl": None,
        "cafile": None,
    }
    for raw in contents.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or line.startswith(";") or "=" not in line:
            continue
        key, value = [x.strip() for x in line.split("=", 1)]
        if key == "registry":
            out["registry"] = value.rstrip("/")
        elif key.endswith(":registry") and key.startswith("@"):
            out["scopedRegistries"][key[:-9]] = value.rstrip("/")
        elif key.endswith(":_authToken"):
            out["hasAuthToken"] = True
        elif key == "proxy":
            out["proxy"] = value
        elif key in ("https-proxy", "https_proxy"):
            out["httpsProxy"] = value
        elif key == "strict-ssl":
            out["strictSsl"] = value.lower() == "true"
        elif key == "cafile":
            out["cafile"] = value
    return out


def run_enterprise(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "reports.py", *argv]

    parser = argparse.ArgumentParser(description="Enterprise .npmrc matrix report")
    parser.add_argument("--config", default="benchmarks/enterprise_guardrails.json")
    parser.add_argument("--out", default="enterprise-npmrc-report.json")
    args = parser.parse_args()

    guardrails = {"minPassRate": 1.0, "minScenarioCount": 4}
    cfg = Path(args.config)
    if cfg.exists():
        guardrails.update(json.loads(cfg.read_text()))

    scenarios = [
        {
            "name": "scoped_registry_and_token",
            "npmrc": "registry=https://registry.npmjs.org/\n@acme:registry=https://npm.acme.local/\n//npm.acme.local/:_authToken=abc\n",
            "expect": {"hasAuthToken": True, "scopedCount": 1},
        },
        {
            "name": "proxy_and_https_proxy",
            "npmrc": "proxy=http://proxy.local:8080\nhttps-proxy=http://proxy.local:8443\n",
            "expect": {"proxy": "http://proxy.local:8080", "httpsProxy": "http://proxy.local:8443"},
        },
        {
            "name": "strict_ssl_false",
            "npmrc": "strict-ssl=false\n",
            "expect": {"strictSsl": False},
        },
        {
            "name": "cafile_present",
            "npmrc": "cafile=/etc/ssl/certs/acme.pem\n",
            "expect": {"cafile": "/etc/ssl/certs/acme.pem"},
        },
    ]

    rows = []
    failures: list[str] = []
    for s in scenarios:
        parsed = parse_npmrc(s["npmrc"])
        expect = s["expect"]
        ok = True
        if "hasAuthToken" in expect and parsed["hasAuthToken"] != expect["hasAuthToken"]:
            ok = False
        if "scopedCount" in expect and len(parsed["scopedRegistries"]) != expect["scopedCount"]:
            ok = False
        for key in ("proxy", "httpsProxy", "strictSsl", "cafile"):
            if key in expect and parsed.get(key) != expect[key]:
                ok = False
        if not ok:
            failures.append(f"scenario {s['name']} failed")
        rows.append({"name": s["name"], "parsed": parsed, "expect": expect, "pass": ok})

    total = len(rows)
    passed = len([r for r in rows if r["pass"]])
    pass_rate = (passed / total) if total else 0.0
    if total < int(guardrails.get("minScenarioCount", 4)):
        failures.append(
            f"scenario count {total} below minScenarioCount {guardrails.get('minScenarioCount')}"
        )
    if pass_rate < float(guardrails.get("minPassRate", 1.0)):
        failures.append(
            f"pass rate {pass_rate:.2%} below threshold {float(guardrails.get('minPassRate', 1.0)):.2%}"
        )

    report = {
        "schemaVersion": "1",
        "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
        "guardrails": guardrails,
        "totals": {
            "scenarioCount": total,
            "passed": passed,
            "failed": total - passed,
            "passRate": pass_rate,
        },
        "rows": rows,
        "failures": failures,
        "status": "pass" if not failures else "fail",
    }
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    print(f"scenarios={total} passed={passed} pass_rate={pass_rate:.2%}")
    print(f"report={args.out}")
    if failures:
        print("Failures:")
        for f in failures:
            print(f"- {f}")
        return 1
    return 0


# ── framework-compat: was framework_compat_report.py ──

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


def run_framework(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "reports.py", *argv]

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


# ── resolver-fixtures: was resolver_fixture_report.py ──

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


def run_resolver(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "reports.py", *argv]

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


_MODES = {
    "enterprise-npmrc": run_enterprise,
    "framework-compat": run_framework,
    "resolver-fixtures": run_resolver,
}

_HELP = """
Modes: enterprise-npmrc | framework-compat | resolver-fixtures
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
