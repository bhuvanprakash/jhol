#!/usr/bin/env python3
"""
Live benchmark: jhol vs npm vs yarn vs pnpm vs bun
Tests cold install (nuked cache + node_modules) and warm install (cache only, no node_modules)
"""
import os, subprocess, shutil, time, json, tempfile, statistics, sys
from pathlib import Path

JHOL = str(Path(__file__).parent.parent / "target" / "release" / "jhol")
RUNS = 5
SMALL_PKGS = ["lodash", "axios", "chalk"]
MEDIUM_PKGS = ["react", "typescript", "express"]

C = {
    "red":    "\033[0;31m",
    "green":  "\033[0;32m",
    "yellow": "\033[1;33m",
    "cyan":   "\033[0;36m",
    "bold":   "\033[1m",
    "reset":  "\033[0m",
}
def c(color, s): return f"{C[color]}{s}{C['reset']}"
def log(s): print(c("cyan", f"[bench] {s}"), flush=True)
def header(s):
    bar = "━" * 50
    print(f"\n{c('yellow', bar)}\n{c('yellow', '  ' + s)}\n{c('yellow', bar)}", flush=True)


def get_jhol_cache_dirs():
    """All places jhol might store its cache.
    utils.rs get_cache_dir() returns $HOME/.jhol-cache (dash, not underscore).
    """
    dirs = []
    # Primary: $HOME/.jhol-cache (matches utils.rs get_cache_dir())
    dirs.append(os.path.expanduser("~/.jhol-cache"))
    # Fallbacks (legacy / XDG variants)
    xdg = os.environ.get("XDG_CACHE_HOME", os.path.expanduser("~/.cache"))
    dirs.append(os.path.join(xdg, "jhol"))
    dirs.append(os.path.expanduser("~/.jhol_cache"))
    dirs.append(os.path.expanduser("~/.jhol"))
    return dirs


def nuke_cache(tool):
    """Nuke tool-specific global cache for true cold install."""
    if tool == "jhol":
        for d in get_jhol_cache_dirs():
            shutil.rmtree(d, ignore_errors=True)
    elif tool == "npm":
        subprocess.run(["npm", "cache", "clean", "--force"], capture_output=True)
    elif tool == "yarn":
        subprocess.run(["yarn", "cache", "clean"], capture_output=True)
    elif tool == "pnpm":
        subprocess.run(["pnpm", "store", "prune"], capture_output=True)
    elif tool == "bun":
        subprocess.run(["bun", "pm", "cache", "rm"], capture_output=True)


def nuke_workdir(workdir):
    """Remove all install artifacts from workdir."""
    for name in ["node_modules", ".jhol_cache", ".npm", ".yarn",
                 "bun.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"]:
        p = os.path.join(workdir, name)
        if os.path.isdir(p):
            shutil.rmtree(p, ignore_errors=True)
        elif os.path.isfile(p):
            os.remove(p)


def run_install(tool, pkgs, workdir):
    """Run install and return elapsed seconds."""
    pkgs_str = pkgs  # list passed as args
    env = os.environ.copy()
    
    if tool == "jhol":
        cmd = [JHOL, "install"] + pkgs
    elif tool == "npm":
        cmd = ["npm", "install", "--no-audit", "--no-fund"] + pkgs
    elif tool == "yarn":
        cmd = ["yarn", "add"] + pkgs
    elif tool == "pnpm":
        cmd = ["pnpm", "add"] + pkgs
    elif tool == "bun":
        cmd = ["bun", "add"] + pkgs
    
    t0 = time.perf_counter()
    subprocess.run(cmd, cwd=workdir, capture_output=True, env=env)
    return time.perf_counter() - t0


def setup_workdir(d):
    os.makedirs(d, exist_ok=True)
    pj = os.path.join(d, "package.json")
    with open(pj, "w") as f:
        json.dump({"name": "bench-test", "version": "1.0.0", "dependencies": {}}, f)


def cold_install_bench(tool, pkgs, label):
    """Run N cold installs, return (runs, median)."""
    workdir = tempfile.mkdtemp(prefix=f"jhol_cold_{tool}_")
    setup_workdir(workdir)
    runs = []
    print(f"  {c('bold', tool)} [{label}] cold:", end="", flush=True)
    for i in range(RUNS):
        nuke_workdir(workdir)
        nuke_cache(tool)
        t = run_install(tool, pkgs, workdir)
        runs.append(round(t, 3))
        print(f"  run{i+1}:{t:.3f}s", end="", flush=True)
    print()
    shutil.rmtree(workdir, ignore_errors=True)
    med = round(statistics.median(runs), 3)
    avg = round(statistics.mean(runs), 3)
    return runs, med, avg


def warm_install_bench(tool, pkgs, label):
    """Populate cache once, then run N warm installs (no node_modules)."""
    workdir = tempfile.mkdtemp(prefix=f"jhol_warm_{tool}_")
    setup_workdir(workdir)
    # seed run
    run_install(tool, pkgs, workdir)
    runs = []
    print(f"  {c('bold', tool)} [{label}] warm:", end="", flush=True)
    for i in range(RUNS):
        shutil.rmtree(os.path.join(workdir, "node_modules"), ignore_errors=True)
        t = run_install(tool, pkgs, workdir)
        runs.append(round(t, 3))
        print(f"  run{i+1}:{t:.3f}s", end="", flush=True)
    print()
    shutil.rmtree(workdir, ignore_errors=True)
    med = round(statistics.median(runs), 3)
    avg = round(statistics.mean(runs), 3)
    return runs, med, avg


def jhol_profile_run(pkgs):
    """Run jhol with JHOL_PROFILE_INSTALL=1 and capture stage timings."""
    workdir = tempfile.mkdtemp(prefix="jhol_profile_")
    setup_workdir(workdir)
    # nuke cache for true cold
    for d in get_jhol_cache_dirs():
        shutil.rmtree(d, ignore_errors=True)
    env = os.environ.copy()
    env["JHOL_PROFILE_INSTALL"] = "1"
    cmd = [JHOL, "install"] + pkgs
    result = subprocess.run(cmd, cwd=workdir, capture_output=True, text=True, env=env)
    shutil.rmtree(workdir, ignore_errors=True)
    lines = []
    for line in (result.stdout + result.stderr).splitlines():
        if "[jhol-profile]" in line:
            lines.append(line)
    return lines


def available_tools():
    tools = []
    if os.path.isfile(JHOL) and os.access(JHOL, os.X_OK):
        tools.append("jhol")
    else:
        print(c("red", f"WARNING: jhol not found at {JHOL}"))
    for t in ["npm", "yarn", "pnpm", "bun"]:
        if shutil.which(t):
            tools.append(t)
    return tools


def ratio_str(a, b):
    if b and b > 0:
        return f"{a/b:.2f}x"
    return "N/A"


def main():
    tools = available_tools()
    print(c("green", f"Tools: {', '.join(tools)}"))
    print(c("green", f"Runs per benchmark: {RUNS}"))

    results = {
        "cold_small": {},
        "cold_medium": {},
        "warm_small": {},
        "warm_medium": {},
    }

    # ─── COLD SMALL ──────────────────────────────────────────────────────────
    header(f"COLD INSTALL: small ({' '.join(SMALL_PKGS)}) — {RUNS} runs")
    for tool in tools:
        runs, med, avg = cold_install_bench(tool, SMALL_PKGS, "small")
        results["cold_small"][tool] = {"runs": runs, "median": med, "avg": avg}
        print(f"    → {c('green', f'median={med}s')}  avg={avg}s  runs={runs}")

    # ─── COLD MEDIUM ─────────────────────────────────────────────────────────
    header(f"COLD INSTALL: medium ({' '.join(MEDIUM_PKGS)}) — {RUNS} runs")
    for tool in tools:
        runs, med, avg = cold_install_bench(tool, MEDIUM_PKGS, "medium")
        results["cold_medium"][tool] = {"runs": runs, "median": med, "avg": avg}
        print(f"    → {c('green', f'median={med}s')}  avg={avg}s  runs={runs}")

    # ─── WARM SMALL ──────────────────────────────────────────────────────────
    header(f"WARM INSTALL: small ({' '.join(SMALL_PKGS)}) — {RUNS} runs")
    for tool in tools:
        runs, med, avg = warm_install_bench(tool, SMALL_PKGS, "small")
        results["warm_small"][tool] = {"runs": runs, "median": med, "avg": avg}
        print(f"    → {c('green', f'median={med}s')}  avg={avg}s  runs={runs}")

    # ─── WARM MEDIUM ─────────────────────────────────────────────────────────
    header(f"WARM INSTALL: medium ({' '.join(MEDIUM_PKGS)}) — {RUNS} runs")
    for tool in tools:
        runs, med, avg = warm_install_bench(tool, MEDIUM_PKGS, "medium")
        results["warm_medium"][tool] = {"runs": runs, "median": med, "avg": avg}
        print(f"    → {c('green', f'median={med}s')}  avg={avg}s  runs={runs}")

    # ─── JHOL PROFILE ────────────────────────────────────────────────────────
    header("JHOL INTERNAL STAGE PROFILE (cold, small packages)")
    profile_lines = jhol_profile_run(SMALL_PKGS)
    if profile_lines:
        for line in profile_lines:
            print(f"  {c('cyan', line)}")
    else:
        print(c("yellow", "  No profile output — JHOL_PROFILE_INSTALL may not be wired. Check stderr."))

    # ─── SUMMARY TABLE ───────────────────────────────────────────────────────
    header("BENCHMARK SUMMARY (median seconds, lower=faster)")
    print()
    col = 13
    header_row = f"{'Tool':<10} {'Cold-Small':>{col}} {'Cold-Medium':>{col}} {'Warm-Small':>{col}} {'Warm-Medium':>{col}}"
    print(c("bold", header_row))
    print("─" * (10 + col*4 + 4))
    for tool in tools:
        cs  = results["cold_small"].get(tool, {}).get("median", "N/A")
        cm  = results["cold_medium"].get(tool, {}).get("median", "N/A")
        ws  = results["warm_small"].get(tool, {}).get("median", "N/A")
        wm  = results["warm_medium"].get(tool, {}).get("median", "N/A")
        row = f"{tool:<10} {str(cs)+'s':>{col}} {str(cm)+'s':>{col}} {str(ws)+'s':>{col}} {str(wm)+'s':>{col}}"
        clr = "green" if tool == "jhol" else "reset"
        print(c(clr, row))

    # ─── RATIOS ──────────────────────────────────────────────────────────────
    print()
    if "jhol" in tools:
        jhol_cs = results["cold_small"].get("jhol", {}).get("median")
        jhol_cm = results["cold_medium"].get("jhol", {}).get("median")
        jhol_ws = results["warm_small"].get("jhol", {}).get("median")
        for other in [t for t in tools if t != "jhol"]:
            b_cs = results["cold_small"].get(other, {}).get("median")
            b_cm = results["cold_medium"].get(other, {}).get("median")
            b_ws = results["warm_small"].get(other, {}).get("median")
            print(c("yellow", f"jhol vs {other}:  cold-small={ratio_str(jhol_cs, b_cs)}  cold-medium={ratio_str(jhol_cm, b_cm)}  warm-small={ratio_str(jhol_ws, b_ws)}"))

    # ─── BOTTLENECK ANALYSIS ─────────────────────────────────────────────────
    header("BOTTLENECK ANALYSIS")
    if "jhol" in tools and "bun" in tools:
        jhol_cs = results["cold_small"].get("jhol", {}).get("median", 999)
        bun_cs  = results["cold_small"].get("bun",  {}).get("median", 1)
        jhol_ws = results["warm_small"].get("jhol", {}).get("median", 999)
        bun_ws  = results["warm_small"].get("bun",  {}).get("median", 1)
        
        print(f"\n  Cold gap vs bun: jhol={jhol_cs}s  bun={bun_cs}s  ratio={ratio_str(jhol_cs, bun_cs)}")
        print(f"  Warm gap vs bun: jhol={jhol_ws}s  bun={bun_ws}s  ratio={ratio_str(jhol_ws, bun_ws)}")
        
        cold_gap = jhol_cs - bun_cs if jhol_cs and bun_cs else 0
        warm_gap = jhol_ws - bun_ws if jhol_ws and bun_ws else 0
        network_portion = cold_gap - warm_gap
        
        print(f"\n  Estimated time breakdown:")
        print(f"    Network/resolve delta  : {network_portion:.3f}s (cold-warm gap difference vs bun)")
        print(f"    Extraction/linking delta: {warm_gap:.3f}s (warm install gap vs bun)")
        
        print(f"\n  {c('bold', 'Likely bottlenecks (priority order):')}")
        bottlenecks = []
        
        if network_portion > 0.3:
            bottlenecks.append((1, "HTTP/resolve", 
                "Registry resolution: Jhol does sequential manifest→packument HTTP round trips. "
                "Bun uses a global module registry with pre-resolved manifests stored in a binary B-tree. "
                "FIX: Implement parallel tarball URL resolution via direct CDN manifest fetch (no packument for known packages)."))
        
        if warm_gap > 0.1:
            bottlenecks.append((2, "Cache extraction", 
                "Warm install still extracting tarballs. "
                "Bun uses hardlinks from a global content-addressed store — O(1) per file. "
                "FIX: Ensure store → node_modules always uses hardlinks (not copy or symlink). "
                "Check if link_package_from_store falls back to copy on macOS APFS."))
        
        if jhol_ws > 0.05:
            bottlenecks.append((3, "Process startup + store index",
                "Even warm installs have overhead. "
                "Bun has ~15ms startup. Jhol reads store index JSON from disk on every run. "
                "FIX: Memory-map the store index or use a compact binary format (not JSON)."))
        
        bottlenecks.append((4, "Tokio async vs threads",
            "Jhol uses std::thread for concurrency. Bun uses a custom event loop (JavaScriptCore + liburing on Linux). "
            "FIX: Migrate HTTP downloads to tokio + reqwest async to maximize I/O concurrency without thread overhead."))
        
        bottlenecks.append((5, "Tarball extraction",
            "Jhol extracts .tgz with synchronous flate2. "
            "Bun uses zstd (Zstandard) compression in its store and parallelizes extraction. "
            "FIX: Use zstd for jhol's internal store format; keep .tgz only for download."))
        
        for rank, name, desc in bottlenecks:
            print(f"\n  {c('yellow', f'#{rank} [{name}]')}")
            print(f"    {desc}")

    # ─── SAVE JSON ───────────────────────────────────────────────────────────
    ts = time.strftime("%Y%m%d-%H%M%S")
    out = Path(__file__).parent.parent / "benchmarks" / f"live-bench-{ts}.json"
    with open(out, "w") as f:
        json.dump({
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "runs": RUNS,
            "tools": tools,
            "results": results,
        }, f, indent=2)
    print(f"\n{c('green', f'Results saved → {out}')}")


if __name__ == "__main__":
    main()
