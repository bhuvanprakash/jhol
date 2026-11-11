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
        let given = path
            .split_once("t=")
            .map(|(_, t)| t.split('&').next().unwrap_or(t).to_string())
            .unwrap_or_default();

        if path.starts_with("/action") {
            let body = read_body(&mut reader);
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
            let body = read_body(&mut reader);
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
            let status = crate::status_view::enrich(
                jhol_core::status::collect(Path::new(".")),
                Path::new("."),
            );
            respond(
                &mut stream,
                "application/json",
                &serde_json::to_string(&status).unwrap_or_else(|e| e.to_string()),
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

fn read_body(reader: &mut BufReader<std::net::TcpStream>) -> Vec<u8> {
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut raw = vec![0u8; length.min(8192)];
    let _ = reader.read_exact(&mut raw);
    raw
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
    let inspected = match crate::status_view::inspect_target(root, target) {
        Ok(i) => i,
        Err(e) => return serde_json::json!({ "error": e }),
    };
    let s = &inspected.summary;
    let host = jhol_core::axes::HostAxes::detect();
    serde_json::json!({
        "path": inspected.display,
        "tensors": s.tensors,
        "params": s.params,
        "params_human": s.params_human(),
        "data_bytes": s.data_bytes,
        "vram_bytes": s.vram_bytes(),
        "header_bytes": s.header_bytes,
        "bytes_read": inspected.bytes_read,
        "requests": inspected.requests,
        "dtypes": s.dtypes.iter().map(|(d, n)| serde_json::json!({"dtype": d, "tensors": n})).collect::<Vec<_>>(),
        "gpu_mem_mb": host.gpu_mem_mb,
        "fits": host.gpu_mem_mb.map(|m| s.vram_bytes().div_ceil(1024 * 1024) <= m),
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

const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>jhol</title>
<style>
 :root { color-scheme: dark }
 body { font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; margin: 0;
        background:#111; color:#e8e8e8; padding: 1.4rem 1.8rem; }
 h1 { font-size: 1.15rem; margin: 0 }
 .sub { color:#8a8a8a; margin-bottom: 1rem }
 nav { display:flex; gap:.35rem; flex-wrap:wrap; margin: 1rem 0 1.2rem }
 nav button { background:#161616; border:1px solid #262626; color:#bbb; border-radius:5px;
              padding:.3rem .65rem; font:inherit; cursor:pointer }
 nav button.on { border-color:#5bd67b; color:#5bd67b }
 section { display:none } section.on { display:block }
 .row { display:flex; gap:1rem; padding:.18rem 0 }
 .k { width:9.5rem; color:#5bd67b; flex:none }
 table { border-collapse:collapse; width:100% }
 th, td { text-align:left; padding:.28rem .5rem; border-bottom:1px solid #1e1e1e }
 th { color:#888; font-weight:400 }
 .ok { color:#5bd67b } .bad { color:#ff6b6b } .unk { color:#e3b341 }
 .acts { margin: 1.2rem 0 .6rem; display:flex; gap:.5rem; flex-wrap:wrap }
 .acts button { background:#1b1b1b; color:#e8e8e8; border:1px solid #333; border-radius:5px;
                padding:.35rem .7rem; font:inherit; cursor:pointer }
 .acts button:hover { border-color:#5bd67b }
 .acts button[disabled] { opacity:.5; cursor:progress }
 pre { background:#0d0d0d; border:1px solid #1e1e1e; border-radius:6px; padding:.7rem;
       min-height:3rem; max-height:20rem; overflow:auto; white-space:pre-wrap }
 input { background:#0d0d0d; border:1px solid #2a2a2a; color:#e8e8e8; border-radius:5px;
         padding:.35rem .6rem; font:inherit; width: 60%; min-width: 16rem }
 code { color:#9ad0ff }
 .muted { color:#777 }
</style></head><body>
<h1 id="name">jhol</h1>
<div class="sub" id="version">connecting…</div>
<nav id="nav"></nav>

<section id="s-status" class="on"></section>
<section id="s-artifacts">
  <table><thead><tr><th>fit</th><th>kind</th><th>name</th><th>size</th><th>resident</th><th>why</th></tr></thead>
  <tbody id="artifacts"></tbody></table>
  <p><input id="path" placeholder="path, directory, pinned model name, or hf://org/repo/file">
     <button id="go">inspect</button></p>
  <pre id="detail">Header looks read a few kilobytes, not the weights.</pre>
</section>
<section id="s-budget"></section>
<section id="s-proof"></section>
<section id="s-forks"></section>
<section id="s-agent"></section>
<section id="s-actions">
  <div class="acts">
    <button data-a="verify">verify</button>
    <button data-a="prune">verify --prune</button>
    <button data-a="install">install</button>
    <button data-a="ci">ci</button>
    <button data-a="policy">install --exclude-vuln</button>
  </div>
  <pre id="out">Output streams as it arrives.</pre>
</section>

<script>
const token = new URLSearchParams(location.search).get("t") || "";
const SECTIONS = ["status","artifacts","budget","proof","forks","agent","actions"];
const nav = document.getElementById("nav");
SECTIONS.forEach((s, i) => {
  const b = document.createElement("button");
  b.textContent = s;
  if (i === 0) b.className = "on";
  b.onclick = () => {
    document.querySelectorAll("nav button").forEach(x => x.className = "");
    document.querySelectorAll("section").forEach(x => x.className = "");
    b.className = "on";
    document.getElementById("s-" + s).className = "on";
  };
  nav.appendChild(b);
});

const gb = n => (n / 1073741824).toFixed(2) + " GB";
const mb = n => (n / 1048576).toFixed(1) + " MB";
const esc = s => String(s).replace(/[&<>]/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;"}[c]));

function row(k, v, cls) {
  return `<div class="row"><span class="k">${k}</span><span class="${cls||""}">${v}</span></div>`;
}

function render(s) {
  document.getElementById("name").textContent = (s.project || "(unnamed)") + " " + (s.version || "");
  document.getElementById("version").textContent =
    `tree ${s.tree.state} · ${s.artifacts.length} artifacts · ${s.plan.items.length} plan items`;

  document.getElementById("s-status").innerHTML =
    row("tree", s.tree.state === "match" ? `match — ${s.tree.packages} packages` : `${esc(s.tree.state)} — ${esc(s.tree.detail)}`,
        s.tree.state === "match" ? "ok" : "bad") +
    row("lockfile", s.manifest_matches_lockfile ? "matches package.json" : "DRIFTED — run install",
        s.manifest_matches_lockfile ? "ok" : "bad") +
    row("policy", s.policy.as_of || s.policy.exclude_vulnerable ? "restricted" : "latest, any licence") +
    row("models", s.models.pinned ? s.models.pinned + " pinned: " + s.models.names.join(", ") : "none") +
    row("python", s.python.pinned ? s.python.pinned + " distributions" : "none") +
    row("axes", `${s.axes.os}/${s.axes.arch} ${s.axes.libc} · python ${s.axes.python || "-"} · cuda ${s.axes.cuda_driver || "none"} · sm ${(s.axes.sm||[]).join(",") || "-"} · gpu ${s.axes.gpu_mem_mb || "?"} MB`) +
    row("proof", `snapshot ${s.proof.snapshot_ready ? "ready" : "absent"} · attested ${s.proof.attested_packages} packages, ${s.proof.attested_models} models`);

  const tbody = document.getElementById("artifacts");
  tbody.innerHTML = s.artifacts.map(a => {
    const mark = a.fits === true ? '<span class="ok">✓</span>'
               : a.fits === false ? '<span class="bad">✗</span>'
               : '<span class="unk">?</span>';
    return `<tr><td>${mark}</td><td class="muted">${a.kind}</td><td>${esc(a.name)}</td>
      <td>${a.bytes ? mb(a.bytes) : "—"}</td><td>${a.vram_bytes ? gb(a.vram_bytes) : "—"}</td>
      <td class="muted">${esc(a.reasons[0] || "")}</td></tr>`;
  }).join("") || '<tr><td colspan="6" class="muted">no wheels pinned, no weights locked</td></tr>';

  const missing = s.plan.items.filter(i => !i.present);
  document.getElementById("s-budget").innerHTML =
    row("missing", s.plan.missing_bytes ? mb(s.plan.missing_bytes) : "nothing",
        s.plan.missing_bytes ? "unk" : "ok") +
    row("cached", mb(s.plan.cached_bytes)) +
    row("estimate", s.plan.missing_bytes ? `~${s.plan.eta_seconds} s at ${(s.plan.throughput_bytes_per_sec/1048576)|0} MB/s` : "—") +
    (missing.length ? `<table><thead><tr><th>missing file</th><th>bytes</th></tr></thead><tbody>` +
      missing.map(i => `<tr><td>${esc(i.name)}</td><td>${mb(i.bytes)}</td></tr>`).join("") +
      `</tbody></table>` : "");

  document.getElementById("s-proof").innerHTML =
    row("snapshot", s.proof.snapshot_ready ? "ready — the next install is a rename" : "absent") +
    row("attested", `${s.proof.attested_packages} packages, ${s.proof.attested_models} models`) +
    row("as-of", s.policy.as_of || "not pinned") +
    row("licences", (s.policy.allowed_licenses || []).join(",") || "any") +
    row("vulnerable", s.policy.exclude_vulnerable ? "excluded" : "not filtered");

  document.getElementById("s-forks").innerHTML =
    (s.forks.length ? `<table><thead><tr><th>fork</th><th>beyond base</th><th></th></tr></thead><tbody>` +
      s.forks.map(f => `<tr><td>${esc(f.name)}</td><td>${mb(f.bytes)}</td>
        <td><button data-p="${esc(f.name)}">promote</button></td></tr>`).join("") + `</tbody></table>`
      : '<p class="muted">no forks — <code>jhol env fork &lt;name&gt;</code> makes one in a few ms</p>');

  document.getElementById("s-agent").innerHTML =
    `<table><thead><tr><th>tool</th><th>cost</th><th>purpose</th></tr></thead><tbody>` +
    s.agent.tools.map(t => `<tr><td><code>${esc(t.name)}</code></td><td class="muted">${t.cost}</td>
      <td>${esc(t.purpose)}</td></tr>`).join("") + `</tbody></table>`;

  document.querySelectorAll("button[data-p]").forEach(b => {
    b.onclick = () => run("promote", b.dataset.p, true);
  });
}

async function tick() {
  try {
    const r = await fetch("/status.json");
    render(await r.json());
  } catch (e) { /* server restarting */ }
}

async function run(action, name, stream) {
  const out = document.getElementById("out");
  document.querySelectorAll(".acts button").forEach(x => x.disabled = true);
  out.textContent = `running ${action}${name ? " " + name : ""}…\n`;
  try {
    const r = await fetch(`/action?t=${encodeURIComponent(token)}${stream ? "&stream=1" : ""}`, {
      method: "POST", headers: {"content-type": "application/json"},
      body: JSON.stringify({action, name: name || ""}),
    });
    if (stream) {
      const reader = r.body.getReader();
      const dec = new TextDecoder();
      for (;;) {
        const {value, done} = await reader.read();
        if (done) break;
        out.textContent += dec.decode(value, {stream: true});
        out.scrollTop = out.scrollHeight;
      }
    } else {
      const j = await r.json();
      out.textContent = `${action} → exit ${j.code}\n${j.stdout || ""}${j.stderr || ""}`;
    }
    tick();
  } catch (e) {
    out.textContent = "error: " + e.message;
  } finally {
    document.querySelectorAll(".acts button").forEach(x => x.disabled = false);
  }
}

document.querySelectorAll("button[data-a]").forEach(b => {
  b.onclick = () => run(b.dataset.a, "", true);
});

document.getElementById("go").onclick = async () => {
  const p = document.getElementById("path").value.trim();
  if (!p) return;
  const detail = document.getElementById("detail");
  detail.textContent = "reading header…";
  try {
    const r = await fetch(`/inspect?t=${encodeURIComponent(token)}&path=${encodeURIComponent(p)}`);
    const j = await r.json();
    if (j.error) { detail.textContent = "error: " + j.error; return; }
    const dtypes = (j.dtypes || []).map(d => `${d.dtype}×${d.tensors}`).join(" ");
    detail.textContent =
      `${j.path}\n  ${j.tensors} tensors · ${j.params_human} params · ${j.data_bytes} bytes of tensor data` +
      `\n  dtypes ${dtypes}` +
      `\n  resident ${gb(j.vram_bytes)}` +
      (j.gpu_mem_mb ? ` — host GPU ${j.gpu_mem_mb} MB → ${j.fits ? "fits" : "does not fit"}`
                    : " — no GPU memory detected, fit unknown") +
      `\n  header ${j.bytes_read} bytes read (${(100 * j.bytes_read / (j.bytes_read + j.data_bytes)).toFixed(4)}% of the file)` +
      (j.requests ? ` over ${j.requests} range request(s)` : "");
  } catch (e) { detail.textContent = "error: " + e.message; }
};

tick(); setInterval(tick, 1500);
</script></body></html>
"#;

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
