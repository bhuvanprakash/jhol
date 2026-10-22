#!/usr/bin/env python3
"""C front door must forward to the shared proof engine, including stale trees."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def check(binary):
    observations = []
    with tempfile.TemporaryDirectory(prefix="jhol-front-proof-") as temporary:
        root = Path(temporary)
        front = root / "jhol-front"
        shutil.copy2(binary, front)
        (root / "package.json").write_text('{"dependencies":{"required":"1.0.0"}}')
        (root / "package-lock.json").write_text('{"lockfileVersion":3,"packages":{"node_modules/required":{"version":"1.0.0"}}}')
        (root / "node_modules").mkdir()
        marker = root / "node_modules/.jhol-ok"
        marker.write_text("legacy timestamp marker")
        timestamp = time.time() + 60
        os.utime(marker, (timestamp, timestamp))
        # The adjacent backend stands in for normal validation. Exit 0 is wrong.
        backend = '#!/usr/bin/env python3\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\nsys.exit(71)\n'
        for name in ("jhol-trampoline", "jhol"):
            path = root / name
            path.write_text(backend)
            path.chmod(0o755)
        cases = [["install"], ["i"], ["install", "--native-only"],
                 ["install", "--no-scripts"], ["install", "--frozen"],
                 ["install", "required@2.0.0"], ["ci"], ["--version"]]
        for args in cases:
            result = subprocess.run([str(front), *args], cwd=root, capture_output=True, text=True, timeout=10)
            assert result.returncode == 71, f"false success or missed delegate: {args}: exit {result.returncode}"
            assert json.loads(result.stdout) == args, "arguments changed"
            observations.append({"args": args, "exit": result.returncode, "arguments_preserved": True})
        (root / "jhol-trampoline").unlink()
        result = subprocess.run([str(front), "install"], cwd=root, capture_output=True, text=True, timeout=10)
        assert result.returncode == 71 and json.loads(result.stdout) == ["install"]
        observations.append({"fallback_to_cli": True, "exit": result.returncode})
    return observations


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    print(json.dumps(check(args.binary.resolve()), indent=2))
