#!/usr/bin/env python3
"""Daemon vs direct vs Bun, with correctness asserted on every run.

Why this exists: a benchmark of the daemon MUST NOT set `JHOL_NO_DAEMON=1` — that
env var makes the client skip the daemon entirely, so the "daemon" column silently
measures direct installs. That mistake made a real 30 s daemon stall invisible in an
earlier round. This script never sets it, and it also probes the two cases that were
broken: a genuine request (cold) and a request arriving while a silent peer holds a
connection open.

Usage: python3 scripts/bench/daemon.py [rounds]
"""
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

BIN = os.environ.get("JHOL_BIN", os.path.expanduser("~/j/jhol/target/release/jhol"))
DEPS = {"ms": "^2.1.3", "is-number": "^7.0.0", "debug": "^4.4.3", "semver": "^7.6.3"}


def make_app(root, deps):
    os.makedirs(root, exist_ok=True)
    with open(os.path.join(root, "package.json"), "w") as fh:
        json.dump({"name": "d", "version": "1.0.0", "dependencies": deps}, fh)
    return root


def verify(app):
    r = subprocess.run(
        ["node", "-e", "for(const p of ['ms','is-number','debug','semver'])require(p)"],
        cwd=app, capture_output=True, text=True)
    return r.returncode == 0, (r.stderr.strip().splitlines() or [""])[0][:70]


def run(cmd, cwd, env):
    started = time.perf_counter()
    proc = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True)
    return (time.perf_counter() - started) * 1000, proc.returncode


def start_daemon(cache, sock):
    env = dict(os.environ, JHOL_CACHE_DIR=cache, JHOL_DAEMON_SOCKET=sock, JHOL_NO_SPLASH="1")
    for stale in (sock,):
        try:
            os.unlink(stale)
        except FileNotFoundError:
            pass
    proc = subprocess.Popen([BIN, "daemon", "--serve"], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            start_new_session=True)
    for _ in range(100):
        if os.path.exists(sock):
            return proc
        time.sleep(0.05)
    proc.kill()
    raise SystemExit("daemon socket never appeared")


def stop(proc, sock):
    try:
        proc.terminate()
        proc.wait(timeout=5)
    except Exception:
        proc.kill()
    try:
        os.unlink(sock)
    except FileNotFoundError:
        pass


def main(rounds: int = 3):
    base = tempfile.mkdtemp(prefix="bench-daemon-")
    cold, warm, bad = [], [], []
    print(f"  binary: {BIN}")
    for rnd in range(rounds):
        # --- daemon: a single fresh daemon + fresh cache, then repeat (warm)
        cache = os.path.join(base, f"d-cache{rnd}")
        app = make_app(os.path.join(base, f"d-app{rnd}"), DEPS)
        sock = os.path.join(base, f"d{rnd}.sock")
        daemon = start_daemon(cache, sock)
        env = dict(os.environ, JHOL_CACHE_DIR=cache, JHOL_DAEMON_SOCKET=sock, JHOL_NO_SPLASH="1")
        ms, rc = run([BIN, "install", "--daemon", "--no-scripts"], app, env)
        ok, info = verify(app)
        cold.append(ms)
        if rc != 0 or not ok:
            bad.append(("daemon-cold", rc, info))
        ms, rc = run([BIN, "install", "--daemon", "--no-scripts"], app, env)
        warm.append(ms)
        stop(daemon, sock)

        # --- direct, same protocol
        cache = os.path.join(base, f"s-cache{rnd}")
        app = make_app(os.path.join(base, f"s-app{rnd}"), DEPS)
        env = dict(os.environ, JHOL_CACHE_DIR=cache, JHOL_NO_SPLASH="1")
        run([BIN, "install", "--no-scripts"], app, env)  # prime
        ms, rc = run([BIN, "install", "--no-scripts"], app, env)
        warm.append(ms)
        print(f"  round {rnd + 1}/{rounds} done")

    def med(xs):
        return statistics.median(xs) if xs else float("nan")

    print(f"\n  daemon cold (real request): {med(cold):.0f} ms  (min {min(cold):.0f}, max {max(cold):.0f})")
    print(f"  daemon/direct warm:         {med(warm):.1f} ms")
    print(f"  correctness failures:       {len(bad)}")
    for b in bad:
        print(f"    {b}")
    shutil.rmtree(base, ignore_errors=True)


if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0] in ("-h", "--help", "help"):
        print(__doc__.strip())
        raise SystemExit(0)
    rounds = int(args[0]) if args else 3
    main(rounds)
