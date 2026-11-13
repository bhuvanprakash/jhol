#!/usr/bin/env python3
"""Recompute archived Jhol results and check the two small reference models."""

import argparse
import ctypes
import hashlib
import itertools
import json
import math
import os
import platform
import plistlib
import random
import shutil
import statistics
import subprocess
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path


EXPERIMENTS = Path(__file__).resolve().parents[2] / "docs" / "architecture" / "experiments"


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[math.ceil(fraction * len(ordered)) - 1]


def cold_install():
    report = json.loads(
        (EXPERIMENTS / "installed-jhol-vs-bun-registry-cold-20261004-014958.json").read_text()
    )
    by_variant = {"installed-jhol": {}, "bun": {}}
    for sample in report["samples"]:
        assert sample["status"] == "pass" and sample["exit_code"] == 0
        assert sample["route_observed"] and not sample["resolver_drift"]
        consumer = sample["consumer_checks"]
        assert consumer["react_server_exit"] == consumer["vite_build_exit"] == 0
        rounds = by_variant[sample["variant"]]
        assert sample["round"] not in rounds
        rounds[sample["round"]] = sample["elapsed_ms"]
    jhol, bun = by_variant.values()
    assert set(jhol) == set(bun) == set(range(1, 31))
    pairs = [(jhol[i], bun[i]) for i in sorted(jhol)]
    rng = random.Random(20261112)
    ratios = []
    for _ in range(10_000):
        left, right = zip(*rng.choices(pairs, k=len(pairs)))
        ratios.append(statistics.median(right) / statistics.median(left))
    return {
        "arms": {
            name: {
                "n": len(rounds),
                "median_ms": round(statistics.median(rounds.values()), 2),
                "p95_ms": round(percentile(rounds.values(), 0.95), 2),
            }
            for name, rounds in by_variant.items()
        },
        "bun_time_over_jhol_time": round(statistics.median(bun.values()) / statistics.median(jhol.values()), 4),
        "paired_bootstrap_95pct": [round(percentile(ratios, q), 4) for q in (0.025, 0.975)],
    }


def store_ablation():
    report = json.loads((EXPERIMENTS / "jgrid-jagrs-ablation-results.json").read_text())
    result = {}
    for arm, rows in report["raw"].items():
        assert len(rows) == 9 and all(row["ok"] and row["exit"] == 0 for row in rows)
        if arm != "bun":
            assert all(row["quality"] for row in rows)
        times = [row["ms"] for row in rows]
        result[arm] = {
            "n": len(rows),
            "median_ms": statistics.median(times),
            "range_ms": [min(times), max(times)],
            "package_counts": sorted({row["dirs"] for row in rows}),
            "grid_bytes": sorted({row["grid_bytes"] for row in rows if "grid_bytes" in row}),
            "mirror_object_bytes": sorted({row["jagrs_bytes"] for row in rows if "jagrs_bytes" in row}),
        }
    return result


def separator_model():
    # A and B are exterior variables. X is the eliminated interior variable.
    domain = (0, 1)
    pairs = list(itertools.product(domain, repeat=2))
    relations = [
        {pair for bit, pair in enumerate(pairs) if mask & (1 << bit)}
        for mask in range(1 << len(pairs))
    ]
    checks = 0
    for ax, xb in itertools.product(relations, repeat=2):
        best = {}
        for a, b in pairs:
            feasible = [x for x in domain if (a, x) in ax and (x, b) in xb]
            if feasible:
                best[a, b] = max(feasible, key=lambda x: 3 * x)
        for outside in relations:
            for a, b in pairs:
                brute = [
                    3 * x - av + 2 * bv
                    for av, x, bv in itertools.product(domain, repeat=3)
                    if (av, bv) == (a, b)
                    and (av, x) in ax
                    and (x, bv) in xb
                    and (av, bv) in outside
                ]
                if (a, b) in outside and (a, b) in best:
                    assert brute and 3 * best[a, b] - a + 2 * b == max(brute)
                    assert (a, best[a, b]) in ax and (best[a, b], b) in xb
                else:
                    assert not brute
                checks += 1
    return {"factor_systems": len(relations) ** 3, "exterior_assignments_checked": checks}


def refill_model():
    # The restore has completed. Insert one deletion of the live tree before,
    # between, or after refill's ordered steps. Every prefix can be interrupted.
    steps = ("begin", "clone", "verify", "publish")
    results = {}
    for source in ("live", "sibling"):
        published = 0
        prefixes = 0
        for position in range(len(steps) + 1):
            schedule = (*steps[:position], "delete-live", *steps[position:])
            live = True
            started = cloned = verified = ready = False
            for event in schedule:
                if event == "delete-live":
                    live = False
                elif event == "begin":
                    started = source == "sibling" or live
                elif event == "clone":
                    cloned = started and (source == "sibling" or live)
                elif event == "verify":
                    verified = cloned
                elif event == "publish":
                    ready = verified
                assert not ready or (cloned and verified)
                prefixes += 1
            published += ready
        results[source] = {
            "schedules": len(steps) + 1,
            "published": published,
            "nonempty_prefixes_checked": prefixes,
        }
    assert results["live"]["published"] == 3 and results["sibling"]["published"] == 5
    return results


def tree_digest(root):
    digest = hashlib.sha256()
    for path in sorted(root.glob("*/*.bin")):
        digest.update(str(path.relative_to(root)).encode() + b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def apfs_summary(samples):
    timings = {}
    for operation in ("clonefile", "rename"):
        rows = [row for row in samples if row["operation"] == operation]
        assert len(rows) == 30 and {row["round"] for row in rows} == set(range(1, 31))
        times = [row["elapsed_ns"] for row in rows]
        timings[operation] = {"median": statistics.median(times), "p95": percentile(times, 0.95)}
    assert len(samples) == 60
    return timings


def archived_apfs_primitives():
    path = EXPERIMENTS / "paper-apfs-replay-primitives-2026-11-12.json"
    report = json.loads(path.read_text())
    summary = apfs_summary(report["samples"])
    assert summary == report["summary_ns"]
    assert report["platform"]["fs"] == "apfs" and report["fixture"]["files"] == 738
    return summary


def measure_apfs_primitives():
    if platform.system() != "Darwin":
        raise SystemExit("The clonefile experiment requires macOS and APFS.")
    clonefile = ctypes.CDLL(None, use_errno=True).clonefile
    clonefile.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int]
    clonefile.restype = ctypes.c_int

    def clone(source, destination):
        if clonefile(os.fsencode(source), os.fsencode(destination), 0):
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error), str(destination))

    rng = random.Random(20261112)
    with tempfile.TemporaryDirectory(prefix="jhol-paper-replay-") as tmp:
        root = Path(tmp)
        device = subprocess.check_output(["df", "-P", tmp], text=True).splitlines()[1].split()[0]
        disk = plistlib.loads(subprocess.check_output(["diskutil", "info", "-plist", device]))
        if disk.get("FilesystemType") != "apfs":
            raise SystemExit("The temporary directory must be on APFS.")
        source = root / "source"
        files, bytes_per_file = 738, 16384
        for number in range(files):
            path = source / f"package-{number // 41:02}" / f"entry-{number:03}.bin"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(hashlib.shake_256(str(number).encode()).digest(bytes_per_file))
        expected = tree_digest(source)
        samples = []
        for trial in range(1, 31):
            slot, destination = root / "slot", root / "destination"
            clone(source, slot)
            for operation in rng.sample(("clonefile", "rename"), 2):
                start = time.perf_counter_ns()
                if operation == "clonefile":
                    clone(source, destination)
                else:
                    os.rename(slot, destination)
                elapsed = time.perf_counter_ns() - start
                assert tree_digest(destination) == expected
                assert sum(1 for _ in destination.glob("*/*.bin")) == files
                samples.append({"round": trial, "operation": operation, "elapsed_ns": elapsed})
                shutil.rmtree(destination)
        clone(source, root / "isolation")
        (root / "isolation/package-00/entry-000.bin").write_bytes(b"modified clone")
        assert tree_digest(source) == expected
    return {
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "platform": {"macOS": platform.mac_ver()[0], "arch": platform.machine(), "fs": "apfs"},
        "fixture": {"files": files, "bytes_per_file": bytes_per_file, "tree_digest": expected},
        "rounds": 30,
        "order_seed": 20261112,
        "scope": (
            "Warm synthetic directory primitives only. "
            "Slot seeding, checks and cleanup untimed. OS cache uncontrolled."
        ),
        "verified": (
            "Every operation matched expected names and file bytes. "
            "Writing a clone preserved the source."
        ),
        "summary_ns": apfs_summary(samples),
        "samples": samples,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--measure-apfs-primitives", action="store_true")
    if parser.parse_args().measure_apfs_primitives:
        result = measure_apfs_primitives()
    else:
        result = {
            "cold_install": cold_install(),
            "storage_ablation": store_ablation(),
            "separator_model": separator_model(),
            "refill_model": refill_model(),
            "archived_apfs_primitives_ns": archived_apfs_primitives(),
        }
    print(json.dumps(result, indent=2))
