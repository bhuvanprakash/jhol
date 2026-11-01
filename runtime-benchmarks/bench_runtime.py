#!/usr/bin/env python3
"""Benchmark jhol-runtime vs Bun runtime on micro-workloads."""

import json
import os
import subprocess
import statistics
import sys
import time
from pathlib import Path

BENCH_DIR = Path(__file__).parent.resolve()
ROOT = BENCH_DIR.parent
WARMUP = 3
RUNS = 10


def run_jhol(file: Path) -> dict:
    cmd = [
        "cargo",
        "run",
        "-p",
        "jhol-runtime",
        "--release",
        "--example",
        "bench",
        "--",
        str(file),
        str(WARMUP),
        str(RUNS),
    ]
    result = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        print("jhol stderr:", result.stderr, file=sys.stderr)
        raise RuntimeError(f"jhol failed for {file.name}")
    line = result.stdout.strip().splitlines()[-1]
    return json.loads(line)


def run_external(runtime: str, executable: str, file: Path) -> dict:
    times = []
    for _ in range(WARMUP):
        subprocess.run([executable, str(file)], capture_output=True)

    for _ in range(RUNS):
        start = time.perf_counter()
        result = subprocess.run([executable, str(file)], capture_output=True)
        elapsed = (time.perf_counter() - start) * 1000.0
        if result.returncode != 0:
            print(f"{runtime} stderr:", result.stderr.decode(), file=sys.stderr)
            raise RuntimeError(f"{runtime} failed for {file.name}")
        times.append(elapsed)

    times.sort()
    n = len(times)
    median = times[n // 2] if n % 2 else (times[n // 2 - 1] + times[n // 2]) / 2
    return {
        "runtime": runtime,
        "file": file.name,
        "runs": RUNS,
        "avg_ms": statistics.mean(times),
        "median_ms": median,
        "min_ms": min(times),
        "max_ms": max(times),
    }


def run_bun(file: Path) -> dict:
    return run_external("bun", "bun", file)


def run_node(file: Path) -> dict:
    return run_external("node", "node", file)


def main():
    files = sorted(BENCH_DIR.glob("*.js"))
    if not files:
        print("No benchmark files found in", BENCH_DIR, file=sys.stderr)
        sys.exit(1)

    print(f"Benchmarking {len(files)} workloads (warmup={WARMUP}, runs={RUNS})...")
    print()

    rows = []
    for file in files:
        print(f"Running {file.name} ...")
        jhol = run_jhol(file)
        bun = run_bun(file)
        node = run_node(file)
        rows.append((jhol, bun, node))

    print()
    print("| benchmark | jhol avg (ms) | jhol median | bun avg (ms) | bun median | node avg (ms) | node median | jhol/bun | jhol/node |")
    print("|-----------|---------------|-------------|--------------|------------|---------------|-------------|----------|-----------|")
    for jhol, bun, node in rows:
        ratio_bun = jhol["median_ms"] / bun["median_ms"] if bun["median_ms"] else float("inf")
        ratio_node = jhol["median_ms"] / node["median_ms"] if node["median_ms"] else float("inf")
        print(
            f"| {jhol['file']:<9} | {jhol['avg_ms']:>13.3f} | {jhol['median_ms']:>11.3f} | "
            f"{bun['avg_ms']:>12.3f} | {bun['median_ms']:>10.3f} | "
            f"{node['avg_ms']:>13.3f} | {node['median_ms']:>11.3f} | "
            f"{ratio_bun:>8.2f} | {ratio_node:>9.2f} |"
        )

    print()
    print("Raw JSON:")
    print(json.dumps([jhol for jhol, _, _ in rows] + [bun for _, bun, _ in rows] + [node for _, _, node in rows], indent=2))


if __name__ == "__main__":
    main()
