#!/usr/bin/env python3
"""Generate binary package graph profiles from npm lockfile closures.

Profiles help bypass dependency expansion for common seed sets.
"""

from __future__ import annotations

import argparse
import json
import platform
import subprocess
import tempfile
from pathlib import Path
from typing import Dict, List, Optional, Tuple


def npm_os_name() -> str:
    sys_name = platform.system().lower()
    if sys_name == "darwin":
        return "darwin"
    if sys_name == "linux":
        return "linux"
    if sys_name.startswith("win"):
        return "win32"
    return sys_name


def npm_cpu_name() -> str:
    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64"}:
        return "x64"
    if machine in {"aarch64", "arm64"}:
        return "arm64"
    return machine


def field_allows_current(value: object, current: str) -> bool:
    if value is None:
        return True
    if isinstance(value, str):
        values = [value]
    elif isinstance(value, list):
        values = [str(v) for v in value]
    else:
        return True

    positives = []
    for token in values:
        token = token.strip()
        if not token:
            continue
        if token.startswith("!"):
            if token[1:] == current:
                return False
        else:
            positives.append(token)

    return not positives or current in positives


def package_name_from_lock_path(path: str, entry: Dict[str, object]) -> Optional[str]:
    name = entry.get("name")
    if isinstance(name, str) and name:
        return name

    parts = [p for p in path.split("/") if p]
    if "node_modules" not in parts:
        return None
    idx = len(parts) - 1 - parts[::-1].index("node_modules")
    seg = parts[idx + 1 :]
    if not seg:
        return None
    if seg[0].startswith("@") and len(seg) >= 2:
        return f"{seg[0]}/{seg[1]}"
    return seg[0]


def build_lockfile(seed_specs: List[str]) -> Dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="jhol-profile-") as td:
        root = Path(td)
        subprocess.run(["npm", "init", "-y"], cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(
            ["npm", "install", *seed_specs, "--package-lock-only", "--ignore-scripts"],
            cwd=root,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return json.loads((root / "package-lock.json").read_text())


def profile_key(seed_specs: List[str]) -> str:
    names = []
    for spec in seed_specs:
        if spec.startswith("@"):
            idx = spec.rfind("@")
            if idx > 0:
                names.append(spec[:idx])
            else:
                names.append(spec)
        else:
            if "@" in spec:
                names.append(spec.split("@", 1)[0])
            else:
                names.append(spec)
    names = sorted(set(names))
    return ",".join(names)


def closure_specs(seed_specs: List[str]) -> List[str]:
    lock = build_lockfile(seed_specs)
    packages_obj = lock.get("packages", {})
    if not isinstance(packages_obj, dict):
        return []

    current_os = npm_os_name()
    current_cpu = npm_cpu_name()

    specs: List[str] = []
    for path, entry_obj in packages_obj.items():
        if path == "" or not isinstance(entry_obj, dict):
            continue
        name = package_name_from_lock_path(path, entry_obj)
        version = entry_obj.get("version")
        if not isinstance(name, str) or not name:
            continue
        if not isinstance(version, str) or not version:
            continue
        if not field_allows_current(entry_obj.get("os"), current_os):
            continue
        if not field_allows_current(entry_obj.get("cpu"), current_cpu):
            continue
        specs.append(f"{name}@{version}")

    return sorted(set(specs))


def main() -> int:
    parser = argparse.ArgumentParser(description="Update binary profiles")
    parser.add_argument("--binary-dir", default="binary-packages")
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        help="Comma-separated seed specs (example: lodash,axios,chalk)",
    )
    args = parser.parse_args()

    binary_dir = Path(args.binary_dir)
    profiles_path = binary_dir / "profiles.json"

    profiles: Dict[str, List[str]] = {}
    if profiles_path.exists():
        raw = json.loads(profiles_path.read_text())
        if isinstance(raw, dict):
            profiles = {
                str(k): sorted(set(str(x) for x in v if isinstance(x, str)))
                for k, v in raw.items()
                if isinstance(v, list)
            }

    entries = args.profile or [
        "lodash,axios,chalk",
        "react,next,typescript,express",
    ]

    for raw in entries:
        seed_specs = [p.strip() for p in raw.split(",") if p.strip()]
        if not seed_specs:
            continue
        key = profile_key(seed_specs)
        specs = closure_specs(seed_specs)
        profiles[key] = specs
        print(f"profile {key}: {len(specs)} specs")

    profiles_path.write_text(json.dumps(dict(sorted(profiles.items())), indent=2) + "\n")
    print(f"wrote {profiles_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
