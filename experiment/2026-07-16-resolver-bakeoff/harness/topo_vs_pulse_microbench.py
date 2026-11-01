#!/usr/bin/env python3
"""Microbench: cubic-ish dense boundary work vs bitset force-wave (Pulse shape).

Does not claim package-manager correctness. Measures structural cost shape only.
Writes JSON to stdout; optional --out path.
"""
from __future__ import annotations

import argparse
import json
import random
import time
from pathlib import Path


def dense_boundary_rank_work(n: int, seed: int = 0) -> float:
    """Simulate O(n^3)-ish elimination on dense n x n bit matrix over GF(2)."""
    rng = random.Random(seed)
    # Row-major bitrows as ints for small n; use lists of ints for larger.
    width = (n + 63) // 64
    rows = [[rng.getrandbits(64) for _ in range(width)] for _ in range(n)]
    t0 = time.perf_counter()
    rank = 0
    used = [False] * n
    for col in range(n):
        pivot = None
        word, bit = divmod(col, 64)
        mask = 1 << bit
        for r in range(n):
            if not used[r] and (rows[r][word] & mask):
                pivot = r
                break
        if pivot is None:
            continue
        used[pivot] = True
        rank += 1
        for r in range(n):
            if r != pivot and (rows[r][word] & mask):
                for w in range(width):
                    rows[r][w] ^= rows[pivot][w]
    _ = rank
    return time.perf_counter() - t0


def pulse_force_wave(packages: int, versions: int, edges: int, seed: int = 0) -> float:
    """Bitset domains + forward/backward support wipe to fixed point."""
    rng = random.Random(seed)
    # domain[p] = bitset of versions still alive
    full = (1 << versions) - 1 if versions < 63 else (1 << 62) - 1
    domain = [full for _ in range(packages)]
    # edges: (src, dst, support_mask_per_src_version) simplified as one mask per edge
    edge_list = []
    for _ in range(edges):
        s = rng.randrange(packages)
        d = rng.randrange(packages)
        if s == d:
            continue
        support = rng.getrandbits(min(versions, 62)) or 1
        edge_list.append((s, d, support))
    t0 = time.perf_counter()
    changed = True
    rounds = 0
    while changed and rounds < 64:
        changed = False
        rounds += 1
        # forward: if src alive, dst must intersect support
        for s, d, support in edge_list:
            if domain[s] == 0:
                continue
            new_d = domain[d] & support
            if new_d != domain[d]:
                domain[d] = new_d
                changed = True
        # backward: src dies if no overlap with dst
        for s, d, support in edge_list:
            if domain[d] & support == 0 and domain[s] != 0:
                domain[s] = 0
                changed = True
    _ = sum(bin(x).count("1") for x in domain)
    return time.perf_counter() - t0


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=None)
    args = ap.parse_args()

    sizes = [32, 64, 96, 128]
    dense = []
    for n in sizes:
        # median of 3
        times = sorted(dense_boundary_rank_work(n, seed=i) for i in range(3))
        dense.append({"n": n, "seconds_median": times[1], "model": "dense_gf2_elim"})

    pulse = []
    for packages, versions, edges in [
        (50, 16, 200),
        (200, 16, 800),
        (500, 16, 2000),
        (1000, 16, 4000),
    ]:
        times = sorted(
            pulse_force_wave(packages, versions, edges, seed=i) for i in range(3)
        )
        pulse.append(
            {
                "packages": packages,
                "versions": versions,
                "edges": edges,
                "seconds_median": times[1],
                "model": "bitset_force_wave",
            }
        )

    report = {
        "name": "topo_vs_pulse_microbench",
        "note": "Structural cost only. Not resolver correctness.",
        "dual_geometry_proxy_cubic": dense,
        "pulse_force_wave": pulse,
    }
    text = json.dumps(report, indent=2)
    print(text)
    if args.out:
        args.out.write_text(text)
        print(f"wrote {args.out}", flush=True)


if __name__ == "__main__":
    main()
