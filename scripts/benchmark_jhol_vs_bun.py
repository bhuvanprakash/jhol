#!/usr/bin/env python3
"""Benchmark Jhol (default JAGR) vs Bun on the jagr-instance-bench fixture."""

import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

FIXTURE = Path(__file__).parent.parent / "tests" / "fixtures" / "jagr-instance-bench"
JHOL_BIN = Path(__file__).parent.parent / "target" / "release" / "jhol"


def run(cmd, cwd, env=None, capture=True):
    kwargs = {"cwd": cwd}
    if env is not None:
        kwargs["env"] = env
    if capture:
        kwargs["capture_output"] = True
        kwargs["text"] = True
    return subprocess.run(cmd, **kwargs)


def measure(cmd, cwd, env=None):
    start = time.perf_counter()
    proc = run(cmd, cwd, env)
    elapsed = time.perf_counter() - start
    if proc.returncode != 0:
        return None, proc.stderr or proc.stdout or "non-zero exit"
    return elapsed, None


def clean_project_dir(project_dir):
    for name in ["node_modules", "package-lock.json", "bun.lock", "jhol-lock.json", ".jhol", ".jholrc"]:
        p = project_dir / name
        if p.exists():
            if p.is_dir():
                shutil.rmtree(p)
            else:
                p.unlink()


def dir_size(path, seen_inodes=None):
    # Follow symlinks to report logical node_modules footprint.
    if seen_inodes is None:
        seen_inodes = set()
    total = 0
    for entry in os.scandir(path):
        try:
            real = entry.stat(follow_symlinks=True)
        except (FileNotFoundError, OSError):
            # Dangling symlink — skip
            continue
        key = (real.st_dev, real.st_ino)
        if key in seen_inodes:
            continue
        seen_inodes.add(key)
        if real.st_size:
            total += real.st_size
        try:
            is_dir = entry.is_dir(follow_symlinks=True)
        except (FileNotFoundError, OSError):
            is_dir = False
        if is_dir:
            total += dir_size(entry.path, seen_inodes)
    return total


def check_correctness(project_dir):
    pkg = json.loads((project_dir / "package.json").read_text())
    deps = pkg.get("dependencies", {})
    missing = []
    for name in deps:
        target = project_dir / "node_modules" / name
        if not target.exists():
            missing.append(name)
    # Sanity-check a few well-known transitive dependencies that must exist.
    transitive = ["express/accepts", "axios/follow-redirects", "debug/ms"]
    for rel in transitive:
        parent, child = rel.split("/")
        target = project_dir / "node_modules" / parent / "node_modules" / child
        alt = project_dir / "node_modules" / child
        if not target.exists() and not alt.exists():
            missing.append(rel)
    return missing


def benchmark_manager(manager, repeats=3):
    with tempfile.TemporaryDirectory() as tmp:
        project_dir = Path(tmp) / "project"
        cache_dir = Path(tmp) / "jhol-cache"
        shutil.copytree(FIXTURE, project_dir)

        if manager == "jhol":
            cold_cmd = [str(JHOL_BIN), "install", "--quiet"]
            warm_cmd = [str(JHOL_BIN), "install", "--quiet"]
            env = os.environ.copy()
            env["JHOL_CACHE_DIR"] = str(cache_dir)
        elif manager == "bun":
            cold_cmd = ["bun", "install", "--no-install-scripts", "--silent"]
            warm_cmd = ["bun", "install", "--no-install-scripts", "--silent"]
            env = os.environ.copy()
        else:
            raise ValueError(manager)

        # Cold runs
        cold_times = []
        for i in range(repeats):
            clean_project_dir(project_dir)
            shutil.rmtree(cache_dir, ignore_errors=True)
            cache_dir.mkdir(parents=True, exist_ok=True)
            # Also clear bun's global cache for cold fairness
            if manager == "bun":
                subprocess.run(["bun", "pm", "cache", "rm"], capture_output=True)
                bun_cache = Path.home() / ".bun" / "install" / "cache"
                if bun_cache.exists():
                    shutil.rmtree(bun_cache)
            t, err = measure(cold_cmd, project_dir, env)
            if err:
                return {"error": f"cold run {i+1} failed: {err}"}
            cold_times.append(t)

        # Save lockfile from last cold run for warm
        saved_lock = None
        if manager == "jhol" and (project_dir / "package-lock.json").exists():
            saved_lock = project_dir / "package-lock.json.bench"
            shutil.copy(project_dir / "package-lock.json", saved_lock)
        elif manager == "bun" and (project_dir / "bun.lock").exists():
            saved_lock = project_dir / "bun.lock.bench"
            shutil.copy(project_dir / "bun.lock", saved_lock)

        # Warm runs
        warm_times = []
        for i in range(repeats):
            if (project_dir / "node_modules").exists():
                shutil.rmtree(project_dir / "node_modules")
            for name in ["package-lock.json", "bun.lock", "jhol-lock.json"]:
                p = project_dir / name
                if p.exists():
                    p.unlink()
            if saved_lock and saved_lock.exists():
                shutil.copy(saved_lock, project_dir / saved_lock.stem)
            t, err = measure(warm_cmd, project_dir, env)
            if err:
                return {"error": f"warm run {i+1} failed: {err}"}
            warm_times.append(t)

        missing = check_correctness(project_dir)
        size = dir_size(project_dir / "node_modules")

        return {
            "cold_s": statistics.median(cold_times),
            "warm_s": statistics.median(warm_times),
            "cold_times": cold_times,
            "warm_times": warm_times,
            "missing_deps": missing,
            "node_modules_bytes": size,
        }


def main():
    if not JHOL_BIN.exists():
        print(f"Error: jhol binary not found at {JHOL_BIN}", file=sys.stderr)
        print("Run: cargo build --release", file=sys.stderr)
        sys.exit(1)

    repeats = int(os.environ.get("BENCH_REPEATS", "3"))

    results = {}
    for manager in ["jhol", "bun"]:
        print(f"Benchmarking {manager}...")
        results[manager] = benchmark_manager(manager, repeats=repeats)

    print("\n=== Results ===")
    for manager, r in results.items():
        if "error" in r:
            print(f"{manager}: {r['error']}")
            continue
        print(f"{manager}:")
        print(f"  cold: {r['cold_s']:.3f}s  (raw: {[f'{t:.3f}' for t in r['cold_times']]})")
        print(f"  warm: {r['warm_s']:.3f}s  (raw: {[f'{t:.3f}' for t in r['warm_times']]})")
        print(f"  node_modules: {r['node_modules_bytes'] / (1024*1024):.1f} MB")
        if r["missing_deps"]:
            print(f"  MISSING: {r['missing_deps']}")

    jhol = results.get("jhol", {})
    bun = results.get("bun", {})
    if "error" not in jhol and "error" not in bun:
        print(f"\nJhol vs Bun:")
        print(f"  cold ratio (jhol/bun): {jhol['cold_s'] / bun['cold_s']:.2f}x")
        print(f"  warm ratio (jhol/bun): {jhol['warm_s'] / bun['warm_s']:.2f}x")


if __name__ == "__main__":
    main()
