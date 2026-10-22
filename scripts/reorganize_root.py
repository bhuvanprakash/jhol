#!/usr/bin/env python3
"""One-shot root cleanup: move files into docs/, scripts/bench/, benchmarks/.
Writes a manifest for verification. No deletions."""
from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

KEEP_AT_ROOT = {
    "AGENTS.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "GET_STARTED.md",
    "INNOVATION_LOG.md",
    "README.md",
    "SECURITY.md",
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "install.sh",
    "install_jhol.bat",
    "reasonix.toml",
    "package.json",
    "package-lock.json",
}

PLANS = {
    "KCC_KNOT_COHOMOLOGY_CLASSIFIER_PLAN.md",
    "SMCC_DESIGN.md",
    "JAGR_INFINITE_PLAN.md",
    "ROADMAP.md",
    "implementation_plan.md",
    "2025_RESEARCH_OPTIMIZATION_PLAN.md",
    "ULTIMATE_OPTIMIZATION_PLAN.md",
    "COMPLETE_OPTIMIZATION_MASTER_PLAN.md",
    "JAGR2_STRATEGIC_ROADMAP.md",
    "JAGR3_NEXT_GEN_PROPOSAL.md",
    "JAGR3_PURE_MATH_IMPROVEMENTS.md",
}

ARCHIVE_MISC = {
    "QWEN.md",
    "README-highdata.md",
    "slow-to-fast-documentary.md",
}

BENCH_SCRIPTS = {
    "bench_jhol.py",
    "bench_vs_bun.sh",
    "brutal-bench.sh",
    "benchmark_jagr_comparison.py",
    "benchmark_large_workload.py",
    "benchmark_repeat.py",
    "deep_profiler.py",
    "fair_bench.py",
    "final_accurate_bench.py",
    "professional_bench.py",
    "profile_cold_install.py",
    "quick_bench.py",
    "run_jagr_benchmark.sh",
    "scientific_bench.py",
    "simple_jagr_benchmark.py",
    "ultra_clean_bench.py",
}

DOC_USER_FROM_ROOT = {"for-windows.md"}

DOC_ENGINEERING_FROM_DOCUMENTATION = {
    "BENCHMARK_IDEAS_APPLIED.md",
    "BOTTLENECKS_AND_PRODUCTION_READINESS.md",
    "BOTTLENECKS_JHOL_VS_BUN.md",
    "JAGR2_SEMANTICS.md",
    "LOCKFILE_DESIGN.md",
}

DOC_USER_FROM_DOCUMENTATION = {
    "main.md",
    "SCOPE_AND_STABILITY.md",
}


def ensure_dir(p: Path) -> None:
    p.mkdir(parents=True, exist_ok=True)


def move_file(src: Path, dst: Path, manifest: list[dict]) -> None:
    if not src.exists():
        return
    if dst.exists():
        raise SystemExit(f"destination already exists: {dst}")
    ensure_dir(dst.parent)
    shutil.move(str(src), str(dst))
    manifest.append({"from": str(src.relative_to(ROOT)), "to": str(dst.relative_to(ROOT))})


def main() -> None:
    manifest: list[dict] = []

    dirs = [
        ROOT / "docs/plans",
        ROOT / "docs/user",
        ROOT / "docs/engineering",
        ROOT / "docs/archive/2025-optimization",
        ROOT / "docs/archive/2026-benchmarks",
        ROOT / "docs/archive/misc",
        ROOT / "docs/archive/junk",
        ROOT / "scripts/bench",
        ROOT / "benchmarks/results/json",
        ROOT / "benchmarks/results/markdown",
        ROOT / "benchmarks/profiles",
    ]
    for d in dirs:
        ensure_dir(d)

    # Documentation/ -> docs/user + docs/engineering
    doc_root = ROOT / "Documentation"
    if doc_root.is_dir():
        for name in DOC_USER_FROM_DOCUMENTATION:
            move_file(doc_root / name, ROOT / "docs/user" / name, manifest)
        for name in DOC_ENGINEERING_FROM_DOCUMENTATION:
            move_file(doc_root / name, ROOT / "docs/engineering" / name, manifest)

    for name in DOC_USER_FROM_ROOT:
        move_file(ROOT / name, ROOT / "docs/user" / name, manifest)

    for name in PLANS:
        move_file(ROOT / name, ROOT / "docs/plans" / name, manifest)

    for name in ARCHIVE_MISC:
        move_file(ROOT / name, ROOT / "docs/archive/misc" / name, manifest)

    for name in BENCH_SCRIPTS:
        move_file(ROOT / name, ROOT / "scripts/bench" / name, manifest)

    # Root markdown -> archive (except keep/plans/misc already moved)
    for path in sorted(ROOT.glob("*.md")):
        name = path.name
        if name in KEEP_AT_ROOT or name in PLANS or name in ARCHIVE_MISC:
            continue
        move_file(path, ROOT / "docs/archive/2025-optimization" / name, manifest)

    # Benchmark JSON at root
    for path in sorted(ROOT.glob("*.json")):
        if path.name in KEEP_AT_ROOT:
            continue
        move_file(path, ROOT / "benchmarks/results/json" / path.name, manifest)

    # Benchmark markdown at root
    for path in sorted(ROOT.glob("benchmark-results*.md")):
        move_file(path, ROOT / "benchmarks/results/markdown" / path.name, manifest)

    # Profile logs
    for path in sorted(ROOT.glob("jhol-*.txt")):
        move_file(path, ROOT / "benchmarks/profiles" / path.name, manifest)
    for path in sorted(ROOT.glob("*-profile.txt")):
        move_file(path, ROOT / "benchmarks/profiles" / path.name, manifest)

    # Junk (empty accident files) -> archive/junk, not deleted
    for name in ("0.5", "="):
        p = ROOT / name
        if p.is_file():
            move_file(p, ROOT / "docs/archive/junk" / name, manifest)

    manifest_path = ROOT / "docs/archive/REORG_MANIFEST.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"moved {len(manifest)} paths")
    print(f"manifest: {manifest_path}")


if __name__ == "__main__":
    main()
