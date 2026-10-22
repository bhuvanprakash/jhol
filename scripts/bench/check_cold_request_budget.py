#!/usr/bin/env python3
"""Check cold installs fetch package data, with no implicit registry-root probes.

Local server delay only makes background probes observable. Not a speed benchmark.
"""
import argparse
import base64
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def check(binary, expect_probes, resolution_mode=None, expect_relinks=False, fresh_reuse=False, symlink_mode=False, no_prefetch=False, io_prefetch=False, poison_archives=False, bun_control=False, canonical_archives=False):
    def archive_path(name, version):
        return f"/{name}/-/{name}-{version}.tgz" if canonical_archives else f"/{name}-{version}.tgz"

    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode="w") as archive:
        for name, content, mode in [
            ("package.json", b'{"name":"jhol-request-fixture","version":"1.0.0","main":"index.js","bin":{"jhol-fixture":"cli.js"}}', 0o644),
            ("index.js", b"module.exports = 42;\n", 0o644),
            ("cli.js", b"#!/usr/bin/env node\nconsole.log(42);\n", 0o755),
        ]:
            entry = tarfile.TarInfo("package/" + name)
            entry.size, entry.mode = len(content), mode
            archive.addfile(entry, io.BytesIO(content))
    body = gzip.compress(payload.getvalue(), mtime=0)
    integrity = "sha512-" + base64.b64encode(hashlib.sha512(body).digest()).decode()
    requests = []
    request_times = []
    responses = {}
    if resolution_mode:
        packages = [("jhol-request-a", "1.0.0", {"jhol-request-child": "1.0.0"}),
                    ("jhol-request-b", "1.0.0", {"jhol-request-child": "2.0.0"}),
                    ("jhol-request-child", "2.0.0", {})]
        if resolution_mode in ("valid", "budget"):
            packages.append(("jhol-request-child", "1.0.0", {}))
        if resolution_mode in ("wave", "wave-error"):
            packages = []
            for i in range(6):
                child = f"jhol-wave-child-{i}"
                packages.extend([(f"jhol-wave-a-{i}", "1.0.0", {child: "1.0.0"}),
                                 (f"jhol-wave-b-{i}", "1.0.0", {child: "2.0.0"}),
                                 (child, "1.0.0", {}), (child, "2.0.0", {})])
        if resolution_mode == "frontier":
            packages = [(f"jhol-frontier-{i}-{level}", "1.0.0",
                         {f"jhol-frontier-{i}-{level+1}": "1.0.0"} if level < 3 else {})
                        for i in range(6) for level in range(4)]
        for name, version, deps in packages:
            info = {"name": name, "version": version, "main": "index.js", "dependencies": deps}
            data = io.BytesIO()
            code = ("module.exports = require(" + json.dumps(next(iter(deps))) + ");\n" if deps else "module.exports = " + json.dumps(version) + ";\n")
            with tarfile.open(fileobj=data, mode="w") as archive:
                for filename, content in [("package.json", json.dumps(info).encode()), ("index.js", code.encode())]:
                    entry = tarfile.TarInfo("package/" + filename)
                    entry.size, entry.mode = len(content), 0o644
                    archive.addfile(entry, io.BytesIO(content))
            compressed = gzip.compress(data.getvalue(), mtime=0)
            responses[archive_path(name, version)] = compressed
            responses["/" + name + "/" + version] = info
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_HEAD(self):
            requests.append(["HEAD", self.path])
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def do_GET(self):
            requests.append(["GET", self.path])
            request_times.append([self.path, time.perf_counter()])
            if resolution_mode in ("wave", "wave-error"):
                if self.path.startswith("/jhol-wave-b-") and self.path.endswith("/1.0.0"):
                    time.sleep(0.06)
                elif self.path.startswith("/jhol-wave-child-"):
                    if self.path.endswith("/1.0.0") or self.path.endswith("/2.0.0"):
                        time.sleep(0.20)
                    elif self.path.count("/") == 1 and not self.path.endswith(".tgz"):
                        time.sleep(0.18)
                        if resolution_mode == "wave-error":
                            self.send_response(404)
                            self.send_header("Content-Length", "0")
                            self.end_headers()
                            return
            if resolution_mode == "frontier":
                time.sleep(0.20 if self.path.endswith(".tgz") else 0.02)
            data = responses.get(self.path, body if self.path == "/fixture.tgz" else b"{}")
            if isinstance(data, dict):
                data = json.dumps(data).encode()
            if self.path == "/fixture.tgz":
                time.sleep(0.15)
            if poison_archives and self.path.endswith(".tgz"):
                data = gzip.compress(gzip.decompress(data).replace(b'module.exports = "1.0.0"', b'module.exports = "1.0.9"'), mtime=0)
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    registry = "http://127.0.0.1:" + str(server.server_port)
    if resolution_mode:
        for name, version, deps in packages:
            info = responses["/" + name + "/" + version]
            archive = responses[archive_path(name, version)]
            info["dist"] = {"tarball": registry + archive_path(name, version),
                            "integrity": "sha512-" + base64.b64encode(hashlib.sha512(archive).digest()).decode()}
            meta = responses.setdefault("/" + name, {"name": name, "versions": {}, "dist-tags": {"latest": version}})
            meta["versions"][version] = info
    observations = []
    shared = tempfile.TemporaryDirectory(prefix="jhol-resolution-reuse-")
    try:
        for native, replay in [(n, r) for n in ([False] if bun_control else [False, True]) for r in range(2 if fresh_reuse else 1)]:
            requests.clear()
            request_times.clear()
            with tempfile.TemporaryDirectory(prefix="jhol-request-budget-") as temporary:
                project = Path(temporary)
                manifest = {"name": "request-budget", "version": "1.0.0", "dependencies": {"jhol-request-fixture": "1.0.0"}}
                lock = {"name": "request-budget", "version": "1.0.0", "lockfileVersion": 3, "packages": {"": manifest, "node_modules/jhol-request-fixture": {"version": "1.0.0", "resolved": registry + "/fixture.tgz", "integrity": integrity}}}
                (project / "package.json").write_text(json.dumps(manifest))
                if resolution_mode:
                    manifest["dependencies"] = {"jhol-request-a": "1.0.0", "jhol-request-b": "1.0.0"}
                    if resolution_mode in ("wave", "wave-error"):
                        manifest["dependencies"] = {f"jhol-wave-{side}-{i}": "1.0.0" for i in range(6) for side in ("a", "b")}
                    if resolution_mode == "frontier":
                        manifest["dependencies"] = {f"jhol-frontier-{i}-0": "1.0.0" for i in range(6)}
                    (project / "package.json").write_text(json.dumps(manifest))
                else:
                    (project / "package-lock.json").write_text(json.dumps(lock))
                env = dict({k: v for k, v in os.environ.items() if not k.startswith("JHOL_")}, HOME=temporary, JHOL_CACHE_DIR=str(project / "cache"), JHOL_REGISTRY=registry, JHOL_NO_HARVEST="1", JHOL_NO_DAEMON="1", JHOL_AUTO_DAEMON="0", JHOL_JGRID="0", JHOL_JGRID_L0="0", JHOL_JAGRS_MIRROR="0", JHOL_STREAM_EXTRACT="0", JHOL_HTTP2_POOL_SIZE="2", JHOL_NO_SPLASH="1")
                if fresh_reuse:
                    env.update(HOME=shared.name, JHOL_CACHE_DIR=str(Path(shared.name) / ("native-cache" if native else "plain-cache")))
                env.pop("JHOL_OFFLINE", None)
                env.pop("JHOL_NO_HTTP2", None)
                args = [str(binary), "install", "--no-scripts"] + (["--native-only"] if native else [])
                env.update(JHOL_PREBUILT="0", JHOL_NO_JMR="1", JHOL_NO_SPLASH="1", JHOL_DEBUG_LAYOUT="1")
                if resolution_mode in ("wave", "wave-error"):
                    env.update(RAYON_NUM_THREADS="32", JHOL_PACKUMENT_CONCURRENCY="32", JHOL_PROFILE_INSTALL="1")
                if resolution_mode == "frontier":
                    env.update(RAYON_NUM_THREADS="4", JHOL_PROFILE_INSTALL="1")
                if no_prefetch:
                    env["JHOL_NO_PREFETCH"] = "1"
                env["JHOL_PREFETCH_IO_POOL"] = "1" if io_prefetch else "0"
                if resolution_mode == "budget":
                    env["JHOL_JAGR_MAX_DEPS"] = "1"
                env.pop("JHOL_LINK", None)
                if symlink_mode:
                    env["JHOL_LINK"] = "1"
                if bun_control:
                    (project / ".npmrc").write_text("registry=" + registry + "\n")
                    env["BUN_INSTALL_CACHE_DIR"] = str(project / "bun-cache")
                    args = [str(binary), "install", "--ignore-scripts"]
                started = time.perf_counter()
                result = subprocess.run(args, cwd=project, env=env, capture_output=True, text=True, timeout=30)
                if resolution_mode:
                    observation = {"mode": resolution_mode, "native_only": native, "fresh_project_reuse": bool(replay), "exit": result.returncode,
                                   "elapsed_ms": (time.perf_counter()-started)*1000,
                                   "requests": list(requests), "raw_output": result.stdout + result.stderr,
                                   "success_marker": (project / "node_modules/.jhol-ok").exists()}
                    if resolution_mode in ("unsatisfiable", "budget", "wave-error") or poison_archives:
                        observation["pass"] = result.returncode != 0 and not observation["success_marker"]
                        if poison_archives:
                            observation["pass"] &= "integrity" in observation["raw_output"].lower()
                    else:
                        consumer_code = "const a=require('jhol-request-a'), b=require('jhol-request-b'); if(a!=='1.0.0'||b!=='2.0.0')process.exit(1);"
                        if resolution_mode == "wave":
                            consumer_code = "for(let i=0;i<6;i++){if(require('jhol-wave-a-'+i)!=='1.0.0'||require('jhol-wave-b-'+i)!=='2.0.0')process.exit(1)}"
                            full = [t for path,t in request_times if path.startswith("/jhol-wave-child-") and path.count("/") == 1 and not path.endswith(".tgz")]
                            observation.update(full_packument_requests=len(full), full_packument_arrival_span_ms=(max(full)-min(full))*1000 if full else None)
                        if resolution_mode == "frontier":
                            consumer_code = "for(let i=0;i<6;i++){if(require('jhol-frontier-'+i+'-0')!=='1.0.0')process.exit(1)}"
                            leaves = [t for path,t in request_times if path.startswith("/jhol-frontier-") and path.endswith("-3/1.0.0")]
                            archives = [path for method,path in requests if path.endswith(".tgz")]
                            observation.update(no_prefetch=no_prefetch, io_prefetch=io_prefetch,
                                               final_leaf_metadata_start_ms=(max(leaves)-started)*1000 if leaves else None,
                                               archive_requests=len(archives), unique_archives=len(set(archives)))
                        q = subprocess.run(["node", "-e", consumer_code], cwd=project, capture_output=True, text=True, timeout=10)
                        aliases = [str(p.relative_to(project)) for p in (project / "node_modules").rglob("*") if p.is_symlink() and ".bin" not in p.parts]
                        observation.update(consumer_exit=q.returncode, generated_aliases=aliases,
                                           relink_passes=observation["raw_output"].count("[collect]"))
                        observation["pass"] = result.returncode == q.returncode == 0 and bool(aliases) == expect_relinks
                        if resolution_mode == "frontier":
                            advertised = {path for path in responses if path.endswith(".tgz")}
                            written = json.loads((project / "package-lock.json").read_text()) if result.returncode == 0 and (project / "package-lock.json").is_file() else {"packages": {}}
                            expected_lock = all(
                                written["packages"].get("node_modules/" + name, {}).get("resolved") == responses["/" + name + "/" + version]["dist"]["tarball"]
                                and written["packages"].get("node_modules/" + name, {}).get("integrity") == responses["/" + name + "/" + version]["dist"]["integrity"]
                                for name, version, deps in packages)
                            observation.update(advertised_urls_only=set(archives) == advertised,
                                               lock_descriptors_preserved=expected_lock)
                            observation["pass"] &= set(archives) == advertised and len(archives) == 24 and (expected_lock or bun_control)
                    observations.append(observation)
                    continue
                assert result.returncode == 0, result.stderr
                consumer = subprocess.check_output(["node", "-e", "console.log(require('jhol-request-fixture'))"], cwd=project, text=True).strip()
                executable = subprocess.check_output([str(project / "node_modules/.bin/jhol-fixture")], cwd=project, text=True).strip()
                assert consumer == executable == "42"
                written = json.loads((project / "package-lock.json").read_text())
                assert written["packages"]["node_modules/jhol-request-fixture"]["integrity"] == integrity
                assert requests.count(["GET", "/fixture.tgz"]) == 1, requests
                roots = [r for r in requests if r[1] == "/"]
                assert bool(roots) == expect_probes, requests
                observations.append({"native_only": native, "requests": list(requests), "consumer": consumer, "executable": executable, "sri_preserved": True})
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
        shared.cleanup()
    return observations


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--resolution-mode", choices=["valid", "unsatisfiable", "budget", "wave", "wave-error", "frontier"], help="Check a conflicting version domain against the local registry")
    parser.add_argument("--canonical-archives", action="store_true", help="Advertise canonical archive URLs to isolate foreground SRI propagation")
    parser.add_argument("--bun-control", action="store_true", help="Run equivalent fresh-cache Bun control on resolution fixtures")
    parser.add_argument("--poison-archives", action="store_true", help="Serve altered archive bytes under unchanged published SRI")
    parser.add_argument("--no-prefetch", action="store_true", help="Disable archive prefetch for discovery interference control")
    parser.add_argument("--io-prefetch", action="store_true", help="Select experimental IO-pool prefetch")
    parser.add_argument("--symlink", action="store_true", help="Check explicit store symlink mode")
    parser.add_argument("--fresh-reuse", action="store_true", help="Repeat in a fresh project using the first project cache")
    parser.add_argument("--expect-relinks", action="store_true", help="Reproduce redundant clone dependency aliases on saved baseline")
    parser.add_argument("--expect-probes", action="store_true", help="Reproduce old behavior using saved baseline")
    options = parser.parse_args()
    observations = check(options.binary.resolve(), options.expect_probes, options.resolution_mode, options.expect_relinks, options.fresh_reuse, options.symlink, options.no_prefetch, options.io_prefetch, options.poison_archives, options.bun_control, options.canonical_archives)
    print(json.dumps(observations, indent=2))
    if options.resolution_mode and not all(o["pass"] for o in observations):
        raise SystemExit(1)
