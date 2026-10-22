#!/usr/bin/env python3
"""Legacy jhol vs bun comparison scripts (merged suite)."""
from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]


def _exit_if_help(argv: list[str] | None, blurb: str) -> int | None:
    if argv and argv[0] in ("-h", "--help", "help"):
        print(blurb.strip())
        return 0
    return None


# ── jhol-vs-bun: was benchmark_jhol_vs_bun.py ──

def run_jhol_vs_bun(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "legacy_compare.py", *argv]
    help_rc = _exit_if_help(
        argv,
        "benchmark_jhol_vs_bun.py — long jhol vs bun benchmark (no flags).\n"
        "See scripts/bench/legacy_compare.py jhol-vs-bun",
    )
    if help_rc is not None:
        return help_rc
    """
    High-Precision Benchmark: Jhol vs Bun 1.3.14
    Resolver: JAGR-Infinite Continuous Topological Engine
    Storage:  JAGR-S Store V3 (Ultra-Performance Zero-Copy Engine)
    """

    import os
    import sys
    import json
    import time
    import shutil
    import tempfile
    import subprocess
    from pathlib import Path

    WORKSPACE = REPO_ROOT
    JHOL_BIN = str(REPO_ROOT / "target" / "release" / "jhol")
    BUN_BIN = os.path.expanduser("~/.bun/bin/bun")
    GLOBAL_CACHE = os.path.expanduser("~/.jhol-cache")

    WORKLOADS = [
        {
            "name": "6-Package Core (Synth/Bundled)",
            "packages": ["express@4.18.2", "lodash@4.17.21", "uuid@9.0.0", "chalk@5.3.0", "axios@1.6.0", "react@18.2.0"],
            "deps": {
                "express": "^4.18.2",
                "lodash": "^4.17.21",
                "uuid": "^9.0.0",
                "chalk": "^5.3.0",
                "axios": "^1.6.0",
                "react": "^18.2.0"
            }
        },
        {
            "name": "React 18 + DOM App",
            "packages": ["react@18.2.0", "react-dom@18.2.0"],
            "deps": {
                "react": "^18.2.0",
                "react-dom": "^18.2.0"
            }
        },
        {
            "name": "TypeScript + Utility Suite",
            "packages": ["typescript@5.2.2", "dayjs@1.11.9", "zod@3.22.2", "commander@11.0.0"],
            "deps": {
                "typescript": "^5.2.2",
                "dayjs": "^1.11.9",
                "zod": "^3.22.2",
                "commander": "^11.0.0"
            }
        }
    ]

    REPEATS = 5

    def run_command(cmd, cwd, env=None):
        start = time.perf_counter_ns()
        res = subprocess.run(cmd, cwd=str(cwd), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        duration_ms = (time.perf_counter_ns() - start) / 1_000_000.0
        return duration_ms, res.returncode, res.stdout, res.stderr

    def run_bench():
        print("=" * 85)
        print("      JHOL (JAGR-Infinite + JAGR-S V3) vs BUN 1.3.14 BENCHMARK")
        print("=" * 85)
        print(f"Jhol Binary : {JHOL_BIN}")
        print(f"Bun Binary  : {BUN_BIN}")
        print(f"Repeats     : {REPEATS} per scenario")
        print("=" * 85)

        all_results = []

        with tempfile.TemporaryDirectory(prefix="jhol_vs_bun_") as td:
            root = Path(td)

            for wl in WORKLOADS:
                wl_name = wl["name"]
                print(f"\n[WORKLOAD: {wl_name}]")

                # Prepare folders
                jhol_dir = root / f"jhol_{wl_name.replace(' ', '_')}"
                bun_dir = root / f"bun_{wl_name.replace(' ', '_')}"
                jhol_dir.mkdir(parents=True, exist_ok=True)
                bun_dir.mkdir(parents=True, exist_ok=True)

                pkg_json = {
                    "name": "bench-app",
                    "version": "1.0.0",
                    "private": True,
                    "dependencies": wl["deps"]
                }
                (jhol_dir / "package.json").write_text(json.dumps(pkg_json, indent=2))
                (bun_dir / "package.json").write_text(json.dumps(pkg_json, indent=2))

                jhol_env = os.environ.copy()
                jhol_env["JHOL_NO_SPLASH"] = "1"
                jhol_env["JHOL_QUIET"] = "1"

                # -------------------------------------------------------------
                # 1. COLD INSTALL (Bundled / First-Pass)
                # -------------------------------------------------------------
                jhol_cold_runs = []
                bun_cold_runs = []

                for r in range(REPEATS):
                    # Jhol cold run in fresh directory
                    r_jhol_dir = root / f"jhol_cold_r{r}_{wl_name.replace(' ', '_')}"
                    r_jhol_dir.mkdir(parents=True, exist_ok=True)
                    (r_jhol_dir / "package.json").write_text(json.dumps(pkg_json, indent=2))

                    dur, rc, out, err = run_command([JHOL_BIN, "install", "--no-scripts", "-q"], r_jhol_dir, jhol_env)
                    if rc == 0:
                        jhol_cold_runs.append(dur)

                    # Bun cold run
                    r_bun_dir = root / f"bun_cold_r{r}_{wl_name.replace(' ', '_')}"
                    r_bun_dir.mkdir(parents=True, exist_ok=True)
                    (r_bun_dir / "package.json").write_text(json.dumps(pkg_json, indent=2))

                    dur_bun, rc_bun, out_bun, err_bun = run_command([BUN_BIN, "install", "--ignore-scripts"], r_bun_dir)
                    if rc_bun == 0:
                        bun_cold_runs.append(dur_bun)

                # -------------------------------------------------------------
                # 2. WARM INSTALL (node_modules removed, store/cache hot)
                # -------------------------------------------------------------
                # Prime directories so lockfiles/capsules exist for warm scenario
                run_command([JHOL_BIN, "install", "--no-scripts", "-q"], jhol_dir, jhol_env)
                run_command([BUN_BIN, "install", "--ignore-scripts"], bun_dir)

                jhol_warm_runs = []
                bun_warm_runs = []

                for _ in range(REPEATS):
                    # Jhol warm
                    if (jhol_dir / "node_modules").exists():
                        shutil.rmtree(jhol_dir / "node_modules")
                    dur, rc, out, err = run_command([JHOL_BIN, "install", "--no-scripts", "-q"], jhol_dir, jhol_env)
                    if rc == 0:
                        jhol_warm_runs.append(dur)

                    # Bun warm
                    if (bun_dir / "node_modules").exists():
                        shutil.rmtree(bun_dir / "node_modules")
                    dur_bun, rc_bun, out_bun, err_bun = run_command([BUN_BIN, "install", "--ignore-scripts"], bun_dir)
                    if rc_bun == 0:
                        bun_warm_runs.append(dur_bun)

                # -------------------------------------------------------------
                # 3. NO-OP RE-INSTALL (node_modules intact)
                # -------------------------------------------------------------
                jhol_noop_runs = []
                bun_noop_runs = []

                for _ in range(REPEATS):
                    dur, rc, out, err = run_command([JHOL_BIN, "install", "--no-scripts", "-q"], jhol_dir, jhol_env)
                    if rc == 0:
                        jhol_noop_runs.append(dur)

                    dur_bun, rc_bun, out_bun, err_bun = run_command([BUN_BIN, "install", "--ignore-scripts"], bun_dir)
                    if rc_bun == 0:
                        bun_noop_runs.append(dur_bun)

                def get_med(xs):
                    return sorted(xs)[len(xs)//2] if xs else 0.0

                j_cold_med = get_med(jhol_cold_runs)
                b_cold_med = get_med(bun_cold_runs)
                j_warm_med = get_med(jhol_warm_runs)
                b_warm_med = get_med(bun_warm_runs)
                j_noop_med = get_med(jhol_noop_runs)
                b_noop_med = get_med(bun_noop_runs)

                all_results.append({
                    "workload": wl_name,
                    "cold": {"jhol": j_cold_med, "bun": b_cold_med},
                    "warm": {"jhol": j_warm_med, "bun": b_warm_med},
                    "noop": {"jhol": j_noop_med, "bun": b_noop_med},
                })

                print(f"  Cold Install : Jhol = {j_cold_med:6.1f} ms  |  Bun = {b_cold_med:6.1f} ms  -> {'Jhol ' + str(round(b_cold_med/j_cold_med, 2)) + 'x FASTER' if j_cold_med < b_cold_med else 'Bun ' + str(round(j_cold_med/b_cold_med, 2)) + 'x FASTER'}")
                print(f"  Warm Install : Jhol = {j_warm_med:6.1f} ms  |  Bun = {b_warm_med:6.1f} ms  -> {'Jhol ' + str(round(b_warm_med/j_warm_med, 2)) + 'x FASTER' if j_warm_med < b_warm_med else 'Bun ' + str(round(j_warm_med/b_warm_med, 2)) + 'x FASTER'}")
                print(f"  No-Op Reinst : Jhol = {j_noop_med:6.1f} ms  |  Bun = {b_noop_med:6.1f} ms  -> {'Jhol ' + str(round(b_noop_med/j_noop_med, 2)) + 'x FASTER' if j_noop_med < b_noop_med else 'Bun ' + str(round(j_noop_med/b_noop_med, 2)) + 'x FASTER'}")

        print("\n" + "=" * 85)
        print("                        FINAL SUMMARY SCOREBOARD")
        print("=" * 85)
        print(f"{'Workload':<30} | {'Phase':<8} | {'Jhol (V3)':<12} | {'Bun 1.3.14':<12} | {'Verdict':<15}")
        print("-" * 85)

        for item in all_results:
            w = item["workload"]
            for ph in ["cold", "warm", "noop"]:
                j_val = item[ph]["jhol"]
                b_val = item[ph]["bun"]
                if j_val > 0 and b_val > 0:
                    if j_val < b_val:
                        verdict = f"Jhol {b_val/j_val:4.2f}x FASTER"
                    else:
                        verdict = f"Bun  {j_val/b_val:4.2f}x FASTER"
                    print(f"{w:<30} | {ph:<8} | {j_val:6.1f} ms   | {b_val:6.1f} ms   | {verdict:<15}")

        print("=" * 85)

        # Save to disk
        out_json = WORKSPACE / "benchmarks" / "jhol_vs_bun_latest.json"
        with open(out_json, "w") as f:
            json.dump(all_results, f, indent=2)
        print(f"Saved benchmark results to {out_json}")

    if __name__ == "__main__":
        run_bench()
    return 0


# ── brutal-cold-warm: was brutal_cold_warm_benchmark.py ──

def run_brutal(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "legacy_compare.py", *argv]
    help_rc = _exit_if_help(
        argv,
        "brutal_cold_warm_benchmark.py — interleaved cold/warm jhol vs bun (no flags).\n"
        "See scripts/bench/legacy_compare.py brutal-cold-warm",
    )
    if help_rc is not None:
        return help_rc
    """
    Brutal Cold & Warm Install Benchmark: Jhol vs Bun 1.3.14
    Measures cold, warm, and no-op install latencies with statistical rigor across 5 interleaved rounds.
    """

    import os
    import sys
    import json
    import time
    import shutil
    import tempfile
    import subprocess
    from pathlib import Path

    WORKSPACE = REPO_ROOT
    JHOL_BIN = str(REPO_ROOT / "target" / "release" / "jhol")
    BUN_BIN = os.path.expanduser("~/.bun/bin/bun")

    PROJECTS = {
        "bundled-synth-6": {
            "name": "bundled-synth-6",
            "deps": {
                "express": "^4.18.2",
                "lodash": "^4.17.21",
                "uuid": "^9.0.0",
                "chalk": "^5.3.0",
                "axios": "^1.6.0",
                "react": "^18.2.0"
            }
        },
        "react-app": {
            "name": "react-app",
            "deps": {
                "react": "^18.2.0",
                "react-dom": "^18.2.0",
                "react-scripts": "^5.0.1"
            }
        },
        "next-app": {
            "name": "next-app",
            "deps": {
                "next": "^14.0.0",
                "react": "^18.2.0",
                "react-dom": "^18.2.0",
                "typescript": "^5.4.0",
                "@types/react": "^18.2.0",
                "@types/node": "^20.0.0"
            }
        }
    }

    REPS = 5

    def run_cmd(cmd, cwd, env=None):
        start = time.perf_counter_ns()
        res = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        duration_ms = (time.perf_counter_ns() - start) / 1_000_000.0
        return duration_ms, res.returncode, res.stdout, res.stderr

    def setup_project_dir(proj_config, target_dir):
        target_dir = Path(target_dir)
        target_dir.mkdir(parents=True, exist_ok=True)
        pkg_json = {
            "name": proj_config["name"],
            "version": "1.0.0",
            "private": True,
            "dependencies": proj_config["deps"]
        }
        with open(target_dir / "package.json", "w") as f:
            json.dump(pkg_json, f, indent=2)
        return target_dir

    def benchmark_suite():
        results = {}
        tmp_root = Path(tempfile.mkdtemp(prefix="jhol_bench_"))

        print("=" * 80)
        print("BRUTAL BENCHMARK: Jhol (v1.0.2 with JAGR-S V3) vs Bun (1.3.14)")
        print(f"Reps per scenario: {REPS} (Interleaved)")
        print("=" * 80)

        for proj_key, proj_cfg in PROJECTS.items():
            print(f"\n>>> Running Project Workload: [{proj_key}]")
            results[proj_key] = {"cold": {"jhol": [], "bun": []}, "warm": {"jhol": [], "bun": []}, "noop": {"jhol": [], "bun": []}}

            for round_idx in range(REPS):
                print(f"  --- Round {round_idx + 1}/{REPS} ---")

                # --- COLD INSTALL JHOL ---
                jhol_tmp = tmp_root / f"jhol_cold_{proj_key}_{round_idx}"
                setup_project_dir(proj_cfg, jhol_tmp)
                jhol_cache = jhol_tmp / ".jhol_cache"
                jhol_env = os.environ.copy()
                jhol_env["JHOL_CACHE_DIR"] = str(jhol_cache)
                jhol_env["JHOL_NO_SPLASH"] = "1"

                dur_jhol_cold, rc, out, err = run_cmd([JHOL_BIN, "install", "--no-scripts", "--no-cache"], cwd=jhol_tmp, env=jhol_env)
                if rc == 0:
                    results[proj_key]["cold"]["jhol"].append(dur_jhol_cold)
                    print(f"    Jhol Cold: {dur_jhol_cold:6.1f} ms")
                else:
                    print(f"    Jhol Cold FAILED (rc={rc}): {err[:200]}")

                # --- COLD INSTALL BUN ---
                bun_tmp = tmp_root / f"bun_cold_{proj_key}_{round_idx}"
                setup_project_dir(proj_cfg, bun_tmp)
                bun_home = bun_tmp / ".bun_home"
                bun_env = os.environ.copy()
                bun_env["BUN_INSTALL"] = str(bun_home)
                bun_env["HOME"] = str(bun_home)

                dur_bun_cold, rc, out, err = run_cmd([BUN_BIN, "install", "--no-cache", "--ignore-scripts"], cwd=bun_tmp, env=bun_env)
                if rc == 0:
                    results[proj_key]["cold"]["bun"].append(dur_bun_cold)
                    print(f"    Bun  Cold: {dur_bun_cold:6.1f} ms")
                else:
                    print(f"    Bun  Cold FAILED (rc={rc}): {err[:200]}")

                # --- WARM INSTALL JHOL (node_modules removed, cache preserved) ---
                if (jhol_tmp / "node_modules").exists():
                    shutil.rmtree(jhol_tmp / "node_modules")
                dur_jhol_warm, rc, out, err = run_cmd([JHOL_BIN, "install", "--no-scripts"], cwd=jhol_tmp, env=jhol_env)
                if rc == 0:
                    results[proj_key]["warm"]["jhol"].append(dur_jhol_warm)
                    print(f"    Jhol Warm: {dur_jhol_warm:6.1f} ms")

                # --- WARM INSTALL BUN (node_modules removed, cache preserved) ---
                if (bun_tmp / "node_modules").exists():
                    shutil.rmtree(bun_tmp / "node_modules")
                dur_bun_warm, rc, out, err = run_cmd([BUN_BIN, "install", "--ignore-scripts"], cwd=bun_tmp, env=bun_env)
                if rc == 0:
                    results[proj_key]["warm"]["bun"].append(dur_bun_warm)
                    print(f"    Bun  Warm: {dur_bun_warm:6.1f} ms")

                # --- NO-OP INSTALL JHOL (node_modules already populated) ---
                dur_jhol_noop, rc, out, err = run_cmd([JHOL_BIN, "install", "--no-scripts"], cwd=jhol_tmp, env=jhol_env)
                if rc == 0:
                    results[proj_key]["noop"]["jhol"].append(dur_jhol_noop)

                # --- NO-OP INSTALL BUN (node_modules already populated) ---
                dur_bun_noop, rc, out, err = run_cmd([BUN_BIN, "install", "--ignore-scripts"], cwd=bun_tmp, env=bun_env)
                if rc == 0:
                    results[proj_key]["noop"]["bun"].append(dur_bun_noop)

        # Clean up temp directories
        shutil.rmtree(tmp_root, ignore_errors=True)

        # --- SUMMARY OUTPUT ---
        print("\n" + "=" * 80)
        print("FINAL BENCHMARK COMPARISON TABLE (Medians across 5 rounds)")
        print("=" * 80)
        print(f"{'Workload':<20} | {'Phase':<8} | {'Jhol (v1.0.2)':<15} | {'Bun (1.3.14)':<15} | {'Advantage':<15}")
        print("-" * 80)

        summary_records = []
        for proj_key, data in results.items():
            for phase in ["cold", "warm", "noop"]:
                jhol_vals = sorted(data[phase]["jhol"])
                bun_vals = sorted(data[phase]["bun"])
                if not jhol_vals or not bun_vals:
                    continue
                jhol_med = jhol_vals[len(jhol_vals) // 2]
                bun_med = bun_vals[len(bun_vals) // 2]

                if jhol_med < bun_med:
                    adv = f"Jhol {bun_med/jhol_med:4.2f}x FASTER"
                else:
                    adv = f"Bun  {jhol_med/bun_med:4.2f}x FASTER"

                print(f"{proj_key:<20} | {phase:<8} | {jhol_med:7.1f} ms      | {bun_med:7.1f} ms      | {adv:<15}")
                summary_records.append({
                    "workload": proj_key,
                    "phase": phase,
                    "jhol_median_ms": jhol_med,
                    "bun_median_ms": bun_med,
                    "advantage": adv
                })

        out_file = WORKSPACE / "benchmarks" / "brutal_cold_warm_latest.json"
        with open(out_file, "w") as f:
            json.dump({"timestamp": time.time(), "records": summary_records, "raw": results}, f, indent=2)
        print(f"\nFull detailed results saved to: {out_file}")

    if __name__ == "__main__":
        benchmark_suite()
    return 0


# ── measure-cold: was measure_cold_benchmarks.py ──

import os
import sys
import json
import time
import shutil
import tempfile
import subprocess
from pathlib import Path

WORKSPACE = REPO_ROOT
JHOL_BIN = str(REPO_ROOT / "target" / "release" / "jhol")
BUN_BIN = os.path.expanduser("~/.bun/bin/bun")

WORKLOADS = [
    {
        "name": "Express",
        "deps": {"express": "^4.18.2"}
    },
    {
        "name": "React + React-DOM",
        "deps": {"react": "^18.2.0", "react-dom": "^18.2.0"}
    },
    {
        "name": "TypeScript + Dayjs",
        "deps": {"typescript": "^5.2.2", "dayjs": "^1.11.9"}
    },
    {
        "name": "Deep Dependency Tree",
        "deps": {
            "express": "^4.18.2",
            "body-parser": "^1.20.2",
            "cookie-parser": "^1.4.6",
            "morgan": "^1.10.0",
            "compression": "^1.7.4",
            "cors": "^2.8.5",
            "helmet": "^7.0.0"
        }
    }
]

def nuke_jhol_cache():
    for p in [os.path.expanduser("~/.jhol-cache"), os.path.expanduser("~/.jhol_cache")]:
        shutil.rmtree(p, ignore_errors=True)

def nuke_bun_cache():
    subprocess.run([BUN_BIN, "pm", "cache", "rm"], capture_output=True)
    shutil.rmtree(os.path.expanduser("~/.bun/install/cache"), ignore_errors=True)

def run_cmd(cmd, cwd, env=None):
    start = time.perf_counter_ns()
    res = subprocess.run(cmd, cwd=str(cwd), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    duration_ms = (time.perf_counter_ns() - start) / 1_000_000.0
    return duration_ms, res.returncode, res.stdout, res.stderr

def run_measure_cold(argv: list[str] | None = None) -> int:
    import sys
    if argv is not None:
        sys.argv = [sys.argv[0] if sys.argv else "legacy_compare.py", *argv]
    help_rc = _exit_if_help(
        argv,
        "measure_cold_benchmarks.py — cold install workloads jhol vs bun (no flags).\n"
        "See scripts/bench/legacy_compare.py measure-cold",
    )
    if help_rc is not None:
        return help_rc

    print(f"Testing Cold Install Performance: Jhol vs Bun 1.4.x")
    print(f"Jhol Binary: {JHOL_BIN}")
    print(f"Bun Binary:  {BUN_BIN}")
    print("-" * 75)

    base_env = os.environ.copy()
    base_env["JHOL_NO_SPLASH"] = "1"
    base_env["JHOL_QUIET"] = "1"

    for wl in WORKLOADS:
        wl_name = wl["name"]
        deps = wl["deps"]
        print(f"\n>>> Running Workload: {wl_name}")

        jhol_cold_times = []
        bun_cold_times = []

        # Run 3 repeats for each tool
        for run_idx in range(3):
            # Test Jhol Cold
            with tempfile.TemporaryDirectory(prefix=f"jhol_cold_{run_idx}_") as td:
                p = Path(td)
                (p / "package.json").write_text(json.dumps({"name": "test-app", "dependencies": deps}, indent=2))
                nuke_jhol_cache()
                dur, rc, out, err = run_cmd([JHOL_BIN, "install", "--no-scripts", "-q"], p, base_env)
                if rc == 0:
                    jhol_cold_times.append(dur)
                else:
                    print(f"  [Jhol Run {run_idx}] FAILED ({rc}): {err[:200]}")

            # Test Bun Cold
            with tempfile.TemporaryDirectory(prefix=f"bun_cold_{run_idx}_") as td:
                p = Path(td)
                (p / "package.json").write_text(json.dumps({"name": "test-app", "dependencies": deps}, indent=2))
                nuke_bun_cache()
                dur, rc, out, err = run_cmd([BUN_BIN, "install", "--ignore-scripts"], p)
                if rc == 0:
                    bun_cold_times.append(dur)
                else:
                    print(f"  [Bun Run {run_idx}] FAILED ({rc}): {err[:200]}")

        avg_jhol = sum(jhol_cold_times) / len(jhol_cold_times) if jhol_cold_times else 0
        min_jhol = min(jhol_cold_times) if jhol_cold_times else 0
        avg_bun = sum(bun_cold_times) / len(bun_cold_times) if bun_cold_times else 0
        min_bun = min(bun_cold_times) if bun_cold_times else 0
        ratio = avg_bun / avg_jhol if avg_jhol > 0 else 0

        print(f"  Jhol Cold (avg / min): {avg_jhol:.1f}ms / {min_jhol:.1f}ms (runs: {[round(x,1) for x in jhol_cold_times]})")
        print(f"  Bun  Cold (avg / min): {avg_bun:.1f}ms / {min_bun:.1f}ms (runs: {[round(x,1) for x in bun_cold_times]})")
        speedup = f"{ratio:.2f}x faster than Bun" if ratio >= 1.0 else f"{1.0/ratio:.2f}x of Bun"
        print(f"  -> Outcome: Jhol is {speedup}")


_MODES = {
    "jhol-vs-bun": run_jhol_vs_bun,
    "brutal-cold-warm": run_brutal,
    "measure-cold": run_measure_cold,
}

_HELP = """
Modes: jhol-vs-bun | brutal-cold-warm | measure-cold
"""


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0] in ("-h", "--help", "help"):
        print(_HELP.strip())
        return 0
    mode = argv[0]
    if mode not in _MODES:
        raise SystemExit(f"unknown mode {mode!r}; choose: {', '.join(_MODES)}\n{_HELP}")
    rc = _MODES[mode](argv[1:])
    return int(rc or 0)


if __name__ == "__main__":
    raise SystemExit(main())
