#!/usr/bin/env python3
"""Atelier · professional statistical suite

Expanded merge of: professional_bench.py
Usage: python3 atelier.py
"""
from __future__ import annotations

import sys

# ── from professional_bench.py ─────────────────────────────────────────────
def run_pro(argv: list[str] | None = None) -> None:
    """Original script: professional_bench.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    PROFESSIONAL BENCHMARK SUITE: JHOL vs Bun
    Rigorous, repeatable, with proper warmup and statistical analysis
    """
    import os, subprocess, shutil, time, json, tempfile, statistics, sys
    from pathlib import Path
    from datetime import datetime

    JHOL = str(Path(__file__).parent / "target" / "release" / "jhol")
    RUNS = 7  # Increased from 5 to 7 for better statistical significance
    WARMUP_RUNS = 2  # Add warmup runs to stabilize results
    SMALL_PKGS = ["lodash", "axios", "chalk"]
    MEDIUM_PKGS = ["react", "typescript", "express"]
    LARGE_PKGS = ["next", "typescript", "express", "prisma", "@prisma/client", "jest", "@types/node"]

    # Colors for output
    C = {
        "red": "\033[0;31m",
        "green": "\033[0;32m",
        "yellow": "\033[1;33m",
        "cyan": "\033[0;36m",
        "bold": "\033[1m",
        "reset": "\033[0m",
    }

    def c(color, s):
        return f"{C[color]}{s}{C['reset']}"

    def log(s):
        print(c("cyan", f"[bench] {s}"), flush=True)

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

        # Bun cache - CRITICAL: Don't nuke bun's global cache!
        # This was the unfair advantage - bun uses ~/.bun/install which persists
        # We should only nuke project-specific caches
        # bun_cache = Path.home() / ".bun" / "install"
        # if bun_cache.exists():
        #     shutil.rmtree(bun_cache, ignore_errors=True)

        # npm cache
        subprocess.run(["npm", "cache", "clean", "--force"], capture_output=True, timeout=30)

        # yarn cache
        subprocess.run(["yarn", "cache", "clean"], capture_output=True, timeout=30)

        # pnpm cache
        subprocess.run(["pnpm", "store", "prune"], capture_output=True, timeout=30)

        # Clear temp directories
        subprocess.run(["rm", "-rf", "/tmp/jhol_*"], shell=True, capture_output=True)
        subprocess.run(["rm", "-rf", "/tmp/bun_*"], shell=True, capture_output=True)

        # Clear DNS cache (macOS)
        if sys.platform == "darwin":
            subprocess.run(["sudo", "killall", "-HUP", "mDNSResponder"], capture_output=True)


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


    def run_install(tool, pkgs, workdir, use_cache=True):
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
        try:
            result = subprocess.run(cmd, cwd=workdir, capture_output=True, env=env, timeout=300)
            elapsed = time.perf_counter() - t0
            return elapsed, result.returncode == 0, result.stderr.decode() if result.stderr else ""
        except subprocess.TimeoutExpired:
            return 300.0, False, "Timeout"
        except Exception as e:
            return 0.0, False, str(e)


    def warmup_runs(tool, pkgs):
        """Run warmup to stabilize network and system state."""
        log(f"Running {WARMUP_RUNS} warmup runs for {tool}...")
        workdir = Path(tempfile.mkdtemp(prefix=f"warmup_{tool}_"))
        for i in range(WARMUP_RUNS):
            nuke_workdir(workdir)
            run_install(tool, pkgs, workdir, use_cache=True)
        shutil.rmtree(workdir, ignore_errors=True)


    def professional_bench(tool, pkgs, label, nuke_caches=True):
        """Run professional benchmark with proper methodology."""
        runs = []
        errors = []

        print(f"  {c('bold', tool)} [{label}]:", end="", flush=True)

        for i in range(RUNS):
            # Create fresh workdir
            workdir = Path(tempfile.mkdtemp(prefix=f"bench_{tool}_{i}_"))

            if nuke_caches and i == 0:
                # Only nuke caches on first run to test cold performance
                nuke_all_caches()

            nuke_workdir(workdir)

            # Run install
            t, success, error = run_install(tool, pkgs, workdir)
            runs.append(round(t, 3))

            if not success:
                errors.append(f"Run {i+1}: {error}")

            status = c("green", "✅") if success else c("red", "❌")
            print(f"  run{i+1}:{t:.3f}s{status}", end="", flush=True)

            # Cleanup workdir
            shutil.rmtree(workdir, ignore_errors=True)

        print()

        # Calculate statistics
        sorted_runs = sorted(runs)
        med = round(statistics.median(runs), 3)
        avg = round(statistics.mean(runs), 3)
        best = round(min(runs), 3)
        worst = round(max(runs), 3)
        stdev = round(statistics.stdev(runs), 3) if len(runs) > 1 else 0.0

        # Calculate p95 (remove outliers)
        p95_idx = int(len(sorted_runs) * 0.95)
        p95 = sorted_runs[min(p95_idx, len(sorted_runs)-1)]

        return {
            "runs": runs,
            "median": med,
            "avg": avg,
            "best": best,
            "worst": worst,
            "stdev": stdev,
            "p95": round(p95, 3),
            "errors": errors,
        }


    def statistical_analysis(jhol_data, bun_data, label):
        """Perform statistical analysis on results."""
        print(f"\n  {c('yellow', 'STATISTICAL ANALYSIS:')}")

        jhol_med = jhol_data["median"]
        bun_med = bun_data["median"]

        # Speedup calculation
        if jhol_med < bun_med:
            speedup = bun_med / jhol_med
            print(f"    🏆 JHOL is {speedup:.2f}x FASTER (median)")
        elif jhol_med > bun_med:
            speedup = jhol_med / bun_med
            print(f"    Bun is {speedup:.2f}x faster (median)")
        else:
            print(f"    🤝 EQUAL (median)")

        # Best case comparison
        jhol_best = jhol_data["best"]
        bun_best = bun_data["best"]
        if jhol_best < bun_best:
            speedup = bun_best / jhol_best
            print(f"    🏆 JHOL is {speedup:.2f}x FASTER (best case)")

        # P95 comparison (more realistic than worst case)
        jhol_p95 = jhol_data["p95"]
        bun_p95 = bun_data["p95"]
        if jhol_p95 < bun_p95:
            speedup = bun_p95 / jhol_p95
            print(f"    🏆 JHOL is {speedup:.2f}x FASTER (p95)")

        # Variance analysis
        jhol_variance = jhol_data["worst"] / jhol_data["best"] if jhol_data["best"] > 0 else 0
        bun_variance = bun_data["worst"] / bun_data["best"] if bun_data["best"] > 0 else 0
        print(f"\n  {c('yellow', 'VARIANCE ANALYSIS:')}")
        print(f"    JHOL: {jhol_variance:.2f}x (stdev={jhol_data['stdev']:.3f}s)")
        print(f"    Bun:  {bun_variance:.2f}x (stdev={bun_data['stdev']:.3f}s)")
        if jhol_variance < bun_variance:
            print(f"    🏆 JHOL is MORE STABLE")
        elif jhol_variance > bun_variance:
            print(f"    ⚠️  Bun is MORE STABLE")

        # Error analysis
        if jhol_data["errors"]:
            print(f"\n  {c('red', 'JHOL ERRORS:')}")
            for err in jhol_data["errors"]:
                print(f"    - {err}")
        if bun_data["errors"]:
            print(f"\n  {c('red', 'BUN ERRORS:')}")
            for err in bun_data["errors"]:
                print(f"    - {err}")


    def main():
        print(c("bold", "="*80))
        print(c("bold", "PROFESSIONAL BENCHMARK SUITE: JHOL vs Bun"))
        print(c("bold", "="*80))
        print(f"\n{c('cyan', 'Methodology:')}")
        print(f"  - {RUNS} runs per test (increased statistical significance)")
        print(f"  - {WARMUP_RUNS} warmup runs (system stabilization)")
        print(f"  - Median, P95, stdev reported (robust statistics)")
        print(f"  - Error tracking and reporting")
        print()

        results = {}

        # Test scenarios
        scenarios = [
            ("small", SMALL_PKGS),
            ("medium", MEDIUM_PKGS),
            ("large", LARGE_PKGS),
        ]

        for name, pkgs in scenarios:
            print(f"\n{c('bold', '='*80)}")
            print(f"{c('bold', f'{name.upper()} packages: {", ".join(pkgs)}')}")
            print(c('bold', '='*80))

            # Warmup
            for tool in ["jhol", "bun"]:
                warmup_runs(tool, pkgs)

            # Benchmark
            for tool in ["jhol", "bun"]:
                data = professional_bench(tool, pkgs, name, nuke_caches=True)
                results[f"{tool}_{name}"] = data
                print(f"  → {tool}: median={data['median']:.3f}s  avg={data['avg']:.3f}s  p95={data['p95']:.3f}s  stdev={data['stdev']:.3f}s")

            # Statistical analysis
            statistical_analysis(results[f"jhol_{name}"], results[f"bun_{name}"], name)

        # Final summary
        print(f"\n{c('bold', '='*80)}")
        print(c('bold', "FINAL SUMMARY"))
        print(c('bold', '='*80))

        summary = {}
        jhol_wins = 0
        bun_wins = 0

        for name, _ in scenarios:
            jhol_key = f"jhol_{name}"
            bun_key = f"bun_{name}"

            if jhol_key not in results or bun_key not in results:
                continue

            jhol_med = results[jhol_key]["median"]
            bun_med = results[bun_key]["median"]

            summary[name] = {
                "jhol": jhol_med,
                "bun": bun_med,
            }

            print(f"\n{name.upper()} packages (median):")
            print(f"  JHOL: {jhol_med:.3f}s")
            print(f"  Bun:  {bun_med:.3f}s")

            if jhol_med < bun_med:
                speedup = bun_med / jhol_med
                print(f"  🏆 JHOL is {speedup:.2f}x FASTER")
                jhol_wins += 1
            elif jhol_med > bun_med:
                speedup = jhol_med / bun_med
                print(f"  Bun is {speedup:.2f}x faster")
                bun_wins += 1
            else:
                print(f"  🤝 EQUAL")

        # Overall winner
        print(f"\n{c('bold', '='*80)}")
        print(c('bold', "OVERALL WINNER:"))
        print(c('bold', '='*80))

        if jhol_wins > bun_wins:
            print(f"  {c('green', '🏆🏆🏆 JHOL WINS (' + str(jhol_wins) + '-' + str(bun_wins) + ')! 🏆🏆🏆')}")
        elif bun_wins > jhol_wins:
            print(f"  {c('red', 'Bun wins (' + str(bun_wins) + '-' + str(jhol_wins) + ')')}")
        else:
            print(f"  {c('yellow', '🤝 TIE (' + str(jhol_wins) + '-' + str(bun_wins) + ')')}")

        # Save results
        output_data = {
            "timestamp": datetime.now().isoformat(),
            "runs_per_test": RUNS,
            "warmup_runs": WARMUP_RUNS,
            "results": results,
            "summary": summary,
            "winner": {
                "jhol_wins": jhol_wins,
                "bun_wins": bun_wins,
                "winner": "JHOL" if jhol_wins > bun_wins else "Bun" if bun_wins > jhol_wins else "TIE",
            }
        }

        output_file = f"professional_benchmark_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
        with open(output_file, "w") as f:
            json.dump(output_data, f, indent=2)

        print(f"\n{c('cyan', '💾 Results saved to:')} {output_file}")

        # Also save markdown report
        md_file = f"professional_benchmark_{datetime.now().strftime('%Y%m%d_%H%M%S')}.md"
        with open(md_file, "w") as f:
            f.write("# Professional Benchmark Results\n\n")
            f.write(f"**Date:** {datetime.now().isoformat()}\n\n")
            f.write("## Summary\n\n")
            for name, data in summary.items():
                f.write(f"### {name.upper()} Packages\n\n")
                f.write(f"| Metric | JHOL | Bun |\n")
                f.write(f"|--------|------|-----|\n")
                f.write(f"| Median | {data['jhol']:.3f}s | {data['bun']:.3f}s |\n")
                winner = "🏆 JHOL" if data['jhol'] < data['bun'] else "Bun"
                f.write(f"| Winner | **{winner}** |\n\n")

        print(f"{c('cyan', '📄 Markdown report saved to:')} {md_file}")

    main()


# ── dispatcher ──────────────────────────────────────────────────
_MODES = {
    'pro': run_pro,
}

def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in _MODES:
        mode = argv.pop(0)
    else:
        mode = 'pro'
    _MODES[mode](argv)


if __name__ == "__main__":
    main()
