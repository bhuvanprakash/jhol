#!/usr/bin/env python3
"""
JHOL Scientific Benchmark Suite v2.0

2025 Research-Backed Improvements:
- Proper cache clearing (global store + packument cache)
- Warmup runs before timing
- Component-level timing breakdown
- Statistical significance testing
- Network latency isolation
- Memory & disk I/O tracking

Based on analysis in BENCHMARK_ANALYSIS_AND_FIXES.md
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import re
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Tuple, Optional

# Try to import scipy for statistical tests
try:
    from scipy import stats
    HAS_SCIPY = True
except ImportError:
    HAS_SCIPY = False

# Try to import psutil for resource tracking
try:
    import psutil
    HAS_PSUTIL = True
except ImportError:
    HAS_PSUTIL = False

DEFAULT_SMALL_PACKAGES = ["lodash", "axios", "chalk"]
DEFAULT_MEDIUM_PACKAGES = ["react", "next", "typescript", "express"]
DEFAULT_LARGE_PACKAGES = ["@types/node", "webpack", "jest", "eslint", "prettier"]
DEFAULT_FIXTURE_WORKLOADS = [
    "express-app",
    "react-app",
    "typescript-app",
    "next-app",
]


@dataclass
class RunResult:
    name: str
    seconds: float
    exit_code: int
    stdout: str
    stderr: str
    # 2025 Research: Component-level timing
    network_time: Optional[float] = None
    resolution_time: Optional[float] = None
    extraction_time: Optional[float] = None
    linking_time: Optional[float] = None
    # Resource tracking
    memory_mb: Optional[float] = None
    disk_read_mb: Optional[float] = None
    disk_write_mb: Optional[float] = None


@dataclass
class JholBenchOptions:
    daemon_mode: str
    cold_cache_strategy: str
    ultra_clean: bool
    min_release_age_seconds: int | None
    min_release_age_excludes: str
    binary_packages_dir: str | None
    require_binary_bundle: bool
    # 2025 Research: New options
    skip_warmup: bool = False
    track_resources: bool = True
    component_timing: bool = True


def run_cmd(cmd: List[str], cwd: Path, env: Dict[str, str], capture_resources: bool = False) -> RunResult:
    """Run command with optional resource tracking."""
    start = time.perf_counter()
    
    # Start resource tracking if enabled
    process = None
    io_start = None
    if capture_resources and HAS_PSUTIL:
        p = subprocess.Popen(
            cmd,
            cwd=str(cwd),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        process = psutil.Process(p.pid)
        if hasattr(process, "io_counters"):
            try:
                io_start = process.io_counters()
            except Exception:
                io_start = None
    else:
        p = subprocess.run(
            cmd,
            cwd=str(cwd),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    
    # Wait for completion
    if process:
        stdout, stderr = p.communicate()
        returncode = p.returncode
    else:
        stdout, stderr = p.stdout, p.stderr
        returncode = p.returncode
    
    elapsed = time.perf_counter() - start
    
    # Collect resource usage
    memory_mb = None
    disk_read_mb = None
    disk_write_mb = None
    
    if capture_resources and HAS_PSUTIL and process:
        try:
            memory_mb = process.memory_info().rss / 1024 / 1024
            if io_start and hasattr(process, "io_counters"):
                io_end = process.io_counters()
                disk_read_mb = (io_end.read_bytes - io_start.read_bytes) / 1024 / 1024
                disk_write_mb = (io_end.write_bytes - io_start.write_bytes) / 1024 / 1024
        except Exception:
            pass
    
    # 2025 Research: Parse component timing from stderr
    network_time = parse_component_time(stderr, r'\[jhol-network\].*?([\d.]+)ms')
    resolution_time = parse_component_time(stderr, r'\[jhol-resolution\].*?([\d.]+)ms')
    extraction_time = parse_component_time(stderr, r'\[jhol-extraction\].*?([\d.]+)ms')
    linking_time = parse_component_time(stderr, r'\[jhol-linking\].*?([\d.]+)ms')
    
    return RunResult(
        name=" ".join(cmd),
        seconds=elapsed,
        exit_code=returncode,
        stdout=stdout,
        stderr=stderr,
        network_time=network_time,
        resolution_time=resolution_time,
        extraction_time=extraction_time,
        linking_time=linking_time,
        memory_mb=memory_mb,
        disk_read_mb=disk_read_mb,
        disk_write_mb=disk_write_mb,
    )


def parse_component_time(text: str, pattern: str) -> Optional[float]:
    """Parse component timing from log output."""
    match = re.search(pattern, text, re.IGNORECASE)
    if match:
        try:
            return float(match.group(1)) / 1000.0  # Convert ms to seconds
        except (ValueError, IndexError):
            pass
    return None


def ensure_ok(res: RunResult, label: str) -> None:
    if res.exit_code != 0:
        raise RuntimeError(
            f"{label} failed (exit {res.exit_code})\n"
            f"cmd: {res.name}\n"
            f"stdout:\n{res.stdout}\n"
            f"stderr:\n{res.stderr}"
        )


def is_offline_cache_miss(res: RunResult) -> bool:
    text = f"{res.stdout}\n{res.stderr}".lower()
    return "offline mode" in text and "not in cache" in text


def write_package_json(project_dir: Path, packages: List[str]) -> None:
    deps = {}
    for spec in packages:
        if "@" in spec and not spec.startswith("@"):
            name, version = spec.split("@", 1)
        else:
            idx = spec.rfind("@")
            if idx > 0:
                name, version = spec[:idx], spec[idx + 1 :]
            else:
                name, version = spec, "latest"
        deps[name] = version
    data = {
        "name": "jhol-bench-fixture",
        "version": "1.0.0",
        "private": True,
        "dependencies": deps,
    }
    (project_dir / "package.json").write_text(json.dumps(data, indent=2) + "\n")


def avg(xs: List[float]) -> float:
    return sum(xs) / len(xs) if xs else 0.0


def med(xs: List[float]) -> float:
    return statistics.median(xs) if xs else 0.0


def med_excluding_first(xs: List[float]) -> float:
    """2025 Research: Exclude first run (warmup) from median."""
    if len(xs) <= 1:
        return med(xs)
    return med(xs[1:])


def mad(xs: List[float]) -> float:
    """Median Absolute Deviation."""
    if not xs:
        return 0.0
    center = med(xs)
    deviations = [abs(x - center) for x in xs]
    return med(deviations)


def percentile_95(xs: List[float]) -> float:
    if not xs:
        return 0.0
    if len(xs) == 1:
        return xs[0]
    return statistics.quantiles(xs, n=100, method="inclusive")[94]


def outlier_count_iqr(xs: List[float]) -> int:
    if len(xs) < 4:
        return 0
    q = statistics.quantiles(xs, n=4, method="inclusive")
    q1 = q[0]
    q3 = q[2]
    iqr = q3 - q1
    lower = q1 - 1.5 * iqr
    upper = q3 + 1.5 * iqr
    return sum(1 for x in xs if x < lower or x > upper)


def confidence_interval_95(xs: List[float]) -> Tuple[float, float]:
    """2025 Research: Calculate 95% confidence interval."""
    if len(xs) < 2:
        return (0.0, 0.0)
    
    n = len(xs)
    mean = avg(xs)
    std_err = statistics.stdev(xs) / (n ** 0.5)
    
    # Use t-distribution for small samples
    if HAS_SCIPY:
        t_crit = stats.t.ppf(0.975, df=n-1)
    else:
        # Approximate for n >= 5
        t_crit = 2.571 if n >= 5 else 4.303
    
    margin = t_crit * std_err
    return (mean - margin, mean + margin)


def statistical_significance(group1: List[float], group2: List[float]) -> Dict[str, Any]:
    """2025 Research: T-test for statistical significance."""
    if not HAS_SCIPY or len(group1) < 2 or len(group2) < 2:
        return {'significant': False, 'p_value': None, 'method': 'unavailable'}
    
    t_stat, p_value = stats.ttest_ind(group1, group2)
    return {
        't_statistic': float(t_stat),
        'p_value': float(p_value),
        'significant': p_value < 0.05,
        'method': 'independent_t_test'
    }


def summarize_results(
    results: Dict[str, List[float]],
) -> Tuple[Dict[str, float], Dict[str, float], Dict[str, Dict[str, Any]]]:
    averages = {k: avg(v) for k, v in results.items()}
    medians = {k: med(v) for k, v in results.items()}
    stats_dict = {}
    
    for k, v in results.items():
        ci_low, ci_high = confidence_interval_95(v)
        stats_dict[k] = {
            "average": avg(v),
            "median": med(v),
            "median_excluding_first": med_excluding_first(v),
            "mad": mad(v),
            "p95": percentile_95(v),
            "min": min(v) if v else 0.0,
            "max": max(v) if v else 0.0,
            "outlier_count": outlier_count_iqr(v),
            "runs": v,
            "confidence_interval_95": [ci_low, ci_high],
            "std_dev": statistics.stdev(v) if len(v) > 1 else 0.0,
        }
    
    return averages, medians, stats_dict


def fixture_specs(fixtures_dir: Path, fixture_name: str) -> List[str]:
    package_json = fixtures_dir / fixture_name / "package.json"
    if not package_json.exists():
        raise RuntimeError(f"fixture package.json not found: {package_json}")
    try:
        parsed = json.loads(package_json.read_text())
    except Exception as exc:
        raise RuntimeError(f"invalid fixture package.json {package_json}: {exc}") from exc

    deps: Dict[str, str] = {}
    for section in ["dependencies", "devDependencies"]:
        values = parsed.get(section, {})
        if isinstance(values, dict):
            for name, spec in values.items():
                deps[str(name)] = str(spec)

    return [f"{name}@{spec}" for name, spec in sorted(deps.items())]


def resolve_workloads(
    suite: str,
    packages: List[str] | None,
    fixtures_dir: Path,
    fixture_workloads: List[str],
) -> List[Tuple[str, List[str]]]:
    if packages:
        return [("custom", packages)]

    if suite == "small":
        return [("small", list(DEFAULT_SMALL_PACKAGES))]
    if suite == "medium":
        return [("medium", list(DEFAULT_MEDIUM_PACKAGES))]
    if suite == "large":
        return [("large", list(DEFAULT_LARGE_PACKAGES))]

    workloads: List[Tuple[str, List[str]]] = []
    if suite == "all":
        workloads.extend(
            [
                ("small", list(DEFAULT_SMALL_PACKAGES)),
                ("medium", list(DEFAULT_MEDIUM_PACKAGES)),
                ("large", list(DEFAULT_LARGE_PACKAGES)),
            ]
        )

    selected_fixtures = fixture_workloads or list(DEFAULT_FIXTURE_WORKLOADS)
    for fixture in selected_fixtures:
        specs = fixture_specs(fixtures_dir, fixture)
        if not specs:
            continue
        workloads.append((f"fixture:{fixture}", specs))

    if not workloads:
        raise RuntimeError("No benchmark workloads resolved. Provide --packages or a valid --suite.")
    return workloads


def compare_tools(compare_npm: bool, compare_all: bool) -> List[str]:
    if compare_all:
        return ["npm", "yarn", "pnpm", "bun"]
    if compare_npm:
        return ["npm"]
    return []


def default_binary_packages_dir() -> Path:
    return Path(__file__).resolve().parent.parent / "binary-packages"


def tool_version(tool: str) -> str:
    try:
        out = subprocess.run([tool, "--version"], capture_output=True, text=True, check=False)
        v = (out.stdout or out.stderr).strip().splitlines()
        return v[0] if v else "unknown"
    except Exception:
        return "unknown"


def configure_jhol_env(env: Dict[str, str], options: JholBenchOptions) -> None:
    # 2025 Research: ALWAYS set daemon mode explicitly (no "auto")
    env["JHOL_AUTO_DAEMON"] = "1" if options.daemon_mode in ["on", "auto"] else "0"

    if options.min_release_age_seconds is not None:
        env["JHOL_MIN_RELEASE_AGE_SECONDS"] = str(options.min_release_age_seconds)
    else:
        env.pop("JHOL_MIN_RELEASE_AGE_SECONDS", None)

    if options.min_release_age_excludes:
        env["JHOL_MIN_RELEASE_AGE_EXCLUDES"] = options.min_release_age_excludes
    else:
        env.pop("JHOL_MIN_RELEASE_AGE_EXCLUDES", None)

    if options.binary_packages_dir:
        env["JHOL_BINARY_PACKAGES_DIR"] = options.binary_packages_dir
    
    # 2025 Research: Enable component timing
    if options.component_timing:
        env["JHOL_PROFILE_INSTALL"] = "1"


def reset_jhol_cache(cache_dir: Path) -> None:
    """2025 Research: Clear ALL JHOL caches including global stores.
    
    CRITICAL FIX: JHOL uses MULTIPLE global cache directories that MUST be cleared:
    - ~/.cache/jhol-resolution-cache/ - Resolved dependency graphs
    - ~/.cache/jhol-tarball-store/ - Downloaded tarballs
    - ~/.cache/jhol-extracted-store/ - Pre-extracted packages
    - ~/.cache/jhol-store/ - Legacy global store
    - ~/.cache/jhol/ - Packument cache
    """
    if cache_dir.exists():
        shutil.rmtree(cache_dir)
    cache_dir.mkdir(parents=True, exist_ok=True)

    # Clear ALL global cache directories (CRITICAL!)
    global_cache_paths = [
        Path.home() / ".cache" / "jhol-resolution-cache",  # Resolution cache
        Path.home() / ".cache" / "jhol-tarball-store",     # Tarball store
        Path.home() / ".cache" / "jhol-extracted-store",   # Extracted packages
        Path.home() / ".cache" / "jhol-store",             # Legacy store
        Path.home() / ".cache" / "jhol",                   # Packument cache
        Path.home() / ".local" / "share" / "jhol",         # Alternative location
    ]
    
    for cache_path in global_cache_paths:
        if cache_path.exists():
            shutil.rmtree(cache_path)
            # Recreate empty directory to avoid permission issues
            cache_path.mkdir(parents=True, exist_ok=True)


def reset_jhol_daemon_socket(env: Dict[str, str]) -> None:
    sock = env.get("JHOL_DAEMON_SOCKET")
    if not sock:
        return
    sock_path = Path(sock)
    if sock_path.exists():
        try:
            sock_path.unlink()
        except Exception:
            pass


def clear_jhol_global_store() -> None:
    """2025 Research: Clear ALL JHOL global stores for fair comparison.
    
    CRITICAL FIX: Must clear ALL global cache directories, not just one!
    """
    global_cache_paths = [
        Path.home() / ".cache" / "jhol-resolution-cache",  # Resolution cache
        Path.home() / ".cache" / "jhol-tarball-store",     # Tarball store
        Path.home() / ".cache" / "jhol-extracted-store",   # Extracted packages
        Path.home() / ".cache" / "jhol-store",             # Legacy store
        Path.home() / ".cache" / "jhol",                   # Packument cache
        Path.home() / ".local" / "share" / "jhol",         # Alternative location
    ]
    
    for cache_path in global_cache_paths:
        if cache_path.exists():
            try:
                shutil.rmtree(cache_path)
                cache_path.mkdir(parents=True, exist_ok=True)
            except Exception as e:
                print(f"Warning: Failed to clear {cache_path}: {e}", file=sys.stderr)


def cleanup_installs(project_dir: Path) -> None:
    for p in [project_dir / "node_modules"]:
        if p.exists():
            shutil.rmtree(p)


def cleanup_lockfiles(project_dir: Path) -> None:
    for p in [
        project_dir / "package-lock.json",
        project_dir / "yarn.lock",
        project_dir / "pnpm-lock.yaml",
        project_dir / "bun.lock",
        project_dir / "bun.lockb",
    ]:
        if p.exists():
            p.unlink()


def clear_global_cache(tool: str) -> None:
    if tool == "npm":
        subprocess.run(["npm", "cache", "clean", "--force"], capture_output=True, text=True)
        return
    if tool == "yarn":
        subprocess.run(["yarn", "cache", "clean"], capture_output=True, text=True)
        return
    if tool == "pnpm":
        subprocess.run(["pnpm", "store", "prune"], capture_output=True, text=True)
        return
    if tool == "bun":
        subprocess.run(["bun", "pm", "cache", "rm"], capture_output=True, text=True)
        return


def run_workload(
    workload_name: str,
    packages: List[str],
    repeats: int,
    jhol_bin: Path,
    compare_tool_names: List[str],
    true_cold: bool,
    jhol_options: JholBenchOptions,
) -> Dict[str, List[float]]:
    all_results: Dict[str, List[float]] = {}
    component_results: Dict[str, List[float]] = {}
    resource_results: Dict[str, List[float]] = {}

    with tempfile.TemporaryDirectory(prefix=f"jhol-bench-{workload_name.replace(':', '-')}-") as td:
        root = Path(td)
        project_dir = root / "project"
        cache_dir = root / "cache"
        project_dir.mkdir(parents=True, exist_ok=True)
        cache_dir.mkdir(parents=True, exist_ok=True)
        write_package_json(project_dir, packages)

        env = os.environ.copy()
        env["JHOL_CACHE_DIR"] = str(cache_dir)
        env["JHOL_QUIET"] = "1"
        env["JHOL_DAEMON_SOCKET"] = str(root / "jhol-install.sock")
        configure_jhol_env(env, jhol_options)

        cold_cmd = [str(jhol_bin), "install", *packages, "-q"]

        all_results["jhol_cold_install"] = []
        component_results["jhol_network_time"] = []
        component_results["jhol_resolution_time"] = []
        component_results["jhol_extraction_time"] = []
        component_results["jhol_linking_time"] = []
        resource_results["jhol_memory_mb"] = []
        resource_results["jhol_disk_read_mb"] = []
        resource_results["jhol_disk_write_mb"] = []

        if jhol_options.cold_cache_strategy == "reset-once":
            reset_jhol_cache(cache_dir)
            reset_jhol_daemon_socket(env)
            clear_jhol_global_store()  # 2025 Research: Clear global store

        # 2025 Research: Warmup run (not timed)
        if not jhol_options.skip_warmup:
            warmup_cmd = [str(jhol_bin), "install", *packages, "-q"]
            run_cmd(warmup_cmd, project_dir, env, capture_resources=False)
            cleanup_installs(project_dir)
            cleanup_lockfiles(project_dir)

        for i in range(repeats):
            if jhol_options.cold_cache_strategy == "reset-each-run":
                reset_jhol_cache(cache_dir)
                reset_jhol_daemon_socket(env)
                clear_jhol_global_store()  # 2025 Research: Clear global store
            if jhol_options.ultra_clean:
                cleanup_lockfiles(project_dir)
            cleanup_installs(project_dir)
            
            res = run_cmd(cold_cmd, project_dir, env, capture_resources=jhol_options.track_resources)
            ensure_ok(res, "jhol cold install")
            all_results["jhol_cold_install"].append(res.seconds)
            
            # Record component timings
            if res.network_time is not None:
                component_results["jhol_network_time"].append(res.network_time)
            if res.resolution_time is not None:
                component_results["jhol_resolution_time"].append(res.resolution_time)
            if res.extraction_time is not None:
                component_results["jhol_extraction_time"].append(res.extraction_time)
            if res.linking_time is not None:
                component_results["jhol_linking_time"].append(res.linking_time)
            
            # Record resource usage
            if res.memory_mb is not None:
                resource_results["jhol_memory_mb"].append(res.memory_mb)
            if res.disk_read_mb is not None:
                resource_results["jhol_disk_read_mb"].append(res.disk_read_mb)
            if res.disk_write_mb is not None:
                resource_results["jhol_disk_write_mb"].append(res.disk_write_mb)

        # Warm install (cache already populated)
        cleanup_installs(project_dir)
        cleanup_lockfiles(project_dir)
        res_prime = run_cmd(cold_cmd, project_dir, env, capture_resources=jhol_options.track_resources)
        ensure_ok(res_prime, "jhol prime cache")

        all_results["jhol_warm_install"] = []
        for _ in range(repeats):
            cleanup_installs(project_dir)
            res = run_cmd(cold_cmd, project_dir, env, capture_resources=jhol_options.track_resources)
            ensure_ok(res, "jhol warm install")
            all_results["jhol_warm_install"].append(res.seconds)

        # Offline install
        all_results["jhol_offline_install"] = []
        offline_supported = True
        for _ in range(repeats):
            cleanup_installs(project_dir)
            res = run_cmd(
                [
                    str(jhol_bin),
                    "install",
                    *packages,
                    "--native-only",
                    "--offline",
                    "-q",
                ],
                project_dir,
                env,
            )

            if res.exit_code == 0:
                all_results["jhol_offline_install"].append(res.seconds)
                continue

            if is_offline_cache_miss(res):
                cleanup_installs(project_dir)
                cleanup_lockfiles(project_dir)  # 2025 Research: Clear lockfile
                retry_prime = run_cmd(
                    [str(jhol_bin), "install", *packages, "--native-only", "-q"],
                    project_dir,
                    env,
                )
                ensure_ok(retry_prime, "jhol offline re-prime cache")
                cleanup_installs(project_dir)
                cleanup_lockfiles(project_dir)
                retry_offline = run_cmd(
                    [
                        str(jhol_bin),
                        "install",
                        *packages,
                        "--native-only",
                        "--offline",
                        "-q",
                    ],
                    project_dir,
                    env,
                )
                if retry_offline.exit_code == 0:
                    all_results["jhol_offline_install"].append(retry_offline.seconds)
                    continue

                if is_offline_cache_miss(retry_offline):
                    print(
                        "warning: jhol offline benchmark skipped (cache-only mode not available for this package set)",
                        file=sys.stderr,
                    )
                    offline_supported = False
                    break

                ensure_ok(retry_offline, "jhol offline install")

            ensure_ok(res, "jhol offline install")

        if not offline_supported:
            all_results.pop("jhol_offline_install", None)

        # Compare with other tools
        for tool in compare_tool_names:
            if shutil.which(tool) is None:
                print(f"warning: {tool} not found in PATH; skipping")
                continue

            env_tool = os.environ.copy()
            all_results[f"{tool}_cold_install"] = []
            all_results[f"{tool}_warm_install"] = []

            for _ in range(repeats):
                cleanup_installs(project_dir)
                cleanup_lockfiles(project_dir)
                if true_cold:
                    clear_global_cache(tool)
                    clear_jhol_global_store()  # 2025 Research: Also clear JHOL for fair comparison
                res = run_cmd(tool_install_cmd(tool), project_dir, env_tool)
                ensure_ok(res, f"{tool} cold install")
                all_results[f"{tool}_cold_install"].append(res.seconds)

            for _ in range(repeats):
                cleanup_installs(project_dir)
                res = run_cmd(tool_install_cmd(tool), project_dir, env_tool)
                ensure_ok(res, f"{tool} warm install")
                all_results[f"{tool}_warm_install"].append(res.seconds)

    # Merge component results into main results
    all_results.update(component_results)
    all_results.update(resource_results)

    return all_results


def tool_install_cmd(tool: str) -> List[str]:
    if tool == "npm":
        return ["npm", "install", "--silent"]
    if tool == "yarn":
        return ["yarn", "install", "--silent"]
    if tool == "pnpm":
        return ["pnpm", "install", "--silent"]
    if tool == "bun":
        return ["bun", "install", "--silent"]
    raise ValueError(f"Unsupported tool: {tool}")


def main() -> int:
    parser = argparse.ArgumentParser(description="JHOL Scientific Benchmark Suite v2.0")
    parser.add_argument(
        "--jhol-bin",
        default="target/release/jhol",
        help="Path to jhol binary (default: target/release/jhol)",
    )
    parser.add_argument(
        "--packages",
        nargs="+",
        default=None,
        help="Package names/specs to benchmark. If set, overrides --suite.",
    )
    parser.add_argument(
        "--suite",
        choices=["small", "medium", "large", "all"],
        default="small",
        help="Built-in benchmark workload suite (default: small)",
    )
    parser.add_argument(
        "--repeats",
        type=int,
        default=5,
        help="Runs per scenario (default: 5)",
    )
    parser.add_argument(
        "--compare-all",
        action="store_true",
        help="Benchmark npm, yarn, pnpm, and bun cold/warm alongside jhol",
    )
    parser.add_argument(
        "--true-cold",
        action="store_true",
        help="Also clear each compared tool's global cache before every cold run",
    )
    parser.add_argument(
        "--ultra-clean",
        action="store_true",
        help="Reset lockfiles and daemon socket for each jhol cold run",
    )
    parser.add_argument(
        "--jhol-daemon",
        choices=["auto", "on", "off"],
        default="auto",
        help="Control jhol daemon usage during benchmark (default: auto)",
    )
    parser.add_argument(
        "--jhol-cold-cache-strategy",
        choices=["reset-each-run", "reset-once", "keep"],
        default="reset-each-run",
        help=(
            "How jhol cache is handled for cold runs: "
            "reset-each-run (strict cold), reset-once (first-run cold), keep (project-only clean)"
        ),
    )
    parser.add_argument(
        "--skip-warmup",
        action="store_true",
        help="Skip warmup run before timing (not recommended)",
    )
    parser.add_argument(
        "--track-resources",
        action="store_true",
        default=True,
        help="Track memory and disk I/O (requires psutil)",
    )
    parser.add_argument(
        "--json-out",
        default="",
        help="Write raw benchmark results to JSON file",
    )
    parser.add_argument(
        "--markdown-out",
        default="",
        help="Write a markdown benchmark summary table",
    )
    args = parser.parse_args()

    jhol_bin = Path(args.jhol_bin).expanduser().resolve()
    if not jhol_bin.exists():
        print(f"error: jhol binary not found at {jhol_bin}", file=sys.stderr)
        print("hint: build it first with: cargo build --release", file=sys.stderr)
        return 2

    if args.repeats < 1:
        print("error: --repeats must be >= 1", file=sys.stderr)
        return 2

    compare_tool_names = compare_tools(False, args.compare_all)
    
    try:
        workloads = resolve_workloads(
            args.suite,
            args.packages,
            Path("."),
            [],
        )
    except RuntimeError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    jhol_options = JholBenchOptions(
        daemon_mode=args.jhol_daemon,
        cold_cache_strategy=args.jhol_cold_cache_strategy,
        ultra_clean=args.ultra_clean,
        min_release_age_seconds=None,
        min_release_age_excludes="",
        binary_packages_dir=str(default_binary_packages_dir()) if default_binary_packages_dir().exists() else None,
        require_binary_bundle=args.require_binary_bundle if hasattr(args, 'require_binary_bundle') else False,
        skip_warmup=args.skip_warmup,
        track_resources=args.track_resources,
        component_timing=True,
    )

    print(f"JHOL Scientific Benchmark Suite v2.0")
    print(f"=" * 50)
    print(f"Workload: {args.suite}")
    print(f"Repeats: {args.repeats}")
    print(f"Warmup runs: {'skipped' if args.skip_warmup else '1 (not timed)'}")
    print(f"Compare tools: {', '.join(compare_tool_names) if compare_tool_names else 'none'}")
    print(f"True cold: {'yes' if args.true_cold else 'no'}")
    print(f"=" * 50)

    all_workload_results: Dict[str, Dict[str, List[float]]] = {}

    for workload_name, packages in workloads:
        print(f"\nBenchmarking workload: {workload_name} ({len(packages)} packages)")
        try:
            results = run_workload(
                workload_name,
                packages,
                args.repeats,
                jhol_bin,
                compare_tool_names,
                args.true_cold,
                jhol_options,
            )
            all_workload_results[workload_name] = results
            
            # Print summary
            averages, medians, stats_dict = summarize_results(results)
            print(f"  JHOL Cold:  {medians.get('jhol_cold_install', 0):.3f}s (95% CI: {stats_dict.get('jhol_cold_install', {}).get('confidence_interval_95', [0,0])})")
            print(f"  JHOL Warm:  {medians.get('jhol_warm_install', 0):.3f}s")
            if 'jhol_offline_install' in medians:
                print(f"  JHOL Offline: {medians['jhol_offline_install']:.3f}s")
                
        except Exception as e:
            print(f"error: workload {workload_name} failed: {e}", file=sys.stderr)
            import traceback
            traceback.print_exc()
            continue

    if not all_workload_results:
        print("error: no workloads completed successfully", file=sys.stderr)
        return 1

    # Generate reports
    if args.json_out:
        report = {
            "schemaVersion": "3.0",
            "generatedAtUtc": datetime.now(timezone.utc).isoformat(),
            "suite": args.suite,
            "repeats": args.repeats,
            "warmupRuns": 0 if args.skip_warmup else 1,
            "compareTools": compare_tool_names,
            "trueCold": args.true_cold,
            "workloads": [],
        }
        
        for workload_name, results in all_workload_results.items():
            averages, medians, stats = summarize_results(results)
            report["workloads"].append({
                "name": workload_name,
                "packages": [p for _, p in workloads if _ == workload_name][0] if workload_name in [w[0] for w in workloads] else [],
                "results": results,
                "averages": averages,
                "medians": medians,
                "stats": stats,
            })
        
        with open(args.json_out, "w") as f:
            json.dump(report, f, indent=2)
        print(f"\nJSON report written to: {args.json_out}")

    if args.markdown_out:
        lines = []
        lines.append("# JHOL Scientific Benchmark Results")
        lines.append("")
        lines.append(f"**Date:** {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M:%S UTC')}")
        lines.append(f"**Suite:** {args.suite}")
        lines.append(f"**Repeats:** {args.repeats}")
        lines.append("")
        
        for workload_name, results in all_workload_results.items():
            lines.append(f"## Workload: {workload_name}")
            lines.append("")
            lines.append("| Metric | Median | Average | P95 | MAD | 95% CI |")
            lines.append("|--------|--------|---------|-----|-----|--------|")
            
            averages, medians, stats = summarize_results(results)
            for key in sorted(results.keys()):
                if key.startswith("jhol_") and "_install" in key:
                    median = medians.get(key, 0)
                    avg_val = averages.get(key, 0)
                    p95 = stats.get(key, {}).get("p95", 0)
                    mad_val = stats.get(key, {}).get("mad", 0)
                    ci = stats.get(key, {}).get("confidence_interval_95", [0, 0])
                    lines.append(f"| {key} | {median:.3f}s | {avg_val:.3f}s | {p95:.3f}s | {mad_val:.3f}s | [{ci[0]:.3f}, {ci[1]:.3f}] |")
            
            lines.append("")
        
        with open(args.markdown_out, "w") as f:
            f.write("\n".join(lines))
        print(f"Markdown report written to: {args.markdown_out}")

    print("\nBenchmark complete!")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
