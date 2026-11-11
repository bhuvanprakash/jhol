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
use std::net::TcpListener;
use std::path::Path;

pub fn serve() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("gui --web: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let token = session_token();
    let url = format!("http://{addr}/?t={token}");
    println!("jhol gui --web on {url}  (Ctrl-C to stop)");
    open_browser(&url);

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let token = token.clone();
        // Each connection is handled inline: a dashboard poll every second is not a
        // workload that needs a thread pool, and serial handling means no request can
        // observe a half-written status.
        let mut reader = match stream.try_clone() {
            Ok(s) => BufReader::new(s),
            Err(_) => continue,
        };
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            continue;
        }
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .to_string();
        let (headers, content_length) = read_headers(&mut reader);
        let if_none_match = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("if-none-match"))
            .map(|(_, v)| v.trim().to_string());
        let given = path
            .split_once("t=")
            .map(|(_, t)| t.split('&').next().unwrap_or(t).to_string())
            .unwrap_or_default();

        if path.starts_with("/action") {
            let body = read_body(&mut reader, content_length);
            if given != token {
                respond(&mut stream, "application/json", &serde_json::json!({
                    "ok": false, "code": 403, "stderr": "bad token"
                }).to_string());
                continue;
            }
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("");
            let name = v.get("name").and_then(|a| a.as_str()).unwrap_or("");
            if path.contains("stream=1") {
                stream_argv(&mut stream, action_argv(action, name));
            } else {
                respond(&mut stream, "application/json", &run_action_json(action, name).to_string());
            }
            continue;
        }

        if path.starts_with("/mcp") {
            let body = read_body(&mut reader, content_length);
            if given != token {
                respond(&mut stream, "application/json", &serde_json::json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": {"code": -32001, "message": "bad token"}
                }).to_string());
                continue;
            }
            respond(&mut stream, "application/json", &mcp_reply(&body));
            continue;
        }

        if path.starts_with("/tools.json") {
            respond(&mut stream, "application/json", &tools_reply());
            continue;
        }

        if path.starts_with("/plan") {
            let action = path
                .split("action=")
                .nth(1)
                .map(|a| percent_decode(a.split('&').next().unwrap_or(a)))
                .unwrap_or_default();
            let status = crate::status_view::enrich(
                jhol_core::status::collect(Path::new(".")),
                Path::new("."),
            );
            let preview = jhol_core::status::action_preview(&action, &status);
            respond(
                &mut stream,
                "application/json",
                &serde_json::to_string(&preview).unwrap_or_else(|e| e.to_string()),
            );
            continue;
        }

        if path.starts_with("/inspect") {
            if given != token {
                respond(&mut stream, "application/json", "{\"error\":\"bad token\"}");
                continue;
            }
            let target = path
                .split("path=")
                .nth(1)
                .map(|p| percent_decode(p.split('&').next().unwrap_or(p)))
                .unwrap_or_default();
            respond(&mut stream, "application/json", &inspect_json(Path::new("."), &target).to_string());
            continue;
        }

        if path.starts_with("/status.json") {
            // A dashboard polls once a second. Collecting a status costs milliseconds; a
            // fingerprint of the files a status depends on costs microseconds. When the
            // fingerprint matches, the answer is 304 and **no collection happens at all**.
            let tag = jhol_core::status::fingerprint(Path::new("."));
            if if_none_match.as_deref() == Some(tag.as_str()) {
                let _ = write!(
                    stream,
                    "HTTP/1.1 304 Not Modified\r\nETag: {tag}\r\n\
                     Cache-Control: no-store\r\nConnection: close\r\n\r\n"
                );
                continue;
            }
            let status = crate::status_view::enrich(
                jhol_core::status::collect(Path::new(".")),
                Path::new("."),
            );
            respond_etag(
                &mut stream,
                "application/json",
                &serde_json::to_string(&status).unwrap_or_else(|e| e.to_string()),
                &tag,
            );
            continue;
        }

        respond(&mut stream, "text/html; charset=utf-8", &page_html());
    }
    Ok(())
}

/// One-shot session token. The page is served to a browser on this machine, but a page on
/// any other origin could POST to `127.0.0.1` too; requiring the token that only the opened
/// tab knows keeps a random web page from driving installs.
fn session_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

fn read_headers(reader: &mut BufReader<std::net::TcpStream>) -> (Vec<(String, String)>, usize) {
    let mut out = Vec::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    (out, length)
}

fn read_body(reader: &mut BufReader<std::net::TcpStream>, length: usize) -> Vec<u8> {
    let mut raw = vec![0u8; length.min(8192)];
    let _ = reader.read_exact(&mut raw);
    raw
}

fn respond_etag(stream: &mut std::net::TcpStream, kind: &str, body: &str, tag: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nETag: {tag}\r\n\
         Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn respond(stream: &mut std::net::TcpStream, kind: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// A fork name becomes a CLI argument, so it is validated here rather than trusted: no
/// separators, no leading dash, nothing that could be read as a flag.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.contains("..")
}

/// The whole write surface. Deliberately a **whitelist of CLI invocations**, not a shell:
/// every action runs the same binary a user would run, so verification, policy and exit
/// codes are the ones already tested.
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

/// Run an action and stream its output as it arrives, so a download reports progress
/// instead of holding the connection silent for a minute.
fn stream_argv(stream: &mut std::net::TcpStream, argv: Option<Vec<String>>) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Cache-Control: no-store\r\nTransfer-Encoding: chunked\r\n\
         Connection: close\r\n\r\n"
    );
    let chunk = |stream: &mut std::net::TcpStream, text: &str| {
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
    let mut child = match std::process::Command::new(exe)
        .args(&argv)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            chunk(stream, &format!("could not start: {e}\n"));
            let _ = write!(stream, "0\r\n\r\n");
            return;
        }
    };
    // stderr is collected off-thread so a chatty error stream cannot deadlock the pipe.
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
        let err = handle.join().unwrap_or_default();
        let text = tail(&err, 4096);
        if !text.is_empty() {
            chunk(stream, &text);
        }
    }
    chunk(stream, &format!("\n[exit {code}]\n"));
    let _ = write!(stream, "0\r\n\r\n");
    let _ = stream.flush();
}

/// `jhol inspect`, in-process: a header read is cheap enough that shelling out would cost
/// more than the work, and a remote target costs one ranged request.
fn inspect_json(root: &Path, target: &str) -> serde_json::Value {
    // A repository has no header; list its weights (or inspect the only one).
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
        "params_human": r.params.to_string(),
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

/// The MCP bridge. An agent and a human drive **one** surface: the same five actions, the
/// same status struct, and the same tool catalog — over JSON-RPC, in-process, with no
/// second server to keep in sync.
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
        // A notification gets no response, per JSON-RPC.
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
   color-scheme: dark;
   --bg:#0b0d10; --panel:#111419; --panel2:#151920; --line:#1e242c; --line2:#2a323c;
   --txt:#e7eaef; --mut:#8b95a3; --dim:#5f6b7a;
   --ok:#4ade80; --bad:#f87171; --warn:#fbbf24; --accent:#60a5fa; --violet:#a78bfa;
   --mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,monospace;
   --sans:-apple-system,BlinkMacSystemFont,"Inter","Segoe UI",system-ui,sans-serif;
 }
 *{box-sizing:border-box}
 html,body{margin:0}
 body{background:var(--bg);color:var(--txt);font:14px/1.55 var(--sans);
      -webkit-font-smoothing:antialiased}
 a{color:var(--accent);text-decoration:none} a:hover{text-decoration:underline}
 .wrap{max-width:1160px;margin:0 auto;padding:0 26px 64px}

 /* ── top bar ───────────────────────────────────────────── */
 header.top{position:sticky;top:0;z-index:20;display:flex;align-items:center;gap:14px;
   padding:14px 26px;background:rgba(11,13,16,.86);backdrop-filter:saturate(140%) blur(10px);
   border-bottom:1px solid var(--line)}
 .brand{font:600 15px/1 var(--mono);letter-spacing:-.02em;color:var(--txt);
   padding:5px 9px;border:1px solid var(--line2);border-radius:7px;background:var(--panel2)}
 .proj{display:flex;align-items:baseline;gap:8px;min-width:0}
 .proj b{font-weight:600;font-size:14.5px}
 .proj span{color:var(--mut);font:12px/1 var(--mono)}
 .pills{margin-left:auto;display:flex;gap:6px;flex-wrap:wrap;justify-content:flex-end}

 /* ── pills + badges ────────────────────────────────────── */
 .pill,.b{display:inline-flex;align-items:center;gap:5px;border-radius:999px;
   padding:3px 9px;font-size:11.5px;line-height:1.4;border:1px solid var(--line2);
   color:var(--mut);background:var(--panel2);white-space:nowrap}
 .b.ok,.pill.ok{color:#bbf7d0;border-color:#1d5c37;background:#0f2a1c}
 .b.bad,.pill.bad{color:#fecaca;border-color:#6b2525;background:#2a1212}
 .b.warn,.pill.warn{color:#fde68a;border-color:#5c4a15;background:#2a220e}
 .b.acc,.pill.acc{color:#bfdbfe;border-color:#1e3f6b;background:#0f1d2e}
 .b.dot::before{content:"";width:6px;height:6px;border-radius:50%;background:currentColor;
   display:inline-block;opacity:.9}

 /* ── tabs ──────────────────────────────────────────────── */
 nav.tabs{display:flex;gap:4px;flex-wrap:wrap;padding:18px 0 20px}
 nav.tabs button{appearance:none;border:1px solid transparent;background:transparent;
   color:var(--mut);font:inherit;font-size:13.5px;padding:7px 12px;border-radius:8px;
   cursor:pointer;display:inline-flex;align-items:center;gap:7px}
 nav.tabs button:hover{background:var(--panel);color:var(--txt)}
 nav.tabs button.on{background:var(--panel2);border-color:var(--line2);color:var(--txt)}
 nav.tabs .n{font:11px/1 var(--mono);color:var(--dim);background:#0d1014;
   border:1px solid var(--line);border-radius:6px;padding:2px 5px}
 nav.tabs button.on .n{color:var(--accent);border-color:#1e3f6b}

 /* ── sections ──────────────────────────────────────────── */
 section{display:none} section.on{display:block;animation:fade .16s ease}
 @keyframes fade{from{opacity:0;transform:translateY(2px)}to{opacity:1;transform:none}}
 .card{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:18px 20px}
 .card+ .card{margin-top:14px}
 h2{font:600 13px/1 var(--sans);letter-spacing:.06em;text-transform:uppercase;
    color:var(--dim);margin:0 0 14px}
 .grid2{display:grid;grid-template-columns:repeat(auto-fit,minmax(300px,1fr));gap:14px}
 .kv{display:grid;grid-template-columns:104px 1fr;gap:10px 14px;align-items:baseline}
 .kv dt{color:var(--mut);font-size:12.5px}
 .kv dd{margin:0}
 .mono{font-family:var(--mono);font-size:12.5px;font-variant-numeric:tabular-nums}
 .muted{color:var(--mut)} .dim{color:var(--dim)}
 .empty{color:var(--dim);padding:22px 0;text-align:center;font-size:13px}

 /* ── tables ────────────────────────────────────────────── */
 table{width:100%;border-collapse:collapse;table-layout:fixed}
 th{font:500 11px/1 var(--sans);letter-spacing:.07em;text-transform:uppercase;
    color:var(--dim);text-align:left;padding:0 10px 9px;border-bottom:1px solid var(--line)}
 td{padding:11px 10px;border-bottom:1px solid var(--line);vertical-align:top}
 tr:last-child td{border-bottom:0}
 tbody tr:hover{background:#0e1216}
 td.num{font-family:var(--mono);font-variant-numeric:tabular-nums;text-align:right;
        white-space:nowrap;color:#cfd7e3}
 td.name{font-family:var(--mono);font-size:12.5px;word-break:break-all}
 .reasons{margin:6px 0 0;padding:0;list-style:none}
 .reasons li{color:var(--mut);font-size:12.3px;line-height:1.5}
 .reasons li::before{content:"·";color:var(--dim);margin-right:6px}

 /* ── forms ─────────────────────────────────────────────── */
 .row{display:flex;gap:10px;align-items:center;flex-wrap:wrap}
 input[type=text],input:not([type]){background:#0c0f13;border:1px solid var(--line2);
   color:var(--txt);border-radius:9px;padding:10px 12px;font:13px/1.4 var(--mono);
   width:100%;outline:none;transition:border-color .15s}
 input:focus{border-color:#2f5d9a;box-shadow:0 0 0 3px rgba(96,165,250,.12)}
 label.check{display:inline-flex;align-items:center;gap:8px;color:var(--mut);font-size:13px;
   cursor:pointer;user-select:none;white-space:nowrap}
 button.btn{appearance:none;font:500 13px/1 var(--sans);color:var(--txt);
   background:var(--panel2);border:1px solid var(--line2);border-radius:9px;
   padding:10px 15px;cursor:pointer;display:inline-flex;align-items:center;gap:7px;
   transition:border-color .15s,background .15s}
 button.btn:hover{border-color:#3b4855;background:#191e26}
 button.btn.primary{background:#132538;border-color:#24507f;color:#dbeafe}
 button.btn.primary:hover{background:#16304a}
 button.btn[disabled]{opacity:.5;cursor:progress}
 .acts{display:flex;flex-direction:column;gap:0}
 .act{display:flex;align-items:center;gap:14px;padding:12px 2px;border-bottom:1px solid var(--line)}
 .act:last-child{border-bottom:0}
 .act .t{font-family:var(--mono);font-size:12.5px;color:var(--txt);min-width:210px}
 .act .d{color:var(--mut);font-size:12.5px;flex:1}
 .act .btn{padding:7px 13px;font-size:12.5px}
 .act.ro .t{color:var(--txt)}
 .kbd{font:11px/1 var(--mono);color:var(--dim);border:1px solid var(--line2);
      border-radius:4px;padding:2px 4px}
 .searchwrap{display:flex;gap:12px;align-items:center;margin-bottom:14px;flex-wrap:wrap}
 .searchwrap input{flex:0 1 460px;min-width:220px}

 /* ── output / inspect ──────────────────────────────────── */
 pre{background:#0a0c0f;border:1px solid var(--line);border-radius:10px;
   padding:14px 16px;font:12.4px/1.62 var(--mono);color:#c8d1dc;overflow:auto;
   max-height:22rem;white-space:pre-wrap;margin:0}
 pre.tail{margin-top:12px}
 .bars{margin-top:14px;display:grid;gap:8px}
 .barrow{display:grid;grid-template-columns:150px 1fr 76px;gap:12px;align-items:center;
         font-size:12.4px}
 .barrow .nm{font-family:var(--mono);color:var(--mut);overflow:hidden;text-overflow:ellipsis;
             white-space:nowrap}
 .bar{height:7px;background:#161b22;border-radius:99px;overflow:hidden;display:flex}
 .bar>span{display:block;height:100%;background:linear-gradient(90deg,#2563eb,#22d3ee)}
 .bar>span.warn{background:linear-gradient(90deg,#b45309,#fbbf24)}
 .bar>span.ok{background:linear-gradient(90deg,#15803d,#4ade80)}
 .barrow .v{text-align:right;font-family:var(--mono);color:#cfd7e3;font-variant-numeric:tabular-nums}
 .stat{display:flex;gap:22px;flex-wrap:wrap;margin-top:4px}
 .stat div{display:flex;flex-direction:column;gap:2px}
 .stat .v{font:600 20px/1.2 var(--sans);font-variant-numeric:tabular-nums}
 .stat .k{font-size:11.5px;color:var(--dim);text-transform:uppercase;letter-spacing:.06em}
 .hint{color:var(--dim);font-size:12.5px;margin-top:10px}
 code{font-family:var(--mono);font-size:12.3px;color:#a5d6ff;background:#0e141b;
      border:1px solid var(--line);border-radius:5px;padding:1px 5px}
 @media (max-width:720px){
   .wrap{padding:0 14px 48px} header.top{padding:12px 14px}
   .pills{display:none} .kv{grid-template-columns:92px 1fr}
   .stat .v{font-size:17px}
 }
</style></head><body>
<header class="top">
  <div class="brand">jhol</div>
  <div class="proj"><b id="name">…</b><span id="version"></span></div>
  <div class="pills" id="pills"></div>
</header>

<div class="wrap">
  <nav class="tabs" id="nav"></nav>

  <section id="s-status" class="on">
    <div class="grid2">
      <div class="card"><h2>Project</h2><dl class="kv" id="k-project"></dl></div>
      <div class="card"><h2>Artefacts</h2><div id="k-artefacts"></div></div>
      <div class="card"><h2>Host</h2><dl class="kv" id="k-host"></dl></div>
    </div>
  </section>

  <section id="s-artifacts">
    <div class="card">
      <div class="searchwrap">
        <input id="q" placeholder="Filter by name, kind, reason…" autocomplete="off">
        <label class="check"><input type="checkbox" id="unfit"> Only what does not fit</label>
      </div>
      <table>
        <colgroup>
          <col style="width:58px"><col style="width:86px"><col style="width:24%">
          <col style="width:104px"><col style="width:104px"><col>
        </colgroup>
        <thead><tr><th>Fit</th><th>Kind</th><th>Name</th><th style="text-align:right">Size</th>
        <th style="text-align:right">Resident</th><th>Why</th></tr></thead>
        <tbody id="artifacts"></tbody>
      </table>
    </div>
    <div class="card" style="margin-top:14px">
      <h2>Inspect a header</h2>
      <div class="row">
        <input id="path" style="flex:1"
          placeholder="path · pinned model · hf://org/repo/file.gguf · https://huggingface.co/…">
        <button class="btn primary" id="go">Inspect</button>
      </div>
      <div id="detail-host" style="margin-top:14px"></div>
      <p class="hint">Reads the header only: parameters, dtypes and resident size without the weights.</p>
    </div>
  </section>

  <section id="s-budget">
    <div class="card"><h2>Download plan</h2><div id="budget"></div></div>
  </section>

  <section id="s-proof">
    <div class="grid2">
      <div class="card"><h2>Reproducibility</h2><dl class="kv" id="proof-kv"></dl></div>
      <div class="card"><h2>Policy in force</h2><dl class="kv" id="policy-kv"></dl></div>
    </div>
    <div class="card" style="margin-top:14px"><h2>Forks</h2><div id="forks"></div></div>
  </section>

  <section id="s-agent">
    <div class="card"><h2>Agent surface</h2><div id="agent"></div>
      <p class="hint">The same tools the MCP server answers — <code>jhol mcp --stdio</code>
      or <code>POST /mcp</code> on this page.</p></div>
  </section>

  <section id="s-actions">
    <div class="card">
      <h2>Run</h2>
      <div class="acts" id="acts"></div>
      <p class="hint" id="arm-hint">Writes ask once — the first click shows what would happen, the second runs it.</p>
      <pre id="out" class="tail" style="min-height:190px">Output streams here.</pre>
    </div>
  </section>
  <footer class="dim" style="margin-top:34px;font:12px/1.6 var(--mono)">
    same data as <code>jhol status --json</code> · <code>jhol status --watch</code> ·
    MCP on <code>POST /mcp</code> · <span class="kbd">1</span>–<span class="kbd">6</span> to switch,
    <span class="kbd">/</span> to filter
  </footer>
</div>

<script>
const token = new URLSearchParams(location.search).get("t") || "";
const SECTIONS = ["status","artifacts","budget","proof","agent","actions"];
const LABEL = {status:"Status",artifacts:"Artefacts",budget:"Budget",proof:"Proof",
               agent:"Agent",actions:"Actions"};
const nav = document.getElementById("nav");
const counts = {};
SECTIONS.forEach((s, i) => {
  const b = document.createElement("button");
  b.dataset.section = s;
  if (i === 0) b.className = "on";
  b.innerHTML = `${LABEL[s]}<span class="n" id="n-${s}">·</span>`;
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
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(v >= 10 ? 1 : 2)} ${u[i]}`;
}
function params(n) {
  if (!n) return "—";
  if (n >= 1e9) return `${(n/1e9).toFixed(2)} B`;
  if (n >= 1e6) return `${(n/1e6).toFixed(1)} M`;
  if (n >= 1e3) return `${(n/1e3).toFixed(1)} K`;
  return String(n);
}
const badge = (kind, text) => `<span class="b ${kind}">${esc(text)}</span>`;
const fitBadge = f => f === true ? badge("ok", "✓ fits")
  : f === false ? badge("bad", "✗ no") : badge("warn", "? unknown");

function kv(el, rows) {
  el.innerHTML = rows.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${v}</dd>`).join("");
}

function render(s) {
  document.getElementById("name").textContent = s.project || "(unnamed)";
  document.getElementById("version").textContent = s.version || "";
  const unfit = s.artifacts.filter(a => a.fits === false).length;
  const unknown = s.artifacts.filter(a => a.fits === null || a.fits === undefined).length;
  document.getElementById("pills").innerHTML = [
    s.tree.state === "match" ? badge("ok dot", `tree match · ${s.tree.packages}`) : badge("bad dot", `tree ${s.tree.state}`),
    s.manifest_matches_lockfile ? badge("ok dot", "lockfile ok") : badge("bad dot", "lockfile drifted"),
    s.plan.missing_bytes ? badge("warn dot", `${bytes(s.plan.missing_bytes)} missing`) : badge("dot", "nothing missing"),
    unfit ? badge("bad dot", `${unfit} unfit`) : badge("ok dot", "all fit"),
    s.forks.length ? badge("acc dot", `${s.forks.length} fork${s.forks.length>1?"s":""}`) : "",
  ].join("");

  counts.status = s.tree.packages; counts.artifacts = s.artifacts.length;
  counts.budget = s.plan.items.filter(i => !i.present).length;
  counts.proof = s.proof.attested_packages; counts.agent = s.agent.tools.length;
  counts.actions = 5;
  SECTIONS.forEach(k => { const el = document.getElementById("n-" + k);
    if (el) el.textContent = counts[k] ?? "·"; });

  kv(document.getElementById("k-project"), [
    ["tree", s.tree.state === "match"
      ? `${badge("ok","match")} <span class="muted">${s.tree.packages} packages, all version-matched</span>`
      : `${badge("bad", esc(s.tree.state))} <span class="muted">${esc(s.tree.detail)}</span>`],
    ["lockfile", s.manifest_matches_lockfile ? `${badge("ok","current")}`
      : `${badge("bad","drifted")} <span class="muted">run install</span>`],
    ["snapshot", s.proof.snapshot_ready ? badge("ok","ready") : badge("warn","absent")],
    ["attested", `<span class="mono">${s.proof.attested_packages}</span> packages ·
                  <span class="mono">${s.proof.attested_models}</span> models`],
  ]);

  const n = s.artifacts.length;
  document.getElementById("k-artefacts").innerHTML = n ? `
    <div class="stat">
      <div><span class="v">${n}</span><span class="k">artefacts</span></div>
      <div><span class="v" style="color:var(--ok)">${n - unfit - unknown}</span><span class="k">fit</span></div>
      <div><span class="v" style="color:var(--bad)">${unfit}</span><span class="k">won't run</span></div>
      <div><span class="v" style="color:var(--warn)">${unknown}</span><span class="k">unknown</span></div>
    </div>` : `<div class="empty">No wheels pinned and no weights locked yet.</div>`;

  kv(document.getElementById("k-host"), [
    ["os / arch", `<span class="mono">${esc(s.axes.os)}/${esc(s.axes.arch)}</span>`],
    ["libc", `<span class="mono">${esc(s.axes.libc)}</span>`],
    ["python", `<span class="mono">${esc(s.axes.python || "not pinned")}</span>`],
    ["cuda", s.axes.cuda_driver ? `<span class="mono">${esc(s.axes.cuda_driver)}</span>
      ${s.axes.sm.length ? badge("acc", "sm " + s.axes.sm.join(",")) : ""}`
      : `<span class="muted">no driver</span>`],
    ["gpu memory", s.axes.gpu_mem_mb ? `<span class="mono">${bytes(s.axes.gpu_mem_mb*1048576)}</span>`
      : `<span class="muted">not detected — weight fit stays unknown</span>`],
  ]);

  renderArtifacts(s);
  renderBudget(s);
  renderProof(s);
  renderForks(s);
  renderAgent(s);
}

function renderArtifacts(s) {
  const tb = document.getElementById("artifacts");
  tb.innerHTML = s.artifacts.map(a => `
    <tr data-fit="${a.fits}">
      <td>${fitBadge(a.fits)}</td>
      <td>${badge("", a.kind)}</td>
      <td class="name">${esc(a.name)}<div class="dim mono" style="font-size:11.5px">${esc(a.version || "")}</div></td>
      <td class="num">${bytes(a.bytes)}</td>
      <td class="num">${bytes(a.vram_bytes)}</td>
      <td><ul class="reasons">${(a.reasons||[]).map(r => `<li>${esc(r)}</li>`).join("")}</ul></td>
    </tr>`).join("") ||
    `<tr><td colspan="6"><div class="empty">No artefacts. Install npm packages, pin wheels, or pin a model.</div></td></tr>`;
  applyFilter();
}

function renderBudget(s) {
  const cached = s.plan.cached_bytes, missing = s.plan.missing_bytes;
  const total = cached + missing || 1;
  const pct = Math.round(cached * 100 / total);
  const missingFiles = s.plan.items.filter(i => !i.present);
  document.getElementById("budget").innerHTML = `
    <div class="stat">
      <div><span class="v">${bytes(missing)}</span><span class="k">to download</span></div>
      <div><span class="v">${bytes(cached)}</span><span class="k">already cached</span></div>
      <div><span class="v">${missing ? s.plan.eta_seconds + " s" : "0 s"}</span>
        <span class="k">at ${Math.round(s.plan.throughput_bytes_per_sec/1048576)} MB/s</span></div>
      <div><span class="v">${missingFiles.length}</span><span class="k">file${missingFiles.length===1?"":"s"} missing</span></div>
    </div>
    <div class="bars">
      <div class="barrow"><span class="nm">${pct}% cached</span>
        <span class="bar"><span class="${pct===100?"ok":""}" style="width:${Math.max(pct,1)}%"></span></span>
        <span class="v">${bytes(cached)}</span></div>
    </div>
    ${missingFiles.length ? `<table style="margin-top:16px">
      <thead><tr><th>Missing file</th><th style="text-align:right;width:120px">Bytes</th></tr></thead>
      <tbody>${missingFiles.map(i => `<tr><td class="name">${esc(i.name)}</td>
        <td class="num">${bytes(i.bytes)}</td></tr>`).join("")}</tbody></table>`
      : `<div class="empty">Nothing to fetch — everything the lockfile pins is on disk.</div>`}`;
}

function renderProof(s) {
  kv(document.getElementById("proof-kv"), [
    ["snapshot", s.proof.snapshot_ready ? `${badge("ok","ready")} <span class="muted">next install is a rename</span>`
      : `${badge("warn","absent")} <span class="muted">next install materialises from the lockfile</span>`],
    ["attested", `<span class="mono">${s.proof.attested_packages}</span> packages ·
                  <span class="mono">${s.proof.attested_models}</span> models`],
    ["models", s.models.pinned ? s.models.names.map(x => badge("acc", x)).join(" ")
      : `<span class="muted">none pinned</span>`],
    ["python", s.python.pinned ? `<span class="mono">${s.python.pinned}</span> distributions`
      : `<span class="muted">none pinned</span>`],
  ]);
  kv(document.getElementById("policy-kv"), [
    ["as-of", s.policy.as_of ? `<span class="mono">${esc(s.policy.as_of)}</span>` : `<span class="muted">latest</span>`],
    ["licences", (s.policy.allowed_licenses||[]).length
      ? s.policy.allowed_licenses.map(l => badge("", l)).join(" ") : `<span class="muted">any</span>`],
    ["vulnerable", s.policy.exclude_vulnerable ? badge("ok","excluded") : `<span class="muted">not filtered</span>`],
  ]);
}

function renderForks(s) {
  const el = document.getElementById("forks");
  if (!s.forks.length) {
    el.innerHTML = `<div class="empty">No forks — <code>jhol env fork &lt;name&gt;</code>
      copies an environment in milliseconds.</div>`;
    return;
  }
  el.innerHTML = `<table><thead><tr><th>Fork</th>
    <th style="text-align:right;width:150px">Beyond base</th><th style="width:130px"></th></tr></thead>
    <tbody>${s.forks.map(f => `<tr><td class="name">${esc(f.name)}</td>
      <td class="num">${bytes(f.bytes)}</td>
      <td><button class="btn" data-p="${esc(f.name)}">Promote</button></td></tr>`).join("")}</tbody></table>`;
  el.querySelectorAll("button[data-p]").forEach(b => {
    b.onclick = () => armOrRun("promote", b.dataset.p, true, "jhol env promote " + b.dataset.p);
  });
}

function renderAgent(s) {
  document.getElementById("agent").innerHTML = `<table>
    <thead><tr><th style="width:250px">Tool</th><th style="width:100px">Cost</th><th>Purpose</th></tr></thead>
    <tbody>${s.agent.tools.map(t => `<tr>
      <td class="name">${esc(t.name)}</td>
      <td>${badge(t.cost === "write" ? "warn" : t.cost === "network" ? "acc" : "ok", t.cost)}</td>
      <td class="muted">${esc(t.purpose)}</td></tr>`).join("")}</tbody></table>`;
}

/* ── inspect ─────────────────────────────────────────────── */
function renderDetail(j) {
  const host = document.getElementById("detail-host");
  if (j.error) { host.innerHTML = `<pre style="border-color:#6b2525">${esc(j.error)}</pre>`; return; }
  if (j.files) {
    host.innerHTML = `<table><thead><tr><th>Weight file</th>
      <th style="text-align:right;width:120px">Bytes</th></tr></thead>
      <tbody>${j.files.map(f => `<tr><td class="name"><a href="#" data-file="${esc(f.name)}">${esc(f.name)}</a></td>
        <td class="num">${bytes(f.bytes)}</td></tr>`).join("")}</tbody></table>
      <p class="hint">${esc(j.repository)} · ${j.files.length} weight file(s) — click one to read its header.</p>`;
    host.querySelectorAll("a[data-file]").forEach(a => {
      a.onclick = e => {
        e.preventDefault();
        document.getElementById("path").value =
          j.repository.replace(/\/$/, "") + "/" + a.dataset.file;
        document.getElementById("go").click();
      };
    });
    return;
  }
  const dtypes = (j.dtypes || []).map(d => `${d.dtype}×${d.tensors}`).join("  ");
  const share = j.vram_bytes ? Math.min(100, (j.data_bytes / (j.data_bytes + 1)) * 100) : 0;
  host.innerHTML = `
    <div class="row" style="margin-bottom:12px">
      <span class="mono" style="color:var(--mut);word-break:break-all">${esc(j.path)}</span>
      ${j.format === "gguf" ? badge("acc", "gguf") : badge("", "safetensors")}
      ${j.architecture ? badge("", j.architecture) : ""}
      ${j.fits === true ? badge("ok", "fits here") : j.fits === false ? badge("bad", "too big for this GPU") : badge("warn", "fit unknown")}
    </div>
    <div class="stat">
      <div><span class="v">${params(j.params)}</span><span class="k">parameters</span></div>
      <div><span class="v">${bytes(j.data_bytes)}</span><span class="k">tensor data</span></div>
      <div><span class="v">${bytes(j.vram_bytes)}</span><span class="k">resident est.</span></div>
      <div><span class="v">${j.tensors}</span><span class="k">tensors</span></div>
      <div><span class="v">${bytes(j.bytes_read)}</span><span class="k">read${j.requests ? ` · ${j.requests} req` : ""}</span></div>
    </div>
    <div class="row" style="margin-top:10px">
      <span class="mono muted">${esc(dtypes || "—")}</span>
      <span class="hint" style="margin:0">header read ${((j.bytes_read/(j.bytes_read+j.data_bytes||1))*100).toFixed(3)}% of the file</span>
    </div>
    <div class="bars">
      ${(j.largest||[]).slice(0,5).map(t => {
        const w = Math.max(2, Math.round(t.bytes / j.largest[0].bytes * 100));
        return `<div class="barrow"><span class="nm">${esc(t.name)}</span>
          <span class="bar"><span style="width:${w}%"></span></span>
          <span class="v">${bytes(t.bytes)}</span></div>`; }).join("")}
    </div>`;
}

document.getElementById("go").onclick = async () => {
  const p = document.getElementById("path").value.trim();
  if (!p) return;
  const host = document.getElementById("detail-host");
  host.innerHTML = `<div class="empty">Reading header…</div>`;
  try {
    const r = await fetch(`/inspect?t=${encodeURIComponent(token)}&path=${encodeURIComponent(p)}`);
    renderDetail(await r.json());
  } catch (e) { host.innerHTML = `<pre style="border-color:#6b2525">${esc(e.message)}</pre>`; }
};
document.getElementById("path").addEventListener("keydown", e => {
  if (e.key === "Enter") document.getElementById("go").click();
});

/* ── filtering ───────────────────────────────────────────── */
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
const armed = {key: null, at: 0, plan: ""};
async function armOrRun(action, name, stream, label) {
  const key = action + ":" + name;
  if (armed.key !== key || Date.now() - armed.at > 10000) {
    let preview = {writes: true, summary: armed.plan};
    try {
      preview = await (await fetch(`/plan?action=${encodeURIComponent(action)}`)).json();
    } catch (e) {}
    if (preview.writes) {
      armed.key = key; armed.at = Date.now();
      show("actions");
      document.getElementById("out").textContent =
        `${label}\n  ${preview.summary}\n\n  click again within 10s to run`;
      document.getElementById("arm-hint").textContent = "Armed — second click runs it.";
      return;
    }
  }
  armed.key = null;
  document.getElementById("arm-hint").textContent = "Writes ask once.";
  run(action, name, stream, label);
}

async function run(action, name, stream, label) {
  show("actions");
  const out = document.getElementById("out");
  document.querySelectorAll(".acts button,button[data-p]").forEach(x => x.disabled = true);
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
    document.querySelectorAll(".acts button,button[data-p]").forEach(x => x.disabled = false);
  }
}

const ACTIONS = [
  ["verify", "jhol verify", "Compare the installed tree with the lockfile. Reads only."],
  ["prune", "jhol verify --prune", "Verify, then remove packages the lockfile does not authorise."],
  ["install", "jhol install", "Materialise the tree from the lockfile; fetches only what is missing."],
  ["ci", "jhol ci", "Reproduce the lockfile exactly; refuses to write a drifting lockfile."],
  ["policy", "jhol install --exclude-vuln", "Install with this project's policy in force."],
];
document.getElementById("acts").innerHTML = ACTIONS.map(([k, cmd, why]) => `
  <div class="act"><span class="t">${esc(cmd)}</span>
    <span class="d">${esc(why)}</span>
    <button class="btn" data-a="${k}" data-say="${esc(cmd)}">Run</button></div>`).join("");
document.querySelectorAll(".acts button[data-a]").forEach(b => {
  b.onclick = () => armOrRun(b.dataset.a, "", true, b.dataset.say);
});

/* ── polling with ETag ───────────────────────────────────── */
let etag = null;
async function tick() {
  try {
    const r = await fetch("/status.json", {headers: etag ? {"If-None-Match": etag} : {}});
    if (r.status === 304) return;
    etag = r.headers.get("ETag");
    render(await r.json());
  } catch (e) { /* server restarting */ }
}
tick(); setInterval(tick, 1500);
</script></body></html>
"##;

#[cfg(test)]
mod bridge_tests {
    use super::*;

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
        assert!(tools.len() >= 10, "catalog bridge must expose the real tool set");
        assert!(tools.iter().any(|t| t["name"] == "jhol_axes"));
    }
}
