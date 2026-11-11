//! Terminal UI — Interactive Ratatui Dashboard for Jhol Package Manager.

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};
use std::{
    collections::VecDeque,
    io,
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Copy, Clone, PartialEq, Eq)]
enum ActiveTab {
    Status = 0,
    Artifacts = 1,
    Fit = 2,
    Budget = 3,
    Proof = 4,
    Actions = 5,
    Dependencies = 6,
    Doctor = 7,
}

impl ActiveTab {
    fn title(&self) -> &'static str {
        match self {
            ActiveTab::Status => " 1. Status ",
            ActiveTab::Artifacts => " 2. Artifacts ",
            ActiveTab::Fit => " 3. Fit ",
            ActiveTab::Budget => " 4. Budget ",
            ActiveTab::Proof => " 5. Proof ",
            ActiveTab::Actions => " 6. Actions ",
            ActiveTab::Dependencies => " 7. Packages ",
            ActiveTab::Doctor => " 8. Health ",
        }
    }

    fn next(&self) -> Self {
        match self {
            ActiveTab::Status => ActiveTab::Artifacts,
            ActiveTab::Artifacts => ActiveTab::Fit,
            ActiveTab::Fit => ActiveTab::Budget,
            ActiveTab::Budget => ActiveTab::Proof,
            ActiveTab::Proof => ActiveTab::Actions,
            ActiveTab::Actions => ActiveTab::Dependencies,
            ActiveTab::Dependencies => ActiveTab::Doctor,
            ActiveTab::Doctor => ActiveTab::Status,
        }
    }

    fn prev(&self) -> Self {
        match self {
            ActiveTab::Status => ActiveTab::Doctor,
            ActiveTab::Artifacts => ActiveTab::Status,
            ActiveTab::Fit => ActiveTab::Artifacts,
            ActiveTab::Budget => ActiveTab::Fit,
            ActiveTab::Proof => ActiveTab::Budget,
            ActiveTab::Actions => ActiveTab::Proof,
            ActiveTab::Dependencies => ActiveTab::Actions,
            ActiveTab::Doctor => ActiveTab::Dependencies,
        }
    }
}

/// The write surface, identical to `jhol gui --web`: each entry is a CLI invocation of this
/// same binary, so verification, policy and exit codes are the ones already tested.
const ACTIONS: [(&str, &str, &[&str]); 5] = [
    ("verify", "verify  — tree vs lockfile", &["verify"]),
    ("prune", "prune   — verify and remove unauthorised", &["verify", "--prune"]),
    ("install", "install — from package.json", &["install"]),
    ("ci", "ci      — reproduce the lockfile", &["ci"]),
    ("policy", "policy  — install, exclude known CVEs", &["install", "--exclude-vuln"]),
];

/// Hidden extras. Each is free until the user triggers it: the sequence buffer is a
/// small ring, animations tick only while active, puns are picked on keypress.
struct Eggs {
    seq: VecDeque<KeyCode>,
    party_until: Option<Instant>,
    pun: Option<&'static str>,
    puns_shown: usize,
    z_count: u8,
    banner: Option<Instant>,
    show_help: bool,
}

/// The 1980s cheat code, still functional. Arrow keys also move tabs and list items,
/// so you can feel the secret being entered.
const KONAMI: [KeyCode; 10] = [
    KeyCode::Up,
    KeyCode::Up,
    KeyCode::Down,
    KeyCode::Down,
    KeyCode::Left,
    KeyCode::Right,
    KeyCode::Left,
    KeyCode::Right,
    KeyCode::Char('b'),
    KeyCode::Char('a'),
];

// Kept short on purpose: the status line is one row, so a long pun gets truncated and the joke
// lands nowhere. Each of these fits an 80-column terminal with room for the key hints.
const PUNS: [&str; 6] = [
    "Schrodinger's dependency: present and absent until `jhol ci`.",
    "A cyclic dependency walks into a bar; it never leaves.",
    "npm and bun walk into a bar. jhol is already at the source.",
    "There is no spoon, only a content-addressed cache.",
    "Tropical geometry explains weather. JAGR explains your cycles.",
    "99 bugs in the tree; patched one out, 127 bugs in the tree.",
];

const TINY_BANNER: &str = "  j h o l  ";

impl Eggs {
    fn new() -> Self {
        Self {
            seq: VecDeque::with_capacity(KONAMI.len()),
            party_until: None,
            pun: None,
            puns_shown: 0,
            z_count: 0,
            banner: None,
            show_help: false,
        }
    }

    /// Feed one keypress; returns true when the screen needs a redraw.
    fn feed(&mut self, key: KeyCode) -> bool {
        let mut fired = false;
        if key == KeyCode::Char('?') {
            self.show_help = !self.show_help;
            return true;
        }
        if key == KeyCode::Char('e') {
            self.pun = Some(PUNS[self.puns_shown % PUNS.len()]);
            self.puns_shown += 1;
            fired = true;
        }
        if key == KeyCode::Char('z') {
            self.z_count = self.z_count.saturating_add(1);
            if self.z_count >= 3 {
                self.z_count = 0;
                self.banner = Some(Instant::now() + Duration::from_secs(3));
                fired = true;
            }
        }
        if self.seq.len() == KONAMI.len() {
            self.seq.pop_front();
        }
        self.seq.push_back(key);
        if self.seq.len() == KONAMI.len() && self.seq.iter().eq(KONAMI.iter()) {
            self.party_until = Some(Instant::now() + Duration::from_secs(8));
            self.seq.clear();
            fired = true;
        }
        fired
    }

    fn party_active(&self) -> bool {
        self.party_until
            .map(|u| Instant::now() < u)
            .unwrap_or(false)
    }

    fn banner_active(&self) -> bool {
        self.banner.map(|u| Instant::now() < u).unwrap_or(false)
    }

    /// Distinct secrets found so far (shown only when JHOL_EGGS is set).
    fn found(&self) -> u8 {
        let mut n = 0;
        if self.party_until.is_some() {
            n += 1;
        }
        if self.puns_shown > 0 {
            n += 1;
        }
        if self.show_help {
            n += 1;
        }
        if self.banner.is_some() {
            n += 1;
        }
        n
    }

    /// Animated status extras, built only while an egg is active.
    fn status_extra(&self, tick: u64, width: u16) -> Option<String> {
        if self.party_active() {
            // Tag first — the status row truncates from the right, so a leading
            // animation bar used to hide "[PARTY MODE]" entirely.
            let tag = "[PARTY MODE] ";
            let w = width.saturating_sub(tag.len() as u16).max(8) as u64;
            let col = tick % w;
            let mut bar = String::from(tag);
            for i in 0..w {
                bar.push(if i == col { '>' } else { ' ' });
            }
            return Some(bar);
        }
        if self.banner_active() {
            return Some(format!("--{}--", TINY_BANNER));
        }
        None
    }
}

/// Header colours while party mode is on.
const RAINBOW: [Color; 6] = [
    Color::Red,
    Color::Yellow,
    Color::Green,
    Color::Cyan,
    Color::Blue,
    Color::Magenta,
];

/// Cheap local Instant-Lock signals (no network).
struct InstantLock {
    warm_ok: bool,
    prebuilt_key: bool,
    lockfile: bool,
    node_modules: bool,
    platform: String,
}

impl InstantLock {
    fn probe() -> Self {
        let nm = Path::new("node_modules");
        Self {
            warm_ok: nm.join(".jhol-ok").exists(),
            prebuilt_key: Path::new(".jhol-prebuilt-key").exists(),
            lockfile: Path::new("package-lock.json").exists()
                || Path::new("bun.lock").exists()
                || Path::new("yarn.lock").exists()
                || Path::new("pnpm-lock.yaml").exists(),
            node_modules: nm.is_dir(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        }
    }

    fn warm_label(&self) -> &'static str {
        if self.warm_ok {
            "armed (trampoline C sentinel exits in ~0.5ms)"
        } else if self.node_modules {
            "tree present — install once to arm .jhol-ok"
        } else {
            "cold — press i to install"
        }
    }
}

struct AppState {
    tab: ActiveTab,
    pkg_name: String,
    pkg_version: String,
    /// The same struct `jhol status`, the CLI and the web page render. The dashboard must
    /// not compute its own truth — that is how two answers to one question start.
    status: Option<jhol_core::status::Status>,
    /// Output of the last action run from tab 5.
    action_out: String,
    /// Show only artifacts that do not fit, so a failing wheel cannot hide in a list.
    only_unfit: bool,
    /// Fit tab: the budget and context the user is sizing against.
    fit_vram_gb: u64,
    fit_ctx: u64,
    fit_out: String,
    /// An action waiting for its second Enter: what was armed, and when.
    pending_action: Option<(usize, Instant)>,
    /// Live chunks from the action currently running, so a download reports progress
    /// instead of leaving the pane silent for a minute.
    stream: Option<mpsc::Receiver<String>>,
    direct_deps: Vec<(String, String)>,
    dev_deps: Vec<(String, String)>,
    cache_tarballs: usize,
    cache_bytes: u64,
    cache_pkgs: Vec<String>,
    scripts: Vec<(String, String)>,
    doctor_report: String,
    list_state: ListState,
    status_msg: String,
    jagr_info: String,
    eggs: Eggs,
    jagr_loaded: bool,
    dirty: bool,
    tick: u64,
    job_rx: Option<mpsc::Receiver<Result<String, String>>>,
    busy_label: String,
    /// Full `terminal.clear()` once after a job — recovers stderr leaks.
    recover_screen: bool,
    job_started: Option<Instant>,
    instant_lock: InstantLock,
}

impl AppState {
    fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        // No I/O here by design: the first frame is drawn before any loading, so the
        // dashboard appears immediately even on a cold project where the JAGR analysis
        // has to resolve over the network (measured: 1106 ms of blank screen when this
        // constructor did the loading; 7 ms after drawing first).
        Self {
            tab: ActiveTab::Status,
            pkg_name: "jhol-project".to_string(),
            pkg_version: "1.0.0".to_string(),
            status: None,
            action_out: String::new(),
            only_unfit: false,
            fit_vram_gb: 24,
            fit_ctx: 8192,
            fit_out: String::new(),
            pending_action: None,
            stream: None,
            direct_deps: Vec::new(),
            dev_deps: Vec::new(),
            cache_tarballs: 0,
            cache_bytes: 0,
            cache_pkgs: Vec::new(),
            scripts: Vec::new(),
            doctor_report: String::new(),
            list_state,
            status_msg: "Ready.".to_string(),
            jagr_info: String::new(),
            eggs: Eggs::new(),
            jagr_loaded: false,
            dirty: true,
            tick: 0,
            job_rx: None,
            busy_label: String::new(),
            recover_screen: false,
            job_started: None,
            instant_lock: InstantLock::probe(),
        }
    }

    /// Cheap local loads only - manifest and cache, no resolver, no network.
    fn load_initial(&mut self) {
        self.reload_manifest();
        self.reload_cache();
        self.reload_doctor_fast();
        self.instant_lock = InstantLock::probe();
        self.status_msg = format!(
            " {} · i install · d doctor · ? help · q ",
            self.instant_lock.platform
        );
    }

    /// JAGR analysis is deferred until its tab is opened (or `r` is pressed).
    fn ensure_jagr(&mut self) {
        if !self.jagr_loaded {
            self.reload_jagr();
            self.jagr_loaded = true;
        }
    }

    fn reload_manifest(&mut self) {
        if let Ok(content) = std::fs::read_to_string("package.json") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(n) = v.get("name").and_then(|s| s.as_str()) {
                    self.pkg_name = n.to_string();
                }
                if let Some(ver) = v.get("version").and_then(|s| s.as_str()) {
                    self.pkg_version = ver.to_string();
                }
                self.status = Some(crate::status_view::enrich(
                    jhol_core::status::collect(Path::new(".")),
                    Path::new("."),
                ));
                self.direct_deps.clear();
                if let Some(deps) = v.get("dependencies").and_then(|d| d.as_object()) {
                    for (k, v) in deps {
                        self.direct_deps
                            .push((k.clone(), v.as_str().unwrap_or("").to_string()));
                    }
                }
                self.dev_deps.clear();
                if let Some(devs) = v.get("devDependencies").and_then(|d| d.as_object()) {
                    for (k, v) in devs {
                        self.dev_deps
                            .push((k.clone(), v.as_str().unwrap_or("").to_string()));
                    }
                }
                self.scripts.clear();
                if let Some(scripts) = v.get("scripts").and_then(|d| d.as_object()) {
                    for (k, v) in scripts {
                        self.scripts
                            .push((k.clone(), v.as_str().unwrap_or("").to_string()));
                    }
                    self.scripts.sort_by(|a, b| a.0.cmp(&b.0));
                }
            }
        }
    }

    fn reload_cache(&mut self) {
        if let Ok((bytes, entries)) = jhol_core::cache_size_bytes() {
            self.cache_tarballs = entries;
            self.cache_bytes = bytes;
        }
        if let Ok(pkgs) = jhol_core::list_cached_packages() {
            self.cache_pkgs = pkgs;
        }
    }

    fn reload_jagr(&mut self) {
        let p = Path::new("package.json");
        if p.exists() {
            if let Ok(rep) = jhol_core::jagr_analyze_project_manifest(p) {
                let mut lines = Vec::new();
                lines.push("JAGR-Infinite Sheaf Cohomology & Topology Invariants:".to_string());
                lines.push(format!("  • Betti Invariants:       β₀={} (components), β₁={} (algebraic cycles), β₂={} (cavities)", rep.betti_0, rep.betti_1, rep.betti_2));
                lines.push(format!(
                    "  • Sheaf Spectral Gap:     {:.6}",
                    rep.spectral_gap
                ));
                lines.push(format!(
                    "  • Fiedler Connectivity:   {:.6}",
                    rep.fiedler_value
                ));
                lines.push(format!(
                    "  • Smith Normal Form:      satisfied={}, trivial={}, difficulty={}",
                    rep.snf_satisfied, rep.snf_trivial, rep.snf_difficulty
                ));
                lines.push(format!(
                    "  • Sheaf Gluing Obstruction:{}",
                    if rep.gluing_has_obstruction {
                        " DETECTED"
                    } else {
                        " none (smooth section)"
                    }
                ));
                lines.push(String::new());
                lines.push("MaxSAT & Resolution Layer:".to_string());
                lines.push(format!("  • Active Strategy:        {}", rep.strategy_used));
                lines.push(format!(
                    "  • Layer 1 MaxSAT:         satisfiable={}, cost={:.3}, variant={}",
                    rep.layer1_satisfiable, rep.layer1_cost, rep.layer1_variant
                ));
                lines.push(format!(
                    "  • Layer 3 Sheaf Filter:   satisfiable={}, kind={}",
                    rep.layer3_satisfiable, rep.layer3_kind
                ));
                lines.push(format!(
                    "  • Resolved Selection:     {} unique packages",
                    rep.selection.len()
                ));
                lines.push(String::new());
                lines.push("Resolved Tree Packages:".to_string());
                let mut sorted_sel: Vec<_> = rep.selection.iter().collect();
                sorted_sel.sort_by(|a, b| a.0.cmp(b.0));
                for (pkg, ver) in sorted_sel.iter().take(20) {
                    lines.push(format!("    {} @ {}", pkg, ver));
                }
                if sorted_sel.len() > 20 {
                    lines.push(format!("    … and {} more packages", sorted_sel.len() - 20));
                }
                self.jagr_info = lines.join("\n");
            } else {
                self.jagr_info = "JAGR analysis ready on next install.".to_string();
            }
        } else {
            self.jagr_info = "No package.json found in current directory.".to_string();
        }
    }

    fn reload_doctor_fast(&mut self) {
        match jhol_core::explain_project_health() {
            Ok(text) => self.doctor_report = text,
            Err(e) => self.doctor_report = format!("Health check unavailable: {e}"),
        }
    }

    fn busy(&self) -> bool {
        self.job_rx.is_some()
    }

    fn selected_dep_name(&self) -> Option<String> {
        let i = self.list_state.selected()?;
        if i < self.direct_deps.len() {
            return Some(self.direct_deps[i].0.clone());
        }
        let j = i - self.direct_deps.len();
        self.dev_deps.get(j).map(|(n, _)| n.clone())
    }

    /// Artifacts after the unfit filter, in a stable order: failures first, then unknowns,
    /// then what fits — the question is "what is wrong", not "what is here".
    fn visible_artifacts(&self) -> Vec<&jhol_core::status::Artifact> {
        let Some(s) = &self.status else {
            return Vec::new();
        };
        let mut rows: Vec<&jhol_core::status::Artifact> = s
            .artifacts
            .iter()
            .filter(|a| !self.only_unfit || a.fits == Some(false))
            .collect();
        rows.sort_by_key(|a| match a.fits {
            Some(false) => 0,
            None => 1,
            Some(true) => 2,
        });
        rows
    }

    /// Enter on the Fit tab: size the selected model against the budget and context on screen.
    fn size_selected_model(&mut self) {
        let Some(s) = &self.status else { return };
        let target = s
            .artifacts
            .iter()
            .filter(|a| a.kind == "model")
            .nth(self.list_state.selected().unwrap_or(0))
            .or_else(|| s.artifacts.iter().find(|a| a.kind == "model"))
            .and_then(|a| a.path.clone());
        let Some(target) = target else {
            self.fit_out = "no pinned model to size — pin one with `jhol ai pin`".to_string();
            return;
        };
        let vram = self.fit_vram_gb * 1_073_741_824;
        match crate::status_view::fit_target(
            std::path::Path::new("."),
            &target,
            Some(vram),
            self.fit_ctx,
            1,
        ) {
            Ok(r) => {
                let gb = |b: u64| b as f64 / 1_073_741_824.0;
                let verdict = match (r.fits, r.headroom) {
                    (Some(true), Some(h)) => {
                        format!("fits — {:.2} GB spare", gb(h.max(0) as u64))
                    }
                    (Some(false), Some(h)) => {
                        format!("does not fit — {:.2} GB over", gb((-h) as u64))
                    }
                    _ => "no budget".to_string(),
                };
                let meta = &r.meta;
                self.fit_out = format!(
                    "{}\n  {} layers · {} heads ({} kv) · head_dim {}\n  weights {:>7.2} GB\n  kv {:>7.2} GB at {} tokens\n  overhead {:>7.2} GB\n  total {:>7.2} GB\n  {}\n{}",
                    r.target,
                    meta.layers.unwrap_or(0),
                    meta.heads.unwrap_or(0),
                    meta.kv_heads.or(meta.heads).unwrap_or(0),
                    meta.head_dim.unwrap_or(0),
                    gb(r.weights),
                    gb(r.kv),
                    r.ctx,
                    gb(r.overhead),
                    gb(r.total),
                    verdict,
                    if r.missing_meta.is_empty() {
                        String::new()
                    } else {
                        format!("  header lacks {} — KV not guessed\n", r.missing_meta.join(", "))
                    }
                );
            }
            Err(e) => self.fit_out = format!("fit: {e}"),
        }
    }

    /// Enter on an action. Read-only actions run at once; anything that writes is armed
    /// first and shows what it would cost, because a download is expensive and the default
    /// must be *look before you leap*.
    fn confirm_or_run_action(&mut self, index: usize) {
        let Some((key, label, _)) = ACTIONS.get(index).map(|(k, l, a)| (k.to_string(), l.to_string(), *a)) else {
            return;
        };
        let armed = self
            .pending_action
            .map(|(i, at)| i == index && at.elapsed().as_secs() < 20)
            .unwrap_or(false);
        // The sentence a user confirms against comes from jhol-core, the same function the
        // web page calls. Two surfaces, one description of one download.
        let preview = self
            .status
            .as_ref()
            .map(|s| jhol_core::status::action_preview(&key, s));
        let writes = preview.as_ref().map(|p| p.writes).unwrap_or(true);
        if writes && !armed {
            let summary = preview
                .map(|p| p.summary)
                .unwrap_or_else(|| "no status yet — run r to refresh".to_string());
            self.action_out = format!(
                "{label}\n  {summary}\n\n  Enter again within 20 s to run · anything else cancels"
            );
            self.pending_action = Some((index, Instant::now()));
            return;
        }
        self.pending_action = None;
        self.run_action(index);
    }

    /// Run one of the whitelisted actions by shelling this same binary, streaming its
    /// output into the Actions pane as it arrives. No logic is reimplemented here: the
    /// exit code and the text are the CLI's.
    fn run_action(&mut self, index: usize) {
        let Some((_, label, argv)) = ACTIONS.get(index).map(|(k, l, a)| (k.to_string(), l.to_string(), *a)) else {
            return;
        };
        if self.busy() {
            self.status_msg = "A job is already running — wait for it to finish.".to_string();
            return;
        }
        self.busy_label = label.clone();
        self.job_started = Some(Instant::now());
        self.status_msg = format!("Working: {label}…");
        self.recover_screen = true;
        self.action_out = format!("$ jhol {}\n", argv.join(" "));

        let (chunks_tx, chunks_rx) = mpsc::channel::<String>();
        let (done_tx, done_rx) = mpsc::channel::<Result<String, String>>();
        self.stream = Some(chunks_rx);
        self.job_rx = Some(done_rx);

        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        thread::spawn(move || {
            let _silence = stdio_silence::StderrSilence::enter();
            let exe = match std::env::current_exe() {
                Ok(e) => e,
                Err(e) => {
                    let _ = done_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let spawned = std::process::Command::new(exe)
                .args(&argv)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    let _ = done_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let err_handle = child.stderr.take().map(|mut e| {
                std::thread::spawn(move || {
                    use std::io::Read;
                    let mut buf = Vec::new();
                    let _ = e.read_to_end(&mut buf);
                    buf
                })
            });
            let mut acc = String::new();
            if let Some(mut out) = child.stdout.take() {
                use std::io::BufRead;
                let mut reader = std::io::BufReader::new(&mut out);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            acc.push_str(&line);
                            let _ = chunks_tx.send(line.clone());
                        }
                    }
                }
            }
            let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            if let Some(h) = err_handle {
                let err = String::from_utf8_lossy(&h.join().unwrap_or_default()).to_string();
                if !err.trim().is_empty() {
                    acc.push_str(&err);
                    let _ = chunks_tx.send(err);
                }
            }
            let _ = chunks_tx.send(format!("[exit {code}]\n"));
            let _ = done_tx.send(Ok(acc));
        });
    }

    /// Enter on the Artifacts tab: describe the selected artifact. A remote weight gets
    /// described through a ranged header fetch, so nothing is downloaded.
    fn inspect_selected_artifact(&mut self) {
        let Some(s) = &self.status else { return };
        let Some(a) = s.artifacts.get(self.list_state.selected().unwrap_or(0)) else {
            return;
        };
        let mut text = format!("{} {} ({})\n", a.kind, a.name, a.version);
        text.push_str(&format!(
            "present: {}   bytes: {}\n",
            if a.present { "yes" } else { "no" },
            a.bytes.map(|b| b.to_string()).unwrap_or_else(|| "unknown".into())
        ));
        if let Some(p) = a.params {
            text.push_str(&format!("parameters: {}\n", human_params(p)));
        }
        if let Some(v) = a.vram_bytes {
            text.push_str(&format!("resident: {:.2} GB\n", v as f64 / 1_073_741_824.0));
        }
        if !a.dtypes.is_empty() {
            let d: Vec<String> = a.dtypes.iter().map(|(k, n)| format!("{k}x{n}")).collect();
            text.push_str(&format!("dtypes: {}\n", d.join(" ")));
        }
        if !a.top_tensors.is_empty() {
            text.push_str("heaviest tensors:\n");
            for (name, bytes) in a.top_tensors.iter().take(5) {
                text.push_str(&format!("  {:<38} {:>8} KB\n", name, (bytes / 1024).max(1)));
            }
        }
        text.push_str(&format!(
            "fit: {}\n",
            match a.fits {
                Some(true) => "fits",
                Some(false) => "does not fit",
                None => "unknown",
            }
        ));
        for r in &a.reasons {
            text.push_str(&format!("- {r}\n"));
        }
        self.action_out = text;
    }

    /// Drain whatever the running action printed since the last frame.
    fn poll_stream(&mut self) {
        let Some(rx) = self.stream.as_ref() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(chunk) => self.action_out.push_str(&chunk),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.stream = None;
                    break;
                }
            }
        }
    }

    fn start_job<F>(&mut self, label: &str, work: F)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        if self.busy() {
            self.status_msg = "A job is already running — wait for it to finish.".to_string();
            return;
        }
        self.busy_label = label.to_string();
        self.job_started = Some(Instant::now());
        self.status_msg = format!("Working: {label}…");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _silence = stdio_silence::StderrSilence::enter();
            let result = work();
            let _ = tx.send(result);
        });
        self.job_rx = Some(rx);
    }

    fn poll_job(&mut self) {
        self.poll_stream();
        let Some(rx) = self.job_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(msg)) => {
                self.job_rx = None;
                self.busy_label.clear();
                self.job_started = None;
                self.recover_screen = true;
                self.action_out = msg.clone();
                self.reload_manifest();
                self.reload_cache();
                self.instant_lock = InstantLock::probe();
                if msg.contains('\n')
                    && (msg.contains("outdated") || msg.contains("Health") || msg.len() > 80)
                {
                    self.doctor_report = msg;
                    self.tab = ActiveTab::Doctor;
                    self.status_msg = "Doctor scan complete — results on tab 4.".to_string();
                } else {
                    self.reload_doctor_fast();
                    self.status_msg = msg;
                }
            }
            Ok(Err(err)) => {
                self.job_rx = None;
                self.busy_label.clear();
                self.job_started = None;
                self.recover_screen = true;
                self.status_msg = format!("Failed: {err}");
            }
            Err(mpsc::TryRecvError::Empty) => {
                if self.busy_label.to_lowercase().contains("install") {
                    let snap = jhol_core::progress_snapshot();
                    let done = snap.linked.max(snap.from_cache + snap.from_network);
                    let elapsed = self.job_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                    self.status_msg = format!(
                        "{} · {} linked · cache {} · net {} · {}s",
                        self.busy_label, done, snap.from_cache, snap.from_network, elapsed
                    );
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job_rx = None;
                self.recover_screen = true;
                self.status_msg = "Job ended unexpectedly.".to_string();
            }
        }
    }

    fn run_install(&mut self) {
        self.start_job("install", || {
            let opts = jhol_core::InstallOptions {
                quiet: true,
                ..Default::default()
            };
            jhol_core::install_package(&[], &opts).map_err(|e| e.to_string())?;
            let armed = Path::new("node_modules/.jhol-ok").exists();
            Ok(if armed {
                "Install finished — Instant Lock armed (.jhol-ok).".to_string()
            } else {
                "Install finished.".to_string()
            })
        });
    }

    fn run_doctor_scan(&mut self) {
        self.start_job("doctor", || {
            let health = jhol_core::explain_project_health().unwrap_or_default();
            let outdated = jhol_core::native_outdated().map_err(|e| e.to_string())?;
            let mut out = health;
            if outdated.is_empty() {
                out.push_str("\n\nAll declared dependencies are up to date.");
            } else {
                out.push_str(&format!("\n\n{} outdated:", outdated.len()));
                for (pkg, cur, latest) in outdated {
                    out.push_str(&format!("\n  {pkg}: {cur} → {latest}"));
                }
            }
            Ok(out)
        });
        // The worker returns a long report; surface it on the doctor tab when done via status.
    }

    fn run_doctor_fix(&mut self) {
        self.start_job("doctor --fix", || {
            jhol_core::fix_dependencies(true, jhol_core::Backend::Npm)
                .map_err(|e| e.to_string())?;
            Ok("Doctor applied available fixes.".to_string())
        });
    }

    fn run_audit(&mut self) {
        self.start_job("audit", || {
            match jhol_core::run_audit(jhol_core::Backend::Npm, true) {
                Ok(()) => Ok("Audit: no vulnerabilities found.".to_string()),
                Err(e) if e.contains("vulnerabilit") => Ok(format!("Audit complete: {e}")),
                Err(e) => Err(e),
            }
        });
    }

    fn run_prune(&mut self) {
        self.start_job("cache prune", || {
            jhol_core::cache_prune(None).map_err(|e| e.to_string())?;
            Ok("Cache prune finished.".to_string())
        });
    }

    fn run_why_selected(&mut self) {
        let Some(name) = self.selected_dep_name() else {
            self.status_msg = "Select a dependency first (tab 2, then ↑/↓).".to_string();
            return;
        };
        let pkg = name.clone();
        self.start_job(&format!("why {name}"), move || {
            let lines = jhol_core::why_package(&pkg)?;
            if lines.is_empty() {
                Ok(format!("{pkg} is not in the current tree."))
            } else {
                Ok(format!("{}: {}", pkg, lines.join(" ← ")))
            }
        });
    }

    fn run_remove_selected(&mut self) {
        let Some(name) = self.selected_dep_name() else {
            self.status_msg = "Select a dependency first (tab 2, then ↑/↓).".to_string();
            return;
        };
        let pkg = name.clone();
        self.start_job(&format!("remove {name}"), move || {
            jhol_core::uninstall(&pkg, true)?;
            Ok(format!("Removed {pkg} from the manifest and tree."))
        });
    }

    fn run_update(&mut self) {
        self.start_job("update", || {
            jhol_core::update_packages(&[])?;
            Ok("Update finished.".to_string())
        });
    }

    fn run_script_selected(&mut self) {
        if self.scripts.is_empty() {
            self.status_msg = "No scripts in package.json.".to_string();
            return;
        }
        let i = self.list_state.selected().unwrap_or(0) % self.scripts.len();
        let name = self.scripts[i].0.clone();
        self.start_job(&format!("run {name}"), move || {
            let status = jhol_core::run_script(&name, Path::new("."), None)?;
            if status.success() {
                Ok(format!("Script `{name}` exited 0."))
            } else {
                Err(format!("Script `{name}` failed ({status})."))
            }
        });
    }

    fn next_item(&mut self) {
        let count = match self.tab {
            ActiveTab::Dependencies => self.direct_deps.len() + self.dev_deps.len(),
            ActiveTab::Artifacts => self.visible_artifacts().len(),
            ActiveTab::Actions => ACTIONS.len(),
            _ => 0,
        };
        if count == 0 {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i + 1 >= count {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn prev_item(&mut self) {
        let count = match self.tab {
            ActiveTab::Dependencies => self.direct_deps.len() + self.dev_deps.len(),
            ActiveTab::Artifacts => self.status.as_ref().map(|s| s.artifacts.len()).unwrap_or(0),
            _ => 0,
        };
        if count == 0 {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    count - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }
}

/// Redirect fd 2 → /dev/null for a background TUI job.
/// The dashboard owns stdout (crossterm); only stderr is safe to mute.
/// Restores the previous stderr on drop.
#[cfg(unix)]
mod stdio_silence {
    use std::os::fd::AsRawFd;

    extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
        fn close(fd: i32) -> i32;
    }

    pub struct StderrSilence {
        saved: Option<i32>,
    }

    impl StderrSilence {
        pub fn enter() -> Self {
            let Ok(null) = std::fs::OpenOptions::new().write(true).open("/dev/null") else {
                return Self { saved: None };
            };
            let nfd = null.as_raw_fd();
            let saved = unsafe { dup(2) };
            if saved < 0 {
                return Self { saved: None };
            }
            if unsafe { dup2(nfd, 2) } < 0 {
                unsafe { close(saved) };
                return Self { saved: None };
            }
            std::mem::forget(null);
            Self { saved: Some(saved) }
        }
    }

    impl Drop for StderrSilence {
        fn drop(&mut self) {
            if let Some(fd) = self.saved.take() {
                unsafe {
                    dup2(fd, 2);
                    close(fd);
                }
            }
        }
    }
}

#[cfg(not(unix))]
mod stdio_silence {
    pub struct StderrSilence;
    impl StderrSilence {
        pub fn enter() -> Self {
            Self
        }
    }
}

pub fn run_tui() -> Result<(), String> {
    // Core install/doctor paths log to stderr; that corrupts the alternate screen
    // if a job runs while the dashboard is open. Silence for the whole session.
    jhol_core::set_output_silenced(true);

    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| e.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let mut app = AppState::new();
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode().map_err(|e| e.to_string())?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .map_err(|e| e.to_string())?;
    terminal.show_cursor().map_err(|e| e.to_string())?;
    jhol_core::set_output_silenced(false);

    res
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut AppState) -> Result<(), String> {
    // Draw before loading: the constructor used to run a full JAGR resolution (network
    // bound on a cold project) first, leaving a blank screen for ~1.1 s (measured).
    // The dashboard now appears immediately and data fills in behind it.
    terminal.draw(|f| ui(f, app)).map_err(|e| e.to_string())?;
    app.load_initial();
    app.dirty = true;

    loop {
        app.poll_job();
        if app.recover_screen {
            let _ = terminal.clear();
            app.recover_screen = false;
            app.dirty = true;
        }
        if app.dirty || app.busy() {
            terminal.draw(|f| ui(f, app)).map_err(|e| e.to_string())?;
            app.dirty = false;
            app.tick = app.tick.wrapping_add(1);
        }

        if event::poll(Duration::from_millis(120)).map_err(|e| e.to_string())? {
            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if app.eggs.feed(key.code) {
                    app.dirty = true;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if app.eggs.show_help {
                            app.eggs.show_help = false;
                            app.dirty = true;
                            continue;
                        }
                        if app.busy() {
                            app.status_msg =
                                "A job is still running — wait, then quit.".to_string();
                            app.dirty = true;
                            continue;
                        }
                        return Ok(());
                    }
                    KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                        app.tab = app.tab.next();
                        app.dirty = true;
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        app.tab = app.tab.prev();
                        app.dirty = true;
                    }
                    KeyCode::Char('1') => {
                        app.tab = ActiveTab::Status;
                        app.dirty = true;
                    }
                    KeyCode::Char('2') => {
                        app.tab = ActiveTab::Artifacts;
                        app.dirty = true;
                    }
                    KeyCode::Char('3') => {
                        app.tab = ActiveTab::Budget;
                        app.dirty = true;
                    }
                    KeyCode::Char('4') => {
                        app.tab = ActiveTab::Proof;
                        app.dirty = true;
                    }
                    KeyCode::Char('5') => {
                        app.tab = ActiveTab::Proof;
                        app.dirty = true;
                    }
                    KeyCode::Char('4') => {
                        app.tab = ActiveTab::Budget;
                        app.dirty = true;
                    }
                    KeyCode::Char('3') => {
                        app.tab = ActiveTab::Fit;
                        app.dirty = true;
                    }
                    KeyCode::Char('a') => {
                        app.tab = ActiveTab::Actions;
                        app.dirty = true;
                    }
                    KeyCode::Char('6') => {
                        app.tab = ActiveTab::Actions;
                        app.dirty = true;
                    }
                    KeyCode::Char('7') => {
                        app.tab = ActiveTab::Dependencies;
                        app.dirty = true;
                    }
                    KeyCode::Char('8') => {
                        app.tab = ActiveTab::Doctor;
                        app.dirty = true;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.next_item();
                        app.dirty = true;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.prev_item();
                        app.dirty = true;
                    }
                    KeyCode::Char('u') if app.tab == ActiveTab::Artifacts => {
                        app.only_unfit = !app.only_unfit;
                        app.status_msg = if app.only_unfit {
                            "showing only artifacts that do not fit (u to show all)".to_string()
                        } else {
                            "showing all artifacts".to_string()
                        };
                        app.list_state.select(Some(0));
                        app.dirty = true;
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') if app.tab == ActiveTab::Fit => {
                        app.fit_vram_gb += 1;
                        app.fit_out.clear();
                        app.dirty = true;
                    }
                    KeyCode::Char('-') if app.tab == ActiveTab::Fit => {
                        app.fit_vram_gb = app.fit_vram_gb.saturating_sub(1).max(1);
                        app.fit_out.clear();
                        app.dirty = true;
                    }
                    KeyCode::Char('c') if app.tab == ActiveTab::Fit => {
                        app.fit_ctx = match app.fit_ctx {
                            2048 => 4096,
                            4096 => 8192,
                            8192 => 16384,
                            16384 => 32768,
                            _ => 2048,
                        };
                        app.fit_out.clear();
                        app.dirty = true;
                    }
                    KeyCode::Enter if app.tab == ActiveTab::Fit => {
                        app.size_selected_model();
                        app.dirty = true;
                    }
                    KeyCode::Enter if app.tab == ActiveTab::Artifacts => {
                        app.inspect_selected_artifact();
                        app.dirty = true;
                    }
                    KeyCode::Enter if app.tab == ActiveTab::Actions => {
                        app.confirm_or_run_action(app.list_state.selected().unwrap_or(0));
                        app.dirty = true;
                    }
                    KeyCode::Char('r') => {
                        app.reload_manifest();
                        app.reload_cache();
                        app.reload_doctor_fast();
                        app.reload_jagr();
                        app.jagr_loaded = true;
                        app.instant_lock = InstantLock::probe();
                        app.status_msg =
                            "Refreshed manifest, cache, doctor, Instant Lock and JAGR.".to_string();
                        app.dirty = true;
                    }
                    KeyCode::Char('c') => {
                        if app.busy() {
                            app.status_msg = "Busy — wait for the current job.".to_string();
                        } else {
                            let _ = jhol_core::cache_clean();
                            app.reload_cache();
                            app.status_msg = "Cache cleaned.".to_string();
                        }
                        app.dirty = true;
                    }
                    KeyCode::Char('i') => {
                        app.run_install();
                        app.dirty = true;
                    }
                    KeyCode::Char('d') => {
                        app.run_doctor_scan();
                        app.tab = ActiveTab::Doctor;
                        app.dirty = true;
                    }
                    KeyCode::Char('f') => {
                        app.run_doctor_fix();
                        app.dirty = true;
                    }
                    KeyCode::Char('a') => {
                        app.run_audit();
                        app.dirty = true;
                    }
                    KeyCode::Char('p') => {
                        app.run_prune();
                        app.dirty = true;
                    }
                    KeyCode::Char('w') | KeyCode::Enter => {
                        app.run_why_selected();
                        app.dirty = true;
                    }
                    KeyCode::Char('x') => {
                        app.run_remove_selected();
                        app.dirty = true;
                    }
                    KeyCode::Char('u') => {
                        app.run_update();
                        app.dirty = true;
                    }
                    KeyCode::Char('s') => {
                        app.run_script_selected();
                        app.dirty = true;
                    }
                    _ => {}
                }

            }
        }

        // Animation frames only while an egg is running; otherwise the loop blocks in
        // `poll` (idle CPU measured at 0.0%).
        if app.eggs.party_active() || app.eggs.banner_active() || app.busy() {
            app.dirty = true;
        }
    }
}

/// A rectangle centred in `area`, for the help overlay.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn ui(f: &mut Frame, app: &mut AppState) {
    let area = f.size();
    // Wipe every cell first so a leaked stderr line cannot ghost under widgets.
    f.render_widget(Clear, area);
    // A zero or tiny terminal used to render silently blank frames; say so instead.
    if area.width < 60 || area.height < 14 {
        let msg = Paragraph::new(format!(
            "Terminal too small ({}x{}).\nResize to at least 60x14.",
            area.width, area.height
        ))
        .block(Block::default().borders(Borders::ALL).title(" jhol "))
        .style(Style::default().fg(Color::Yellow));
        f.render_widget(msg, area);
        return;
    }
    let accent = if app.eggs.party_active() {
        RAINBOW[(app.tick as usize) % RAINBOW.len()]
    } else if app.busy() {
        Color::Green
    } else {
        Color::Rgb(170, 120, 255)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(4),
        ])
        .split(f.size());

    // Title Bar — matches CLI splash (platform is the install filter machine).
    let warm_tag = if app.instant_lock.warm_ok {
        "warm-ok"
    } else if app.instant_lock.node_modules {
        "tree"
    } else {
        "cold"
    };
    let title_text = vec![
        Span::styled(
            " jhol ",
            Style::default()
                .fg(Color::Rgb(235, 225, 255))
                .bg(Color::Rgb(90, 50, 160))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " v{}  {}  ·  {}@{}  ·  {} ",
                env!("CARGO_PKG_VERSION"),
                app.instant_lock.platform,
                app.pkg_name,
                app.pkg_version,
                warm_tag,
            ),
            Style::default()
                .fg(Color::Rgb(220, 210, 240))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let title_par = Paragraph::new(Line::from(title_text)).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(accent)),
    );
    f.render_widget(title_par, chunks[0]);

    // Navigation Tabs
    let tab_titles = vec![
        ActiveTab::Status.title(),
        ActiveTab::Artifacts.title(),
        ActiveTab::Fit.title(),
        ActiveTab::Budget.title(),
        ActiveTab::Proof.title(),
        ActiveTab::Actions.title(),
        ActiveTab::Dependencies.title(),
        ActiveTab::Doctor.title(),
    ];
    let tabs = Tabs::new(tab_titles)
        .select(app.tab as usize)
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(accent).add_modifier(Modifier::BOLD));
    f.render_widget(tabs, chunks[1]);

    // Content Area
    match app.tab {
        ActiveTab::Status => render_overview(f, app, chunks[2]),
        ActiveTab::Artifacts => render_artifacts(f, app, chunks[2]),
        ActiveTab::Fit => render_fit(f, app, chunks[2]),
        ActiveTab::Budget => render_budget(f, app, chunks[2]),
        ActiveTab::Proof => render_proof(f, app, chunks[2]),
        ActiveTab::Actions => render_actions(f, app, chunks[2]),
        ActiveTab::Dependencies => render_dependencies(f, app, chunks[2]),
        ActiveTab::Doctor => render_doctor(f, app, chunks[2]),
    }

    // Status / Help Bar — two rows so a long status never mashes into the key hints.
    // Clear the chunk first: leftover cells from a stderr leak look like this bar "ghosting".
    f.render_widget(Clear, chunks[3]);
    let inner_w = chunks[3].width.saturating_sub(2) as usize;
    let truncate = |s: &str, max: usize| -> String {
        if max == 0 {
            return String::new();
        }
        let mut out = String::new();
        for ch in s.chars() {
            if out.chars().count() + 1 >= max {
                out.push('…');
                break;
            }
            out.push(ch);
        }
        out
    };
    let mut status = String::new();
    if let Some(extra) = app
        .eggs
        .status_extra(app.tick, chunks[3].width.saturating_sub(4))
    {
        status.push_str(&extra);
        status.push(' ');
    }
    // A pun takes the status line over rather than being appended.
    match app.eggs.pun {
        Some(pun) => status.push_str(pun),
        None if app.busy() => {
            let spin = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let frame = spin[(app.tick as usize) % spin.len()];
            let elapsed = app.job_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let snap = jhol_core::progress_snapshot();
            let done = snap.from_cache + snap.from_network;
            if snap.total > 1 {
                status.push_str(&format!(
                    "{frame} {}  {}/{}  cache {} net {}  {}s",
                    app.busy_label, done, snap.total, snap.from_cache, snap.from_network, elapsed
                ));
            } else if done > 0 {
                status.push_str(&format!(
                    "{frame} {}  cache {} net {}  {}s",
                    app.busy_label, snap.from_cache, snap.from_network, elapsed
                ));
            } else {
                status.push_str(&format!("{frame} {}  {}s", app.busy_label, elapsed));
            }
        }
        None => status.push_str(&app.status_msg),
    }
    if std::env::var_os("JHOL_EGGS").is_some() {
        status.push_str(&format!("  [secrets {}/4]", app.eggs.found()));
    }
    let keys = "⏎ inspect/run  i install  d doctor  f fix  a audit  p prune  w why  x remove  s script  ? help  q";
    let help_lines = vec![
        Line::from(Span::styled(
            truncate(&format!(" {status}"), inner_w),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            truncate(&format!(" {keys}"), inner_w),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let help_par = Paragraph::new(help_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(help_par, chunks[3]);

    // Help overlay: also the discovery surface for the hidden extras.
    if app.eggs.show_help {
        let popup = centered_rect(72, 20, area);
        f.render_widget(Clear, popup);
        let lines = vec![
            Line::from(Span::styled(
                "jhol dashboard — working keys",
                Style::default()
                    .fg(Color::Rgb(170, 120, 255))
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  1-5 / Tab / arrows     switch view"),
            Line::from("  j/k or up/down         move in a list"),
            Line::from("  i                      install this project"),
            Line::from("  d / f                  doctor scan / apply fixes"),
            Line::from("  a                      audit"),
            Line::from("  p / c                  prune cache / clean cache"),
            Line::from("  w or Enter             why the selected dependency"),
            Line::from("  x                      remove the selected dependency"),
            Line::from("  u                      update"),
            Line::from("  s                      run a package.json script"),
            Line::from("  r                      refresh  ·  q quit"),
            Line::from(""),
            Line::from(Span::styled(
                "  Instant Lock: after install, bare `jhol install` exits in ~ms",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  when node_modules/.jhol-ok is fresh vs package.json.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  not everything is listed here — some keys are hidden.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let popup_par = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Help - ? to close ")
                    .style(Style::default().fg(accent)),
            )
            .style(Style::default().bg(Color::Black));
        f.render_widget(popup_par, popup);
    }
}

fn human_params(p: u64) -> String {
    let f = p as f64;
    if f >= 1e9 {
        format!("{:.2} B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1} M", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.1} K", f / 1e3)
    } else {
        format!("{p}")
    }
}

fn status_lines(app: &AppState) -> Vec<Line<'static>> {
    let Some(s) = &app.status else {
        return vec![Line::from("  collecting…")];
    };
    if !s.has_manifest {
        let dim = Style::default().fg(Color::DarkGray);
        return vec![
            Line::from(Span::styled(
                "  no project here",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  This directory has no package.json, so there is nothing to verify."),
            Line::from("  The dashboard is read-only until a project exists."),
            Line::from(""),
            Line::from(vec![
                Span::styled("  start one  ", dim),
                Span::raw("jhol install express"),
            ]),
            Line::from(vec![
                Span::styled("  weights    ", dim),
                Span::raw("jhol ai pull hf://org/repo/model.safetensors"),
            ]),
            Line::from(vec![
                Span::styled("  watch      ", dim),
                Span::raw("jhol status --watch"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  press r to re-read the directory",
                dim,
            )),
        ];
    }
    let ok = Style::default().fg(Color::Green);
    let bad = Style::default().fg(Color::Red);
    let key = |k: &'static str| {
        Span::styled(
            format!("{k:<10}"),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )
    };
    let mut out = Vec::new();
    out.push(Line::from(vec![
        key("tree"),
        if s.tree.state == "match" {
            Span::styled(
                format!("match — {} packages", s.tree.packages),
                ok,
            )
        } else {
            Span::styled(format!("{} — {}", s.tree.state, s.tree.detail), bad)
        },
    ]));
    out.push(Line::from(vec![
        key("lockfile"),
        if s.manifest_matches_lockfile {
            Span::styled("matches package.json", ok)
        } else {
            Span::styled("drifted — run jhol install", bad)
        },
    ]));
    let mut policy = Vec::new();
    if let Some(at) = &s.policy.as_of {
        policy.push(format!("as-of {at}"));
    }
    if let Some(l) = &s.policy.allowed_licenses {
        policy.push(format!("licences {}", l.join(",")));
    }
    if s.policy.exclude_vulnerable {
        policy.push("exclude-vulnerable".to_string());
    }
    out.push(Line::from(vec![
        key("policy"),
        Span::raw(if policy.is_empty() {
            "none".to_string()
        } else {
            policy.join(" · ")
        }),
    ]));
    out.push(Line::from(vec![
        key("axes"),
        Span::raw(format!(
            "{}/{} {} · python {} · cuda {}",
            s.axes.os,
            s.axes.arch,
            s.axes.libc,
            s.axes.python.as_deref().unwrap_or("-"),
            s.axes.cuda_driver.as_deref().unwrap_or("none")
        )),
    ]));
    out.push(Line::from(vec![
        key("artifacts"),
        Span::raw(format!(
            "{} models · {} python dists",
            s.models.pinned, s.python.pinned
        )),
    ]));
    out.push(Line::from(vec![
        key("proof"),
        Span::raw(format!(
            "snapshot {} · attested {} packages, {} models",
            if s.proof.snapshot_ready { "ready" } else { "absent" },
            s.proof.attested_packages,
            s.proof.attested_models
        )),
    ]));
    out
}

fn render_overview(f: &mut Frame, app: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let mut info_text = vec![
        Line::from(vec![
            Span::styled(
                "Project:       ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(&app.pkg_name),
        ]),
        Line::from(vec![
            Span::styled(
                "Version:       ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(&app.pkg_version),
        ]),
        Line::from(vec![
            Span::styled(
                "Directory:     ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Instant-Lock Sentinel & JGrid Status:",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!(
            "  • Warm sentinel (.jhol-ok):  {}",
            if app.instant_lock.warm_ok {
                "armed (sub-millisecond C sentinel exit)"
            } else {
                "missing (cold)"
            }
        )),
        Line::from(format!(
            "  • JGrid Prebuilt Key (L0):   {}",
            if app.instant_lock.prebuilt_key {
                "present (zero-copy clone projection ready)"
            } else {
                "—"
            }
        )),
        Line::from(format!(
            "  • Lockfile Integrity:        {}",
            if app.instant_lock.lockfile {
                "verified"
            } else {
                "none"
            }
        )),
        Line::from(format!(
            "  • Warm Path State:           {}",
            app.instant_lock.warm_label()
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  C sentinel front-door skips multi-MB runtime when warm is armed.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    info_text.extend(status_lines(app));
    let left_block = Paragraph::new(info_text)
        .block(
            Block::default()
                .title(" Project Architecture & Sentinel Status ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(left_block, chunks[0]);

    let cache_mb = app.cache_bytes as f64 / (1024.0 * 1024.0);
    let mut right_text = vec![
        Line::from(vec![Span::styled(
            "Dependencies:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!(
            "  • Direct Dependencies: {}",
            app.direct_deps.len()
        )),
        Line::from(format!("  • Dev Dependencies:    {}", app.dev_deps.len())),
        Line::from(format!(
            "  • Total Direct:        {}",
            app.direct_deps.len() + app.dev_deps.len()
        )),
        Line::from(""),
        Line::from(vec![Span::styled(
            "JGrid Content Store:",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!("  • Cached Tarballs:     {}", app.cache_tarballs)),
        Line::from(format!("  • Total Disk Usage:    {:.2} MB", cache_mb)),
        Line::from(format!("  • Cached Packages:     {}", app.cache_pkgs.len())),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Actions (Live Execution):",
            Style::default()
                .fg(Color::Rgb(170, 120, 255))
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  i  install          d  doctor scan     f  doctor --fix"),
        Line::from("  a  audit            p  cache prune     u  update"),
        Line::from("  w  why selected     x  remove selected s  run a script"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Scripts:",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )]),
    ];
    if app.scripts.is_empty() {
        right_text.push(Line::from("  (none in package.json)"));
    } else {
        for (name, cmd) in app.scripts.iter().take(8) {
            right_text.push(Line::from(format!("  {name}  →  {cmd}")));
        }
        if app.scripts.len() > 8 {
            right_text.push(Line::from(format!(
                "  … {} more (press s to run)",
                app.scripts.len() - 8
            )));
        }
    }
    let right_block = Paragraph::new(right_text)
        .block(
            Block::default()
                .title(" JGrid Metrics & Quick Actions ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(right_block, chunks[1]);
}

fn render_dependencies(f: &mut Frame, app: &mut AppState, area: Rect) {
    let mut items = Vec::new();
    for (name, spec) in &app.direct_deps {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(" [direct] ", Style::default().fg(Color::Cyan)),
            Span::styled(
                name,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" @ "),
            Span::styled(spec, Style::default().fg(Color::Yellow)),
        ])));
    }
    for (name, spec) in &app.dev_deps {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(" [dev]    ", Style::default().fg(Color::Magenta)),
            Span::styled(name, Style::default().fg(Color::White)),
            Span::raw(" @ "),
            Span::styled(spec, Style::default().fg(Color::DarkGray)),
        ])));
    }

    if items.is_empty() {
        items.push(ListItem::new("No dependencies declared in package.json."));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Declared Dependencies  ·  Enter/w why  ·  x remove ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_budget(f: &mut Frame, app: &AppState, area: Rect) {
    let Some(s) = &app.status else {
        f.render_widget(Paragraph::new("  collecting…"), area);
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:.1} MB missing", s.plan.missing_bytes as f64 / 1_048_576.0),
            if s.plan.missing_bytes > 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Green)
            },
        ),
        Span::raw(format!(
            " · {:.1} MB cached · ~{} s at {} MB/s",
            s.plan.cached_bytes as f64 / 1_048_576.0,
            s.plan.eta_seconds,
            s.plan.throughput_bytes_per_sec / 1_048_576
        )),
    ]));
    lines.push(Line::from(""));
    for item in &s.plan.items {
        lines.push(Line::from(vec![
            Span::styled(
                if item.present { " cached  " } else { " missing " },
                if item.present {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
            Span::raw(format!(
                "{:<44} {:>9}",
                item.name,
                format!("{:.1} MB", item.bytes as f64 / 1_048_576.0)
            )),
        ]));
    }
    if !s.forks.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " forks",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        for fork in &s.forks {
            lines.push(Line::from(format!(
                "   {:<20} {:.1} MB beyond base",
                fork.name,
                fork.bytes as f64 / 1_048_576.0
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " the rate is stated, not implied: a number without its assumption is a guess",
        Style::default().fg(Color::DarkGray),
    )));
    let block = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Budget — bytes and seconds before you commit ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(block, area);
}

fn render_artifacts(f: &mut Frame, app: &AppState, area: Rect) {
    let Some(s) = &app.status else {
        f.render_widget(Paragraph::new("  collecting…"), area);
        return;
    };
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(area);

    let rows = app.visible_artifacts();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|a| {
            let (mark, style) = match a.fits {
                Some(true) => ("✓", Style::default().fg(Color::Green)),
                Some(false) => ("✗", Style::default().fg(Color::Red)),
                None => ("?", Style::default().fg(Color::Yellow)),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {mark} "), style),
                Span::styled(format!("{:<11}", a.kind), Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{} {}", a.name, a.version)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    " Artifacts — will it run here? (Enter describes, u {}unfit only) ",
                    if app.only_unfit { "shows " } else { "hides " }
                ))
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, chunks[0], &mut app.list_state.clone());

    let selected = app.list_state.selected().unwrap_or(0);
    let lines: Vec<Line> = match rows.get(selected).copied() {
        Some(a) => {
            let mut out = vec![Line::from(Span::styled(
                format!("{} {}", a.name, a.version),
                Style::default().add_modifier(Modifier::BOLD),
            ))];
            if let Some(p) = a.params {
                out.push(Line::from(format!("  {} parameters", human_params(p))));
            }
            if let Some(v) = a.vram_bytes {
                out.push(Line::from(format!(
                    "  resident ~{:.1} GB (tensors + 20%)",
                    v as f64 / 1_073_741_824.0
                )));
            }
            if !a.dtypes.is_empty() {
                let d: Vec<String> = a.dtypes.iter().map(|(k, n)| format!("{k}×{n}")).collect();
                out.push(Line::from(format!("  dtypes {}", d.join(" "))));
            }
            if !a.digest.is_empty() {
                out.push(Line::from(format!(
                    "  sha256 {}",
                    &a.digest[..a.digest.len().min(16)]
                )));
            }
            if !a.reasons.is_empty() {
                out.push(Line::from(""));
                for r in &a.reasons {
                    out.push(Line::from(format!("  • {r}")));
                }
            }
            out
        }
        None => vec![Line::from("  no artifacts: no wheels pinned, no weights locked")],
    };
    let pane = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Detail ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(pane, chunks[1]);
}

fn render_fit(f: &mut Frame, app: &AppState, area: Rect) {
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("budget  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{} GB ", app.fit_vram_gb)),
            Span::styled("[+/-]", Style::default().fg(Color::DarkGray)),
            Span::raw("   "),
            Span::styled("context  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{} tokens ", app.fit_ctx)),
            Span::styled("[c]", Style::default().fg(Color::DarkGray)),
            Span::raw("   "),
            Span::styled("[Enter] size the selected model", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
    ];
    if app.fit_out.is_empty() {
        lines.push(Line::from(Span::styled(
            "  press Enter: weights + KV cache + 15% overhead, from the header alone",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for line in app.fit_out.lines() {
            lines.push(Line::from(line.to_string()));
        }
    }
    let block = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Fit — will it run here? ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(block, area);
}

fn render_proof(f: &mut Frame, app: &AppState, area: Rect) {
    let Some(s) = &app.status else {
        f.render_widget(Paragraph::new("  collecting…"), area);
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("snapshot  ", Style::default().fg(Color::Cyan)),
            Span::raw(if s.proof.snapshot_ready {
                "ready — the next install is a rename"
            } else {
                "absent — the next install materialises from the lockfile"
            }),
        ]),
        Line::from(vec![
            Span::styled("attested  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!(
                "{} packages, {} models",
                s.proof.attested_packages, s.proof.attested_models
            )),
        ]),
        Line::from(vec![
            Span::styled("policy    ", Style::default().fg(Color::Cyan)),
            Span::raw(if s.policy.as_of.is_some() || s.policy.exclude_vulnerable {
                "restricted — see tab 1"
            } else {
                "latest, any licence, CVEs not filtered"
            }),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " agent surface — what a tool can ask this project to do",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
    ];
    for tool in &s.agent.tools {
        lines.push(Line::from(format!(
            "   {:<26} {:<8} {}",
            tool.name, tool.cost, tool.purpose
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " jhol attest · jhol replay · jhol verify --as-of — one line each, into a README",
        Style::default().fg(Color::DarkGray),
    )));
    let block = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Proof — can this environment be reproduced? ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(block, area);
}

fn render_doctor(f: &mut Frame, app: &AppState, area: Rect) {
    let mut lines = vec![
        Line::from(vec![Span::styled(
            " Live health  ·  d rescan  ·  f apply fixes  ·  a audit",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
    ];
    if app.doctor_report.trim().is_empty() {
        lines.push(Line::from(
            "Press d to run a live doctor scan for this project.",
        ));
    } else {
        for row in app.doctor_report.lines().take(28) {
            lines.push(Line::from(row.to_string()));
        }
    }
    let block = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" System & Project Runtime Health Diagnostics ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(block, area);
}

fn render_actions(f: &mut Frame, app: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(area);

    let items: Vec<ListItem> = ACTIONS
        .iter()
        .map(|(_, label, _)| ListItem::new(Line::from(format!("  {label}"))))
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Actions — Enter runs the selected one ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, chunks[0], &mut app.list_state.clone());

    let out = if app.action_out.is_empty() {
        "Nothing run yet. These are the same five actions `jhol gui --web` exposes; each one\n\
         runs this binary, so the exit code and output are the CLI's own."
            .to_string()
    } else {
        app.action_out.clone()
    };
    let lines: Vec<Line> = out.lines().map(Line::from).collect();
    let pane = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Output ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(pane, chunks[1]);
}

#[cfg(test)]
mod action_tests {
    use super::ACTIONS;

    /// The TUI and `jhol gui --web` claim the same five actions. If one gains an entry the
    /// other does not, the two surfaces disagree about what the tool can do.
    #[test]
    fn tui_actions_match_the_web_surface() {
        let mut argv: Vec<Vec<&str>> = ACTIONS.iter().map(|(_, _, a)| a.to_vec()).collect();
        argv.sort();
        let mut web: Vec<Vec<&str>> = vec![
            vec!["verify"],
            vec!["verify", "--prune"],
            vec!["install"],
            vec!["ci"],
            vec!["install", "--exclude-vuln"],
        ];
        web.sort();
        assert_eq!(argv, web);
    }
}
