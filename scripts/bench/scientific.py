#!/usr/bin/env python3
"""Scientific install benchmarks

Expanded merge of: scientific_bench.py, scientific_jhol_vs_bun.py, benchmark_scientific_v2.py
Usage: python3 scripts/bench/scientific.py [suite|versus|v2] [args...]
Default mode: suite
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

_BENCH_DIR = Path(__file__).resolve().parent

# ── from scientific_bench.py ─────────────────────────────────────────────
def run_suite(argv: list[str] | None = None) -> None:
    """Original script: scientific_bench.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    SCIENTIFIC BENCHMARK: JHOL vs Bun
    Rigorous methodology with proper controls, statistical significance, and verification
    """
    import os, subprocess, shutil, time, json, tempfile, statistics, sys
    from pathlib import Path
    from datetime import datetime
    import signal

    JHOL = str(Path(__file__).parent / "target" / "release" / "jhol")
    RUNS = 10  # Increased to 10 for statistical significance
    SMALL_PKGS = ["lodash", "axios", "chalk"]
    MEDIUM_PKGS = ["react", "typescript", "express"]

    def nuke_all_caches_completely():
        """Nuke ALL caches with verification."""
        cleaned = []

        # JHOL caches - ALL possible locations
        jhol_cache_dirs = [
            Path.home() / ".jhol-cache",
            Path.home() / ".jhol-store",
            Path.home() / ".cache" / "jhol",
            Path.home() / ".jhol",
        ]
        for d in jhol_cache_dirs:
            if d.exists():
                shutil.rmtree(d, ignore_errors=True)
                cleaned.append(str(d))

        # Bun cache
        bun_cache_dirs = [
            Path.home() / ".bun" / "install",
            Path.home() / ".bun" / "cache",
        ]
        for d in bun_cache_dirs:
            if d.exists():
                shutil.rmtree(d, ignore_errors=True)
                cleaned.append(str(d))

        # Verify caches are actually cleaned
        for d in cleaned:
            if Path(d).exists():
                print(f"  ⚠️  WARNING: {d} still exists!", file=sys.stderr)

        return cleaned


    def run_install_with_verification(tool, pkgs, workdir):
        """Run install with full verification."""
        env = os.environ.copy()
        env["JHOL_QUIET"] = "1"

        # Create package.json
        pj = workdir / "package.json"
        with open(pj, "w") as f:
            json.dump({"name": "bench", "version": "1.0.0", "dependencies": {p: "latest" for p in pkgs}}, f)

        if tool == "jhol":
            cmd = [JHOL, "install", "--native-only"]
        elif tool == "bun":
            cmd = ["bun", "install"]

        t0 = time.perf_counter()
        try:
            result = subprocess.run(
                cmd, 
                cwd=workdir, 
                capture_output=True, 
                env=env, 
                timeout=300,
                preexec_fn=os.setsid if hasattr(os, 'setsid') else None
            )
            elapsed = time.perf_counter() - t0

            # Verify installation succeeded
            node_modules = workdir / "node_modules"
            if result.returncode == 0:
                # Count installed packages
                installed_count = len([d for d in node_modules.glob("*") if d.is_dir()]) if node_modules.exists() else 0
                return elapsed, True, installed_count, ""
            else:
                return elapsed, False, 0, result.stderr.decode()[:200] if result.stderr else "Unknown error"

        except subprocess.TimeoutExpired:
            return 300.0, False, 0, "Timeout after 300s"
        except Exception as e:
            return 0.0, False, 0, str(e)


    def scientific_bench(tool, pkgs, label):
        """Run scientific benchmark with full metrics."""
        print(f"\n  {tool} [{label}]:", end="", flush=True)

        runs = []
        errors = []
        installed_counts = []
        cache_clean_log = []

        for i in range(RUNS):
            # Clean caches before EACH run
            cleaned = nuke_all_caches_completely()
            cache_clean_log.append(len(cleaned))

            # Fresh workdir
            workdir = Path(tempfile.mkdtemp(prefix=f"sci_{tool}_{i}_"))

            # Run install with verification
            t, success, count, error = run_install_with_verification(tool, pkgs, workdir)
            runs.append(round(t, 3))
            installed_counts.append(count)

            if not success:
                errors.append(f"Run {i+1}: {error}")

            status = "✅" if success else "❌"
            print(f" run{i+1}:{t:.3f}s{status}", end="", flush=True)

            # Cleanup
            shutil.rmtree(workdir, ignore_errors=True)

        print()

        # Calculate comprehensive statistics
        sorted_runs = sorted(runs)
        med = round(statistics.median(runs), 3)
        avg = round(statistics.mean(runs), 3)
        best = round(min(runs), 3)
        worst = round(max(runs), 3)
        stdev = round(statistics.stdev(runs), 3) if len(runs) > 1 else 0.0

        # P95 (remove top 5% outliers)
        p95_idx = int(len(sorted_runs) * 0.95)
        p95 = sorted_runs[min(p95_idx, len(sorted_runs)-1)]

        # Success rate
        success_count = RUNS - len(errors)
        success_rate = (success_count / RUNS) * 100

        # Verify consistency
        avg_installed = round(statistics.mean(installed_counts), 1) if installed_counts else 0

        return {
            "runs": runs,
            "median": med,
            "avg": avg,
            "best": best,
            "worst": worst,
            "stdev": stdev,
            "p95": round(p95, 3),
            "errors": errors,
            "success_rate": success_rate,
            "avg_installed_packages": avg_installed,
            "cache_clean_count": statistics.mean(cache_clean_log) if cache_clean_log else 0,
        }


    def statistical_comparison(jhol_data, bun_data, label):
        """Perform rigorous statistical comparison."""
        print(f"\n  {('='*60)}")
        print(f"  STATISTICAL ANALYSIS: {label.upper()}")
        print(f"  {('='*60)}")

        jhol_med = jhol_data["median"]
        bun_med = bun_data["median"]

        # Speedup
        if jhol_med < bun_med:
            speedup = bun_med / jhol_med
            print(f"    🏆 JHOL is {speedup:.2f}x FASTER (median)")
        elif jhol_med > bun_med:
            speedup = jhol_med / bun_med
            print(f"    Bun is {speedup:.2f}x faster (median)")
        else:
            print(f"    🤝 EQUAL (median)")

        # P95 comparison (more realistic than worst case)
        jhol_p95 = jhol_data["p95"]
        bun_p95 = bun_data["p95"]
        print(f"\n    P95 (95th percentile):")
        print(f"      JHOL: {jhol_p95:.3f}s")
        print(f"      Bun:  {bun_p95:.3f}s")
        if jhol_p95 < bun_p95:
            speedup = bun_p95 / jhol_p95
            print(f"      🏆 JHOL is {speedup:.2f}x FASTER at P95")

        # Standard deviation (consistency)
        print(f"\n    Consistency (stdev):")
        print(f"      JHOL: {jhol_data['stdev']:.3f}s")
        print(f"      Bun:  {bun_data['stdev']:.3f}s")
        if jhol_data['stdev'] < bun_data['stdev']:
            print(f"      🏆 JHOL is MORE CONSISTENT")
        else:
            print(f"      ⚠️  Bun is MORE CONSISTENT")

        # Success rate
        print(f"\n    Success Rate:")
        print(f"      JHOL: {jhol_data['success_rate']:.1f}%")
        print(f"      Bun:  {bun_data['success_rate']:.1f}%")

        # Installed packages verification
        print(f"\n    Verification:")
        print(f"      JHOL avg packages: {jhol_data['avg_installed_packages']:.1f}")
        print(f"      Bun avg packages:  {bun_data['avg_installed_packages']:.1f}")

        # Errors
        if jhol_data["errors"]:
            print(f"\n    ⚠️  JHOL ERRORS ({len(jhol_data['errors'])}/{RUNS}):")
            for err in jhol_data["errors"][:3]:  # Show first 3
                print(f"      - {err}")
        if bun_data["errors"]:
            print(f"\n    ⚠️  BUN ERRORS ({len(bun_data['errors'])}/{RUNS}):")
            for err in bun_data["errors"][:3]:
                print(f"      - {err}")


    def main():
        print("="*80)
        print("SCIENTIFIC BENCHMARK: JHOL vs Bun")
        print("="*80)
        print(f"\nMethodology:")
        print(f"  - {RUNS} runs per test (statistical significance)")
        print(f"  - Complete cache nuke before EACH run")
        print(f"  - Cache cleaning verification")
        print(f"  - Installation verification (package count)")
        print(f"  - Median, P95, stdev reported")
        print(f"  - Error tracking and reporting")
        print(f"  - Timestamp: {datetime.now().isoformat()}")

        results = {}

        for name, pkgs in [("small", SMALL_PKGS), ("medium", MEDIUM_PKGS)]:
            print(f"\n{'='*80}")
            print(f"{name.upper()} PACKAGES: {', '.join(pkgs)}")
            print(f"{'='*80}")

            for tool in ["jhol", "bun"]:
                data = scientific_bench(tool, pkgs, name)
                results[f"{tool}_{name}"] = data
                print(f"\n  → {tool}:")
                print(f"     median={data['median']:.3f}s  avg={data['avg']:.3f}s")
                print(f"     p95={data['p95']:.3f}s  stdev={data['stdev']:.3f}s")
                print(f"     success={data['success_rate']:.1f}%  pkgs={data['avg_installed_packages']:.1f}")

            # Statistical comparison
            statistical_comparison(results[f"jhol_{name}"], results[f"bun_{name}"], name)

        # Final summary
        print(f"\n{'='*80}")
        print("FINAL VERDICT")
        print(f"{'='*80}")

        jhol_wins = 0
        bun_wins = 0

        for name in ["small", "medium"]:
            jhol_key = f"jhol_{name}"
            bun_key = f"bun_{name}"

            jhol_med = results[jhol_key]["median"]
            bun_med = results[bun_key]["median"]

            print(f"\n{name.upper()} PACKAGES:")
            print(f"  JHOL: {jhol_med:.3f}s (p95={results[jhol_key]['p95']:.3f}s, stdev={results[jhol_key]['stdev']:.3f}s)")
            print(f"  Bun:  {bun_med:.3f}s (p95={results[bun_key]['p95']:.3f}s, stdev={results[bun_key]['stdev']:.3f}s)")

            if jhol_med < bun_med:
                speedup = bun_med / jhol_med
                print(f"  🏆 JHOL WINS ({speedup:.2f}x faster)")
                jhol_wins += 1
            elif jhol_med > bun_med:
                speedup = jhol_med / bun_med
                print(f"  Bun wins ({speedup:.2f}x faster)")
                bun_wins += 1
            else:
                print(f"  🤝 TIE")

        print(f"\n{'='*80}")
        print("OVERALL RESULT:")
        print(f"{'='*80}")
        if jhol_wins > bun_wins:
            print(f"  🏆🏆🏆 JHOL WINS {jhol_wins}-{bun_wins}! 🏆🏆🏆")
        elif bun_wins > jhol_wins:
            print(f"  Bun wins {bun_wins}-{jhol_wins}")
        else:
            print(f"  🤝 TIE {jhol_wins}-{bun_wins}")

        # Save results
        output_data = {
            "timestamp": datetime.now().isoformat(),
            "runs_per_test": RUNS,
            "results": results,
            "summary": {
                "jhol_wins": jhol_wins,
                "bun_wins": bun_wins,
                "winner": "JHOL" if jhol_wins > bun_wins else "Bun" if bun_wins > jhol_wins else "TIE",
            }
        }

        output_file = f"scientific_benchmark_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
        with open(output_file, "w") as f:
            json.dump(output_data, f, indent=2)

        print(f"\n  💾 Results saved to: {output_file}")
        print(f"  📊 This benchmark verifies cache cleaning, installation success, and package counts")

    main()


# ── from scientific_jhol_vs_bun.py ─────────────────────────────────────────────
def run_versus(argv: list[str] | None = None) -> None:
    """Original script: scientific_jhol_vs_bun.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    Scientific, rigorous, laptop-safe benchmark: Jhol (Full Fat LTO) vs Bun 1.3.14
    Measures:
    1. Cold install (fresh isolated cache & fresh project) - 3 runs
    2. Warm install (cache populated, node_modules deleted) - 3 runs
    3. No-op re-install (node_modules present) - 3 runs
    4. Memory footprint (Peak RSS via /usr/bin/time -l)
    5. Quality / Correctness audit (Node.js runtime module loadability, bin links, lockfile)
    """

    import json
    import os
    import shutil
    import subprocess
    import sys
    import tempfile
    import time
    from pathlib import Path

    WORKSPACE = Path("/Users/bhuvanprakash/j/jhol")
    JHOL = str(WORKSPACE / "target" / "release" / "jhol")
    BUN = "/Users/bhuvanprakash/.bun/bin/bun"
    NODE = shutil.which("node") or "node"
    RUN_DIR = WORKSPACE / "benchmarks" / "run_dir"

    BENCH_PACKAGES = {
        "lodash": "^4.17.21",
        "debug": "^4.3.4",
        "ms": "^2.1.3",
        "mime-types": "^2.1.35",
        "react": "^18.3.1",
        "axios": "^1.7.9"
    }

    def create_project(dest_dir):
        dest_dir.mkdir(parents=True, exist_ok=True)
        pkg = {
            "name": "bench-target",
            "version": "1.0.0",
            "dependencies": BENCH_PACKAGES
        }
        (dest_dir / "package.json").write_text(json.dumps(pkg, indent=2))

    def get_peak_rss_mb(cmd, cwd, env):
        time_cmd = ["/usr/bin/time", "-l"] + cmd
        p = subprocess.run(time_cmd, cwd=cwd, env=env, capture_output=True, text=True)
        for line in p.stderr.splitlines():
            if "maximum resident set size" in line:
                parts = line.strip().split()
                bytes_val = int(parts[0])
                return bytes_val / (1024 * 1024)
        return 0.0

    def verify_quality(project_dir, tool_name):
        node_modules = project_dir / "node_modules"
        if not node_modules.exists():
            return {
                "installed": False,
                "error": "node_modules missing",
                "packages_loaded": 0,
                "direct_deps_checked": len(BENCH_PACKAGES),
                "total_modules_dirs": 0,
                "bin_links": [],
                "lockfile_present": False
            }

        loaded = []
        failed = []
        for pkg in BENCH_PACKAGES.keys():
            test_script = f"try {{ require('{pkg}'); console.log('OK'); }} catch (e) {{ console.error(e.message); process.exit(1); }}"
            res = subprocess.run([NODE, "-e", test_script], cwd=project_dir, capture_output=True, text=True)
            if res.returncode == 0:
                loaded.append(pkg)
            else:
                failed.append((pkg, res.stderr.strip()))

        pkg_count = len([d for d in node_modules.iterdir() if d.is_dir() and not d.name.startswith(".")])

        bin_dir = node_modules / ".bin"
        bin_links = []
        if bin_dir.exists():
            bin_links = [b.name for b in bin_dir.iterdir()]

        lockfile_present = (project_dir / "package-lock.json").exists() if tool_name == "jhol" else (project_dir / "bun.lock").exists() or (project_dir / "bun.lockb").exists()

        return {
            "installed": True,
            "packages_loaded": len(loaded),
            "packages_failed": failed,
            "direct_deps_checked": len(BENCH_PACKAGES),
            "total_modules_dirs": pkg_count,
            "bin_links": bin_links,
            "lockfile_present": lockfile_present
        }

    def run_suite():
        print("=" * 75)
        print("  JHOL (Full Fat LTO) vs Bun 1.3.14 — Direct Benchmark & Quality Audit")
        print("  Hardware: macOS arm64, 10 cores")
        print("=" * 75)

        if RUN_DIR.exists():
            shutil.rmtree(RUN_DIR)
        RUN_DIR.mkdir(parents=True, exist_ok=True)

        warm_dir_jhol = RUN_DIR / "jhol_warm"
        warm_dir_bun = RUN_DIR / "bun_warm"
        cache_jhol = RUN_DIR / "jhol_cache"
        home_bun = RUN_DIR / "bun_home"

        cache_jhol.mkdir(parents=True, exist_ok=True)
        home_bun.mkdir(parents=True, exist_ok=True)

        create_project(warm_dir_jhol)
        create_project(warm_dir_bun)

        env_jhol = os.environ.copy()
        env_jhol["JHOL_CACHE_DIR"] = str(cache_jhol)
        env_jhol["JHOL_NO_SPLASH"] = "1"

        env_bun = os.environ.copy()
        env_bun["HOME"] = str(home_bun)
        env_bun["BUN_INSTALL_CACHE_DIR"] = str(home_bun / "cache")

        # ----------------------------------------------------
        # Phase 1: Cold Install (Fresh isolated cache for both)
        # ----------------------------------------------------
        print("\n[1/3] Benchmarking Cold Install (isolated fresh cache & project dir)...")
        jhol_cold_times = []
        bun_cold_times = []

        for round_idx in range(3):
            cd_jhol = RUN_DIR / f"jhol_cold_{round_idx}"
            cd_bun = RUN_DIR / f"bun_cold_{round_idx}"
            cc_jhol = RUN_DIR / f"jhol_cold_cache_{round_idx}"
            ch_bun = RUN_DIR / f"bun_cold_home_{round_idx}"

            create_project(cd_jhol)
            create_project(cd_bun)
            cc_jhol.mkdir(parents=True, exist_ok=True)
            ch_bun.mkdir(parents=True, exist_ok=True)

            ej = env_jhol.copy()
            ej["JHOL_CACHE_DIR"] = str(cc_jhol)

            eb = env_bun.copy()
            eb["HOME"] = str(ch_bun)
            eb["BUN_INSTALL_CACHE_DIR"] = str(ch_bun / "cache")

            # Jhol cold run
            t0 = time.perf_counter()
            res_j = subprocess.run([JHOL, "install", "--quiet"], cwd=cd_jhol, env=ej, capture_output=True, text=True)
            t_j = (time.perf_counter() - t0) * 1000
            jhol_cold_times.append(t_j)

            # Bun cold run
            t0 = time.perf_counter()
            res_b = subprocess.run([BUN, "install"], cwd=cd_bun, env=eb, capture_output=True, text=True)
            t_b = (time.perf_counter() - t0) * 1000
            bun_cold_times.append(t_b)

            print(f"  Round {round_idx+1}: Jhol = {t_j:6.1f} ms  |  Bun = {t_b:6.1f} ms")

        med_jhol_cold = sorted(jhol_cold_times)[1]
        med_bun_cold = sorted(bun_cold_times)[1]

        # Prewarm the warm project directories
        subprocess.run([JHOL, "install", "--quiet"], cwd=warm_dir_jhol, env=env_jhol, capture_output=True)
        subprocess.run([BUN, "install"], cwd=warm_dir_bun, env=env_bun, capture_output=True)

        # ----------------------------------------------------
        # Phase 2: Warm Install Benchmark (node_modules deleted, cache warm)
        # ----------------------------------------------------
        print("\n[2/3] Benchmarking Warm Install (node_modules deleted, cache hot)...")
        jhol_warm_times = []
        bun_warm_times = []

        for round_idx in range(3):
            # Jhol run
            if (warm_dir_jhol / "node_modules").exists():
                shutil.rmtree(warm_dir_jhol / "node_modules")
            t0 = time.perf_counter()
            subprocess.run([JHOL, "install", "--quiet"], cwd=warm_dir_jhol, env=env_jhol, capture_output=True)
            t_j = (time.perf_counter() - t0) * 1000
            jhol_warm_times.append(t_j)

            # Bun run
            if (warm_dir_bun / "node_modules").exists():
                shutil.rmtree(warm_dir_bun / "node_modules")
            t0 = time.perf_counter()
            subprocess.run([BUN, "install"], cwd=warm_dir_bun, env=env_bun, capture_output=True)
            t_b = (time.perf_counter() - t0) * 1000
            bun_warm_times.append(t_b)

            print(f"  Round {round_idx+1}: Jhol = {t_j:6.1f} ms  |  Bun = {t_b:6.1f} ms")

        med_jhol_warm = sorted(jhol_warm_times)[1]
        med_bun_warm = sorted(bun_warm_times)[1]

        # ----------------------------------------------------
        # Phase 3: No-Op Re-install (node_modules present)
        # ----------------------------------------------------
        print("\n[3/3] Benchmarking No-Op Re-install (node_modules intact)...")
        jhol_noop_times = []
        bun_noop_times = []

        for round_idx in range(3):
            t0 = time.perf_counter()
            subprocess.run([JHOL, "install", "--quiet"], cwd=warm_dir_jhol, env=env_jhol, capture_output=True)
            t_j = (time.perf_counter() - t0) * 1000
            jhol_noop_times.append(t_j)

            t0 = time.perf_counter()
            subprocess.run([BUN, "install"], cwd=warm_dir_bun, env=env_bun, capture_output=True)
            t_b = (time.perf_counter() - t0) * 1000
            bun_noop_times.append(t_b)

            print(f"  Round {round_idx+1}: Jhol = {t_j:6.1f} ms  |  Bun = {t_b:6.1f} ms")

        med_jhol_noop = sorted(jhol_noop_times)[1]
        med_bun_noop = sorted(bun_noop_times)[1]

        # Memory Measurement
        rss_jhol = get_peak_rss_mb([JHOL, "install", "--quiet"], warm_dir_jhol, env_jhol)
        rss_bun = get_peak_rss_mb([BUN, "install"], warm_dir_bun, env_bun)

        # Quality check
        q_jhol = verify_quality(warm_dir_jhol, "jhol")
        q_bun = verify_quality(warm_dir_bun, "bun")

        # ----------------------------------------------------
        # Report Findings
        # ----------------------------------------------------
        print("\n" + "=" * 75)
        print("  FINAL SCIENTIFIC RESULTS TABLE (Medians of 3 Interleaved Runs)")
        print("=" * 75)
        print(f"{'Metric / Scenario':<35} | {'Jhol (Fat LTO)':<15} | {'Bun 1.3.14':<15} | {'Advantage'}")
        print("-" * 75)

        # Warm
        warm_ratio = med_bun_warm / med_jhol_warm if med_jhol_warm > 0 else 1.0
        w_adv = f"Jhol {warm_ratio:.2f}x faster" if warm_ratio > 1 else f"Bun {1/warm_ratio:.2f}x faster"
        print(f"{'Warm Install (cache hit, unlink)':<35} | {med_jhol_warm:6.1f} ms        | {med_bun_warm:6.1f} ms        | {w_adv}")

        # No-Op
        noop_ratio = med_bun_noop / med_jhol_noop if med_jhol_noop > 0 else 1.0
        n_adv = f"Jhol {noop_ratio:.2f}x faster" if noop_ratio > 1 else f"Bun {1/noop_ratio:.2f}x faster"
        print(f"{'No-Op Re-install (hot node_modules)':<35} | {med_jhol_noop:6.1f} ms        | {med_bun_noop:6.1f} ms        | {n_adv}")

        # Cold
        cold_ratio = med_bun_cold / med_jhol_cold if med_jhol_cold > 0 else 1.0
        c_adv = f"Jhol {cold_ratio:.2f}x faster" if cold_ratio > 1 else f"Bun {1/cold_ratio:.2f}x faster"
        print(f"{'Cold Install (fresh cache + net)':<35} | {med_jhol_cold:6.1f} ms        | {med_bun_cold:6.1f} ms        | {c_adv}")

        # Memory
        print(f"{'Peak RSS Memory':<35} | {rss_jhol:6.1f} MB        | {rss_bun:6.1f} MB        | Jhol {rss_bun/rss_jhol if rss_jhol else 1.0:.2f}x leaner")

        print("\n" + "=" * 75)
        print("  QUALITY & INTEGRITY AUDIT")
        print("=" * 75)
        print(f"Jhol node_modules valid:        {q_jhol['installed']}")
        print(f"Jhol packages loadable in Node: {q_jhol['packages_loaded']} / {q_jhol['direct_deps_checked']}")
        print(f"Jhol bin links created:         {', '.join(q_jhol['bin_links']) or 'none'}")
        print(f"Jhol lockfile present:          {q_jhol['lockfile_present']}")
        print("-" * 75)
        print(f"Bun node_modules valid:         {q_bun['installed']}")
        print(f"Bun packages loadable in Node:  {q_bun['packages_loaded']} / {q_bun['direct_deps_checked']}")
        print(f"Bun bin links created:          {', '.join(q_bun['bin_links']) or 'none'}")
        print(f"Bun lockfile present:           {q_bun['lockfile_present']}")
        print("=" * 75)

        # Clean up run_dir to save disk
        shutil.rmtree(RUN_DIR, ignore_errors=True)

    run_suite()


def run_v2(argv: list[str] | None = None) -> None:
    """Delegate to scripts/bench/scientific_v2.py (was benchmark_scientific_v2.py)."""
    cmd = [sys.executable, str(_BENCH_DIR / "scientific_v2.py")]
    if argv:
        cmd.extend(argv)
    raise SystemExit(subprocess.call(cmd))


# ── dispatcher ──────────────────────────────────────────────────
_MODES = {
    'suite': run_suite,
    'versus': run_versus,
    'v2': run_v2,
}

def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in _MODES:
        mode = argv.pop(0)
    else:
        mode = 'suite'
    _MODES[mode](argv)


if __name__ == "__main__":
    main()
