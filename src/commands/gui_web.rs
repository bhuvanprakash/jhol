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
</style>
<h1 id="name">jhol</h1>
<div class="sub" id="version"></div>
<div id="rows"></div>
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
tick(); setInterval(tick, 1000);
</script>
"#;

pub fn serve() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let url = format!("http://{addr}/");
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
        let path = request.split_whitespace().nth(1).unwrap_or("/");
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
