#!/usr/bin/env python3
"""Versus — jhol vs bun benchmark suite.

Modes (pass as first argument):
  fair    Persistent cache across runs (real-world usage)
  clean   Ultra-clean: nuke all caches between every run
  truth   Accurate cold: thorough cache cleanup each run
  quick   Quick smoke (3 runs, small/medium sets)
  brutal  Cold / warm / daemon matrix on test-project fixture

Usage:
  python3 scripts/bench/versus.py fair
  python3 scripts/bench/versus.py brutal

Merged from: fair_bench, ultra_clean_bench, final_accurate_bench,
quick_bench, bench_jhol — all behaviour preserved.
"""
from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
JHOL_BIN = REPO_ROOT / "target" / "release" / "jhol"
TEST_PROJECT_PKG = REPO_ROOT / "test-project" / "package.json"


def _repo_jhol_path_expr() -> None:
    """Document where binaries and fixtures live (for maintainers)."""
    pass


# ── fair: was fair_bench.py ──
def run_fair(argv: list[str] | None = None) -> None:
    """Original script: fair_bench.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    FAIR benchmark: jhol vs bun with PERSISTENT cache (like real usage)
    """
    import os, subprocess, shutil, time, json, tempfile, statistics
    from pathlib import Path

    JHOL = str(JHOL_BIN)
    RUNS = 5
    SMALL_PKGS = ["lodash", "axios", "chalk"]
    MEDIUM_PKGS = ["react", "typescript", "express"]

    def run_install(tool, pkgs, workdir, cache_dir):
        """Run install and return elapsed seconds."""
        env = os.environ.copy()
        if tool == "jhol":
            env["JHOL_CACHE_DIR"] = str(cache_dir)
            env["JHOL_QUIET"] = "1"
            cmd = [JHOL, "install", "--native-only"]
        elif tool == "bun":
            cmd = ["bun", "install"]

        # Create package.json
        pj = os.path.join(workdir, "package.json")
        with open(pj, "w") as f:
            json.dump({"name": "bench-test", "version": "1.0.0", "dependencies": {p: "latest" for p in pkgs}}, f)

        t0 = time.perf_counter()
        subprocess.run(cmd, cwd=workdir, capture_output=True, env=env)
        return time.perf_counter() - t0


    def fair_bench(tool, pkgs, label):
        """Run N installs with SAME cache (simulating real usage)."""
        # Single persistent cache for all runs
        if tool == "jhol":
            cache_dir = Path.home() / ".jhol-fair-bench-cache"
            cache_dir.mkdir(exist_ok=True)
        else:
            cache_dir = None

        runs = []
        print(f"  {tool} [{label}]:", end="", flush=True)

        for i in range(RUNS):
            workdir = tempfile.mkdtemp(prefix=f"fair_{tool}_")
            t = run_install(tool, pkgs, workdir, cache_dir)
            runs.append(round(t, 3))
            print(f"  run{i+1}:{t:.3f}s", end="", flush=True)
            shutil.rmtree(workdir, ignore_errors=True)

        print()
        med = round(statistics.median(runs), 3)
        avg = round(statistics.mean(runs), 3)
        return runs, med, avg


    def main():
        print("="*60)
        print("FAIR BENCHMARK: Persistent cache (real-world scenario)")
        print("="*60)

        for name, pkgs in [("small", SMALL_PKGS), ("medium", MEDIUM_PKGS)]:
            print(f"\n{name.upper()} packages: {', '.join(pkgs)}")
            print("-"*40)

            for tool in ["jhol", "bun"]:
                runs, med, avg = fair_bench(tool, pkgs, name)
                print(f"  → {tool} median: {med:.3f}s  avg: {avg:.3f}s")

                if tool == "jhol":
                    jhol_runs, jhol_med, jhol_avg = runs, med, avg
                else:
                    bun_runs, bun_med, bun_avg = runs, med, avg

            print(f"\n  Comparison:")
            print(f"    jhol: {jhol_med:.3f}s  (runs: {jhol_runs})")
            print(f"    bun:  {bun_med:.3f}s  (runs: {bun_runs})")

            # Show first run vs subsequent runs
            jhol_first = jhol_runs[0]
            jhol_subsequent = statistics.median(jhol_runs[1:])
            bun_first = bun_runs[0]
            bun_subsequent = statistics.median(bun_runs[1:])

            print(f"\n  First run: jhol={jhol_first:.3f}s  bun={bun_first:.3f}s  (gap: {jhol_first/bun_first:.1f}x)")
            print(f"  Subsequent: jhol={jhol_subsequent:.3f}s  bun={bun_subsequent:.3f}s  (gap: {jhol_subsequent/bun_subsequent:.1f}x)")

    main()

# ── clean: was ultra_clean_bench.py ──
def run_clean(argv: list[str] | None = None) -> None:
    """Original script: ultra_clean_bench.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    ULTRA-CLEAN BENCHMARK: Complete cache nuke between every run
    Ensures true cold install comparison with no cache benefits
    """
    import os, subprocess, shutil, time, json, tempfile, statistics
    from pathlib import Path

    JHOL = str(JHOL_BIN)
    RUNS = 5
    SMALL_PKGS = ["lodash", "axios", "chalk"]
    MEDIUM_PKGS = ["react", "typescript", "express"]

    def nuke_all_caches():
        """Nuke ALL package manager caches completely."""
        # JHOL caches
        jhol_cache_dirs = [
            Path.home() / ".jhol-cache",
            Path.home() / ".jhol-store",
            Path.home() / ".cache" / "jhol",
            Path.home() / ".jhol",
        ]
        for d in jhol_cache_dirs:
            if d.exists():
                shutil.rmtree(d, ignore_errors=True)

        # Bun cache
        bun_cache = Path.home() / ".bun" / "install"
        if bun_cache.exists():
            shutil.rmtree(bun_cache, ignore_errors=True)

        # npm cache
        subprocess.run(["npm", "cache", "clean", "--force"], capture_output=True)

        # yarn cache
        subprocess.run(["yarn", "cache", "clean"], capture_output=True)

        # pnpm cache
        subprocess.run(["pnpm", "store", "prune"], capture_output=True)

        # Clear temp directories
        subprocess.run(["rm", "-rf", "/tmp/jhol_*"], capture_output=True)
        subprocess.run(["rm", "-rf", "/tmp/bun_*"], capture_output=True)


    def nuke_workdir(workdir):
        """Remove all install artifacts."""
        for name in ["node_modules", ".jhol-cache", ".npm", ".yarn", ".pnpm",
                     "bun.lockb", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"]:
            p = workdir / name
            if p.exists():
                if p.is_dir():
                    shutil.rmtree(p, ignore_errors=True)
                else:
                    p.unlink()


    def run_install(tool, pkgs, workdir):
        """Run install and return elapsed seconds."""
        env = os.environ.copy()
        env["JHOL_QUIET"] = "1"

        # Create package.json with dependencies
        pj = workdir / "package.json"
        with open(pj, "w") as f:
            json.dump({"name": "bench-test", "version": "1.0.0", "dependencies": {p: "latest" for p in pkgs}}, f)

        if tool == "jhol":
            cmd = [JHOL, "install", "--native-only"]
        elif tool == "bun":
            cmd = ["bun", "install"]

        t0 = time.perf_counter()
        result = subprocess.run(cmd, cwd=workdir, capture_output=True, env=env)
        elapsed = time.perf_counter() - t0

        return elapsed, result.returncode == 0


    def ultra_clean_bench(tool, pkgs, label):
        """Run N ultra-cold installs with complete cache nuke between each."""
        runs = []
        print(f"  {tool} [{label}] ultra-cold:", end="", flush=True)

        for i in range(RUNS):
            # Create fresh workdir
            workdir = Path(tempfile.mkdtemp(prefix=f"ultra_clean_{tool}_{i}_"))

            # Create package.json
            pj = workdir / "package.json"
            with open(pj, "w") as f:
                json.dump({"name": "bench-test", "version": "1.0.0", "dependencies": {}}, f)

            # NUKE EVERYTHING before each run
            nuke_all_caches()
            nuke_workdir(workdir)

            # Run install
            t, success = run_install(tool, pkgs, workdir)
            runs.append(round(t, 3))

            status = "✅" if success else "❌"
            print(f"  run{i+1}:{t:.3f}s{status}", end="", flush=True)

            # Cleanup workdir
            shutil.rmtree(workdir, ignore_errors=True)

        print()
        med = round(statistics.median(runs), 3)
        avg = round(statistics.mean(runs), 3)
        best = round(min(runs), 3)
        worst = round(max(runs), 3)
        return runs, med, avg, best, worst


    def main():
        print("="*70)
        print("ULTRA-CLEAN BENCHMARK: Complete cache nuke between EVERY run")
        print("="*70)
        print("\nThis ensures TRUE cold installs with NO cache benefits.\n")

        results = {}

        for name, pkgs in [("small", SMALL_PKGS), ("medium", MEDIUM_PKGS)]:
            print(f"\n{'='*70}")
            print(f"{name.upper()} packages: {', '.join(pkgs)}")
            print(f"{'='*70}")

            for tool in ["jhol", "bun"]:
                runs, med, avg, best, worst = ultra_clean_bench(tool, pkgs, name)
                results[f"{tool}_{name}"] = {
                    "runs": runs,
                    "median": med,
                    "avg": avg,
                    "best": best,
                    "worst": worst,
                }
                print(f"  → {tool}: median={med:.3f}s  avg={avg:.3f}s  best={best:.3f}s  worst={worst:.3f}s")

            # Compare
            print(f"\n  COMPARISON:")
            jhol_key = f"jhol_{name}"
            bun_key = f"bun_{name}"

            jhol_med = results[jhol_key]["median"]
            bun_med = results[bun_key]["median"]

            if jhol_med < bun_med:
                speedup = bun_med / jhol_med
                print(f"    🏆 JHOL is {speedup:.1f}x FASTER than Bun!")
            elif jhol_med > bun_med:
                speedup = jhol_med / bun_med
                print(f"    Bun is {speedup:.1f}x faster than JHOL")
            else:
                print(f"    🤝 JHOL and Bun are EQUAL!")

            # Show variance
            jhol_variance = results[jhol_key]["worst"] / results[jhol_key]["best"]
            bun_variance = results[bun_key]["worst"] / results[bun_key]["best"]
            print(f"\n  VARIANCE:")
            print(f"    JHOL: {jhol_variance:.1f}x (best={results[jhol_key]['best']:.3f}s, worst={results[jhol_key]['worst']:.3f}s)")
            print(f"    Bun:  {bun_variance:.1f}x (best={results[bun_key]['best']:.3f}s, worst={results[bun_key]['worst']:.3f}s)")

        # Final summary
        print(f"\n{'='*70}")
        print("FINAL SUMMARY")
        print(f"{'='*70}")

        jhol_small = results["jhol_small"]["median"]
        bun_small = results["bun_small"]["median"]
        jhol_medium = results["jhol_medium"]["median"]
        bun_medium = results["bun_medium"]["median"]

        print(f"\nSmall packages (median):")
        print(f"  JHOL: {jhol_small:.3f}s")
        print(f"  Bun:  {bun_small:.3f}s")
        if jhol_small < bun_small:
            print(f"  🏆 JHOL is {bun_small/jhol_small:.1f}x FASTER")
        elif jhol_small > bun_small:
            print(f"  Bun is {jhol_small/bun_small:.1f}x faster")
        else:
            print(f"  🤝 EQUAL")

        print(f"\nMedium packages (median):")
        print(f"  JHOL: {jhol_medium:.3f}s")
        print(f"  Bun:  {bun_medium:.3f}s")
        if jhol_medium < bun_medium:
            print(f"  🏆 JHOL is {bun_medium/jhol_medium:.1f}x FASTER")
        elif jhol_medium > bun_medium:
            print(f"  Bun is {jhol_medium/jhol_medium:.1f}x faster")
        else:
            print(f"  🤝 EQUAL")

        # Overall winner
        print(f"\n{'='*70}")
        print("OVERALL WINNER:")
        print(f"{'='*70}")

        jhol_wins = 0
        bun_wins = 0

        if jhol_small < bun_small:
            jhol_wins += 1
        elif jhol_small > bun_small:
            bun_wins += 1

        if jhol_medium < bun_medium:
            jhol_wins += 1
        elif jhol_medium > bun_medium:
            bun_wins += 1

        if jhol_wins > bun_wins:
            print(f"  🏆🏆🏆 JHOL WINS ({jhol_wins}-{bun_wins})! 🏆🏆🏆")
        elif bun_wins > jhol_wins:
            print(f"  Bun wins ({bun_wins}-{jhol_wins})")
        else:
            print(f"  🤝 TIE ({jhol_wins}-{bun_wins})")

        # Save results
        with open("ultra_clean_benchmark.json", "w") as f:
            json.dump({
                "timestamp": time.strftime("%Y-%m-%d %H:%M:%S"),
                "runs_per_test": RUNS,
                "results": results,
                "summary": {
                    "jhol_wins": jhol_wins,
                    "bun_wins": bun_wins,
                    "winner": "JHOL" if jhol_wins > bun_wins else "Bun" if bun_wins > jhol_wins else "TIE",
                }
            }, f, indent=2)

        print(f"\n💾 Results saved to: ultra_clean_benchmark.json")

    main()

# ── truth: was final_accurate_bench.py ──
def run_truth(argv: list[str] | None = None) -> None:
    """Original script: final_accurate_bench.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    FINAL ACCURATE BENCHMARK: JHOL vs Bun
    Properly cleans ALL cache locations, measures TRUE performance
    """
    import os, subprocess, shutil, time, json, tempfile, statistics, sys
    from pathlib import Path
    from datetime import datetime

    JHOL = str(JHOL_BIN)
    RUNS = 5
    SMALL_PKGS = ["lodash", "axios", "chalk"]
    MEDIUM_PKGS = ["react", "typescript", "express"]

    def nuke_all_caches_properly():
        """Nuke ALL caches properly - this time getting ALL locations."""
        # JHOL caches - ALL possible locations
        jhol_cache_dirs = [
            Path.home() / ".jhol-cache",           # Primary cache
            Path.home() / ".jhol-store",           # Global module store
            Path.home() / ".cache" / "jhol",       # XDG cache
            Path.home() / ".jhol",                 # Legacy
            Path.home() / "Library" / "Caches" / "jhol",  # macOS
        ]
        for d in jhol_cache_dirs:
            if d.exists():
                shutil.rmtree(d, ignore_errors=True)
                print(f"  Cleaned: {d}", file=sys.stderr)

        # Bun cache - ALL locations
        bun_cache_dirs = [
            Path.home() / ".bun" / "install",
            Path.home() / ".bun" / "cache",
        ]
        for d in bun_cache_dirs:
            if d.exists():
                shutil.rmtree(d, ignore_errors=True)
                print(f"  Cleaned: {d}", file=sys.stderr)

        # npm/yarn/pnpm caches
        try:
            subprocess.run(["npm", "cache", "clean", "--force"], capture_output=True, timeout=10)
            subprocess.run(["yarn", "cache", "clean"], capture_output=True, timeout=10)
            subprocess.run(["pnpm", "store", "prune"], capture_output=True, timeout=10)
        except:
            pass

        # System temp
        subprocess.run(["rm", "-rf", "/tmp/jhol_*", "/tmp/bun_*"], shell=True, capture_output=True)


    def run_install(tool, pkgs, workdir):
        """Run install and return elapsed seconds."""
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
            result = subprocess.run(cmd, cwd=workdir, capture_output=True, env=env, timeout=300)
            elapsed = time.perf_counter() - t0
            return elapsed, result.returncode == 0
        except:
            return 300.0, False


    def main():
        print("="*70)
        print("FINAL ACCURATE BENCHMARK: TRUE Cold Install Performance")
        print("="*70)
        print("\nCleaning ALL cache locations properly...\n")

        results = {}

        for name, pkgs in [("small", SMALL_PKGS), ("medium", MEDIUM_PKGS)]:
            print(f"\n{'='*70}")
            print(f"{name.upper()} packages: {', '.join(pkgs)}")
            print(f"{'='*70}\n")

            for tool in ["jhol", "bun"]:
                runs = []
                print(f"  {tool}:", end="", flush=True)

                for i in range(RUNS):
                    # CRITICAL: Nuke ALL caches before EACH run
                    nuke_all_caches_properly()

                    # Fresh workdir
                    workdir = Path(tempfile.mkdtemp(prefix=f"final_{tool}_{i}_"))

                    # Run install
                    t, success = run_install(tool, pkgs, workdir)
                    runs.append(round(t, 3))

                    status = "✅" if success else "❌"
                    print(f" run{i+1}:{t:.3f}s{status}", end="", flush=True)

                    # Cleanup
                    shutil.rmtree(workdir, ignore_errors=True)

                print()
                med = round(statistics.median(runs), 3)
                avg = round(statistics.mean(runs), 3)
                best = round(min(runs), 3)
                worst = round(max(runs), 3)

                results[f"{tool}_{name}"] = {
                    "runs": runs,
                    "median": med,
                    "avg": avg,
                    "best": best,
                    "worst": worst,
                }
                print(f"  → {tool}: median={med:.3f}s  best={best:.3f}s")

            # Compare
            print(f"\n  COMPARISON:")
            jhol_med = results[f"jhol_{name}"]["median"]
            bun_med = results[f"bun_{name}"]["median"]

            if jhol_med < bun_med:
                print(f"    🏆 JHOL is {bun_med/jhol_med:.2f}x FASTER!")
            elif jhol_med > bun_med:
                print(f"    Bun is {jhol_med/bun_med:.2f}x faster")
            else:
                print(f"    🤝 EQUAL")

        # Final summary
        print(f"\n{'='*70}")
        print("FINAL RESULTS")
        print(f"{'='*70}")

        jhol_wins = 0
        bun_wins = 0

        for name in ["small", "medium"]:
            jhol_med = results[f"jhol_{name}"]["median"]
            bun_med = results[f"bun_{name}"]["median"]

            print(f"\n{name.upper()}:")
            print(f"  JHOL: {jhol_med:.3f}s")
            print(f"  Bun:  {bun_med:.3f}s")

            if jhol_med < bun_med:
                print(f"  🏆 JHOL WINS ({bun_med/jhol_med:.2f}x faster)")
                jhol_wins += 1
            else:
                print(f"  Bun wins ({jhol_med/bun_med:.2f}x faster)")
                bun_wins += 1

        print(f"\n{'='*70}")
        if jhol_wins > bun_wins:
            print(f"🏆🏆🏆 JHOL WINS {jhol_wins}-{bun_wins}! 🏆🏆🏆")
        elif bun_wins > jhol_wins:
            print(f"Bun wins {bun_wins}-{jhol_wins}")
        else:
            print(f"🤝 TIE {jhol_wins}-{bun_wins}")
        print(f"{'='*70}")

    main()

# ── quick: was quick_bench.py ──
def run_quick(argv: list[str] | None = None) -> None:
    """Original script: quick_bench.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    Quick benchmark to compare jhol vs bun
    """
    import subprocess
    import time
    import tempfile
    import shutil
    from pathlib import Path
    import json

    def run_test(tool, packages, name):
        """Run install test"""
        with tempfile.TemporaryDirectory() as tmpdir:
            test_dir = Path(tmpdir) / "test"
            test_dir.mkdir()

            # Create package.json
            pkg_json = {
                "name": "test",
                "version": "1.0.0",
                "dependencies": {pkg: "latest" for pkg in packages}
            }
            (test_dir / "package.json").write_text(json.dumps(pkg_json, indent=2))

            # Clean cache
            cache_dir = test_dir / ".cache"
            cache_dir.mkdir()

            # Run install
            start = time.perf_counter()
            if tool == "jhol":
                env = {"JHOL_CACHE_DIR": str(cache_dir), "JHOL_QUIET": "1"}
                jhol_bin = JHOL_BIN
                result = subprocess.run(
                    [str(jhol_bin), "install", "--native-only"],
                    cwd=test_dir,
                    env={**subprocess.os.environ, **env},
                    capture_output=True
                )
            else:
                result = subprocess.run(
                    [tool, "install"],
                    cwd=test_dir,
                    capture_output=True
                )
            elapsed = time.perf_counter() - start

            success = result.returncode == 0
            return elapsed, success

    def main():
        packages_small = ["lodash", "axios", "chalk"]
        packages_medium = ["react", "typescript", "express"]

        print("="*60)
        print("QUICK BENCHMARK: jhol vs bun")
        print("="*60)

        for name, packages in [("small", packages_small), ("medium", packages_medium)]:
            print(f"\n{name.upper()} packages: {', '.join(packages)}")
            print("-"*40)

            for tool in ["jhol", "bun"]:
                times = []
                for i in range(3):
                    elapsed, success = run_test(tool, packages, name)
                    times.append(elapsed)
                    status = "✅" if success else "❌"
                    print(f"  {tool} run {i+1}: {elapsed:.3f}s {status}")

                median = sorted(times)[1]
                print(f"  → {tool} median: {median:.3f}s")

                if tool == "jhol":
                    jhol_median = median
                else:
                    bun_median = median

            if jhol_median and bun_median:
                ratio = jhol_median / bun_median
                print(f"\n  Gap: jhol is {ratio:.1f}x slower than bun")

    main()

# ── brutal: was bench_jhol.py ──
def run_brutal(argv: list[str] | None = None) -> None:
    """Original script: bench_jhol.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """JHOL vs BUN brutal bench."""
    import subprocess, time, os, shutil

    JHOL = str(JHOL_BIN)
    BASE = "/tmp/jhol-bench-v5"
    os.makedirs(BASE, exist_ok=True)

    PROJ = os.path.join(BASE, "proj")
    BUN_PROJ = os.path.join(BASE, "bun")
    os.makedirs(PROJ, exist_ok=True)
    os.makedirs(BUN_PROJ, exist_ok=True)

    test_pkg = str(TEST_PROJECT_PKG)
    shutil.copy(test_pkg, os.path.join(PROJ, "package.json"))
    shutil.copy(test_pkg, os.path.join(BUN_PROJ, "package.json"))

    JHC_WARM = os.path.join(BASE, "jhc-warm")
    JHC_DAEMON = os.path.join(BASE, "jhc-daemon")
    BUN_CACHE = os.path.join(BASE, "bun-cache")

    def clean_jhol():
        for p in ["node_modules", "package-lock.json", ".jhol.capsule", "bun.lock"]:
            shutil.rmtree(os.path.join(PROJ, p), ignore_errors=True)
            try: os.unlink(os.path.join(PROJ, p))
            except: pass

    def clean_bun():
        for p in ["node_modules", "bun.lock", "package-lock.json"]:
            shutil.rmtree(os.path.join(BUN_PROJ, p), ignore_errors=True)
            try: os.unlink(os.path.join(BUN_PROJ, p))
            except: pass

    def run_jhol(args, cache_dir=None):
        env = {**os.environ}
        if cache_dir:
            env["JHOL_CACHE_DIR"] = cache_dir
        r = subprocess.run([JHOL]+args, capture_output=True, cwd=PROJ, env=env)
        return r.returncode, r.stderr.decode()[:100] if r.returncode else ""

    def run_bun(args, cache_dir=None):
        env = {**os.environ}
        if cache_dir:
            env["BUN_INSTALL_CACHE_DIR"] = cache_dir
        r = subprocess.run(args, capture_output=True, cwd=BUN_PROJ, env=env)
        return r.returncode, r.stderr.decode()[:100] if r.returncode else ""

    def time_fn(fn):
        t0 = time.perf_counter()
        rc, err = fn()
        t1 = time.perf_counter()
        return round((t1-t0)*1000), rc, err

    def bench(label, fn, n=5):
        print(f"  {label}")
        times = []
        for i in range(n):
            ms, rc, err = time_fn(fn)
            ok = "ok" if rc == 0 else f"err={err[:50]}"
            times.append(ms)
            print(f"    {i+1}: {ms}ms  {ok}")
        return times

    print("="*60)
    print("JHOL vs BUN BRUTAL BENCHMARK")
    print("="*60)

    # Warm
    rc, _ = run_jhol(["install","--no-scripts"], JHC_WARM)
    print(f"  JHOL warm: rc={rc}")
    rc, _ = run_bun(["bun","install"], BUN_CACHE)
    print(f"  BUN warm: rc={rc}")

    # Daemon
    subprocess.run("pkill -f 'jhol daemon' 2>/dev/null", shell=True)
    time.sleep(1)
    shutil.copytree(JHC_WARM, JHC_DAEMON, dirs_exist_ok=True)
    dproc = subprocess.Popen(
        [JHOL,"daemon","--serve"], cwd=PROJ,
        env={**os.environ,"JHOL_CACHE_DIR":JHC_DAEMON},
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    time.sleep(2)
    print(f"Daemon up: PID={dproc.pid}")
    print()

    print("=== RESULTS ===\n")

    def jcold():
        cold = os.path.join(BASE, f"jc{time.time()}")
        clean_jhol()
        return run_jhol(["install","--no-scripts"], cold)

    jhol_cold = bench("JHOL COLD", jcold)
    jhol_warm = bench("JHOL WARM", lambda: (clean_jhol(), run_jhol(["install","--no-scripts"], JHC_WARM))[-1])
    jhol_daemon = bench("JHOL DAEMON", lambda: (clean_jhol(), run_jhol(["install","--daemon","--no-scripts"], JHC_DAEMON))[-1])
    bun_warm = bench("BUN WARM", lambda: (clean_bun(), run_bun(["bun","install"], BUN_CACHE))[-1])

    print()
    print("="*60)
    for name, times in [("JHOL COLD",jhol_cold),("JHOL WARM",jhol_warm),("JHOL DAEMON",jhol_daemon),("BUN WARM",bun_warm)]:
        mn,av,mx = min(times), sum(times)//len(times), max(times)
        print(f"  {name:15s}: {mn:4d} / {av:4d} / {mx:4d}  (min/avg/max)")

    dproc.terminate()
    time.sleep(1)
    shutil.rmtree(BASE, ignore_errors=True)
    subprocess.run("pkill -f 'jhol daemon' 2>/dev/null", shell=True)
    print("\nDone.")

_MODES = {
    "fair": run_fair,
    "clean": run_clean,
    "truth": run_truth,
    "quick": run_quick,
    "brutal": run_brutal,
}

_MODE_HELP = """
Modes:
  fair    persistent JHOL cache across runs
  clean   full cache nuke between each run (+ JSON report)
  truth   expanded cache locations, cold each run
  quick   3-run smoke benchmark
  brutal  cold/warm/daemon on test-project
"""


def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0] in ("-h", "--help", "help"):
        print(_MODE_HELP.strip())
        print("Example: python3 scripts/bench/versus.py fair")
        return
    if argv[0] not in _MODES:
        keys = ", ".join(_MODES)
        raise SystemExit(f"unknown mode {argv[0]!r}; choose: {keys}\n{_MODE_HELP}")
    mode = argv.pop(0)
    _MODES[mode](argv)


if __name__ == "__main__":
    main()
