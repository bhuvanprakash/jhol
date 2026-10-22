#!/usr/bin/env python3
"""Run a command as a child, killing it if system memory use crosses a limit.

This laptop (16 GB total, ~5 GB free, heavy swap) cannot survive an unbounded
`cargo build --release`: the default release profile is thin LTO with
`codegen-units = 1`, which peaks far above its headroom. Every long command is
run through this guard instead, so the machine stays usable. Exit status 75
means the guard stopped the command.
"""
import argparse, os, re, signal, subprocess, sys, time


def used_percent():
    try:
        out = subprocess.run(["memory_pressure"], capture_output=True, text=True, timeout=10).stdout
    except Exception:
        return None
    m = re.search(r"free percentage:\s*(\d+)", out)
    return None if m is None else 100 - int(m.group(1))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--threshold", type=float, default=95.0)
    ap.add_argument("--print-only", action="store_true")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()

    pct = used_percent()
    if a.print_only:
        print(f"  memory used: {pct if pct is not None else '?'}%  (limit {a.threshold}%)")
        return 0

    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    if not cmd:
        print("usage: mem_guard.py [--threshold N] [--print-only] -- CMD [ARGS...]", file=sys.stderr)
        return 2

    proc = subprocess.Popen(cmd, start_new_session=True)
    peak = pct or 0
    try:
        while True:
            if proc.poll() is not None:
                print(f"  mem_guard: finished (exit {proc.returncode}) - peak memory used {peak:.0f}%")
                return proc.returncode
            cur = used_percent()
            if cur is not None:
                peak = max(peak, cur)
                if cur >= a.threshold:
                    print(f"  mem_guard: STOP - memory used {cur}% >= {a.threshold}%; killing", flush=True)
                    os.killpg(proc.pid, signal.SIGKILL); proc.wait(); return 75
                print(f"  mem_guard: memory used {cur}% (peak {peak:.0f}%)", flush=True)
            time.sleep(2.0)
    except KeyboardInterrupt:
        os.killpg(proc.pid, signal.SIGKILL); proc.wait(); return 130


if __name__ == "__main__":
    sys.exit(main())
