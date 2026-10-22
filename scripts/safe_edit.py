#!/usr/bin/env python3
"""Snapshot files, run an editing command, verify the result, auto-restore on damage.

Why this exists
---------------
A bulk emoji-removal pass used a malformed Unicode regex range (`\\u1F300` is a
*four*-digit escape, so the class silently became the ASCII range `'0'`-`U+1FAFF`) and
deleted nearly every letter, digit and punctuation mark from six Rust files in place.
It was not caught until the compiler hit an unterminated string, and recovery depended
on copies that happened to exist. That must not be able to happen again.

This wrapper makes the class impossible to ship:

    snapshot  ->  run your command  ->  validate  ->  (restore on any failure)

Validation is deliberately blunt, because the failure mode is blunt:
  * the file still exists, is not empty, and is still valid UTF-8;
  * its printable-ASCII ratio has not collapsed versus the original
    (this is exactly what a bad Unicode range destroys);
  * its line count is within tolerance of the original;
  * no NUL bytes appeared.

Nothing here uses regex ranges over Unicode: checks iterate characters and compare
code points, which is the rule that was broken.

Usage
-----
    scripts/safe_edit.py FILE... -- COMMAND [ARGS...]
    scripts/safe_edit.py --dir PATH -- COMMAND [ARGS...]     # every *.rs/*.toml/... under PATH
    scripts/safe_edit.py FILE... --cargo-check -- COMMAND

Examples
--------
    scripts/safe_edit.py src/main.rs -- python3 patch_main.py
    scripts/safe_edit.py --dir crates/jhol-core/src --cargo-check -- python3 sweep.py

Exit codes: 0 = command ran and files are sane; 1 = validation failed (files restored);
the command's own exit code is reported separately in the summary when it fails first.
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time

BACKUP_ROOT = os.path.expanduser(os.environ.get("JHOL_BACKUP_DIR", "~/.jhol-edit-backups"))
PRINTABLE_ASCII_MIN_RATIO = 0.85   # vs the original file's ratio
LINE_COUNT_TOLERANCE = 0.20        # 20% either way
DEFAULT_GLOBS = (".rs", ".toml", ".md", ".sh", ".py", ".json", ".yml", ".yaml")


def sha256_of(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def printable_ascii_ratio(text: str) -> float:
    """Share of characters that are printable ASCII. Iterates code points on purpose."""
    if not text:
        return 1.0
    n = 0
    for ch in text:
        cp = ord(ch)
        if 0x20 <= cp <= 0x7E:
            n += 1
    return n / len(text)


def alnum_ratio(text: str) -> float:
    """Share of characters that are letters or digits.

    This is the signal that actually catches a malformed Unicode range: such a range
    deletes letters and digits while leaving spaces, newlines and quotes, so the
    printable-ASCII ratio stays high (space counts as printable) but the alphanumeric
    content collapses. Measure by code point, never with a regex range.
    """
    if not text:
        return 1.0
    n = 0
    for ch in text:
        cp = ord(ch)
        if 0x30 <= cp <= 0x39 or 0x41 <= cp <= 0x5A or 0x61 <= cp <= 0x7A:
            n += 1
    return n / len(text)


def non_whitespace(text: str) -> int:
    return sum(1 for ch in text if not ch.isspace())


def line_count(text: str) -> int:
    return text.count("\n") + (0 if text.endswith("\n") else 1)


def collect_files(files, directory):
    out = []
    for f in files or []:
        if not os.path.isfile(f):
            print(f"  ! not a file, skipping: {f}", file=sys.stderr)
            continue
        out.append(os.path.abspath(f))
    if directory:
        for root, dirs, names in os.walk(directory):
            dirs[:] = [d for d in dirs if d not in ("target", ".git", "node_modules")]
            for name in names:
                if name.endswith(DEFAULT_GLOBS):
                    out.append(os.path.abspath(os.path.join(root, name)))
    # de-duplicate, keep order
    seen = set()
    uniq = []
    for p in out:
        if p not in seen:
            seen.add(p)
            uniq.append(p)
    return uniq


def main() -> int:
    # Split on the first standalone `--` by hand: argparse would let a greedy
    # positional (`files`, nargs="*") swallow the command.
    argv = sys.argv[1:]
    if "--" in argv:
        cut = argv.index("--")
        head, command = argv[:cut], argv[cut + 1:]
    else:
        # No separator: let argparse handle `--help`, and report the missing command
        # below rather than refusing to explain itself.
        head, command = argv, []

    ap = argparse.ArgumentParser(add_help=True, prog="safe_edit.py")
    ap.add_argument("files", nargs="*", help="files to protect while COMMAND runs")
    ap.add_argument("--dir", help="protect every source-like file under this directory")
    ap.add_argument("--cargo-check", action="store_true", help="run `cargo check` after the command")
    ap.add_argument("--keep", action="store_true", help="keep the snapshot even on success")
    ap.add_argument("--line-tolerance", type=float, default=None,
                    help="override the line-count tolerance (e.g. 0.6 for an intentional "
                         "large addition; default 0.20)")
    ap.add_argument("--no-line-check", action="store_true",
                    help="skip the line-count check (still validates content collapse)")
    args = ap.parse_args(head)
    args.command = command

    if not command:
        print("error: no command given (use `-- COMMAND [ARGS...]`)", file=sys.stderr)
        return 2

    targets = collect_files(args.files, args.dir)
    if not targets:
        print("error: nothing to protect (pass files or --dir)", file=sys.stderr)
        return 2

    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    backup_dir = os.path.join(BACKUP_ROOT, f"{stamp}-{os.getpid()}")
    os.makedirs(backup_dir, exist_ok=True)

    before = {}
    for path in targets:
        try:
            raw = open(path, "rb").read()
        except OSError as e:
            print(f"  ! cannot read {path}: {e}", file=sys.stderr)
            return 2
        rel = os.path.relpath(path, "/")
        dest = os.path.join(backup_dir, rel)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copy2(path, dest)
        text = raw.decode("utf8", "replace")
        before[path] = {
            "backup": dest,
            "sha256": hashlib.sha256(raw).hexdigest(),
            "bytes": len(raw),
            "lines": line_count(text),
            "ascii_ratio": printable_ascii_ratio(text),
            "alnum_ratio": alnum_ratio(text),
            "non_ws": non_whitespace(text),
            "has_nul": b"\x00" in raw,
        }

    manifest = os.path.join(backup_dir, "manifest.json")
    with open(manifest, "w") as fh:
        json.dump({"created": stamp, "command": command, "files": before}, fh, indent=2)

    print(f"  snapshot: {len(targets)} file(s) -> {backup_dir}")
    print(f"  running : {' '.join(command)}")

    started = time.time()
    proc = subprocess.run(command)
    elapsed = time.time() - started

    problems = []
    for path, meta in before.items():
        if not os.path.exists(path):
            problems.append(f"{path}: file disappeared")
            continue
        raw = open(path, "rb").read()
        text = raw.decode("utf8", "replace")
        if not raw:
            problems.append(f"{path}: file is now empty")
            continue
        if b"\x00" in raw and not meta["has_nul"]:
            problems.append(f"{path}: NUL bytes appeared")
        ratio = printable_ascii_ratio(text)
        if ratio + 1e-9 < meta["ascii_ratio"] * PRINTABLE_ASCII_MIN_RATIO:
            problems.append(
                f"{path}: printable-ASCII collapsed {meta['ascii_ratio']:.2f} -> {ratio:.2f}"
            )
        # The decisive check: a bad Unicode range strips letters/digits and leaves
        # whitespace and quotes behind, so only this signal drops.
        new_alnum = alnum_ratio(text)
        if meta["alnum_ratio"] > 0.05 and new_alnum < meta["alnum_ratio"] * 0.8:
            problems.append(
                f"{path}: alphanumeric content collapsed "
                f"{meta['alnum_ratio']:.2f} -> {new_alnum:.2f} (letters/digits deleted)"
            )
        new_non_ws = non_whitespace(text)
        if meta["non_ws"] > 40 and new_non_ws < meta["non_ws"] * 0.75:
            problems.append(
                f"{path}: non-whitespace characters {meta['non_ws']} -> {new_non_ws} "
                f"(content mass-deleted)"
            )
        if not args.no_line_check:
            tol = args.line_tolerance if args.line_tolerance is not None else LINE_COUNT_TOLERANCE
            lines = line_count(text)
            if meta["lines"] and abs(lines - meta["lines"]) > meta["lines"] * tol:
                problems.append(
                    f"{path}: line count {meta['lines']} -> {lines} (beyond {tol:.0%} tolerance; "
                    f"pass --line-tolerance or --no-line-check for an intentional large edit)"
                )

    if args.cargo_check and not problems:
        # Run from the workspace root (nearest ancestor with Cargo.toml), not from the
        # directory of the first edited file.
        root = os.getcwd()
        probe = os.path.dirname(list(before)[0])
        while probe and probe != "/":
            if os.path.exists(os.path.join(probe, "Cargo.toml")):
                root = probe
                break
            probe = os.path.dirname(probe)
        print(f"  verifying: cargo check --workspace (in {root})")
        cc = subprocess.run(["cargo", "check", "--workspace", "--quiet"],
                            cwd=root, capture_output=True, text=True)
        if cc.returncode != 0:
            tail = "\n".join(cc.stderr.strip().splitlines()[-8:])
            problems.append(f"cargo check failed:\n{tail}")

    if problems:
        print("\n  VALIDATION FAILED — restoring the snapshot", file=sys.stderr)
        for p in problems:
            print(f"    - {p}", file=sys.stderr)
        for path, meta in before.items():
            shutil.copy2(meta["backup"], path)
        print(f"  restored {len(before)} file(s); snapshot kept at {backup_dir}", file=sys.stderr)
        return 1

    changed = []
    for path, meta in before.items():
        raw = open(path, "rb").read()
        if hashlib.sha256(raw).hexdigest() != meta["sha256"]:
            changed.append(path)

    print(f"  command exit: {proc.returncode} in {elapsed:.1f}s")
    print(f"  validated   : {len(before)} file(s), {len(changed)} changed")
    for c in changed[:20]:
        print(f"    ~ {c}")
    if not args.keep and not changed:
        shutil.rmtree(backup_dir, ignore_errors=True)
        print("  snapshot removed (nothing changed)")
    else:
        print(f"  snapshot kept: {backup_dir}")
    return 0 if proc.returncode == 0 else proc.returncode


if __name__ == "__main__":
    sys.exit(main())
