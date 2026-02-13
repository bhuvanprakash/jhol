#!/usr/bin/env python3
"""
Simple benchmark harness for Jhol install performance.

Scenarios:
- cold_install: empty cache + install
- warm_install: reuse cache + reinstall
- offline_install: reuse cache + install --offline

Optional:
- compare npm cold/warm with --compare-npm

This script creates and cleans temporary benchmark directories automatically.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List


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


def package_name_only(spec: str) -> str:
    """Convert package spec to package name for CLI args.

    Examples:
    - lodash@4.17.21 -> lodash
    - @scope/pkg@1.2.3 -> @scope/pkg
    - react -> react
    """
    if spec.startswith("@"):
        idx = spec.rfind("@")
        return spec[:idx] if idx > 0 else spec
    if "@" in spec:
        return spec.split("@", 1)[0]
    return spec


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
        default=["lodash", "axios", "chalk"],
        help="Package names/specs to benchmark",
    )
    parser.add_argument(
        "--repeats",
        type=int,
        default=3,
        help="Runs per scenario (default: 3)",
    )
    parser.add_argument(
        "--compare-npm",
        action="store_true",
        help="Also benchmark npm cold/warm for quick comparison",
    )
    parser.add_argument(
        "--json-out",
        default="",
        help="Write raw benchmark results to JSON file",
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

    all_results: Dict[str, List[float]] = {}
    jhol_packages = [package_name_only(p) for p in args.packages]

    with tempfile.TemporaryDirectory(prefix="jhol-bench-") as td:
        root = Path(td)
        project_dir = root / "project"
        cache_dir = root / "cache"
        project_dir.mkdir(parents=True, exist_ok=True)
        cache_dir.mkdir(parents=True, exist_ok=True)
        write_package_json(project_dir, args.packages)

        env = os.environ.copy()
        env["JHOL_CACHE_DIR"] = str(cache_dir)
        env["JHOL_QUIET"] = "1"

        # Cold
        all_results["jhol_cold_install"] = []
        for _ in range(args.repeats):
            if (project_dir / "node_modules").exists():
                shutil.rmtree(project_dir / "node_modules")
            res = run_cmd(
                [str(jhol_bin), "install", *jhol_packages, "--native-only", "-q"],
                project_dir,
                env,
            )
            ensure_ok(res, "jhol cold install")
            all_results["jhol_cold_install"].append(res.seconds)
            # Reset cache for each cold run
            if cache_dir.exists():
                shutil.rmtree(cache_dir)
            cache_dir.mkdir(parents=True, exist_ok=True)

        # Prime cache once for warm/offline
        if (project_dir / "node_modules").exists():
            shutil.rmtree(project_dir / "node_modules")
        res_prime = run_cmd(
            [str(jhol_bin), "install", *jhol_packages, "--native-only", "-q"],
            project_dir,
            env,
        )
        ensure_ok(res_prime, "jhol prime cache")

        # Warm
        all_results["jhol_warm_install"] = []
        for _ in range(args.repeats):
            if (project_dir / "node_modules").exists():
                shutil.rmtree(project_dir / "node_modules")
            res = run_cmd(
                [str(jhol_bin), "install", *jhol_packages, "--native-only", "-q"],
                project_dir,
                env,
            )
            ensure_ok(res, "jhol warm install")
            all_results["jhol_warm_install"].append(res.seconds)

        # Offline
        all_results["jhol_offline_install"] = []
        for _ in range(args.repeats):
            if (project_dir / "node_modules").exists():
                shutil.rmtree(project_dir / "node_modules")
            res = run_cmd(
                [
                    str(jhol_bin),
                    "install",
                    *jhol_packages,
                    "--native-only",
                    "--offline",
                    "-q",
                ],
                project_dir,
                env,
            )
            ensure_ok(res, "jhol offline install")
            all_results["jhol_offline_install"].append(res.seconds)

        if args.compare_npm:
            npm_env = os.environ.copy()
            all_results["npm_cold_install"] = []
            all_results["npm_warm_install"] = []

            for _ in range(args.repeats):
                if (project_dir / "node_modules").exists():
                    shutil.rmtree(project_dir / "node_modules")
                if (project_dir / "package-lock.json").exists():
                    (project_dir / "package-lock.json").unlink()
                res = run_cmd(["npm", "install", "--silent"], project_dir, npm_env)
                ensure_ok(res, "npm cold install")
                all_results["npm_cold_install"].append(res.seconds)

            for _ in range(args.repeats):
                if (project_dir / "node_modules").exists():
                    shutil.rmtree(project_dir / "node_modules")
                res = run_cmd(["npm", "install", "--silent"], project_dir, npm_env)
                ensure_ok(res, "npm warm install")
                all_results["npm_warm_install"].append(res.seconds)

    def avg(xs: List[float]) -> float:
        return sum(xs) / len(xs) if xs else 0.0

    print("\nJhol benchmark results (seconds)")
    print("=" * 44)
    for key, vals in all_results.items():
        print(f"{key:22} avg={avg(vals):8.3f}  runs={', '.join(f'{v:.3f}' for v in vals)}")

    if args.json_out:
        out = {
            "packages": args.packages,
            "repeats": args.repeats,
            "results": all_results,
            "averages": {k: avg(v) for k, v in all_results.items()},
        }
        Path(args.json_out).write_text(json.dumps(out, indent=2) + "\n")
        print(f"\nSaved JSON report: {args.json_out}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
