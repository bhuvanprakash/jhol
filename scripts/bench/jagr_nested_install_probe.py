#!/usr/bin/env python3
"""Verify a native nested install and offline replay against a local fixture.

Usage: python3 scripts/bench/jagr_nested_install_probe.py target/release/jhol
"""
import base64
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

responses = {}
requests = []

class Registry(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        requests.append(path)
        value = responses.get(path)
        if value is None:
            self.send_error(404)
            return
        body = json.dumps(value).encode() if isinstance(value, dict) else value
        self.send_response(200)
        self.send_header("Content-Type", "application/json" if isinstance(value, dict) else "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass

def release(name, version, deps=None, optional=None, peers=None):
    manifest = {"name": name, "version": version, "main": "index.js"}
    for field, value in [("dependencies", deps), ("optionalDependencies", optional), ("peerDependencies", peers)]:
        if value:
            manifest[field] = value
    stream = io.BytesIO()
    code = f"module.exports = {json.dumps(name + '@' + version)};\n"
    with tarfile.open(fileobj=stream, mode="w") as archive:
        for filename, data in [("package.json", json.dumps(manifest).encode()), ("index.js", code.encode())]:
            member = tarfile.TarInfo(f"package/{filename}")
            member.size, member.mode = len(data), 0o644
            archive.addfile(member, io.BytesIO(data))
    body = gzip.compress(stream.getvalue(), mtime=0)
    artifact = f"/{name}/-/{name}-{version}.tgz"
    responses[artifact] = body
    manifest["dist"] = {"tarball": registry + artifact,
                        "integrity": "sha512-" + base64.b64encode(hashlib.sha512(body).digest()).decode()}
    packument = responses.setdefault(f"/{name}", {"name": name, "versions": {}, "dist-tags": {}})
    packument["versions"][version] = manifest
    packument["dist-tags"]["latest"] = version
    responses[f"/{name}/{version}"] = manifest

binary = Path(sys.argv[1]).resolve(strict=True)
with ThreadingHTTPServer(("127.0.0.1", 0), Registry) as server, tempfile.TemporaryDirectory(prefix="jhol-nested-install-") as temporary:
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    registry = f"http://127.0.0.1:{server.server_port}"
    release("jhol-probe-real", "1.0.0")
    release("jhol-probe-real", "2.0.0")
    release("jhol-probe-other", "1.0.0")
    release("jhol-probe-wrapper", "1.0.0", {
        "jhol-probe-alias": "npm:jhol-probe-other@1.0.0",
        "jhol-probe-real": "2.0.0",
    })
    root = Path(temporary)
    project = root / "project"
    project.mkdir()
    (root / "home").mkdir()
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("JHOL_", "JAGR_", "npm_config_", "NPM_CONFIG_"))}
    env.update(HOME=str(root / "home"), USERPROFILE=str(root / "home"),
               JHOL_CACHE_DIR=str(root / "cache"), JHOL_REGISTRY=registry,
               JHOL_NO_HARVEST="1", JHOL_NO_DAEMON="1", JHOL_AUTO_DAEMON="0",
               JHOL_JGRID="0", JHOL_JGRID_L0="0", JHOL_JAGRS_MIRROR="0",
               JHOL_NO_PREPARED_SNAPSHOT="1", JHOL_NO_SPLASH="1", JHOL_SINGLE_PASS="0")
    manifest = {"name": "jhol-nested-probe", "version": "1.0.0", "private": True, "dependencies": {
        "jhol-probe-real": "1.0.0",
        "jhol-probe-alias": "npm:jhol-probe-real@1.0.0",
        "jhol-probe-alias-two": "npm:jhol-probe-real@2.0.0",
        "jhol-probe-wrapper": "1.0.0",
    }}
    (project / "package.json").write_text(json.dumps(manifest))
    expected = {
        "node_modules/jhol-probe-real": ("jhol-probe-real", "1.0.0"),
        "node_modules/jhol-probe-alias": ("jhol-probe-real", "1.0.0"),
        "node_modules/jhol-probe-alias-two": ("jhol-probe-real", "2.0.0"),
        "node_modules/jhol-probe-wrapper": ("jhol-probe-wrapper", "1.0.0"),
        "node_modules/jhol-probe-wrapper/node_modules/jhol-probe-real": ("jhol-probe-real", "2.0.0"),
        "node_modules/jhol-probe-wrapper/node_modules/jhol-probe-alias": ("jhol-probe-other", "1.0.0"),
    }
    results = []
    for arguments in (("install", "--native-only", "--no-scripts"),
                      ("install", "--native-only", "--offline", "--frozen-lockfile", "--no-scripts")):
        before = len(requests)
        result = subprocess.run([binary, *arguments], cwd=project, env=env, capture_output=True, text=True, timeout=90)
        if result.returncode:
            raise RuntimeError(f"{arguments}: {result.returncode}\n{result.stdout[-2000:]}\n{result.stderr[-2000:]}")
        lock = json.loads((project / "package-lock.json").read_text())["packages"]
        assert set(lock) == {"", *expected}, sorted(lock)
        for slot, (name, version) in expected.items():
            current = json.loads((project / slot / "package.json").read_text())
            assert (current["name"], current["version"]) == (name, version), (slot, current)
            assert lock[slot].get("name", slot.rsplit("node_modules/", 1)[-1]) == name, lock[slot]
            assert lock[slot]["version"] == version
            assert not (project / slot).is_symlink()
        assert not (project / ".jhol.capsule").exists()
        if "--offline" in arguments:
            assert len(requests) == before, requests[before:]
        results.append({"arguments": arguments, "verified_slots": len(expected), "registry_requests": len(requests) - before})
        shutil.rmtree(project / "node_modules")
    print(json.dumps(results, indent=2))
    server.shutdown()
