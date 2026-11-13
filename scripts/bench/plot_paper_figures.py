#!/usr/bin/env python3
"""Render the research-series figures from the archived observations."""

import argparse
import json
from pathlib import Path
from statistics import median

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

from paper_evidence import EXPERIMENTS, apfs_summary, cold_install, store_ablation


INK, TEAL, GOLD = "#243441", "#087f8c", "#a85405"


def read_report(name):
    return json.loads((EXPERIMENTS / name).read_text())


def style(ax, axis="x"):
    ax.set_axisbelow(True)
    ax.grid(axis=axis, color="#dfe5e9", linewidth=0.7)
    ax.spines[["top", "right"]].set_visible(False)


def save(fig, path):
    fig.savefig(path, dpi=300, bbox_inches="tight", facecolor="white")
    plt.close(fig)


def resolution(path):
    cold_install()
    rows = read_report("installed-jhol-vs-bun-registry-cold-20261004-014958.json")["samples"]
    points = {(r["variant"], r["round"]): r["elapsed_ms"] for r in rows}
    jhol = [points["installed-jhol", i] for i in range(1, 31)]
    bun = [points["bun", i] for i in range(1, 31)]
    limit = 1.07 * max(jhol + bun)
    fig, ax = plt.subplots(figsize=(4.5, 3.5), layout="constrained")
    ax.plot([0, limit], [0, limit], "--", color=INK, linewidth=1, label="Equal latency")
    ax.scatter(jhol, bun, s=30, color=TEAL, alpha=0.8, edgecolor="white", linewidth=0.45)
    ax.set(xlim=(0, limit), ylim=(0, limit), xlabel="Jhol 1.0.2 install time (ms)",
           ylabel="Bun 1.4.2 install time (ms)")
    ax.set_aspect("equal", adjustable="box")
    ax.legend(frameon=False, loc="upper left")
    style(ax, "both")
    save(fig, path)


def storage(path):
    summary = store_ablation()
    raw = read_report("jgrid-jagrs-ablation-results.json")["raw"]
    labels = ("Jhol defaults", "Jhol forced stores", "Jhol harvest + mirror", "Bun isolated cache")
    assert tuple(raw) == ("A_agent_defaults", "B_jgrid_jagrs_forced", "C_both_harvest_plus_mirrors", "bun")
    fig, (latency, objects) = plt.subplots(1, 2, figsize=(6.7, 2.6), sharey=True,
                                          width_ratios=(2.35, 1), layout="constrained")
    for pos, (arm, rows) in enumerate(raw.items()):
        samples = [r["ms"] for r in rows]
        latency.scatter(samples, [pos + (i - 4) * 0.036 for i in range(9)], color=TEAL, s=24, alpha=0.78)
        latency.vlines(median(samples), pos - 0.24, pos + 0.24, color=INK, linewidth=1.6)
        sizes = summary[arm]["mirror_object_bytes"]
        if sizes:
            assert len(sizes) == 1
            size_kb = sizes[0] / 1000
            objects.barh(pos, size_kb, height=0.35, color=GOLD)
            objects.text(size_kb + 25, pos, f"{size_kb:.0f}", va="center")
        else:
            objects.text(25, pos, "n/a", va="center")
    latency.set_yticks(range(len(labels)), labels)
    latency.invert_yaxis()
    latency.set(xlim=(0, 2500), xlabel="Locked install time (ms)")
    objects.set(xlim=(0, 1000), xlabel="Mirror objects (kB)")
    objects.tick_params(axis="y", left=False, labelleft=False)
    for ax in (latency, objects):
        style(ax)
    save(fig, path)


def replay(path):
    report = read_report("paper-apfs-replay-primitives-2026-11-12.json")
    summary = apfs_summary(report["samples"])
    assert summary == report["summary_ns"]
    fig, ax = plt.subplots(figsize=(6.7, 2.35), layout="constrained")
    for pos, operation in enumerate(("rename", "clonefile")):
        samples = [r["elapsed_ns"] / 1e6 for r in report["samples"] if r["operation"] == operation]
        offsets = [((i * 7) % 30 - 14.5) * 0.012 for i in range(30)]
        ax.scatter(samples, [pos + offset for offset in offsets], color=TEAL, s=21, alpha=0.65)
        ax.vlines(summary[operation]["median"] / 1e6, pos - 0.26, pos + 0.26,
                  color=INK, linewidth=1.6, label="Median" if pos == 0 else None)
        ax.scatter(summary[operation]["p95"] / 1e6, pos, color=GOLD, marker="D", s=32,
                   label="p95" if pos == 0 else None, zorder=4)
    ax.set_yticks((0, 1), ("Rename prepared directory", "Clone directory"))
    ax.set_ylim(1.43, -0.5)
    ax.set_xscale("log")
    ax.set_xticks((0.2, 0.5, 1, 2, 5, 10, 20), ("0.2", "0.5", "1", "2", "5", "10", "20"))
    ax.set(xlabel="Filesystem operation time (ms, log scale)")
    ax.legend(frameon=False, loc="upper right", ncols=2)
    style(ax)
    save(fig, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    output = parser.parse_args().output
    output.mkdir(parents=True, exist_ok=True)
    plt.rcParams.update({"font.size": 10, "axes.labelcolor": INK, "text.color": INK})
    resolution(output / "resolution-paired-cold.png")
    storage(output / "storage-locked-ablation.png")
    replay(output / "replay-apfs-primitives.png")


if __name__ == "__main__":
    main()
