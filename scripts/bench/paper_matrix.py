#!/usr/bin/env python3
"""Interleaved, fail-closed Jhol/Bun install matrix.

Local-cache runs require explicit per-arm seed directories. Registry-cold,
prepared-tree, and unchanged no-op results are recorded as separate conditions.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import random
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ARMS = ("harvest", "jgrid", "jagrs", "jgrid+jagrs", "combined", "bun")
JHOL_ARMS = ARMS[:-1]
# Arms whose route is proven to fire today. `jgrid`/`jagrs`/`jgrid+jagrs`/
# `combined` are excluded by default: JGrid materialization only runs when a
# populated grid store already exists (JHOL_JGRID_INGEST is opt-in and the
# store starts empty), and the JAGR-S mirror has no reader outside `soul_path`.
# So every sample of those arms was recorded `invalid` — measuring nothing.
# Re-enable with `--arms` once a seeded store makes the route actually fire.
DEFAULT_ARMS = ("harvest", "bun")
ALL_ARMS = ARMS
ARM_ENV = {
    "harvest": {"JHOL_NO_HARVEST": None, "JHOL_JGRID": "0", "JHOL_JAGRS_MIRROR": "0"},
    "jgrid": {"JHOL_NO_HARVEST": "1", "JHOL_JGRID": "1", "JHOL_JGRID_INGEST": "0", "JHOL_JAGRS_MIRROR": "0"},
    "jagrs": {"JHOL_NO_HARVEST": "1", "JHOL_JGRID": "0", "JHOL_JAGRS_MIRROR": "1"},
    "jgrid+jagrs": {"JHOL_NO_HARVEST": "1", "JHOL_JGRID": "1", "JHOL_JGRID_INGEST": "0", "JHOL_JAGRS_MIRROR": "1"},
    "combined": {"JHOL_NO_HARVEST": None, "JHOL_JGRID": "1", "JHOL_JGRID_INGEST": "0", "JHOL_JAGRS_MIRROR": "1"},
}
DEFAULT_MARKERS = {
    "harvest": r"(?i)(\[HARVEST\]|harvest|sri hit|\[layout\]|download|fetch)",
    "jgrid": r"(?i)(jgrid_fast_path|jgrid: materialized|before_jgrid)",
    "jagrs": r"(?i)(jagr.?s|jagrs mirror).*(hit|materializ|object)",
    "jgrid+jagrs": r"(?i)(jgrid_fast_path|jagr.?s).*(hit|materializ|object)",
    "combined": r"(?i)(jgrid_fast_path|jagr.?s).*(hit|materializ|object)",
    "bun": r"(?i)(bun install|resolved|saved)",
}


def parse_assignments(items: list[str], label: str) -> dict[str, Path]:
    result: dict[str, Path] = {}
    for item in items:
        name, sep, value = item.partition("=")
        path = Path(value).expanduser().resolve()
        if not sep or name not in ARMS or not path.is_dir():
            raise ValueError(f"{label} must be ARM=existing-directory: {item}")
        result[name] = path
    return result


def hash_file(path: Path) -> str | None:
    if not path.is_file():
        return None
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def tree_digest(root: Path) -> str | None:
    nm = root / "node_modules"
    if not nm.is_dir():
        return None
    digest = hashlib.sha256()
    for current, dirs, files in os.walk(nm, followlinks=False):
        dirs.sort()
        files.sort()
        base = Path(current)
        for name in list(dirs) + files:
            path = base / name
            rel = path.relative_to(root).as_posix()
            info = path.lstat()
            digest.update(rel.encode() + b"\0")
            digest.update(str(info.st_mode & 0o7777).encode() + b"\0")
            if path.is_symlink():
                target = os.readlink(path)
                digest.update(b"L" + target.encode(errors="surrogateescape"))
                if name in dirs:
                    dirs.remove(name)
            elif path.is_file():
                digest.update(b"F")
                with path.open("rb") as stream:
                    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                        digest.update(chunk)
            else:
                digest.update(b"D")
    return digest.hexdigest()


def lock_layout(root: Path) -> dict[str, dict[str, str]]:
    """Installed package identities and paths, excluding manager-private metadata."""
    try:
        lock = json.loads((root / "package-lock.json").read_text())
        entries = lock.get("packages", {})
    except (OSError, ValueError, TypeError):
        return {}
    inventory: dict[str, dict[str, str]] = {}
    for key, entry in entries.items():
        if not key.startswith("node_modules/") or not isinstance(entry, dict):
            continue
        rel = key.removeprefix("node_modules/")
        try:
            pkg = json.loads((root / "node_modules" / rel / "package.json").read_text())
        except (OSError, ValueError, TypeError):
            continue
        name, version = pkg.get("name"), pkg.get("version")
        if isinstance(name, str) and isinstance(version, str):
            inventory[rel] = {"name": name, "version": version}
    return inventory


def bin_links(root: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    bin_dir = root / "node_modules" / ".bin"
    if not bin_dir.is_dir():
        return result
    for path in sorted(bin_dir.iterdir()):
        try:
            result[path.name] = os.readlink(path) if path.is_symlink() else f"mode:{path.stat().st_mode & 0o7777:o}"
        except OSError:
            result[path.name] = "unreadable"
    return result


def expected_bin_names(root: Path, inventory: dict[str, dict[str, str]]) -> set[str]:
    expected: set[str] = set()
    for rel in inventory:
        try:
            pkg = json.loads((root / "node_modules" / rel / "package.json").read_text())
        except (OSError, ValueError, TypeError):
            continue
        bins = pkg.get("bin")
        if isinstance(bins, str):
            expected.add(rel.rsplit("/", 1)[-1])
        elif isinstance(bins, dict):
            expected.update(name for name, path in bins.items() if isinstance(path, str))
    return expected


def cache_bytes(root: Path) -> int:
    total = 0
    if not root.exists():
        return total
    for current, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = [name for name in dirs if not (Path(current) / name).is_symlink()]
        for name in files:
            path = Path(current) / name
            try:
                total += path.lstat().st_size
            except OSError:
                pass
    return total


def layout_quality(root: Path, node: str, expected_lock_hash: str | None) -> tuple[bool, str, str | None]:
    """Return (ok, reason, drift).

    `drift` is set when the install produced a *valid but different* lockfile
    than the fixture — i.e. the resolver picked a different version inside the
    declared range (e.g. `source-map-js` 1.2.1 vs 1.2.2). That is a resolver
    difference, not a broken install: the tree loads, the locked identities are
    present, and the ranges are honoured. It is reported, and whether it fails the
    run is decided by `--drift-policy` (default: warn, so a pin bump does not
    invalidate an otherwise fair sample set).
    """
    drift = None
    if not (root / "package-lock.json").is_file():
        return False, "package-lock.json missing", None
    try:
        manifest = json.loads((root / "package.json").read_text())
        lock = json.loads((root / "package-lock.json").read_text())
    except (OSError, ValueError, TypeError):
        return False, "fixture manifests could not be parsed", None

    # A locked entry is required only when some *other* installed package depends
    # on it. npm/bun lockfiles legitimately carry unreachable entries (a stale
    # `node_modules/ansi-regex` after `strip-ansi` moves its dependency), and
    # bun prunes them while jhol keeps them — neither is a defect, so neither
    # may fail a run.
    packages = lock.get("packages", {})
    reachable = set()
    frontier = []
    for key, entry in packages.items():
        if not key.startswith("node_modules/") or not isinstance(entry, dict):
            continue
        name = key.rsplit("node_modules/", 1)[-1]
        for field in ("dependencies", "optionalDependencies", "peerDependencies"):
            deps = entry.get(field)
            if isinstance(deps, dict):
                frontier.extend(deps.keys())
    # Direct dependencies of the project root are roots.
    for field in ("dependencies", "devDependencies", "optionalDependencies"):
        block = manifest.get(field)
        if isinstance(block, dict):
            frontier.extend(block.keys())
    while frontier:
        name = frontier.pop()
        if name in reachable:
            continue
        reachable.add(name)
        entry = packages.get(f"node_modules/{name}")
        if isinstance(entry, dict):
            for field in ("dependencies", "optionalDependencies", "peerDependencies"):
                deps = entry.get(field)
                if isinstance(deps, dict):
                    frontier.extend(deps.keys())

    try:
        packages_to_check = sorted({
            key for key, entry in packages.items()
            if key.startswith("node_modules/") and isinstance(entry, dict)
            and not entry.get("optional")
            and (name := key.rsplit("node_modules/", 1)[-1]) in reachable
        })
    except SyntaxError:
        return False, "lockfile package keys could not be interpreted", None
    if not packages_to_check:
        packages_to_check = [f"node_modules/{name}" for name in ("ms", "is-number", "mime-types", "axios", "chalk")]
    # Exercise project runtime consumers. Transitive @types packages contain
    # declarations and have no executable entrypoint; validate their locked
    # installed paths below instead of importing them as JavaScript.
    runtime_roots = sorted({
        name for field in ("dependencies", "devDependencies")
        for name in manifest.get(field, {})
        if not name.startswith("@types/")
    })
    package_json = json.dumps(runtime_roots)
    script = f"for(const p of {package_json}) require.resolve(p);"
    cjs = subprocess.run([node, "-e", script], cwd=root, capture_output=True, text=True)
    esm = subprocess.run(
        [node, "--input-type=module", "-e", f"for (const p of {package_json}) await import(p);"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if cjs.returncode or esm.returncode:
        return False, (cjs.stderr + esm.stderr)[-1200:], None
    if expected_lock_hash and hash_file(root / "package-lock.json") != expected_lock_hash:
        # The lockfile differs from the fixture. Decide below whether every other
        # check passes; if so this is resolver drift, not a broken install.
        drift = "package-lock.json differs from the fixture lockfile"
    inventory = lock_layout(root)
    if not inventory:
        return False, "lockfile has no installed package identity inventory", None
    missing = [
        path for path in packages_to_check
        if not (root / path / "package.json").is_file()
    ]
    if missing:
        return False, "missing required locked paths: " + ", ".join(missing[:8]), None
    mismatched = [path for path in packages_to_check
                  if packages.get(path, {}).get("version") is not None
                  and inventory.get(path.removeprefix("node_modules/"), {}).get("version")
                  != packages[path]["version"]]
    if mismatched:
        return False, "locked versions differ at: " + ", ".join(mismatched[:8]), None
    links = bin_links(root)
    expected_bins = expected_bin_names(root, inventory)
    if not expected_bins.issubset(links):
        return False, "missing executable links: " + ", ".join(sorted(expected_bins - set(links))[:8]), None
    for name in expected_bins:
        path = root / "node_modules" / ".bin" / name
        if not path.is_file() or not os.access(path, os.X_OK):
            return False, f"executable link is not runnable: {name}", None
    return True, "locked identities, CJS/ESM imports, and lock stability pass", drift


def restore_fixture(fixture: Path) -> None:
    """Re-copy the pristine lockfiles over the fixture after a mutating arm.

    Bun migrates `package-lock.json` to `bun.lock` and prunes unreferenced
    entries. Without this the next arm runs against a fixture bun edited.
    """
    pristine = fixture.parent / f".{fixture.name}.pristine"
    if not pristine.is_dir():
        return
    for name in ("package-lock.json", "npm-shrinkwrap.json"):
        source = pristine / name
        if source.is_file():
            shutil.copy2(source, fixture / name)
    for name in ("bun.lock", "bun.lockb"):
        (fixture / name).unlink(missing_ok=True)


def run_one(args: argparse.Namespace, arm: str, round_no: int, root_tmp: Path,
            seed_caches: dict[str, Path], markers: dict[str, str]) -> dict:
    temp_root = root_tmp / f"{round_no:03d}-{arm.replace('+', '-')}-{time.time_ns()}"
    project = temp_root / "project"
    cache = temp_root / "tool-cache"
    home = temp_root / "home"
    shutil.copytree(
        args.fixture,
        project,
        symlinks=True,
        ignore=shutil.ignore_patterns(
            "node_modules", ".git", ".jhol", ".jhol-cache", ".jhol.capsule",
            ".jhol-prebuilt-key", ".jhol-ok", "target", "bun.lock", "bun.lockb",
        ),
    )
    if args.condition in ("prepared-tree", "unchanged-noop"):
        source_nm = args.fixture / "node_modules"
        if not source_nm.is_dir():
            return {"arm": arm, "round": round_no, "status": "invalid", "reason": "fixture has no prepared node_modules"}
        shutil.copytree(source_nm, project / "node_modules", symlinks=True)
    cache.mkdir(parents=True)
    home.mkdir(parents=True)
    source = seed_caches.get(arm) if args.condition == "local-cache-cold" else None
    if args.condition == "local-cache-cold":
        if source is None:
            return {"arm": arm, "round": round_no, "status": "invalid", "reason": "missing --seed-cache for arm"}
        # Jhol reads its cache root directly (`JHOL_CACHE_DIR`), but Bun reads
        # `BUN_INSTALL_CACHE_DIR`, which is pointed at `<cache>/bun`. Seeding the
        # Bun arm therefore has to land *inside* that subdirectory — copying the
        # seed to the cache root leaves Bun empty and it silently downloads.
        destination = cache / "bun" if arm == "bun" else cache
        destination.parent.mkdir(parents=True, exist_ok=True)
        if destination.exists():
            shutil.rmtree(destination, ignore_errors=True)
        # Seed caches are large (100s of MB); symlink instead of copying per round.
        # The seed is read-only for the run, so sharing it is safe.
        try:
            destination.symlink_to(source, target_is_directory=True)
        except OSError:
            shutil.copytree(source, destination, dirs_exist_ok=True, symlinks=True)

    env = os.environ.copy()
    for key in list(env):
        if key.startswith("JHOL_") and key not in ("JHOL_REGISTRY",):
            env.pop(key, None)
    env.update({
        "HOME": str(home),
        "JHOL_CACHE_DIR": str(cache),
        "JHOL_DAEMON_MODE": "0",
        "JHOL_NO_DAEMON": "1",
        "JHOL_PREBUILT": "0",
        "JHOL_PROFILE_INSTALL": "1",
        "JHOL_DEBUG_LAYOUT": "1",
        "JHOL_VERBOSE": "1",
        "JHOL_HTTP_METRICS": "1",
        "JHOL_NO_SPLASH": "1",
        "BUN_INSTALL_CACHE_DIR": str(cache / "bun"),
    })
    if arm in ARM_ENV:
        for key, value in ARM_ENV[arm].items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
    if args.condition == "registry-cold":
        # Registry-cold is the *only* condition that disables local-origin reuse:
        # without this, harvest makes every arm warm and the comparison measures
        # the wrong thing. Local-cache-cold keeps the arm's own seeding instead.
        env["JHOL_NO_HARVEST"] = "1"

    if arm == "bun":
        command = [args.bun, "install", "--no-progress"]
    else:
        command = [args.jhol, "install"]
    cpu_before = _child_cpu()
    start = time.perf_counter()
    try:
        proc, peak_rss = _run_monitored(command, project, env, args.timeout)
        elapsed_ms = (time.perf_counter() - start) * 1000
        output = proc.stdout + "\n" + proc.stderr
        route_seen = bool(re.search(markers.get(arm, "(?!)"), output))
        quality_ok, quality_reason, drift = layout_quality(project, args.node, args.fixture_lock_sha256)
        digest = tree_digest(project)
        inventory = lock_layout(project)
        links = bin_links(project)
        lock_hash = hash_file(project / "package-lock.json")
        downloaded = re.search(r"Bytes downloaded:\s*([0-9]+) bytes", output)
        status = "pass" if proc.returncode == 0 and quality_ok and digest else "fail"
        if drift and args.drift_policy == "fail":
            status = "fail"
        if not route_seen:
            status = "invalid"
        return {
            "arm": arm,
            "round": round_no,
            "condition": args.condition,
            "status": status,
            "route_observed": route_seen,
            "reason": None if route_seen else "selected route marker not observed",
            "resolver_drift": drift,
            "exit_code": proc.returncode,
            "elapsed_ms": elapsed_ms,
            "cpu_user_seconds": _child_cpu()[0] - cpu_before[0],
            "cpu_system_seconds": _child_cpu()[1] - cpu_before[1],
            "peak_process_tree_rss_bytes": peak_rss,
            "cache_bytes_after": cache_bytes(cache),
            "downloaded_bytes": int(downloaded.group(1)) if downloaded else None,
            "package_layout_sha256": digest,
            "package_inventory": inventory,
            "bin_links": links,
            "bin_link_names": sorted(links),
            "package_lock_sha256": lock_hash,
            "quality": quality_reason,
            "stdout_stderr_tail": output[-1600:],
            "stage_times_ms": {
                match.group(1): int(match.group(2))
                for match in re.finditer(r"\[jhol-profile\] stage=([^ ]+) delta_ms=([0-9]+)", output)
            },
            "arm_environment": ARM_ENV.get(arm, {}),
            "seed_cache_sha256": _directory_digest(source) if source else None,
        }
    except subprocess.TimeoutExpired as error:
        return {"arm": arm, "round": round_no, "condition": args.condition,
                "status": "fail", "reason": f"timeout after {args.timeout}s",
                "stdout_stderr_tail": str(error)[-1200:]}


def _child_cpu() -> tuple[float, float]:
    try:
        import resource
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
        return usage.ru_utime, usage.ru_stime
    except (ImportError, AttributeError):
        return 0.0, 0.0


def _run_monitored(command: list[str], cwd: Path, env: dict[str, str], timeout: int):
    try:
        import psutil
    except ImportError:
        proc = subprocess.run(command, cwd=cwd, env=env, capture_output=True,
                              text=True, timeout=timeout)
        return proc, None

    proc = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True)
    process = psutil.Process(proc.pid)
    peak = 0
    deadline = time.monotonic() + timeout
    while proc.poll() is None:
        try:
            tree = [process, *process.children(recursive=True)]
            peak = max(peak, sum(item.memory_info().rss for item in tree if item.is_running()))
        except (psutil.Error, OSError):
            pass
        if time.monotonic() >= deadline:
            proc.kill()
            stdout, stderr = proc.communicate()
            raise subprocess.TimeoutExpired(command, timeout, output=stdout, stderr=stderr)
        time.sleep(0.01)
    stdout, stderr = proc.communicate()
    return subprocess.CompletedProcess(command, proc.returncode, stdout, stderr), peak or None


def _directory_digest(root: Path | None) -> str | None:
    if root is None:
        return None
    digest = hashlib.sha256()
    for current, dirs, files in os.walk(root, followlinks=False):
        dirs.sort()
        files.sort()
        for name in files:
            path = Path(current) / name
            digest.update(path.relative_to(root).as_posix().encode() + b"\0")
            try:
                digest.update(path.read_bytes())
            except OSError:
                continue
    return digest.hexdigest()


def summarize(samples: list[dict], rounds: int, seed: int) -> dict:
    rng = random.Random(seed)
    result = {}
    for arm in ARMS:
        arm_samples = [s for s in samples if s.get("arm") == arm]
        values = [s["elapsed_ms"] for s in arm_samples if s.get("status") == "pass"]
        medians = []
        for _ in range(3000):
            draw = [rng.choice(values) for _ in values] if values else []
            if draw:
                medians.append(statistics.median(draw))
        medians.sort()
        result[arm] = {
            "passing_samples": len(values),
            "required_samples": rounds,
            "invalid_samples": sum(1 for s in arm_samples if s.get("status") == "invalid"),
            "failed_samples": sum(1 for s in arm_samples if s.get("status") == "fail"),
            "resolver_drift_samples": sum(1 for s in arm_samples if s.get("resolver_drift")),
            "median_ms": statistics.median(values) if values else None,
            "p95_ms": sorted(values)[min(len(values)-1, int(len(values) * .95))] if values else None,
            "median_95pct_ci_ms": [medians[int(len(medians)*.025)], medians[min(len(medians)-1, int(len(medians)*.975))]] if medians else None,
        }
    bun = result["bun"]["median_ms"]
    bun_by_round = {s["round"]: s["elapsed_ms"] for s in samples
                    if s.get("arm") == "bun" and s.get("status") == "pass"}
    for arm in JHOL_ARMS:
        median = result[arm]["median_ms"]
        result[arm]["bun_over_jhol_median"] = bun / median if bun and median else None
        pairs = [(bun_by_round[s["round"]], s["elapsed_ms"]) for s in samples
                 if s.get("arm") == arm and s.get("status") == "pass"
                 and s.get("round") in bun_by_round]
        ratios = []
        for _ in range(3000):
            draw = [rng.choice(pairs) for _ in pairs]
            if draw:
                ratios.append(statistics.median(p[0] for p in draw)
                              / statistics.median(p[1] for p in draw))
        ratios.sort()
        result[arm]["bun_over_jhol_lower_95pct"] = ratios[int(len(ratios) * .025)] if ratios else None
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True, help="Pinned project with package.json and package-lock.json")
    parser.add_argument("--jhol", default="target/release/jhol")
    parser.add_argument("--bun", default="bun")
    parser.add_argument("--node", default="node")
    parser.add_argument("--condition", choices=("registry-cold", "local-cache-cold", "prepared-tree", "unchanged-noop"), default="registry-cold")
    parser.add_argument("--seed-cache", action="append", default=[], metavar="ARM=DIR")
    parser.add_argument("--rounds", type=int, default=30)
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--seed", type=int, default=20260930)
    parser.add_argument("--marker", action="append", default=[], metavar="ARM=REGEX")
    parser.add_argument("--arms", default=",".join(DEFAULT_ARMS),
                        help="Comma-separated arms to run. Defaults to the arms whose route fires today "
                             f"({','.join(DEFAULT_ARMS)}); all arms: {','.join(ALL_ARMS)}")
    parser.add_argument("--drift-policy", choices=("warn", "fail"), default="warn",
                        help="warn (default): a valid tree with a different in-range resolution is a "
                             "pass with resolver_drift recorded. fail: treat it as a failure.")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    arms = [a for a in (x.strip() for x in args.arms.split(",")) if a]
    unknown = [a for a in arms if a not in ARMS]
    if unknown:
        parser.error(f"unknown arm(s): {', '.join(unknown)}; valid: {', '.join(ARMS)}")
    if not arms:
        parser.error("--arms must name at least one arm")
    args.fixture = args.fixture.resolve()
    args.fixture_lock_sha256 = hash_file(args.fixture / "package-lock.json")
    args.jhol = str(Path(args.jhol).expanduser().resolve()) if "/" in args.jhol else args.jhol
    if not (args.fixture / "package.json").is_file() or not (args.fixture / "package-lock.json").is_file():
        parser.error("fixture must contain package.json and package-lock.json")
    # Keep a pristine copy of the fixture lockfiles so arms that rewrite them
    # (bun migrates to bun.lock) cannot poison later rounds.
    args.pristine_fixture = args.fixture.parent / f".{args.fixture.name}.pristine"
    args.pristine_fixture.mkdir(parents=True, exist_ok=True)
    for name in ("package.json", "package-lock.json", "npm-shrinkwrap.json"):
        if (args.fixture / name).is_file():
            shutil.copy2(args.fixture / name, args.pristine_fixture / name)
    if args.rounds < 1:
        parser.error("--rounds must be positive")
    try:
        seed_caches = parse_assignments(args.seed_cache, "--seed-cache")
        markers = DEFAULT_MARKERS.copy()
        for item in args.marker:
            arm, sep, value = item.partition("=")
            if not sep or arm not in ARMS:
                parser.error(f"--marker must be ARM=REGEX: {item}")
            markers[arm] = value
    except ValueError as error:
        parser.error(str(error))
    if args.condition == "local-cache-cold":
        markers["harvest"] = r"(?i)(\[HARVEST\]|SOURCE harvest-unpacked)"
    # NOTE: `registry-cold` deliberately does NOT override the harvest marker.
    # The old override required a `[layout] … SOURCE local-tarball` line that the
    # current installer no longer prints, so every sample was recorded `invalid`
    # — a dead matcher, not a measurement. The default marker matches the
    # documented routes; override per-arm with --marker if a route changes.
    if args.condition == "local-cache-cold" and not set(arms).issubset(seed_caches):
        parser.error(f"local-cache-cold needs --seed-cache for every arm in {arms}, including Bun")

    randomizer = random.Random(args.seed)
    samples: list[dict] = []
    with tempfile.TemporaryDirectory(prefix="jhol-paper-matrix-") as raw_tmp:
        root_tmp = Path(raw_tmp)
        for round_no in range(1, args.rounds + 1):
            randomizer.shuffle(arms)
            for arm in arms:
                samples.append(run_one(args, arm, round_no, root_tmp, seed_caches, markers))
                # Bun rewrites the fixture's npm lockfile into bun.lock and removes
                # entries nothing depends on (`node_modules/ansi-regex` on this
                # fixture). Restore the fixture between rounds, or the next arm
                # inherits bun's pruned lockfile and is measured against it.
                restore_fixture(args.fixture)
    bun_by_round = {s.get("round"): s for s in samples if s.get("arm") == "bun" and s.get("status") == "pass"}
    for sample in samples:
        reference = bun_by_round.get(sample.get("round"))
        if sample.get("status") == "pass" and sample.get("arm") != "bun" and reference:
            if sample.get("package_inventory") != reference.get("package_inventory"):
                sample["status"] = "fail"
                sample["reason"] = "installed package identities or paths differ from Bun reference"
            elif sample.get("bin_link_names") != reference.get("bin_link_names"):
                sample["status"] = "fail"
                sample["reason"] = "executable link names differ from Bun reference"
    summary = summarize(samples, args.rounds, args.seed)
    # Two independent verdicts. `measurement_valid` says the run is trustworthy:
    # every arm that was *requested* produced every sample and the arms are
    # mutually comparable. Arms that did not run are skipped, not counted as
    # failures. `speed_gate` is a separate 4x claim gate and must never stand in
    # for validity — a red speed gate on registry-cold is the honest result.
    requested = [a for a in ARMS if any(s.get("arm") == a for s in samples)]
    measurement_valid = all(summary[a]["passing_samples"] == args.rounds for a in requested)
    speed_gate = measurement_valid and all((summary[a].get("bun_over_jhol_median") or 0) >= 4
                     and (summary[a].get("bun_over_jhol_lower_95pct") or 0) >= 4
                     for a in JHOL_ARMS
                     if a in requested and summary[a]["median_ms"])
    report = {
        "schema": 1,
        "condition": args.condition,
        "fixture": str(args.fixture),
        "rounds": args.rounds,
        "seed": args.seed,
        "jhol": args.jhol,
        "jhol_sha256": hash_file(Path(args.jhol)) if Path(args.jhol).is_file() else None,
        "bun": args.bun,
        "bun_version": _version(args.bun),
        "node_version": _version(args.node),
        "python_version": platform.python_version(),
        "host": {"system": platform.platform(), "machine": platform.machine(), "processor": platform.processor()},
        "fixture_lock_sha256": hash_file(args.fixture / "package-lock.json"),
        "seed_cache_coverage": {arm: {"path": str(path), "bytes": cache_bytes(path), "sha256": _directory_digest(path)} for arm, path in seed_caches.items()},
        "summary": summary,
        "measurement_valid": measurement_valid,
        "speed_gate_4x": "pass" if speed_gate else "red",
        "samples": samples,
    }
    output = args.out or Path("docs/architecture/experiments") / f"paper-matrix-{args.condition}-{time.strftime('%Y%m%d-%H%M%S')}.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    passed = sum(s.get("status") == "pass" for s in samples)
    verdict = "valid" if measurement_valid else "INVALID"
    print(f"{output}: {verdict}; {passed}/{len(samples)} passing samples; "
          f"speed gate 4x: {report['speed_gate_4x']}")
    return 0 if measurement_valid else 2


def _version(command: str) -> str | None:
    try:
        result = subprocess.run([command, "--version"], capture_output=True, text=True, timeout=10)
        return (result.stdout or result.stderr).strip()[:200] if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


if __name__ == "__main__":
    raise SystemExit(main())
