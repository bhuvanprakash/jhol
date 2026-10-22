#!/usr/bin/env python3
"""One-shot generator for merged benchmark/guardrail scripts. Run from repo root."""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
BENCH = SCRIPTS / "bench"

MAIN_RE = re.compile(r"^def main\(\)(?:\s*->\s*[^:]+)?\s*:\s*$", re.MULTILINE)


def strip_entrypoint(text: str) -> str:
    text = re.sub(r"^#!/usr/bin/env python3\n", "", text)
    text = re.sub(r"^from __future__ import .+\n", "", text, flags=re.MULTILINE)
    text = re.sub(
        r"\nif __name__ == [\"']__main__[\"']:\n(?:\s+raise SystemExit\(main\(\)\)|\s+main\(\))\s*\n?$",
        "\n",
        text,
        flags=re.MULTILINE,
    )
    return text.strip() + "\n"


def wrap_main(text: str, new_name: str, prog: str) -> str:
    m = MAIN_RE.search(text)
    if not m:
        raise ValueError(f"no def main() in {new_name}")
    prefix = text[: m.start()]
    suffix = text[m.end() :]
    header_fn = (
        f"def {new_name}(argv: list[str] | None = None) -> int:\n"
        f"    import sys\n"
        f"    if argv is not None:\n"
        f'        sys.argv = [sys.argv[0] if sys.argv else "{prog}", *argv]\n'
    )
    return prefix + header_fn + suffix


def wrap_script_body(text: str, new_name: str, prog: str) -> str:
    """Scripts with no def main(): wrap top-level statements in run_*."""
    indented = "\n".join(
        ("    " + line) if line.strip() else "" for line in text.splitlines()
    )
    return (
        f"def {new_name}(argv: list[str] | None = None) -> int:\n"
        f"    import sys\n"
        f"    if argv is not None:\n"
        f'        sys.argv = [sys.argv[0] if sys.argv else "{prog}", *argv]\n'
        f"{indented}\n"
        f"    return 0\n"
    )


def prepare_piece(text: str, new_name: str, prog: str) -> str:
    if MAIN_RE.search(text):
        return wrap_main(text, new_name, prog)
    return wrap_script_body(text, new_name, prog)


def merge_modules(
    out_path: Path,
    doc: str,
    pieces: list[tuple[str, Path, str]],
    prog_name: str,
) -> None:
    header = (
        "#!/usr/bin/env python3\n"
        f'"""{doc}"""\n'
        "from __future__ import annotations\n\n"
        "import sys\n\n"
    )
    body = ""
    mode_map: list[tuple[str, str]] = []
    for mode, src, run_name in pieces:
        if not src.is_file():
            raise FileNotFoundError(src)
        text = strip_entrypoint(src.read_text())
        text = prepare_piece(text, run_name, prog_name)
        body += f"\n# ── {mode}: was {src.name} ──\n\n{text}\n"
        mode_map.append((mode, run_name))

    modes_dict = ",\n    ".join(f'"{m}": {r}' for m, r in mode_map)
    help_modes = " | ".join(m for m, _ in mode_map)
    footer = f'''
_MODES = {{
    {modes_dict},
}}

_HELP = """
Modes: {help_modes}
"""


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0] in ("-h", "--help", "help"):
        print(_HELP.strip())
        return 0
    mode = argv[0]
    if mode not in _MODES:
        raise SystemExit(f"unknown mode {{mode!r}}; choose: {{', '.join(_MODES)}}\\n{{_HELP}}")
    rc = _MODES[mode](argv[1:])
    return int(rc or 0)


if __name__ == "__main__":
    raise SystemExit(main())
'''
    text = header + body + footer
    compile(text, str(out_path), "exec")
    out_path.write_text(text)
    out_path.chmod(0o755)


def write_shim(path: Path, target_rel: str, mode: str | None) -> None:
    mode_lit = "None" if mode is None else repr(mode)
    target_expr = f'root / "{target_rel}"'
    if mode is None:
        cmd_build = "cmd = [sys.executable, str(target), *sys.argv[1:]]"
    else:
        cmd_build = f'cmd = [sys.executable, str(target), {repr(mode)}, *sys.argv[1:]]'
    text = f'''#!/usr/bin/env python3
"""Compatibility shim — forwards to scripts/{target_rel}."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parent
    target = {target_expr}
    {cmd_build}
    return subprocess.call(cmd)


if __name__ == "__main__":
    raise SystemExit(main())
'''
    path.write_text(text)
    path.chmod(0o755)


def main() -> int:
    merge_modules(
        BENCH / "guardrails.py",
        "Benchmark CI guardrails and artifact utilities (merged suite).",
        [
            ("regression", SCRIPTS / "check_benchmark_regression.py", "run_regression"),
            ("collect", SCRIPTS / "collect_baseline.py", "run_collect"),
            ("guardrails", SCRIPTS / "check_guardrails.py", "run_guardrails"),
            ("fallback-trend", SCRIPTS / "check_fallback_trend.py", "run_fallback_trend"),
            (
                "clean-artifacts",
                SCRIPTS / "clean_benchmark_artifacts.py",
                "run_clean_artifacts",
            ),
            ("dashboard", SCRIPTS / "performance_dashboard.py", "run_dashboard"),
        ],
        "guardrails.py",
    )
    print("wrote guardrails.py")

    merge_modules(
        BENCH / "live.py",
        "Live multi-tool benchmarks (merged suite).",
        [
            ("all", SCRIPTS / "live_bench.py", "run_all"),
            ("bun", SCRIPTS / "live_bench_jhol_vs_bun.py", "run_bun"),
        ],
        "live.py",
    )
    print("wrote live.py")

    merge_modules(
        SCRIPTS / "reports.py",
        "Engineering reports (merged suite).",
        [
            ("enterprise-npmrc", SCRIPTS / "enterprise_npmrc_report.py", "run_enterprise"),
            ("framework-compat", SCRIPTS / "framework_compat_report.py", "run_framework"),
            ("resolver-fixtures", SCRIPTS / "resolver_fixture_report.py", "run_resolver"),
        ],
        "reports.py",
    )
    print("wrote reports.py")

    merge_modules(
        SCRIPTS / "binary_packages.py",
        "Binary package cache tooling (merged suite).",
        [
            ("build", SCRIPTS / "build_binary_packages.py", "run_build"),
            ("bundle", SCRIPTS / "build_binary_bundle.py", "run_bundle"),
            ("add-from-lock", SCRIPTS / "add_binary_from_npm_lock.py", "run_add_from_lock"),
            ("expand-set", SCRIPTS / "expand_binary_package_set.py", "run_expand_set"),
            ("update-profiles", SCRIPTS / "update_binary_profiles.py", "run_update_profiles"),
            (
                "precompute-unpacked",
                SCRIPTS / "precompute_unpacked_binary_cache.py",
                "run_precompute_unpacked",
            ),
        ],
        "binary_packages.py",
    )
    print("wrote binary_packages.py")

    merge_modules(
        BENCH / "legacy_compare.py",
        "Legacy jhol vs bun comparison scripts (merged suite).",
        [
            ("jhol-vs-bun", SCRIPTS / "benchmark_jhol_vs_bun.py", "run_jhol_vs_bun"),
            ("brutal-cold-warm", SCRIPTS / "brutal_cold_warm_benchmark.py", "run_brutal"),
            ("measure-cold", SCRIPTS / "measure_cold_benchmarks.py", "run_measure_cold"),
        ],
        "legacy_compare.py",
    )
    print("wrote legacy_compare.py")

    # Move standalone bench helpers into scripts/bench/ (preserve content)
    for name in ("bench_scales.py", "bench_daemon.py", "benchmark_offline.py"):
        src = SCRIPTS / name
        if src.is_file():
            dest_name = {
                "bench_scales.py": "scales.py",
                "bench_daemon.py": "daemon.py",
                "benchmark_offline.py": "offline.py",
            }[name]
            dest = BENCH / dest_name
            if not dest.exists() or dest.stat().st_mtime < src.stat().st_mtime:
                dest.write_text(src.read_text())
                dest.chmod(0o755)
            print(f"copied {name} -> bench/{dest_name}")

    v2_src = SCRIPTS / "benchmark_scientific_v2.py"
    v2_dest = BENCH / "scientific_v2.py"
    if v2_src.is_file():
        v2_dest.write_text(v2_src.read_text())
        v2_dest.chmod(0o755)
        print("copied benchmark_scientific_v2.py -> bench/scientific_v2.py")

    # Shims at old paths (overwrite merged sources)
    shims = [
        ("check_benchmark_regression.py", "bench/guardrails.py", "regression"),
        ("collect_baseline.py", "bench/guardrails.py", "collect"),
        ("check_guardrails.py", "bench/guardrails.py", "guardrails"),
        ("check_fallback_trend.py", "bench/guardrails.py", "fallback-trend"),
        ("clean_benchmark_artifacts.py", "bench/guardrails.py", "clean-artifacts"),
        ("performance_dashboard.py", "bench/guardrails.py", "dashboard"),
        ("live_bench.py", "bench/live.py", "all"),
        ("live_bench_jhol_vs_bun.py", "bench/live.py", "bun"),
        ("enterprise_npmrc_report.py", "reports.py", "enterprise-npmrc"),
        ("framework_compat_report.py", "reports.py", "framework-compat"),
        ("resolver_fixture_report.py", "reports.py", "resolver-fixtures"),
        ("build_binary_packages.py", "binary_packages.py", "build"),
        ("build_binary_bundle.py", "binary_packages.py", "bundle"),
        ("add_binary_from_npm_lock.py", "binary_packages.py", "add-from-lock"),
        ("expand_binary_package_set.py", "binary_packages.py", "expand-set"),
        ("update_binary_profiles.py", "binary_packages.py", "update-profiles"),
        (
            "precompute_unpacked_binary_cache.py",
            "binary_packages.py",
            "precompute-unpacked",
        ),
        ("benchmark_jhol_vs_bun.py", "bench/legacy_compare.py", "jhol-vs-bun"),
        ("brutal_cold_warm_benchmark.py", "bench/legacy_compare.py", "brutal-cold-warm"),
        ("measure_cold_benchmarks.py", "bench/legacy_compare.py", "measure-cold"),
        ("bench_scales.py", "bench/scales.py", None),
        ("bench_daemon.py", "bench/daemon.py", None),
        ("benchmark_offline.py", "bench/offline.py", None),
        ("benchmark_scientific_v2.py", "bench/scientific_v2.py", None),
    ]
    for old, target, mode in shims:
        write_shim(SCRIPTS / old, target, mode)

    print("wrote shims")

    # Post-fix repo-relative paths (sources may use scripts/ or hardcoded home paths)
    live_path = BENCH / "live.py"
    live_text = live_path.read_text()
    if "REPO_ROOT" not in live_text:
        live_text = live_text.replace(
            "import sys\n\n\n# ── all:",
            "import sys\nfrom pathlib import Path\n\nREPO_ROOT = Path(__file__).resolve().parents[2]\n\n\n# ── all:",
            1,
        )
        live_text = live_text.replace(
            'Path(__file__).parent.parent / "target"',
            'REPO_ROOT / "target"',
        )
        live_text = live_text.replace(
            "Path(__file__).parent.parent / \"benchmarks\"",
            'REPO_ROOT / "benchmarks"',
        )
        live_path.write_text(live_text)

    legacy_path = BENCH / "legacy_compare.py"
    legacy_text = legacy_path.read_text()
    legacy_text = legacy_text.replace(
        'Path("/Users/bhuvanprakash/j/jhol")',
        "REPO_ROOT",
    )
    if "REPO_ROOT = " not in legacy_text:
        legacy_text = legacy_text.replace(
            "import sys\n\n",
            "import sys\nfrom pathlib import Path\n\nREPO_ROOT = Path(__file__).resolve().parents[2]\n\n",
            1,
        )
    legacy_path.write_text(legacy_text)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
