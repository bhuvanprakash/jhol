#!/usr/bin/env python3
"""Run live benchmark: jhol vs bun only (cold + warm, small + medium)."""
import os
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

JHOL = str(Path(__file__).parent.parent / "target" / "release" / "jhol")
RUNS = 5
SMALL_PKGS = ["lodash", "axios", "chalk"]
MEDIUM_PKGS = ["react", "typescript", "express"]


def get_jhol_cache_dirs():
    dirs = [
        os.path.expanduser("~/.jhol-cache"),
        os.path.join(os.environ.get("XDG_CACHE_HOME", os.path.expanduser("~/.cache")), "jhol"),
        os.path.expanduser("~/.jhol_cache"),
        os.path.expanduser("~/.jhol"),
    ]
    return dirs


def nuke_cache(tool):
    if tool == "jhol":
        for d in get_jhol_cache_dirs():
            shutil.rmtree(d, ignore_errors=True)
    elif tool == "bun":
        subprocess.run(["bun", "pm", "cache", "rm"], capture_output=True)


def nuke_workdir(workdir):
    for name in ["node_modules", ".jhol_cache", ".npm", ".yarn",
                 "bun.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"]:
        p = os.path.join(workdir, name)
        if os.path.isdir(p):
            shutil.rmtree(p, ignore_errors=True)
        elif os.path.isfile(p):
            try:
                os.remove(p)
            except OSError:
                pass


def run_install(tool, pkgs, workdir):
    env = os.environ.copy()
    if tool == "jhol":
        cmd = [JHOL, "install"] + pkgs
    else:
        cmd = ["bun", "add"] + pkgs
    t0 = time.perf_counter()
    subprocess.run(cmd, cwd=workdir, capture_output=True, env=env)
    return time.perf_counter() - t0


def setup_workdir(d):
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "package.json"), "w") as f:
        f.write('{"name":"bench-test","version":"1.0.0","dependencies":{}}\n')


def cold_bench(tool, pkgs, label):
    workdir = tempfile.mkdtemp(prefix=f"jhol_bench_{tool}_")
    setup_workdir(workdir)
    runs = []
    for i in range(RUNS):
        nuke_workdir(workdir)
        nuke_cache(tool)
        t = run_install(tool, pkgs, workdir)
        runs.append(round(t, 3))
    shutil.rmtree(workdir, ignore_errors=True)
    return runs, statistics.median(runs)


def warm_bench(tool, pkgs, label):
    workdir = tempfile.mkdtemp(prefix=f"jhol_bench_{tool}_")
    setup_workdir(workdir)
    run_install(tool, pkgs, workdir)
    runs = []
    for i in range(RUNS):
        shutil.rmtree(os.path.join(workdir, "node_modules"), ignore_errors=True)
        t = run_install(tool, pkgs, workdir)
        runs.append(round(t, 3))
    shutil.rmtree(workdir, ignore_errors=True)
    return runs, statistics.median(runs)


def main():
    if not os.path.isfile(JHOL) or not os.access(JHOL, os.X_OK):
        print(f"ERROR: jhol not found at {JHOL}. Run: cargo build --release")
        return 1
    if not shutil.which("bun"):
        print("ERROR: bun not in PATH")
        return 1

    print("Tools: jhol, bun")
    print(f"Runs per benchmark: {RUNS}\n")

    results = {}

    print("COLD INSTALL: small (lodash axios chalk)")
    for tool in ["jhol", "bun"]:
        runs, med = cold_bench(tool, SMALL_PKGS, "small")
        results["cold_small_" + tool] = med
        print(f"  {tool}: runs={runs}  median={med:.3f}s")

    print("\nCOLD INSTALL: medium (react typescript express)")
    for tool in ["jhol", "bun"]:
        runs, med = cold_bench(tool, MEDIUM_PKGS, "medium")
        results["cold_medium_" + tool] = med
        print(f"  {tool}: runs={runs}  median={med:.3f}s")

    print("\nWARM INSTALL: small")
    for tool in ["jhol", "bun"]:
        runs, med = warm_bench(tool, SMALL_PKGS, "small")
        results["warm_small_" + tool] = med
        print(f"  {tool}: runs={runs}  median={med:.3f}s")

    print("\nWARM INSTALL: medium")
    for tool in ["jhol", "bun"]:
        runs, med = warm_bench(tool, MEDIUM_PKGS, "medium")
        results["warm_medium_" + tool] = med
        print(f"  {tool}: runs={runs}  median={med:.3f}s")

    # Summary: jhol vs bun
    print("\n" + "=" * 60)
    print("JHOL vs BUN (median seconds)")
    print("=" * 60)
    print(f"{'Scenario':<20} {'jhol':>12} {'bun':>12} {'Ratio (jhol/bun)':>18}")
    print("-" * 60)
    for scenario in ["cold_small", "cold_medium", "warm_small", "warm_medium"]:
        j, b = results[scenario + "_jhol"], results[scenario + "_bun"]
        ratio = (j / b) if b and b > 0 else 0
        faster = "jhol faster" if ratio < 1 else "bun faster"
        print(f"{scenario:<20} {j:>10.3f}s {b:>10.3f}s {ratio:>10.2f}x ({faster})")
    print("=" * 60)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
