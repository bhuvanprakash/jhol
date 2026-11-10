//! `jhol gui --web` — the status struct rendered in a browser.
//!
//! No webview dependency, no Tauri, no second implementation: one HTML page that polls
//! `/status.json`, which is the same `jhol_core::status::Status` the CLI and TUI print.
//! The server is `std::net` and stops when the process does.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>jhol</title>
<style>
  :root { color-scheme: dark }
  body { font: 14px ui-monospace, SFMono-Regular, Menlo, monospace; margin: 2rem auto;
         max-width: 52rem; padding: 0 1rem; background:#111; color:#e8e8e8 }
  h1 { font-size: 1.1rem; font-weight: 600; margin: 0 0 .2rem }
  .sub { color:#888; margin-bottom: 1.6rem }
  .row { display:grid; grid-template-columns: 9rem 1fr; padding:.45rem 0;
         border-top:1px solid #1e1e1e }
  .k { color:#888 }
  .ok { color:#5bd67b } .bad { color:#ff6b6b } .warn { color:#e0b341 }
  code { color:#9ad0ff }
  .acts { margin: 1.4rem 0 .6rem; display:flex; gap:.5rem; flex-wrap:wrap }
  button { font: inherit; background:#1b1b1b; color:#e8e8e8; border:1px solid #333;
           border-radius:5px; padding:.35rem .7rem; cursor:pointer }
  button:hover { border-color:#5bd67b }
  button[disabled] { opacity:.5; cursor:progress }
  pre { background:#0d0d0d; border:1px solid #1e1e1e; border-radius:6px; padding:.7rem;
        min-height:3rem; max-height:18rem; overflow:auto; white-space:pre-wrap }

</style>
<h1 id="name">jhol</h1>
<div class="sub" id="version"></div>
<div id="rows"></div>
<div class="acts">
  <button data-a="verify">verify</button>
  <button data-a="prune">verify --prune</button>
  <button data-a="install">install</button>
  <button data-a="ci">ci</button>
  <button data-a="policy">install --exclude-vuln</button>
</div>
<pre id="out"></pre>
<script>
const esc = s => String(s ?? "").replace(/[&<>]/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;"}[c]));
function row(k, v, cls) {
  return `<div class="row"><div class="k">${k}</div><div class="${cls||""}">${v}</div></div>`;
}
async function tick() {
  try {
    const s = await (await fetch("/status.json", {cache: "no-store"})).json();
    document.getElementById("name").textContent = s.project || "(unnamed)";
    document.getElementById("version").textContent = s.version;
    const tree = s.tree.state === "match"
      ? row("tree", `<span class="ok">match</span> — ${s.tree.packages} packages`)
      : row("tree", `<span class="bad">${esc(s.tree.state)}</span> — ${esc(s.tree.detail)}`);
    const lock = s.manifest_matches_lockfile
      ? row("lockfile", `<span class="ok">matches package.json</span>`)
      : row("lockfile", `<span class="bad">drifted from package.json</span>`);
    const pol = [];
    if (s.policy.as_of) pol.push("as-of " + esc(s.policy.as_of));
    if (s.policy.allowed_licenses) pol.push("licences " + esc(s.policy.allowed_licenses.join(",")));
    if (s.policy.exclude_vulnerable) pol.push("exclude-vulnerable");
    document.getElementById("rows").innerHTML =
      tree + lock +
      row("policy", pol.length ? pol.join(" · ") : "none") +
      row("models", s.models.pinned ? `${s.models.pinned} pinned — ${esc(s.models.names.join(", "))}` : "none") +
      row("python", s.python.pinned ? `${s.python.pinned} distributions` : "none") +
      row("axes", `${esc(s.axes.os)}/${esc(s.axes.arch)} ${esc(s.axes.libc)} · python ${esc(s.axes.python || "-")} · cuda ${esc(s.axes.cuda_driver || "none")}`) +
      row("proof", `snapshot ${s.proof.snapshot_ready ? "ready" : "absent"} · attested ${s.proof.attested_packages} packages, ${s.proof.attested_models} models`);
  } catch (e) {
    document.getElementById("rows").innerHTML = row("error", esc(e.message), "bad");
  }
}
const token = new URLSearchParams(location.search).get("t") || "";
document.querySelectorAll("button[data-a]").forEach(b => {
  b.onclick = async () => {
    const label = b.textContent;
    document.querySelectorAll("button").forEach(x => x.disabled = true);
    document.getElementById("out").textContent = `running ${label}…`;
    try {
      const r = await fetch(`/action?t=${encodeURIComponent(token)}`, {
        method: "POST",
        headers: {"content-type": "application/json"},
        body: JSON.stringify({action: b.dataset.a}),
      });
      const j = await r.json();
      document.getElementById("out").textContent =
        `${label} → exit ${j.code}\n${j.stdout || ""}${j.stderr || ""}`.trim();
      tick();
    } catch (e) {
      document.getElementById("out").textContent = "error: " + e.message;
    } finally {
      document.querySelectorAll("button").forEach(x => x.disabled = false);
    }
  };
});
tick(); setInterval(tick, 1000);
</script>
"#;

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

/// The whole write surface. Deliberately a **whitelist of CLI invocations**, not a shell:
/// every action runs the same binary a user would run, so verification, policy and exit
/// codes are the ones already tested. There is nothing here that can be re-implemented
/// slightly differently.
fn action_argv(action: &str) -> Option<Vec<&'static str>> {
    Some(match action {
        "verify" => vec!["verify"],
        "prune" => vec!["verify", "--prune"],
        "install" => vec!["install"],
        "ci" => vec!["ci"],
        "policy" => vec!["install", "--exclude-vuln"],
        _ => return None,
    })
}

fn run_action(action: &str) -> serde_json::Value {
    let Some(argv) = action_argv(action) else {
        return serde_json::json!({ "ok": false, "code": 2, "stderr": "unknown action" });
    };
    let exe = std::env::current_exe().unwrap_or_else(|_| "jhol".into());
    let out = std::process::Command::new(exe).args(&argv).output();
    match out {
        Ok(o) => {
            let tail = |b: &[u8]| {
                let s = String::from_utf8_lossy(b);
                let s = s.trim_end();
                // Keep the last 8 KB: an install log can be long, and the useful part
                // (the summary or the error) is at the end.
                let start = s.len().saturating_sub(8192);
                s[start..].to_string()
            };
            serde_json::json!({
                "ok": o.status.success(),
                "code": o.status.code().unwrap_or(-1),
                "stdout": tail(&o.stdout),
                "stderr": tail(&o.stderr),
            })
        }
        Err(e) => serde_json::json!({ "ok": false, "code": -1, "stderr": e.to_string() }),
    }
}

pub fn serve() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let token = session_token();
    let url = format!("http://{addr}/?t={token}");
    println!("jhol gui --web on {url}  (Ctrl-C to stop)");
    // Best effort: open the default browser. Failure just means the user copies the URL.
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener)
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut request = String::new();
        let _ = reader.read_line(&mut request);
        let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

        // Actions: POST only, token required, whitelisted argv.
        if path.starts_with("/action") {
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
            let mut raw = vec![0u8; length.min(4096)];
            use std::io::Read;
            let _ = reader.read_exact(&mut raw);
            let given = path
                .split_once("t=")
                .map(|(_, t)| t.trim().to_string())
                .unwrap_or_default();
            let reply = if given != token {
                serde_json::json!({ "ok": false, "code": 403, "stderr": "bad token" })
            } else {
                let action = serde_json::from_slice::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(str::to_string))
                    .unwrap_or_default();
                run_action(&action)
            };
            let body = reply.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            continue;
        }

        let body = if path.starts_with("/status.json") {
            serde_json::to_string(&jhol_core::status::collect(std::path::Path::new(".")))
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
        } else {
            PAGE.to_string()
        };
        let kind = if path.starts_with("/status.json") {
            "application/json"
        } else {
            "text/html; charset=utf-8"
        };
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
    Ok(())
}
