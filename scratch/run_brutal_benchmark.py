import os
import sys
import shutil
import time
import subprocess
import json
from pathlib import Path

JHOL = os.path.expanduser("~/j/jhol/target/release/jhol")
BUN = shutil.which("bun")

PROJECTS = [
    (
        "medium (3)",
        {
            "name": "bench-medium",
            "dependencies": {
                "express": "4.18.2",
                "react": "18.2.0",
                "react-dom": "18.2.0",
            },
        },
    ),
    (
        "react-app (2)",
        {
            "name": "bench-react-app",
            "dependencies": {
                "react": "18.2.0",
                "react-dom": "18.2.0",
            },
        },
    ),
    (
        "next-app (3)",
        {
            "name": "bench-next-app",
            "dependencies": {
                "next": "14.2.5",
                "react": "18.2.0",
                "react-dom": "18.2.0",
            },
        },
    ),
    (
        "jagr-bench (20)",
        {
            "name": "jagr-instance-bench",
            "dependencies": {
                "lodash": "^4.17.21",
                "react": "^18.3.1",
                "react-dom": "^18.3.1",
                "axios": "^1.7.9",
                "mime-types": "^2.1.35",
                "qs": "^6.14.0",
                "typescript": "^5.7.3",
                "commander": "^13.1.0",
                "chalk": "^5.4.1",
                "uuid": "^11.0.5",
                "dayjs": "^1.11.13",
                "zod": "^3.24.1",
                "@types/node": "^22.13.1",
                "fs-extra": "^11.3.0",
                "dotenv": "^16.4.7",
                "debug": "^2.6.9",
                "semver": "^7.7.1",
                "jsonwebtoken": "^9.0.2",
                "bcryptjs": "^2.4.3",
                "cors": "^2.8.5",
            },
        },
    ),
]

def run_cmd(cmd, cwd):
    t0 = time.perf_counter()
    res = subprocess.run(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    t1 = time.perf_counter()
    duration_ms = int((t1 - t0) * 1000)
    return duration_ms, res.returncode, res.stderr.decode("utf-8", "replace")

def median(lst):
    s = sorted(lst)
    return s[len(s) // 2]

def clean_dir(d):
    for item in ["node_modules", "package-lock.json", "bun.lock", "bun.lockb", "jhol-lock.json"]:
        p = d / item
        if p.is_dir():
            shutil.rmtree(p, ignore_errors=True)
        elif p.exists():
            p.unlink()

def clean_caches():
    shutil.rmtree(os.path.expanduser("~/.jhol-cache"), ignore_errors=True)
    shutil.rmtree(os.path.expanduser("~/.cache/jhol"), ignore_errors=True)
    shutil.rmtree(os.path.expanduser("~/.bun/install/cache"), ignore_errors=True)

def main():
    print("=" * 80, flush=True)
    print("  jhol vs bun — BRUTAL Install Bench (macOS APFS)", flush=True)
    print("  (3 repeats, median, cold=no cache, warm=cached, done=recheck)", flush=True)
    print("=" * 80, flush=True)
    print(f"{'Project':<18} {'bun (cold / warm)':<24} {'jhol (cold / warm / done)':<32} {'Ratio'}", flush=True)
    print("-" * 80, flush=True)

    work_dir = Path("/tmp/jhol_brutal_bench")
    if work_dir.exists():
        shutil.rmtree(work_dir)
    work_dir.mkdir(parents=True)

    for name, manifest in PROJECTS:
        proj_dir = work_dir / name.split()[0]
        proj_dir.mkdir(parents=True, exist_ok=True)
        (proj_dir / "package.json").write_text(json.dumps(manifest, indent=2))

        # Bench Bun
        bun_cold_times = []
        bun_warm_times = []
        if BUN:
            for _ in range(3):
                clean_dir(proj_dir)
                clean_caches()
                ms, code, err = run_cmd([BUN, "install", "--no-save"], proj_dir)
                bun_cold_times.append(ms)

                # warm
                p_nm = proj_dir / "node_modules"
                if p_nm.exists():
                    shutil.rmtree(p_nm)
                ms, code, err = run_cmd([BUN, "install", "--no-save"], proj_dir)
                bun_warm_times.append(ms)

        bun_cold_med = median(bun_cold_times) if bun_cold_times else 0
        bun_warm_med = median(bun_warm_times) if bun_warm_times else 0

        # Bench Jhol
        jhol_cold_times = []
        jhol_warm_times = []
        jhol_done_times = []
        for _ in range(3):
            clean_dir(proj_dir)
            clean_caches()
            ms, code, err = run_cmd([JHOL, "install", "--no-cache"], proj_dir)
            jhol_cold_times.append(ms)

            # warm
            p_nm = proj_dir / "node_modules"
            if p_nm.exists():
                shutil.rmtree(p_nm)
            ms, code, err = run_cmd([JHOL, "install"], proj_dir)
            jhol_warm_times.append(ms)

            # done (recheck)
            ms, code, err = run_cmd([JHOL, "install"], proj_dir)
            jhol_done_times.append(ms)

        jhol_cold_med = median(jhol_cold_times)
        jhol_warm_med = median(jhol_warm_times)
        jhol_done_med = median(jhol_done_times)

        ratio = f"[🏆 {bun_cold_med / jhol_cold_med:.2f}x FASTER COLD]" if jhol_cold_med and bun_cold_med else ""
        bun_str = f"{bun_cold_med}ms / {bun_warm_med}ms"
        jhol_str = f"{jhol_cold_med}ms / {jhol_warm_med}ms / {jhol_done_med}ms"

        print(f"{name:<18} {bun_str:<24} {jhol_str:<32} {ratio}", flush=True)

    print("=" * 80, flush=True)

if __name__ == "__main__":
    main()
