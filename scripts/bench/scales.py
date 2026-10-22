#!/usr/bin/env python3
"""Seven-scale jhol benchmark (macOS/Apple-Silicon focus, works anywhere).

    python3 scripts/bench_scales.py            # run all seven, print a table
    python3 scripts/bench_scales.py --json out.json

Scales
------
  1 cold-1        one dependency, cold cache
  2 cold-2        two dependencies, cold cache
  3 cold-5        five dependencies, cold cache
  4 cold-20       the large fixture (20 direct deps, ~74 packages), cold cache
  5 cold-next     next + react + react-dom (~300 packages), cold cache
  6 warm-cache    the 20-dep fixture with a *warmed cache* but a fresh project
  7 warm-noop     an already-installed project re-installed unchanged

Protocol (the one this project settled on after several bad measurements)
  * every cold run gets a **fresh cache directory and a fresh HOME** — both matter:
    Bun otherwise serves its global cache and reports a "cold" install in 53 ms;
  * warm runs pre-warm once (discarded) and then measure;
  * every run is asserted to produce a usable tree (package present, `node_modules`
    populated), so a fast wrong answer cannot win;
  * medians are reported, with min/max, and the run count is explicit.
"""
import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
J = os.environ.get("JHOL_BIN", os.path.join(ROOT, "target/release/jhol"))
BENCH = os.path.expanduser("~/j/jhol-bench")

SCALES = [
    # name,          fixture package.json,                      expect package, runs, mode
    ("1 cold-1", [("debug", "^4.4.3")], "debug", 3, "cold"),
    ("2 cold-2", [("debug", "^4.4.3"), ("ms", "2.1.3")], "debug", 3, "cold"),
    ("3 cold-5", [("debug", "^4.4.3"), ("ms", "2.1.3"), ("semver", "^7.7.1"),
                  ("is-number", "7.0.0"), ("chalk", "5.6.2")], "semver", 3, "cold"),
    ("4 cold-20", os.path.join(BENCH, "large-test/package.json"), "axios", 3, "cold"),
    ("5 cold-next", os.path.join(BENCH, "next-test/package.json"), "next", 3, "cold"),
    ("6 warm-cache-20", os.path.join(BENCH, "large-test/package.json"), "axios", 5, "warmc"),
    ("7 warm-noop-20", os.path.join(BENCH, "large-test/package.json"), "axios", 7, "warmp"),
]


def make_project(spec, cache, home):
    d = tempfile.mkdtemp(prefix="jhol-bench-")
    if isinstance(spec, str):
        shutil.copy(spec, os.path.join(d, "package.json"))
    else:
        deps = ", ".join(f'"{n}": "{v}"' for n, v in spec)
        with open(os.path.join(d, "package.json"), "w") as fh:
            fh.write('{"name":"bench","version":"1.0.0","dependencies":{%s}}\n' % deps)
    env = dict(os.environ, HOME=home, JHOL_CACHE_DIR=cache,
               JHOL_NO_SPLASH="1", JHOL_NO_DAEMON="1")
    return d, env


def run_once(spec, expect, cache, home, cwd=None):
    if cwd is None:
        cwd, env = make_project(spec, cache, home)
    else:
        env = dict(os.environ, HOME=home, JHOL_CACHE_DIR=cache,
                   JHOL_NO_SPLASH="1", JHOL_NO_DAEMON="1")
    t0 = time.perf_counter()
    r = subprocess.run([J, "install", "--no-scripts"], cwd=cwd, env=env,
                       capture_output=True, text=True)
    if env.get("JHOL_HTTP_TIMING"):
        packument_sizes = []
        for line in r.stderr.splitlines():
            if "[HTTP-T]" in line and "/next " in line:
                print(line, file=sys.stderr)
            if "[HTTP-STREAM-T]" in line:
                print(line, file=sys.stderr)
            if "[PACKUMENT-VALIDATE]" in line:
                fields = dict(part.split("=", 1) for part in line.split() if "=" in part)
                try:
                    packument_sizes.append((fields["package"], int(fields["bytes"])))
                except (KeyError, ValueError):
                    pass
            if "[PACKUMENT-" in line and "package=next " in line:
                print(line, file=sys.stderr)
        if packument_sizes:
            sizes = sorted(size for _, size in packument_sizes)
            largest = sorted(packument_sizes, key=lambda row: row[1], reverse=True)[:5]
            print(
                "[PACKUMENT-SIZES] responses={} total_bytes={} median_bytes={} p95_bytes={} max_bytes={} largest={}".format(
                    len(sizes),
                    sum(sizes),
                    int(statistics.median(sizes)),
                    sizes[(95 * len(sizes) + 99) // 100 - 1],
                    sizes[-1],
                    ",".join(f"{name}:{size}" for name, size in largest),
                ),
                file=sys.stderr,
            )
    if env.get("JHOL_PROFILE_INSTALL"):
        for line in r.stderr.splitlines():
            if "[profile]" in line or "[jhol-profile]" in line:
                print(line, file=sys.stderr)
    dt = (time.perf_counter() - t0) * 1000
    nm = os.path.join(cwd, "node_modules")
    ok = (r.returncode == 0
          and os.path.exists(os.path.join(nm, expect))
          and os.path.isdir(nm)
          and len(os.listdir(nm)) > 0)
    loads = subprocess.run(["node", "-e", f"require('{expect}')"], cwd=cwd, env=env,
                           capture_output=True).returncode == 0
    return dt, (ok and loads), cwd, env


def bench(name, spec, expect, runs, mode):
    if mode == "cold":
        times = []
        for _ in range(runs):
            cache, home = tempfile.mkdtemp(), tempfile.mkdtemp()
            cwd = None
            try:
                dt, ok, cwd, _ = run_once(spec, expect, cache, home)
                if not ok:
                    raise SystemExit(f"{name}: a run produced no usable tree")
                times.append(dt)
            finally:
                if cwd:
                    shutil.rmtree(cwd, ignore_errors=True)
                shutil.rmtree(cache, ignore_errors=True)
                shutil.rmtree(home, ignore_errors=True)
    elif mode == "warmc":
        cache, home = tempfile.mkdtemp(), tempfile.mkdtemp()
        cwd = None
        try:
            _, ok, cwd, _ = run_once(spec, expect, cache, home)
            if not ok:
                raise SystemExit(f"{name}: prewarm produced no usable tree")
            shutil.rmtree(cwd, ignore_errors=True)
            cwd = None
            times = []
            for _ in range(runs):
                dt, ok, cwd, _ = run_once(spec, expect, cache, home)
                if not ok:
                    raise SystemExit(f"{name}: a run produced no usable tree")
                times.append(dt)
                shutil.rmtree(cwd, ignore_errors=True)
                cwd = None
        finally:
            if cwd:
                shutil.rmtree(cwd, ignore_errors=True)
            shutil.rmtree(cache, ignore_errors=True)
            shutil.rmtree(home, ignore_errors=True)
    else:  # warmp — one project, re-installed unchanged
        cache, home = tempfile.mkdtemp(), tempfile.mkdtemp()
        cwd, env = make_project(spec, cache, home)
        try:
            _, ok, _, _ = run_once(spec, expect, cache, home, cwd=cwd)
            if not ok:
                raise SystemExit(f"{name}: prewarm produced no usable tree")
            times = []
            for _ in range(runs):
                dt, ok, _, _ = run_once(spec, expect, cache, home, cwd=cwd)
                if not ok:
                    raise SystemExit(f"{name}: a run produced no usable tree")
                times.append(dt)
        finally:
            shutil.rmtree(cwd, ignore_errors=True)
            shutil.rmtree(cache, ignore_errors=True)
            shutil.rmtree(home, ignore_errors=True)
    return {
        "median_ms": round(statistics.median(times), 1),
        "min_ms": round(min(times), 1),
        "max_ms": round(max(times), 1),
        "samples_ms": [round(t, 1) for t in times],
        "runs": runs,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json")
    ap.add_argument("--only", help="substring filter on scale name")
    args = ap.parse_args()

    print(f"  binary: {J}")
    print(f"  host:   {os.uname().machine} / {os.uname().sysname} / {os.cpu_count()} cores")
    print()
    print(f"  {'scale':16} {'median':>9} {'min':>8} {'max':>8}   runs")
    out = {}
    for name, spec, expect, runs, mode in SCALES:
        if args.only and args.only not in name:
            continue
        res = bench(name, spec, expect, runs, mode)
        out[name] = res
        print(f"  {name:16} {res['median_ms']:8.1f}ms {res['min_ms']:7.1f} {res['max_ms']:7.1f}   {runs}")
    if args.json:
        with open(args.json, "w") as fh:
            json.dump({"binary": J, "host": os.uname().machine, "scales": out}, fh, indent=2)
        print(f"\n  written: {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
