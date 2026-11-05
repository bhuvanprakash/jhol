#!/usr/bin/env python3
"""Local npm-compatible registry serving hostile tarballs.

Usage: python3 serve_hostile.py [port]
Each GET /hostile-<scenario> serves a packument whose dist.tarball points
back at this server, which serves the raw hostile tarball bytes.
"""
import base64, hashlib, http.server, io, json, re, tarfile

def make_tar(entries):
    buf = io.BytesIO()
    tar = tarfile.open(fileobj=buf, mode="w:gz")
    for name, data in entries:
        info = tarfile.TarInfo(name)
        info.size = len(data)
        tar.addfile(info, io.BytesIO(data))
    tar.close()
    return buf.getvalue()

TARBALLS = {
    "traverse": make_tar([
        ("package/index.js", b"module.exports = 1;"),
        ("package/../../escaped.txt", b"pwned"),
    ]),
    "absolute": make_tar([
        ("package/index.js", b"module.exports = 1;"),
        ("/tmp/jhol-hostile-absolute.txt", b"pwned"),
    ]),
    "backslash": make_tar([
        ("package\\..\\evil.js", b"pwned"),
    ]),
    "symlink_escape": make_tar([
        ("package/index.js", b"module.exports = 1;"),
        ("package/escape", b"../../../outside"),
    ]),
    "clean": make_tar([
        ("package/index.js", b"module.exports = { ok: true };"),
        ("package/package.json", json.dumps({"name": "hostile-clean", "version": "1.0.0", "main": "index.js"}).encode()),
    ]),
}

INTEGRITY = {k: "sha512-" + base64.b64encode(hashlib.sha512(v).digest()).decode()
             for k, v in TARBALLS.items()}

class Handler(http.server.BaseHTTPRequestHandler):
    port = 4891
    def log_message(self, *a): pass
    def do_GET(self):
        p = self.path.lstrip("/")
        m = re.match(r"^hostile-(\w+)/-/pkg\.tgz$", p)
        if m:
            body = TARBALLS.get(m.group(1))
            if body:
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers(); self.wfile.write(body)
            else:
                self.send_response(404); self.end_headers()
            return
        m = re.match(r"^hostile-(\w+)$", p)
        if m:
            scenario = m.group(1)
            name = f"hostile-{scenario}"
            tgz_url = f"http://127.0.0.1:{self.port}/{name}/-/pkg.tgz"
            pk = {"_id": name, "name": name, "dist-tags": {"latest": "1.0.0"},
                  "versions": {"1.0.0": {"name": name, "version": "1.0.0",
                    "dist": {"tarball": tgz_url, "integrity": INTEGRITY[scenario]}}}}
            body = json.dumps(pk).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body)
            return
        self.send_response(404); self.end_headers()

if __name__ == "__main__":
    port = int(__import__("sys").argv[1]) if len(__import__("sys").argv) > 1 else 4891
    Handler.port = port
    http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
