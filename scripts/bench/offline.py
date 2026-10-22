#!/usr/bin/env python3
"""
JHOL Offline Benchmark Suite v1.0

Measures PURE JHOL performance with ZERO network dependency.
Perfect for slow internet connections!

This benchmark:
1. Pre-populates ALL caches (one-time network call)
2. Runs ALL benchmarks in OFFLINE mode
3. Measures TRUE local performance (no network variance)
"""
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
from typing import Any, Dict, List

DEFAULT_SMALL_PACKAGES = ["lodash", "axios", "chalk"]
DEFAULT_MEDIUM_PACKAGES = ["react", "next", "typescript", "express"]


@dataclass
class RunResult:
    name: str
    seconds: float
    exit_code: int
    stdout: str
    stderr: str


def run_cmd(cmd: List[str], cwd: Path, env: Dict[str, str]) -> RunResult:
    """Run command and measure time."""
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


def clear_all_jhol_caches() -> None:
    """Clear ALL JHOL cache directories."""
    global_cache_paths = [
        Path.home() / ".cache" / "jhol-resolution-cache",
        Path.home() / ".cache" / "jhol-tarball-store",
        Path.home() / ".cache" / "jhol-extracted-store",
        Path.home() / ".cache" / "jhol-store",
        Path.home() / ".cache" / "jhol",
        Path.home() / ".local" / "share" / "jhol",
    ]
    for cache_path in global_cache_paths:
        if cache_path.exists():
            try:
                shutil.rmtree(cache_path)
                cache_path.mkdir(parents=True, exist_ok=True)
            except Exception as e:
                print(f"Warning: Failed to clear {cache_path}: {e}", file=sys.stderr)


def run_offline_benchmark(
    workload_name: str,
    packages: List[str],
    repeats: int,
    jhol_bin: Path,
) -> Dict[str, List[float]]:
    """Run offline benchmark (ZERO network, pure local performance)."""
    all_results: Dict[str, List[float]] = {}

    with tempfile.TemporaryDirectory(prefix=f"jhol-offline-bench-{workload_name}-") as td:
        root = Path(td)
        project_dir = root / "project"
        cache_dir = root / "cache"
        project_dir.mkdir(parents=True, exist_ok=True)
        cache_dir.mkdir(parents=True, exist_ok=True)
        write_package_json(project_dir, packages)

        env = os.environ.copy()
        env["JHOL_CACHE_DIR"] = str(cache_dir)
        env["JHOL_QUIET"] = "1"

        # STEP 1: Pre-populate caches (ONE-TIME network call)
        print(f"\n📦 Pre-populating caches for {workload_name}...")
        prewarm_cmd = [str(jhol_bin), "install", *packages, "-q"]
        res = run_cmd(prewarm_cmd, project_dir, env)
        ensure_ok(res, "Pre-populate caches")
        print(f"✅ Caches populated! Now running offline benchmarks...")

        # STEP 2: Clear project, keep global caches
        cleanup_installs(project_dir)
        cleanup_lockfiles(project_dir)

        # STEP 3: OFFLINE install benchmark (ZERO network!)
        print(f"\n🚀 Running OFFLINE cold installs (no network)...")
        all_results["offline_cold_install"] = []
        offline_cmd = [str(jhol_bin), "install", "--offline", *packages, "-q"]

        for i in range(repeats):
            cleanup_installs(project_dir)
            cleanup_lockfiles(project_dir)
            res = run_cmd(offline_cmd, project_dir, env)
            ensure_ok(res, "Offline cold install")
            all_results["offline_cold_install"].append(res.seconds)
            print(f"  Run {i+1}/{repeats}: {res.seconds:.3f}s")

        # STEP 4: WARM install benchmark (all cache hits)
        print(f"\n🔥 Running WARM installs (all cache hits)...")
        all_results["warm_install"] = []
        warm_cmd = [str(jhol_bin), "install", *packages, "-q"]

        # Prime the cache
        cleanup_installs(project_dir)
        cleanup_lockfiles(project_dir)
        res_prime = run_cmd(warm_cmd, project_dir, env)
        ensure_ok(res_prime, "Prime warm cache")

        for i in range(repeats):
            cleanup_installs(project_dir)
            cleanup_lockfiles(project_dir)
            res = run_cmd(warm_cmd, project_dir, env)
            ensure_ok(res, "Warm install")
            all_results["warm_install"].append(res.seconds)
            print(f"  Run {i+1}/{repeats}: {res.seconds:.3f}s")

    return all_results


def summarize_results(results: Dict[str, List[float]]) -> Dict[str, Any]:
    """Calculate statistics."""
    stats = {}
    for key, values in results.items():
        if not values:
            continue
        stats[key] = {
            "average": sum(values) / len(values),
            "median": statistics.median(values),
            "min": min(values),
            "max": max(values),
            "std_dev": statistics.stdev(values) if len(values) > 1 else 0.0,
            "variance_pct": (statistics.stdev(values) / statistics.median(values) * 100) if len(values) > 1 and statistics.median(values) > 0 else 0.0,
            "runs": values,
        }
    return stats


def main() -> int:
    parser = argparse.ArgumentParser(description="JHOL Offline Benchmark (ZERO network)")
    parser.add_argument(
        "--jhol-bin",
        default="target/release/jhol",
        help="Path to jhol binary",
    )
    parser.add_argument(
        "--packages",
        nargs="+",
        default=None,
        help="Package names/specs to benchmark",
    )
    parser.add_argument(
        "--suite",
        choices=["small", "medium"],
        default="small",
        help="Benchmark workload suite",
    )
    parser.add_argument(
        "--repeats",
        type=int,
        default=5,
        help="Runs per scenario",
    )
    parser.add_argument(
        "--json-out",
        default="",
        help="Write results to JSON file",
    )
    parser.add_argument(
        "--markdown-out",
        default="",
        help="Write markdown summary",
    )
    args = parser.parse_args()

    jhol_bin = Path(args.jhol_bin).expanduser().resolve()
    if not jhol_bin.exists():
        print(f"error: jhol binary not found at {jhol_bin}", file=sys.stderr)
        print("hint: cargo build --release", file=sys.stderr)
        return 2

    if args.repeats < 1:
        print("error: --repeats must be >= 1", file=sys.stderr)
        return 2

    # Select packages
    if args.packages:
        workload_name = "custom"
        packages = args.packages
    elif args.suite == "small":
        workload_name = "small"
        packages = DEFAULT_SMALL_PACKAGES
    else:
        workload_name = "medium"
        packages = DEFAULT_MEDIUM_PACKAGES

    print("=" * 60)
    print("🎯 JHOL OFFLINE BENCHMARK SUITE v1.0")
    print("=" * 60)
    print(f"Workload: {workload_name} ({len(packages)} packages)")
    print(f"Repeats: {args.repeats}")
    print(f"Mode: OFFLINE (ZERO network variance)")
    print("=" * 60)

    # Clear all caches first for true cold test
    print("\n🧹 Clearing ALL JHOL caches...")
    clear_all_jhol_caches()
    print("✅ Caches cleared!")

    # Run benchmark
    results = run_offline_benchmark(
        workload_name,
        packages,
        args.repeats,
        jhol_bin,
    )

    # Calculate statistics
    stats = summarize_results(results)

    # Print results
    print("\n" + "=" * 60)
    print("📊 RESULTS")
    print("=" * 60)

    for key, key_stats in stats.items():
        print(f"\n{key}:")
        print(f"  Median: {key_stats['median']:.3f}s")
        print(f"  Min: {key_stats['min']:.3f}s")
        print(f"  Max: {key_stats['max']:.3f}s")
        print(f"  Variance: {key_stats['variance_pct']:.1f}%")

    # Save JSON
    if args.json_out:
        report = {
            "schemaVersion": "1.0",
            "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
            "suite": args.suite,
            "repeats": args.repeats,
            "mode": "offline",
            "workload": {
                "name": workload_name,
                "packages": packages,
            },
            "results": results,
            "stats": stats,
        }
        with open(args.json_out, "w") as f:
            json.dump(report, f, indent=2)
        print(f"\n💾 JSON report: {args.json_out}")

    # Save markdown
    if args.markdown_out:
        lines = [
            "# JHOL Offline Benchmark Results",
            "",
            f"**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M:%S UTC')}",
            f"**Mode:** OFFLINE (ZERO network)",
            f"**Workload:** {workload_name} ({len(packages)} packages)",
            "",
            "## Results",
            "",
            "| Metric | Median | Min | Max | Variance |",
            "|--------|--------|-----|-----|----------|",
        ]
        for key, key_stats in stats.items():
            lines.append(
                f"| {key} | {key_stats['median']:.3f}s | {key_stats['min']:.3f}s | {key_stats['max']:.3f}s | {key_stats['variance_pct']:.1f}% |"
            )
        with open(args.markdown_out, "w") as f:
            f.write("\n".join(lines))
        print(f"📝 Markdown report: {args.markdown_out}")

    print("\n✅ Benchmark complete!")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
