import subprocess
import time
import os
import shutil
import json
from pathlib import Path

JHOL_BIN = "/Users/bhuvanprakash/j/jhol/target/release/jhol"
BUN_BIN = shutil.which("bun") or "/Users/bhuvanprakash/.bun/bin/bun"
BENCH_DIR = Path("/tmp/jhol-benchmark-suite")
CACHE_DIR = Path("/tmp/jhol-bench-cache")

FIXTURES = {
    "express-app": {
        "name": "express-app",
        "version": "1.0.0",
        "dependencies": {
            "express": "4.18.2",
            "cors": "2.8.5",
            "dotenv": "16.3.1"
        }
    },
    "react-app": {
        "name": "react-app",
        "version": "1.0.0",
        "dependencies": {
            "react": "18.2.0",
            "react-dom": "18.2.0"
        }
    },
    "utility-app": {
        "name": "utility-app",
        "version": "1.0.0",
        "dependencies": {
            "chalk": "4.1.2",
            "commander": "10.0.1",
            "lodash": "4.17.21"
        }
    }
}

def time_cmd(cmd, cwd, env=None):
    start = time.perf_counter()
    res = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    elapsed = time.perf_counter() - start
    return elapsed, res.returncode, res.stdout, res.stderr

def run_suite():
    if BENCH_DIR.exists():
        shutil.rmtree(BENCH_DIR)
    BENCH_DIR.mkdir(parents=True)
    
    results = {}

    for fix_name, fix_pkg in FIXTURES.items():
        results[fix_name] = {}
        fix_dir = BENCH_DIR / fix_name
        fix_dir.mkdir(parents=True)
        
        # ── 1. COLD INSTALL ──
        jhol_work = fix_dir / "jhol_cold"
        jhol_work.mkdir(parents=True)
        with open(jhol_work / "package.json", "w") as f:
            json.dump(fix_pkg, f)
        
        jhol_cache = CACHE_DIR / f"{fix_name}_jhol_cache"
        if jhol_cache.exists():
            shutil.rmtree(jhol_cache)
        jhol_env = {**os.environ, "JHOL_CACHE_DIR": str(jhol_cache)}
        
        t_jhol_cold, rc_jc, _, err_jc = time_cmd([JHOL_BIN, "install"], jhol_work, jhol_env)
        
        bun_work = fix_dir / "bun_cold"
        bun_work.mkdir(parents=True)
        with open(bun_work / "package.json", "w") as f:
            json.dump(fix_pkg, f)
        bun_cache = CACHE_DIR / f"{fix_name}_bun_cache"
        if bun_cache.exists():
            shutil.rmtree(bun_cache)
        bun_env = {**os.environ, "BUN_INSTALL_CACHE_DIR": str(bun_cache)}
        
        t_bun_cold, rc_bc, _, err_bc = time_cmd([BUN_BIN, "install"], bun_work, bun_env)
        
        results[fix_name]["cold"] = {
            "jhol": round(t_jhol_cold, 4),
            "bun": round(t_bun_cold, 4),
            "jhol_ok": rc_jc == 0,
            "bun_ok": rc_bc == 0,
        }

        # ── 2. WARM INSTALL WITH LOCKFILE (Clean node_modules) ──
        if (jhol_work / "node_modules").exists():
            shutil.rmtree(jhol_work / "node_modules")
        t_jhol_warm_lock, rc_jwl, _, _ = time_cmd([JHOL_BIN, "install"], jhol_work, jhol_env)
        
        if (bun_work / "node_modules").exists():
            shutil.rmtree(bun_work / "node_modules")
        t_bun_warm_lock, rc_bwl, _, _ = time_cmd([BUN_BIN, "install"], bun_work, bun_env)
        
        results[fix_name]["warm_with_lockfile"] = {
            "jhol": round(t_jhol_warm_lock, 4),
            "bun": round(t_bun_warm_lock, 4),
            "jhol_ok": rc_jwl == 0,
            "bun_ok": rc_bwl == 0,
        }

        # ── 3. WARM INSTALL NO LOCKFILE (Cache hit, no lockfile, clean node_modules) ──
        jhol_nolock = fix_dir / "jhol_nolock"
        jhol_nolock.mkdir(parents=True)
        with open(jhol_nolock / "package.json", "w") as f:
            json.dump(fix_pkg, f)
        t_jhol_warm_nolock, rc_jwnl, _, _ = time_cmd([JHOL_BIN, "install"], jhol_nolock, jhol_env)

        bun_nolock = fix_dir / "bun_nolock"
        bun_nolock.mkdir(parents=True)
        with open(bun_nolock / "package.json", "w") as f:
            json.dump(fix_pkg, f)
        t_bun_warm_nolock, rc_bwnl, _, _ = time_cmd([BUN_BIN, "install"], bun_nolock, bun_env)

        results[fix_name]["warm_no_lockfile"] = {
            "jhol": round(t_jhol_warm_nolock, 4),
            "bun": round(t_bun_warm_nolock, 4),
            "jhol_ok": rc_jwnl == 0,
            "bun_ok": rc_bwnl == 0,
        }

        # ── 4. INCREMENTAL ADD (+1 dependency) ──
        t_jhol_add, rc_ja, _, _ = time_cmd([JHOL_BIN, "add", "ms@2.1.3"], jhol_work, jhol_env)
        t_bun_add, rc_ba, _, _ = time_cmd([BUN_BIN, "add", "ms@2.1.3"], bun_work, bun_env)

        results[fix_name]["incremental_add"] = {
            "jhol": round(t_jhol_add, 4),
            "bun": round(t_bun_add, 4),
            "jhol_ok": rc_ja == 0,
            "bun_ok": rc_ba == 0,
        }

    print(json.dumps(results, indent=2))
    with open("/tmp/jhol_benchmark_results.json", "w") as f:
        json.dump(results, f, indent=2)

if __name__ == "__main__":
    run_suite()
