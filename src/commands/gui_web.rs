//! `jhol gui --web` — the project in a browser, without a webview dependency.
//!
//! One page on `127.0.0.1:<random>`, reading `/status.json` (the same struct `jhol status`
//! prints and the TUI renders) and posting to `/action`. `std::net` and a page: no Tauri,
//! no Electron, and no second implementation of anything.
//!
//! Read endpoints are `GET`; anything that changes the tree is a `POST` carrying the
//! session token from the URL that was opened, so a page on another origin cannot drive
//! installs on `127.0.0.1` even though it can reach the port.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Refuse to read a body larger than this. The UI posts small JSON objects; anything bigger
/// is either a mistake or an attempt to make the server allocate.
const MAX_BODY: usize = 8 * 1024;
/// A connection that cannot produce a request line in this long is not a browser.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// A token that survives restarts, so an open dashboard does not become stale every time the
/// binary is rebuilt.
///
/// It lives in `.jhol/gui-token` mode 0600 — the same protection any file in the project has —
/// and it only guards against *other origins* reaching loopback, so a per-project credential is
/// the right lifetime. `--new-token` rotates it when a token has been seen or copied.
fn project_token(rotate: bool) -> String {
    let path = Path::new(".jhol/gui-token");
    if !rotate {
        if let Ok(existing) = std::fs::read_to_string(path) {
            let t = existing.trim().to_string();
            if t.len() >= 16 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                return t;
            }
        }
    }
    let token = session_token();
    if std::fs::create_dir_all(".jhol").is_ok() {
        let tmp = Path::new(".jhol/gui-token.tmp");
        if std::fs::write(tmp, format!("{token}\n")).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(tmp, path);
        }
    }
    token
}

pub fn serve(port: u16, open: bool, rotate: bool) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| match e.kind() {
        std::io::ErrorKind::AddrInUse => format!(
            "gui --web: port {port} is already in use — pass --port 0 for a free one"
        ),
        _ => format!("gui --web: {e}"),
    })?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let shared = Arc::new(Shared {
        token: project_token(rotate),
        cache: Mutex::new((String::new(), String::new(), None)),
        packages: Mutex::new((String::new(), String::new())),
    });
    let url = format!("http://{addr}/?t={}", shared.token);
    println!("jhol gui --web on {url}");
    println!(
        "  stop with Ctrl-C · loopback only · token kept in .jhol/gui-token (rotate with --new-token)"
    );
    if open {
        open_browser(&url);
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        // One thread per connection: an inspect that fetches a 16 MB remote header must not
        // freeze the dashboard, and neither must a slow client.
        std::thread::spawn(move || {
            let _ = handle(stream, &shared);
        });
    }
    Ok(())
}

struct Shared {
    token: String,
    /// (fingerprint, serialised package list). Walking 108 package directories is the one
    /// expensive read in this server, so it happens once per change, not once per request.
    packages: Mutex<(String, String)>,
    /// (fingerprint, serialised status, the status itself). A matching fingerprint serves the
    /// previous body, so two tabs polling at once cost one collection.
    cache: Mutex<(String, String, Option<jhol_core::status::Status>)>,
}

fn handle(mut stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    if !path.starts_with('/') {
        return respond(&mut stream, 400, "application/json", "{\"error\":\"bad request\"}", &[]);
    }
    let (headers, length) = read_headers(&mut reader)?;
    let if_none_match = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("if-none-match"))
        .map(|(_, v)| v.trim().to_string());
    let given = query_param(&path, "t");

    // Two ways to be trusted, and only two:
    //   1. the session token, for scripts and for explicit use;
    //   2. a browser attesting that the request is same-origin (`Sec-Fetch-Site`), which a page
    //      on another origin cannot forge.
    //
    // The second exists because a token that lives in a URL goes stale the moment the token
    // rotates, and a dashboard that breaks on reload is worse than the CSRF it prevents: a
    // cross-site page cannot read our responses (no CORS) and now cannot write either.
    let needs_authority =
        path.starts_with("/action") || path.starts_with("/mcp") || path.starts_with("/inspect");
    let same_origin = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("sec-fetch-site"))
        .map(|(_, v)| v.trim().eq_ignore_ascii_case("same-origin"))
        .unwrap_or(false);
    if needs_authority && !same_origin && !token_matches(&given, &shared.token) {
        return respond(
            &mut stream,
            403,
            "application/json",
            "{\"ok\":false,\"code\":403,\"stderr\":\"bad token\"}",
            &[],
        );
    }

    if path == "/healthz" {
        return respond(&mut stream, 200, "text/plain; charset=utf-8", "ok", &[]);
    }

    if path.starts_with("/action") {
        if method != "POST" {
            return respond(&mut stream, 405, "application/json", "{\"error\":\"POST only\"}", &[("Allow", "POST")]);
        }
        let body = read_body(&mut reader, length);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("");
        let name = v.get("name").and_then(|a| a.as_str()).unwrap_or("");
        if path.contains("stream=1") {
            stream_argv(&mut stream, action_argv(action, name));
        } else {
            respond(&mut stream, 200, "application/json", &run_action_json(action, name).to_string(), &[]);
        }
        return Ok(());
    }

    if path.starts_with("/mcp") {
        if method != "POST" {
            return respond(&mut stream, 405, "application/json", "{\"error\":\"POST only\"}", &[("Allow", "POST")]);
        }
        let body = read_body(&mut reader, length);
        let reply = mcp_reply(&body);
        if reply.is_empty() {
            return respond(&mut stream, 204, "application/json", "", &[]);
        }
        return respond(&mut stream, 200, "application/json", &reply, &[]);
    }

    if path.starts_with("/tools.json") {
        return respond(&mut stream, 200, "application/json", &tools_reply(), &[]);
    }

    if path.starts_with("/inspect") {
        let target = query_param(&path, "path");
        return respond(
            &mut stream,
            200,
            "application/json",
            &inspect_json(Path::new("."), &target).to_string(),
            &[],
        );
    }

    if path.starts_with("/fleet.json") {
        return respond(&mut stream, 200, "application/json", &fleet_json(), &[]);
    }

    if path.starts_with("/fit/sweep") {
        let dir = query_param(&path, "dir");
        let vram = query_param(&path, "vram")
            .parse::<f64>()
            .ok()
            .map(|g| (g * 1_073_741_824.0) as u64);
        let ctx = query_param(&path, "ctx").parse::<u64>().ok().unwrap_or(8192).max(128);
        let precision = jhol_ai::gguf::KvPrecision::parse(&query_param(&path, "kv"))
            .unwrap_or_default();
        let reports = crate::status_view::fit_sweep(
            Path::new(if dir.is_empty() { "." } else { &dir }),
            vram,
            ctx,
            1,
            precision,
        );
        let rows: Vec<serde_json::Value> = reports
            .iter()
            .map(|r| {
                serde_json::json!({
                    "target": r.target,
                    "name": r.target.rsplit('/').next().unwrap_or(&r.target),
                    "weights": r.weights,
                    "total": r.total,
                    "fits": r.fits,
                    "max_context": r.max_context,
                })
            })
            .collect();
        return respond(
            &mut stream,
            200,
            "application/json",
            &serde_json::json!({ "models": rows }).to_string(),
            &[],
        );
    }

    if path.starts_with("/fit") {
        let target = query_param(&path, "path");
        let vram = query_param(&path, "vram")
            .parse::<f64>()
            .ok()
            .map(|g| (g * 1_073_741_824.0) as u64);
        let ctx = query_param(&path, "ctx").parse::<u64>().unwrap_or(8192).max(128);
        let body = match crate::status_view::fit_target(Path::new("."), &target, vram, ctx, 1) {
            Ok(r) => serde_json::json!({
                "target": r.target,
                "weights": r.weights,
                "kv_cache": r.kv,
                "overhead": r.overhead,
                "total": r.total,
                "context": r.ctx,
                "vram": r.vram,
                "fits": r.fits,
                "headroom": r.headroom,
                "layers": r.meta.layers,
                "heads": r.meta.heads,
                "kv_heads": r.meta.kv_heads,
                "head_dim": r.meta.head_dim,
                "native_context": r.meta.context,
                "vocab": r.meta.vocab,
                "missing_meta": r.missing_meta,
                "kv_precision": r.kv_precision.label(),
                "max_context": r.max_context,
                "quants": r.quants.iter().map(|q| serde_json::json!({
                    "name": q.name, "weights": q.weights, "total": q.total, "fits": q.fits,
                    "max_context": q.max_context,
                })).collect::<Vec<_>>(),
                "recommend": r.recommend,
                "bytes_read": r.bytes_read,
                "requests": r.requests,
            }),
            Err(e) => serde_json::json!({ "error": e }),
        };
        return respond(&mut stream, 200, "application/json", &body.to_string(), &[]);
    }

    if path.starts_with("/packages.json") {
        return respond(
            &mut stream,
            200,
            "application/json",
            &packages_json(shared),
            &[],
        );
    }

    if path.starts_with("/why") {
        let pkg = query_param(&path, "pkg");
        let report = jhol_core::why::explain(Path::new("."), &pkg);
        let body = match report {
            Some(r) => serde_json::json!({ "package": pkg, "text": r.render() }),
            None => serde_json::json!({ "package": pkg, "text": format!("{pkg}: not in the tree") }),
        };
        return respond(&mut stream, 200, "application/json", &body.to_string(), &[]);
    }

    if path.starts_with("/plan") {
        let action = query_param(&path, "action");
        let status = cached_status(shared);
        let preview = jhol_core::status::action_preview(&action, &status);
        return respond(
            &mut stream,
            200,
            "application/json",
            &serde_json::to_string(&preview).unwrap_or_else(|e| e.to_string()),
            &[],
        );
    }

    if path.starts_with("/status.json") {
        let tag = jhol_core::status::fingerprint(Path::new("."));
        if if_none_match.as_deref() == Some(tag.as_str()) {
            let _ = write!(
                stream,
                "HTTP/1.1 304 Not Modified\r\nETag: {tag}\r\n{}\r\n",
                SECURITY_HEADERS
            );
            return Ok(());
        }
        let body = status_json(shared, &tag);
        return respond(&mut stream, 200, "application/json", &body, &[("ETag", &tag)]);
    }

    if path == "/" || path.starts_with("/?") {
        return respond(&mut stream, 200, "text/html; charset=utf-8", &page_html(), &[]);
    }

    respond(&mut stream, 404, "application/json", "{\"error\":\"not found\"}", &[])
}

/// Collected into the status once, and reused while the fingerprint holds.
fn status_json(shared: &Shared, tag: &str) -> String {
    let mut cache = shared.cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.0 == tag && !cache.1.is_empty() {
        return cache.1.clone();
    }
    let status = crate::status_view::enrich(jhol_core::status::collect(Path::new(".")), Path::new("."));
    let body = serde_json::to_string(&status).unwrap_or_else(|e| e.to_string());
    *cache = (tag.to_string(), body.clone(), Some(status));
    body
}

/// Pinned weights across the project: what is on disk, what is shared, what is missing.
fn fleet_json() -> String {
    let root = Path::new(".");
    let lock = jhol_core::model_lock::ModelLock::load(root).unwrap_or_default();
    let mut models = Vec::new();
    let mut digests: HashMap<String, Vec<String>> = HashMap::new();
    for (name, entry) in &lock.models {
        let mut total = 0u64;
        let mut missing = 0usize;
        for file in &entry.files {
            total += file.size;
            if !root.join(&entry.dir).join(&file.path).is_file() {
                missing += 1;
            }
            digests
                .entry(file.sha256.clone())
                .or_default()
                .push(name.clone());
        }
        models.push(serde_json::json!({
            "name": name,
            "source": entry.source,
            "revision": entry.revision,
            "bytes": total,
            "files": entry.files.len(),
            "missing": missing,
            "sharded": entry.files.len() > 1,
        }));
    }
    models.sort_by(|a, b| b["bytes"].as_u64().cmp(&a["bytes"].as_u64()));
    let shared_files = digests.values().filter(|v| v.len() > 1).count();
    serde_json::json!({
        "models": models,
        "total_bytes": models.iter().filter_map(|m| m["bytes"].as_u64()).sum::<u64>(),
        "shared_files": shared_files,
    })
    .to_string()
}

fn dir_bytes(path: &Path, cap: usize) -> u64 {
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > cap {
                return total;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

fn packages_json(shared: &Shared) -> String {
    let tag = jhol_core::status::fingerprint(Path::new("."));
    {
        let cache = shared.packages.lock().unwrap_or_else(|e| e.into_inner());
        if cache.0 == tag && !cache.1.is_empty() {
            return cache.1.clone();
        }
    }
    let root = Path::new(".");
    let resolved = jhol_core::lockfile::read_resolved_from_dir(root).unwrap_or_default();
    let direct = jhol_core::lockfile::registry_only_deps(&root.join("package.json"))
        .unwrap_or_default();
    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(resolved.len());
    let mut total = 0u64;
    for (name, version) in &resolved {
        let dir = root.join("node_modules").join(name);
        let bytes = dir_bytes(&dir, 8192);
        total += bytes;
        let license = std::fs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| {
                v.get("license")
                    .and_then(|l| l.as_str().map(str::to_string))
                    .or_else(|| {
                        v.get("licenses")
                            .and_then(|l| l.as_array())
                            .and_then(|a| a.first())
                            .and_then(|f| f.get("type"))
                            .and_then(|t| t.as_str())
                            .map(str::to_string)
                    })
            })
            .unwrap_or_default();
        rows.push(serde_json::json!({
            "name": name,
            "version": version,
            "bytes": bytes,
            "license": license,
            "direct": direct.contains_key(name),
            "present": dir.is_dir(),
        }));
    }
    rows.sort_by(|a, b| {
        b["bytes"].as_u64().cmp(&a["bytes"].as_u64())
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });
    let body = serde_json::json!({
        "tree": jhol_core::count_installed_packages(root),
        "total_bytes": total,
        "packages": rows,
    })
    .to_string();
    let mut cache = shared.packages.lock().unwrap_or_else(|e| e.into_inner());
    *cache = (tag, body.clone());
    body
}

fn cached_status(shared: &Shared) -> jhol_core::status::Status {
    let tag = jhol_core::status::fingerprint(Path::new("."));
    let _ = status_json(shared, &tag);
    shared
        .cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .2
        .clone()
        .unwrap_or_default()
}

const SECURITY_HEADERS: &str = "\
X-Content-Type-Options: nosniff\r\n\
Referrer-Policy: no-referrer\r\n\
X-Frame-Options: DENY\r\n\
Content-Security-Policy: default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:\r\n";

fn respond(
    stream: &mut TcpStream,
    code: u16,
    kind: &str,
    body: &str,
    extra: &[(&str, &str)],
) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let mut head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\n\
         Content-Length: {}\r\nConnection: close\r\n{SECURITY_HEADERS}",
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn read_headers(reader: &mut BufReader<TcpStream>) -> std::io::Result<(Vec<(String, String)>, usize)> {
    let mut out = Vec::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok((out, length))
}

fn read_body(reader: &mut BufReader<TcpStream>, length: usize) -> Vec<u8> {
    let mut raw = vec![0u8; length.min(MAX_BODY)];
    let _ = reader.read_exact(&mut raw);
    raw
}

/// One-shot session token. A page on any other origin can reach `127.0.0.1`, so every write
/// (and every inspect, which reads files) requires this.
fn session_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

/// Compare without an early exit on the first differing byte.
fn token_matches(given: &str, want: &str) -> bool {
    let (a, b) = (given.as_bytes(), want.as_bytes());
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn query_param(path: &str, key: &str) -> String {
    let Some(query) = path.split_once('?').map(|(_, q)| q) else {
        return String::new();
    };
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return percent_decode(v);
            }
        }
    }
    String::new()
}

fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// A fork name becomes a CLI argument, so it is validated here rather than trusted.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.contains("..")
}

/// The whole write surface: a whitelist of CLI invocations, never a shell.
fn action_argv(action: &str, name: &str) -> Option<Vec<String>> {
    let fixed: &[&str] = match action {
        "verify" => &["verify"],
        "prune" => &["verify", "--prune"],
        "install" => &["install"],
        "ci" => &["ci"],
        "policy" => &["install", "--exclude-vuln"],
        "fork" | "promote" if valid_name(name) => {
            let verb = if action == "fork" { "fork" } else { "promote" };
            return Some(vec!["env".into(), verb.into(), name.to_string()]);
        }
        _ => return None,
    };
    Some(fixed.iter().map(|s| s.to_string()).collect())
}

fn run_action_json(action: &str, name: &str) -> serde_json::Value {
    let Some(argv) = action_argv(action, name) else {
        return serde_json::json!({ "ok": false, "code": 2, "stderr": "unknown action" });
    };
    let exe = std::env::current_exe().unwrap_or_else(|_| "jhol".into());
    match std::process::Command::new(exe).args(&argv).output() {
        Ok(o) => serde_json::json!({
            "ok": o.status.success(),
            "code": o.status.code().unwrap_or(-1),
            "stdout": tail(&o.stdout, 8192),
            "stderr": tail(&o.stderr, 4096),
        }),
        Err(e) => serde_json::json!({ "ok": false, "code": -1, "stderr": e.to_string() }),
    }
}

fn tail(bytes: &[u8], keep: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s = s.trim_end();
    let start = s.len().saturating_sub(keep);
    s[start..].to_string()
}

fn stream_argv(stream: &mut TcpStream, argv: Option<Vec<String>>) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Cache-Control: no-store\r\nTransfer-Encoding: chunked\r\n\
         Connection: close\r\n{SECURITY_HEADERS}\r\n"
    );
    let chunk = |stream: &mut TcpStream, text: &str| {
        if text.is_empty() {
            return;
        }
        let _ = write!(stream, "{:X}\r\n{text}\r\n", text.len());
        let _ = stream.flush();
    };
    let Some(argv) = argv else {
        chunk(stream, "unknown action\n");
        let _ = write!(stream, "0\r\n\r\n");
        return;
    };
    let exe = std::env::current_exe().unwrap_or_else(|_| "jhol".into());
    let spawned = std::process::Command::new(exe)
        .args(&argv)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            chunk(stream, &format!("could not start: {e}\n"));
            let _ = write!(stream, "0\r\n\r\n");
            return;
        }
    };
    let stderr = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = e.read_to_end(&mut buf);
            buf
        })
    });
    if let Some(mut out) = child.stdout.take() {
        let mut buf = [0u8; 4096];
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => chunk(stream, &String::from_utf8_lossy(&buf[..n])),
            }
        }
    }
    let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
    if let Some(handle) = stderr {
        let text = tail(&handle.join().unwrap_or_default(), 4096);
        if !text.is_empty() {
            chunk(stream, &text);
        }
    }
    chunk(stream, &format!("\n[exit {code}]\n"));
    let _ = write!(stream, "0\r\n\r\n");
    let _ = stream.flush();
}

/// `jhol inspect`, in-process: a header read is cheaper than a process, and a remote target
/// costs one ranged request per probe step.
fn inspect_json(root: &Path, target: &str) -> serde_json::Value {
    let mut target = target.to_string();
    if let Some((org, repo)) = crate::status_view::repo_ref_parts(&target) {
        let files = match cached_repo_weights(&org, &repo) {
            Ok(f) => f,
            Err(e) => return serde_json::json!({ "error": e }),
        };
        if files.len() != 1 {
            return serde_json::json!({
                "repository": target,
                "files": files.iter().map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
            });
        }
        target = format!("hf://{org}/{repo}/{}", files[0].0);
    }
    if let Some(resolved) = crate::status_view::resolve_repo(&target) {
        match resolved {
            Ok(crate::status_view::RepoTarget::File(url)) => target = url,
            Ok(crate::status_view::RepoTarget::Listing(files)) => {
                return serde_json::json!({
                    "repository": target,
                    "files": files.iter().map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
                })
            }
            Err(e) => return serde_json::json!({ "error": e }),
        }
    }
    let r = match crate::status_view::inspect_target(root, &target) {
        Ok(i) => i,
        Err(e) => return serde_json::json!({ "error": e }),
    };
    let host = jhol_core::axes::HostAxes::detect();
    serde_json::json!({
        "path": r.display,
        "format": r.format,
        "architecture": r.architecture,
        "tensors": r.tensors,
        "params": r.params,
        "data_bytes": r.data_bytes,
        "vram_bytes": r.vram_bytes,
        "header_bytes": r.header_bytes,
        "bytes_read": r.bytes_read,
        "requests": r.requests,
        "dtypes": r.dtypes.iter().map(|(d, n)| serde_json::json!({"dtype": d, "tensors": n})).collect::<Vec<_>>(),
        "largest": r.largest.iter().map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
        "shards": r.shards.iter().map(|(n, b)| serde_json::json!({"name": n, "bytes": b})).collect::<Vec<_>>(),
        "sidecars": r.sidecars.iter().map(|(n, ok)| serde_json::json!({"name": n, "present": ok})).collect::<Vec<_>>(),
        "layers": r.meta.layers,
        "heads": r.meta.heads,
        "kv_heads": r.meta.kv_heads,
        "head_dim": r.meta.head_dim,
        "native_context": r.meta.context,
        "vocab": r.meta.vocab,
        "rope_theta": r.meta.rope_theta,
        "missing_meta": r.missing_meta,
        "gpu_mem_mb": host.gpu_mem_mb,
        "fits": host.gpu_mem_mb.map(|m| r.vram_bytes.div_ceil(1024 * 1024) <= m),
    })
}

/// The MCP bridge: an agent and a human drive one surface over JSON-RPC, in-process.
fn mcp_reply(body: &[u8]) -> String {
    let req: jhol_core::mcp::JsonRpcRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({
                "jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": format!("parse error: {e}")},
            })
            .to_string()
        }
    };
    match jhol_core::mcp::handle_mcp_request(&req) {
        Some(resp) => serde_json::to_string(&resp).unwrap_or_else(|e| e.to_string()),
        None => String::new(),
    }
}

fn tools_reply() -> String {
    mcp_reply(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
            .to_string()
            .as_bytes(),
    )
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Repository listings change rarely; a browser click should not pay for the API each time.
static REPO_CACHE: Mutex<Option<HashMap<(String, String), (Instant, Vec<(String, u64)>)>>> =
    Mutex::new(None);
const REPO_TTL: Duration = Duration::from_secs(300);

fn cached_repo_weights(org: &str, repo: &str) -> Result<Vec<(String, u64)>, String> {
    let key = (org.to_string(), repo.to_string());
    {
        let guard = REPO_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(map) = guard.as_ref() {
            if let Some((at, files)) = map.get(&key) {
                if at.elapsed() < REPO_TTL {
                    return Ok(files.clone());
                }
            }
        }
    }
    let files = crate::status_view::repo_weights(org, repo)?;
    let mut guard = REPO_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(HashMap::new)
        .insert(key, (Instant::now(), files.clone()));
    Ok(files)
}

fn page_html() -> String {
    PAGE.to_string()
}

const PAGE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>jhol</title>
<style>
 :root{
   color-scheme:dark;
   --bg:#0b0c0e; --card:#12141a; --card2:#171a21; --line:#232733; --line2:#2f3542;
   --ink:#e9ecf2; --mut:#98a0ae; --dim:#6b7280;
   --ok:#62c98a; --bad:#e08a8a; --warn:#d8bb74; --acc:#7aa7e0;
   --sans:-apple-system,BlinkMacSystemFont,"Inter","Segoe UI",system-ui,sans-serif;
   --mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,monospace;
 }
 @media (prefers-color-scheme:light){
   :root{ --bg:#f7f7f5; --card:#fff; --card2:#f1f1ee; --line:#e3e2dd; --line2:#d5d4ce;
          --ink:#191a1d; --mut:#5b6169; --dim:#8a8f97; --ok:#2f7d52; --bad:#a03f3f;
          --warn:#7d6119; --acc:#2f5f99; color-scheme:light }
 }
 *{box-sizing:border-box}
 html,body{margin:0}
 body{background:var(--bg);color:var(--ink);font:14px/1.55 var(--sans);
      -webkit-font-smoothing:antialiased}
 ::selection{background:var(--acc);color:var(--bg)}
 a{color:var(--acc);text-decoration:none} a:hover{text-decoration:underline}
 .sr-only{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);border:0}
 :focus-visible{outline:2px solid var(--acc);outline-offset:2px;border-radius:4px}
 .shell{max-width:1180px;margin:0 auto;padding:20px 22px 64px}

 /* ── top bar ───────────────────────────────────────────── */
 .top{display:flex;align-items:center;gap:14px;flex-wrap:wrap;padding:6px 0 16px}
 .logo{font:600 14px/1 var(--mono);letter-spacing:-.02em;padding:6px 9px;
       border:1px solid var(--line2);border-radius:7px;background:var(--card2)}
 .who{display:flex;align-items:baseline;gap:8px;min-width:0}
 .who b{font-size:15px;font-weight:600;overflow:hidden;text-overflow:ellipsis;
        white-space:nowrap;max-width:46ch}
 .who span{font:12px/1 var(--mono);color:var(--mut)}
 .rootpath{font:11.5px/1.4 var(--mono);color:var(--dim);max-width:26ch;overflow:hidden;
           text-overflow:ellipsis;white-space:nowrap}
 .pills{margin-left:auto;display:flex;gap:6px;flex-wrap:wrap;justify-content:flex-end}
 .pill{display:inline-flex;align-items:center;gap:6px;padding:4px 9px;border-radius:999px;
       font-size:11.5px;line-height:1.35;border:1px solid var(--line2);background:var(--card2);
       color:var(--mut);white-space:nowrap;max-width:100%}
 .pill i{width:6px;height:6px;border-radius:50%;background:currentColor;flex:none}
 .pill.ok{color:var(--ok);border-color:color-mix(in srgb, var(--ok) 35%, var(--line2))}
 .pill.bad{color:var(--bad);border-color:color-mix(in srgb, var(--bad) 35%, var(--line2))}
 .pill.warn{color:var(--warn);border-color:color-mix(in srgb, var(--warn) 35%, var(--line2))}

 /* ── tabs ──────────────────────────────────────────────── */
 .tabs{display:flex;gap:4px;flex-wrap:wrap;margin-bottom:18px}
 .tabs button{appearance:none;border:1px solid transparent;background:transparent;
   color:var(--mut);font:inherit;font-size:13px;padding:7px 11px;border-radius:8px;
   cursor:pointer;display:inline-flex;align-items:center;gap:7px;max-width:100%}
 .tabs button:hover{background:var(--card);color:var(--ink)}
 .tabs button.on{background:var(--card2);border-color:var(--line2);color:var(--ink)}
 .tabs .n{font:11px/1 var(--mono);color:var(--dim);background:var(--bg);
   border:1px solid var(--line);border-radius:5px;padding:2px 5px}
 .tabs .n:empty{display:none}
 .tabs button.on .n{color:var(--acc);border-color:var(--line2)}

 /* ── cards ─────────────────────────────────────────────── */
 section{display:none} section.on{display:block}
 .card{background:var(--card);border:1px solid var(--line);border-radius:12px;
       padding:16px 18px;min-width:0}
 .card+.card,.grid+.card{margin-top:14px}
 .grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,420px),1fr));
       gap:14px;align-items:start}
 h2{font:600 11.5px/1 var(--sans);letter-spacing:.08em;text-transform:uppercase;
    color:var(--dim);margin:0 0 13px}
 .kv{display:grid;grid-template-columns:minmax(72px,96px) minmax(0,1fr);gap:9px 14px;
     margin:0;align-items:baseline}
 .split{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:0 22px}
 @media (max-width:520px){ .split{grid-template-columns:minmax(0,1fr);gap:0} }
 .kv dt{color:var(--mut);font-size:12.5px}
 .kv dd{margin:0;min-width:0;overflow-wrap:anywhere}
 .mono{font-family:var(--mono);font-size:12.5px;font-variant-numeric:tabular-nums}
 .muted{color:var(--mut)} .dim{color:var(--dim)}
 .empty{color:var(--dim);font-size:13px;padding:10px 0}
 .note{color:var(--dim);font-size:12.5px;margin:12px 0 0;overflow-wrap:anywhere}
 .mark{display:inline-flex;align-items:center;gap:6px;white-space:nowrap}
 .mark::before{content:"";width:5px;height:5px;border-radius:50%;background:var(--dim);
   flex:none;transform:translateY(-1px)}
 .mark.ok{color:var(--ok)} .mark.ok::before{background:var(--ok)}
 .mark.bad{color:var(--bad)} .mark.bad::before{background:var(--bad)}
 .mark.warn{color:var(--warn)} .mark.warn::before{background:var(--warn)}

 /* ── rows (never tables: a fixed column cannot overflow if it is a grid) ── */
 .rowhead,.arow{display:grid;
   grid-template-columns:minmax(64px,78px) minmax(56px,74px) minmax(0,1fr) 74px 84px;
   gap:6px 14px;padding:11px 0;border-top:1px solid var(--line);align-items:baseline}
 .rowhead{color:var(--dim);font-size:11px;letter-spacing:.06em;text-transform:uppercase;
          padding-top:2px;border-top:0}
 .rowhead span:nth-child(4),.rowhead span:nth-child(5),
 .arow .num{text-align:right}
 .arow .num{font-family:var(--mono);font-size:12.5px;font-variant-numeric:tabular-nums;
            white-space:nowrap}
 .arow .name{font-family:var(--mono);font-size:12.5px;overflow-wrap:anywhere;min-width:0}
 .arow .kind{color:var(--dim);font-size:12px;white-space:nowrap;overflow:hidden;
             text-overflow:ellipsis}
 .arow .why{grid-column:1/-1;color:var(--mut);font-size:12.5px;overflow-wrap:anywhere}
 .arow .why span+span::before{content:" · ";color:var(--dim)}
 .arow .sub{color:var(--dim);font:11.5px/1.5 var(--mono);overflow-wrap:anywhere}
 .prow{display:grid;grid-template-columns:minmax(0,1.5fr) 96px 88px minmax(70px,.8fr) 58px;
   gap:6px 14px;padding:10px 0;border-top:1px solid var(--line);align-items:baseline}
 .prow .nm{font-family:var(--mono);font-size:12.5px;overflow-wrap:anywhere;min-width:0}
 .prow .v{font-family:var(--mono);font-size:12px;color:var(--mut);white-space:nowrap;
          font-variant-numeric:tabular-nums}
 .prow .num{text-align:right}
 .prow .why{text-align:right}
 @media (max-width:760px){
   .prow{grid-template-columns:minmax(0,1fr) 80px 62px}
   .prow .lic,.prow .ver{display:none}
 }
 .list{display:block}
 .lrow{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:4px 14px;
       padding:11px 0;border-top:1px solid var(--line);align-items:baseline}
 .lrow .t{font-family:var(--mono);font-size:12.5px;overflow-wrap:anywhere}
 .lrow .d{grid-column:1/-1;color:var(--mut);font-size:12.5px;overflow-wrap:anywhere}
 .lrow .v{font-family:var(--mono);font-size:12.5px;color:var(--mut);text-align:right;
          white-space:nowrap;font-variant-numeric:tabular-nums}

 /* ── controls ──────────────────────────────────────────── */
 input{background:var(--bg);border:1px solid var(--line2);color:var(--ink);
   border-radius:8px;padding:9px 11px;font:13px/1.4 var(--sans);width:100%;min-width:0;
   outline:none}
 input:focus{border-color:var(--acc)}
 input.mono{font-family:var(--mono);font-size:12.5px}
 .toolbar{display:flex;gap:10px;align-items:center;flex-wrap:wrap;margin-bottom:10px}
 .toolbar .grow{flex:1 1 240px;min-width:0}
 .btn{appearance:none;font:500 13px/1 var(--sans);color:var(--ink);background:var(--card2);
   border:1px solid var(--line2);border-radius:8px;padding:9px 14px;cursor:pointer;
   white-space:nowrap}
 .btn:hover{border-color:var(--acc)}
 .btn.primary{background:color-mix(in srgb, var(--acc) 18%, var(--card2));
   border-color:color-mix(in srgb, var(--acc) 45%, var(--line2))}
 .btn[disabled]{opacity:.5;cursor:progress}
 .check{display:inline-flex;align-items:center;gap:8px;color:var(--mut);font-size:12.5px;
        cursor:pointer;white-space:nowrap}
 .check input{width:auto;accent-color:var(--acc)}

 /* ── figures + bars ────────────────────────────────────── */
 .figures{display:flex;flex-wrap:wrap;gap:10px 34px;margin:0}
 .fig{display:flex;flex-direction:column;gap:2px;min-width:0}
 .fig b{font-size:19px;font-weight:600;font-variant-numeric:tabular-nums;letter-spacing:-.01em}
 .fig span{font-size:11.5px;color:var(--dim)}
 .bars{margin-top:14px;display:grid;gap:8px}
 .barrow{display:grid;grid-template-columns:minmax(0,1fr) minmax(60px,180px) 78px;
         gap:12px;align-items:center}
 .barrow .nm{font:12px/1.4 var(--mono);color:var(--mut);overflow:hidden;
             text-overflow:ellipsis;white-space:nowrap}
 .bar{height:6px;background:var(--card2);border-radius:99px;overflow:hidden}
 .bar>i{display:block;height:100%;background:var(--acc);opacity:.85;border-radius:99px}
 .barrow .v{text-align:right;font:12px/1 var(--mono);color:var(--mut);
            font-variant-numeric:tabular-nums;white-space:nowrap}
 .result{margin-top:16px;border-top:1px solid var(--line);padding-top:14px}
 .result .path{font:12px/1.5 var(--mono);color:var(--mut);overflow-wrap:anywhere}
 .fixrow{margin-top:12px;display:flex;flex-wrap:wrap;gap:8px;align-items:center}
 .fixrow .dim{white-space:nowrap}
 .copy{background:none;border:0;color:var(--dim);font:11.5px/1 var(--mono);cursor:pointer;
       padding:0 2px;border-bottom:1px dotted var(--line2)}
 .copy:hover{color:var(--acc)}
 .tag{display:inline-block;font:11px/1 var(--mono);color:var(--dim);
      border:1px solid var(--line2);border-radius:4px;padding:2px 5px;margin:0 4px 4px 0}
 .error{border-left:2px solid var(--bad);padding:8px 0 8px 12px;color:var(--bad);
        font:12.5px/1.6 var(--mono);overflow-wrap:anywhere;white-space:pre-wrap}
 pre{font:12.5px/1.6 var(--mono);color:var(--mut);white-space:pre-wrap;overflow:auto;
     max-height:24rem;margin:0;padding:12px 0 0;border-top:1px solid var(--line);
     overflow-wrap:anywhere}
 pre:empty{display:none}
 footer{color:var(--dim);font:11.5px/1.9 var(--mono);margin-top:26px;
        overflow-wrap:anywhere}
 footer .kbd{border:1px solid var(--line2);border-radius:3px;padding:1px 4px}

 @media (max-width:760px){
   .shell{padding:14px 14px 48px}
   .who b{max-width:60vw}
   .pills{display:none}
   .rowhead,.arow{grid-template-columns:70px minmax(0,1fr) 78px}
   .rowhead span:nth-child(2),.arow .kind{display:none}
   .rowhead span:nth-child(4),.arow .size{display:none}
   .barrow{grid-template-columns:minmax(0,1fr) 70px}
   .barrow .bar{display:none}
 }
</style></head><body>
<div class="shell">

 <header class="top">
   <div class="logo">jhol</div>
   <div class="who">
     <b id="name">…</b><span id="version"></span>
     <span id="root" class="rootpath" title=""></span>
   </div>
   <div class="pills" id="pills"></div>
 </header>

 <p id="stale" hidden style="margin:0 0 14px;padding:10px 12px;border-radius:8px;
    border:1px solid color-mix(in srgb, var(--warn) 45%, var(--line2));
    background:color-mix(in srgb, var(--warn) 12%, var(--card));color:var(--warn);
    font-size:12.5px"></p>

 <nav class="tabs" id="nav"></nav>

 <section id="s-status" class="on">
   <div class="card" id="onboard" hidden>
     <h2>No project here</h2>
     <p class="muted" style="margin:0 0 12px;max-width:62ch">
       This directory has no <span class="mono">package.json</span>, so there is nothing to verify
       yet. Run one of these in the terminal — the dashboard fills in on its own afterwards.</p>
     <dl class="kv">
       <dt>start one</dt><dd class="mono">jhol install express</dd>
       <dt>weights</dt><dd class="mono">jhol ai pull hf://org/repo/model.safetensors</dd>
       <dt>watch</dt><dd class="mono">jhol status --watch</dd>
     </dl>
   </div>
   <div class="grid status-block">
     <div class="card"><h2>Project</h2><dl class="kv" id="k-project"></dl>
       <div id="k-fix" class="fixrow"></div></div>
     <div class="card"><h2>Artefacts</h2><div id="k-artefacts"></div></div>
     <div class="card"><h2>Host</h2><dl class="kv" id="k-host"></dl></div>
     <div class="card"><h2>Policy</h2><dl class="kv" id="k-policy"></dl></div>
   </div>
 </section>

 <section id="s-artifacts">
   <div class="card">
     <div class="toolbar">
       <div class="grow"><label class="sr-only" for="q">Filter artefacts</label>
         <input id="q" placeholder="Filter by name, kind or reason" autocomplete="off"></div>
       <label class="check"><input type="checkbox" id="unfit"> Only what will not run</label>
     </div>
     <div class="rowhead"><span>Fit</span><span>Kind</span><span>Name</span>
       <span>Size</span><span>Resident</span></div>
     <div id="artifacts"></div>
   </div>
   <div class="card">
     <h2>Read a header</h2>
     <div class="toolbar">
       <div class="grow"><label class="sr-only" for="path">Weight file, model or URL</label>
         <input id="path" class="mono" placeholder="path · pinned model · hf://org/repo/file.gguf · huggingface.co URL"></div>
       <button class="btn primary" id="go" type="button">Inspect</button>
     </div>
     <div id="detail-host"></div>
     <p class="note">Reads the header only: parameters, dtypes and resident size without the weights.</p>
   </div>
 </section>

 <section id="s-packages">
   <div class="card">
     <div class="toolbar">
       <div class="grow"><label class="sr-only" for="pq">Filter packages</label>
         <input id="pq" placeholder="Filter packages" autocomplete="off"></div>
       <label class="check"><input type="checkbox" id="pdirect"> Direct only</label>
       <button class="btn" id="psort" type="button">By size</button>
     </div>
     <div class="rowhead"><span>Name</span><span>Version</span><span>On disk</span>
       <span style="text-align:right">License</span><span>Why</span></div>
     <div id="packages"><div class="empty">Reading the tree…</div></div>
     <p class="note" id="psummary"></p>
   </div>
   <div class="card" id="why-card" hidden>
     <h2>Why is this here?</h2>
     <pre id="why-text" style="border-top:0;padding-top:0"></pre>
     <div class="toolbar" style="margin-top:10px">
       <button class="btn" id="why-copy" type="button">Copy the command</button>
       <span class="dim mono" id="why-cmd" style="font-size:11.5px"></span>
     </div>
   </div>
 </section>

 <section id="s-fit">
   <div class="card">
     <h2>Will it run?</h2>
     <div class="toolbar">
       <div class="grow"><label class="sr-only" for="fitpath">Model to size</label>
         <input id="fitpath" class="mono" placeholder="hf://org/repo · path · pinned model">
       </div>
       <label class="check" for="fitvram">VRAM</label>
       <input id="fitvram" class="mono" value="24" style="width:70px" inputmode="decimal">
       <span class="dim mono" style="font-size:11.5px">GB</span>
       <label class="check" for="fitctx">context</label>
       <input id="fitctx" class="mono" value="8192" style="width:86px" inputmode="numeric">
       <label class="check" for="fitkv">kv</label>
       <select id="fitkv" class="mono" style="background:var(--bg);color:var(--ink);
         border:1px solid var(--line2);border-radius:8px;padding:8px 6px">
         <option value="f16">f16</option><option value="q8">q8_0</option><option value="q4">q4_0</option>
       </select>
       <button class="btn primary" id="fitgo" type="button">Size it</button>
     </div>
     <div id="fitout"></div>
     <div class="toolbar" style="margin-top:16px">
       <div class="grow"><label class="sr-only" for="fitdir">Directory to sweep</label>
         <input id="fitdir" class="mono" placeholder="sweep a directory of models…"></div>
       <button class="btn" id="fitsweep" type="button">Sweep</button>
     </div>
     <div id="sweepout"></div>
     <p class="note">weights from the header, KV cache from layers × kv-heads × head-dim,
       overhead a stated 15 % of weights. Nothing is downloaded beyond the header.</p>
   </div>
 </section>

 <section id="s-budget">
   <div class="card"><h2>Download plan</h2><div id="budget"></div></div>
   <div class="card"><h2>Forks</h2><div id="forks"></div></div>
 </section>

 <section id="s-proof">
   <div class="grid">
     <div class="card"><h2>Reproducibility</h2><dl class="kv" id="proof-kv"></dl></div>
     <div class="card"><h2>Attestation</h2><dl class="kv" id="proof-kv2"></dl></div>
   </div>
   <div class="card"><h2>Provenance</h2><div id="provenance"></div></div>
   <div class="card"><h2>Weights fleet</h2><div id="fleet"><div class="empty">reading…</div></div></div>
 </section>

 <section id="s-agent">
   <div class="card"><h2>Tools an agent can call</h2><div id="agent"></div>
     <div class="toolbar" style="margin-top:12px">
       <button class="btn" id="mcp-config" type="button">Copy MCP config</button>
       <span class="dim mono" id="mcp-hint" style="font-size:11.5px"></span>
     </div>
     <pre id="tool-out" style="min-height:0;max-height:18rem"></pre>
     <p class="note">Answered over <span class="mono">jhol mcp --stdio</span> and
     <span class="mono">POST /mcp</span> on this page.</p></div>
 </section>

 <section id="s-actions">
   <div class="card"><h2>Run</h2>
     <div class="list" id="acts"></div>
     <p class="note" id="arm-hint">Writes ask once: the first click states what would happen.</p>
     <pre id="out" role="log" aria-live="polite"></pre>
   </div>
 </section>

 <footer>
   same data as <span class="mono">jhol status --json</span> ·
   <span class="mono">jhol status --watch</span> ·
   MCP on <span class="mono">POST /mcp</span> ·
   <span class="kbd">1</span>–<span class="kbd">8</span> sections ·
   <span class="kbd">/</span> filter
 </footer>
</div>

<script>
const token = new URLSearchParams(location.search).get("t") || "";
const SECTIONS = ["status","packages","fit","artifacts","budget","proof","agent","actions"];
const LABEL = {status:"Status",packages:"Packages",fit:"Fit",artifacts:"Artefacts",
               budget:"Budget",proof:"Proof",agent:"Agent",actions:"Actions"};
const nav = document.getElementById("nav");
SECTIONS.forEach((s, i) => {
  const b = document.createElement("button");
  b.type = "button"; b.dataset.section = s;
  if (i === 0) b.className = "on";
  b.innerHTML = `${LABEL[s]}<span class="n" id="n-${s}"></span>`;
  b.onclick = () => show(s);
  nav.appendChild(b);
});
function show(s) {
  document.querySelectorAll("nav button").forEach(x => x.classList.toggle("on", x.dataset.section === s));
  document.querySelectorAll("section").forEach(x => x.classList.toggle("on", x.id === "s-" + s));
  if (s === "packages") loadPackages();
  if (s === "proof") loadFleet();
}

let packageRows = [];
let sortBySize = true;

function renderPackages() {
  const q = (document.getElementById("pq").value || "").toLowerCase();
  const directOnly = document.getElementById("pdirect").checked;
  const rows = packageRows
    .filter(p => (!q || p.name.toLowerCase().includes(q)) && (!directOnly || p.direct))
    .sort((a, b) => sortBySize ? b.bytes - a.bytes : a.name.localeCompare(b.name));
  document.getElementById("packages").innerHTML = rows.map(p => `
    <div class="prow">
      <span class="nm">${esc(p.name)}${p.direct ? " <span class=\"dim\" style=\"font-size:11px\">direct</span>" : ""}${p.present ? "" : " <span style=\"color:var(--bad);font-size:11px\">missing</span>"}</span>
      <span class="v ver">${esc(p.version)}</span>
      <span class="v num">${bytes(p.bytes)}</span>
      <span class="v lic">${esc(p.license || "—")}</span>
      <span class="why"><button class="copy" data-why="${esc(p.name)}" type="button">why</button></span>
    </div>`).join("") || `<div class="empty">Nothing matches.</div>`;
  document.getElementById("psummary").textContent =
    `${rows.length} of ${packageRows.length} packages · ${bytes(packageRows.reduce((n, p) => n + p.bytes, 0))} on disk`;
  document.querySelectorAll("button[data-why]").forEach(b => {
    b.onclick = () => explainPackage(b.dataset.why);
  });
  document.getElementById("psort").textContent = sortBySize ? "By size" : "By name";
}

async function loadFleet() {
  const el = document.getElementById("fleet");
  const j = await jsonOrError("/fleet.json");
  if (j.error) { el.innerHTML = `<div class="empty">${esc(j.error)}</div>`; return; }
  if (!(j.models || []).length) {
    el.innerHTML = `<div class="empty">No weights pinned. <span class="mono">jhol ai pin &lt;dir&gt;</span>
      records digests so the fleet shows up here.</div>`;
    return;
  }
  el.innerHTML = j.models.map(m => `<div class="lrow">
      <span class="t">${esc(m.name)}${m.sharded ? ` <span class="dim" style="font-size:11px">${m.files} shards</span>` : ""}</span>
      <span class="v">${bytes(m.bytes)}</span>
      <span class="d">${esc(m.source)} @ <span class="mono">${esc((m.revision || "").slice(0, 12))}</span>
        ${m.missing ? `· <span style="color:var(--bad)">${m.missing} file(s) missing</span>` : "· all files present"}</span>
    </div>`).join("") +
    `<p class="note">${bytes(j.total_bytes)} pinned across ${j.models.length} model(s)` +
    (j.shared_files ? ` · ${j.shared_files} file(s) share a digest` : "") + `</p>`;
}

async function loadPackages() {
  if (packageRows.length) { renderPackages(); return; }
  const el = document.getElementById("packages");
  const j = await jsonOrError("/packages.json");
  if (j.error) { el.innerHTML = `<div class="empty">${esc(j.error)}</div>`; return; }
  packageRows = j.packages || [];
  renderPackages();
}

async function explainPackage(name) {
  const card = document.getElementById("why-card");
  card.hidden = false;
  const text = document.getElementById("why-text");
  text.textContent = "reading the graph…";
  const j = await jsonOrError(`/why?pkg=${encodeURIComponent(name)}`);
  text.textContent = j.error ? j.error : j.text;
  const cmd = `jhol why ${name}`;
  document.getElementById("why-cmd").textContent = cmd;
  document.getElementById("why-copy").onclick = async () => {
    try { await navigator.clipboard.writeText(cmd); } catch (e) {}
    card.scrollIntoView({behavior: "smooth", block: "nearest"});
  };
  card.scrollIntoView({behavior: "smooth", block: "nearest"});
}

const FIT_KEY = "jhol.fit";
function saveFitControls() {
  try {
    localStorage.setItem(FIT_KEY, JSON.stringify({
      vram: document.getElementById("fitvram").value,
      ctx: document.getElementById("fitctx").value,
      kv: document.getElementById("fitkv").value,
      dir: document.getElementById("fitdir").value,
    }));
  } catch (e) {}
}
function restoreFitControls() {
  try {
    const s = JSON.parse(localStorage.getItem(FIT_KEY) || "{}");
    if (s.vram) document.getElementById("fitvram").value = s.vram;
    if (s.ctx) document.getElementById("fitctx").value = s.ctx;
    if (s.kv) document.getElementById("fitkv").value = s.kv;
    if (s.dir) document.getElementById("fitdir").value = s.dir;
  } catch (e) {}
}

async function sweep() {
  const dir = document.getElementById("fitdir").value.trim();
  const vram = parseFloat(document.getElementById("fitvram").value) || 0;
  const ctx = parseInt(document.getElementById("fitctx").value, 10) || 8192;
  const kv = document.getElementById("fitkv").value;
  const out = document.getElementById("sweepout");
  out.innerHTML = `<div class="empty">Sizing every model…</div>`;
  const j = await jsonOrError(
    `/fit/sweep?dir=${encodeURIComponent(dir)}&vram=${vram || ""}&ctx=${ctx}&kv=${kv}`);
  if (j.error) { out.innerHTML = `<div class="error">${esc(j.error)}</div>`; return; }
  if (!(j.models || []).length) {
    out.innerHTML = `<div class="empty">No .gguf or .safetensors files there.</div>`;
    return;
  }
  out.innerHTML = `<div class="rowhead"><span style="grid-column:1/4">Model</span>
      <span class="num">Total</span><span style="text-align:right">Verdict</span></div>` +
    j.models.map(m => `<div class="lrow">
      <span class="t">${esc(m.name)}</span>
      <span class="v">${bytes(m.total)}</span>
      <span class="d" style="text-align:right;color:${m.fits === false ? "var(--bad)" : m.fits === true ? "var(--ok)" : "var(--dim)"}">
        ${m.fits === true ? "fits" : m.fits === false ? "too big" : "—"}
        ${m.max_context ? ` · up to ${m.max_context} tokens` : ""}</span>
    </div>`).join("");
}

async function runFit() {
  const target = document.getElementById("fitpath").value.trim();
  const vram = parseFloat(document.getElementById("fitvram").value) || 0;
  const ctx = parseInt(document.getElementById("fitctx").value, 10) || 8192;
  const kv = document.getElementById("fitkv").value;
  saveFitControls();
  const out = document.getElementById("fitout");
  if (!target) { out.innerHTML = `<div class="empty">Give a model: a repo, a path, or a pinned name.</div>`; return; }
  out.innerHTML = `<div class="empty">Sizing from the header…</div>`;
  const j = await jsonOrError(
    `/fit?path=${encodeURIComponent(target)}&vram=${vram || ""}&ctx=${ctx}&kv=${kv}`);
  if (j.error) { out.innerHTML = `<div class="result"><div class="error">${esc(j.error)}</div></div>`; return; }
  const kvUnknown = !j.kv_cache;
  out.innerHTML = `<div class="result">
    <div class="path">${esc(j.target)}</div>
    ${j.layers ? `<p class="note" style="margin:8px 0 0">${j.layers} layers ·
      ${j.heads} heads (${j.kv_heads ?? j.heads} kv) · head_dim ${j.head_dim ?? "?"} ·
      native context ${j.native_context ?? "?"}</p>` : ""}
    <div class="figures" style="margin-top:12px">
      <div class="fig"><b>${bytes(j.weights)}</b><span>weights</span></div>
      <div class="fig"><b>${kvUnknown ? "—" : bytes(j.kv_cache)}</b>
        <span>kv cache · ${j.context} tokens · ${esc(j.kv_precision || "f16")}</span></div>
      <div class="fig"><b>${bytes(j.overhead)}</b><span>overhead · 15%</span></div>
      <div class="fig"><b>${bytes(j.total)}</b><span>total</span></div>
      <div class="fig"><b>${j.max_context ? j.max_context : "—"}</b>
        <span>tokens fit at this budget</span></div>
      <div class="fig"><b style="color:${j.fits === true ? "var(--ok)" : j.fits === false ? "var(--bad)" : "var(--warn)"}">
        ${j.fits === true ? "fits" : j.fits === false ? "does not fit" : "no budget"}</b>
        <span>${j.headroom != null ? (j.headroom >= 0 ? bytes(j.headroom) + " spare" : bytes(-j.headroom) + " over") : "vs " + bytes(j.vram || 0)}</span></div>
    </div>
    ${kvUnknown && (j.missing_meta || []).length ? `<p class="note">KV cache unknown: the header
      lacks <span class="mono">${esc(j.missing_meta.join(", "))}</span> — not guessed.</p>` : ""}
    ${(j.quants || []).length ? `<div style="margin-top:16px">
      <div class="rowhead"><span style="grid-column:1/3">Quant</span>
        <span>Total</span><span style="grid-column:5;text-align:right">Verdict</span></div>
      ${j.quants.map(q => `<div class="lrow"><span class="t">${esc(q.name)}</span>
        <span class="v">${bytes(q.total)}${q.max_context ? ` · ${q.max_context} tok` : ""}</span>
        <span class="d" style="text-align:right;color:${q.fits === false ? "var(--bad)" : q.fits === true ? "var(--ok)" : "var(--dim)"}">
          ${q.fits === true ? "fits" : q.fits === false ? "too big" : "—"}</span></div>`).join("")}
      ${j.recommend ? `<p class="note" style="color:var(--ok)">Recommend
        <span class="mono">${esc(j.recommend)}</span> — the largest quant that still fits.</p>` : ""}
    </div>` : ""}
  </div>`;
}

document.addEventListener("DOMContentLoaded", () => {
  restoreFitControls();
  document.getElementById("fitgo").onclick = runFit;
  document.getElementById("fitsweep").onclick = sweep;
  ["fitvram", "fitctx", "fitkv", "fitdir"].forEach(id => {
    document.getElementById(id).addEventListener("change", saveFitControls);
  });
  document.getElementById("fitpath").addEventListener("keydown",
    e => { if (e.key === "Enter") runFit(); });
  document.getElementById("pq").oninput = renderPackages;
  document.getElementById("pdirect").onchange = renderPackages;
  document.getElementById("psort").onclick = () => { sortBySize = !sortBySize; renderPackages(); };
});
document.addEventListener("keydown", e => {
  if (e.target.tagName === "INPUT") { if (e.key === "Escape") e.target.blur(); return; }
  const i = parseInt(e.key, 10);
  if (i >= 1 && i <= SECTIONS.length) show(SECTIONS[i - 1]);
  const active = SECTIONS.findIndex(s => document.getElementById("s-" + s).classList.contains("on"));
  if (e.key === "ArrowRight") { e.preventDefault(); show(SECTIONS[(active + 1) % SECTIONS.length]); }
  if (e.key === "ArrowLeft") { e.preventDefault(); show(SECTIONS[(active - 1 + SECTIONS.length) % SECTIONS.length]); }
  if (e.key === "/") { e.preventDefault(); show("artifacts"); document.getElementById("q").focus(); }
});

const esc = s => String(s ?? "").replace(/[&<>"]/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
function bytes(n) {
  if (!n) return "—";
  const u = ["B","KB","MB","GB","TB"]; let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${i === 0 ? v : v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${u[i]}`;
}
function params(n) {
  if (!n) return "—";
  if (n >= 1e9) return `${(n/1e9).toFixed(2)} B`;
  if (n >= 1e6) return `${(n/1e6).toFixed(1)} M`;
  if (n >= 1e3) return `${(n/1e3).toFixed(1)} K`;
  return String(n);
}
const mark = (k, t) => `<span class="mark ${k}">${esc(t)}</span>`;
const fitMark = f => f === true ? mark("ok","fits")
  : f === false ? mark("bad","will not run") : mark("warn","unknown");
const pill = (k, t) => `<span class="pill ${k}"><i></i>${esc(t)}</span>`;
function kv(el, rows) {
  el.innerHTML = rows.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${v}</dd>`).join("");
}

function render(s) {
  const hasManifest = !!s.has_manifest;
  document.getElementById("onboard").hidden = hasManifest;
  document.querySelectorAll(".status-block").forEach(el => { el.hidden = !hasManifest; });
  document.getElementById("name").textContent = hasManifest ? (s.project || "(unnamed)") : "no project";
  document.getElementById("version").textContent = s.version || "";
  const rootEl = document.getElementById("root");
  const segs = (s.root || "").split("/").filter(Boolean);
  rootEl.textContent = segs.length ? "…/" + segs.slice(-2).join("/") : "";
  rootEl.title = s.root || "";

  const unfit = s.artifacts.filter(a => a.fits === false).length;
  const unknown = s.artifacts.filter(a => a.fits === null || a.fits === undefined).length;
  const total = s.artifacts.length;

  document.getElementById("pills").innerHTML = !hasManifest ? "" : [
    s.tree.state === "match" ? pill("ok", `tree match · ${s.tree.packages}`) : pill("bad", `tree ${s.tree.state}`),
    s.manifest_matches_lockfile ? pill("ok","lockfile current") : pill("bad","lockfile drifted"),
    s.plan.missing_bytes ? pill("warn", `${bytes(s.plan.missing_bytes)} to fetch`) : pill("ok","nothing to fetch"),
    unfit ? pill("bad", `${unfit} will not run`) : pill("ok","all fit"),
    s.forks.length ? pill("", `${s.forks.length} fork${s.forks.length>1?"s":""}`) : "",
  ].join("");

  document.getElementById("n-status").textContent = hasManifest ? s.tree.packages : "";
  document.getElementById("n-packages").textContent = hasManifest ? s.tree.packages : "";
  document.getElementById("n-artifacts").textContent = total || "";
  document.getElementById("n-budget").textContent = s.plan.items.filter(i => !i.present).length || "";
  document.getElementById("n-proof").textContent = s.proof.attested_packages || "";
  document.getElementById("n-agent").textContent = s.agent.tools.length;
  document.getElementById("n-actions").textContent = "";
  if (!hasManifest) { renderArtifacts(s); return; }

  kv(document.getElementById("k-project"), [
    ["tree", s.tree.state === "match" ? mark("ok", `match — ${s.tree.packages} packages`)
      : `${mark("bad", s.tree.state)} <span class="muted">${esc(s.tree.detail)}</span>`],
    ["lockfile", s.manifest_matches_lockfile ? mark("ok","current") : mark("bad","drifted")],
    ["snapshot", s.proof.snapshot_ready ? mark("ok","ready") : mark("warn","absent")],
    ["attested", `<span class="mono">${s.proof.attested_packages}</span> packages ·
                  <span class="mono">${s.proof.attested_models}</span> models`],
  ]);

  const fix = document.getElementById("k-fix");
  if (hasManifest && s.tree.state !== "match") {
    const want = s.tree.state === "extraneous"
      ? { a: "prune", label: "Remove what is not locked", why: "verify --prune" }
      : { a: "install", label: "Bring the tree back to the lockfile", why: "install" };
    fix.innerHTML = `<button class="btn" data-fix="${want.a}" type="button">${esc(want.label)}</button>
      <span class="dim mono" style="font-size:11.5px;margin-left:8px">jhol ${esc(want.why)}</span>`;
    fix.querySelector("[data-fix]").onclick = (e) => {
      armOrRun(want.a, "", true, "jhol " + want.why, e.currentTarget);
    };
    fix.dataset.action = want.a;
  } else {
    fix.innerHTML = "";
  }

  document.getElementById("k-artefacts").innerHTML = total ? `
    <div class="figures">
      <div class="fig"><b>${total}</b><span>pinned</span></div>
      <div class="fig"><b style="color:var(--ok)">${total-unfit-unknown}</b><span>fit</span></div>
      <div class="fig"><b style="color:var(--bad)">${unfit}</b><span>will not run</span></div>
      <div class="fig"><b style="color:var(--warn)">${unknown}</b><span>unknown</span></div>
    </div>
    ${unknown ? `<p class="note">Unknown means undecidable — no GPU to compare against, or a wheel
      with no platform tag. Never counted as a pass.</p>` : ""}`
    : `<div class="empty">No wheels pinned and no weights locked yet.</div>`;

  kv(document.getElementById("k-host"), [
    ["platform", `<span class="mono">${esc(s.axes.os)}/${esc(s.axes.arch)}</span>`],
    ["libc", `<span class="mono">${esc(s.axes.libc.replace(/^libc /, ""))}</span>`],
    ["python", s.axes.python ? `<span class="mono">${esc(s.axes.python)}</span>` : `<span class="dim">not pinned</span>`],
    ["cuda", s.axes.cuda_driver ? `<span class="mono">${esc(s.axes.cuda_driver)}</span>${s.axes.sm.length ? ` · sm ${s.axes.sm.join(",")}` : ""}` : `<span class="dim">no driver</span>`],
    ["gpu memory", s.axes.gpu_mem_mb ? `<span class="mono">${bytes(s.axes.gpu_mem_mb*1048576)}</span>` : `<span class="dim">not detected</span>`],
  ]);

  kv(document.getElementById("k-policy"), [
    ["as-of", s.policy.as_of ? `<span class="mono">${esc(s.policy.as_of)}</span>` : `<span class="dim">latest</span>`],
    ["licences", (s.policy.allowed_licenses||[]).length ? `<span class="mono">${esc(s.policy.allowed_licenses.join(", "))}</span>` : `<span class="dim">any</span>`],
    ["vulnerable", s.policy.exclude_vulnerable ? mark("ok","excluded") : `<span class="dim">not filtered</span>`],
  ]);

  renderArtifacts(s); renderBudget(s); renderProof(s); renderForks(s); renderAgent(s);
}

function renderArtifacts(s) {
  const host = document.getElementById("artifacts");
  host.innerHTML = s.artifacts.map(a => `
    <div class="arow" data-fit="${a.fits}">
      <span>${fitMark(a.fits)}</span>
      <span class="kind">${esc(a.kind)}</span>
      <span class="name">${esc(a.name)}${a.version ? `<div class="sub">${esc(a.version)}</div>` : ""}</span>
      <span class="num size">${bytes(a.bytes)}</span>
      <span class="num">${bytes(a.vram_bytes)}</span>
      <span class="why">${(a.reasons||[]).map(r => `<span>${esc(r)}</span>`).join("")}</span>
    </div>`).join("") ||
    `<div class="empty">Nothing pinned. Install npm packages, pin wheels, or pin a model.</div>`;
  applyFilter();
}

function renderBudget(s) {
  const cached = s.plan.cached_bytes, missing = s.plan.missing_bytes;
  const total = cached + missing || 1;
  const pct = Math.round(cached * 100 / total);
  const missingFiles = s.plan.items.filter(i => !i.present);
  document.getElementById("budget").innerHTML = `
    <div class="figures">
      <div class="fig"><b>${bytes(missing)}</b><span>to download</span></div>
      <div class="fig"><b>${bytes(cached)}</b><span>cached</span></div>
      <div class="fig"><b>${missing ? s.plan.eta_seconds + " s" : "0 s"}</b>
        <span>at ${Math.round(s.plan.throughput_bytes_per_sec/1048576)} MB/s</span></div>
      <div class="fig"><b>${missingFiles.length}</b>
        <span>file${missingFiles.length===1?"":"s"} missing</span></div>
    </div>
    <div class="bars"><div class="barrow">
      <span class="nm">${pct}% of the plan is already on disk</span>
      <span class="bar"><i style="width:${Math.max(pct,1)}%"></i></span>
      <span class="v">${bytes(cached)}</span></div></div>
    ${missingFiles.length ? `<div style="margin-top:14px">
      <div class="rowhead"><span style="grid-column:1/3">Missing file</span>
        <span class="num" style="grid-column:5">Bytes</span></div>
      ${missingFiles.map(i => `<div class="lrow"><span class="t">${esc(i.name)}</span>
        <span class="v">${bytes(i.bytes)}</span></div>`).join("")}</div>`
      : `<p class="note">Nothing to fetch — everything the lockfile pins is on disk.</p>`}`;
}

function renderProof(s) {
  kv(document.getElementById("proof-kv"), [
    ["snapshot", s.proof.snapshot_ready ? `${mark("ok","ready")} <span class="muted">next install is a rename</span>`
      : `${mark("warn","absent")} <span class="muted">next install materialises from the lockfile</span>`],
    ["models", s.models.pinned ? `<span class="mono">${esc(s.models.names.join(", "))}</span>` : `<span class="dim">none pinned</span>`],
    ["python", s.python.pinned ? `<span class="mono">${s.python.pinned}</span> distributions` : `<span class="dim">none pinned</span>`],
  ]);
  kv(document.getElementById("proof-kv2"), [
    ["packages", `<span class="mono">${s.proof.attested_packages}</span>`],
    ["models", `<span class="mono">${s.proof.attested_models}</span>`],
    ["command", `<span class="mono">jhol attest</span>`],
  ]);
  document.getElementById("provenance").innerHTML = s.artifacts.length
    ? s.artifacts.map(a => `<div class="lrow">
        <span class="t">${esc(a.name)}</span>
        <span class="v">${bytes(a.bytes)}</span>
        <span class="d">${esc(a.kind)}${a.digest ? ` · <span class="mono">${esc(a.digest.slice(0,24))}</span>
          <button class="copy" data-copy="${esc(a.digest)}" type="button">copy</button>` : ""}</span>
      </div>`).join("")
    : `<div class="empty">No artefacts pinned.</div>`;
  document.querySelectorAll("button[data-copy]").forEach(b => {
    b.onclick = async () => {
      try { await navigator.clipboard.writeText(b.dataset.copy); b.textContent = "copied"; }
      catch (e) { b.textContent = "ctrl-c"; }
      setTimeout(() => { b.textContent = "copy"; }, 1500);
    };
  });
}

function renderForks(s) {
  const el = document.getElementById("forks");
  if (!s.forks.length) {
    el.innerHTML = `<div class="empty">No forks · <span class="mono">jhol env fork &lt;name&gt;</span>
      copies an environment in milliseconds.</div>`;
    return;
  }
  el.innerHTML = s.forks.map(f => `<div class="lrow">
      <span class="t">${esc(f.name)}</span>
      <span class="v">${bytes(f.bytes)} beyond base</span>
      <span class="d"><button class="btn" data-p="${esc(f.name)}" type="button">Promote</button></span>
    </div>`).join("");
  el.querySelectorAll("button[data-p]").forEach(b => {
    b.onclick = () => armOrRun("promote", b.dataset.p, true, "jhol env promote " + b.dataset.p, b);
  });
}

function renderAgent(s) {
  document.getElementById("agent").innerHTML = `<div class="rowhead">
      <span style="grid-column:1/3">Tool</span><span>Cost</span>
      <span style="grid-column:4/5">Purpose</span><span style="text-align:right">Try</span></div>` +
    s.agent.tools.map(t => `<div class="lrow">
      <span class="t">${esc(t.name)}</span><span class="v">${esc(t.cost)}</span>
      <span class="d">${esc(t.purpose)}</span>
      <button class="copy" data-tool="${esc(t.name)}" type="button">try</button></div>`).join("");
  document.querySelectorAll("button[data-tool]").forEach(b => {
    b.onclick = () => callTool(b.dataset.tool, b);
  });
  document.getElementById("mcp-config").onclick = async () => {
    const cfg = {
      mcpServers: {
        jhol: { command: "jhol", args: ["mcp", "--stdio"], cwd: s.root || "." },
      },
    };
    try {
      await navigator.clipboard.writeText(JSON.stringify(cfg, null, 2));
      document.getElementById("mcp-hint").textContent = "copied — paste into your client config";
    } catch (e) {
      document.getElementById("mcp-hint").textContent = "copy failed";
    }
  };
}

async function callTool(name, btn) {
  const out = document.getElementById("tool-out");
  out.textContent = `${name} …`;
  const j = await jsonOrError("/mcp", {
    method: "POST",
    headers: {"content-type": "application/json"},
    body: JSON.stringify({jsonrpc: "2.0", id: 1, method: "tools/call",
                          params: {name, arguments: {}}}),
  });
  const text = j.result && j.result.content
    ? j.result.content.map(c => c.text || "").join("")
    : (j.error ? (j.error.message || JSON.stringify(j.error)) : JSON.stringify(j));
  let pretty = text;
  try { pretty = JSON.stringify(JSON.parse(text), null, 2); } catch (e) {}
  out.textContent = `${name}\n${pretty}`.slice(0, 4000);
}

/* ── inspect ─────────────────────────────────────────────── */
function renderDetail(j) {
  const host = document.getElementById("detail-host");
  if (!j || typeof j !== "object" || (!j.error && !j.files && !j.path)) {
    // A 403 (stale token), a 404 or an empty body used to fall through to the file template
    // and paint "— / NaN%" as if that were a model. Say what happened instead.
    const why = j && (j.stderr || j.error) ? (j.stderr || j.error) : "no usable reply";
    host.innerHTML = `<div class="result"><div class="error">${esc(why)}</div>
      <p class="note">If this says <span class="mono">bad token</span>, the tab predates a server
      restart — reopen the URL printed by <span class="mono">jhol gui --web</span>.</p></div>`;
    return;
  }
  if (j.error) { host.innerHTML = `<div class="result"><div class="error">${esc(j.error)}</div></div>`; return; }
  if (j.files) {
    host.innerHTML = `<div class="result"><div class="path">${esc(j.repository)}</div>
      ${j.files.map(f => `<div class="lrow">
        <span class="t"><button class="btn" data-file="${esc(f.name)}" type="button">${esc(f.name)}</button></span>
        <span class="v">${bytes(f.bytes)}</span></div>`).join("")}
      <p class="note">Click a file to read its header — nothing is downloaded.</p></div>`;
    host.querySelectorAll("button[data-file]").forEach(b => {
      b.onclick = () => {
        document.getElementById("path").value = j.repository.replace(/\/$/, "") + "/" + b.dataset.file;
        document.getElementById("go").click();
      };
    });
    return;
  }
  const dtypes = (j.dtypes || []).map(d => `${d.dtype}×${d.tensors}`).join("  ");
  const readBase = (j.bytes_read || 0) + (j.data_bytes || 0);
  const readPct = readBase ? ((j.bytes_read / readBase) * 100).toFixed(3) : "0";
  host.innerHTML = `<div class="result">
    <div class="path">${esc(j.path)}</div>
    <div>
      <span class="tag">${j.format === "gguf" ? "GGUF" : "safetensors"}</span>
      ${j.architecture ? `<span class="tag">${esc(j.architecture)}</span>` : ""}
      <span class="tag">${j.fits === true ? "fits this host" : j.fits === false ? "too large for this GPU" : "fit unknown"}</span>
    </div>
    <div class="figures" style="margin-top:12px">
      <div class="fig"><b>${params(j.params)}</b><span>parameters</span></div>
      <div class="fig"><b>${bytes(j.data_bytes)}</b><span>tensor data</span></div>
      <div class="fig"><b>${bytes(j.vram_bytes)}</b><span>resident estimate</span></div>
      <div class="fig"><b>${j.tensors}</b><span>tensors</span></div>
      <div class="fig"><b>${bytes(j.bytes_read)}</b><span>read${j.requests ? ` · ${j.requests} request${j.requests===1?"":"s"}` : ""}</span></div>
    </div>
    <div class="bars">
      ${(j.largest||[]).slice(0,6).map(t => {
        const w = Math.max(1, Math.round(t.bytes / j.largest[0].bytes * 100));
        return `<div class="barrow"><span class="nm">${esc(t.name)}</span>
          <span class="bar"><i style="width:${w}%"></i></span>
          <span class="v">${bytes(t.bytes)}</span></div>`; }).join("")}
    </div>
    ${j.layers ? `<p class="note" style="margin-top:10px">
      <span class="mono">${j.layers} layers · ${j.heads} heads${j.kv_heads && j.kv_heads !== j.heads ? ` (${j.kv_heads} kv)` : ""} · head_dim ${j.head_dim ?? "?"}${j.native_context ? ` · context ${j.native_context}` : ""}${j.vocab ? ` · vocab ${j.vocab}` : ""}${j.rope_theta ? ` · rope ${j.rope_theta}` : ""}</span>
      ${(j.missing_meta || []).length ? `<span class="dim"> · no ${esc(j.missing_meta.join(", "))}</span>` : ""}</p>` : ""}
    ${(j.shards || []).length > 1 ? `<div style="margin-top:14px">
      <div class="rowhead"><span style="grid-column:1/4">Shard</span>
        <span class="num" style="grid-column:5">Data</span></div>
      ${j.shards.map(sh => `<div class="lrow"><span class="t">${esc(sh.name)}</span>
        <span class="v">${bytes(sh.bytes)}</span></div>`).join("")}</div>` : ""}
    <p class="note">${esc(dtypes)} · header is ${readPct}% of the file ·
      <button class="copy" id="to-fit" type="button">size it</button></p></div>`;
  const toFit = document.getElementById("to-fit");
  if (toFit) {
    toFit.onclick = () => {
      document.getElementById("fitpath").value = j.path;
      show("fit");
      runFit();
    };
  }
}

function staleNotice(text) {
  const el = document.getElementById("stale");
  if (!el) return;
  el.hidden = false;
  el.textContent = text;
}

async function jsonOrError(url, init) {
  const r = await fetch(url, init);
  if (r.status === 403) {
    const hint = "This tab is stale: its token no longer matches the running server. Reopen the URL printed by `jhol gui --web`.";
    staleNotice(hint);
    return { error: hint };
  }
  if (!r.ok) return { error: `request failed (${r.status})` };
  if (r.status === 204) return {};
  try { return await r.json(); } catch (e) { return { error: "unreadable reply from server" }; }
}

document.getElementById("go").onclick = async () => {
  const p = document.getElementById("path").value.trim();
  if (!p) return;
  const host = document.getElementById("detail-host");
  host.innerHTML = `<div class="result"><div class="empty">Reading the header…</div></div>`;
  const j = await jsonOrError(`/inspect?t=${encodeURIComponent(token)}&path=${encodeURIComponent(p)}`);
  renderDetail(j);
};
document.getElementById("path").addEventListener("keydown", e => {
  if (e.key === "Enter") document.getElementById("go").click();
});

function applyFilter() {
  const q = (document.getElementById("q").value || "").toLowerCase();
  const unfitOnly = document.getElementById("unfit").checked;
  document.querySelectorAll("#artifacts .arow").forEach(row => {
    const text = row.textContent.toLowerCase();
    const isUnfit = row.dataset.fit === "false";
    row.style.display = (!q || text.includes(q)) && (!unfitOnly || isUnfit) ? "" : "none";
  });
}
document.getElementById("q").oninput = applyFilter;
document.getElementById("unfit").onchange = applyFilter;

/* ── actions ─────────────────────────────────────────────── */
const armed = {key: null, at: 0};
const ACTIONS = [
  ["verify", "jhol verify", "Compare the installed tree with the lockfile. Reads only."],
  ["prune", "jhol verify --prune", "Verify, then remove packages the lockfile does not authorise."],
  ["install", "jhol install", "Materialise the tree from the lockfile, fetching only what is missing."],
  ["ci", "jhol ci", "Reproduce the lockfile exactly; refuses to write a drifting one."],
  ["policy", "jhol install --exclude-vuln", "Install with this project's policy in force."],
];
document.getElementById("acts").innerHTML = ACTIONS.map(([k, cmd, why]) => `
  <div class="lrow" data-row="${k}">
    <span class="t">${esc(cmd)}</span>
    <span class="v"><button class="btn" data-a="${k}" type="button">Run</button></span>
    <span class="d" data-desc="${k}">${esc(why)}</span>
  </div>`).join("");

async function armOrRun(action, name, stream, label, btn) {
  const key = action + ":" + name;
  if (armed.key !== key || Date.now() - armed.at > 10000) {
    let preview = {writes: true, summary: ""};
    try { preview = await (await fetch(`/plan?action=${encodeURIComponent(action)}`)).json(); } catch (e) {}
    if (preview.writes) {
      armed.key = key; armed.at = Date.now();
      show("actions");
      const desc = document.querySelector(`[data-desc="${action}"]`);
      if (desc) {
        desc.dataset.original = desc.dataset.original || desc.textContent;
        desc.textContent = `${preview.summary} — click Run again to continue.`;
        desc.style.color = "var(--warn)";
        setTimeout(() => {
          if (desc.dataset.original) { desc.textContent = desc.dataset.original; desc.style.color = ""; }
        }, 10000);
      }
      document.getElementById("arm-hint").textContent = "Armed. Click Run again within ten seconds.";
      if (btn) btn.textContent = "Confirm";
      return;
    }
  }
  armed.key = null;
  if (btn) btn.textContent = "Run";
  document.getElementById("arm-hint").textContent = "Writes ask once: the first click states what would happen.";
  run(action, name, stream, label);
}

async function run(action, name, stream, label) {
  show("actions");
  const out = document.getElementById("out");
  document.querySelectorAll(".btn").forEach(x => x.disabled = true);
  const shown = label || `jhol ${action}${name ? " " + name : ""}`;
  out.textContent = `$ ${shown}\n`;
  const hint = document.getElementById("arm-hint");
  const started = Date.now();
  const ticker = setInterval(() => {
    hint.textContent = `Running ${label || action} — ${Math.round((Date.now() - started) / 1000)} s`;
  }, 1000);
  try {
    const r = await fetch(`/action?t=${encodeURIComponent(token)}${stream ? "&stream=1" : ""}`, {
      method: "POST", headers: {"content-type": "application/json"},
      body: JSON.stringify({action, name: name || ""}),
    });
    if (stream && r.body) {
      const reader = r.body.getReader(), dec = new TextDecoder();
      for (;;) {
        const {value, done} = await reader.read();
        if (done) break;
        out.textContent += dec.decode(value, {stream: true});
        out.scrollTop = out.scrollHeight;
      }
    } else {
      const j = await r.json();
      out.textContent = `${label || action} → exit ${j.code}\n${j.stdout || ""}${j.stderr || ""}`;
    }
    packageRows = [];
    tick();
  } catch (e) {
    out.textContent += `\nerror: ${e.message}`;
  } finally {
    clearInterval(ticker);
    document.getElementById("arm-hint").textContent =
      "Writes ask once: the first click states what would happen.";
    document.querySelectorAll(".btn").forEach(x => x.disabled = false);
    document.querySelectorAll("[data-a]").forEach(x => x.textContent = "Run");
  }
}

document.querySelectorAll("[data-a]").forEach(b => {
  b.onclick = () => armOrRun(b.dataset.a, "", true, "jhol " + b.dataset.a, b);
});

/* ── polling with ETag ───────────────────────────────────── */
let etag = null;
async function tick() {
  try {
    const r = await fetch("/status.json", {headers: etag ? {"If-None-Match": etag} : {}});
    if (r.status === 304) return;
    etag = r.headers.get("ETag");
    render(await r.json());
  } catch (e) {}
}
tick(); setInterval(tick, 2000);
</script></body></html>
"##;

#[cfg(test)]
mod server_tests {
    use super::*;

    #[test]
    fn the_token_survives_a_restart_and_rotates_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let first = project_token(false);
        let again = project_token(false);
        assert_eq!(first, again, "a restart must reuse the token so tabs stay valid");
        let rotated = project_token(true);
        assert_ne!(first, rotated, "--new-token must mint a fresh one");
        assert_eq!(project_token(false), rotated, "and persist the new one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(".jhol/gui-token").unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must not be world-readable");
        }
        std::env::set_current_dir(previous).unwrap();
    }

    /// The rule the server applies, extracted so it can be tested without a socket.
    fn authorised(given: &str, want: &str, sec_fetch_site: Option<&str>) -> bool {
        sec_fetch_site
            .map(|v| v.eq_ignore_ascii_case("same-origin"))
            .unwrap_or(false)
            || token_matches(given, want)
    }

    #[test]
    fn a_browser_on_this_origin_is_authorised_without_the_token_and_a_foreign_one_never() {
        assert!(authorised("", "secret", Some("same-origin")), "the page itself must work");
        assert!(!authorised("", "secret", Some("cross-site")), "another origin must not");
        assert!(!authorised("", "secret", Some("same-site")), "same-site is a different origin");
        assert!(!authorised("", "secret", None), "no attestation needs the token");
        assert!(authorised("secret", "secret", None), "scripts keep working with the token");
    }

    #[test]
    fn the_token_is_compared_without_short_circuits_or_prefix_matches() {
        assert!(token_matches("abcd", "abcd"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("abce", "abcd"));
        assert!(!token_matches("", ""));
        assert!(!token_matches("", "abcd"));
    }

    #[test]
    fn the_write_surface_is_a_whitelist_and_names_are_validated() {
        assert!(action_argv("verify", "").is_some());
        assert!(action_argv("rm -rf /", "").is_none());
        assert!(action_argv("fork", "trial").is_some());
        // A name that could be read as a flag, a path, or two names is refused.
        for bad in ["-rf", "../x", "a/b", "", "x".repeat(65).as_str()] {
            assert!(action_argv("promote", bad).is_none(), "{bad:?} must be refused");
        }
        assert_eq!(
            action_argv("policy", "").unwrap(),
            vec!["install".to_string(), "--exclude-vuln".to_string()]
        );
    }

    #[test]
    fn query_parameters_decode_and_missing_keys_are_empty() {
        assert_eq!(query_param("/inspect?t=abc&path=a%2Fb+c", "path"), "a/b c");
        assert_eq!(query_param("/inspect?path=a", "t"), "");
        assert_eq!(query_param("/status.json", "t"), "");
    }

    #[test]
    fn a_notification_gets_no_response_and_a_bad_request_names_the_parse_error() {
        let note = mcp_reply(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(note.is_empty(), "JSON-RPC notifications must not be answered");

        let bad = mcp_reply(b"{nope");
        let v: serde_json::Value = serde_json::from_str(&bad).unwrap();
        assert_eq!(v["error"]["code"], -32700);
    }

    #[test]
    fn the_bridge_lists_the_tools_the_server_answers() {
        let v: serde_json::Value = serde_json::from_str(&tools_reply()).unwrap();
        let tools = v["result"]["tools"].as_array().expect("tools array");
        assert!(tools.len() >= 10, "bridge must expose the real tool set");
        assert!(tools.iter().any(|t| t["name"] == "jhol_axes"));
    }
}
