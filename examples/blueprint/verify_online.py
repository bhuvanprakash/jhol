#!/usr/bin/env python3
"""Opt-in third-party dependency smoke test for the JS/Python/Rust adapters."""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tempfile


def fail(message: str) -> None:
    raise SystemExit(f"verify_online: {message}")


def main() -> None:
    repo_root = pathlib.Path(__file__).resolve().parents[2]
    executable_suffix = ".exe" if os.name == "nt" else ""
    configured = os.environ.get("JHOL_BIN")
    candidates = [
        pathlib.Path(configured) if configured else None,
        repo_root / f"target/debug/jhol{executable_suffix}",
        repo_root / f"target/release/jhol{executable_suffix}",
    ]
    binary = next((path for path in candidates if path and path.is_file()), None)
    if binary is None:
        found = shutil.which("jhol")
        binary = pathlib.Path(found) if found else None
    if binary is None:
        fail("jhol binary not found; set JHOL_BIN or build --bin jhol")
    for tool in ("uv", "cargo"):
        if shutil.which(tool) is None:
            fail(f"required native tool not found on PATH: {tool}")

    with tempfile.TemporaryDirectory(prefix="jhol-online-adapters-") as temp:
        root = pathlib.Path(temp)
        (root / "package.json").write_text(
            json.dumps(
                {
                    "name": "jhol-adapter-real-check",
                    "version": "0.0.1",
                    "private": True,
                    "dependencies": {"ms": "2.1.3"},
                }
            )
            + "\n",
            encoding="utf-8",
        )
        (root / "pyproject.toml").write_text(
            '[project]\nname = "jhol-adapter-real-check"\nversion = "0.0.1"\n'
            'requires-python = ">=3.10"\ndependencies = ["idna==3.10"]\n',
            encoding="utf-8",
        )
        (root / "Cargo.toml").write_text(
            '[package]\nname = "jhol-adapter-real-check"\nversion = "0.1.0"\n'
            'edition = "2021"\n\n[dependencies]\nitoa = "=1.0.18"\n',
            encoding="utf-8",
        )
        (root / "src").mkdir()
        (root / "src/main.rs").write_text(
            'fn main() { println!("{}", itoa::Buffer::new().format(42)); }\n',
            encoding="utf-8",
        )

        result = subprocess.run(
            [
                str(binary),
                "sync",
                "--path",
                str(root),
                "--apply",
                "--allow-build-scripts",
                "--json",
            ],
            text=True,
            capture_output=True,
            check=False,
        )
        marker = result.stdout.find('{\n  "adapter_plans"')
        if result.returncode or marker < 0:
            fail(
                f"sync failed or JSON receipt missing (exit {result.returncode}); "
                f"stdout={result.stdout[:800]!r}; stderr={result.stderr[-1600:]!r}"
            )
        report = json.loads(result.stdout[marker:])
        receipt = report["receipt"]
        if receipt["status"] != "ok":
            fail(f"sync returned receipt status {receipt['status']!r}")

        receipts = receipt["adapter_receipts"]
        ecosystems = {item["adapter"]["ecosystem"] for item in receipts}
        if ecosystems != {"javascript", "python", "rust"}:
            fail(f"expected all three adapter receipts, found {sorted(ecosystems)}")
        if any(item["exit_status"] != 0 for item in receipts):
            fail("an adapter receipt contains a nonzero exit status")
        native_locks = {item["kind"] for item in report["lock"]["native_locks"]}
        if native_locks != {"npm", "uv", "cargo"}:
            fail(f"expected npm, uv and cargo lock references, found {sorted(native_locks)}")
        project_lock = json.loads(
            (root / "jhol.project.lock").read_text(encoding="utf-8")
        )
        for reference in project_lock["native_locks"]:
            digest = hashlib.sha256((root / reference["path"]).read_bytes()).hexdigest()
            if digest != reference["digest"]:
                fail(f"project lock digest mismatch for {reference['path']}")

        required = [
            "package-lock.json",
            "uv.lock",
            "Cargo.lock",
            "jhol.project.lock",
            "node_modules/ms/index.js",
            ".venv/pyvenv.cfg",
        ]
        missing = [name for name in required if not (root / name).exists()]
        if missing:
            fail(f"expected dependency outputs are missing: {missing}")
        package = json.loads(
            (root / "node_modules/ms/package.json").read_text(encoding="utf-8")
        )
        if package.get("version") != "2.1.3":
            fail(f"installed ms version mismatch: {package.get('version')!r}")

        if os.name == "nt":
            python = root / ".venv/Scripts/python.exe"
            cargo_program = root / f"target/debug/jhol-adapter-real-check{executable_suffix}"
        else:
            python = root / ".venv/bin/python"
            cargo_program = root / "target/debug/jhol-adapter-real-check"
        subprocess.run(
            [str(python), "-c", "import idna; assert idna.__version__ == '3.10'"],
            check=True,
        )
        cargo_output = subprocess.run(
            [str(cargo_program)], text=True, capture_output=True, check=True
        ).stdout.strip()
        if cargo_output != "42":
            fail(f"compiled Rust dependency smoke test returned {cargo_output!r}")

        versions = {
            item["adapter"]["ecosystem"]: item["tool_versions"]
            for item in receipts
        }
        print("verify_online: real JS/Python/Rust dependencies and native locks passed")
        print("verify_online: adapters=javascript,python,rust")
        print("verify_online: packages=ms@2.1.3,idna==3.10,itoa=1.0.18")
        print("verify_online: tool_versions=" + json.dumps(versions, sort_keys=True))


if __name__ == "__main__":
    main()
