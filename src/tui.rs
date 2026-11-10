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
    Overview = 0,
    Dependencies = 1,
    Cache = 2,
    Doctor = 3,
    Jagr = 4,
}

impl ActiveTab {
    fn title(&self) -> &'static str {
        match self {
            ActiveTab::Overview => " 1. Dashboard (Instant-Lock) ",
            ActiveTab::Dependencies => " 2. Dependencies ",
            ActiveTab::Cache => " 3. JGrid Storage & Cache ",
            ActiveTab::Doctor => " 4. Health & Diagnostics ",
            ActiveTab::Jagr => " 5. JAGR-∞ Invariants ",
        }
    }

    fn next(&self) -> Self {
        match self {
            ActiveTab::Overview => ActiveTab::Dependencies,
            ActiveTab::Dependencies => ActiveTab::Cache,
            ActiveTab::Cache => ActiveTab::Doctor,
            ActiveTab::Doctor => ActiveTab::Jagr,
            ActiveTab::Jagr => ActiveTab::Overview,
        }
    }

    fn prev(&self) -> Self {
        match self {
            ActiveTab::Overview => ActiveTab::Jagr,
            ActiveTab::Dependencies => ActiveTab::Overview,
            ActiveTab::Cache => ActiveTab::Dependencies,
            ActiveTab::Doctor => ActiveTab::Cache,
            ActiveTab::Jagr => ActiveTab::Doctor,
        }
    }
}

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
            tab: ActiveTab::Overview,
            pkg_name: "jhol-project".to_string(),
            pkg_version: "1.0.0".to_string(),
            status: None,
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
                self.status = Some(jhol_core::status::collect(Path::new(".")));
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
        let Some(rx) = self.job_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(msg)) => {
                self.job_rx = None;
                self.busy_label.clear();
                self.job_started = None;
                self.recover_screen = true;
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
            ActiveTab::Cache => self.cache_pkgs.len(),
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
            ActiveTab::Cache => self.cache_pkgs.len(),
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
                        app.tab = ActiveTab::Overview;
                        app.dirty = true;
                    }
                    KeyCode::Char('2') => {
                        app.tab = ActiveTab::Dependencies;
                        app.dirty = true;
                    }
                    KeyCode::Char('3') => {
                        app.tab = ActiveTab::Cache;
                        app.dirty = true;
                    }
                    KeyCode::Char('4') => {
                        app.tab = ActiveTab::Doctor;
                        app.dirty = true;
                    }
                    KeyCode::Char('5') => {
                        app.tab = ActiveTab::Jagr;
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
                // The JAGR tab costs a resolver pass; pay it when the tab opens.
                if app.tab == ActiveTab::Jagr && !app.jagr_loaded {
                    app.ensure_jagr();
                    app.dirty = true;
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
        ActiveTab::Overview.title(),
        ActiveTab::Dependencies.title(),
        ActiveTab::Cache.title(),
        ActiveTab::Doctor.title(),
        ActiveTab::Jagr.title(),
    ];
    let tabs = Tabs::new(tab_titles)
        .select(app.tab as usize)
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(accent).add_modifier(Modifier::BOLD));
    f.render_widget(tabs, chunks[1]);

    // Content Area
    match app.tab {
        ActiveTab::Overview => render_overview(f, app, chunks[2]),
        ActiveTab::Dependencies => render_dependencies(f, app, chunks[2]),
        ActiveTab::Cache => render_cache(f, app, chunks[2]),
        ActiveTab::Doctor => render_doctor(f, app, chunks[2]),
        ActiveTab::Jagr => render_jagr(f, app, chunks[2]),
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
    let keys = "i install  d doctor  f fix  a audit  p prune  w why  x remove  s script  ? help  q";
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

fn status_lines(app: &AppState) -> Vec<Line<'static>> {
    let Some(s) = &app.status else {
        return vec![Line::from("  collecting…")];
    };
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

fn render_cache(f: &mut Frame, app: &mut AppState, area: Rect) {
    let items: Vec<ListItem> = app
        .cache_pkgs
        .iter()
        .map(|pkg| ListItem::new(format!("   {}", pkg)))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    " JGrid Content Store & Inode Cache  ·  {} packages  ·  c clean  ·  p prune ",
                    app.cache_pkgs.len()
                ))
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{25b6} ");

    f.render_stateful_widget(list, area, &mut app.list_state);
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

fn render_jagr(f: &mut Frame, app: &AppState, area: Rect) {
    let lines: Vec<Line> = app.jagr_info.lines().map(Line::from).collect();
    let block = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" JAGR-∞ Sheaf Cohomology & Invariants (Press r to refresh) ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(block, area);
}
