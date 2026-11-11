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
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Refuse to read a body larger than this. The UI posts small JSON objects; anything bigger
/// is either a mistake or an attempt to make the server allocate.
const MAX_BODY: usize = 8 * 1024;
/// A connection that cannot produce a request line in this long is not a browser.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

pub fn serve(port: u16, open: bool) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| match e.kind() {
        std::io::ErrorKind::AddrInUse => format!(
            "gui --web: port {port} is already in use — pass --port 0 for a free one"
        ),
        _ => format!("gui --web: {e}"),
    })?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let shared = Arc::new(Shared {
        token: session_token(),
        cache: Mutex::new((String::new(), String::new(), None)),
    });
    let url = format!("http://{addr}/?t={}", shared.token);
    println!("jhol gui --web on {url}");
    println!("  stop with Ctrl-C · loopback only · requests need the token in the URL");
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

    let needs_token = path.starts_with("/action") || path.starts_with("/mcp") || path.starts_with("/inspect");
    if needs_token && !token_matches(&given, &shared.token) {
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

fn page_html() -> String {
    PAGE.to_string()
}

const PAGE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>jhol</title>
<style>
 :root{
   --bg:#0b0b0c; --ink:#ececf0; --mut:#9b9ba4; --dim:#6a6a72;
   --hair:#1c1c20; --hair2:#27272c; --panel:#0f0f11;
   --accent:#8fc7a5; --bad:#d9918f; --warn:#c9b47c; --link:#a9c7e8;
   --sans:-apple-system,BlinkMacSystemFont,"Segoe UI",Inter,system-ui,sans-serif;
   --serif:ui-serif,"New York",Iowan,Georgia,serif;
   --mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,monospace;
   color-scheme:dark;
 }
 @media (prefers-color-scheme: light){
   :root{
     --bg:#fcfcfa; --ink:#171719; --mut:#5e5e67; --dim:#8d8d95;
     --hair:#e7e6e1; --hair2:#d8d7d1; --panel:#f7f7f4;
     --accent:#2e6b4b; --bad:#9c3b38; --warn:#7c6118; --link:#2c5f96;
     color-scheme:light;
   }
 }
 *{box-sizing:border-box}
 html,body{margin:0}
 body{background:var(--bg);color:var(--ink);font:13.5px/1.6 var(--sans);
      -webkit-font-smoothing:antialiased;text-rendering:optimizeLegibility}
 ::selection{background:var(--accent);color:var(--bg)}
 .sr-only{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);
          white-space:nowrap;border:0}
 :focus-visible{outline:1px solid var(--accent);outline-offset:2px}
 a{color:inherit;text-decoration:none;border-bottom:1px solid var(--hair2)}
 a:hover{border-bottom-color:var(--accent)}
 .wrap{max-width:1080px;margin:0 auto;padding:0 32px 96px}

 /* ── masthead ───────────────────────────────────────────── */
 header.top{max-width:1080px;margin:0 auto;padding:38px 32px 0;
   display:grid;grid-template-columns:1fr auto;gap:8px 28px;align-items:end}
 .brand{font:400 15px/1 var(--mono);letter-spacing:.14em;text-transform:lowercase;
   color:var(--mut)}
 .title{font:400 26px/1.15 var(--serif);letter-spacing:-.015em;margin:6px 0 0;
   color:var(--ink)}
 .title .ver{font:12px/1 var(--mono);color:var(--dim);letter-spacing:0;margin-left:8px}
 .lede{grid-column:1/-1;color:var(--mut);font-size:13px;margin:14px 0 0;max-width:70ch}
 .lede .sep{color:var(--dim);padding:0 7px}
 .state-ok{color:var(--accent)} .state-bad{color:var(--bad)}

 /* ── nav ────────────────────────────────────────────────── */
 nav{border-bottom:1px solid var(--hair);margin-top:26px}
 .wrap nav{display:flex;gap:22px;overflow-x:auto;scrollbar-width:none}
 nav button{appearance:none;background:none;border:0;border-bottom:1px solid transparent;
   color:var(--mut);font:inherit;font-size:13px;padding:11px 0 12px;cursor:pointer;
   white-space:nowrap;display:inline-flex;align-items:baseline;gap:6px;margin-bottom:-1px}
 nav button:hover{color:var(--ink)}
 nav button.on{color:var(--ink);border-bottom-color:var(--ink)}
 nav .n{font:11px/1 var(--mono);color:var(--dim);font-variant-numeric:tabular-nums}

 /* ── sections + type ────────────────────────────────────── */
 section{display:none} section.on{display:block}
 h2{font:400 12px/1 var(--sans);color:var(--dim);margin:0 0 14px;letter-spacing:.01em}
 .block{padding:30px 0 6px;border-bottom:1px solid var(--hair)}
 .block:last-child{border-bottom:0}
 .cols{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));
       gap:0 56px}
 .kv{display:grid;grid-template-columns:96px 1fr;gap:9px 16px;align-items:baseline;margin:0}
 .kv dt{color:var(--dim);font-size:12.5px}
 .kv dd{margin:0;min-width:0}
 .mono{font-family:var(--mono);font-size:12.5px;font-variant-numeric:tabular-nums;
       letter-spacing:-.01em}
 .muted{color:var(--mut)} .dim{color:var(--dim)}
 .empty{color:var(--dim);padding:26px 0;font-size:13px}
 .note{color:var(--dim);font-size:12.5px;margin:12px 0 0}

 /* ── status marks: a word, not a sticker ────────────────── */
 .mark{display:inline-flex;align-items:center;gap:7px}
 .mark::before{content:"";width:5px;height:5px;border-radius:50%;background:var(--dim);
   flex:none;transform:translateY(-1px)}
 .mark.ok{color:var(--accent)} .mark.ok::before{background:var(--accent)}
 .mark.bad{color:var(--bad)} .mark.bad::before{background:var(--bad)}
 .mark.warn{color:var(--warn)} .mark.warn::before{background:var(--warn)}
 .tag{font:11px/1 var(--mono);color:var(--dim);border:1px solid var(--hair2);
   border-radius:3px;padding:2px 5px;letter-spacing:0}

 /* ── tables ─────────────────────────────────────────────── */
 table{width:100%;border-collapse:collapse;table-layout:fixed}
 th{font:400 12px/1 var(--sans);color:var(--dim);text-align:left;
    padding:0 16px 10px 0;border-bottom:1px solid var(--hair2)}
 td{padding:13px 16px 13px 0;border-bottom:1px solid var(--hair);vertical-align:baseline}
 tr:last-child td{border-bottom:0}
 td.num,th.num{font-family:var(--mono);font-variant-numeric:tabular-nums;text-align:right;
   padding-right:16px;white-space:nowrap}
 th:last-child,td:last-child{padding-right:0}
 td.name{font-family:var(--mono);font-size:12.5px;overflow-wrap:anywhere}
 td .sub{color:var(--dim);font:11.5px/1.5 var(--mono)}
 .why{color:var(--mut);font-size:12.5px}
 .why span+span::before{content:" · ";color:var(--dim)}

 /* ── controls ───────────────────────────────────────────── */
 input{background:transparent;border:0;border-bottom:1px solid var(--hair2);
   color:var(--ink);font:13px/1.5 var(--sans);padding:9px 2px;width:100%;outline:none}
 input:focus{border-bottom-color:var(--accent)}
 input::placeholder{color:var(--dim)}
 input.mono{font-family:var(--mono);font-size:12.5px}
 .field{display:flex;gap:18px;align-items:center;flex-wrap:wrap}
 .field .grow{flex:1;min-width:240px}
 .link{appearance:none;background:none;border:0;border-bottom:1px solid var(--hair2);
   color:var(--ink);font:inherit;font-size:13px;padding:0 0 2px;cursor:pointer}
 .link:hover{border-bottom-color:var(--accent);color:var(--accent)}
 .link[disabled]{color:var(--dim);border-bottom-color:transparent;cursor:progress}
 .check{display:inline-flex;align-items:center;gap:8px;color:var(--mut);font-size:12.5px;
   cursor:pointer;user-select:none;white-space:nowrap}
 .check input{width:auto;border:0;accent-color:var(--accent)}

 /* ── inspect result ─────────────────────────────────────── */
 .result{margin-top:26px}
 .result .path{font:12px/1.5 var(--mono);color:var(--mut);overflow-wrap:anywhere}
 .figures{display:flex;flex-wrap:wrap;gap:0 40px;margin:14px 0 4px}
 .fig{display:flex;flex-direction:column;gap:3px;padding:6px 0}
 .fig b{font:400 19px/1.15 var(--serif);font-variant-numeric:tabular-nums;letter-spacing:-.01em}
 .fig span{font-size:11.5px;color:var(--dim)}
 .bars{margin-top:18px;display:grid;gap:9px}
 .barrow{display:grid;grid-template-columns:minmax(0,1fr) 132px 74px;gap:16px;
         align-items:center}
 .barrow .nm{font:12px/1.4 var(--mono);color:var(--mut);overflow:hidden;
             text-overflow:ellipsis;white-space:nowrap}
 .bar{height:2px;background:var(--hair2);position:relative}
 .bar>i{position:absolute;inset:0 auto 0 0;background:var(--accent);opacity:.9}
 .barrow .v{text-align:right;font:12px/1 var(--mono);color:var(--mut);
            font-variant-numeric:tabular-nums}
 .error{border-left:2px solid var(--bad);padding:10px 0 10px 14px;color:var(--bad);
        font:12.5px/1.6 var(--mono);white-space:pre-wrap}

 /* ── actions ────────────────────────────────────────────── */
 .act{display:grid;grid-template-columns:230px 1fr auto;gap:20px;align-items:baseline;
      padding:15px 0;border-bottom:1px solid var(--hair)}
 .act:first-of-type{border-top:1px solid var(--hair)}
 .act .cmd{font:12.5px/1.5 var(--mono);color:var(--ink)}
 .act .d{color:var(--mut);font-size:12.5px}
 .act .armed{color:var(--warn)}
 pre{font:12.5px/1.65 var(--mono);color:var(--mut);white-space:pre-wrap;
     overflow:auto;max-height:24rem;margin:0;padding:16px 0 0;
     border-top:1px solid var(--hair)}
 pre:empty{display:none}
 footer{max-width:1080px;margin:46px auto 0;padding:0 32px;color:var(--dim);
        font:11.5px/1.9 var(--mono)}
 footer .kbd{border:1px solid var(--hair2);border-radius:3px;padding:1px 4px}
 @media (max-width:720px){
   .wrap{padding:0 18px 64px} header.top{padding:26px 18px 0}
   .title{font-size:22px} .act{grid-template-columns:1fr auto;gap:6px 16px}
   .act .d{grid-column:1/-1} .kv{grid-template-columns:86px 1fr}
 }
</style></head><body>

<header class="top">
  <div>
    <div class="brand">jhol</div>
    <h1 class="title"><span id="name">…</span><span class="ver" id="version"></span></h1>
  </div>
  <div class="dim mono" style="text-align:right" id="stamp"></div>
  <p class="lede" id="lede" role="status" aria-live="polite"></p>
</header>

<div class="wrap">
  <nav id="nav"></nav>

  <section id="s-status" class="on">
    <div class="block" id="onboard" hidden>
      <h2>No project here</h2>
      <p class="muted" style="max-width:60ch;margin:0 0 14px">
        This directory has no <span class="mono">package.json</span>, so there is nothing to
        verify yet. Pick one of these and change what you need in the terminal — the dashboard
        is read-only until a project exists.</p>
      <dl class="kv">
        <dt>start one</dt><dd class="mono">jhol install express</dd>
        <dt>weights</dt><dd class="mono">jhol ai pull hf://org/repo/model.safetensors</dd>
        <dt>watch</dt><dd class="mono">jhol status --watch</dd>
      </dl>
      <p class="note">Once a lockfile exists, this page fills in on its own — it polls the
      filesystem, not a server cache.</p>
    </div>
    <div class="block status-block">
      <div class="cols">
        <div><h2>Tree</h2><dl class="kv" id="k-tree"></dl></div>
        <div><h2>Artefacts</h2><div id="k-artefacts"></div></div>
      </div>
    </div>
    <div class="block status-block">
      <div class="cols">
        <div><h2>Host</h2><dl class="kv" id="k-host"></dl></div>
        <div><h2>Policy</h2><dl class="kv" id="k-policy"></dl></div>
      </div>
    </div>
    </div>
  </section>

  <section id="s-artifacts">
    <div class="block">
      <div class="field">
        <div class="grow"><label class="sr-only" for="q">Filter artefacts</label>
          <input id="q" placeholder="Filter by name, kind or reason" autocomplete="off"></div>
        <label class="check"><input type="checkbox" id="unfit"> Only what will not run</label>
      </div>
    </div>
    <div class="block">
      <table>
        <colgroup><col style="width:86px"><col style="width:64px"><col style="width:27%">
          <col style="width:88px"><col style="width:92px"><col></colgroup>
        <thead><tr><th>Fit</th><th>Kind</th><th>Name</th><th class="num">Size</th>
          <th class="num">Resident</th><th>Why</th></tr></thead>
        <tbody id="artifacts"></tbody>
      </table>
    </div>
    <div class="block">
      <h2>Read a header</h2>
      <div class="field">
        <div class="grow"><label class="sr-only" for="path">Weight file, pinned model or URL</label>
          <input id="path" class="mono"
          placeholder="path, pinned model, hf://org/repo/file.gguf, or a huggingface.co URL"></div>
        <button class="link" id="go" type="button">Inspect</button>
      </div>
      <div id="detail-host"></div>
    </div>
  </section>

  <section id="s-budget">
    <div class="block"><h2>Download plan</h2><div id="budget"></div></div>
    <div class="block"><h2>Forks</h2><div id="forks"></div></div>
  </section>

  <section id="s-proof">
    <div class="block"><div class="cols">
      <div><h2>Reproducibility</h2><dl class="kv" id="proof-kv"></dl></div>
      <div><h2>Attestation</h2><dl class="kv" id="proof-kv2"></dl></div>
    </div></div>
    <div class="block"><h2>Artefact provenance</h2><div id="provenance"></div></div>
  </section>

  <section id="s-agent">
    <div class="block"><h2>Tools an agent can call</h2><div id="agent"></div>
      <p class="note">Answered over <span class="mono">jhol mcp --stdio</span> and
      <span class="mono">POST /mcp</span> on this page.</p></div>
  </section>

  <section id="s-actions">
    <div class="block"><h2>Run</h2>
      <div id="acts"></div>
      <p class="note" id="arm-hint">Writes ask once: the first click states what would happen.</p>
      <pre id="out" role="log" aria-live="polite"></pre>
    </div>
  </section>

  <footer>
    same data as <span class="mono">jhol status --json</span> ·
    watch with <span class="mono">jhol status --watch</span> ·
    <span class="kbd">1</span>–<span class="kbd">6</span> sections ·
    <span class="kbd">/</span> filter
  </footer>
</div>

<script>
const token = new URLSearchParams(location.search).get("t") || "";
const SECTIONS = ["status","artifacts","budget","proof","agent","actions"];
const LABEL = {status:"Status",artifacts:"Artefacts",budget:"Budget",
               proof:"Proof",agent:"Agent",actions:"Actions"};
const nav = document.getElementById("nav");
SECTIONS.forEach((s, i) => {
  const b = document.createElement("button");
  b.dataset.section = s;
  if (i === 0) b.className = "on";
  b.innerHTML = `${LABEL[s]}<span class="n" id="n-${s}"></span>`;
  b.onclick = () => show(s);
  nav.appendChild(b);
});
function show(s) {
  document.querySelectorAll("nav button").forEach(x => x.classList.toggle("on", x.dataset.section === s));
  document.querySelectorAll("section").forEach(x => x.classList.toggle("on", x.id === "s-" + s));
}
document.addEventListener("keydown", e => {
  if (e.target.tagName === "INPUT") {
    if (e.key === "Escape") e.target.blur();
    return;
  }
  const i = parseInt(e.key, 10);
  if (i >= 1 && i <= SECTIONS.length) show(SECTIONS[i - 1]);
  if (e.key === "/") { e.preventDefault(); show("artifacts");
    document.getElementById("q").focus(); }
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
const mark = (kind, text) => `<span class="mark ${kind}">${esc(text)}</span>`;
const fitMark = f => f === true ? mark("ok", "fits")
  : f === false ? mark("bad", "will not run") : mark("warn", "unknown");
function kv(el, rows) {
  el.innerHTML = rows.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${v}</dd>`).join("");
}

function render(s) {
  const onboard = document.getElementById("onboard");
  onboard.hidden = !!s.has_manifest;
  document.querySelectorAll(".status-block").forEach(el => { el.hidden = !s.has_manifest; });
  document.getElementById("name").textContent = s.has_manifest ? (s.project || "(unnamed)") : "no project";
  document.getElementById("version").textContent = s.version || "";
  const unfit = s.artifacts.filter(a => a.fits === false).length;
  const unknown = s.artifacts.filter(a => a.fits === null || a.fits === undefined).length;
  const total = s.artifacts.length;

  document.getElementById("stamp").textContent =
    new Date().toLocaleTimeString([], {hour:"2-digit", minute:"2-digit"});

  if (!s.has_manifest) {
    document.getElementById("lede").innerHTML =
      `No <span class="mono">package.json</span> in this directory<span class="sep">·</span>nothing to verify yet`;
    ["status","artifacts","budget","proof","agent","actions"].forEach(k => {
      const n = document.getElementById("n-" + k); if (n) n.textContent = "";
    });
    renderArtifacts(s); return;
  }
  document.getElementById("lede").innerHTML = s.tree.state === "match"
    ? `<span class="state-ok">Tree matches the lockfile</span><span class="sep">·</span>${s.tree.packages} packages<span class="sep">·</span>${s.manifest_matches_lockfile ? "lockfile current" : `<span class="state-bad">lockfile drifted</span>`}<span class="sep">·</span>${s.plan.missing_bytes ? `${bytes(s.plan.missing_bytes)} to fetch` : "nothing to fetch"}<span class="sep">·</span>${unfit ? `<span class="state-bad">${unfit} artefact${unfit===1?"":"s"} will not run here</span>` : total ? "all fit" : "no artefacts pinned"}`
    : `<span class="state-bad">Tree ${esc(s.tree.state)}</span><span class="sep">·</span>${esc(s.tree.detail)}<span class="sep">·</span>${s.tree.packages} packages`;

  document.getElementById("n-status").textContent = s.tree.packages;
  document.getElementById("n-artifacts").textContent = total || "";
  document.getElementById("n-budget").textContent = s.plan.items.filter(i => !i.present).length || "";
  document.getElementById("n-proof").textContent = s.proof.attested_packages || "";
  document.getElementById("n-agent").textContent = s.agent.tools.length;
  document.getElementById("n-actions").textContent = "";

  kv(document.getElementById("k-tree"), [
    ["verdict", s.tree.state === "match"
      ? mark("ok", `match — ${s.tree.packages} packages`) : mark("bad", esc(s.tree.state))],
    ["detail", `<span class="muted">${esc(s.tree.detail || "every package is at its locked version")}</span>`],
    ["lockfile", s.manifest_matches_lockfile ? mark("ok","current") : mark("bad","drifted from package.json")],
    ["snapshot", s.proof.snapshot_ready ? mark("ok","ready") : mark("warn","absent")],
  ]);

  document.getElementById("k-artefacts").innerHTML = total ? `
    <dl class="kv">
      <dt>pinned</dt><dd class="mono">${total}</dd>
      <dt>fit</dt><dd>${total - unfit - unknown ? mark("ok", String(total - unfit - unknown)) : `<span class="dim">0</span>`}</dd>
      <dt>will not run</dt><dd>${unfit ? mark("bad", String(unfit)) : `<span class="dim">0</span>`}</dd>
      <dt>unknown</dt><dd>${unknown ? mark("warn", String(unknown)) : `<span class="dim">0</span>`}</dd>
    </dl>
    ${unknown ? `<p class="note">Unknown means we cannot decide: no GPU to compare against, or a wheel with no platform tag. It is never counted as a pass.</p>` : ""}`
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
  const tb = document.getElementById("artifacts");
  tb.innerHTML = s.artifacts.map(a => `
    <tr data-fit="${a.fits}">
      <td>${fitMark(a.fits)}</td>
      <td class="dim">${esc(a.kind)}</td>
      <td class="name">${esc(a.name)}${a.version ? `<div class="sub">${esc(a.version)}</div>` : ""}</td>
      <td class="num">${bytes(a.bytes)}</td>
      <td class="num">${bytes(a.vram_bytes)}</td>
      <td class="why"><span>${(a.reasons||[]).map(esc).join("</span><span>")}</span></td>
    </tr>`).join("") ||
    `<tr><td colspan="6"><div class="empty">Nothing pinned. Install npm packages, pin wheels, or pin a model.</div></td></tr>`;
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
      <div class="fig"><b>${bytes(cached)}</b><span>already cached</span></div>
      <div class="fig"><b>${missing ? s.plan.eta_seconds + " s" : "0 s"}</b>
        <span>at ${Math.round(s.plan.throughput_bytes_per_sec/1048576)} MB/s</span></div>
      <div class="fig"><b>${missingFiles.length}</b>
        <span>file${missingFiles.length===1?"":"s"} missing</span></div>
    </div>
    <div class="bars"><div class="barrow">
      <span class="nm">${pct}% of the plan is already on disk</span>
      <span class="bar"><i style="width:${Math.max(pct,0.6)}%"></i></span>
      <span class="v">${bytes(cached)}</span></div></div>
    ${missingFiles.length ? `<table style="margin-top:22px">
      <thead><tr><th>Missing file</th><th class="num" style="width:110px">Bytes</th></tr></thead>
      <tbody>${missingFiles.map(i => `<tr><td class="name">${esc(i.name)}</td>
        <td class="num">${bytes(i.bytes)}</td></tr>`).join("")}</tbody></table>`
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
  document.getElementById("provenance").innerHTML = s.artifacts.length ? `<table>
    <thead><tr><th>Artefact</th><th style="width:150px">Kind</th>
      <th class="num" style="width:110px">Bytes</th><th>Digest</th></tr></thead>
    <tbody>${s.artifacts.map(a => `<tr>
      <td class="name">${esc(a.name)}</td><td class="dim">${esc(a.kind)}</td>
      <td class="num">${bytes(a.bytes)}</td>
      <td class="mono dim">${esc((a.digest||"").slice(0,24)) || "—"}</td></tr>`).join("")}</tbody></table>`
    : `<div class="empty">No artefacts pinned.</div>`;
}

function renderForks(s) {
  const el = document.getElementById("forks");
  if (!s.forks.length) {
    el.innerHTML = `<div class="empty">No forks · <span class="mono">jhol env fork &lt;name&gt;</span> copies an environment in milliseconds.</div>`;
    return;
  }
  el.innerHTML = `<table><thead><tr><th>Fork</th>
    <th class="num" style="width:150px">Beyond base</th><th style="width:100px"></th></tr></thead>
    <tbody>${s.forks.map(f => `<tr><td class="name">${esc(f.name)}</td>
      <td class="num">${bytes(f.bytes)}</td>
      <td style="text-align:right"><button class="link" data-p="${esc(f.name)}" type="button">Promote</button></td></tr>`).join("")}</tbody></table>`;
  el.querySelectorAll("button[data-p]").forEach(b => {
    b.onclick = () => armOrRun("promote", b.dataset.p, true, "jhol env promote " + b.dataset.p, b);
  });
}

function renderAgent(s) {
  document.getElementById("agent").innerHTML = `<table>
    <thead><tr><th style="width:250px">Tool</th><th style="width:90px">Cost</th><th>Purpose</th></tr></thead>
    <tbody>${s.agent.tools.map(t => `<tr>
      <td class="name">${esc(t.name)}</td>
      <td class="dim">${esc(t.cost)}</td>
      <td class="muted">${esc(t.purpose)}</td></tr>`).join("")}</tbody></table>`;
}

/* ── inspect ─────────────────────────────────────────────── */
function renderDetail(j) {
  const host = document.getElementById("detail-host");
  if (j.error) { host.innerHTML = `<div class="result"><div class="error">${esc(j.error)}</div></div>`; return; }
  if (j.files) {
    host.innerHTML = `<div class="result"><div class="path">${esc(j.repository)}</div>
      <table style="margin-top:16px"><thead><tr><th>Weight file</th>
        <th class="num" style="width:120px">Bytes</th></tr></thead>
      <tbody>${j.files.map(f => `<tr><td class="name"><a href="#" data-file="${esc(f.name)}">${esc(f.name)}</a></td>
        <td class="num">${bytes(f.bytes)}</td></tr>`).join("")}</tbody></table>
      <p class="note">Click a file to read its header — nothing is downloaded.</p></div>`;
    host.querySelectorAll("a[data-file]").forEach(a => {
      a.onclick = e => {
        e.preventDefault();
        document.getElementById("path").value = j.repository.replace(/\/$/, "") + "/" + a.dataset.file;
        document.getElementById("go").click();
      };
    });
    return;
  }
  const dtypes = (j.dtypes || []).map(d => `${d.dtype}×${d.tensors}`).join("  ");
  const readPct = ((j.bytes_read/(j.bytes_read+j.data_bytes||1))*100).toFixed(3);
  host.innerHTML = `<div class="result">
    <div class="path">${esc(j.path)}</div>
    <div class="figures">
      <div class="fig"><b>${params(j.params)}</b><span>parameters</span></div>
      <div class="fig"><b>${bytes(j.data_bytes)}</b><span>tensor data</span></div>
      <div class="fig"><b>${bytes(j.vram_bytes)}</b><span>resident estimate</span></div>
      <div class="fig"><b>${j.tensors}</b><span>tensors</span></div>
      <div class="fig"><b>${bytes(j.bytes_read)}</b><span>read${j.requests ? ` · ${j.requests} request${j.requests===1?"":"s"}` : ""}</span></div>
    </div>
    <p class="note">${j.format === "gguf" ? "GGUF" : "safetensors"}${j.architecture ? ` · ${esc(j.architecture)}` : ""} · ${esc(dtypes)} · header is ${readPct}% of the file · ${j.fits === true ? mark("ok","fits this host") : j.fits === false ? mark("bad","too large for this GPU") : mark("warn","fit unknown")}</p>
    <div class="bars">
      ${(j.largest||[]).slice(0,6).map(t => {
        const w = Math.max(0.6, Math.round(t.bytes / j.largest[0].bytes * 100));
        return `<div class="barrow"><span class="nm">${esc(t.name)}</span>
          <span class="bar"><i style="width:${w}%"></i></span>
          <span class="v">${bytes(t.bytes)}</span></div>`; }).join("")}
    </div></div>`;
}

document.getElementById("go").onclick = async () => {
  const p = document.getElementById("path").value.trim();
  if (!p) return;
  const host = document.getElementById("detail-host");
  host.innerHTML = `<div class="result"><div class="empty">Reading the header…</div></div>`;
  try {
    const r = await fetch(`/inspect?t=${encodeURIComponent(token)}&path=${encodeURIComponent(p)}`);
    renderDetail(await r.json());
  } catch (e) {
    host.innerHTML = `<div class="result"><div class="error">${esc(e.message)}</div></div>`;
  }
};
document.getElementById("path").addEventListener("keydown", e => {
  if (e.key === "Enter") document.getElementById("go").click();
});

function applyFilter() {
  const q = (document.getElementById("q").value || "").toLowerCase();
  const unfitOnly = document.getElementById("unfit").checked;
  document.querySelectorAll("#artifacts tr[data-fit]").forEach(row => {
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
  <div class="act" data-row="${k}">
    <span class="cmd">${esc(cmd)}</span>
    <span class="d" data-desc="${k}">${esc(why)}</span>
    <button class="link" data-a="${k}" type="button">Run</button>
  </div>`).join("");

async function armOrRun(action, name, stream, label, btn) {
  const key = action + ":" + name;
  const fresh = armed.key === key && Date.now() - armed.at < 10000;
  if (!fresh) {
    let preview = {writes: true, summary: ""};
    try { preview = await (await fetch(`/plan?action=${encodeURIComponent(action)}`)).json(); } catch (e) {}
    if (preview.writes) {
      armed.key = key; armed.at = Date.now();
      show("actions");
      const desc = document.querySelector(`[data-desc="${action}"]`);
      if (desc) {
        desc.dataset.original = desc.dataset.original || desc.textContent;
        desc.textContent = `${preview.summary} — click Run again to continue.`;
        desc.className = "d armed";
        setTimeout(() => {
          if (desc.dataset.original) { desc.textContent = desc.dataset.original; desc.className = "d"; }
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
  document.querySelectorAll(".act button,.link").forEach(x => x.disabled = true);
  out.textContent = `$ jhol ${action}${name ? " " + name : ""}\n`;
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
    tick();
  } catch (e) {
    out.textContent += `\nerror: ${e.message}`;
  } finally {
    document.querySelectorAll(".act button,.link").forEach(x => x.disabled = false);
    document.querySelectorAll(".act button").forEach(x => x.textContent = "Run");
  }
}

document.querySelectorAll(".act button[data-a]").forEach(b => {
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
