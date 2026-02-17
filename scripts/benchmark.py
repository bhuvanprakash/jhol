#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Tuple


DEFAULT_SMALL_PACKAGES = ["lodash", "axios", "chalk"]
DEFAULT_MEDIUM_PACKAGES = ["react", "next", "typescript", "express"]
DEFAULT_FIXTURE_WORKLOADS = [
    "express-app",
    "react-app",
    "typescript-app",
    "next-app",
]


@dataclass
class RunResult:
    name: str
    seconds: float
    exit_code: int
    stdout: str
    stderr: str


def run_cmd(cmd: List[str], cwd: Path, env: Dict[str, str]) -> RunResult:
    start = time.perf_counter()
    p = subprocess.run(
        cmd,
        cwd=str(cwd),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    elapsed = time.perf_counter() - start
    return RunResult(
        name=" ".join(cmd),
        seconds=elapsed,
        exit_code=p.returncode,
        stdout=p.stdout,
        stderr=p.stderr,
    )


def ensure_ok(res: RunResult, label: str) -> None:
    if res.exit_code != 0:
        raise RuntimeError(
            f"{label} failed (exit {res.exit_code})\n"
            f"cmd: {res.name}\n"
            f"stdout:\n{res.stdout}\n"
            f"stderr:\n{res.stderr}"
        )


def is_offline_cache_miss(res: RunResult) -> bool:
    text = f"{res.stdout}\n{res.stderr}".lower()
    return "offline mode" in text and "not in cache" in text


def write_package_json(project_dir: Path, packages: List[str]) -> None:
    deps = {}
    for spec in packages:
        if "@" in spec and not spec.startswith("@"):
            name, version = spec.split("@", 1)
        else:
            idx = spec.rfind("@")
            if idx > 0:
                name, version = spec[:idx], spec[idx + 1 :]
            else:
                name, version = spec, "latest"
        deps[name] = version
    data = {
        "name": "jhol-bench-fixture",
        "version": "1.0.0",
        "private": True,
        "dependencies": deps,
    }
    (project_dir / "package.json").write_text(json.dumps(data, indent=2) + "\n")


def avg(xs: List[float]) -> float:
    return sum(xs) / len(xs) if xs else 0.0


def med(xs: List[float]) -> float:
    return statistics.median(xs) if xs else 0.0


def mad(xs: List[float]) -> float:
    if not xs:
        return 0.0
    center = med(xs)
    deviations = [abs(x - center) for x in xs]
    return med(deviations)


def percentile_95(xs: List[float]) -> float:
    if not xs:
        return 0.0
    if len(xs) == 1:
        return xs[0]
    return statistics.quantiles(xs, n=100, method="inclusive")[94]


def outlier_count_iqr(xs: List[float]) -> int:
    if len(xs) < 4:
        return 0
    q = statistics.quantiles(xs, n=4, method="inclusive")
    q1 = q[0]
    q3 = q[2]
    iqr = q3 - q1
    lower = q1 - 1.5 * iqr
    upper = q3 + 1.5 * iqr
    return sum(1 for x in xs if x < lower or x > upper)


def summarize_results(results: Dict[str, List[float]]) -> Tuple[Dict[str, float], Dict[str, float], Dict[str, Dict[str, Any]]]:
    averages = {k: avg(v) for k, v in results.items()}
    medians = {k: med(v) for k, v in results.items()}
    stats = {
        k: {
            "average": avg(v),
            "median": med(v),
            "mad": mad(v),
            "p95": percentile_95(v),
            "min": min(v) if v else 0.0,
            "max": max(v) if v else 0.0,
            "outlier_count": outlier_count_iqr(v),
            "runs": v,
        }
        for k, v in results.items()
    }
    return averages, medians, stats


def fixture_specs(fixtures_dir: Path, fixture_name: str) -> List[str]:
    package_json = fixtures_dir / fixture_name / "package.json"
    if not package_json.exists():
        raise RuntimeError(f"fixture package.json not found: {package_json}")
    try:
        parsed = json.loads(package_json.read_text())
    except Exception as exc:
        raise RuntimeError(f"invalid fixture package.json {package_json}: {exc}") from exc

    deps: Dict[str, str] = {}
    for section in ["dependencies", "devDependencies"]:
        values = parsed.get(section, {})
        if isinstance(values, dict):
            for name, spec in values.items():
                deps[str(name)] = str(spec)

    return [f"{name}@{spec}" for name, spec in sorted(deps.items())]


def resolve_workloads(
    suite: str,
    packages: List[str] | None,
    fixtures_dir: Path,
    fixture_workloads: List[str],
) -> List[Tuple[str, List[str]]]:
    if packages:
        return [("custom", packages)]

    if suite == "small":
        return [("small", list(DEFAULT_SMALL_PACKAGES))]
    if suite == "medium":
        return [("medium", list(DEFAULT_MEDIUM_PACKAGES))]

    workloads: List[Tuple[str, List[str]]] = []
    if suite == "all":
        workloads.extend(
            [
                ("small", list(DEFAULT_SMALL_PACKAGES)),
                ("medium", list(DEFAULT_MEDIUM_PACKAGES)),
            ]
        )

    selected_fixtures = fixture_workloads or list(DEFAULT_FIXTURE_WORKLOADS)
    for fixture in selected_fixtures:
        specs = fixture_specs(fixtures_dir, fixture)
        if not specs:
            continue
        workloads.append((f"fixture:{fixture}", specs))

    if not workloads:
        raise RuntimeError("No benchmark workloads resolved. Provide --packages or a valid --suite.")
    return workloads


def compare_tools(compare_npm: bool, compare_all: bool) -> List[str]:
    if compare_all:
        return ["npm", "yarn", "pnpm", "bun"]
    if compare_npm:
        return ["npm"]
    return []


def run_workload(
    workload_name: str,
    packages: List[str],
    repeats: int,
    jhol_bin: Path,
    compare_tool_names: List[str],
) -> Dict[str, List[float]]:
    all_results: Dict[str, List[float]] = {}

    with tempfile.TemporaryDirectory(prefix=f"jhol-bench-{workload_name.replace(':', '-')}-") as td:
        root = Path(td)
        project_dir = root / "project"
        cache_dir = root / "cache"
        project_dir.mkdir(parents=True, exist_ok=True)
        cache_dir.mkdir(parents=True, exist_ok=True)
        write_package_json(project_dir, packages)

        env = os.environ.copy()
        env["JHOL_CACHE_DIR"] = str(cache_dir)
        env["JHOL_QUIET"] = "1"

        all_results["jhol_cold_install"] = []
        for _ in range(repeats):
            if (project_dir / "node_modules").exists():
                shutil.rmtree(project_dir / "node_modules")
            res = run_cmd(
                [str(jhol_bin), "install", *packages, "--native-only", "-q"],
                project_dir,
                env,
            )
            ensure_ok(res, "jhol cold install")
            all_results["jhol_cold_install"].append(res.seconds)
            if cache_dir.exists():
                shutil.rmtree(cache_dir)
            cache_dir.mkdir(parents=True, exist_ok=True)

        if (project_dir / "node_modules").exists():
            shutil.rmtree(project_dir / "node_modules")
        res_prime = run_cmd(
            [str(jhol_bin), "install", *packages, "--native-only", "-q"],
            project_dir,
            env,
        )
        ensure_ok(res_prime, "jhol prime cache")

        all_results["jhol_warm_install"] = []
        for _ in range(repeats):
            if (project_dir / "node_modules").exists():
                shutil.rmtree(project_dir / "node_modules")
            res = run_cmd(
                [str(jhol_bin), "install", *packages, "--native-only", "-q"],
                project_dir,
                env,
            )
            ensure_ok(res, "jhol warm install")
            all_results["jhol_warm_install"].append(res.seconds)

        all_results["jhol_offline_install"] = []
        offline_supported = True
        for _ in range(repeats):
            if (project_dir / "node_modules").exists():
                shutil.rmtree(project_dir / "node_modules")
            res = run_cmd(
                [
                    str(jhol_bin),
                    "install",
                    *packages,
                    "--native-only",
                    "--offline",
                    "-q",
                ],
                project_dir,
                env,
            )

            if res.exit_code == 0:
                all_results["jhol_offline_install"].append(res.seconds)
                continue

            if is_offline_cache_miss(res):
                if (project_dir / "node_modules").exists():
                    shutil.rmtree(project_dir / "node_modules")
                retry_prime = run_cmd(
                    [str(jhol_bin), "install", *packages, "--native-only", "-q"],
                    project_dir,
                    env,
                )
                ensure_ok(retry_prime, "jhol offline re-prime cache")
                if (project_dir / "node_modules").exists():
                    shutil.rmtree(project_dir / "node_modules")
                retry_offline = run_cmd(
                    [
                        str(jhol_bin),
                        "install",
                        *packages,
                        "--native-only",
                        "--offline",
                        "-q",
                    ],
                    project_dir,
                    env,
                )
                if retry_offline.exit_code == 0:
                    all_results["jhol_offline_install"].append(retry_offline.seconds)
                    continue

                if is_offline_cache_miss(retry_offline):
                    print(
                        "warning: jhol offline benchmark skipped (cache-only mode not available for this package set)",
                        file=sys.stderr,
                    )
                    offline_supported = False
                    break

                ensure_ok(retry_offline, "jhol offline install")

            ensure_ok(res, "jhol offline install")

        if not offline_supported:
            all_results.pop("jhol_offline_install", None)

        for tool in compare_tool_names:
            if shutil.which(tool) is None:
                print(f"warning: {tool} not found in PATH; skipping")
                continue

            env_tool = os.environ.copy()
            all_results[f"{tool}_cold_install"] = []
            all_results[f"{tool}_warm_install"] = []

            for _ in range(repeats):
                cleanup_installs(project_dir)
                cleanup_lockfiles(project_dir)
                res = run_cmd(tool_install_cmd(tool), project_dir, env_tool)
                ensure_ok(res, f"{tool} cold install")
                all_results[f"{tool}_cold_install"].append(res.seconds)

            for _ in range(repeats):
                cleanup_installs(project_dir)
                res = run_cmd(tool_install_cmd(tool), project_dir, env_tool)
                ensure_ok(res, f"{tool} warm install")
                all_results[f"{tool}_warm_install"].append(res.seconds)

    return all_results


def main() -> int:
    parser = argparse.ArgumentParser(description="Benchmark Jhol install performance")
    parser.add_argument(
        "--jhol-bin",
        default="target/release/jhol",
        help="Path to jhol binary (default: target/release/jhol)",
    )
    parser.add_argument(
        "--packages",
        nargs="+",
        default=None,
        help="Package names/specs to benchmark. If set, overrides --suite.",
    )
    parser.add_argument(
        "--suite",
        choices=["small", "medium", "fixtures", "all"],
        default="small",
        help="Built-in benchmark workload suite (default: small)",
    )
    parser.add_argument(
        "--fixtures-dir",
        default="tests/fixtures",
        help="Fixtures directory used by --suite fixtures/all",
    )
    parser.add_argument(
        "--fixture-workloads",
        nargs="*",
        default=[],
        help=(
            "Fixture directory names to include for --suite fixtures/all "
            f"(default: {', '.join(DEFAULT_FIXTURE_WORKLOADS)})"
        ),
    )
    parser.add_argument(
        "--repeats",
        type=int,
        default=5,
        help="Runs per scenario (default: 5)",
    )
    parser.add_argument(
        "--compare-npm",
        action="store_true",
        help="Also benchmark npm cold/warm for quick comparison",
    )
    parser.add_argument(
        "--compare-all",
        action="store_true",
        help="Benchmark npm, yarn, pnpm, and bun cold/warm alongside jhol",
    )
    parser.add_argument(
        "--json-out",
        default="",
        help="Write raw benchmark results to JSON file",
    )
    parser.add_argument(
        "--markdown-out",
        default="",
        help="Write a markdown benchmark summary table",
    )
    args = parser.parse_args()

    jhol_bin = Path(args.jhol_bin).expanduser().resolve()
    if not jhol_bin.exists():
        print(f"error: jhol binary not found at {jhol_bin}", file=sys.stderr)
        print("hint: build it first with: cargo build --release", file=sys.stderr)
        return 2

    if args.repeats < 1:
        print("error: --repeats must be >= 1", file=sys.stderr)
        return 2

    compare_tool_names = compare_tools(args.compare_npm, args.compare_all)
    try:
        workloads = resolve_workloads(
            args.suite,
            args.packages,
            Path(args.fixtures_dir),
            args.fixture_workloads,
        )
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    workload_reports: List[Dict[str, Any]] = []
    print("\nJhol benchmark results (seconds)")
    print("=" * 44)

    for workload_name, packages in workloads:
        results = run_workload(
            workload_name=workload_name,
            packages=packages,
            repeats=args.repeats,
            jhol_bin=jhol_bin,
            compare_tool_names=compare_tool_names,
        )
        averages, medians, stats = summarize_results(results)
        workload_reports.append(
            {
                "name": workload_name,
                "packages": packages,
                "results": results,
                "averages": averages,
                "medians": medians,
                "stats": stats,
            }
        )

        print(f"\n[{workload_name}] packages={', '.join(packages)}")
        for key, vals in results.items():
            s = stats[key]
            print(
                (
                    f"{key:22} avg={s['average']:8.3f} med={s['median']:8.3f} "
                    f"mad={s['mad']:8.3f} p95={s['p95']:8.3f} outliers={s['outlier_count']:2d} "
                    f"runs={', '.join(f'{v:.3f}' for v in vals)}"
                )
            )

    if args.json_out:
        out: Dict[str, Any] = {
            "schemaVersion": "2",
            "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
            "suite": args.suite,
            "repeats": args.repeats,
            "compareTools": compare_tool_names,
            "workloads": workload_reports,
        }
        # Backward compatibility for existing scripts expecting top-level single-workload fields.
        if len(workload_reports) == 1:
            first = workload_reports[0]
            out.update(
                {
                    "workload": first["name"],
                    "packages": first["packages"],
                    "results": first["results"],
                    "averages": first["averages"],
                    "medians": first["medians"],
                    "stats": first["stats"],
                }
            )
        Path(args.json_out).write_text(json.dumps(out, indent=2) + "\n")
        print(f"\nSaved JSON report: {args.json_out}")

    if args.markdown_out:
        lines: List[str] = []
        lines.append("# Jhol benchmark summary")
        lines.append("")
        lines.append(f"Suite: `{args.suite}`")
        lines.append(f"Repeats: `{args.repeats}`")
        lines.append("")
        for workload in workload_reports:
            lines.append(f"## Workload: `{workload['name']}`")
            lines.append("")
            lines.append(f"Packages: `{', '.join(workload['packages'])}`")
            lines.append("")
            lines.append("| Metric | Average (s) | Median (s) | MAD (s) | P95 (s) | Outliers | Runs |")
            lines.append("|---|---:|---:|---:|---:|---:|---|")
            for key in sorted(workload["results"].keys()):
                stat = workload["stats"][key]
                runs = ", ".join(f"{v:.3f}" for v in workload["results"][key])
                lines.append(
                    (
                        f"| {key} | {stat['average']:.3f} | {stat['median']:.3f} | "
                        f"{stat['mad']:.3f} | {stat['p95']:.3f} | {stat['outlier_count']} | {runs} |"
                    )
                )
            lines.append("")
        Path(args.markdown_out).write_text("\n".join(lines) + "\n")
        print(f"Saved markdown summary: {args.markdown_out}")

    return 0


def tool_install_cmd(tool: str) -> List[str]:
    if tool == "npm":
        return ["npm", "install", "--silent"]
    if tool == "yarn":
        return ["yarn", "install", "--silent"]
    if tool == "pnpm":
        return ["pnpm", "install", "--silent"]
    if tool == "bun":
        return ["bun", "install", "--silent"]
    raise ValueError(f"Unsupported tool: {tool}")


def cleanup_installs(project_dir: Path) -> None:
    for p in [project_dir / "node_modules"]:
        if p.exists():
            shutil.rmtree(p)


def cleanup_lockfiles(project_dir: Path) -> None:
    for p in [
        project_dir / "package-lock.json",
        project_dir / "yarn.lock",
        project_dir / "pnpm-lock.yaml",
        project_dir / "bun.lock",
        project_dir / "bun.lockb",
    ]:
        if p.exists():
            p.unlink()


if __name__ == "__main__":
    raise SystemExit(main())
