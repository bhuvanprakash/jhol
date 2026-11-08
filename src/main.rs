//! Thin CLI layer: parse args, styled output, and call into jhol-core.
//! Crash-proof: panic caught and reported; all errors return Result.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod tui;

use clap::{Arg, ArgAction, Command};
use clap_complete;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// Global guard: lazy init for anticipatory watcher (spawned once after first install with >3 deps)
static ANTI_WATCHER_STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

// ── colour helpers (no-op when stdout isn't a TTY) ────────────────────────────

fn use_color() -> bool {
    std::io::stdout().is_terminal() && env::var("NO_COLOR").unwrap_or_default().is_empty()
}

fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

fn success(msg: &str) {
    if use_color() {
        println!("{}", msg.green());
    } else {
        println!("{}", msg);
    }
}

fn error(msg: &str) {
    if use_color() {
        eprintln!("{}", msg.red());
    } else {
        eprintln!("{}", msg);
    }
}

#[allow(dead_code)]
fn warning(msg: &str) {
    if use_color() {
        eprintln!("{}", msg.yellow());
    } else {
        eprintln!("{}", msg);
    }
}

fn info(msg: &str) {
    if use_color() {
        println!("{}", msg.cyan());
    } else {
        println!("{}", msg);
    }
}

fn dim(msg: &str) {
    if use_color() {
        println!("{}", msg.dimmed());
    } else {
        println!("{}", msg);
    }
}

#[allow(dead_code)]
fn dim_err(msg: &str) {
    if use_color() {
        eprintln!("{}", msg.dimmed());
    } else {
        eprintln!("{}", msg);
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", t)
    }
}

// ── Progress & install display ─────────────────────────────────────────────────

/// Animated spinner styles used across all commands.
fn spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .tick_strings(&["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"])
        .template("{spinner:.magenta.bold} {msg}")
        .unwrap()
}

#[allow(dead_code)]
fn bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {bar:38.cyan/dim}  {pos:>3}/{len:3}  {msg:.dim}")
        .unwrap()
        .progress_chars("▰▰▱")
}

/// Live install progress driven by jhol-core's lock-free atomic counters.
///
/// The install runs on a background thread; this renders a real-time view
/// (a progress bar + per-stage counts + throughput) from `progress_snapshot()`
/// on the main thread until the install completes. Captivating, zero-cost to
/// the install itself (it only reads atomics).
struct InstallProgress {
    #[allow(dead_code)]
    mp: MultiProgress,
    /// Row 1: the resolver has no per-package total, so this row is a comet that
    /// travels instead of a bar that fills - a filling bar that then empties reads as
    /// "dancing" rather than progress.
    resolve: ProgressBar,
    /// Row 2: packages fetched (cache + network) against the resolved total.
    fetch: ProgressBar,
    /// Row 3: packages linked into node_modules.
    link: ProgressBar,
    /// Row 4: the cache/network split, dim.
    detail: ProgressBar,
    enabled: bool,
    started: std::time::Instant,
    /// Packages the install was asked for. `ProgressSnapshot::total` is only known
    /// once resolution finishes, so the rows used to show a meaningless 0/1.
    requested: u64,
    /// Counter values at construction. A lockfile/capsule replay marks packages as
    /// linked before the resolver runs, which made the "linking" row read 344/344
    /// while nothing had happened yet.
    base_linked: u64,
    base_fetched: u64,
}

impl InstallProgress {
    fn new(pkg_count: usize, quiet: bool, json_out: bool) -> Self {
        let enabled = !quiet && !json_out && use_color() && is_tty();
        let mp = MultiProgress::new();
        // Snapshot before any row is drawn: this is the baseline the rows subtract.
        let baseline = jhol_core::progress::snapshot();
        let ticks = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let bar_style = || {
            ProgressStyle::default_bar()
                .template("  {spinner:.141}  {msg:<11} {bar:28.141/dim} {pos:>3}/{len:<3}")
                .unwrap()
                .tick_strings(&ticks)
                .progress_chars("▰▱")
        };
        // The resolver row draws its own travelling comet inside the message, so it
        // has no `{bar}` of its own.
        let comet_style = || {
            ProgressStyle::default_spinner()
                .template("  {spinner:.141}  {msg}")
                .unwrap()
                .tick_strings(&ticks)
        };
        let rows = if enabled {
            let resolve = mp.add(ProgressBar::new_spinner());
            resolve.set_style(comet_style());
            resolve.set_message(format!("{:<11} {}", "resolving", Self::comet(0)));
            resolve.enable_steady_tick(Duration::from_millis(80));
            let fetch = mp.add(ProgressBar::new(pkg_count.max(1) as u64));
            fetch.set_style(bar_style());
            fetch.set_message("fetching");
            fetch.enable_steady_tick(Duration::from_millis(80));
            let link = mp.add(ProgressBar::new(pkg_count.max(1) as u64));
            link.set_style(bar_style());
            link.set_message("linking");
            link.enable_steady_tick(Duration::from_millis(80));
            let detail = mp.add(ProgressBar::new_spinner());
            detail.set_style(
                ProgressStyle::default_spinner()
                    .template("     {msg:.dim}")
                    .unwrap(),
            );
            (resolve, fetch, link, detail)
        } else {
            (
                ProgressBar::hidden(),
                ProgressBar::hidden(),
                ProgressBar::hidden(),
                ProgressBar::hidden(),
            )
        };
        Self {
            mp,
            resolve: rows.0,
            fetch: rows.1,
            link: rows.2,
            detail: rows.3,
            enabled,
            started: std::time::Instant::now(),
            requested: pkg_count.max(1) as u64,
            base_linked: baseline.linked,
            base_fetched: baseline.from_cache + baseline.from_network,
        }
    }

    /// A block of filled cells travelling across the row, wrapping around. Unlike a
    /// growing bar it never "completes", so it reads as continuous work rather than
    /// filling up and resetting.
    fn comet(tick: usize) -> String {
        const CELLS: usize = 28;
        const HEAD: usize = 6;
        let accent = "▰".truecolor(170, 140, 255);
        let dim = "▱".truecolor(70, 70, 90);
        let start = tick % CELLS;
        (0..CELLS)
            .map(|i| {
                let d = (i + CELLS - start) % CELLS;
                if d < HEAD {
                    accent.to_string()
                } else {
                    dim.to_string()
                }
            })
            .collect()
    }

    /// Refresh the bar + detail line from a live progress snapshot.
    fn render(&self, snap: &jhol_core::ProgressSnapshot) {
        if !self.enabled {
            return;
        }
        let total = self.requested.max(snap.total).max(1);
        let elapsed = self.started.elapsed().as_secs();
        let tick = (self.started.elapsed().as_millis() / 90) as usize;

        // Row 1 - resolver: comet, no total to report yet.
        // The comet is drawn into the message so the row shows a travelling block
        // instead of a filling bar (indicatif's {bar} can only fill a prefix).
        // `{:<11} ` puts the comet where the other rows put their bar.
        let comet_msg = format!("{:<11} {}", "resolving", Self::comet(tick));
        self.resolve.set_message(comet_msg);

        // Row 2 - fetch: cache + network against the resolved total.
        let fetched = (snap.from_cache + snap.from_network).saturating_sub(self.base_fetched);
        self.fetch.set_length(total);
        self.fetch.set_position(fetched.min(total));
        self.fetch.set_message("fetching");

        // Row 3 - link: what has landed in node_modules.
        self.link.set_length(total);
        self.link
            .set_position(snap.linked.saturating_sub(self.base_linked).min(total));
        self.link.set_message("linking");

        let mut detail = if fetched > 0 {
            format!(
                "{} from cache · {} from network",
                snap.from_cache.saturating_sub(self.base_fetched),
                snap.from_network
            )
        } else {
            "building the dependency graph".to_string()
        };
        if snap.bytes_downloaded > 0 {
            detail.push_str(&format!(" · {}", snap.downloaded_label()));
        }
        if snap.failed > 0 {
            detail.push_str(&format!(" · {} failed", snap.failed));
        }
        detail.push_str(&format!(" · {}s", elapsed));
        self.detail.set_message(detail);
    }

    fn finish(self) {
        if self.enabled {
            self.resolve.finish_and_clear();
            self.fetch.finish_and_clear();
            self.link.finish_and_clear();
            self.detail.finish_and_clear();
        }
    }
}

/// Set when a fast lane declined or pruned because the tree disagreed with the
/// lockfile. The summary must not then say "Up to date": the install repaired
/// something, and saying otherwise is the same class of lie the verification
/// exists to stop.
static TREE_REPAIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Take the per-project install lock for commands that write `node_modules`.
///
/// Returns a guard held until the process exits. SMART: commands that only read
/// (`jhol view`, `jhol fmt`, `jhol run`) never take it, so they cannot block on
/// a concurrent install.
fn install_command_lock() -> Option<jhol_core::project_lock::ProjectLock> {
    let sub = std::env::args().nth(1)?;
    let writes_tree = matches!(
        sub.as_str(),
        "install"
            | "i"
            | "ci"
            | "add"
            | "remove"
            | "rm"
            | "uninstall"
            | "update"
            | "dedupe"
            | "prune"
    );
    if !writes_tree {
        return None;
    }
    // Reconcile before the lock even: an unauthorised directory needs no
    // resolution to remove, and doing it here means every tree-writing command
    // starts from a tree that at least matches the lockfile's *set*. It also
    // records the repair so whichever summary prints later says "Repaired"
    // instead of "Up to date" (the entry short-circuit above only covers plain
    // installs, so `install --frozen-lockfile` and `ci` used to repair silently).
    {
        use jhol_core::tree_state::{reconcile_tree, verify_tree};
        let before = verify_tree(Path::new("."));
        let _ = reconcile_tree(Path::new("."));
        // The tree disagreed with the lockfile when we arrived, so whatever the
        // command ends up doing, its summary must not say "Up to date".
        if !before.is_match()
            && !matches!(before, jhol_core::tree_state::TreeState::Unverified { .. })
        {
            TREE_REPAIRED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    match jhol_core::project_lock::ProjectLock::acquire(Path::new(".")) {
        Ok(lock) => Some(lock),
        Err(e) => {
            eprintln!("jhol: {}", e);
            std::process::exit(1);
        }
    }
}

/// Ask the tree whether anything needs doing, reconcile whatever needs no
/// resolution to fix, and report honestly. `true` means the install is already
/// done.
///
/// This exists so "is it up to date?" is answered in exactly one place. Before
/// it, five gates answered with five kinds of evidence — an mtime marker, a
/// lockfile fingerprint, a prepared-tree proof, `nm_fresh()`, a daemon verdict —
/// and none of them could see a package.json edited inside a package, or a
/// directory the lockfile never authorised.
fn tree_is_current_and_reported(quiet: bool) -> bool {
    use jhol_core::tree_state::{reconcile_tree, verify_tree};
    let before = verify_tree(Path::new("."));
    let state = reconcile_tree(Path::new("."));
    if !before.is_match() && !matches!(before, jhol_core::tree_state::TreeState::Unverified { .. })
    {
        // The tree disagreed with the lockfile when we arrived, so whatever the
        // install ends up doing, the summary must not say "Up to date".
        TREE_REPAIRED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let jhol_core::tree_state::TreeState::Extraneous { path } = &state {
        // Installed but not in the lockfile, and pruning is opt-in because a
        // lockfile can be an incomplete record of what it installed (see
        // jhol-treecheck). Report it plainly, keep the tree, and do not re-run
        // the install on every invocation for a superset.
        if !quiet {
            eprintln!(
                "jhol: warning: {} is not in the lockfile (prune with JHOL_PRUNE_EXTRANEOUS=1)",
                path
            );
        }
        let total = jhol_core::count_installed_packages(Path::new("."));
        if !quiet {
            println!("Up to date ({} packages in node_modules)", total);
        }
        return true;
    }
    if !state.is_match() {
        if env::var("JHOL_VERBOSE").is_ok() {
            eprintln!("[jhol] tree needs work: {}", state.describe());
        }
        return false;
    }
    let repaired = TREE_REPAIRED.swap(false, std::sync::atomic::Ordering::Relaxed);
    if !quiet {
        let total = jhol_core::count_installed_packages(Path::new("."));
        if repaired {
            println!("Repaired ({} packages in node_modules)", total);
        } else {
            println!("Up to date ({} packages in node_modules)", total);
        }
    }
    true
}

/// For a plain `jhol install` / `jhol i` with no package arguments and no flags
/// that change what is installed: if the tree already matches the lockfile,
/// report it and finish — no clap parsing, no workspace walk, no daemon, no
/// lane.
fn plain_install_fully_settled() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first() else {
        return false;
    };
    if first != "install" && first != "i" {
        return false;
    }
    for arg in &args[1..] {
        match arg.as_str() {
            "-q" | "--quiet" | "--native-only" | "--offline" | "--no-scripts" => {}
            _ => return false,
        }
    }
    let quiet = args.iter().any(|a| a == "-q" || a == "--quiet")
        || std::env::var("JHOL_QUIET").is_ok();
    tree_is_current_and_reported(quiet)
}

/// Read a boolean flag without panicking when the argument is not declared
/// for this subcommand.
///
/// `ArgMatches::get_flag` asserts that the id exists *and* is a `SetTrue` flag;
/// when the two clap builders drifted, `jhol ci` crashed with an internal clap
/// assertion (`arg 'engine-strict''s ArgAction should be one of SetTrue,
/// SetFalse`) instead of doing anything. A missing flag means "not requested",
/// which is what an absent flag means everywhere else, so this is also the more
/// honest default: `ci` never accepted `--engine-strict`, so it is off.
fn flag(m: &clap::ArgMatches, name: &str) -> bool {
    m.try_get_one::<bool>(name).ok().flatten().copied().unwrap_or(false)
}

/// Print the final install summary line using accurate per-install metrics.
/// Summary semantics from before/after package counts on disk. A fresh tree
/// reports the full count as `added`; no change reports "up to date" instead
/// of the lying "Added 0 packages"; growth reports only the delta.
/// Returns (added, up_to_date if added == 0 and total > 0).
fn install_summary_counts(before: usize, after: usize) -> (usize, bool) {
    let added = after.saturating_sub(before);
    let up_to_date = added == 0 && after > 0;
    (added, up_to_date)
}

fn print_install_summary(
    added: usize,
    total: usize,
    elapsed: Duration,
    quiet: bool,
    json_out: bool,
) {
    if quiet || json_out {
        return;
    }
    let ms = elapsed.as_millis();
    let speed = match ms {
        0..=9 => format!("{}instant{}", "\x1b[1;36m", "\x1b[0m"),
        10..=99 => format!("{}{}ms{}", "\x1b[1;36m", ms, "\x1b[0m"),
        100..=999 => format!("{}{}ms{}", "\x1b[36m", ms, "\x1b[0m"),
        _ => format!("{}{:.2}s{}", "\x1b[33m", elapsed.as_secs_f64(), "\x1b[0m"),
    };

    let up_to_date = added == 0 && total > 0;
    if use_color() {
        if up_to_date {
            println!();
            println!(
                "  {}  {}  {} {} in tree",
                "✓".green().bold(),
                "up to date".white().bold(),
                format!("{}", total).white().bold(),
                "packages".dimmed()
            );
            println!("  {}  {}", "│".dimmed(), speed);
            println!();
            return;
        }
        let pkg_word = if added == 1 { "package" } else { "packages" };
        println!();
        println!(
            "  {}  {} {}  {}",
            "✓".green().bold(),
            format!("{}", added).white().bold(),
            pkg_word.dimmed(),
            speed,
        );
        if total != added {
            println!("  {}  {} in node_modules", "│".dimmed(), total);
        }

        // Accurate cache hit rate from per-install progress counters.
        let snap = jhol_core::progress_snapshot();
        let done = snap.from_cache + snap.from_network;
        if done > 0 {
            let rate = snap.cache_rate();
            let rate_str = if rate >= 99.9 {
                format!("{:.0}%", rate).green().bold().to_string()
            } else if rate >= 50.0 {
                format!("{:.0}%", rate).yellow().to_string()
            } else {
                format!("{:.0}%", rate).cyan().to_string()
            };
            let mut detail = format!(
                "  {}  cache {} hit  •  {} cached  •  {} downloaded",
                "│".dimmed(),
                rate_str,
                snap.from_cache.to_string().green(),
                snap.from_network.to_string().cyan(),
            );
            if snap.bytes_downloaded > 0 {
                detail.push_str(&format!("  •  {}", snap.downloaded_label().dimmed()));
            }
            println!("{}", detail);
        }
        println!();
    } else {
        let ms_or_s = if ms < 1000 {
            format!("{}ms", ms)
        } else {
            format!("{:.2}s", elapsed.as_secs_f64())
        };
        if up_to_date && TREE_REPAIRED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            println!("Repaired ({} packages in node_modules)", total);
        } else if up_to_date {
            println!("Up to date ({} packages in node_modules)", total);
        } else {
            let extra = if total != added {
                format!(" ({} in node_modules)", total)
            } else {
                String::new()
            };
            println!("Added {} packages in {}{}", added, ms_or_s, extra);
        }
    }
}

/// Report `engines.node` mismatches for the installed tree. Warns by default;
/// with `strict`, returns an error so the caller can fail the install.
/// Warn (or fail, under strict) when the project pins a different package
/// manager. Silent divergence produces a tree nobody else can reproduce.
fn check_package_manager_pin(project: &std::path::Path, strict: bool, quiet: bool) -> Result<(), String> {
    let verdict = jhol_core::pm_field::check(project);
    if verdict.is_ok() {
        return Ok(());
    }
    let msg = verdict.message().unwrap_or_default();
    if strict || jhol_core::pm_field::strict() {
        return Err(format!("{} (refusing: --pm-strict)", msg));
    }
    if !quiet {
        eprintln!("warning: {}", msg);
    }
    Ok(())
}

fn report_engine_mismatches(project: &std::path::Path, strict: bool, quiet: bool) -> Result<(), String> {
    let mismatches = jhol_core::engines::scan_installed(project);
    if std::env::var_os("JHOL_OMIT_DEBUG").is_some() {
        eprintln!("[engines] scan {:?} -> {} mismatch(es)", project, mismatches.len());
    }
    if mismatches.is_empty() {
        return Ok(());
    }
    let shown: Vec<String> = mismatches
        .iter()
        .take(5)
        .map(|(n, req, act)| format!("{} needs {} (have {})", n, req, act))
        .collect();
    let more = if mismatches.len() > 5 {
        format!("; ... and {} more", mismatches.len() - 5)
    } else {
        String::new()
    };
    if strict {
        return Err(format!(
            "{} package(s) declare engines.node above this runtime: {}{}",
            mismatches.len(),
            shown.join("; "),
            more
        ));
    }
    if !quiet {
        eprintln!(
            "warning: {} package(s) declare engines.node above this runtime: {}{}",
            mismatches.len(),
            shown.join("; "),
            more
        );
    }
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", n)
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

/// Print a friendly, actionable error with consistent formatting.
fn print_error(cmd: &str, msg: &str) {
    if use_color() {
        eprintln!();
        eprintln!("  {} {}", "✗".red().bold(), msg.red());
        if !cmd.is_empty() {
            eprintln!("  {} command: {}", "│".dimmed(), cmd.dimmed());
        }
        eprintln!(
            "  {} Run {} for details",
            "╰".dimmed(),
            "jhol doctor --explain".cyan()
        );
        eprintln!();
    } else {
        eprintln!("Error: {}", msg);
        if !cmd.is_empty() {
            eprintln!("Command: {}", cmd);
        }
    }
}

/// Run a long-running task; in quiet mode show a spinner until done.
#[allow(dead_code)]
fn run_with_spinner<F>(message: &str, quiet: bool, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    if !quiet {
        return f();
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = f();
        let _ = tx.send(result);
    });
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(spinner_style());
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(Duration::from_millis(80));
    let mut elapsed = Duration::ZERO;
    let timeout = Duration::from_secs(600);
    let tick = Duration::from_millis(80);
    loop {
        match rx.try_recv() {
            Ok(res) => {
                spinner.finish_and_clear();
                return res;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                spinner.finish_and_clear();
                return Err("Operation failed.".to_string());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if elapsed >= timeout {
            spinner.finish_and_clear();
            return Err("Operation timed out.".to_string());
        }
        thread::sleep(tick);
        elapsed += tick;
    }
}

#[cfg(unix)]
fn daemon_socket_path() -> std::path::PathBuf {
    if let Ok(v) = env::var("JHOL_DAEMON_SOCKET") {
        return std::path::PathBuf::from(v);
    }
    // Prefer the stable per-user runtime path so an auto-started daemon remains
    // discoverable by later jhol processes.
    if let Ok(xdg) = env::var("XDG_RUNTIME_DIR") {
        return std::path::PathBuf::from(xdg).join("jhol-install.sock");
    }
    std::env::temp_dir().join("jhol-install.sock")
}

/// Phase 1+2: Predictive Anticipatory Daemon — watches package.json,
/// trains Markov from git history, predicts next adds, pre-fetches deps
/// based on DSSC spectral certificates + GRM/JMR/lattice resolution,
/// and pre-builds capsule + status so trampoline exits 0ms.
///
/// When package.json changes:
/// 1. Train Markov from git log incrementally
/// 2. Predict next 3 likely package adds
/// 3. For each predicted: DSSC cert → GRM/JMR/lattice → JAGR-S prefetch
/// 4. Pre-build capsule + write status.bin for predicted state
fn anticipatory_daemon_loop(cwd: &Path) {
    let pj_path = cwd.join("package.json");
    if !pj_path.exists() {
        return;
    }

    let mut last_mtime = std::fs::metadata(&pj_path)
        .ok()
        .and_then(|m| m.modified().ok());
    let mut last_trained_commit = String::new();
    let mut last_predicted_deps: Vec<(String, String)> = Vec::new();

    loop {
        for _ in 0..20 {
            if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let current_mtime = std::fs::metadata(&pj_path)
            .ok()
            .and_then(|m| m.modified().ok());

        if current_mtime == last_mtime {
            continue;
        }
        last_mtime = current_mtime;

        // Load current deps
        let deps = match jhol_core::lockfile::read_package_json_deps(&pj_path) {
            Some(d) if !d.is_empty() => d,
            _ => continue,
        };

        // ── 1.1 Git-aware Markov training (incremental) ──
        let git_dir = cwd.join(".git");
        if git_dir.exists() {
            let _ = train_markov_from_git(cwd, &mut last_trained_commit);
        }

        // ── 1.2 Predict next adds from Markov ──
        // Use current deps as context: take last 2 dep names for 2nd-order Markov key
        let dep_names: Vec<&String> = deps.keys().collect();
        let predicted: Vec<String> = if dep_names.len() >= 2 {
            let prev = dep_names[dep_names.len() - 2];
            let cur = dep_names[dep_names.len() - 1];
            jhol_core::second_order_markov::predict_with_context(prev, cur, 3)
        } else if dep_names.len() == 1 {
            // First-order fallback: predict similar from embedding if available
            let cur = dep_names[0];
            jhol_core::second_order_markov::predict_with_context("", cur, 2)
        } else {
            Vec::new()
        };

        // ── 1.2+1.3 DSSC-certified prefetch + GRM/JMR fusion ──
        let cache_dir = jhol_core::utils::get_cache_dir();
        let cache_dir_s = cache_dir.clone();

        // Prefetch all current deps (existing behavior) + predicted packages
        let mut all_to_prefetch: Vec<(String, String)> = Vec::new();

        // Existing deps prefetch (already in original code)
        for (name, spec) in &deps {
            all_to_prefetch.push((name.clone(), spec.clone()));
        }

        // Add Markov prediction with latest version spec
        let latest_known_versions: std::collections::HashMap<String, String> = {
            let mut map = std::collections::HashMap::new();
            for pkg_name in &predicted {
                // Try GRM for known version, else use "*"
                let spec = format!("{}@latest", pkg_name);
                let version = if let Some(rec) = jhol_core::lock_free_grm::query_global(
                    &spec,
                    Some(std::env::consts::OS),
                    std::env::var("NODE_VERSION").ok().as_deref(),
                ) {
                    rec.resolved_version
                } else if let Some(v) = jhol_core::resolution_mirror::global_mirror()
                    .lookup(pkg_name, "latest")
                    .map(|m| m.version.to_string())
                {
                    v
                } else {
                    "*".to_string()
                };
                map.insert(pkg_name.clone(), version);
            }
            map
        };

        for pkg_name in &predicted {
            let spec = latest_known_versions
                .get(pkg_name)
                .cloned()
                .unwrap_or_else(|| "*".to_string());
            all_to_prefetch.push((pkg_name.clone(), spec));
        }

        // Deduplicate
        all_to_prefetch.sort();
        all_to_prefetch.dedup();

        let store_idx = jhol_core::utils::read_store_index();

        for (name, spec) in &all_to_prefetch {
            let base = if name.starts_with('@') {
                let parts: Vec<&str> = name.split('/').collect();
                if parts.len() >= 2 {
                    format!("{}/{}", parts[0], parts[1])
                } else {
                    name.clone()
                }
            } else {
                name.clone()
            };
            let spec_str = format!("{}@{}", base, spec);

            // Skip if already cached
            if store_idx.contains_key(&spec_str) {
                continue;
            }

            // DSSC cert check: prefetch if Stable, skip if Fragile
            if let Some(cert) = jhol_core::jagr::resolution_differential::lookup_dssc_cert(name) {
                if cert.stability == jhol_core::StabilityClass::Fragile {
                    continue;
                }
            }

            // Prefetch in background (non-blocking)
            let pkg_name = name.clone();
            let pkg_spec = spec.clone();
            let cache_dir_clone = cache_dir_s.clone();
            spawn_daemon_background("jhol-daemon-prefetch", move || {
                let url = format!(
                    "https://registry.npmjs.org/{}",
                    pkg_name.replace('/', "%2F")
                );
                if let Ok(bytes) = jhol_core::http_client::get(&url) {
                    let _ = jhol_core::jagrs::store_tarball(&bytes);
                    let _ = jhol_core::jagrs::store_alias(&pkg_name, &pkg_spec, "");
                    let _ = jhol_core::registry::download_tarball_to_store(
                        &url,
                        std::path::Path::new(&cache_dir_clone),
                        &format!("{}@{}", pkg_name, pkg_spec),
                        None,
                        None,
                    );
                    // GRM + lattice: remember this resolution for cross-project benefit
                    let _ =
                        jhol_core::lock_free_grm::store_global(jhol_core::grm::ResolutionRecord {
                            package_spec: format!("{}@{}", pkg_name, pkg_spec),
                            resolved_version: pkg_spec.clone(),
                            id: 0,
                            timestamp: jhol_core::grm::current_timestamp(),
                            ttl_secs: 7200,
                            dependencies: std::collections::HashMap::new(),
                            platform: Some(std::env::consts::OS.to_string()),
                            node_version: std::env::var("NODE_VERSION").ok(),
                            cached_at: jhol_core::grm::current_timestamp(),
                            hit_count: 1,
                            ..Default::default()
                        });
                    let _ = jhol_core::lattice_cache::remember(&pkg_name, &pkg_spec, &pkg_spec);
                }
            });
        }

        // ── 1.4+1.5 Pre-build capsule + write status for predicted state ──
        // Only if we have predicted packages different from last run
        let predicted_pairs: Vec<(String, String)> = predicted
            .iter()
            .filter_map(|p| {
                let v = latest_known_versions.get(p)?;
                Some((p.clone(), v.clone()))
            })
            .collect();

        if !predicted_pairs.is_empty() && predicted_pairs != last_predicted_deps {
            last_predicted_deps = predicted_pairs.clone();

            let cwd_clone = cwd.to_path_buf();
            let predicted_owned = last_predicted_deps.clone();
            // No `set_current_dir` in a background thread: cwd is per-process, so it
            // would move the main thread's relative paths too. The capsule APIs take
            // an explicit project directory.
            spawn_daemon_background("jhol-daemon-capsule", move || {
                // Write capsule with predicted deps included
                let predicted_refs: Vec<(&str, &str)> = predicted_owned
                    .iter()
                    .map(|(n, v)| (n.as_str(), v.as_str()))
                    .collect();
                let _ = jhol_core::capsule::write_capsule_from_deps(&cwd_clone, &predicted_refs);
                // Write status.bin for predicted state (trampoline can exit 0ms)
                let _ = jhol_core::capsule::write_cga_capsule_from_project(&cwd_clone);
            });
        } else {
            // Fallback: standard capsule pre-build
            let cwd_clone = cwd.to_path_buf();
            spawn_daemon_background("jhol-daemon-capsule", move || {
                let _ = jhol_core::capsule::write_capsule_from_project(&cwd_clone);
                let _ = jhol_core::capsule::write_cga_capsule_from_project(&cwd_clone);
            });
        }
    }
}

/// Train second-order Markov from git history incrementally.
/// Reads `git log --oneline --all -- package.json`, parses dep sequences,
/// and feeds them to the Markov model. Tracks last trained commit to avoid
/// re-training already-seen sequences.
fn train_markov_from_git(cwd: &Path, last_trained: &mut String) -> usize {
    // Get new commits (after last_trained) that modify package.json
    let output = if last_trained.is_empty() {
        std::process::Command::new("git")
            .args([
                "-C",
                &cwd.to_string_lossy(),
                "log",
                "--reverse",
                "--all",
                "--format=%H",
                "--",
                "package.json",
            ])
            .output()
    } else {
        std::process::Command::new("git")
            .args([
                "-C",
                &cwd.to_string_lossy(),
                "log",
                "--reverse",
                "--all",
                "--format=%H",
                &format!("--after={}", last_trained),
                "--",
                "package.json",
            ])
            .output()
    };

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return 0,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let commits: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    if commits.is_empty() {
        return 0;
    }

    let mut sequences: Vec<(String, Vec<String>)> = Vec::new();
    for sha in &commits {
        // git show <sha>:package.json → parse deps
        let show_out = std::process::Command::new("git")
            .args([
                "-C",
                &cwd.to_string_lossy(),
                "show",
                &format!("{}:package.json", sha),
            ])
            .output();
        let Ok(show) = show_out else { continue };
        if !show.status.success() {
            continue;
        }
        let pj_str = String::from_utf8_lossy(&show.stdout);
        let pj: serde_json::Value = match serde_json::from_str(&pj_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let dep_names: Vec<String> = pj
            .get("dependencies")
            .and_then(|d| d.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();
        if dep_names.len() >= 2 {
            sequences.push((sha.to_string(), dep_names));
        }
    }

    if sequences.is_empty() {
        return 0;
    }

    let count = jhol_core::second_order_markov::train_from_sequence_incremental(&sequences);

    // Update last trained commit
    if let Some(last) = commits.last() {
        *last_trained = last.to_string();
    }

    count
}

/// Serializes the working-directory section of the daemon.
///
/// `install_package` works on relative paths and changes the process working
/// directory, so two installs must not interleave that section. It is taken after
/// the request has been read, so a connection that never sends anything cannot hold
/// it — which is exactly what made the sequential accept loop stall the daemon.
static DAEMON_CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

static DAEMON_SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

static ACTIVE_DAEMON_BACKGROUND_TASKS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

struct DaemonBackgroundTaskPermit;

impl Drop for DaemonBackgroundTaskPermit {
    fn drop(&mut self) {
        ACTIVE_DAEMON_BACKGROUND_TASKS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn spawn_daemon_background<F>(name: &'static str, task: F)
where
    F: FnOnce() + Send + 'static,
{
    ACTIVE_DAEMON_BACKGROUND_TASKS.fetch_add(1, Ordering::AcqRel);
    let spawn = thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _permit = DaemonBackgroundTaskPermit;
            task();
        });
    if let Err(spawn_error) = spawn {
        ACTIVE_DAEMON_BACKGROUND_TASKS.fetch_sub(1, Ordering::AcqRel);
        error(&format!(
            "daemon background task failed to start: {spawn_error}"
        ));
    }
}

#[cfg(unix)]
fn active_daemon_background_tasks() -> usize {
    ACTIVE_DAEMON_BACKGROUND_TASKS.load(Ordering::Acquire)
}

#[cfg(unix)]
fn prepare_daemon_socket(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let initial = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect daemon socket {}: {error}", path.display())),
    };
    if !initial.file_type().is_socket() {
        return Err(format!(
            "refusing to replace non-socket daemon path {}",
            path.display()
        ));
    }

    match UnixStream::connect(path) {
        Ok(_) => return Err(format!("daemon already listening at {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot verify existing daemon socket {}: {error}",
                path.display()
            ));
        }
    }

    // Startup is serialized by DaemonSocketLock. Recheck the inode as well so a
    // replaced path is never mistaken for the stale socket observed above.
    match fs::symlink_metadata(path) {
        Ok(current)
            if current.file_type().is_socket()
                && current.dev() == initial.dev()
                && current.ino() == initial.ino() =>
        {
            fs::remove_file(path)
                .map_err(|error| format!("remove stale daemon socket {}: {error}", path.display()))
        }
        Ok(_) => Err(format!(
            "daemon socket changed while checking {}; refusing replacement",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("recheck daemon socket {}: {error}", path.display())),
    }
}

#[cfg(unix)]
struct DaemonSocketCleanup {
    path: PathBuf,
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl DaemonSocketCleanup {
    fn capture(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("inspect bound daemon socket {}: {error}", path.display()))?;
        if !metadata.file_type().is_socket() {
            return Err(format!(
                "bound daemon path {} is not a socket",
                path.display()
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn remove_if_owned(&self) -> Result<(), String> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        match fs::symlink_metadata(&self.path) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && metadata.dev() == self.device
                    && metadata.ino() == self.inode =>
            {
                fs::remove_file(&self.path).map_err(|error| {
                    format!("remove daemon socket {}: {error}", self.path.display())
                })
            }
            Ok(_) => Err(format!(
                "daemon socket path changed at {}; refusing removal",
                self.path.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "inspect daemon socket {} during shutdown: {error}",
                self.path.display()
            )),
        }
    }
}

#[cfg(unix)]
impl Drop for DaemonSocketCleanup {
    fn drop(&mut self) {
        let _ = self.remove_if_owned();
    }
}

#[cfg(unix)]
fn daemon_stop() -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let timeout_secs = env::var("JHOL_DAEMON_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(60);
    let socket_path = daemon_socket_path();
    let initial = fs::symlink_metadata(&socket_path)
        .map_err(|error| format!("inspect daemon socket {}: {error}", socket_path.display()))?;
    if !initial.file_type().is_socket() {
        return Err(format!(
            "refusing to contact non-socket daemon path {}",
            socket_path.display()
        ));
    }

    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|error| format!("connect to daemon {}: {error}", socket_path.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(timeout_secs)))
        .map_err(|error| format!("set daemon response timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(timeout_secs)))
        .map_err(|error| format!("set daemon request timeout: {error}"))?;
    let connected = fs::symlink_metadata(&socket_path)
        .map_err(|error| format!("recheck daemon socket {}: {error}", socket_path.display()))?;
    if !connected.file_type().is_socket()
        || connected.dev() != initial.dev()
        || connected.ino() != initial.ino()
    {
        return Err("daemon socket changed while connecting; shutdown was not requested".into());
    }

    let cwd = env::current_dir()
        .map_err(|error| format!("read current directory for daemon request: {error}"))?;
    let cwd = cwd
        .to_str()
        .ok_or("daemon cannot represent a non-UTF-8 working directory")?;
    let request = jhol_core::daemon_cache::DaemonRequest::shutdown(cwd)?;
    jhol_core::daemon_cache::write_request(&mut stream, &request)
        .map_err(|error| format!("write daemon shutdown request: {error}"))?;
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let response = jhol_core::daemon_cache::read_response(&mut stream)
        .map_err(|error| format!("read daemon shutdown response: {error}"))?;
    if response.ok != 0 {
        let message = response.error_str();
        return Err(if message.is_empty() {
            "daemon rejected shutdown request".to_string()
        } else {
            message.to_string()
        });
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match fs::symlink_metadata(&socket_path) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && metadata.dev() == initial.dev()
                    && metadata.ino() == initial.ino() => {}
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "wait for daemon socket {} to close: {error}",
                    socket_path.display()
                ));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "daemon accepted shutdown but has not drained active requests after {timeout_secs} seconds"
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn daemon_serve() -> Result<(), String> {
    let socket_path = daemon_socket_path();
    if let Some(parent) = socket_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create daemon socket directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let _socket_lock = jhol_core::daemon_cache::DaemonSocketLock::acquire(&socket_path)?;
    prepare_daemon_socket(&socket_path)?;

    let listener = UnixListener::bind(&socket_path).map_err(|e| {
        format!(
            "failed to bind daemon socket {}: {}",
            socket_path.display(),
            e
        )
    })?;
    let socket_cleanup = DaemonSocketCleanup::capture(&socket_path)?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("configure daemon listener: {error}"))?;
    DAEMON_SHUTDOWN_REQUESTED.store(false, Ordering::Release);

    // Set socket permissions to 0o600 (owner only) to prevent unauthorized access
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600));
    }

    info(&format!(
        "JHOL daemon listening on {}",
        socket_path.display()
    ));

    // Anticipatory watcher is LAZY: spawned after first successful install with >3 deps,
    // not at daemon boot. See daemon_handle_one success path.

    // One thread per connection. This used to be a sequential loop, and a single
    // client that connected and sent nothing held the whole daemon for the read
    // timeout: every real request queued behind it and the *client's first socket
    // write* blocked for the full 30 s before the install (measured: request write
    // 30.0005 s, response 30.2024 s, while the install itself is ~200 ms). Accepting
    // and handing a connection to its own thread costs a few microseconds and makes
    // a stalled peer harmless.
    //
    // Installs stay serialized: `install_package` is relative-path based and the
    // process working directory is per-process, so concurrent handlers would corrupt
    // each other's target directory (observed in an earlier threaded variant — one
    // install reported success while materializing into the daemon's own directory).
    // `DAEMON_CWD_LOCK` is taken *after* the request is read and held across the cwd
    // section, so it serializes the install without letting an idle reader block it.
    let mut accept_failure = None;
    loop {
        if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
            break;
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(accept_error) => {
                error(&format!("daemon accept failed: {accept_error}"));
                accept_failure = Some(accept_error);
                break;
            }
        };
        if let Err(connection_error) = stream.set_nonblocking(false) {
            error(&format!("configure daemon connection: {connection_error}"));
            let response = jhol_core::daemon_cache::DaemonResponse::error(
                "daemon could not configure the accepted connection",
            );
            let _ = jhol_core::daemon_cache::write_response(&mut stream, &response);
            continue;
        }
        if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
            let response =
                jhol_core::daemon_cache::DaemonResponse::error("daemon is shutting down");
            let _ = jhol_core::daemon_cache::write_response(&mut stream, &response);
            continue;
        }
        let Some(permit) = jhol_core::daemon_cache::try_daemon_connection_permit() else {
            let response = jhol_core::daemon_cache::DaemonResponse::error(
                "daemon busy: connection limit reached",
            );
            let _ = jhol_core::daemon_cache::write_response(&mut stream, &response);
            continue;
        };
        let spawn = std::thread::Builder::new()
            .name("jhol-daemon-connection".to_string())
            .spawn(move || {
                let _permit = permit;
                // A client that connects and then stalls must not hold the daemon hostage.
                let read_timeout = std::env::var("JHOL_DAEMON_READ_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|v| *v > 0)
                    .unwrap_or(30);
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(read_timeout)));
                let _ = handle_daemon_request(stream);
            });
        if let Err(e) = spawn {
            error(&format!("daemon connection worker failed to start: {e}"));
        }
    }

    drop(listener);
    while jhol_core::daemon_cache::active_daemon_connections() != 0
        || active_daemon_background_tasks() != 0
    {
        thread::sleep(Duration::from_millis(10));
    }

    // Phase 3: flush JAGR-S store on daemon shutdown
    let _ = jhol_core::jagrs::daemon_flush();
    socket_cleanup.remove_if_owned()?;

    match accept_failure {
        Some(error) => Err(format!("daemon accept loop stopped: {error}")),
        None => Ok(()),
    }
}
/// Try to install via daemon. On non-Unix, always returns an error (the daemon
/// itself is Unix-only), and `install_via_daemon` exists only on Unix - the old
/// cfg gate here removed that non-Unix branch, so the Windows build failed to
/// compile at all.
fn daemon_try_install(
    _cwd: &Path,
    _packages: &[String],
    _opts: &jhol_core::InstallOptions,
    _auto_attach: bool,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        install_via_daemon(_cwd, _packages, _opts, _auto_attach)
    }
    #[cfg(not(unix))]
    {
        Err("--daemon install is only supported on Unix".to_string())
    }
}

/// Direct install (non-daemon path) with progress reporting.
fn direct_install(
    _cwd: &Path,
    specs: &[String],
    opts: &jhol_core::InstallOptions,
    pkg_count: usize,
    quiet: bool,
    json_out: bool,
) -> Result<(), String> {
    // Zero the lock-free counters before the bar starts: a capsule/lockfile replay can
    // mark packages as linked before this point, which made the "linking" row read
    // 344/344 while the resolver had not done anything yet.
    jhol_core::progress::reset();
    let progress = InstallProgress::new(pkg_count, quiet, json_out);
    let specs_owned: Vec<String> = specs.to_vec();
    let opts_owned = opts.clone();
    if !progress.enabled {
        let refs2: Vec<&str> = specs_owned.iter().map(|s| s.as_str()).collect();

        return jhol_core::install_package(&refs2, &opts_owned).map_err(|e| e.to_string());
    }
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let worker = thread::spawn(move || {
        let refs2: Vec<&str> = specs_owned.iter().map(|s| s.as_str()).collect();
        let r = jhol_core::install_package(&refs2, &opts_owned).map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    jhol_core::progress::set_bar_active(progress.enabled);
    let install_result = loop {
        match rx.recv_timeout(Duration::from_millis(70)) {
            Ok(r) => break r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let snap = jhol_core::progress_snapshot();
                progress.render(&snap);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("install worker terminated unexpectedly".to_string());
            }
        }
    };
    let _ = worker.join();
    jhol_core::progress::set_bar_active(false);
    progress.finish();
    // Markers (write_status, store_last_hash, fingerprint) are written by
    // install_package via write_install_markers() — single write.
    install_result
}

#[cfg(unix)]
fn install_via_daemon(
    cwd: &Path,
    packages: &[String],
    opts: &jhol_core::InstallOptions,
    auto_attach: bool,
) -> Result<(), String> {
    let socket_path = daemon_socket_path();

    // Try to connect
    let stream_result = UnixStream::connect(&socket_path);
    if stream_result.is_err() {
        if auto_attach {
            // Safety gate: auto-attach only connects to existing socket.
            // Don't start daemon (avoids capsule write race). Fall back.
            return Err("no daemon socket found; falling back to direct install".to_string());
        }
        if !opts.quiet {
            info("Daemon not running — auto-starting...");
        }
        start_daemon_process(&socket_path)?;
    }

    // Connect (retry once)
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("daemon unavailable at {}: {}", socket_path.display(), e))?;
    // Bounded: an unresponsive or half-dead daemon must not park an install for
    // ten minutes. A local install reply is fast; 60 s is already far beyond it.
    let client_timeout = std::env::var("JHOL_DAEMON_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(60);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(client_timeout)));

    // Packed struct — zero serde, zero alloc
    let cwd_str = cwd
        .to_str()
        .ok_or("daemon cannot represent a non-UTF-8 working directory")?;
    let req = jhol_core::daemon_cache::DaemonRequest::try_new(
        cwd_str,
        packages,
        opts.quiet,
        opts.no_cache,
        opts.offline,
        opts.strict_lockfile,
        opts.strict_peer_deps,
        opts.from_lockfile,
        opts.lockfile_only,
        opts.native_only,
        opts.no_scripts,
    )
    .map_err(|error| format!("cannot use daemon for this request: {error}"))?;
    jhol_core::daemon_cache::write_request(&mut stream, &req)
        .map_err(|e| format!("daemon write failed: {}", e))?;
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let resp = jhol_core::daemon_cache::read_response(&mut stream)
        .map_err(|e| format!("daemon read failed: {}", e))?;
    if resp.ok == 0 {
        Ok(())
    } else {
        let msg = resp.error_str();
        Err(if msg.is_empty() {
            "daemon install failed".to_string()
        } else {
            msg.to_string()
        })
    }
}

/// Start a persistent daemon process and wait until its socket accepts clients.
#[cfg(unix)]
fn start_daemon_process(socket_path: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let executable =
        env::current_exe().map_err(|error| format!("locate jhol executable: {error}"))?;
    let mut command = std::process::Command::new(executable);
    command
        .args(["daemon", "--serve"])
        .env("JHOL_DAEMON_SOCKET", socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("start jhol daemon: {error}"))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(socket_path) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(connect_error) => match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!(
                        "jhol daemon exited before becoming ready ({status}): {connect_error}"
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("check jhol daemon startup: {error}"));
                }
            },
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("jhol daemon did not become ready within 5 seconds".to_string());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Handle a single daemon request (extracted for reuse by auto-start).
#[cfg(unix)]
fn handle_daemon_request(stream: std::os::unix::net::UnixStream) -> Result<(), String> {
    daemon_handle_one(stream)
}

/// Handle one daemon connection (the actual logic, extracted from daemon_serve).
#[cfg(unix)]
fn daemon_handle_one(mut stream: std::os::unix::net::UnixStream) -> Result<(), String> {
    // Read packed request struct — single read, zero parse
    let req = match jhol_core::daemon_cache::read_request(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            let resp = jhol_core::daemon_cache::DaemonResponse::error(&format!("read: {}", e));
            let _ = jhol_core::daemon_cache::write_response(&mut stream, &resp);
            return Ok(());
        }
    };

    if req.is_shutdown() {
        DAEMON_SHUTDOWN_REQUESTED.store(true, Ordering::Release);
        let response = jhol_core::daemon_cache::DaemonResponse::success();
        let _ = jhol_core::daemon_cache::write_response(&mut stream, &response);
        return Ok(());
    }

    let cwd = req.cwd_str().to_string();
    if cwd.is_empty() || cwd.len() > 4096 || cwd.contains('\0') {
        let resp = jhol_core::daemon_cache::DaemonResponse::error("invalid cwd");
        let _ = jhol_core::daemon_cache::write_response(&mut stream, &resp);
        return Ok(());
    }

    let packages = req.packages();
    if packages.len() > 1000 {
        let resp = jhol_core::daemon_cache::DaemonResponse::error("too many packages");
        let _ = jhol_core::daemon_cache::write_response(&mut stream, &resp);
        return Ok(());
    }

    let quiet = req.quiet();
    let no_cache = req.no_cache();
    let offline = req.offline();
    let strict_lockfile = req.strict_lockfile();
    let strict_peer_deps = req.strict_peer_deps();
    let from_lockfile = req.from_lockfile();
    let lockfile_only = req.lockfile_only();
    let native_only = req.native_only();
    let no_scripts = req.no_scripts();
    // The daemon request does not carry --omit (it is a per-invocation
    // intent, and the daemon is an optimization lane). Honour the process
    // environment so a production daemon still omits dev dependencies.
    let omitted = jhol_core::omit::Omit::NONE.with_env(false);

    // Serialize the cwd-dependent section (see DAEMON_CWD_LOCK). Poisoning is
    // ignored: a panic in one install must not permanently break the daemon.
    let _cwd_guard = DAEMON_CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if DAEMON_SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
        let response = jhol_core::daemon_cache::DaemonResponse::error("daemon is shutting down");
        let _ = jhol_core::daemon_cache::write_response(&mut stream, &response);
        return Ok(());
    }
    let prev_cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        std::env::set_current_dir(&cwd).map_err(|e| format!("chdir {}: {}", cwd, e))?;

        // Keep the daemon process warm, but let install_package own the
        // lockfile-bound proof and capsule validation. The old daemon-local
        // status/fingerprint checks hashed only package.json and could skip
        // `ci`, changed locks, or missing nested paths.

        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        let opts = jhol_core::InstallOptions {
            no_cache,
            quiet,
            backend: if native_only {
                jhol_core::Backend::Npm
            } else {
                jhol_core::resolve_backend(None)
            },
            lockfile_only,
            offline,
            strict_lockfile,
            strict_peer_deps,
            from_lockfile,
            native_only,
            no_scripts,
            script_allowlist: None,
            resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
            overrides: None,
            telemetry: false,
            omit: omitted,
            engine_strict: false,
        };
        jhol_core::set_daemon_mode(true);
        jhol_core::set_transitive_depth(2);

        let install_result = jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string());
        // SNR speculation: after successful install, pre-compute neighbor resolutions.
        // Gated behind JHOL_ANTICIPATORY=1 AND >3 deps (small projects don't need speculation).
        if install_result.is_ok() && !packages.is_empty() {
            let ant_gate = std::env::var("JHOL_ANTICIPATORY")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if ant_gate {
                if let Some(deps) =
                    jhol_core::lockfile::read_package_json_deps(Path::new("package.json"))
                {
                    if deps.len() > 3 {
                        let fp = jhol_core::resolution_mirror::fingerprint_deps(&deps);
                        jhol_core::daemon_cache::speculate_neighborhoods(Path::new("."), &deps, fp);
                    }
                }
            }
        }
        // CGA capsule write after successful install
        if install_result.is_ok() {
            let cga_gate = std::env::var("JHOL_CGA")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(true);
            if cga_gate {
                let _ = jhol_core::capsule::write_cga_capsule_from_project(Path::new("."));
            }
            // Phase 3: populate JAGR-S daemon store from lockfile
            let _ = jhol_core::jagrs::daemon_populate_from_lockfile(Path::new("."));
            // Markers written by install_package via write_install_markers().
        }
        install_result
    })();
    let _ = std::env::set_current_dir(prev_cwd);

    let _payload = match result {
        Ok(_) => {
            // Ensure capsule + CGA capsule exist for next fast-path boot.
            // Markers are written by the inner install handler.
            let _ = jhol_core::capsule::write_capsule_from_project(Path::new("."));
            let _ = jhol_core::capsule::write_cga_capsule_from_project(Path::new("."));
            // Lazy spawn anticipatory watcher (once) if JHOL_ANTICIPATORY=1 and >3 deps.
            if ANTI_WATCHER_STARTED
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
                && std::env::var("JHOL_ANTICIPATORY")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false)
            {
                if let Some(deps) =
                    jhol_core::lockfile::read_package_json_deps(Path::new("package.json"))
                {
                    if deps.len() > 3 {
                        let wc: std::path::PathBuf = cwd.clone().into();
                        spawn_daemon_background("jhol-daemon-watcher", move || {
                            anticipatory_daemon_loop(&wc);
                        });
                    }
                }
            }
            // Store resolved graph + pin artifacts
            let cap_path = jhol_core::capsule::capsule_path(Path::new("."));
            if cap_path.exists() {
                if let Ok(view) = jhol_core::capsule::read_capsule(&cap_path) {
                    let hash = view.manifest_hash();
                    jhol_core::daemon_cache::store_capsule_view(hash, view);
                    jhol_core::daemon_cache::repin_install_artifacts(Path::new("."));
                    if let Some(tree) = jhol_core::lockfile::read_resolved_from_dir(Path::new("."))
                    {
                        let mut resolved = std::collections::HashMap::new();
                        for (spec, _version) in &tree {
                            if let Some(_name) = spec.split('@').next() {
                                let entry = jhol_core::lockfile_write::ResolvedPackage {
                                    version: _version.clone(),
                                    resolved: String::new(),
                                    integrity: None,
                                    dependencies: std::collections::HashMap::new(),
                                    peer_dependencies: std::collections::HashMap::new(),
                                    peer_dependencies_meta: std::collections::HashMap::new(),
                                };
                                resolved.insert(spec.clone(), entry);
                            }
                        }
                        jhol_core::daemon_cache::store_resolved_graph(hash, resolved);
                    }
                }
            }
            // Pre-warm CAS + predictive prefetch in background
            let _packages = packages.clone();
            let cwd = cwd.clone();
            spawn_daemon_background("jhol-daemon-install-prefetch", move || {
                let lock_path = std::path::Path::new(&cwd).join("package-lock.json");
                if let Ok(bytes) = std::fs::read(&lock_path) {
                    if let Ok(lock) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        if let Some(pkgs) = lock.get("packages").and_then(|p| p.as_object()) {
                            // Soul Path predictive prefetch from installed graph
                            let all_pkgs: Vec<String> = pkgs
                                .keys()
                                .filter(|k| !k.is_empty())
                                .map(|k| k.strip_prefix("node_modules/").unwrap_or(k).to_string())
                                .collect();
                            if all_pkgs.len() >= 2 {
                                jhol_core::soul_path::predictive_prefetch(&all_pkgs, 3);
                            }
                            // Pre-warm CAS in background
                            for (key, pkg) in pkgs {
                                if let Some(resolved) = pkg.get("resolved").and_then(|r| r.as_str())
                                {
                                    let name = key.strip_prefix("node_modules/").unwrap_or(key);
                                    let version =
                                        pkg.get("version").and_then(|v| v.as_str()).unwrap_or("");
                                    jhol_core::registry::download_tarball_to_store(
                                        resolved,
                                        &std::path::PathBuf::from(jhol_core::utils::get_cache_dir()),
                                        &format!("{}@{}", name, version),
                                        None,
                                        None,
                                    ).ok();
                                }
                            }
                        }
                    }
                }
            });
            // Write install markers unconditionally after any success.
            // Simple hash of package.json — no deps, always works.
            let pj_path = std::path::Path::new("package.json");
            let hash = if pj_path.exists() {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                if let Ok(s) = std::fs::read_to_string(pj_path) {
                    let mut h = DefaultHasher::new();
                    s.hash(&mut h);
                    Some(h.finish().to_le_bytes())
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(h) = hash {
                jhol_core::daemon_cache::write_nm_fingerprint(std::path::Path::new("."), h);
                jhol_core::daemon_cache::store_last_hash(h);
                jhol_core::daemon_cache::write_status(h, std::path::Path::new("."));
            } else {
                // If no package.json, still write status with zero hash.
                // Trampoline only checks nm_fresh byte — hash is ignored.
                jhol_core::daemon_cache::write_status([0; 8], std::path::Path::new("."));
            }
            let resp = jhol_core::daemon_cache::DaemonResponse::success();
            let _ = jhol_core::daemon_cache::write_response(&mut stream, &resp);
        }
        Err(e) => {
            let resp = jhol_core::daemon_cache::DaemonResponse::error(&e);
            let _ = jhol_core::daemon_cache::write_response(&mut stream, &resp);
        }
    };
    Ok(())
}

fn install_globally() -> Result<(), String> {
    let bin_dir = if cfg!(target_os = "windows") {
        let p = env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Program Files".to_string());
        format!("{}\\jhol", p)
    } else {
        "/usr/local/bin".to_string()
    };
    let bin_path = format!("{}/jhol-bin", bin_dir);
    let tramp_path = format!("{}/jhol", bin_dir);

    let exe_path = env::current_exe().map_err(|e| e.to_string())?;

    // Full CLI → jhol-bin; thin Rust trampoline → jhol (warm no-op without loading CLI).
    info(&format!("Installing binary at {} …", &bin_path));
    fs::copy(&exe_path, &bin_path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        let meta = fs::metadata(&bin_path).map_err(|e| e.to_string())?;
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin_path, perms).map_err(|e| e.to_string())?;
    }

    // Prefer Instant-Lock C front door (~34 KB, spawn floor), then Rust trampoline.
    let tramp_src = {
        let mut front = exe_path.clone();
        front.set_file_name(if cfg!(windows) {
            "jhol-front.exe"
        } else {
            "jhol-front"
        });
        if front.is_file() {
            front
        } else {
            let mut sibling = exe_path.clone();
            sibling.set_file_name(if cfg!(windows) {
                "jhol-trampoline.exe"
            } else {
                "jhol-trampoline"
            });
            sibling
        }
    };
    if tramp_src.is_file() {
        info(&format!(
            "Installing warm trampoline from {} …",
            tramp_src.display()
        ));
        fs::copy(&tramp_src, &tramp_path).map_err(|e| e.to_string())?;
        // The C front execs an adjacent `jhol-trampoline` FIRST (warm proof +
        // projection lanes); place it too, so the chain is
        // jhol(front) → jhol-trampoline(lanes) → jhol-bin(full CLI) instead of
        // skipping straight to the full CLI spawn.
        let mut rust_tramp = exe_path.clone();
        rust_tramp.set_file_name(if cfg!(windows) {
            "jhol-trampoline.exe"
        } else {
            "jhol-trampoline"
        });
        if rust_tramp.is_file() && rust_tramp != tramp_src {
            let dest = format!(
                "{}/jhol-trampoline{}",
                bin_dir,
                if cfg!(windows) { ".exe" } else { "" }
            );
            if fs::copy(&rust_tramp, &dest).is_ok() {
                #[cfg(unix)]
                if let Ok(m) = fs::metadata(&dest) {
                    let mut p = m.permissions();
                    p.set_mode(0o755);
                    let _ = fs::set_permissions(&dest, p);
                }
            }
        }
    } else {
        // Dev/single-binary fallback: shell trampoline (slower warm path).
        info("jhol-front/jhol-trampoline not next to this binary; installing shell trampoline (build -p jhol-trampoline for Instant Lock)");
        let trampoline = r#"#!/bin/sh
exec "$(dirname "$0")/jhol-bin" "$@"
"#;
        fs::write(&tramp_path, trampoline.as_bytes()).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&tramp_path)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tramp_path, perms).map_err(|e| e.to_string())?;
    }

    // Auto-start daemon as systemd/launchd service
    #[cfg(target_os = "linux")]
    {
        let service_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
            .join("systemd/user");
        let _ = fs::create_dir_all(&service_dir);
        let service_path = service_dir.join("jhol-daemon.service");
        let service_content = format!(
            "[Unit]\n\
             Description=Jhol Predictive Daemon\n\
             After=network.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={bin} daemon --serve\n\
             Restart=always\n\
             RestartSec=2\n\
             Environment=JHOL_DAEMON_SOCKET=%t/jhol-%U.sock\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            bin = bin_path
        );
        if fs::write(&service_path, service_content).is_ok() {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "enable", "--now", "jhol-daemon"])
                .status();
            info("Daemon auto-started via systemd.");
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let plist_dir = std::path::PathBuf::from(&home).join("Library/LaunchAgents");
        let _ = fs::create_dir_all(&plist_dir);
        let plist_path = plist_dir.join("com.jhol.daemon.plist");
        let plist_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.jhol.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>daemon</string>
        <string>--serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/jhol-daemon.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/jhol-daemon.log</string>
</dict>
</plist>"#,
            bin = bin_path
        );
        if fs::write(&plist_path, plist_content).is_ok() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", plist_path.to_str().unwrap_or("")])
                .status();
            let _ = std::process::Command::new("launchctl")
                .args(["load", plist_path.to_str().unwrap_or("")])
                .status();
            info("Daemon auto-started via launchd.");
        }
    }

    success(&format!(
        "Installed. `{}` is the fast trampoline, `{}` is the native binary.",
        tramp_path, bin_path
    ));
    Ok(())
}


/// A complete, valid `jhol.toml` for `jhol dev` (schema 1).
const DEV_CONFIG_EXAMPLE: &str = r#"schema = 1

[[service]]
name = "db"
kind = "process"
run = ["docker", "compose", "up", "db"]
ready_tcp = "127.0.0.1:5432"

[[service]]
name = "api"
kind = "process"
needs = ["db"]
run = ["node", "server.js"]
cwd = "."
"#;

fn read_optional_text(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// True when `node_modules` exists and has at least one entry.
fn node_modules_present(root: &std::path::Path) -> bool {
    std::fs::read_dir(root.join("node_modules"))
        .map(|mut d| d.next().is_some())
        .unwrap_or(false)
}

/// The snapshot with the latest `created_at`. Snapshot ids are random UUIDs, so
/// id order says nothing about time.
fn latest_snapshot(root: &std::path::Path) -> Result<Option<jhol_receipt::Snapshot>, String> {
    let mut best: Option<jhol_receipt::Snapshot> = None;
    for (id, _) in jhol_receipt::Snapshot::list(root).map_err(|e| format!("snapshot list: {e}"))? {
        let snap =
            jhol_receipt::Snapshot::load(root, &id).map_err(|e| format!("snapshot {id}: {e}"))?;
        if best.as_ref().is_none_or(|b| snap.created_at > b.created_at) {
            best = Some(snap);
        }
    }
    Ok(best)
}

/// `jhol diff`: compare the project's manifest, npm lock and installed tree with a
/// snapshot (the given id, or the latest one).
fn cmd_state_diff(
    root: &std::path::Path,
    snapshot_id: Option<String>,
    as_json: bool,
    exit_code: bool,
) -> Result<(), String> {
    let snap = match snapshot_id {
        Some(id) => jhol_receipt::Snapshot::load(root, &id).map_err(|e| format!("diff: {e}"))?,
        None => latest_snapshot(root)?.ok_or_else(|| {
            "diff: no snapshot to compare against; run `jhol snapshot create` first".to_string()
        })?,
    };
    let cur_manifest = read_optional_text(&root.join("package.json"));
    let cur_lock = read_optional_text(&root.join("package-lock.json"));
    let base = jhol_diff::FileState {
        manifest: snap.manifest.as_deref(),
        lock: snap.package_lock.as_deref(),
    };
    let now = jhol_diff::FileState {
        manifest: cur_manifest.as_deref(),
        lock: cur_lock.as_deref(),
    };
    let mut report = jhol_diff::diff_state(base, now, node_modules_present(root));
    // Files can match a snapshot while the tree on disk does not (for example
    // after restoring an older lock). Compare what is installed with the lock.
    if let (Some(lock), true) = (cur_lock.as_deref(), node_modules_present(root)) {
        let locked = jhol_diff::lock_package_versions(lock).unwrap_or_default();
        let installed: std::collections::BTreeMap<String, String> = locked
            .keys()
            .filter_map(|path| {
                let pj = root.join(path).join("package.json");
                let raw = std::fs::read_to_string(pj).ok()?;
                let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
                Some((path.clone(), v.get("version")?.as_str()?.to_string()))
            })
            .collect();
        match jhol_diff::installed_drift(lock, &installed) {
            Ok(drift) => report.mark_installed(drift),
            Err(e) => report
                .notes
                .push(format!("installed tree not compared: {e}")),
        }
    }
    if snap.manifest.is_none() {
        report
            .notes
            .push("snapshot predates manifest capture; package.json was not compared".into());
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "snapshot": snap.id,
                "report": report,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("diff against snapshot {} ({})", snap.id, snap.created_at);
        println!("  package.json:      {:?}", report.manifest);
        println!("  package-lock.json: {:?}", report.lock);
        const SHOWN: usize = 50;
        for c in report.packages.iter().take(SHOWN) {
            match (&c.before, &c.after) {
                (Some(b), Some(a)) => println!("  ~ {}  {b} -> {a}", c.path),
                (None, Some(a)) => println!("  + {}  {a}", c.path),
                (Some(b), None) => println!("  - {}  {b}", c.path),
                (None, None) => {}
            }
        }
        if report.packages.len() > SHOWN {
            println!(
                "  … {} more package changes (use --json)",
                report.packages.len() - SHOWN
            );
        }
        for n in &report.notes {
            println!("  note: {n}");
        }
        if !report.installed.is_empty() {
            println!("installed tree vs lock:");
            for c in report.installed.iter().take(SHOWN) {
                match (&c.before, &c.after) {
                    (Some(have), Some(want)) => {
                        println!("  ! {}  installed {have}, lock {want}", c.path)
                    }
                    (None, Some(want)) => println!("  ! {}  not installed, lock {want}", c.path),
                    (Some(have), None) => println!("  ! {}  installed {have}, not in lock", c.path),
                    _ => {}
                }
            }
            if report.installed.len() > SHOWN {
                println!("  … {} more (use --json)", report.installed.len() - SHOWN);
            }
        }
        if report.needs_sync {
            println!("next: jhol sync --apply");
        } else {
            println!("no changes since the snapshot; nothing to sync");
        }
    }
    if exit_code && report.needs_sync {
        std::process::exit(1);
    }
    Ok(())
}

fn print_splash() {
    let version = env!("CARGO_PKG_VERSION");
    // The platform is not decoration: it is the machine every platform-specific
    // dependency is filtered against, so the header states it instead of implying it.
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let width = term_width();
    if use_color() {
        let name = "jhol".truecolor(235, 225, 255).bold();
        let ver = format!("v{}", version).truecolor(170, 140, 255);
        println!("  {name} {ver}   {}", platform.truecolor(140, 140, 165));
        println!(
            "  {}",
            "cache-first · offline-capable · npm-compatible".truecolor(105, 105, 130)
        );
        println!("{}", "─".repeat(width).truecolor(70, 70, 90));
        println!();
    } else {
        println!("jhol v{version}   {platform}");
        println!("cache-first · offline-capable · npm-compatible");
        println!("{}", "-".repeat(width));
        println!();
    }
}

/// Visible terminal width, used so rules and panels span the real window instead of
/// a guessed constant (the old banner's rule was hard-coded and looked cut short).
fn term_width() -> usize {
    // A PTY without a window size reports 0, and clamping that up to 40 produced a
    // stubby rule - the exact thing the header was meant to fix. Fall back to a
    // sensible width instead of trusting a degenerate value.
    match crossterm::terminal::size() {
        Ok((w, _)) if w >= 40 => (w as usize).min(120),
        _ => 78,
    }
}

/// Some(Ok((before, after))) on a fast lane: before/after are the
/// node_modules package counts around the lane, for the honest summary.
fn try_fast_install() -> Option<Result<(usize, usize), String>> {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    if env::var("JHOL_DEBUG_INTEGRITY").is_ok() {
        eprintln!("[lane] try_fast_install entered");
    }
    let is_plain = if raw_args.is_empty() {
        Path::new("package.json").is_file()
    } else {
        let mut has_install_cmd = false;
        let mut valid_flags = true;
        for arg in &raw_args {
            match arg.as_str() {
                "install" | "i" => has_install_cmd = true,
                "-q" | "--quiet" | "--native-only" | "--no-scripts" | "--offline" => {}
                _ => {
                    valid_flags = false;
                    break;
                }
            }
        }
        has_install_cmd && valid_flags
    };

    if !is_plain {
        return None;
    }

    // Count what is on disk before any fast lane runs, so the caller can say
    // honestly whether the tree changed and by how many packages.
    let count_before = jhol_core::count_installed_packages(Path::new("."));
    // "The prepared tree is valid" is a statement about a proof, not about the
    // tree. A package.json edited on disk and a directory the lockfile never
    // authorises both satisfied it, and this lane returns before
    // install_package() can notice. Verify the tree itself here, and prune what
    // the lockfile does not authorise (that needs no resolution).
    let tree_ok = {
        let state = jhol_core::tree_state::reconcile_tree(Path::new("."));
        if !state.is_match() && env::var("JHOL_VERBOSE").is_ok() {
            eprintln!("[jhol] fast lane declined: {}", state.describe());
        }
        state.is_match()
    };
    let result = if tree_ok && jhol_core::instant_lock_proof::prepared_tree_is_valid(Path::new(".")) {
        // 1. Zero-cost recheck: node_modules already matches the lockfile
        Some(Ok(()))
    } else if jhol_core::instant_lock_proof::try_direct_bundle_projection(Path::new(".")) {
        // 2. Direct L0 bundle projection from cache
        Some(Ok(()))
    } else if jhol_core::try_fresh_project_bundle_replay(Path::new(".")) {
        // 2b. Fresh project (no lockfile yet): replay the bundle published by a
        // previous byte-identical install — same contract as a committed lockfile.
        Some(Ok(()))
    } else {
        // 3. Bare-metal fast warm materialization via parallel APFS clonefile(2)
        jhol_core::fast_direct_warm_install()
    };
    // Every lane here completed an install: run the lifecycle phase so script
    // consent is enforced (and skipped scripts surfaced) on the fast path too.
    if result.as_ref().map(|r| r.is_ok()).unwrap_or(false) {
        let _ = check_package_manager_pin(std::path::Path::new("."), false, true);
        let _ = jhol_core::patch::apply_all(std::path::Path::new("."));
        // The fast lane is the one that runs for a plain `jhol install`, so
        // the engines report has to live here too, not only at the clap paths.
        report_engine_mismatches(
            std::path::Path::new("."),
            std::env::args().any(|a| a == "--engine-strict"),
            std::env::args().any(|a| a == "-q" || a == "--quiet"),
        )
        .ok();
        if let Err(e) = jhol_core::install::run_post_install_lifecycle_default() {
            return Some(Err(e));
        }
    }
    // All three lanes return before clap, so the install_package wrapper never
    // runs here — uphold the integrity promise for every successful fast lane.
    if let Some(Ok(())) = &result {
        if env::var("JHOL_DEBUG_INTEGRITY").is_ok() {
            eprintln!("[lane] try_fast_install success -> backfill");
        }
        jhol_core::backfill_lockfile_integrity_after_install();
    }
    // Attach the counts only on success so the error type stays a plain String.
    result.map(|r| {
        r.map(move |()| {
            (
                count_before,
                jhol_core::count_installed_packages(Path::new(".")),
            )
        })
    })
}

fn collect_fmt_files(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if root.is_file() {
        out.push(root.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "node_modules" || name == "target" || name == ".git" || name == "dist" {
            continue;
        }
        if p.is_dir() {
            collect_fmt_files(&p, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

/// .prettierignore → globset. Gitignore-style trailing "/" means
/// "everything under this directory"; negations ("!") are not supported yet.
fn load_prettier_ignore(dir: &std::path::Path) -> globset::GlobSet {
    use globset::GlobSetBuilder;
    let mut builder = GlobSetBuilder::new();
    if let Ok(raw) = std::fs::read_to_string(dir.join(".prettierignore")) {
        for line in raw.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with('!') {
                continue;
            }
            let pattern = if l.ends_with('/') {
                format!("{}/**", l.trim_end_matches('/'))
            } else {
                l.to_string()
            };
            if let Ok(g) = globset::Glob::new(&pattern) {
                builder.add(g);
            }
        }
    }
    builder.build().unwrap_or_default()
}

fn run() -> Result<(), String> {
    let fast_start = std::time::Instant::now();
    if let Some(fast_res) = try_fast_install() {
        let fast_elapsed = fast_start.elapsed();
        match fast_res {
            Ok((before, after)) => {
                // The ONE convergence point. Regardless of which lane won, the
                // tree must now agree with the lockfile; a lane that returns
                // "done" while the tree still disagrees has not finished the
                // job, and claiming success here is how a tampered package and
                // an unauthorised directory both survived an install.
                let state = jhol_core::tree_state::reconcile_tree(Path::new("."));
                if state.is_match() {
                    let (added, _) = install_summary_counts(before, after);
                    let fast_quiet = env::var("JHOL_QUIET").is_ok()
                        || env::args().any(|a| a == "-q" || a == "--quiet");
                    if added == 0 && env::var("JHOL_VERBOSE").is_ok() {
                        eprintln!("[jhol] fast lane verified: {}", state.describe());
                    }
                    print_install_summary(added, after, fast_elapsed, fast_quiet, false);
                    return Ok(());
                }
                if env::var("JHOL_VERBOSE").is_ok() {
                    eprintln!("[jhol] fast lane incomplete: {}", state.describe());
                }
                TREE_REPAIRED.store(true, std::sync::atomic::Ordering::Relaxed);
                // Fall through to the full CLI install, which can resolve and
                // materialise whatever is actually missing or wrong.
            }
            Err(e) => return Err(e),
        }
    }

    // Splash first: it states which machine platform-specific packages are being
    // filtered for, and it is the one piece of branding the CLI prints.
    let show_splash = env::var("JHOL_NO_SPLASH").unwrap_or_default().is_empty()
        && std::io::stdout().is_terminal();
    if show_splash {
        print_splash();
    }

    fn read_trusted_dependencies(path: &Path) -> HashSet<String> {
        // Read trustedDependencies from the given path (package.json or .jholrc)
        let Ok(raw) = fs::read_to_string(path) else {
            return HashSet::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return HashSet::new();
        };
        v.get("trustedDependencies")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default()
    }

    let matches = cli_command().get_matches();

    // global-install (no cache needed)
    if let Some(("global-install", _)) = matches.subcommand() {
        install_globally().map_err(|e| e.to_string())?;
        return Ok(());
    }

    // ── jhol gui ─ Terminal GUI ──────────────────────────────────────────────
    if let Some(("gui", _)) = matches.subcommand() {
        return tui::run_tui();
    }

    // ── jhol completions ─ Shell completion generation ───────────────────────
    if let Some(("completions", sub_m)) = matches.subcommand() {
        use clap_complete::{
            generate,
            shells::{Bash, Elvish, Fish, PowerShell, Zsh},
        };
        let shell = sub_m
            .get_one::<String>("shell")
            .map(|s| s.as_str())
            .unwrap_or("bash");
        // Build a fresh copy of the CLI for completions generation
        let mut cmd = cli_command();
        let mut stdout = std::io::stdout();
        match shell {
            "bash" => {
                generate(Bash, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.bashrc:\n# eval \"$(jhol completions bash)\"");
            }
            "zsh" => {
                generate(Zsh, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.zshrc:\n# eval \"$(jhol completions zsh)\"");
            }
            "fish" => {
                generate(Fish, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.config/fish/config.fish:\n# jhol completions fish | source");
            }
            "powershell" => {
                generate(PowerShell, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to your PowerShell profile:\n# jhol completions powershell | Out-String | Invoke-Expression");
            }
            "elvish" => {
                generate(Elvish, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.elvish/rc.elv:\n# eval (jhol completions elvish)");
            }
            _ => return Err(format!("Unknown shell: {}", shell)),
        }
        return Ok(());
    }

    // ── jhol mcp ─ Model Context Protocol Server for AI Agents ───────────────
    if let Some(("mcp", _sub_m)) = matches.subcommand() {
        return jhol_core::mcp::run_mcp_server_stdio();
    }

    // ── jhol merge-lock ─ 3-Way Git Lockfile Merge Resolver ───────────────────
    if let Some(("merge-lock", sub_m)) = matches.subcommand() {
        let base = sub_m
            .get_one::<String>("base")
            .map(std::path::Path::new)
            .unwrap_or(std::path::Path::new("package-lock.json"));
        let ours = sub_m
            .get_one::<String>("ours")
            .map(std::path::Path::new)
            .unwrap_or(std::path::Path::new("package-lock.json"));
        let theirs = sub_m
            .get_one::<String>("theirs")
            .map(std::path::Path::new)
            .unwrap_or(std::path::Path::new("package-lock.json"));
        let out = sub_m
            .get_one::<String>("output")
            .map(std::path::Path::new)
            .unwrap_or(std::path::Path::new("package-lock.json"));

        let res = jhol_core::git_lifecycle::merge_3way_lockfiles(base, ours, theirs, out)?;
        success(&format!(
            "Successfully merged lockfiles: {} packages ({} conflict(s) resolved) -> {}",
            res.total_packages_merged, res.conflicts_resolved, res.output_path
        ));
        return Ok(());
    }

    // ── jhol bisect-deps ─ Automated Dependency Bisection ─────────────────────
    if let Some(("bisect-deps", sub_m)) = matches.subcommand() {
        let test_cmd = sub_m
            .get_one::<String>("test")
            .ok_or("Missing --test command")?;
        info(&format!(
            "Starting automated dependency bisection with test: '{}'",
            test_cmd
        ));
        let res = jhol_core::git_lifecycle::bisect_dependency_upgrades(&[], test_cmd)?;
        if let Some(culprit) = res.culprit_package {
            warning(&format!("Culprit package upgrade identified: {}", culprit));
        } else {
            success("No culprit identified (tests passed on evaluated states).");
        }
        return Ok(());
    }

    // ── jhol git-hook ─ Git Hook Auto-Install ────────────────────────────────
    if let Some(("git-hook", sub_m)) = matches.subcommand() {
        if let Some(("install", _)) = sub_m.subcommand() {
            let msg = jhol_core::git_lifecycle::install_git_hooks(std::path::Path::new("."))?;
            success(&msg);
            return Ok(());
        }
        return Err("use `jhol git-hook install` to install Git hooks".to_string());
    }

    if let Some(("daemon", sub_m)) = matches.subcommand() {
        if flag(&sub_m, "serve") && flag(&sub_m, "stop") {
            return Err("choose either `jhol daemon --serve` or `jhol daemon --stop`".to_string());
        }
        if flag(&sub_m, "stop") {
            #[cfg(unix)]
            {
                daemon_stop()?;
                success("Jhol daemon stopped after active requests finished.");
                return Ok(());
            }
            #[cfg(not(unix))]
            {
                return Err("daemon mode is only supported on Unix".to_string());
            }
        }
        if flag(&sub_m, "serve") {
            #[cfg(unix)]
            {
                daemon_serve()?;
                return Ok(());
            }
            #[cfg(not(unix))]
            {
                return Err("daemon mode is only supported on Unix".to_string());
            }
        }
        return Err("use `jhol daemon --serve` or `jhol daemon --stop`".to_string());
    }

    // cache list | size | prune | export | import | clean
    if let Some(("cache", sub)) = matches.subcommand() {
        // `cache clean` must work in exactly the state that breaks cache
        // initialisation: a read-only or foreign-owned cache directory is when
        // you need it most. Every other subcommand still requires a usable
        // cache, because it reads or writes one.
        if let Err(e) = jhol_core::init_cache() {
            if !matches!(sub.subcommand(), Some(("clean", _))) {
                return Err(format!("Failed to initialize cache: {}", e));
            }
            if std::env::var("JHOL_VERBOSE").is_ok() {
                eprintln!("[jhol] cache init failed ({e}); continuing with clean");
            }
        }
        match sub.subcommand() {
            Some(("list", _)) => {
                let list = jhol_core::list_cached_packages()
                    .map_err(|e| format!("Failed to list cache: {}", e))?;
                let (bytes, _) = jhol_core::cache_size_bytes()
                    .map_err(|e| format!("Failed to get cache size: {}", e))?;
                if list.is_empty() {
                    dim("No cached packages.");
                } else {
                    info(&format!("Cached packages ({})", list.len()));
                    for name in list {
                        println!("  {}", name);
                    }
                    dim(&format!("Total size: {} MB", bytes / 1024 / 1024));
                }
            }
            Some(("size", _)) => {
                let (bytes, count) = jhol_core::cache_size_bytes()
                    .map_err(|e| format!("Failed to get cache size: {}", e))?;
                info(&format!(
                    "Cache: {} tarball(s), {} MB",
                    count,
                    bytes / 1024 / 1024
                ));
            }
            Some(("prune", sub_prune)) => {
                let keep = sub_prune.get_one::<usize>("keep").copied();
                let n = jhol_core::cache_prune(keep)
                    .map_err(|e| format!("Failed to prune cache: {}", e))?;
                success(&format!("Pruned {} tarball(s).", n));
            }
            Some(("gc", gc_m)) => {
                let dry = flag(&gc_m, "dry-run");
                let mut lockfiles: Vec<std::path::PathBuf> = gc_m
                    .get_many::<String>("from")
                    .map(|v| v.map(std::path::PathBuf::from).collect())
                    .unwrap_or_default();
                if lockfiles.is_empty() {
                    lockfiles.push(std::path::PathBuf::from("package-lock.json"));
                }
                match jhol_core::registry_query::store_gc(&lockfiles, dry) {
                    Ok((kept, freed, files)) => {
                        if dry {
                            println!(
                                "gc (dry run): {} tarball(s) reachable, {} file(s) / {} freeable",
                                kept,
                                files,
                                human_bytes(freed)
                            );
                        } else {
                            println!(
                                "gc: {} tarball(s) kept, freed {} file(s) / {}",
                                kept,
                                files,
                                human_bytes(freed)
                            );
                        }
                    }
                    Err(e) => return Err(format!("cache gc: {}", e)),
                }
            }
            Some(("export", sub_exp)) => {
                let dir = sub_exp
                    .get_one::<String>("dir")
                    .map(|s| s.as_str())
                    .unwrap();
                let n = jhol_core::cache_export(std::path::Path::new(dir))
                    .map_err(|e| format!("Export failed: {}", e))?;
                success(&format!("Exported {} package(s) to {}.", n, dir));
            }
            Some(("import", sub_imp)) => {
                let dir = sub_imp
                    .get_one::<String>("dir")
                    .map(|s| s.as_str())
                    .unwrap();
                let n = jhol_core::cache_import(std::path::Path::new(dir))
                    .map_err(|e| format!("Import failed: {}", e))?;
                success(&format!("Imported {} package(s) from {}.", n, dir));
            }
            Some(("clean", _)) => {
                let report = jhol_core::cache_clean_report()
                    .map_err(|e| format!("Failed to clean cache: {}", e))?;
                if report.freed_bytes > 0 && report.removed > 0 {
                    success(&format!(
                        "Removed {} cached package(s), freed {}.",
                        report.removed,
                        human_bytes(report.freed_bytes)
                    ));
                } else {
                    success(&format!("Removed {} cached package(s).", report.removed));
                }
                if !report.failed.is_empty() {
                    eprintln!(
                        "jhol: {} entr{} could not be removed:",
                        report.failed.len(),
                        if report.failed.len() == 1 { "y" } else { "ies" }
                    );
                    for (path, reason) in report.failed.iter().take(5) {
                        eprintln!("  {}  ({})", path.display(), reason);
                    }
                    if report.failed.len() > 5 {
                        eprintln!("  … and {} more", report.failed.len() - 5);
                    }
                    if let Some(hint) = report.hint() {
                        eprintln!("\n{}", hint);
                    }
                    // Partial failure is failure: a script that runs
                    // `jhol cache clean` and checks the status must not be told
                    // the cache is empty when entries survived.
                    return Err(format!(
                        "{} cached entr{} could not be removed",
                        report.failed.len(),
                        if report.failed.len() == 1 { "y" } else { "ies" }
                    ));
                }
            }
            Some(("telemetry", _)) => {
                let v = jhol_core::read_fallback_telemetry();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
                );
            }
            Some(("key", _)) => {
                let hash = jhol_core::lockfile_content_hash(std::path::Path::new("."))
                    .unwrap_or_else(|| "none".to_string());
                println!("{}", hash);
            }
            Some(("mirror", sub)) => match sub.subcommand() {
                Some(("stats", _)) => {
                    let stats = jhol_core::resolution_mirror::global_mirror().stats();
                    info(&format!("JMR Resolution Mirror:"));
                    println!("  Total entries:    {}", stats.total_entries);
                    println!("  Fresh entries:    {}", stats.fresh_entries);
                    println!("  Expired entries:  {}", stats.expired_entries);
                    if stats.total_entries > 0 {
                        let pct = (stats.fresh_entries as f64 / stats.total_entries as f64) * 100.0;
                        println!("  Fresh ratio:      {:.1}%", pct);
                    }
                }
                Some(("prune", _)) => {
                    let n = jhol_core::resolution_mirror::global_mirror()
                        .prune()
                        .map_err(|e| format!("Failed to prune mirror: {}", e))?;
                    success(&format!("Pruned {} expired mirror entries.", n));
                }
                Some(("clear", _)) => {
                    jhol_core::resolution_mirror::global_mirror()
                        .clear()
                        .map_err(|e| format!("Failed to clear mirror: {}", e))?;
                    success("Cleared all mirror entries.");
                }
                Some(("lookup", sub_l)) => {
                    let pkg = sub_l
                        .get_one::<String>("package")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let (name, range) = if pkg.contains('@') && !pkg.starts_with('@') {
                        let parts: Vec<&str> = pkg.splitn(2, '@').collect();
                        (parts[0], if parts.len() > 1 { parts[1] } else { "*" })
                    } else if pkg.starts_with('@') {
                        let at_idx = pkg.rfind('@').unwrap_or(0);
                        if at_idx > 0 {
                            (&pkg[..at_idx], &pkg[at_idx + 1..])
                        } else {
                            (pkg, "*")
                        }
                    } else {
                        (pkg, "*")
                    };
                    match jhol_core::resolution_mirror::global_mirror().lookup(name, range) {
                        Some(entry) => {
                            info(&format!("Mirror entry for {}@{}", name, range));
                            println!("  Version:          {}", entry.version);
                            println!("  Dep signature:    {}", entry.dep_signature);
                            println!("  Dependencies:     {}", entry.deps.len());
                            println!(
                                "  Tarball URL:      {}",
                                entry.tarball_url.as_deref().unwrap_or("(none)")
                            );
                            println!(
                                "  Integrity:        {}",
                                entry.integrity.as_deref().unwrap_or("(none)")
                            );
                        }
                        None => dim(&format!("No mirror entry for {}@{}", name, range)),
                    }
                }
                _ => {
                    dim("Use `jhol cache mirror stats`, `jhol cache mirror prune`, `jhol cache mirror clear`, or `jhol cache mirror lookup <package>`.");
                }
            },
            _ => {
                dim("Use `jhol cache list`, `jhol cache size`, `jhol cache prune`, `jhol cache export <dir>`, `jhol cache import <dir>`, `jhol cache clean`, `jhol cache telemetry`, or `jhol cache key`.");
            }
        }
        return Ok(());
    }

    if let Some(("import-lock", sub_m)) = matches.subcommand() {
        let from = sub_m
            .get_one::<String>("from")
            .map(|s| s.as_str())
            .unwrap_or("auto");
        let msg = jhol_core::import_lockfile(from).map_err(|e| e.to_string())?;
        success(&msg);
        return Ok(());
    }

    if let Some(("run", sub_m)) = matches.subcommand() {
        let script_or_file = sub_m
            .get_one::<String>("script")
            .unwrap()
            .as_str()
            .to_string();
        let is_file = flag(&sub_m, "file");
        let watch = flag(&sub_m, "watch");
        let engine = sub_m
            .get_one::<String>("engine")
            .map(|s| s.as_str())
            .unwrap_or("auto");

        // If --file flag is set, run with Jhol runtime
        if is_file
            || std::path::Path::new(&script_or_file).exists()
                && (script_or_file.ends_with(".js")
                    || script_or_file.ends_with(".ts")
                    || script_or_file.ends_with(".jsx")
                    || script_or_file.ends_with(".tsx")
                    || script_or_file.ends_with(".mjs")
                    || script_or_file.ends_with(".cjs"))
        {
            if engine == "js-engine" && !cfg!(feature = "js-engine") {
                return Err(
                    "JSEngine backend not enabled. Rebuild with --features js-engine".to_string(),
                );
            }
            #[cfg(feature = "runtime")]
            {
                let path = std::path::Path::new(&script_or_file);
                if watch {
                    jhol_runtime::run_file_watch(path).map_err(|e| e.to_string())?;
                } else {
                    let code = jhol_runtime::run_file(path).map_err(|e| e.to_string())?;
                    std::process::exit(code);
                }
                return Ok(());
            }
            #[cfg(not(feature = "runtime"))]
            {
                return Err("Jhol runtime not enabled. Rebuild with --features runtime".to_string());
            }
        }

        // An argument that looks like a path must be reported as a missing
        // file: falling through to package.json lookup produced
        // `Error: package.json has no "scripts" object.` for
        // `jhol run ./nope.js`, which says nothing about the actual problem.
        {
            let looks_like_file = {
                let p = std::path::Path::new(&script_or_file);
                script_or_file.contains('/')
                    || script_or_file.ends_with(".js")
                    || script_or_file.ends_with(".ts")
                    || script_or_file.ends_with(".jsx")
                    || script_or_file.ends_with(".tsx")
                    || script_or_file.ends_with(".mjs")
                    || script_or_file.ends_with(".cjs")
                    || (p.extension().is_some() && is_file)
            };
            if looks_like_file {
                let p = std::path::Path::new(&script_or_file);
                if p.is_dir() {
                    return Err(format!("{} is a directory, not a file", script_or_file));
                }
                if !p.exists() {
                    return Err(format!("no such file: {}", script_or_file));
                }
            }
        }

        // Existing behavior: run a package.json script
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let roots: Vec<std::path::PathBuf> = if flag(&sub_m, "all-workspaces") {
            let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
            if r.is_empty() {
                vec![cwd]
            } else {
                r.into_iter().map(|p| cwd.join(p)).collect()
            }
        } else {
            vec![cwd]
        };
        let mut last_code = 0i32;
        for root in &roots {
            let status =
                jhol_core::run_script(&script_or_file, root, None).map_err(|e| e.to_string())?;
            last_code = status.code().unwrap_or(1);
        }
        std::process::exit(last_code);
    }

    // jhol eval — evaluate inline JavaScript/TypeScript
    if let Some(("eval", sub_m)) = matches.subcommand() {
        let code = sub_m.get_one::<String>("code").unwrap();
        let file_type = sub_m
            .get_one::<String>("type")
            .map(|s| s.as_str())
            .unwrap_or("js");
        let filename = format!("<eval>.{}", file_type);

        // `jhol eval '1+1'` (like `node -e`) discards the completion value.
        // `--print` matches `node -p` / `bun -e`: evaluate the code as an
        // expression and print its value. The user's code is passed as a
        // string through `eval()` so its syntax is unchanged; TypeScript and
        // JSX are transpiled first because `eval()` sees raw source.
        let code = if flag(&sub_m, "print") {
            let transpiled = if file_type == "js" {
                code.clone()
            } else {
                #[cfg(feature = "runtime")]
                {
                    jhol_runtime::compile_ts(code, &filename).map_err(|e| e.to_string())?
                }
                #[cfg(not(feature = "runtime"))]
                {
                    let _ = &filename;
                    return Err(
                        "Jhol runtime not enabled. Rebuild with --features runtime".to_string()
                    );
                }
            };
            format!(
                "console.log(eval({}));",
                serde_json::to_string(&transpiled).unwrap_or_else(|_| "\"\"".to_string())
            )
        } else {
            code.clone()
        };

        #[cfg(feature = "runtime")]
        {
            let exit_code =
                jhol_runtime::eval_code(&code, Some(&filename)).map_err(|e| e.to_string())?;
            std::process::exit(exit_code);
        }
        #[cfg(not(feature = "runtime"))]
        {
            return Err("Jhol runtime not enabled. Rebuild with --features runtime".to_string());
        }
    }

    if let Some(("plan", sub_m)) = matches.subcommand() {
        let root = sub_m
            .get_one::<String>("path")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let as_json = flag(&sub_m, "json");
        let reg = jhol_adapters::AdapterRegistry::default_local();
        let decls = reg
            .discover_all(&root)
            .map_err(|e| format!("plan discover failed: {e}"))?;
        let plans = reg
            .plan_all(&root, true)
            .map_err(|e| format!("plan failed: {e}"))?;
        let tools = jhol_env::probe_host();
        if as_json {
            let out = serde_json::json!({
                "root": root,
                "side_effects": [],
                "note": "read-only plan; no filesystem or network mutations",
                "declarations": decls,
                "adapter_plans": plans,
                "toolchain": tools,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        } else {
            println!("jhol plan (read-only) — {}", root.display());
            if decls.is_empty() {
                println!("  (no known manifests found)");
            } else {
                println!("declarations:");
                for d in &decls {
                    println!("  • {} — {}", d.kind, d.path.display());
                }
            }
            if !plans.is_empty() {
                println!("adapter plans:");
                for p in &plans {
                    println!(
                        "  • {} ({}) — {}",
                        p.adapter.id, p.adapter.ecosystem, p.human_summary
                    );
                    for lim in &p.limitations {
                        println!("      limitation: {lim}");
                    }
                }
            }
            println!(
                "host: {}-{}  (tools probed: {})",
                tools.os,
                tools.arch,
                tools.tools.len()
            );
            println!("no side effects performed.");
        }
        return Ok(());
    }

    if let Some(("diff", sub_m)) = matches.subcommand() {
        let before_arg = sub_m.get_one::<String>("before").cloned();
        let after_arg = sub_m.get_one::<String>("after").cloned();
        // Two-receipt mode needs both files. A single argument that is not a file
        // is a snapshot id; no argument means "latest snapshot".
        let two_receipts = after_arg.is_some();
        let one_is_file = before_arg
            .as_deref()
            .is_some_and(|b| std::path::Path::new(b).is_file());
        if !two_receipts && !one_is_file {
            let root = sub_m
                .get_one::<String>("path")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
            return cmd_state_diff(
                &root,
                before_arg,
                flag(&sub_m, "json"),
                flag(&sub_m, "exit-code"),
            );
        }
        let before_path =
            before_arg.ok_or("diff: give two receipt files, or a snapshot id, or nothing")?;
        let after_path = after_arg
            .ok_or("diff: two-receipt mode needs both files (or pass only a snapshot id)")?;
        let before_path = &before_path;
        let after_path = &after_path;
        let as_json = flag(&sub_m, "json");
        let before_s =
            std::fs::read_to_string(before_path).map_err(|e| format!("read {before_path}: {e}"))?;
        let after_s =
            std::fs::read_to_string(after_path).map_err(|e| format!("read {after_path}: {e}"))?;
        let before = jhol_receipt::ExecutionReceipt::from_json(&before_s)
            .map_err(|e| format!("parse before receipt: {e}"))?;
        let after = jhol_receipt::ExecutionReceipt::from_json(&after_s)
            .map_err(|e| format!("parse after receipt: {e}"))?;
        let report = jhol_diff::diff_receipts(&before, &after);
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
        } else {
            println!(
                "diff {} → {} ({} entries)",
                before.id,
                after.id,
                report.entries.len()
            );
            for e in &report.entries {
                println!("  [{:?}] {} — {}", e.class, e.node_or_path, e.reason);
            }
        }
        return Ok(());
    }

    if let Some(("sync", sub_m)) = matches.subcommand() {
        let requested_root = sub_m
            .get_one::<String>("path")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let root = requested_root
            .canonicalize()
            .map_err(|e| format!("project root {}: {e}", requested_root.display()))?;
        let apply = flag(&sub_m, "apply");
        let as_json = flag(&sub_m, "json");
        let offline = flag(&sub_m, "offline");
        let allow_build_scripts = flag(&sub_m, "allow-build-scripts");
        let reg = jhol_adapters::AdapterRegistry::default_local();
        let adapter_refs: Vec<jhol_graph::AdapterRef> = reg
            .iter()
            .map(|adapter| {
                let id = adapter.identity();
                jhol_graph::AdapterRef {
                    id: id.id,
                    version: id.version,
                }
            })
            .collect();
        let plans = reg
            .plan_all_with_options(&root, !apply, offline)
            .map_err(|e| format!("sync planning failed: {e}"))?;
        let mut receipt = jhol_receipt::ExecutionReceipt::new(jhol_receipt::ReceiptStatus::Ok);
        receipt.command = Some("sync".into());
        receipt.project_root = Some(root.display().to_string());
        let mut notes = Vec::new();
        let plans_json = plans.clone();
        let plan_bytes =
            serde_json::to_vec(&plans_json).map_err(|e| format!("serialize adapter plans: {e}"))?;
        receipt.plan_digest = Some(jhol_graph::digest_bytes(&plan_bytes));

        if apply {
            let mut policy = jhol_policy::Policy::deny_all();
            policy.allow(jhol_policy::Capability::ReadOnlyInspect);
            policy.allow(jhol_policy::Capability::LocalPackageInstall);
            policy.allow(jhol_policy::Capability::ProcessLaunch);
            if !offline {
                policy.allow(jhol_policy::Capability::NetworkFetch);
            }
            if allow_build_scripts {
                policy.allow(jhol_policy::Capability::ArbitraryExternalCommand);
                policy.allow(jhol_policy::Capability::LifecycleScript);
            }
            // Validate every capability before any adapter can mutate the project.
            let approved = jhol_adapters::AdapterRegistry::authorize_all(plans, &policy)
                .map_err(|e| format!("sync policy denied before apply: {e}; use --allow-build-scripts only when you approve native build backends/scripts"))?;
            for approved_plan in &approved {
                reg.preflight(approved_plan)
                    .map_err(|e| format!("sync preflight failed before apply: {e}"))?;
            }
            for approved_plan in &approved {
                let plan = approved_plan.plan();
                receipt.limitations.extend(plan.limitations.iter().cloned());
                if plan.actions.is_empty() {
                    notes.push(format!(
                        "{}: no apply action (lockfile-only input)",
                        plan.adapter.ecosystem
                    ));
                    continue;
                }
                for (effect, needed) in [
                    (jhol_receipt::SideEffect::ProcessLaunch, true),
                    (jhol_receipt::SideEffect::FilesystemWrite, true),
                    (jhol_receipt::SideEffect::NetworkFetch, plan.network_access),
                    (
                        jhol_receipt::SideEffect::LifecycleScript,
                        plan.script_execution_possible,
                    ),
                ] {
                    if needed && !receipt.side_effects.contains(&effect) {
                        receipt.side_effects.push(effect);
                    }
                }
                match reg.apply(approved_plan) {
                    Ok(adapter_receipt) => {
                        notes.push(format!(
                            "applied and verified {} adapter",
                            plan.adapter.ecosystem
                        ));
                        receipt.outputs.extend(adapter_receipt.outputs.clone());
                        receipt.adapter_receipts.push(
                            serde_json::to_value(adapter_receipt)
                                .map_err(|e| format!("serialize adapter receipt: {e}"))?,
                        );
                    }
                    Err(error) => {
                        receipt.status = jhol_receipt::ReceiptStatus::Partial;
                        receipt.warnings.push(format!(
                            "{} adapter failed: {error}",
                            plan.adapter.ecosystem
                        ));
                        notes.push(format!(
                            "stopped after {} adapter failed",
                            plan.adapter.ecosystem
                        ));
                        break;
                    }
                }
            }
        } else {
            receipt.limitations.extend(
                plans
                    .iter()
                    .flat_map(|plan| plan.limitations.iter().cloned()),
            );
            notes.push("dry record only; no adapter process launched".into());
        }

        // Build this after apply so the lock describes the native files left by each adapter.
        let lock = jhol_graph::build_project_lock(&root, adapter_refs)
            .map_err(|e| format!("build project lock: {e}"))?;
        let lock_path = jhol_graph::write_project_lock(&root, &lock)
            .map_err(|e| format!("write {}: {e}", jhol_graph::PROJECT_LOCK_FILENAME))?;
        if !receipt
            .side_effects
            .contains(&jhol_receipt::SideEffect::FilesystemWrite)
        {
            receipt
                .side_effects
                .push(jhol_receipt::SideEffect::FilesystemWrite);
        }
        let lock_digest =
            jhol_graph::digest_file(&lock_path).map_err(|e| format!("digest project lock: {e}"))?;
        receipt.inputs = lock
            .native_locks
            .iter()
            .map(|native| jhol_receipt::DigestedPath {
                path: native.path.clone(),
                digest: native.digest.clone(),
                verified: Some(true),
            })
            .collect();
        receipt.outputs.push(jhol_receipt::DigestedPath {
            path: jhol_graph::PROJECT_LOCK_FILENAME.into(),
            digest: lock_digest,
            verified: Some(true),
        });
        notes.push(format!(
            "wrote {} ({} native lock ref(s))",
            lock_path.display(),
            lock.native_locks.len()
        ));

        let input_pairs: Vec<(&str, &str)> = lock
            .native_locks
            .iter()
            .map(|native| (native.path.as_str(), native.digest.as_str()))
            .collect();
        let action_key = jhol_graph::action_key("sync", &input_pairs);
        let action_cache_root = root.join(".jhol");
        let receipt_json = receipt.to_json_pretty().unwrap_or_else(|_| "{}".into());
        if receipt.status == jhol_receipt::ReceiptStatus::Ok {
            if let Ok(path) = jhol_graph::action_cache_store(
                &action_cache_root,
                &action_key,
                receipt_json.as_bytes(),
            ) {
                notes.push(format!("action-cache store {}", path.display()));
            }
        } else {
            notes.push("partial execution receipt not cached".into());
        }
        // Save the receipt for `jhol diff` and `jhol snapshot`, including partial
        // ones: a failed sync is exactly what a later diff must be able to show.
        match jhol_receipt::save_receipt(&root, &receipt) {
            Ok(path) => notes.push(format!("receipt {} → {}", receipt.id, path.display())),
            Err(e) => notes.push(format!("receipt not saved: {e}")),
        }

        if as_json {
            let out = serde_json::json!({
                "lock": lock,
                "lock_path": lock_path,
                "adapter_plans": plans_json,
                "notes": notes,
                "receipt": receipt,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        } else {
            println!("jhol sync — {}", root.display());
            for plan in &plans_json {
                println!("  plan {}: {}", plan.adapter.ecosystem, plan.human_summary);
            }
            for note in &notes {
                println!("  • {note}");
            }
            for native in &lock.native_locks {
                println!(
                    "  lock {} ({}) {}",
                    native.path,
                    native.kind,
                    &native.digest[..12.min(native.digest.len())]
                );
            }
        }
        if receipt.status != jhol_receipt::ReceiptStatus::Ok {
            return Err("sync completed partially; inspect the receipt warnings and repair the failed adapter".into());
        }
        return Ok(());
    }

    if let Some(("snapshot", sub_m)) = matches.subcommand() {
        let root = sub_m
            .get_one::<String>("path")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let action = sub_m.get_one::<String>("action").unwrap().as_str();
        let as_json = flag(&sub_m, "json");
        match action {
            "list" => {
                let list = jhol_receipt::Snapshot::list(&root)
                    .map_err(|e| format!("snapshot list: {e}"))?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&list).unwrap_or_default()
                    );
                } else if list.is_empty() {
                    println!("(no snapshots under .jhol/snapshots)");
                } else {
                    for (id, p) in list {
                        println!("{id}  {}", p.display());
                    }
                }
            }
            "show" => {
                let id = sub_m
                    .get_one::<String>("id")
                    .ok_or_else(|| "snapshot show needs <id>".to_string())?;
                let snap = jhol_receipt::Snapshot::load(&root, id)
                    .map_err(|e| format!("snapshot show: {e}"))?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&snap).unwrap_or_default()
                    );
                } else {
                    println!("snapshot {} @ {}", snap.id, snap.created_at);
                    for lim in &snap.limitations {
                        println!("  limitation: {lim}");
                    }
                }
            }
            "create" => {
                let lock_path = root.join(jhol_graph::PROJECT_LOCK_FILENAME);
                let lock_val = if lock_path.is_file() {
                    serde_json::from_str(
                        &std::fs::read_to_string(&lock_path)
                            .map_err(|e| format!("read {}: {e}", lock_path.display()))?,
                    )
                    .map_err(|e| format!("parse project.lock: {e}"))?
                } else {
                    let adapters: Vec<jhol_graph::AdapterRef> =
                        jhol_adapters::AdapterRegistry::default_local()
                            .iter()
                            .map(|a| {
                                let id = a.identity();
                                jhol_graph::AdapterRef {
                                    id: id.id,
                                    version: id.version,
                                }
                            })
                            .collect();
                    let lock = jhol_graph::build_project_lock(&root, adapters)
                        .map_err(|e| format!("build lock: {e}"))?;
                    jhol_graph::write_project_lock(&root, &lock)
                        .map_err(|e| format!("write lock: {e}"))?;
                    serde_json::to_value(&lock).unwrap_or_default()
                };
                // Attach the most recent saved `sync` receipt when there is one.
                // Otherwise say so: a snapshot must not imply a sync that never ran.
                let mut snap = match jhol_receipt::load_latest_receipt(&root) {
                    Some(receipt) => jhol_receipt::Snapshot::create(lock_val, receipt),
                    None => {
                        let mut s = jhol_receipt::Snapshot::create(
                            lock_val,
                            jhol_receipt::empty_ok_receipt(&root),
                        );
                        s.limitations
                            .push("no sync receipt recorded; run `jhol sync --apply` first".into());
                        s
                    }
                };
                snap = snap.with_project_files(
                    read_optional_text(&root.join("package.json")),
                    read_optional_text(&root.join("package-lock.json")),
                );
                let path = snap
                    .save(&root)
                    .map_err(|e| format!("save snapshot: {e}"))?;
                if as_json {
                    println!("{}", serde_json::json!({"id": snap.id, "path": path}));
                } else {
                    println!("created snapshot {} → {}", snap.id, path.display());
                }
            }
            _ => return Err(format!("unknown snapshot action {action}")),
        }
        return Ok(());
    }

    if let Some(("restore", sub_m)) = matches.subcommand() {
        let root = sub_m
            .get_one::<String>("path")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let id = sub_m.get_one::<String>("id").unwrap();
        let as_json = flag(&sub_m, "json");
        let do_plan = flag(&sub_m, "plan");
        let do_apply = flag(&sub_m, "apply");
        let want_db = flag(&sub_m, "databases");
        if !do_plan && !do_apply {
            return Err("restore needs --plan or --apply".into());
        }
        let snap = jhol_receipt::Snapshot::load(&root, id).map_err(|e| format!("restore: {e}"))?;
        let plan = if do_apply {
            snap.restore_apply(&root, want_db)
                .map_err(|e| format!("restore apply: {e}"))?
        } else {
            if want_db {
                return Err(
                    "restore --databases is refused (database restore is not supported)".into(),
                );
            }
            snap.restore_plan_in(&root)
        };
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&plan).unwrap_or_default()
            );
        } else {
            println!(
                "restore {} ({})",
                plan.snapshot_id,
                if do_apply { "applied" } else { "plan" }
            );
            for w in &plan.would_write {
                println!("  write: {w}");
            }
            for s in &plan.would_skip {
                println!("  skip: {s}");
            }
            for b in &plan.blocked {
                println!("  blocked: {b}");
            }
            for lim in &plan.limitations {
                println!("  limitation: {lim}");
            }
            if do_apply {
                println!(
                    "  next: jhol sync --apply   (installs node_modules from the restored lock)"
                );
            }
        }
        return Ok(());
    }

    if let Some(("db", sub_m)) = matches.subcommand() {
        if let Some(("plan", plan_m)) = sub_m.subcommand() {
            let root = plan_m
                .get_one::<String>("path")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
            let mig = plan_m
                .get_one::<String>("migrations")
                .map(|s| s.as_str())
                .unwrap_or("migrations");
            let as_json = flag(&plan_m, "json");
            let plan = jhol_db::plan_migrations(&root, mig).map_err(|e| format!("db plan: {e}"))?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).unwrap_or_default()
                );
            } else {
                println!(
                    "db plan — {} ({} migration file(s))",
                    plan.migrations_dir,
                    plan.migrations.len()
                );
                for m in &plan.migrations {
                    println!("  {}  {}", &m.digest[..12.min(m.digest.len())], m.path);
                }
                for n in &plan.drift_notes {
                    println!("  note: {n}");
                }
                for lim in &plan.limitations {
                    println!("  limitation: {lim}");
                }
            }
            return Ok(());
        }
        if let Some(("probe", probe_m)) = sub_m.subcommand() {
            let timeout_ms: u64 = probe_m
                .get_one::<String>("timeout-ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(500);
            let as_json = flag(&probe_m, "json");
            let result = jhol_db::probe_from_env(timeout_ms);
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
            } else {
                println!("db probe — {}", result.note);
                if let (Some(h), Some(p)) = (&result.host, result.port) {
                    println!("  target: {h}:{p}");
                }
                if let Some(ok) = result.reachable {
                    println!("  reachable: {ok}");
                }
            }
            return Ok(());
        }
        return Err("usage: jhol db plan|probe".into());
    }

    if let Some(("dev", sub_m)) = matches.subcommand() {
        let root = sub_m
            .get_one::<String>("path")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let as_json = flag(&sub_m, "json");
        let do_probe = flag(&sub_m, "probe");
        let do_spawn = flag(&sub_m, "spawn");
        let hold_secs: u64 = sub_m
            .get_one::<String>("hold-secs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let services = jhol_task::load_project_services(&root).map_err(|e| match e {
            jhol_task::ServiceConfigError::Io(ref io)
                if io.kind() == std::io::ErrorKind::NotFound =>
            {
                format!(
                    "dev: no jhol.toml in {}. Declare services in one, for example:\n\n{DEV_CONFIG_EXAMPLE}",
                    root.display()
                )
            }
            other => format!("dev config: {other}"),
        })?;
        let order =
            jhol_task::service_boot_order(&services).map_err(|e| format!("dev order: {e}"))?;
        let readiness: Vec<_> = services
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "ready_tcp": s.ready_tcp,
                    "tcp_ready_now": if do_probe || do_spawn {
                        s.ready_tcp.as_deref().map(jhol_task::tcp_ready)
                    } else {
                        None
                    },
                })
            })
            .collect();
        if do_spawn {
            let handles = jhol_task::spawn_services(&services, &order, 5_000)
                .map_err(|e| format!("dev spawn: {e}"))?;
            if as_json {
                println!(
                    "{}",
                    serde_json::json!({
                        "root": root,
                        "boot_order": order,
                        "spawned": handles.iter().map(|h| &h.name).collect::<Vec<_>>(),
                        "hold_secs": hold_secs,
                        "note": "processes held then killed on drop",
                    })
                );
            } else {
                println!(
                    "jhol dev (spawned {}) — holding {}s",
                    order.join(" → "),
                    hold_secs
                );
            }
            std::thread::sleep(std::time::Duration::from_secs(hold_secs));
            drop(handles);
            if !as_json {
                println!("cleaned up spawned services.");
            }
            return Ok(());
        }
        if as_json {
            println!(
                "{}",
                serde_json::json!({
                    "root": root,
                    "boot_order": order,
                    "services": readiness,
                    "probed": do_probe,
                    "note": "does not spawn processes; use --spawn to run declared commands",
                })
            );
        } else {
            println!(
                "jhol dev ({}) — {}",
                if do_probe {
                    "order + probe"
                } else {
                    "order only"
                },
                root.display()
            );
            println!("boot: {}", order.join(" → "));
            for s in &services {
                let ready = if do_probe {
                    s.ready_tcp
                        .as_deref()
                        .map(|a| {
                            if jhol_task::tcp_ready(a) {
                                "up"
                            } else {
                                "down"
                            }
                        })
                        .unwrap_or("-")
                } else {
                    s.ready_tcp.as_deref().unwrap_or("-")
                };
                println!("  • {} [{}]", s.name, ready);
            }
            println!("no processes spawned.");
        }
        return Ok(());
    }

    if let Some(("ai", sub_m)) = matches.subcommand() {
        let action = sub_m.subcommand_name().unwrap_or("");
        let act_m = sub_m.subcommand().map(|(_, m)| m);
        let Some(act_m) = act_m else {
            return Err(
                "usage: jhol ai pin|pin-remote|verify|prepare|infer --manifest PATH".into(),
            );
        };
        if action == "pull" {
            let out = pull_model(&act_m)?;
            println!("{out}");
            return Ok(());
        }
        if action == "inspect" {
            let report = inspect_model(&act_m)?;
            println!("{}", report);
            return Ok(());
        }
        if action != "pin"
            && action != "pin-remote"
            && action != "verify"
            && action != "prepare"
            && action != "infer"
        {
            return Err(format!("unknown ai action {action}"));
        }
        let root = act_m
            .get_one::<String>("path")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let man = act_m
            .get_one::<String>("manifest")
            .map(|s| s.as_str())
            .unwrap_or("models/demo.jhol-model.json");
        let man_path = root.join(man);
        let as_json = flag(&act_m, "json");
        if action == "pin" {
            let name = act_m.get_one::<String>("name").expect("required by clap");
            let source = act_m.get_one::<String>("source").expect("required by clap");
            let revision = act_m
                .get_one::<String>("revision")
                .expect("required by clap");
            let files: Vec<&str> = act_m
                .get_many::<String>("file")
                .expect("required by clap")
                .map(String::as_str)
                .collect();
            let artifact = jhol_ai::ModelArtifact::pin_directory_at_revision(
                name, source, revision, &root, &files,
            )
            .map_err(|e| format!("ai pin: {e}"))?;
            artifact
                .write_manifest(&man_path)
                .map_err(|e| format!("ai pin: {e}"))?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&artifact).unwrap_or_default()
                );
            } else {
                println!(
                    "ai pin ok — {} ({} file(s))",
                    artifact.name,
                    artifact.files.len()
                );
            }
            return Ok(());
        }
        if action == "pin-remote" {
            let name = act_m.get_one::<String>("name").expect("required by clap");
            let source = act_m.get_one::<String>("source").expect("required by clap");
            let revision = act_m
                .get_one::<String>("revision")
                .expect("required by clap");
            let files: Vec<String> = act_m
                .get_many::<String>("file")
                .expect("required by clap")
                .cloned()
                .collect();
            let digests: Vec<String> = act_m
                .get_many::<String>("digest")
                .expect("required by clap")
                .cloned()
                .collect();
            let sizes: Vec<String> = act_m
                .get_many::<String>("size-bytes")
                .expect("required by clap")
                .cloned()
                .collect();
            if files.len() != digests.len() || files.len() != sizes.len() {
                return Err(
                    "ai pin-remote: provide one --digest and --size-bytes per --file".into(),
                );
            }
            let pins = files
                .into_iter()
                .zip(digests)
                .zip(sizes)
                .map(|((path, digest), size)| {
                    let size_bytes = size.parse::<u64>().map_err(|_| {
                        format!("ai pin-remote: invalid byte size for {path}: {size}")
                    })?;
                    Ok(jhol_ai::FilePin {
                        path,
                        digest,
                        size_bytes: Some(size_bytes),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let artifact = jhol_ai::ModelArtifact::pin_remote_files(name, source, revision, pins)
                .map_err(|e| format!("ai pin-remote: {e}"))?;
            artifact
                .write_manifest(&man_path)
                .map_err(|e| format!("ai pin-remote: {e}"))?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&artifact).unwrap_or_default()
                );
            } else {
                println!(
                    "ai pin-remote ok — {} ({} file(s))",
                    artifact.name,
                    artifact.files.len()
                );
            }
            return Ok(());
        }
        if action == "infer" {
            let config = act_m.get_one::<String>("config").expect("required by clap");
            let raw_input = act_m.get_one::<String>("input").expect("required by clap");
            if raw_input.split(',').count() > 64 {
                return Err("ai infer: input exceeds the 64-feature runtime limit".into());
            }
            let inputs = raw_input
                .split(',')
                .map(|value| {
                    value
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| format!("ai infer: invalid numeric feature: {value}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let artifact = jhol_ai::ModelArtifact::read_manifest(&man_path)
                .map_err(|e| format!("ai infer: {e}"))?;
            let result = jhol_ai::infer_linear_cpu(&artifact, &root, config, &inputs)
                .map_err(|e| format!("ai infer: {e}"))?;
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
            } else {
                println!(
                    "{} via {}: {}",
                    result.model, result.runtime, result.predicted_label
                );
            }
            return Ok(());
        }
        let artifact = jhol_ai::ModelArtifact::read_manifest(&man_path)
            .map_err(|e| format!("ai {action}: {e}"))?;
        let receipt: Option<jhol_ai::DownloadReceipt> = if action == "verify"
            || !flag(&act_m, "allow-network")
        {
            jhol_ai::prepare_offline(&root, &man_path).map_err(|e| format!("ai {action}: {e}"))?;
            None
        } else {
            let max_bytes = act_m
                .get_one::<String>("max-bytes")
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| "ai prepare: --max-bytes must be a positive integer".to_string())?;
            if max_bytes == 0 {
                return Err("ai prepare: --max-bytes must be greater than zero".into());
            }
            #[cfg(feature = "http-reqwest")]
            {
                Some(
                    artifact
                        .prepare_with_download(
                            &root,
                            true,
                            max_bytes,
                            |url, path, offset, limit| {
                                jhol_core::http2_download::download_to_file_resume_bounded(
                                    url, path, offset, limit,
                                )
                            },
                        )
                        .map_err(|e| format!("ai prepare: {e}"))?,
                )
            }
            #[cfg(not(feature = "http-reqwest"))]
            {
                let _ = (artifact, max_bytes);
                return Err("ai prepare --allow-network requires the http-reqwest feature".into());
            }
        };
        if as_json {
            if let Some(receipt) = receipt {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "artifact": artifact,
                        "download": receipt
                    }))
                    .unwrap_or_default()
                );
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&artifact).unwrap_or_default()
                );
            }
        } else {
            println!(
                "ai {action} ok — {} ({} file(s), digest {})",
                artifact.name,
                artifact.files.len(),
                &artifact.digest[..12.min(artifact.digest.len())]
            );
            if let Some(receipt) = receipt {
                println!(
                    "downloaded {} byte(s) from {} at {}",
                    receipt.bytes_received, receipt.source, receipt.revision
                );
            }
        }
        return Ok(());
    }

    if let Some(("cdn", sub_m)) = matches.subcommand() {
        let spec = sub_m.get_one::<String>("package").unwrap().as_str();
        let (name, version) = if let Some(at) = spec.rfind('@') {
            if at > 0 {
                (&spec[..at], Some(&spec[at + 1..]))
            } else {
                (spec, None)
            }
        } else {
            (spec, None)
        };
        let url = jhol_core::esm_sh_url(name, version);
        println!("{}", url);
        if let Some(out_path) = sub_m.get_one::<String>("output") {
            if let Err(e) = jhol_core::fetch_esm_to_file(&url, std::path::Path::new(out_path)) {
                eprintln!("Fetch failed: {}", e);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    if let Some(("exec", sub_m)) = matches.subcommand() {
        let binary = sub_m.get_one::<String>("binary").unwrap().as_str();
        let args: Vec<String> = sub_m
            .get_many::<String>("args")
            .map(|it| it.map(String::clone).collect())
            .unwrap_or_default();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let bin_path = jhol_core::find_binary_in_node_modules(binary, &cwd).ok_or_else(|| {
            format!(
                "Binary \"{}\" not found in node_modules/.bin. Run jhol install first.",
                binary
            )
        })?;
        let status = jhol_core::exec_binary(&bin_path, &args, &cwd).map_err(|e| e.to_string())?;
        std::process::exit(status.code().unwrap_or(1));
    }

    if let Some(("uninstall", sub_m)) = matches.subcommand() {
        let package = sub_m.get_one::<String>("package").unwrap();
        let save = !flag(&sub_m, "no-save");
        jhol_core::uninstall(package, save).map_err(|e| e.to_string())?;
        success(&format!("Removed {}", package));
        return Ok(());
    }

    if let Some(("update", sub_m)) = matches.subcommand() {
        // Unlike `remove`, updating a *specific* package still requires --save: the
        // test `test_update_requires_save_for_specific_packages` pins that contract,
        // and rewriting package.json implicitly on an explicit package list would be
        // surprising.
        let save = flag(&sub_m, "save");
        let packages: Vec<String> = sub_m
            .get_many::<String>("package")
            .map(|it| it.map(|s| s.clone()).collect())
            .unwrap_or_default();
        if save {
            jhol_core::update_packages(&packages).map_err(|e| e.to_string())?;
        } else if packages.is_empty() {
            jhol_core::update_packages(&packages).map_err(|e| e.to_string())?;
        } else {
            return Err(
                "Use --save to update package.json when updating specific packages".to_string(),
            );
        }
        success("Update complete.");
        return Ok(());
    }

    if let Some(("why", sub_m)) = matches.subcommand() {
        let package = sub_m.get_one::<String>("package").unwrap();
        let use_jagr = flag(&sub_m, "jagr");
        if use_jagr {
            let manifest = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("manifest")
                    .cloned()
                    .unwrap_or_else(|| "package.json".to_string()),
            );
            let paths =
                jhol_core::jagr_why_package_deep(&manifest, package).map_err(|e| e.to_string())?;
            println!("{}", paths);
        } else {
            let paths = jhol_core::why_package(package).map_err(|e| e.to_string())?;
            for path in paths {
                println!("{}", path);
            }
            // Then the derivation the lockfile already records: which copies are
            // installed, at what versions, and which range each dependent asked
            // for. Offline, no re-resolution — the answer was written down when
            // the tree was installed.
            if let Some(report) =
                jhol_core::why::explain(std::path::Path::new("."), package)
            {
                if !report.is_empty() {
                    println!();
                    print!("{}", report.render());
                }
            }
        }
        return Ok(());
    }

    if let Some(("replay", sub_m)) = matches.subcommand() {
        let path = sub_m
            .get_one::<String>("spec")
            .map(std::path::PathBuf::from)
            .ok_or("replay needs a spec file")?;
        let paper = jhol_core::paper::Paper::load(&path).map_err(|e| format!("replay: {e}"))?;
        let current: std::collections::BTreeMap<String, String> =
            jhol_core::lockfile::read_package_json_deps(std::path::Path::new("package.json"))
                .map(|m| m.into_iter().collect())
                .unwrap_or_default();
        let drift = paper.diff_manifest(&current);
        if !drift.is_empty() {
            eprintln!("replay: the project does not match the spec:");
            for (name, have, want) in &drift {
                eprintln!("  {name}: have {} want {want}", have.clone().unwrap_or_else(|| "<none>".into()));
            }
            return Err(
                "refusing to edit package.json silently; align the manifest with the spec first"
                    .to_string(),
            );
        }
        let plan = paper.plan();
        if !flag(&sub_m, "execute") {
            println!("replay plan for {}:", paper.name.clone().unwrap_or_else(|| path.display().to_string()));
            for cmd in plan {
                println!("  {cmd}");
            }
            return Ok(());
        }
        // Execute the same commands a user would type, so semantics cannot drift.
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        for cmd in &plan {
            let mut parts = cmd.split_whitespace();
            let _binary = parts.next();
            let args: Vec<&str> = parts.collect();
            let mut c = std::process::Command::new(&exe);
            c.args(&args);
            if let Some(as_of) = &paper.as_of {
                c.env("JHOL_AS_OF", as_of);
            }
            let status = c.status().map_err(|e| format!("{cmd}: {e}"))?;
            if !status.success() {
                return Err(format!("replay step failed: {cmd}"));
            }
        }
        println!("replay complete: {}", paper.name.clone().unwrap_or_default());
        return Ok(());
    }

    if let Some(("env", sub_m)) = matches.subcommand() {
        let envs = std::path::Path::new(".jhol/envs");
        match sub_m.subcommand() {
            Some(("fork", m)) => {
                let name = m.get_one::<String>("name").cloned().unwrap_or_default();
                if name.is_empty() || name.contains('/') || name.contains("..") {
                    return Err("env fork: invalid name".into());
                }
                let from = m
                    .get_one::<String>("from")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let dest = envs.join(&name);
                std::fs::create_dir_all(envs).map_err(|e| e.to_string())?;
                let started = std::time::Instant::now();
                // Only node_modules is forked: the fork shares the identity files by
                // copying them, so both environments stay independently lockable.
                let stats = jhol_core::env_fork::fork_tree(
                    &from.join("node_modules"),
                    &dest.join("node_modules"),
                )
                .map_err(|e| format!("env fork: {e}"))?;
                for file in jhol_core::env_fork::identity_files(&from) {
                    if let Some(name) = file.file_name() {
                        let _ = std::fs::copy(&file, dest.join(name));
                    }
                }
                if flag(&m, "json") {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "name": name,
                            "dir": dest.display().to_string(),
                            "dirs": stats.dirs, "files": stats.files,
                            "cloned": stats.cloned, "copied": stats.copied,
                            "referenced_bytes": stats.bytes,
                            "added_bytes": stats.extra_bytes(),
                            "ms": started.elapsed().as_millis() as u64,
                        }))
                        .unwrap_or_default()
                    );
                } else {
                    println!(
                        "forked {name} in {} ms — {}",
                        started.elapsed().as_millis(),
                        stats.describe()
                    );
                    println!("  run it with: cd {} · or copy it back when it works", dest.display());
                }
                return Ok(());
            }
            Some(("list", m)) => {
                let mut names: Vec<String> = std::fs::read_dir(envs)
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| e.path().is_dir())
                            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                names.sort();
                if flag(&m, "json") {
                    println!("{}", serde_json::to_string_pretty(&names).unwrap_or_default());
                } else if names.is_empty() {
                    println!("no forks (jhol env fork <name>)");
                } else {
                    for n in names {
                        println!("{n}");
                    }
                }
                return Ok(());
            }
            Some(("diff", m)) => {
                let a = m.get_one::<String>("a").cloned().unwrap_or_default();
                let b = m.get_one::<String>("b").cloned().unwrap_or_default();
                let read = |name: &str| {
                    jhol_core::lockfile::read_resolved_from_dir(&envs.join(name)).unwrap_or_default()
                };
                let (ma, mb) = (read(&a), read(&b));
                let mut keys: Vec<&String> = ma.keys().chain(mb.keys()).collect();
                keys.sort();
                keys.dedup();
                let mut n = 0;
                for k in keys {
                    let va = ma.get(k);
                    let vb = mb.get(k);
                    if va != vb {
                        n += 1;
                        println!(
                            "{k}: {} -> {}",
                            va.map(String::as_str).unwrap_or("-"),
                            vb.map(String::as_str).unwrap_or("-")
                        );
                    }
                }
                if n == 0 {
                    println!("{a} and {b} resolve identically");
                }
                return Ok(());
            }
            Some(("rm", m)) => {
                let name = m.get_one::<String>("name").cloned().unwrap_or_default();
                std::fs::remove_dir_all(envs.join(&name)).map_err(|e| e.to_string())?;
                println!("removed fork {name}");
                return Ok(());
            }
            _ => return Err("usage: jhol env fork|list|diff|rm".into()),
        }
    }

    if let Some(("attest", sub_m)) = matches.subcommand() {
        if flag(&sub_m, "verify") {
            let Some(att) = jhol_core::attest::Attestation::load(std::path::Path::new(".")) else {
                return Err("no attestation found; run `jhol attest` first".into());
            };
            match att.verify(std::path::Path::new(".")) {
                Ok(()) => {
                    if !flag(&sub_m, "json") {
                        println!("attestation verified ({} pinned packages)", att.packages.len());
                    }
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        let att = jhol_core::attest::Attestation::build(std::path::Path::new("."))
            .map_err(|e| format!("attest: {e}"))?;
        att.save(std::path::Path::new(".")).map_err(|e| e.to_string())?;
        if flag(&sub_m, "json") {
            println!("{}", serde_json::to_string_pretty(&att).unwrap_or_default());
        } else {
            println!(
                "attested {} packages and {} models at {}",
                att.packages.len(),
                att.models.len(),
                jhol_core::attest::Attestation::path(std::path::Path::new(".")).display()
            );
        }
        return Ok(());
    }

    if let Some(("axes", sub_m)) = matches.subcommand() {
        let mut host = jhol_core::axes::HostAxes::detect();
        if let Some(py) = sub_m.get_one::<String>("python") {
            host.python = Some(py.clone());
        }
        if flag(&sub_m, "json") {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "os": host.os,
                    "arch": host.arch,
                    "libc": host.libc.to_string(),
                    "python": host.python,
                    "cuda_driver": host.cuda_driver,
                    "sm": host.sm,
                    "gpu_mem_mb": host.gpu_mem_mb,
                }))
                .unwrap_or_default()
            );
        } else {
            println!("{}", host.summary());
        }
        return Ok(());
    }

    if let Some(("pysolve", sub_m)) = matches.subcommand() {
        let raw = sub_m
            .get_one::<String>("spec")
            .map(|s| s.as_str())
            .unwrap_or_default();
        let (pkg, ver) = match raw.rsplit_once('@') {
            Some((p, v)) if !p.is_empty() && !v.is_empty() => (p.to_string(), Some(v.to_string())),
            _ => (raw.to_string(), None),
        };
        let mut host = jhol_core::axes::HostAxes::detect();
        if let Some(py) = sub_m.get_one::<String>("python") {
            host.python = Some(py.clone());
        }
        let options = jhol_core::pyresolve::Options {
            require_wheel: !flag(&sub_m, "allow-sdist"),
            allow_prerelease: flag(&sub_m, "pre"),
            ..Default::default()
        };
        let roots = vec![(pkg.clone(), ver.map(|v| format!("=={}", v)).unwrap_or_default())];
        let mut provider = jhol_core::pyresolve::PyPIProvider::new();
        let started = std::time::Instant::now();
        let resolved = jhol_core::pyresolve::resolve(&mut provider, &roots, &host, &options)
            .map_err(|e| format!("pysolve: {e}"))?;
        if flag(&sub_m, "json") {
            let packages: Vec<serde_json::Value> = resolved
                .packages
                .iter()
                .map(|(name, (version, artifact))| {
                    serde_json::json!({"name": name, "version": version, "artifact": artifact})
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "host": host.summary(),
                    "packages": packages,
                    "skipped": resolved.skipped,
                    "ms": started.elapsed().as_millis() as u64,
                }))
                .unwrap_or_default()
            );
        } else {
            println!(
                "{} package(s) resolved in {} ms for {}",
                resolved.packages.len(),
                started.elapsed().as_millis(),
                host.summary()
            );
            for (name, (version, artifact)) in &resolved.packages {
                println!(
                    "  {name} {version}{}",
                    artifact
                        .as_ref()
                        .map(|a| format!("  ({a})"))
                        .unwrap_or_default()
                );
            }
            if !resolved.skipped.is_empty() {
                println!("  skipped {} requirement(s) by environment marker", resolved.skipped.len());
            }
        }
        return Ok(());
    }

    if let Some(("wheels", sub_m)) = matches.subcommand() {
        let raw = sub_m
            .get_one::<String>("spec")
            .map(|s| s.as_str())
            .unwrap_or_default();
        let (pkg, version) = match raw.rsplit_once('@') {
            Some((p, v)) if !p.is_empty() && !v.is_empty() => (p.to_string(), Some(v.to_string())),
            _ => (raw.to_string(), None),
        };
        let mut host = jhol_core::axes::HostAxes::detect();
        if let Some(py) = sub_m.get_one::<String>("python") {
            host.python = Some(py.clone());
        }
        let release = jhol_core::pypi::fetch_release(&pkg, version.as_deref())
            .map_err(|e| format!("pypi: {e}"))?;
        let verdicts = jhol_core::pypi::classify(&release, &host);
        let compatible: Vec<_> = verdicts.iter().filter(|v| v.reason.is_none()).collect();
        if flag(&sub_m, "json") {
            let body: Vec<serde_json::Value> = verdicts
                .iter()
                .map(|v| serde_json::json!({
                    "file": v.filename, "size": v.size, "reason": v.reason,
                }))
                .collect();
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
        } else {
            println!(
                "{}@{} — {} file(s), {} compatible with {}",
                release.package,
                release.version,
                verdicts.len(),
                compatible.len(),
                host.summary()
            );
            for v in verdicts.iter().filter(|v| v.reason.is_none()).take(5) {
                println!("  ok        {}", v.filename);
            }
            if compatible.is_empty() {
                for v in verdicts.iter().take(6) {
                    println!(
                        "  no        {}  ({})",
                        v.filename,
                        v.reason.as_deref().unwrap_or("")
                    );
                }
            }
        }
        if compatible.is_empty() {
            return Err(format!(
                "no published artifact of {}@{} runs on this machine",
                release.package, release.version
            ));
        }
        return Ok(());
    }

    if let Some(("verify", sub_m)) = matches.subcommand() {
        // A gate CI can trust: exit 0 only when the tree is exactly what the
        // lockfile authorises. `Unverified` (no lockfile) is not a failure — there
        // is nothing to disagree with — but it is reported, because silence there
        // would hide a missing lockfile.
        let state = if flag(&sub_m, "prune") {
            // `--prune` means prune: remove every unauthorised directory even when
            // another discrepancy (a wrong version) is reported first, then re-read
            // the tree. The default `reconcile_tree` only prunes an extraneous-only
            // tree, which is the right caution for an install but not for an
            // explicit request.
            let _ = jhol_core::tree_state::prune_extraneous(std::path::Path::new("."));
            jhol_core::tree_state::verify_tree(std::path::Path::new("."))
        } else {
            jhol_core::tree_state::verify_tree(std::path::Path::new("."))
        };
        // Models are part of the environment proof. When a model lock exists, one
        // command answers for packages *and* weights — that is the whole point of
        // treating models as artifacts.
        let model_state = jhol_core::model_lock::ModelLock::load(std::path::Path::new("."))
            .map(|lock| lock.verify(std::path::Path::new(".")))
            .unwrap_or(jhol_core::model_lock::ModelState::NoLock);
        let json = flag(&sub_m, "json");
        let quiet = flag(&sub_m, "quiet");
        if !model_state.is_match()
            && !matches!(model_state, jhol_core::model_lock::ModelState::NoLock)
        {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "tree": format!("{:?}", state),
                        "models": model_state.describe(),
                    }))
                    .unwrap_or_default()
                );
            } else {
                eprintln!("NOT OK: {}", model_state.describe());
                eprintln!("  run `jhol ai pull ...` to restore the pinned model files");
            }
            return Err("a pinned model file does not match the lock".to_string());
        }
        match &state {
            jhol_core::tree_state::TreeState::Match => {
                if json {
                    println!("{{\"state\":\"match\"}}");
                } else if !quiet {
                    let total = jhol_core::count_installed_packages(std::path::Path::new("."));
                    let models = match &model_state {
                        jhol_core::model_lock::ModelState::Match => " + models",
                        _ => "",
                    };
                    println!("OK: {} packages{} match the lockfile", total, models);
                }
                return Ok(());
            }
            jhol_core::tree_state::TreeState::Unverified { reason } => {
                // Models can be the only pinned artifacts in a project (a manifest
                // with no npm dependencies). Saying "not verified" while the model
                // lock was checked and matched would be wrong.
                if model_state.is_match() {
                    if json {
                        println!("{{\"state\":\"models_only\"}}");
                    } else if !quiet {
                        println!("OK: models match the lock (no package lockfile)");
                    }
                    return Ok(());
                }
                if json {
                    println!("{{\"state\":\"unverified\",\"reason\":\"{}\"}}", reason);
                } else if !quiet {
                    println!("not verified: {}", reason);
                }
                return Ok(());
            }
            other => {
                if json {
                    let v = serde_json::json!({
                        "state": format!("{:?}", other),
                        "detail": other.describe(),
                    });
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                } else {
                    eprintln!("NOT OK: {}", other.describe());
                    eprintln!("  run `jhol install` to repair, or `jhol verify --prune` to remove unauthorised packages");
                }
                return Err("tree does not match the lockfile".to_string());
            }
        }
    }

    if let Some(("why-not", sub_m)) = matches.subcommand() {
        let raw = sub_m
            .get_one::<String>("spec")
            .map(|s| s.as_str())
            .unwrap_or_default();
        let (package, range) = match raw.rsplit_once('@') {
            Some((name, r)) if !name.is_empty() => (name.to_string(), r.to_string()),
            _ => (raw.to_string(), "*".to_string()),
        };
        let range = if range.trim().is_empty() {
            "*".to_string()
        } else {
            range
        };
        let cache = std::path::PathBuf::from(jhol_core::utils::get_cache_dir()).join("packuments");
        match jhol_core::why_not::explain(
            &cache,
            std::path::Path::new("."),
            &package,
            &range,
        ) {
            Some(report) => print!("{}", report.render()),
            None => {
                return Err(format!(
                    "no cached metadata for {}; run `jhol install {}` once so its versions are known",
                    package, package
                ))
            }
        }
        return Ok(());
    }

    if let Some(("outdated", sub_m)) = matches.subcommand() {
        let quiet = flag(&sub_m, "quiet");
        let all_workspaces = flag(&sub_m, "all-workspaces");
        let with_jagr = flag(&sub_m, "jagr");
        if quiet {
            env::set_var("JHOL_QUIET", "1");
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let roots: Vec<std::path::PathBuf> = if all_workspaces {
            let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
            if r.is_empty() {
                vec![cwd]
            } else {
                r.into_iter().map(|p| cwd.join(p)).collect()
            }
        } else {
            vec![cwd]
        };
        for root in &roots {
            if roots.len() > 1 {
                info(&format!("Workspace: {}", root.display()));
            }
            jhol_core::check_dependencies(quiet, jhol_core::Backend::Npm)
                .map_err(|e| e.to_string())?;
        }

        // --jagr: After listing outdated packages, also report JAGR
        // topological impact (β₁/β₂/SNF status from JAGR's three-layer pipeline).
        if with_jagr {
            if use_color() {
                println!(
                    "\n{}",
                    "JAGR Topological Impact on Upgrades".bright_cyan().bold()
                );
            } else {
                println!("\nJAGR Topological Impact on Upgrades");
            }
            for root in &roots {
                let manifest = root.join("package.json");
                if !manifest.exists() {
                    info(&format!("No package.json in {}", root.display()));
                    continue;
                }
                match jhol_core::jagr_analyze_project_manifest(&manifest) {
                    Ok(report) => {
                        println!(
                            "  {:<14} β₀={}, β₁={}, β₂={}",
                            root.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                            report.betti_0,
                            report.betti_1,
                            report.betti_2
                        );
                        println!(
                            "  {:<14} SNF satisfied = {}, trivial = {}",
                            "", report.snf_satisfied, report.snf_trivial
                        );
                        println!(
                            "  {:<14} Layer 3: {}  Layer 1: {}",
                            "",
                            report.layer3_kind,
                            if report.layer1_satisfiable {
                                "satisfiable"
                            } else {
                                "not invoked"
                            }
                        );
                        println!(
                            "  {:<14} Strategy: {}  Selection: {} packages",
                            "",
                            report.strategy_used,
                            report.selection.len()
                        );
                    }
                    Err(e) => {
                        info(&format!(
                            "  JAGR analysis failed for {}: {}",
                            root.display(),
                            e
                        ));
                    }
                }
            }
        }
        return Ok(());
    }

    if let Some(("init", sub_m)) = matches.subcommand() {
        let _accept_defaults = flag(&sub_m, "yes");
        let private = flag(&sub_m, "private");
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let package_json_path = cwd.join("package.json");
        if package_json_path.exists() {
            return Err("package.json already exists".to_string());
        }
        let name = cwd
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-package")
            .to_lowercase()
            .replace(" ", "-");
        let json_value = serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "description": "",
            "main": "index.js",
            "scripts": {
                "test": "echo \"Error: no test specified\" && exit 1"
            },
            "keywords": [],
            "author": "",
            "license": "MIT",
            "private": private
        });
        let json_str = serde_json::to_string_pretty(&json_value).map_err(|e| e.to_string())?;
        fs::write(&package_json_path, &json_str).map_err(|e| e.to_string())?;
        success(&format!(
            "Created {} in {}",
            package_json_path.display(),
            cwd.display()
        ));
        info("Run `jhol install` to get started.");
        return Ok(());
    }

    if let Some(("add", sub_m)) = matches.subcommand() {
        let packages: Vec<&str> = sub_m
            .get_many::<String>("package")
            .map(|v| v.map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let quiet = flag(&sub_m, "quiet");
        if packages.is_empty() {
            return Err("At least one package must be specified".to_string());
        }
        // Manifest first: this is both the documented `add` behaviour and the signal
        // that makes the install run instead of taking a "node_modules is fresh" path.
        persist_added_deps(&packages)?;

        // Soul Path: Jacobian conflict risk prediction before fetch.
        // Compute ∂R/∂X against existing deps to warn if adding is high-risk.
        let mut frontier_risk = -1.0f64;
        let mut has_deps = false;
        if let Ok(pj) = std::fs::read_to_string("package.json") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&pj) {
                let mut deps_map = std::collections::HashMap::new();
                if let Some(deps) = json.get("dependencies").and_then(|v| v.as_object()) {
                    for (k, v) in deps {
                        deps_map.insert(k.clone(), v.as_str().unwrap_or("*").to_string());
                    }
                }
                if let Some(deps) = json.get("devDependencies").and_then(|v| v.as_object()) {
                    for (k, v) in deps {
                        deps_map
                            .entry(k.clone())
                            .or_insert_with(|| v.as_str().unwrap_or("*").to_string());
                    }
                }
                if !deps_map.is_empty() {
                    has_deps = true;
                    let add_strs: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
                    let risk = jhol_core::soul_path::add_conflict_risk(&deps_map, &add_strs);
                    frontier_risk = risk;
                    if risk > 0.5 && !quiet {
                        println!(
                            "{} Jacobian risk: {:.0}% — adding {} may cause conflicts",
                            "!".yellow(),
                            risk * 100.0,
                            packages.join(", ")
                        );
                    } else if !quiet {
                        println!(
                            "{} Jacobian risk: {:.0}% — adding {} is low-risk",
                            "✓".green(),
                            risk * 100.0,
                            packages.join(", ")
                        );
                    }
                }
            }
        }

        let config = jhol_core::load_config(
            &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        );

        // Soul Path: low-risk add uses frontier-only append (homological delta).
        // Skips full re-resolve; downloads frontier + projects into node_modules.
        let use_frontier = has_deps
            && frontier_risk >= 0.0
            && frontier_risk < 0.4
            && jhol_core::soul_path::auto_daemon_enabled(); // gate: daemon keeps zero-copy hot
        if use_frontier {
            let add_strs: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
            match jhol_core::soul_path::frontier_add(&add_strs, quiet) {
                Ok(()) => {
                    refine_star_specs(&packages);
                    success("Package(s) added (frontier delta).");
                    return Ok(());
                }
                Err(e) => {
                    if !quiet {
                        println!("Frontier add failed ({}), falling back to full install", e);
                    }
                }
            }
        }

        // Soul Path: high-risk add prefetches predicted conflict cluster first.
        // Uses spectral conflict_risk to warm the cache for likely problem packages.
        if has_deps && frontier_risk >= 0.5 && !quiet {
            let add_strs: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
            // Prefetch using context (existing deps + adding packages)
            jhol_core::soul_path::predictive_prefetch(&add_strs, 5);
        }

        let opts = jhol_core::InstallOptions {
            no_cache: false,
            quiet,
            backend: config.backend.unwrap_or(jhol_core::Backend::Bun),
            lockfile_only: false,
            offline: false,
            strict_lockfile: false,
            strict_peer_deps: false,
            from_lockfile: false,
            native_only: false,
            no_scripts: true,
            script_allowlist: None,
            resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
            overrides: None,
            telemetry: jhol_core::telemetry_enabled(&config),
            omit: jhol_core::omit::Omit::NONE,
            engine_strict: false,
        };
        let refs: Vec<&str> = packages;
        jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string())?;
        // Markers (write_status, store_last_hash, fingerprint) are written by
        // install_package via write_install_markers() — single write.
        refine_star_specs(&refs);
        success("Package(s) added.");
        return Ok(());
    }

    if let Some(("remove", sub_m)) = matches.subcommand() {
        let packages: Vec<&str> = sub_m
            .get_many::<String>("package")
            .map(|v| v.map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let save = !flag(&sub_m, "no-save");
        if packages.is_empty() {
            return Err("At least one package must be specified".to_string());
        }
        for pkg in packages {
            jhol_core::uninstall(pkg, save).map_err(|e| e.to_string())?;
        }
        success("Package(s) removed.");
        return Ok(());
    }

    if let Some(("global", sub_m)) = matches.subcommand() {
        match sub_m.subcommand() {
            Some(("list", _)) | None => {
                let installed = jhol_core::global_install::installed();
                if installed.is_empty() {
                    println!("No globally installed packages.");
                } else {
                    println!("{} global package(s) in {}:", installed.len(),
                        jhol_core::global_install::prefix().display());
                    for (name, range) in installed {
                        println!("  {}@{}", name, range);
                    }
                }
            }
            Some(("path", _)) => println!("{}", jhol_core::global_install::prefix().display()),
            Some(("remove", m)) => {
                let name = m.get_one::<String>("package").cloned().unwrap_or_default();
                match jhol_core::global_install::uninstall_global(&name) {
                    Ok(true) => println!("Removed {} from the global prefix.", name),
                    Ok(false) => println!("{} is not globally installed.", name),
                    Err(e) => {
                        eprintln!("global remove: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Some((other, _)) => {
                eprintln!("global: unknown action '{}' (use list, path, or remove)", other);
                std::process::exit(1);
            }
        }
    }

    if let Some(("dedupe", sub_m)) = matches.subcommand() {
        let dups = jhol_core::pm_field::find_duplicates(std::path::Path::new("."));
        if flag(&sub_m, "json") {
            let body: Vec<serde_json::Value> = dups
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "name": d.name,
                        "versions": d.versions.iter().map(|(v, n)| serde_json::json!({"version": v, "copies": n})).collect::<Vec<_>>()
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
        } else if dups.is_empty() {
            println!("No duplicate versions in node_modules.");
        } else {
            println!("{} package(s) installed at more than one version:", dups.len());
            for d in &dups {
                let vs: Vec<String> = d
                    .versions
                    .iter()
                    .map(|(v, n)| if *n > 1 { format!("{} ({}x)", v, n) } else { v.clone() })
                    .collect();
                println!("  {} — {}", d.name, vs.join(", "));
            }
            println!(
                "\nDuplicates are structural: a range conflict between dependents needs \
                 an override (`overrides` in package.json) or a dependency update."
            );
        }
    }

    if let Some(("patch", sub_m)) = matches.subcommand() {
        let project = std::path::Path::new(".");
        let pkg = sub_m.get_one::<String>("package").cloned();
        if flag(&sub_m, "list") || (pkg.is_none() && !flag(&sub_m, "commit")) {
            let patches = jhol_core::patch::list(project);
            if patches.is_empty() {
                println!("No patches recorded.");
            } else {
                println!("{} patch(es):", patches.len());
                for p in patches {
                    println!(
                        "  {}@{} — {} file(s), {} bytes  ({})",
                        p.package,
                        p.version,
                        p.files,
                        p.bytes,
                        p.path.display()
                    );
                }
            }
        } else if let Some(pkg) = pkg {
            let result = if flag(&sub_m, "rollback") {
                jhol_core::patch::rollback(project, &pkg)
                    .map(|removed| {
                        if removed {
                            println!("Removed the patch for {}.", pkg);
                        } else {
                            println!("No patch recorded for {}.", pkg);
                        }
                    })
                    .map_err(|e| e)
            } else if flag(&sub_m, "commit") {
                jhol_core::patch::commit(project, &pkg)
                    .map(|rec| {
                        println!(
                            "Patched {}@{} — {} file(s) stored in {}",
                            rec.package,
                            rec.version,
                            rec.files,
                            rec.path.display()
                        );
                        println!("Commit the patches/ directory so the fix ships with the repo.");
                    })
                    .map_err(|e| e)
            } else {
                jhol_core::patch::extract_for_edit(project, &pkg)
                    .map(|dir| {
                        println!("Edit the files in:\n  {}", dir.display());
                        println!("Then run:  jhol patch {} --commit", pkg);
                    })
                    .map_err(|e| e)
            };
            if let Err(e) = result {
                eprintln!("patch: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("login", sub_m)) = matches.subcommand() {
        let registry = sub_m
            .get_one::<String>("registry")
            .cloned()
            .unwrap_or_else(|| {
                jhol_core::config::effective_registry_url(std::path::Path::new("."))
            });
        let token = sub_m
            .get_one::<String>("token")
            .cloned()
            .unwrap_or_default();
        let global = flag(&sub_m, "global");
        match jhol_core::publish::login(std::path::Path::new("."), &registry, &token, global) {
            Ok(path) => {
                println!("Stored token for {} in {}", registry, path.display());
            }
            Err(e) => {
                eprintln!("login: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("publish", sub_m)) = matches.subcommand() {
        let dir = PathBuf::from(
            sub_m
                .get_one::<String>("dir")
                .cloned()
                .unwrap_or(".".into()),
        );
        let dry = flag(&sub_m, "dry-run");
        let registry = sub_m
            .get_one::<String>("registry")
            .cloned()
            .unwrap_or_else(|| jhol_core::config::effective_registry_url(&dir));
        match jhol_core::publish::publish(&dir, &registry, None, dry) {
            Ok(o) => {
                if o.dry_run {
                    println!(
                        "dry run: would publish {}@{} to {} ({} bytes, sha256 {})",
                        o.name, o.version, o.registry, o.tarball_bytes, o.sha256
                    );
                } else {
                    println!(
                        "published {}@{} -> {} ({} bytes, sha256 {})",
                        o.name, o.version, o.registry, o.tarball_bytes, o.sha256
                    );
                }
            }
            Err(e) => {
                eprintln!("publish: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("view", sub_m)) = matches.subcommand() {
        let pkg = sub_m
            .get_one::<String>("package")
            .cloned()
            .unwrap_or_default();
        match jhol_core::registry_query::view(&pkg) {
            Ok(text) => print!("{}", text),
            Err(e) => {
                eprintln!("view: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("search", sub_m)) = matches.subcommand() {
        let term = sub_m.get_one::<String>("term").cloned().unwrap_or_default();
        let limit = sub_m.get_one::<usize>("limit").copied().unwrap_or(20);
        match jhol_core::registry_query::search(&term, limit) {
            Ok(text) => print!("{}", text),
            Err(e) => {
                eprintln!("search: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("pack", sub_m)) = matches.subcommand() {
        let dir = PathBuf::from(
            sub_m
                .get_one::<String>("dir")
                .cloned()
                .unwrap_or(".".into()),
        );
        match jhol_core::pack::pack_bytes(&dir) {
            Ok((bytes, label, files)) => {
                let filename = label.split(':').next().unwrap_or("package.tgz").to_string();
                std::fs::write(&filename, &bytes).unwrap_or_else(|e| {
                    eprintln!("pack: write {}: {}", filename, e);
                    std::process::exit(1);
                });
                println!(
                    "packed {} files -> {} ({} bytes)\nsha256: {}\nre-pack to verify: identical bytes, every time",
                    files,
                    filename,
                    bytes.len(),
                    label.split(':').nth(1).unwrap_or("?")
                );
            }
            Err(e) => {
                eprintln!("pack: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("cache", sub_m)) = matches.subcommand() {
        if let Some(("gc", gc_m)) = sub_m.subcommand() {
            let dry = flag(&gc_m, "dry-run");
            let mut lockfiles: Vec<PathBuf> = gc_m
                .get_many::<String>("from")
                .map(|v| v.map(PathBuf::from).collect())
                .unwrap_or_default();
            if lockfiles.is_empty() {
                lockfiles.push(PathBuf::from("package-lock.json"));
            }
            let store_root = jhol_core::global_cache::global_cache_dir();
            match jhol_core::registry_query::store_gc(&lockfiles, dry) {
                Ok((kept, freed, files)) => {
                    if dry {
                        println!(
                            "gc (dry run): {} tarball(s) reachable, {} file(s) / {} bytes freeable",
                            kept,
                            files,
                            human_bytes(freed)
                        );
                    } else {
                        println!(
                            "gc: {} tarball(s) kept, freed {} file(s) / {} bytes",
                            kept,
                            files,
                            human_bytes(freed)
                        );
                    }
                }
                Err(e) => {
                    eprintln!("cache gc: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }

    if let Some(("approve-scripts", sub_m)) = matches.subcommand() {
        let want_all = flag(&sub_m, "all");
        let asked: Vec<String> = sub_m
            .get_many::<String>("packages")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        match jhol_core::lifecycle::approve_scripts(Path::new("."), &asked, want_all) {
            Ok(granted) => {
                if granted.is_empty() {
                    println!("No packages with install scripts found to approve.");
                } else {
                    println!("Approved install scripts for {} package(s):", granted.len());
                    for g in &granted {
                        println!("  + {}", g);
                    }
                    println!(
                        "Receipts recorded in {}. Commit this file: an approval is bound to the\npackage version, the exact script text, and the tarball integrity - any change\nrequires re-approval.",
                        jhol_core::lifecycle::RECEIPTS_REL_PATH
                    );
                }
            }
            Err(e) => {
                eprintln!("approve-scripts: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("revoke-scripts", sub_m)) = matches.subcommand() {
        let asked: Vec<String> = sub_m
            .get_many::<String>("packages")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        match jhol_core::lifecycle::revoke_scripts(Path::new("."), &asked) {
            Ok(revoked) => {
                if revoked.is_empty() {
                    println!("No matching receipts to revoke.");
                } else {
                    println!("Revoked script consent for {} package(s):", revoked.len());
                    for r in &revoked {
                        println!("  - {}", r);
                    }
                }
            }
            Err(e) => {
                eprintln!("revoke-scripts: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(("link", sub_m)) = matches.subcommand() {
        let package_dir = sub_m
            .get_one::<String>("package")
            .map(|s| std::path::PathBuf::from(s))
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        if !package_dir.exists() {
            return Err(format!(
                "Package directory does not exist: {}",
                package_dir.display()
            ));
        }
        let package_json = package_dir.join("package.json");
        if !package_json.exists() {
            return Err(format!(
                "No package.json found in {}",
                package_dir.display()
            ));
        }
        let pkg_data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&package_json).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let name = pkg_data
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or("Package name not found in package.json")?;
        // Validate package name to prevent path traversal
        jhol_core::validate_package_name(name)
            .ok_or_else(|| format!("invalid package name: {}", name))?;
        let node_modules = cwd.join("node_modules");
        fs::create_dir_all(&node_modules).map_err(|e| e.to_string())?;
        let link_path = if name.starts_with('@') {
            let parts: Vec<&str> = name.split('/').collect();
            let scope_dir = node_modules.join(parts[0]);
            fs::create_dir_all(&scope_dir).map_err(|e| e.to_string())?;
            scope_dir.join(parts[1])
        } else {
            node_modules.join(name)
        };
        if link_path.exists() {
            if link_path.is_symlink() {
                fs::remove_file(&link_path).map_err(|e| e.to_string())?;
            } else {
                fs::remove_dir_all(&link_path).map_err(|e| e.to_string())?;
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&package_dir, &link_path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&package_dir, &link_path).map_err(|e| e.to_string())?;
        success(&format!("Linked {} -> {}", name, link_path.display()));
        return Ok(());
    }

    if let Some(("unlink", sub_m)) = matches.subcommand() {
        let package = sub_m
            .get_one::<String>("package")
            .map(|s| s.as_str())
            .unwrap();
        // Validate package name to prevent path traversal
        jhol_core::validate_package_name(package)
            .ok_or_else(|| format!("invalid package name: {}", package))?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let node_modules = cwd.join("node_modules");
        let link_path = if package.starts_with('@') {
            let parts: Vec<&str> = package.split('/').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid scoped package name: {}", package));
            }
            node_modules.join(parts[0]).join(parts[1])
        } else {
            node_modules.join(package)
        };
        if !link_path.exists() {
            return Err(format!("Package {} is not linked", package));
        }
        if !link_path.is_symlink() {
            return Err(format!("Package {} is not a symlink", package));
        }
        fs::remove_file(&link_path).map_err(|e| e.to_string())?;
        success(&format!("Unlinked {}", package));
        return Ok(());
    }

    if let Some(("ci", sub_m)) = matches.subcommand() {
        jhol_core::init_cache().map_err(|e| format!("Failed to initialize cache: {}", e))?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let config = jhol_core::load_config(&cwd);
        if let Some(ref d) = config.cache_dir {
            env::set_var("JHOL_CACHE_DIR", d);
        }
        let quiet = false;
        let offline = flag(&sub_m, "offline")
            || env::var("JHOL_OFFLINE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let all_workspaces = flag(&sub_m, "all-workspaces");
        let jagr_check = flag(&sub_m, "jagr-check");
        let backend = match sub_m.get_one::<String>("backend") {
            Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
            Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
            _ => config.backend,
        };
        // CI always installs natively; no external backend probe is needed.
        let backend = backend.unwrap_or(jhol_core::Backend::Npm);
        let json_out = flag(&sub_m, "json");
        let roots: Vec<std::path::PathBuf> = if all_workspaces {
            let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
            if r.is_empty() {
                vec![cwd.clone()]
            } else {
                r.into_iter().map(|p| cwd.join(p)).collect()
            }
        } else {
            vec![cwd.clone()]
        };
        let mut jagr_all_ok = true;
        for root in &roots {
            std::env::set_current_dir(root)
                .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
            // ci honours --omit and NODE_ENV=production too: a deploy that runs
            // `jhol ci` must be able to leave devDependencies out. Resolve the
            // classes BEFORE the spec list, since the list depends on them.
            let omitted_classes = jhol_core::omit::Omit::NONE.with_env(false);
            let specs =
                jhol_core::resolve_install_from_package_json_omit(true, omitted_classes)
                    .map_err(|e| e.to_string())?;
            let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
            let opts = jhol_core::InstallOptions {
                no_cache: false,
                quiet,
                backend,
                lockfile_only: false,
                offline,
                strict_lockfile: true,
                strict_peer_deps: flag(&sub_m, "strict-peer-deps"),
                from_lockfile: true,
                native_only: true,
                // Parity with `install`: scripts stay off by default, and naming
                // packages in --script-allowlist is itself the opt-in.
                no_scripts: !flag(&sub_m, "scripts") && {
                    let allow: Vec<String> = sub_m
                        .get_many::<String>("script-allowlist")
                        .into_iter()
                        .flatten()
                        .flat_map(|v| v.split(','))
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                        .collect();
                    allow.is_empty()
                },
                script_allowlist: {
                    let mut allow: std::collections::HashSet<String> = sub_m
                        .get_many::<String>("script-allowlist")
                        .into_iter()
                        .flatten()
                        .flat_map(|v| v.split(','))
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                        .collect();
                    allow.extend(read_trusted_dependencies(std::path::Path::new(
                        "package.json",
                    )));
                    if allow.is_empty() {
                        None
                    } else {
                        Some(allow)
                    }
                },
                resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
                overrides: None,
                telemetry: jhol_core::telemetry_enabled(&config),
                omit: omitted_classes,
                engine_strict: flag(&sub_m, "engine-strict"),
            };
            // `npm ci` semantics: a clean install. Without this, `ci` reused
            // whatever was already in `node_modules`, and every entry found there
            // was skipped *unverified* — so a stale or tampered tree survived `ci`,
            // the exact opposite of its contract (and of its own help text).
            // Removing it first also means nothing is inherited from a previous
            // install, so the result is determined by the lockfile alone.
            let nm = root.join("node_modules");
            if nm.exists() {
                if !quiet {
                    info(&format!("Removing {} (clean install)", nm.display()));
                }
                std::fs::remove_dir_all(&nm)
                    .map_err(|e| format!("Failed to clear {}: {}", nm.display(), e))?;
            }
            jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string())?;
            // Markers (write_status, store_last_hash, fingerprint) are written by
            // install_package via write_install_markers() — single write.

            if jagr_check {
                // After install, run JAGR three-layer analysis on the manifest
                // to confirm the lockfile's resolutions still satisfy it.
                let manifest = root.join("package.json");
                match jhol_core::jagr_analyze_project_manifest(&manifest) {
                    Ok(r) => {
                        if r.is_satisfiable {
                            if use_color() {
                                println!(
                                    "{} {}: JAGR OK ({}: β₁={}, β₂={}, SNF={})",
                                    "[JAGR]".bright_green(),
                                    root.display(),
                                    r.strategy_used,
                                    r.betti_1,
                                    r.betti_2,
                                    r.snf_satisfied
                                );
                            } else {
                                println!(
                                    "[JAGR] {}: JAGR OK ({}: β₁={}, β₂={}, SNF={})",
                                    root.display(),
                                    r.strategy_used,
                                    r.betti_1,
                                    r.betti_2,
                                    r.snf_satisfied
                                );
                            }
                        } else {
                            jagr_all_ok = false;
                            error(&format!(
                                "[JAGR] {}: UNSAT — {}",
                                root.display(),
                                r.explanation
                            ));
                        }
                    }
                    Err(e) => {
                        jagr_all_ok = false;
                        error(&format!(
                            "[JAGR] {}: analysis failed: {}",
                            root.display(),
                            e
                        ));
                    }
                }
            }
        }
        std::env::set_current_dir(&cwd).ok();
        if json_out {
            let jagr_field = if jagr_check {
                format!(
                    ", \"jagr_check\":\"{}\"",
                    if jagr_all_ok { "ok" } else { "fail" }
                )
            } else {
                String::new()
            };
            println!("{{\"schemaVersion\":\"1\",\"command\":\"ci\",\"status\":\"ok\",\"workspaces\":{}{}}}",
                roots.len(), jagr_field);
        }
        if jagr_check && !jagr_all_ok {
            std::process::exit(2);
        }
        return Ok(());
    }

    jhol_core::init_cache().map_err(|e| format!("Failed to initialize cache: {}", e))?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    jhol_core::apply_enterprise_network_env(&cwd);
    let config = jhol_core::load_config(&cwd);
    if let Some(ref d) = config.cache_dir {
        env::set_var("JHOL_CACHE_DIR", d);
    }

    // ── Feature 10: output config ──────────────────────────────────────────────
    if config.output_quiet == Some(true) && env::var("JHOL_QUIET").is_err() {
        env::set_var("JHOL_QUIET", "1");
    }
    if config.output_no_color == Some(true) && env::var("NO_COLOR").is_err() {
        env::set_var("NO_COLOR", "1");
    }
    if config.output_json_log == Some(true) {
        if env::var("JHOL_LOG_FILE").is_err() {
            env::set_var("JHOL_LOG_FILE", "1");
        }
    }

    match matches.subcommand() {
        Some(("install", sub_m)) => {
            // History as an input: the resolver reads this, so it is set before
            // any lane can act on the manifest.
            if let Some(as_of) = sub_m.get_one::<String>("as-of") {
                std::env::set_var("JHOL_AS_OF", as_of);
            }
            if let Some(list) = sub_m.get_one::<String>("allow-license") {
                std::env::set_var("JHOL_ALLOW_LICENSE", list);
            }
            if flag(&sub_m, "license-allow-unknown") {
                std::env::set_var("JHOL_LICENSE_ALLOW_UNKNOWN", "1");
            }
            if flag(&sub_m, "exclude-vuln") {
                std::env::set_var("JHOL_EXCLUDE_VULN", "1");
            }
            // --omit / --prod / --include-dev, composed with NODE_ENV.
            // Resolved once here; both the sequential and parallel workspace
            // paths and the direct-install closure read the same value.
            let cli_omit: jhol_core::omit::Omit = match sub_m.get_one::<String>("omit") {
                Some(list) => match jhol_core::omit::Omit::parse(list) {
                    Ok(o) => o,
                    Err(e) => return Err(e),
                },
                None => jhol_core::omit::Omit::NONE,
            };
            let include_dev = flag(&sub_m, "include-dev");
            if std::env::var_os("JHOL_OMIT_DEBUG").is_some() {
                eprintln!("[omit] cli_omit={:?} prod={} include_dev={}", cli_omit, flag(&sub_m, "prod"), include_dev);
            }

            let omitted_classes: jhol_core::omit::Omit = if flag(&sub_m, "prod") {
                jhol_core::omit::Omit { dev: true, ..cli_omit }.with_env(include_dev)
            } else {
                cli_omit.with_env(include_dev)
            };
            if flag(&sub_m, "global") {
                let pkg_refs: Vec<&str> = sub_m
                    .get_many::<String>("package")
                    .map(|v| v.map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                let gopts = jhol_core::InstallOptions {
                    quiet: flag(&sub_m, "quiet"),
                    offline: flag(&sub_m, "offline"),
                    native_only: true,
                    no_scripts: !flag(&sub_m, "scripts"),
                    omit: omitted_classes,
                    engine_strict: false,
                    ..Default::default()
                };
                match jhol_core::global_install::install_global(&pkg_refs, &gopts) {
                    Ok(shims) => {
                        if pkg_refs.is_empty() {
                            println!("Installed global packages into {}.",
                                jhol_core::global_install::prefix().display());
                        }
                        for shim in shims {
                            println!("  linked {}", shim);
                        }
                        println!("Add to PATH: export PATH=\"{}:$PATH\"",
                            jhol_core::global_install::bin_dir().display());
                    }
                    Err(e) => {
                        eprintln!("global install: {}", e);
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            let no_cache = flag(&sub_m, "no-cache");
            let quiet = flag(&sub_m, "quiet");
            let lockfile_only = flag(&sub_m, "lockfile-only");
            let offline = flag(&sub_m, "offline")
                || env::var("JHOL_OFFLINE")
                    .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
            let strict_lockfile = flag(&sub_m, "frozen") || config.frozen.unwrap_or(false);
            let strict_peer_deps = flag(&sub_m, "strict-peer-deps")
                || env::var("JHOL_STRICT_PEER_DEPS")
                    .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

            // Feature 1: native_only / fallback_backend from config (CLI takes precedence)
            let cli_fallback = flag(&sub_m, "fallback-backend");
            let cli_native_only = flag(&sub_m, "native-only");
            let native_only = if cli_native_only {
                true
            } else if cli_fallback {
                false
            } else {
                // Neither CLI flag set — use config fallback_backend if present
                config.fallback_backend.map(|fb| !fb).unwrap_or(true)
            };

            let all_workspaces = flag(&sub_m, "all-workspaces");

            // Feature 1: no_scripts — CLI flags take precedence, then config
            let cli_scripts_flag = flag(&sub_m, "scripts");
            let cli_no_scripts_flag = flag(&sub_m, "no-scripts");
            // `--script-allowlist pkg[,pkg]` (repeatable). Naming packages to allow
            // is itself an opt-in, so it turns scripts on for those packages rather
            // than being silently ignored while `no_scripts` stays true.
            let cli_script_allowlist: HashSet<String> = sub_m
                .get_many::<String>("script-allowlist")
                .into_iter()
                .flatten()
                .flat_map(|v| v.split(','))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            let no_scripts = if cli_scripts_flag || !cli_script_allowlist.is_empty() {
                false
            } else if cli_no_scripts_flag {
                true
            } else {
                config.no_scripts.unwrap_or(true)
            };

            // The daemon request is a packed fixed-size wire struct; it has no
            // slot for dependency-class omission. Sending a --omit install to
            // the daemon would silently install the omitted classes, so an
            // omitting install always runs in-process.
            let mut use_daemon = flag(&sub_m, "daemon") && omitted_classes.is_empty();
            let mut daemon_auto_attach = false;
            if !use_daemon
                && omitted_classes.is_empty()
                && jhol_core::soul_path::auto_daemon_enabled()
            {
                #[cfg(unix)]
                {
                    // Safety gate: only connect to existing socket, don't auto-start
                    let sock = daemon_socket_path();
                    if sock.exists() {
                        use_daemon = true;
                        daemon_auto_attach = true;
                        if !quiet {
                            info("Soul Path: auto-daemon enabled");
                        }
                    }
                }
                #[cfg(not(unix))]
                { /* daemon is Unix-only */ }
            }
            if let Some(resolver) = sub_m.get_one::<String>("resolver") {
                env::set_var("JHOL_RESOLVER", resolver);
            }

            if let Some(ft) = sub_m.get_one::<String>("fetch-timeout") {
                env::set_var("JHOL_FETCH_TIMEOUT_SECS", ft);
            } else if let Some(ft) = config.fetch_timeout {
                if env::var("JHOL_FETCH_TIMEOUT_SECS").is_err() {
                    env::set_var("JHOL_FETCH_TIMEOUT_SECS", ft.to_string());
                }
            }
            if let Some(it) = sub_m.get_one::<String>("install-timeout") {
                env::set_var("JHOL_INSTALL_TIMEOUT_SECS", it);
            } else if let Some(it) = config.install_timeout {
                if env::var("JHOL_INSTALL_TIMEOUT_SECS").is_err() {
                    env::set_var("JHOL_INSTALL_TIMEOUT_SECS", it.to_string());
                }
            }
            if let Some(con) = config.concurrency {
                if env::var("JHOL_DOWNLOAD_CONCURRENCY").is_err() {
                    env::set_var("JHOL_DOWNLOAD_CONCURRENCY", con.to_string());
                }
            }

            let script_allowlist: Option<HashSet<String>> = env::var("JHOL_SCRIPT_ALLOWLIST")
                .ok()
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<HashSet<_>>()
                })
                .filter(|s| !s.is_empty());
            let backend = match sub_m.get_one::<String>("backend") {
                Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
                Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
                _ => config.backend,
            };
            // Only fallback-enabled installs need to launch an external backend probe.
            let backend = if native_only {
                backend.unwrap_or(jhol_core::Backend::Npm)
            } else {
                jhol_core::resolve_backend(backend)
            };
            let json_out = flag(&sub_m, "json");
            if quiet || json_out {
                env::set_var("JHOL_QUIET", "1");
            }
            let packages: Vec<String> = sub_m
                .get_many::<String>("package")
                .map(|it| it.map(|s| s.clone()).collect())
                .unwrap_or_default();
            let save = !flag(&sub_m, "no-save");
            if save && !packages.is_empty() {
                let pkg_refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
                let target = if flag(&sub_m, "save-dev") {
                    SaveTarget::Dev
                } else if flag(&sub_m, "save-peer") {
                    SaveTarget::Peer
                } else if flag(&sub_m, "save-optional") {
                    SaveTarget::Optional
                } else {
                    SaveTarget::Dependencies
                };
                let _ = persist_added_deps_opts(
                    &pkg_refs,
                    target,
                    flag(&sub_m, "save-exact"),
                );
            }

            // Feature 5: workspaces parallel / bail
            let ws_parallel = config.workspaces_parallel.unwrap_or(false);
            let ws_bail = config.workspaces_bail.unwrap_or(true);

            let mut roots: Vec<std::path::PathBuf> = if all_workspaces {
                let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                if r.is_empty() {
                    vec![cwd.clone()]
                } else {
                    r.into_iter().map(|p| cwd.join(p)).collect()
                }
            } else {
                vec![cwd.clone()]
            };
            // `--filter`: keep only workspaces whose package.json name matches
            // a pattern (`name`, `pkg-*`, `@scope/*`).
            let filters: Vec<String> = sub_m
                .get_many::<String>("filter")
                .map(|v| v.cloned().collect())
                .unwrap_or_default();
            if !filters.is_empty() {
                let matches = |name: &str| -> bool {
                    filters.iter().any(|pat| match pat.strip_suffix('*') {
                        Some(prefix) => name.starts_with(prefix),
                        None => name == pat,
                    })
                };
                let before = roots.len();
                roots.retain(|root| {
                    if *root == cwd {
                        return false;
                    }
                    std::fs::read_to_string(root.join("package.json"))
                        .ok()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                        .and_then(|v| {
                            v.get("name").and_then(|n| n.as_str()).map(String::from)
                        })
                        .map(|n| matches(&n))
                        .unwrap_or(false)
                });
                if roots.is_empty() {
                    return Err(format!(
                        "--filter matched none of the {} workspace root(s); patterns: {}",
                        before,
                        filters.join(", ")
                    ));
                }
                if !quiet {
                    info(&format!("--filter selected {} workspace(s)", roots.len()));
                }
            }

            // Helper: run install for a single root (used by both sequential and parallel paths)
            let run_install_for_root = |root: &std::path::Path,
                                        packages: &Vec<String>,
                                        config: &jhol_core::Config,
                                        script_allowlist: &Option<HashSet<String>>,
                                        quiet: bool,
                                        json_out: bool,
                                        no_cache: bool,
                                        lockfile_only: bool,
                                        offline: bool,
                                        strict_lockfile: bool,
                                        strict_peer_deps: bool,
                                        from_lockfile_override: Option<bool>,
                                        native_only: bool,
                                        no_scripts: bool,
                                        backend: jhol_core::Backend,
                                        use_daemon: bool|
             -> Result<(), String> {
                let specs: Vec<String> = if packages.is_empty() {
                    jhol_core::resolve_install_from_package_json_omit(strict_lockfile, omitted_classes)
                        .map_err(|e| e.to_string())?
                } else {
                    packages.iter().map(|s| s.clone()).collect()
                };
                if specs.is_empty() {
                    // Nothing to install, but the tree on disk may still hold
                    // packages whose engines.node excludes this runtime.
                    check_package_manager_pin(std::path::Path::new(&root), false, quiet)?;
                    report_engine_mismatches(
                        std::path::Path::new(&root),
                        flag(&sub_m, "engine-strict"),
                        quiet,
                    )?;
                    return Ok(());
                }
                let from_lockfile = from_lockfile_override.unwrap_or_else(|| {
                    packages.is_empty()
                        && specs.iter().all(|s| {
                            s.contains('@') && s.rfind('@').map(|i| i > 0).unwrap_or(false)
                        })
                        && jhol_core::has_jhol_lockfile(std::path::Path::new("."))
                });
                let opts = jhol_core::InstallOptions {
                    no_cache,
                    quiet,
                    backend,
                    lockfile_only,
                    offline,
                    strict_lockfile,
                    strict_peer_deps,
                    from_lockfile,
                    native_only,
                    no_scripts,
                    script_allowlist: {
                        let mut merged = script_allowlist.clone().unwrap_or_default();
                        merged.extend(cli_script_allowlist.iter().cloned());
                        if !no_scripts {
                            merged.extend(read_trusted_dependencies(std::path::Path::new(
                                "package.json",
                            )));
                        }
                        if let Some(ref td) = config.trusted_dependencies {
                            merged.extend(td.iter().cloned());
                        }
                        if merged.is_empty() {
                            None
                        } else {
                            Some(merged)
                        }
                    },
                    resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
                    overrides: {
                        let mut ov = config.overrides.clone().unwrap_or_default();
                        ov.extend(config.resolutions.clone().unwrap_or_default());
                        if ov.is_empty() {
                            None
                        } else {
                            Some(ov)
                        }
                    },
                    telemetry: jhol_core::telemetry_enabled(&config),
                    omit: omitted_classes,
                    engine_strict: false,
                };
                // Feature 6: pre-install hook
                if let Some(ref hook) = config.hook_pre_install {
                    if let Err(e) = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(hook)
                        .current_dir(root)
                        .status()
                    {
                        eprintln!("Warning: pre-install hook failed: {}", e);
                    }
                }
                // One readable line instead of a raw Vec dump; the full list is
                // for the log file and for JHOL_VERBOSE.
                if jhol_core::utils::is_verbose() {
                    jhol_core::log(&format!("installing {} packages: {:?}", specs.len(), specs));
                } else {
                    jhol_core::log(&format!(
                        "installing {} dependenc{}",
                        specs.len(),
                        if specs.len() == 1 { "y" } else { "ies" }
                    ));
                }
                let install_start = std::time::Instant::now();
                let pkg_count = specs.len();
                let count_before = jhol_core::count_installed_packages(root);
                let install_result = if use_daemon {
                    // Lanes before daemon: the transport serves *work*, not answers
                    // we can serve locally in milliseconds. A bundle published by a
                    // byte-identical earlier install replays locally (the daemon
                    // never sees the trampoline lanes — flags bypass them), and a
                    // valid tree's proof short-circuits inside install_package on
                    // the daemon regardless. Without this gate, every
                    // lane-answerable project paid client spawn + IPC + a full
                    // install_package on the daemon: measured +12 ms on
                    // fresh-replay-able installs.
                    if jhol_core::try_fresh_project_bundle_replay(Path::new(".")) {
                        // The replayed bundle is byte-identical to the approved
                        // install that published it, so script artifacts are
                        // already inside. The lifecycle phase still runs: its
                        // run-marker suppresses re-execution while keeping the
                        // consent summary honest (new packages, new receipts).
                        jhol_core::install::run_post_install_lifecycle_default()?;
                        Ok(())
                    } else {
                        let dr = daemon_try_install(root, &specs, &opts, daemon_auto_attach);
                        // Safety gate fallback: auto-attach with no socket → direct install
                        if daemon_auto_attach && dr.is_err() {
                            if !quiet {
                                info("Daemon unavailable — falling back to direct install");
                            }
                            direct_install(root, &specs, &opts, pkg_count, quiet, json_out)
                        } else {
                            dr
                        }
                    }
                } else {
                    direct_install(root, &specs, &opts, pkg_count, quiet, json_out)
                };
                install_result?;
                // Recorded patches are the user's own fixes for a broken
                // dependency: re-apply them after every lane, including the
                // fast ones that materialise a pristine tree.
                if let Ok((pkgs, _files)) =
                    jhol_core::patch::apply_all(std::path::Path::new(&root))
                {
                    if pkgs > 0 && !quiet {
                        info(&format!("Applied {} patch(es)", pkgs));
                    }
                }
                check_package_manager_pin(
                    std::path::Path::new(&root),
                    false,
                    quiet,
                )?;
                report_engine_mismatches(
                    std::path::Path::new(&root),
                    flag(&sub_m, "engine-strict"),
                    quiet,
                )?;
                let elapsed = install_start.elapsed();
                let count_after = jhol_core::count_installed_packages(root);
                let (added, _) = install_summary_counts(count_before, count_after);
                print_install_summary(added, count_after, elapsed, quiet, json_out);
                // Feature 6: post-install hook
                if let Some(ref hook) = config.hook_post_install {
                    if let Err(e) = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(hook)
                        .current_dir(root)
                        .status()
                    {
                        eprintln!("Warning: post-install hook failed: {}", e);
                    }
                }
                // Feature 4: cache auto-management
                if let Some(max_gb) = config.cache_max_size_gb {
                    if let Ok((bytes, _)) = jhol_core::cache_size_bytes() {
                        let max_bytes = (max_gb * 1024.0 * 1024.0 * 1024.0) as u64;
                        if bytes > max_bytes {
                            let _ = jhol_core::cache_prune(config.cache_keep_recent);
                        }
                    }
                }
                if config.cache_auto_prewarm == Some(true) {
                    let _ = thread::spawn(|| {
                        let _ = jhol_core::prefetch_from_lockfile(true);
                    });
                }
                Ok(())
            };

            if all_workspaces && ws_parallel && roots.len() > 1 {
                // Feature 5: parallel workspace install
                let mut handles = Vec::new();
                let telemetry_flag = jhol_core::telemetry_enabled(&config);
                for root in &roots {
                    let root = root.clone();
                    let packages = packages.clone();
                    let config_overrides_clone = config.overrides.clone();
                    let config_resolutions_clone = config.resolutions.clone();
                    let config_trusted_clone = config.trusted_dependencies.clone();
                    let config_hook_pre = config.hook_pre_install.clone();
                    let config_hook_post = config.hook_post_install.clone();
                    let config_cache_max = config.cache_max_size_gb;
                    let config_cache_keep = config.cache_keep_recent;
                    let config_prewarm = config.cache_auto_prewarm;
                    let script_allowlist_clone = script_allowlist.clone();
                    handles.push(thread::spawn(move || -> Result<(), String> {
                        std::env::set_current_dir(&root)
                            .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                        let specs: Vec<String> = if packages.is_empty() {
                            jhol_core::resolve_install_from_package_json_omit(strict_lockfile, omitted_classes)
                                .map_err(|e| e.to_string())?
                        } else {
                            packages.clone()
                        };
                        if specs.is_empty() {
                            return Ok(());
                        }
                        let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
                        let from_lockfile = packages.is_empty()
                            && specs.iter().all(|s| {
                                s.contains('@') && s.rfind('@').map(|i| i > 0).unwrap_or(false)
                            })
                            && jhol_core::has_jhol_lockfile(std::path::Path::new("."));
                        let opts = jhol_core::InstallOptions {
                            no_cache,
                            quiet,
                            backend,
                            lockfile_only,
                            offline,
                            strict_lockfile,
                            strict_peer_deps,
                            from_lockfile,
                            native_only,
                            no_scripts,
                            script_allowlist: {
                                let mut merged = script_allowlist_clone.unwrap_or_default();
                                if !no_scripts {
                                    merged.extend(read_trusted_dependencies(std::path::Path::new(
                                        "package.json",
                                    )));
                                }
                                if let Some(ref td) = config_trusted_clone {
                                    merged.extend(td.iter().cloned());
                                }
                                if merged.is_empty() {
                                    None
                                } else {
                                    Some(merged)
                                }
                            },
                            resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
                            overrides: {
                                let mut ov = config_overrides_clone.unwrap_or_default();
                                ov.extend(config_resolutions_clone.unwrap_or_default());
                                if ov.is_empty() {
                                    None
                                } else {
                                    Some(ov)
                                }
                            },
                            telemetry: telemetry_flag,
                            omit: omitted_classes,
                            engine_strict: false,
                        };
                        if let Some(ref hook) = config_hook_pre {
                            if let Err(e) = std::process::Command::new("sh")
                                .arg("-c")
                                .arg(hook)
                                .current_dir(&root)
                                .status()
                            {
                                eprintln!("Warning: pre-install hook failed: {}", e);
                            }
                        }
                        jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string())?;
                        // Markers (write_status, store_last_hash, fingerprint) are written by
                        // install_package via write_install_markers() — single write.
                        if let Some(ref hook) = config_hook_post {
                            if let Err(e) = std::process::Command::new("sh")
                                .arg("-c")
                                .arg(hook)
                                .current_dir(&root)
                                .status()
                            {
                                eprintln!("Warning: post-install hook failed: {}", e);
                            }
                        }
                        if let Some(max_gb) = config_cache_max {
                            if let Ok((bytes, _)) = jhol_core::cache_size_bytes() {
                                let max_bytes = (max_gb * 1024.0 * 1024.0 * 1024.0) as u64;
                                if bytes > max_bytes {
                                    let _ = jhol_core::cache_prune(config_cache_keep);
                                }
                            }
                        }
                        if config_prewarm == Some(true) {
                            let _ = thread::spawn(|| {
                                let _ = jhol_core::prefetch_from_lockfile(true);
                            });
                        }
                        Ok(())
                    }));
                }
                let mut errors: Vec<String> = Vec::new();
                for h in handles {
                    match h.join() {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            if ws_bail {
                                return Err(e);
                            }
                            errors.push(e);
                        }
                        Err(_) => {
                            let e = "workspace install thread panicked".to_string();
                            if ws_bail {
                                return Err(e);
                            }
                            errors.push(e);
                        }
                    }
                }
                std::env::set_current_dir(&cwd).ok();
                if !errors.is_empty() {
                    return Err(format!("Workspace install errors:\n{}", errors.join("\n")));
                }
            } else {
                // Sequential workspace install (original behavior)
                for root in &roots {
                    std::env::set_current_dir(root)
                        .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                    if !quiet && roots.len() > 1 {
                        info(&format!("Workspace: {}", root.display()));
                    }
                    if packages.is_empty() && lockfile_only {
                        jhol_core::install_lockfile_only(backend).map_err(|e| e.to_string())?;
                        continue;
                    }
                    if packages.is_empty()
                        && !no_cache
                        && !offline
                        && jhol_core::lockfile_fingerprint_matches()
                    {
                        if !quiet {
                            jhol_core::log("Already up-to-date.");
                        }
                        continue;
                    }
                    run_install_for_root(
                        root,
                        &packages,
                        &config,
                        &script_allowlist,
                        quiet,
                        json_out,
                        no_cache,
                        lockfile_only,
                        offline,
                        strict_lockfile,
                        strict_peer_deps,
                        None,
                        native_only,
                        no_scripts,
                        backend,
                        use_daemon,
                    )?;
                }
                std::env::set_current_dir(&cwd).ok();
            }
            if save && !packages.is_empty() {
                let pkg_refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
                refine_star_specs(&pkg_refs);
            }
            if json_out {
                println!("{{\"schemaVersion\":\"1\",\"command\":\"install\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
            } else if quiet {
                success("Done.");
            }
        }
        Some(("doctor", sub_m)) => {
            let quiet = flag(&sub_m, "quiet");
            let do_fix = flag(&sub_m, "fix");
            let explain = flag(&sub_m, "explain");
            let use_jagr = flag(&sub_m, "jagr");
            let all_workspaces = flag(&sub_m, "all-workspaces");
            let json_out = flag(&sub_m, "json");
            let backend = match sub_m.get_one::<String>("backend") {
                Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
                Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
                _ => config.backend,
            };
            let backend = jhol_core::resolve_backend(backend);
            if quiet || json_out {
                env::set_var("JHOL_QUIET", "1");
            }
            if explain {
                let report = jhol_core::explain_project_health()?;
                println!("{}", report);
                return Ok(());
            }
            if use_jagr {
                // JAGR-driven doctor: run the three-layer analysis on the
                // project's manifest, then print a health summary that
                // combines layer outputs with project-level recommendations.
                let manifest = cwd.join("package.json");
                match jhol_core::jagr_analyze_project_manifest(&manifest) {
                    Ok(report) => {
                        if use_color() {
                            println!(
                                "{}",
                                "JAGR Doctor — Three-Layer Resolver Health Check"
                                    .bright_cyan()
                                    .bold()
                            );
                        } else {
                            println!("JAGR Doctor — Three-Layer Resolver Health Check");
                        }
                        println!("Manifest:       {}", report.manifest_path);
                        println!("Satisfiable:    {}", report.is_satisfiable);
                        println!("Strategy:       {}", report.strategy_used);
                        println!("β₁:             {}", report.betti_1);
                        println!("β₂:             {}", report.betti_2);
                        println!("SNF satisfied:  {}", report.snf_satisfied);
                        println!("Packages:       {}", report.package_count);
                        let total = report.layer_times_ms.get("total").copied().unwrap_or(0.0);
                        let layer0 = report.layer_times_ms.get("layer0").copied().unwrap_or(0.0);
                        println!("Total time:     {:.2} ms", total);

                        // ── Resolution Differential (Layer 0) output ──────────
                        if let Some(ref diff) = report.differential {
                            let stability_label = match diff.stability {
                                jhol_core::StabilityClass::Stable => "STABLE ✓",
                                jhol_core::StabilityClass::Marginal => "MARGINAL !",
                                jhol_core::StabilityClass::Fragile => "FRAGILE ✗",
                                jhol_core::StabilityClass::Degenerate => "–",
                            };
                            if use_color() {
                                println!();
                                println!("{}", "  ── Resolution Differential (Layer 0) ──".cyan());
                                println!(
                                    "  Spectral gap δ:    {:.4}  [{:.2} ms]",
                                    diff.spectral_gap, layer0
                                );
                                println!("  Fiedler value λ₂:  {:.4}", diff.fiedler_value);
                                println!("  Predicted β₁:      {}", diff.predicted_beta1);
                                println!("  Stability:         {}", stability_label);
                                let risky = diff.high_risk_packages(0.5);
                                if !risky.is_empty() {
                                    println!(
                                        "  High-risk pkgs:    {}",
                                        risky
                                            .iter()
                                            .take(3)
                                            .map(|(k, v)| format!("{} ({:.0}%)", k, v * 100.0))
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    );
                                }
                            } else {
                                println!(
                                    "Spectral gap:   {:.4}  [{:.2}ms]",
                                    diff.spectral_gap, layer0
                                );
                                println!("Stability:      {}", stability_label);
                            }
                        }
                        println!();
                        // Health verdict
                        if !report.is_satisfiable {
                            error("JAGR verdict: UNSAT — dependency conflict detected");
                            if !report.explanation.is_empty() {
                                println!("Explanation:");
                                for line in report.explanation.lines() {
                                    println!("  {}", line);
                                }
                            }
                            std::process::exit(2);
                        } else if !report.snf_satisfied {
                            warning("JAGR verdict: Non-SNF — project has topological cycles in its dep graph");
                            println!("  Resolution used Layer 1 (Max-SAT) instead of the faster greedy path.");
                            println!("  Consider reviewing peer/optional dependencies to reduce the Betti numbers.");
                        } else if report.gluing_has_obstruction {
                            warning(
                                "JAGR verdict: SNF, but Mayer-Vietoris gluing obstruction present",
                            );
                        } else {
                            success("JAGR verdict: Healthy — SNF satisfied, no gluing obstruction, satisfiable.");
                        }
                        if json_out {
                            let j = serde_json::json!({
                                "manifest": report.manifest_path,
                                "satisfiable": report.is_satisfiable,
                                "strategy_used": report.strategy_used,
                                "b1": report.betti_1,
                                "b2": report.betti_2,
                                "snf_satisfied": report.snf_satisfied,
                                "package_count": report.package_count,
                                "total_time_ms": report.layer_times_ms.get("total").copied().unwrap_or(0.0),
                                "explanation": report.explanation,
                            });
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&j)
                                    .unwrap_or_else(|_| "{}".to_string())
                            );
                        }
                    }
                    Err(e) => {
                        error(&format!("JAGR doctor failed: {}", e));
                        std::process::exit(2);
                    }
                }
                return Ok(());
            }
            let roots: Vec<std::path::PathBuf> = if all_workspaces {
                let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                if r.is_empty() {
                    vec![cwd.clone()]
                } else {
                    r.into_iter().map(|p| cwd.join(p)).collect()
                }
            } else {
                vec![cwd.clone()]
            };
            for root in &roots {
                std::env::set_current_dir(root)
                    .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                if !quiet && !json_out && roots.len() > 1 {
                    info(&format!("Workspace: {}", root.display()));
                }
                if do_fix {
                    jhol_core::log("Running doctor --fix");
                    jhol_core::fix_dependencies(quiet, backend).map_err(|e| e.to_string())?;
                } else {
                    jhol_core::log("Running doctor (check only)");
                    jhol_core::check_dependencies(quiet, backend).map_err(|e| e.to_string())?;
                }
            }
            std::env::set_current_dir(&cwd).ok();
            if json_out {
                println!("{{\"schemaVersion\":\"1\",\"command\":\"doctor\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
            } else if quiet {
                success("Done.");
            }
        }
        Some(("audit", sub_m)) => {
            let quiet = flag(&sub_m, "quiet");
            let do_fix = flag(&sub_m, "fix");
            let gate = flag(&sub_m, "gate") || config.audit_gate == Some(true);
            let all_workspaces = flag(&sub_m, "all-workspaces");
            let json_out = flag(&sub_m, "json");
            let backend = match sub_m.get_one::<String>("backend") {
                Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
                Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
                _ => config.backend,
            };
            let backend = jhol_core::resolve_backend(backend);
            if quiet || json_out {
                env::set_var("JHOL_QUIET", "1");
            }
            // Feature 3: audit filtering
            let ignore_advisories: Vec<String> =
                config.audit_ignore_advisories.clone().unwrap_or_default();
            let min_severity: Option<String> = config.audit_min_severity.clone();

            // Feature 6: pre-audit hook
            if let Some(ref hook) = config.hook_pre_audit {
                if let Err(e) = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(hook)
                    .current_dir(&cwd)
                    .status()
                {
                    eprintln!("Warning: pre-audit hook failed: {}", e);
                }
            }

            let audit_result: Result<(), String> = (|| {
                if gate {
                    jhol_core::run_audit_gate_filtered(
                        backend,
                        &ignore_advisories,
                        min_severity.as_deref(),
                    )?;
                    if json_out {
                        println!("{{\"schemaVersion\":\"1\",\"command\":\"audit\",\"status\":\"ok\",\"gate\":true}}");
                    }
                } else if json_out && !do_fix {
                    let json_bytes = jhol_core::run_audit_raw_filtered(
                        backend,
                        &ignore_advisories,
                        min_severity.as_deref(),
                    )
                    .map_err(|e| e.to_string())?;
                    println!("{}", String::from_utf8_lossy(&json_bytes));
                } else {
                    let roots: Vec<std::path::PathBuf> = if all_workspaces {
                        let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                        if r.is_empty() {
                            vec![cwd.clone()]
                        } else {
                            r.into_iter().map(|p| cwd.join(p)).collect()
                        }
                    } else {
                        vec![cwd.clone()]
                    };
                    for root in &roots {
                        std::env::set_current_dir(root)
                            .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                        if !quiet && !json_out && roots.len() > 1 {
                            info(&format!("Workspace: {}", root.display()));
                        }
                        if do_fix {
                            jhol_core::run_audit_fix_filtered(
                                backend,
                                quiet,
                                &ignore_advisories,
                                min_severity.as_deref(),
                            )?;
                        } else {
                            jhol_core::run_audit_filtered(
                                backend,
                                quiet,
                                &ignore_advisories,
                                min_severity.as_deref(),
                            )?;
                        }
                    }
                    std::env::set_current_dir(&cwd).ok();
                    if json_out {
                        println!("{{\"schemaVersion\":\"1\",\"command\":\"audit\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
                    } else if quiet {
                        success("Done.");
                    }
                }
                Ok(())
            })();

            // Feature 6: post-audit hook (best-effort, even if audit found vulns)
            if let Some(ref hook) = config.hook_post_audit {
                if let Err(e) = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(hook)
                    .current_dir(&cwd)
                    .status()
                {
                    eprintln!("Warning: post-audit hook failed: {}", e);
                }
            }
            audit_result?;
        }
        #[cfg(feature = "bundle")]
        Some(("fmt", sub_m)) => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let check = flag(&sub_m, "check") || flag(&sub_m, "list-different");
            let no_cache = flag(&sub_m, "no-cache");

            // Config precedence: CLI flags > .jholrc [fmt] > .prettierrc > spec defaults.
            let mut config = jhol_bundle::fmt::load_config(&cwd);
            if let Some(n) = sub_m.get_one::<String>("print-width") {
                if let Ok(n) = n.parse::<usize>() {
                    config.print_width = n;
                }
            }
            if let Some(n) = sub_m.get_one::<String>("tab-width") {
                if let Ok(n) = n.parse::<usize>() {
                    config.tab_width = n;
                }
            }
            if flag(&sub_m, "use-tabs") {
                config.use_tabs = true;
            }
            if flag(&sub_m, "no-semi") {
                config.semi = false;
            }
            if flag(&sub_m, "single-quote") {
                config.single_quote = true;
            }
            if let Some(tc) = sub_m.get_one::<String>("trailing-comma") {
                config.trailing_comma = tc.clone();
            }

            // --init: write spec defaults to .prettierrc.json when absent.
            if flag(&sub_m, "init") {
                let path = cwd.join(".prettierrc.json");
                if path.exists() {
                    info(".prettierrc.json already exists — leaving it alone");
                } else {
                    let body = serde_json::to_string_pretty(&serde_json::json!({
                        "printWidth": 80, "tabWidth": 2, "useTabs": false,
                        "semi": true, "singleQuote": false, "trailingComma": "all"
                    }))
                    .unwrap_or_default();
                    std::fs::write(&path, format!("{}\n", body)).map_err(|e| e.to_string())?;
                    success("created .prettierrc.json with jhol defaults");
                }
                return Ok(());
            }

            // stdin mode: format stdin as the given path, print, write nothing.
            if let Some(fp) = sub_m.get_one::<String>("stdin-filepath") {
                use std::io::Read;
                let mut input = String::new();
                std::io::stdin()
                    .read_to_string(&mut input)
                    .map_err(|e| e.to_string())?;
                let out = jhol_bundle::fmt::format_source(&input, fp, &config)
                    .map_err(|e| e.to_string())?;
                print!("{}", out);
                return Ok(());
            }

            // discovery: explicit paths or the whole project tree.
            let roots: Vec<String> = sub_m
                .get_many::<String>("path")
                .map(|it| it.map(|s| s.clone()).collect())
                .unwrap_or_default();
            let mut files: Vec<std::path::PathBuf> = Vec::new();
            if roots.is_empty() {
                collect_fmt_files(std::path::Path::new("."), &mut files);
            } else {
                for r in &roots {
                    let pp = std::path::Path::new(r);
                    // An explicitly named path must be real, and explicitly named
                    // files must have an engine. Silently reporting
                    // "0 file(s), 0 changed" with exit 0 for a typo or an
                    // unsupported extension makes `jhol fmt --check` useless in
                    // CI: it passes while formatting nothing.
                    if !pp.exists() {
                        return Err(format!("fmt: no such file or directory: {r}"));
                    }
                    if pp.is_dir() {
                        collect_fmt_files(pp, &mut files);
                    } else {
                        let has_engine = jhol_bundle::fmt::is_supported(pp)
                            || pp
                                .extension()
                                .and_then(|e| e.to_str())
                                .and_then(jhol_bundle::fmt::external_tool)
                                .is_some();
                        if !has_engine {
                            return Err(format!(
                                "fmt: no formatter for {r} (supported: js, mjs, cjs, jsx, ts, tsx, mts, cts, json, css, scss, less, rs, go, py)"
                            ));
                        }
                        files.push(pp.to_path_buf());
                    }
                }
            }
            // keep files we have an engine for (in-process OR external tool)
            files.retain(|p| {
                jhol_bundle::fmt::is_supported(p)
                    || p.extension()
                        .and_then(|e| e.to_str())
                        .and_then(jhol_bundle::fmt::external_tool)
                        .is_some()
            });
            files.retain(|p| !jhol_bundle::fmt::is_ignored(p));
            let ignore_set = load_prettier_ignore(&cwd);
            files.retain(|p| {
                let rel = p.strip_prefix(&cwd).unwrap_or(p);
                let rel = rel.to_string_lossy().replace('\\', "/");
                !ignore_set.is_match(&rel)
            });
            files.sort();
            files.dedup();

            // cache: keyed by engine version + config digest so option
            // changes invalidate every entry.
            let digest = config.digest();
            let mut cache = if no_cache {
                jhol_bundle::fmt::FmtCache::default()
            } else {
                jhol_bundle::fmt::FmtCache::load(&cwd, jhol_bundle::fmt::FMT_ENGINE, digest)
            };
            let cache_lock = std::sync::Mutex::new(&mut cache);

            use rayon::prelude::*;
            let results: Vec<(std::path::PathBuf, Result<bool, String>)> = files
                .par_iter()
                .map(|p| {
                    let res = (|| -> Result<bool, String> {
                        let rel = p.strip_prefix(&cwd).unwrap_or(p);
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        let ext = p
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();

                        // external ecosystem formatter (rustfmt / gofmt / ruff)
                        if !jhol_bundle::fmt::is_supported(p) {
                            if let Some((tool, args)) = jhol_bundle::fmt::external_tool(&ext) {
                                let before =
                                    std::fs::read_to_string(p).map_err(|e| e.to_string())?;
                                let hash_before = jhol_bundle::fmt::content_hash(&before);
                                let status = match std::process::Command::new(tool)
                                    .args(&args)
                                    .arg(p)
                                    .status()
                                {
                                    Ok(s) => s,
                                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                        eprintln!(
                                            "jhol fmt: {} not installed — skipped {}",
                                            tool,
                                            p.display()
                                        );
                                        return Ok(false);
                                    }
                                    Err(e) => return Err(format!("{} failed to run: {}", tool, e)),
                                };
                                if !status.success() {
                                    return Err(format!("{} failed on {}", tool, p.display()));
                                }
                                let after =
                                    std::fs::read_to_string(p).map_err(|e| e.to_string())?;
                                let hash_after = jhol_bundle::fmt::content_hash(&after);
                                let changed = hash_after != hash_before;
                                if changed {
                                    cache_lock.lock().unwrap().record(&rel, &hash_after);
                                }
                                return Ok(changed);
                            }
                        }

                        // in-process engines
                        let src = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
                        let hash = jhol_bundle::fmt::content_hash(&src);
                        if !no_cache && cache_lock.lock().unwrap().is_fresh(&rel, &hash) {
                            return Ok(false); // same bytes already formatted
                        }
                        let formatted =
                            jhol_bundle::fmt::format_source(&src, &p.to_string_lossy(), &config)
                                .map_err(|e| e.to_string())?;
                        if formatted == src {
                            cache_lock.lock().unwrap().record(&rel, &hash);
                            return Ok(false);
                        }
                        if !check {
                            // atomic write: temp file + rename, so a crash
                            // never leaves half-formatted source.
                            let tmp = p.with_file_name(format!(
                                ".{}.jhol-tmp",
                                p.file_name().and_then(|n| n.to_str()).unwrap_or("fmt")
                            ));
                            std::fs::write(&tmp, &formatted).map_err(|e| e.to_string())?;
                            std::fs::rename(&tmp, p).map_err(|e| e.to_string())?;
                            // Record the FORMATTED bytes so a later run sees the
                            // file as already done. In --check mode the file is
                            // untouched, so recording here would poison the cache
                            // and make the next real run skip the file forever.
                            cache_lock
                                .lock()
                                .unwrap()
                                .record(&rel, &jhol_bundle::fmt::content_hash(&formatted));
                        }
                        Ok(true)
                    })();
                    (p.clone(), res)
                })
                .collect();
            drop(cache_lock);

            let mut changed = 0usize;
            let mut errors = 0usize;
            for (p, res) in &results {
                match res {
                    Ok(true) => {
                        changed += 1;
                        println!("{}", p.display());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        errors += 1;
                        eprintln!("{}: {}", p.display(), e);
                    }
                }
            }
            if !no_cache {
                let _ = cache.save(&cwd);
            }
            if errors > 0 {
                eprintln!("fmt: {} error(s)", errors);
                std::process::exit(2);
            }
            if check && changed > 0 {
                std::process::exit(1);
            }
            eprintln!(
                "fmt: {} file(s), {} changed, engine {}",
                files.len(),
                changed,
                jhol_bundle::fmt::FMT_ENGINE
            );
            return Ok(());
        }
        #[cfg(not(feature = "bundle"))]
        Some(("fmt", _sub_m)) => {
            return Err(
                "fmt requires the bundle feature (build with default features)".to_string(),
            );
        }
        Some(("build", sub_m)) => {
            #[cfg(feature = "bundle")]
            {
                let entries: Vec<std::path::PathBuf> = sub_m
                    .get_many::<String>("entry")
                    .map(|v| v.map(|s| std::path::PathBuf::from(s)).collect())
                    .unwrap_or_default();
                let outdir = std::path::PathBuf::from(
                    sub_m
                        .get_one::<String>("outdir")
                        .map(|s| s.as_str())
                        .unwrap_or("dist"),
                );
                let format = sub_m
                    .get_one::<String>("format")
                    .map(|s| s.as_str())
                    .unwrap_or("esm");
                let target = sub_m
                    .get_one::<String>("target")
                    .map(|s| s.as_str())
                    .unwrap_or("browser");
                let minify = flag(&sub_m, "minify");
                let watch = flag(&sub_m, "watch");
                let tree_shake = flag(&sub_m, "tree-shake");
                let code_splitting = flag(&sub_m, "code-splitting");

                let mut config = jhol_bundle::config::BundleConfig::default();
                config.entries = entries;
                config.out_dir = outdir;

                if let Some(f) = jhol_bundle::config::OutputFormat::from_str(format) {
                    config.format = f;
                }
                if let Some(t) = jhol_bundle::config::Target::from_str(target) {
                    config.target = t;
                }
                config.minify = minify;
                config.watch = watch;
                config.tree_shake = tree_shake;
                config.code_splitting = code_splitting;

                if watch {
                    jhol_bundle::bundle_watch(config).map_err(|e| e.to_string())?;
                } else {
                    let result = jhol_bundle::bundle(&config).map_err(|e| e.to_string())?;
                    success(&format!(
                        "✓ Built {} modules to {} ({} files, {:?})",
                        result.module_count,
                        config.out_dir.display(),
                        result.files.len(),
                        std::time::Duration::from_millis(result.total_time_ms),
                    ));
                }
            }
            #[cfg(not(feature = "bundle"))]
            {
                return Err("Jhol bundler not enabled. Rebuild with --features bundle".to_string());
            }
        }
        Some(("prefetch", sub_m)) => {
            let quiet = flag(&sub_m, "quiet");
            jhol_core::prefetch_from_lockfile(quiet).map_err(|e| e.to_string())?;
            if !quiet {
                success("Prefetch done. Run `jhol install --offline` to install from cache.");
            }
        }
        Some(("sbom", sub_m)) => {
            let format = match sub_m.get_one::<String>("format").map(|s| s.as_str()) {
                Some("simple") => jhol_core::SbomFormat::Simple,
                _ => jhol_core::SbomFormat::CycloneDx,
            };
            let json = jhol_core::generate_sbom(format).map_err(|e| e.to_string())?;
            if let Some(out_path) = sub_m.get_one::<String>("output") {
                fs::write(out_path, &json).map_err(|e| format!("Write failed: {}", e))?;
                success(&format!("SBOM written to {}.", out_path));
            } else {
                println!("{}", json);
            }
        }
        Some(("audit-jagr", sub_m)) => {
            if let Some(diff) = sub_m.get_one::<String>("diff") {
                // --diff OLD,NEW
                let mut parts = diff.splitn(2, ',');
                let old = parts.next().unwrap_or("").to_string();
                let new = parts.next().unwrap_or("").to_string();
                if old.is_empty() || new.is_empty() {
                    return Err(
                        "audit-jagr --diff requires two paths separated by a comma".to_string()
                    );
                }
                let old_path = std::path::PathBuf::from(&old);
                let new_path = std::path::PathBuf::from(&new);
                let old_report = jhol_core::jagr_load_audit_trail(&old_path)
                    .map_err(|e| format!("Load {}: {}", old, e))?;
                let new_report = jhol_core::jagr_load_audit_trail(&new_path)
                    .map_err(|e| format!("Load {}: {}", new, e))?;
                let diffs = jhol_core::jagr_diff_reports(&old_report, &new_report);
                if diffs.is_empty() {
                    success("Reports are equivalent (no semantic diff).");
                } else {
                    println!("JAGR audit diff ({} → {}):", old, new);
                    for d in diffs {
                        println!("  • {}", d);
                    }
                }
                return Ok(());
            }
            if let Some(load_path) = sub_m.get_one::<String>("load") {
                let p = std::path::PathBuf::from(load_path);
                let r = jhol_core::jagr_load_audit_trail(&p)
                    .map_err(|e| format!("Load {}: {}", load_path, e))?;
                println!("JAGR audit trail: {}", load_path);
                println!("  Manifest:        {}", r.manifest_path);
                println!("  Satisfiable:     {}", r.is_satisfiable);
                println!("  Strategy:        {}", r.strategy_used);
                println!("  β₁/β₂:           {}/{}", r.betti_1, r.betti_2);
                println!("  SNF satisfied:   {}", r.snf_satisfied);
                println!("  Packages:        {}", r.package_count);
                println!("  Selection size:  {}", r.selection.len());
                return Ok(());
            }
            if let Some(save_path) = sub_m.get_one::<String>("save") {
                let manifest = cwd.join(
                    sub_m
                        .get_one::<String>("manifest")
                        .cloned()
                        .unwrap_or_else(|| "package.json".to_string()),
                );
                let report = jhol_core::jagr_analyze_project_manifest(&manifest)
                    .map_err(|e| e.to_string())?;
                let p = std::path::PathBuf::from(save_path);
                jhol_core::jagr_save_audit_trail(&report, &p)
                    .map_err(|e| format!("Save failed: {}", e))?;
                success(&format!("JAGR audit trail saved to {}.", save_path));
                return Ok(());
            }
            return Err(
                "audit-jagr requires one of: --save <path>, --load <path>, --diff <old,new>"
                    .to_string(),
            );
        }
        Some(("betti-history", sub_m)) => {
            let path = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("file")
                    .cloned()
                    .unwrap_or_else(|| ".jagr_betti_history.jsonl".to_string()),
            );
            let entries = jhol_core::jagr_read_betti_history(&path);
            let last: Option<usize> = sub_m.get_one::<String>("last").and_then(|s| s.parse().ok());
            let to_show: &[jhol_core::BettiHistoryEntry] = match last {
                Some(n) => {
                    let start = entries.len().saturating_sub(n);
                    &entries[start..]
                }
                None => &entries[..],
            };
            if entries.is_empty() {
                info(&format!("No JAGR Betti history at {}", path.display()));
                info("Set JHOL_BETTI_HISTORY=1 to start recording.");
            } else {
                println!(
                    "JAGR Betti history ({} entries, file: {}):",
                    entries.len(),
                    path.display()
                );
                println!(
                    "  {:<28} {:<8} {:<8} {:<8} {:<8} {:<10} {:<10}",
                    "timestamp", "β₀", "β₁", "β₂", "snf", "strategy", "pkgs"
                );
                for e in to_show {
                    let ts_str = e.timestamp.to_string();
                    let ts = if ts_str.len() > 26 {
                        format!("{}…", &ts_str[..25])
                    } else {
                        ts_str
                    };
                    println!(
                        "  {:<28} {:<8} {:<8} {:<8} {:<8} {:<10} {:<10}",
                        ts,
                        e.b0,
                        e.b1,
                        e.b2,
                        e.snf_satisfied,
                        truncate(&e.strategy, 10),
                        e.package_count
                    );
                }
            }
        }
        Some(("jagr-impact", sub_m)) => {
            let manifest_path = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("manifest")
                    .cloned()
                    .unwrap_or_else(|| "package.json".to_string()),
            );
            let json_out = flag(&sub_m, "json");

            // Accept either --target PKG@VERSION or --package PKG --version VER.
            let (pkg, ver): (String, String) =
                if let Some(target) = sub_m.get_one::<String>("target") {
                    let (a, b) = target.split_once('@').ok_or_else(|| {
                        format!("--target requires PKG@VERSION, got '{}'", target)
                    })?;
                    (a.to_string(), b.to_string())
                } else {
                    let pkg = sub_m
                        .get_one::<String>("package")
                        .cloned()
                        .ok_or("--package PKG (or --target PKG@VERSION) is required")?;
                    let ver = sub_m
                        .get_one::<String>("version")
                        .cloned()
                        .ok_or("--version VER (or --target PKG@VERSION) is required")?;
                    (pkg, ver)
                };

            let entry = jhol_core::jagr_simulate_upgrade(&manifest_path, &pkg, &ver)
                .map_err(|e| e.to_string())?;

            if json_out {
                let j = serde_json::json!({
                    "package": entry.package,
                    "from_version": entry.from_version,
                    "to_version": entry.to_version,
                    "risk": entry.risk.as_str(),
                    "betti_1_before": entry.betti_1_before,
                    "betti_1_after": entry.betti_1_after,
                    "snf_satisfied_before": entry.snf_satisfied_before,
                    "snf_satisfied_after": entry.snf_satisfied_after,
                    "recommendation": entry.recommendation,
                });
                println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
            } else {
                if use_color() {
                    println!("{}", "JAGR Upgrade Impact".bright_cyan().bold());
                } else {
                    println!("JAGR Upgrade Impact");
                }
                println!("  Package:        {}", entry.package);
                println!("  From:           {}", entry.from_version);
                println!("  To:             {}", entry.to_version);
                println!(
                    "  Risk:           {} {}",
                    entry.risk.emoji(),
                    entry.risk.as_str()
                );
                println!(
                    "  β₁:             {} → {}",
                    entry.betti_1_before, entry.betti_1_after
                );
                println!(
                    "  SNF satisfied:  {} → {}",
                    entry.snf_satisfied_before, entry.snf_satisfied_after
                );
                println!("  Recommendation: {}", entry.recommendation);
            }
        }
        Some(("jagr", sub_m)) => {
            let manifest_path = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("manifest")
                    .cloned()
                    .unwrap_or_else(|| "package.json".to_string()),
            );
            let json_out = flag(&sub_m, "json");
            let quiet = flag(&sub_m, "quiet");
            let all_workspaces = flag(&sub_m, "all-workspaces");
            let graph_format = sub_m.get_one::<String>("graph").cloned();
            let output_path = sub_m.get_one::<String>("output").cloned();
            let show_config = flag(&sub_m, "show-config");

            if show_config {
                let cfg = jhol_core::load_jagr_config(&cwd);
                println!("JAGR config (from .jholrc + env, merged):");
                println!("  use_lp:           {:?}", cfg.use_lp);
                println!("  timeout_ms:       {:?}", cfg.timeout_ms);
                println!("  skip_layer3:      {:?}", cfg.skip_layer3);
                println!("  skip_layer2:      {:?}", cfg.skip_layer2);
                println!("  run_all_layers:   {:?}", cfg.run_all_layers);
                println!("  betti_history:    {:?}", cfg.betti_history);
                println!("  betti_history_dir:{:?}", cfg.betti_history_dir);
                println!("  max_versions:     {:?}", cfg.max_versions);
                println!("  strict:           {:?}", cfg.strict);
                return Ok(());
            }

            let lockfile_only = flag(&sub_m, "lockfile-only");

            // Read CLI-flag overrides for the per-project JAGR config.
            // These override the .jholrc values when set on the command line.
            let cli_skip_layer3 = flag(&sub_m, "skip-layer3");
            let cli_skip_layer2 = flag(&sub_m, "skip-layer2");
            let cli_run_all_layers = flag(&sub_m, "run-all-layers");

            // Collect the list of (cwd_relative_root, manifest_path) pairs to analyze.
            // Preserve the manifest's directory component when the user pointed
            // at a subdir manifest, not just the basename.
            let manifest_basename = manifest_path
                .file_name()
                .map(|s| s.to_os_string())
                .unwrap_or_else(|| std::ffi::OsString::from("package.json"));
            let manifest_dir = manifest_path
                .parent()
                .map(|p| p.to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let roots: Vec<(std::path::PathBuf, std::path::PathBuf)> = if all_workspaces {
                let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                if r.is_empty() {
                    vec![(cwd.clone(), manifest_path.clone())]
                } else {
                    r.into_iter()
                        .map(|p| {
                            let workspace_root = cwd.join(&p);
                            let candidate =
                                workspace_root.join(&manifest_dir).join(&manifest_basename);
                            (workspace_root, candidate)
                        })
                        .collect()
                }
            } else {
                vec![(cwd.clone(), manifest_path.clone())]
            };

            // Read all reports first (so we can also do graph output across all).
            let mut reports: Vec<(std::path::PathBuf, jhol_core::JagrAnalysisReport)> = Vec::new();
            for (root, manifest) in &roots {
                if !quiet && roots.len() > 1 {
                    info(&format!("Workspace: {}", root.display()));
                }
                // For each workspace, load the config from that workspace's root
                // directory so .jholrc settings are per-workspace. CLI flag
                // overrides (skip-layer2/3, run-all-layers) still apply globally.
                let jagr_cfg = jhol_core::load_jagr_config(root);
                let mut effective_cfg = jagr_cfg.clone();
                if cli_skip_layer3 {
                    effective_cfg.skip_layer3 = Some(true);
                }
                if cli_skip_layer2 {
                    effective_cfg.skip_layer2 = Some(true);
                }
                if cli_run_all_layers {
                    effective_cfg.run_all_layers = Some(true);
                }
                let resolver = effective_cfg.apply_to_resolver();
                let r = jhol_core::jagr_analyze_project_manifest_with_resolver(
                    manifest,
                    &resolver,
                    &effective_cfg,
                )
                .map_err(|e| e.to_string())?;
                reports.push((root.clone(), r));
            }

            // Graph export path: write the derivation graph of the first report
            // (or all of them concatenated if --all-workspaces).
            if let Some(fmt) = graph_format {
                let mut out = String::new();
                for (i, (root, r)) in reports.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&format!("\n# === Workspace: {} ===\n", root.display()));
                    }
                    match fmt.as_str() {
                        "dot" => {
                            out.push_str(&format!(
                                "# JAGR derivation graph for {}\n",
                                r.manifest_path
                            ));
                            out.push_str("digraph jagr_derivation {\n");
                            out.push_str("  rankdir=LR;\n");
                            out.push_str("  node [shape=box, style=\"rounded,filled\", fillcolor=\"#eef\"];\n");
                            for (idx, node) in
                                r.derivation_nodes_for_graph_export().iter().enumerate()
                            {
                                let label = node.replace('"', "\\\"");
                                let color = match idx {
                                    _ if idx == 0 => "fillcolor=\"#cfe\"", // root decision: green
                                    _ => "fillcolor=\"#eef\"",
                                };
                                out.push_str(&format!(
                                    "  n{} [label=\"{}\", {}];\n",
                                    idx, label, color
                                ));
                            }
                            for (i, _node) in
                                r.derivation_nodes_for_graph_export().iter().enumerate()
                            {
                                for j in 0..r.derivation_edge_count_at(i) {
                                    let _ = j; // count only; actual edges not in this struct
                                }
                            }
                            // Use the count to emit a synthetic label.
                            out.push_str(&format!(
                                "  edge_count_label [label=\"{} edges (see JSON --graph json for full)\", shape=plaintext];\n",
                                r.derivation_edge_count
                            ));
                            out.push_str("}\n");
                        }
                        "mermaid" => {
                            out.push_str(&format!(
                                "%% JAGR derivation graph for {}\n",
                                r.manifest_path
                            ));
                            out.push_str("graph LR\n");
                            for (idx, node) in
                                r.derivation_nodes_for_graph_export().iter().enumerate()
                            {
                                let label =
                                    node.replace('"', "'").replace('[', "(").replace(']', ")");
                                out.push_str(&format!("  n{}[\"{}\"]\n", idx, label));
                            }
                            out.push_str(&format!(
                                "  edges[\"{} edges\"]\n",
                                r.derivation_edge_count
                            ));
                        }
                        _ => {}
                    }
                }
                if let Some(p) = output_path {
                    fs::write(&p, &out).map_err(|e| format!("Write failed: {}", e))?;
                    success(&format!("Graph written to {}.", p));
                } else {
                    println!("{}", out);
                }
                return Ok(());
            }

            // For multi-workspace, prefer the summary view.
            if roots.len() > 1 {
                // Summary table
                if use_color() {
                    println!(
                        "{}",
                        "JAGR — Multi-Workspace Three-Layer Analysis"
                            .bright_cyan()
                            .bold()
                    );
                } else {
                    println!("JAGR — Multi-Workspace Three-Layer Analysis");
                }
                println!(
                    "{:<40} {:<10} {:<8} {:<8} {:<8} {:<12} {:<10}",
                    "workspace", "ok", "pkgs", "β₁", "β₂", "strategy", "time_ms"
                );
                for (root, r) in &reports {
                    let manifest_short = root
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| r.manifest_path.clone());
                    let t = r.layer_times_ms.get("total").copied().unwrap_or(0.0);
                    println!(
                        "{:<40} {:<10} {:<8} {:<8} {:<8} {:<12} {:<10.2}",
                        manifest_short,
                        if r.is_satisfiable { "yes" } else { "no" },
                        r.package_count,
                        r.betti_1,
                        r.betti_2,
                        truncate(&r.strategy_used, 12),
                        t,
                    );
                }
                if json_out {
                    // JSON array of all reports
                    let arr: Vec<serde_json::Value> = reports.iter().map(|(_root, r)| {
                        serde_json::json!({
                            "manifest": r.manifest_path,
                            "root": { "name": r.root_name, "version": r.root_version },
                            "package_count": r.package_count,
                            "is_satisfiable": r.is_satisfiable,
                            "cost": r.cost,
                            "strategy_used": r.strategy_used,
                            "layer_times_ms": r.layer_times_ms,
                            "layer2": { "b1": r.betti_1, "b2": r.betti_2, "snf_satisfied": r.snf_satisfied },
                            "selection_size": r.selection.len(),
                        })
                    }).collect();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
                    );
                }
                if let Some(p) = output_path {
                    let mut buf = String::new();
                    for (root, r) in &reports {
                        buf.push_str(&format!("# {}\n", root.display()));
                        buf.push_str(&format!("strategy: {}\n", r.strategy_used));
                        buf.push_str(&format!("satisfiable: {}\n", r.is_satisfiable));
                        buf.push_str(&format!("β₁={}, β₂={}\n", r.betti_1, r.betti_2));
                        buf.push_str(&format!(
                            "time_ms: {}\n",
                            r.layer_times_ms.get("total").copied().unwrap_or(0.0)
                        ));
                        buf.push('\n');
                    }
                    fs::write(&p, &buf).map_err(|e| format!("Write failed: {}", e))?;
                    success(&format!("Summary written to {}.", p));
                }
                return Ok(());
            }

            // Single-workspace path (original behavior).
            let (_root, report) = reports.into_iter().next().unwrap();
            let _ = sub_m; // keep for potential future use
            if json_out {
                // Serialize the JagrAnalysisReport to JSON for tooling.
                let j: serde_json::Value = serde_json::json!({
                    "manifest": report.manifest_path,
                    "root": { "name": report.root_name, "version": report.root_version },
                    "package_count": report.package_count,
                    "version_pair_count": report.version_pair_count,
                    "edge_count": report.edge_count,
                    "is_satisfiable": report.is_satisfiable,
                    "cost": report.cost,
                    "strategy_used": report.strategy_used,
                    "layer_times_ms": report.layer_times_ms,
                    "layer3": {
                        "satisfiable": report.layer3_satisfiable,
                        "kind": report.layer3_kind,
                    },
                    "layer2": {
                        "betti": { "b0": report.betti_0, "b1": report.betti_1, "b2": report.betti_2 },
                        "snf_satisfied": report.snf_satisfied,
                        "snf_trivial": report.snf_trivial,
                        "snf_difficulty": report.snf_difficulty,
                        "gluing_has_obstruction": report.gluing_has_obstruction,
                    },
                    "layer1": {
                        "cost": report.layer1_cost,
                        "satisfiable": report.layer1_satisfiable,
                        "variant": report.layer1_variant,
                    },
                    "derivation": {
                        "node_count": report.derivation_node_count,
                        "edge_count": report.derivation_edge_count,
                    },
                    "selection": report.selection,
                    "explanation": report.explanation,
                });
                let s = serde_json::to_string_pretty(&j).unwrap_or_else(|_| "{}".to_string());
                if let Some(p) = output_path {
                    fs::write(&p, &s).map_err(|e| format!("Write failed: {}", e))?;
                    success(&format!("JSON written to {}.", p));
                } else {
                    println!("{}", s);
                }
            } else if quiet {
                // Minimal output
                println!("JAGR strategy: {}", report.strategy_used);
                println!("Satisfiable:   {}", report.is_satisfiable);
                let mut names: Vec<&String> = report.selection.keys().collect();
                names.sort();
                for name in names {
                    let v = report.selection.get(name).cloned().unwrap_or_default();
                    println!("  {}@{}", name, v);
                }
            } else {
                // Rich, three-layer report.
                if use_color() {
                    println!(
                        "{}",
                        "JAGR — Three-Layer Resolver Analysis".bright_cyan().bold()
                    );
                } else {
                    println!("JAGR — Three-Layer Resolver Analysis");
                }
                println!("Manifest:        {}", report.manifest_path);
                println!(
                    "Root:            {}@{}",
                    report.root_name, report.root_version
                );
                println!(
                    "Packages:        {} ({} version-pairs, {} edges)",
                    report.package_count, report.version_pair_count, report.edge_count
                );
                println!();
                // Layer 3
                dim("── Layer 3: Tropical Geometry (PTIME filter) ──");
                println!(
                    "  Result:        {} ({})",
                    if report.layer3_satisfiable {
                        "satisfiable"
                    } else {
                        "unsatisfiable"
                    },
                    report.layer3_kind
                );
                if let Some(t) = report.layer_times_ms.get("layer3") {
                    println!("  Time:          {:.2} ms", t);
                }
                println!();
                // Layer 2
                dim("── Layer 2: Tropical Sheaf Cohomology (topological filter) ──");
                println!("  β₀:            {}", report.betti_0);
                println!("  β₁:            {}", report.betti_1);
                println!("  β₂:            {}", report.betti_2);
                println!("  SNF satisfied: {}", report.snf_satisfied);
                println!("  SNF trivial:   {}", report.snf_trivial);
                println!("  Difficulty:    {}", report.snf_difficulty);
                println!(
                    "  Gluing obstruction: {}",
                    if report.gluing_has_obstruction {
                        "present"
                    } else {
                        "absent"
                    }
                );
                if let Some(t) = report.layer_times_ms.get("layer2") {
                    println!("  Time:          {:.2} ms", t);
                }
                println!();
                // Layer 1
                dim("── Layer 1: Core-Guided MaxSAT (exact solver) ──");
                println!(
                    "  Invoked:       {}",
                    report.layer1_variant != "not_invoked"
                );
                println!("  Variant:       {}", report.layer1_variant);
                println!("  Cost:          {}", report.layer1_cost);
                println!("  Satisfiable:   {:?}", report.layer1_satisfiable);
                if let Some(t) = report.layer_times_ms.get("layer1") {
                    println!("  Time:          {:.2} ms", t);
                }
                println!();
                // Strategy and selection
                dim("── Strategy & Selection ──");
                println!("  Strategy:      {}", report.strategy_used);
                println!("  Satisfiable:   {}", report.is_satisfiable);
                println!("  Total cost:    {}", report.cost);
                if let Some(t) = report.layer_times_ms.get("total") {
                    println!("  Total time:    {:.2} ms", t);
                }
                println!();
                let mut names: Vec<&String> = report.selection.keys().collect();
                names.sort();
                println!("  Selected ({}):", names.len());
                for name in names {
                    let v = report.selection.get(name).cloned().unwrap_or_default();
                    println!("    {}@{}", name, v);
                }
                println!();
                // Derivation graph
                dim("── Derivation Graph (Conflict Explainer) ──");
                println!("  Nodes:         {}", report.derivation_node_count);
                println!("  Edges:         {}", report.derivation_edge_count);
                println!();
                if !report.explanation.is_empty() {
                    dim("── Explanation ──");
                    for line in report.explanation.lines() {
                        println!("  {}", line);
                    }
                }
                if let Some(p) = output_path {
                    // Write the same rich text to the file.
                    // (We don't re-render; we just record the manifest info so users
                    // know what was written.)
                    fs::write(&p, format!("# JAGR analysis for {}\n# See `jhol jagr --json {}` for machine-readable form\n",
                        report.manifest_path, report.manifest_path))
                        .map_err(|e| format!("Write failed: {}", e))?;
                }
            }

            // --explain-each: print per-package layer decisions
            if flag(&sub_m, "explain-each") {
                let explanations = report.selection_with_explanations();
                if use_color() {
                    println!(
                        "{}",
                        "JAGR per-package layer decisions:".bright_cyan().bold()
                    );
                } else {
                    println!("JAGR per-package layer decisions:");
                }
                if explanations.is_empty() {
                    info("No packages selected.");
                } else {
                    for (name, explanation) in &explanations {
                        println!("  {:<40} {}", name, explanation);
                    }
                }
            }

            // --compare-resolvers: run JAGR + pubgrub + sat and compare.
            if flag(&sub_m, "compare-resolvers") {
                let rows =
                    jhol_core::jagr_compare_resolvers(&manifest_path).map_err(|e| e.to_string())?;
                if use_color() {
                    println!(
                        "{}",
                        "JAGR vs Legacy Resolvers — Side-by-Side"
                            .bright_cyan()
                            .bold()
                    );
                } else {
                    println!("JAGR vs Legacy Resolvers — Side-by-Side");
                }
                println!(
                    "{:<12} {:<12} {:<10} {:<10}",
                    "resolver", "time_ms", "sat", "pkgs"
                );
                for row in &rows {
                    let time_ms = row.layer_times_ms.get("total").copied().unwrap_or(0.0);
                    println!(
                        "{:<12} {:<12.4} {:<10} {:<10}",
                        row.strategy_used,
                        time_ms,
                        if row.is_satisfiable { "yes" } else { "no" },
                        row.selection.len(),
                    );
                }
            }

            // --upgrade-plan: for each declared dep, simulate a candidate
            // version change and report the topological impact.
            if flag(&sub_m, "upgrade-plan") {
                let plan =
                    jhol_core::jagr_upgrade_plan(&manifest_path).map_err(|e| e.to_string())?;
                if use_color() {
                    println!("{}", "JAGR Topological Upgrade Plan".bright_cyan().bold());
                } else {
                    println!("JAGR Topological Upgrade Plan");
                }
                println!(
                    "{:<6} {:<30} {:<14} {:<14} {:<6} {:<6} {}",
                    "risk", "package", "from", "to", "β₁→β₁", "SNF", "recommendation"
                );
                for entry in &plan {
                    let risk_str = entry.risk.as_str();
                    let snf = if entry.snf_satisfied_before != entry.snf_satisfied_after {
                        format!(
                            "{}→{}",
                            entry.snf_satisfied_before, entry.snf_satisfied_after
                        )
                    } else {
                        entry.snf_satisfied_before.to_string()
                    };
                    println!(
                        "{:<6} {:<30} {:<14} {:<14} {:<6} {:<6} {}",
                        format!("{} {}", entry.risk.emoji(), risk_str),
                        truncate(&entry.package, 30),
                        truncate(&entry.from_version, 14),
                        truncate(&entry.to_version, 14),
                        format!("{}→{}", entry.betti_1_before, entry.betti_1_after),
                        snf,
                        entry.recommendation,
                    );
                }
                // Summary footer
                let n_stable = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::SNFStable)
                    .count();
                let n_warn = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::TopologyChanged)
                    .count();
                let n_broken = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::SNFBroken)
                    .count();
                let n_unsat = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::Unsatisfiable)
                    .count();
                let n_skip = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::Skipped)
                    .count();
                println!("\nSummary: {} SNF-stable, {} topology-changed, {} SNF-broken, {} UNSAT, {} skipped ({} total)",
                    n_stable, n_warn, n_broken, n_unsat, n_skip, plan.len());
            }

            // --dry-run-upgrade <pkg>@<version>: simulate a single upgrade
            // and report the JAGR topological impact.
            if let Some(target) = sub_m.get_one::<String>("dry-run-upgrade") {
                let (pkg, ver) = target.split_once('@').ok_or_else(|| {
                    format!("--dry-run-upgrade requires PKG@VERSION, got '{}'", target)
                })?;
                let entry = jhol_core::jagr_simulate_upgrade(&manifest_path, pkg, ver)
                    .map_err(|e| e.to_string())?;
                if use_color() {
                    println!("{}", "JAGR Dry-Run Upgrade Impact".bright_cyan().bold());
                } else {
                    println!("JAGR Dry-Run Upgrade Impact");
                }
                println!("  Package:        {}", entry.package);
                println!("  From:           {}", entry.from_version);
                println!("  To:             {}", entry.to_version);
                println!(
                    "  Risk:           {} {}",
                    entry.risk.emoji(),
                    entry.risk.as_str()
                );
                println!(
                    "  β₁:             {} → {}",
                    entry.betti_1_before, entry.betti_1_after
                );
                println!(
                    "  SNF satisfied:  {} → {}",
                    entry.snf_satisfied_before, entry.snf_satisfied_after
                );
                println!("  Recommendation: {}", entry.recommendation);
            }

            // --lockfile-only: resolve with JAGR, then write package-lock.json
            // without installing anything. The lockfile is written alongside
            // the manifest that was analyzed, NOT hard-coded to cwd.
            if lockfile_only {
                if !report.is_satisfiable {
                    return Err(format!(
                        "JAGR found no satisfying assignment; lockfile cannot be written.\n{}",
                        report.explanation
                    ));
                }
                if use_color() {
                    println!(
                        "{}",
                        "JAGR lockfile-only: resolving manifest and writing package-lock.json..."
                            .bright_cyan()
                    );
                } else {
                    println!(
                        "JAGR lockfile-only: resolving manifest and writing package-lock.json..."
                    );
                }
                // Resolve `manifest_path` to an absolute path (it can be
                // relative to cwd as passed via --manifest).
                let pj = if manifest_path.is_absolute() {
                    manifest_path.clone()
                } else {
                    cwd.join(&manifest_path)
                };
                let lock_path = pj.with_file_name("package-lock.json");
                let tree = jhol_core::resolve_full_tree(&pj)
                    .map_err(|e| format!("JAGR lockfile write failed: {}", e))?;
                jhol_core::write_package_lock(&lock_path, &pj, &tree)
                    .map_err(|e| format!("Write lockfile failed: {}", e))?;
                success(&format!(
                    "JAGR: wrote {} ({} packages resolved in {:.2}ms)",
                    lock_path.display(),
                    tree.len(),
                    report.layer_times_ms.get("total").copied().unwrap_or(0.0)
                ));
                return Ok(());
            }
        }
        _ => {
            if use_color() {
                println!("{}", "jhol".bright_cyan().bold());
                dim("Fast, offline-friendly package manager — cache first, Bun/npm backend.");
            } else {
                println!("jhol — Fast, offline-friendly package manager");
            }
            dim("\nRun `jhol --help` for details.");
        }
    }

    Ok(())
}

/// Record `jhol add` targets in package.json **before** installing.
///
/// Writing the manifest first is what makes the install actually run: the install fast
/// paths compare package.json's hash, so a second `add` used to see a "fresh"
/// node_modules, skip the work, and still print "Package(s) added." while installing
/// nothing and recording nothing. Bare names are stored as `*` here and refined to
/// `^<installed>` afterwards, which is what npm does.
/// Which package.json section `add`/`install --save` writes into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveTarget {
    Dependencies,
    Dev,
    Peer,
    Optional,
}

impl SaveTarget {
    fn section(self) -> &'static str {
        match self {
            SaveTarget::Dependencies => "dependencies",
            SaveTarget::Dev => "devDependencies",
            SaveTarget::Peer => "peerDependencies",
            SaveTarget::Optional => "optionalDependencies",
        }
    }
}


/// The version of `name` as installed in node_modules, if present.
fn installed_version(name: &str) -> Option<String> {
    let pj = std::path::Path::new("node_modules").join(name).join("package.json");
    let text = fs::read_to_string(pj).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

fn persist_added_deps(packages: &[&str]) -> Result<(), String> {
    persist_added_deps_opts(packages, SaveTarget::Dependencies, false)
}

fn persist_added_deps_opts(
    packages: &[&str],
    target: SaveTarget,
    exact: bool,
) -> Result<(), String> {
    let path = std::path::Path::new("package.json");
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            let cwd_name = std::env::current_dir()
                .ok()
                .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| "project".to_string());
            let default_pj = serde_json::json!({
                "name": cwd_name,
                "version": "1.0.0",
                "dependencies": {}
            });
            serde_json::to_string_pretty(&default_pj).unwrap_or_default()
        }
    };
    let mut json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid package.json: {}", e))?;
    let root = json
        .as_object_mut()
        .ok_or("package.json must be a JSON object")?;
    let deps_value = root
        .entry(target.section())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let deps = deps_value
        .as_object_mut()
        .ok_or("dependencies must be an object")?;
    for spec in packages {
        let (name, version) = if let Some(stripped) = spec.strip_prefix('@') {
            if let Some((scope_pkg, ver)) = stripped.split_once('@') {
                (format!("@{}", scope_pkg), ver.to_string())
            } else {
                ((*spec).to_string(), "*".to_string())
            }
        } else if let Some((n, v)) = spec.split_once('@') {
            (n.to_string(), v.to_string())
        } else {
            ((*spec).to_string(), "*".to_string())
        };
        let value = if exact {
            // `--save-exact`: pin the resolved version, which is what npm
            // writes. The requested range is only the request.
            installed_version(&name).unwrap_or(version)
        } else {
            version
        };
        deps.insert(name, serde_json::Value::String(value));
    }
    let pretty = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    fs::write(path, format!("{}\n", pretty))
        .map_err(|e| format!("cannot write package.json: {}", e))
}

/// Turn `*` specs (a bare `jhol add name` or `jhol install name`) into `^<installed version>`, like npm.
fn refine_star_specs(packages: &[&str]) {
    let path = std::path::Path::new("package.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let mut changed = false;
    for spec in packages {
        let (name, has_explicit_version) = if let Some(stripped) = spec.strip_prefix('@') {
            if let Some((scope_pkg, _ver)) = stripped.split_once('@') {
                (format!("@{}", scope_pkg), true)
            } else {
                ((*spec).to_string(), false)
            }
        } else if let Some((pkg, _ver)) = spec.split_once('@') {
            (pkg.to_string(), true)
        } else {
            ((*spec).to_string(), false)
        };
        if has_explicit_version {
            continue;
        }
        let manifest = std::path::Path::new("node_modules")
            .join(&name)
            .join("package.json");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(version) = meta.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        // Refine whichever section the package was saved into: `-D`,
        // `--save-peer` and `-O` write their own sections, and refining only
        // `dependencies` left those at "*".
        let section = ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"]
            .into_iter()
            .find(|s| {
                json.get(*s)
                    .and_then(|d| d.as_object())
                    .map(|d| d.contains_key(&name))
                    .unwrap_or(false)
            })
            .unwrap_or("dependencies");
        if let Some(deps) = json.get_mut(section).and_then(|d| d.as_object_mut()) {
            if let Some(existing) = deps.get(&name).and_then(|v| v.as_str()) {
                if existing == "*" || existing.is_empty() {
                    deps.insert(name, serde_json::Value::String(format!("^{}", version)));
                    changed = true;
                }
            }
        }
    }
    if changed {
        if let Ok(pretty) = serde_json::to_string_pretty(&json) {
            let _ = fs::write(path, format!("{}\n", pretty));
        }
    }
}

/// The one CLI definition. Dispatch and shell-completion generation both use it,
/// so a flag cannot be declared for `--help` and missing from parsing (or the
/// reverse) — a drift that cost five separate fixes, including a 100 % `ci` crash.
fn cli_command() -> Command {
    let backend_arg = Arg::new("backend")
        .long("backend")
        .value_parser(["bun", "npm"])
        .help("Package manager backend (default: bun if available, else npm)");

    Command::new("jhol")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Bhuvan Prakash <bhuvanstark6@gmail.com>")
        .about("Fast, offline-friendly package manager — cache first, Bun/npm backend")
        .after_help(
            "Examples:\n  jhol install lodash\n  jhol install react react-dom\n  jhol install\n  jhol doctor --fix\n  jhol cache list",
        )
        .subcommand(
            Command::new("install")
                .about("Install packages (from args or package.json)")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .num_args(0..)
                        .help("Package(s) to install; omit to install from package.json"),
                )
                .arg(
                    Arg::new("save")
                        .long("save")
                        .action(ArgAction::SetTrue)
                        .help("Save installed packages to dependencies (default when packages specified)"),
                )
                .arg(
                    Arg::new("no-save")
                        .long("no-save")
                        .action(ArgAction::SetTrue)
                        .help("Do not save installed packages to package.json"),
                )
                .arg(
                    Arg::new("save-dev")
                        .short('D')
                        .long("save-dev")
                        .action(ArgAction::SetTrue)
                        .help("Save added packages to devDependencies"),
                )
                .arg(
                    Arg::new("save-peer")
                        .long("save-peer")
                        .action(ArgAction::SetTrue)
                        .help("Save added packages to peerDependencies"),
                )
                .arg(
                    Arg::new("save-optional")
                        .short('O')
                        .long("save-optional")
                        .action(ArgAction::SetTrue)
                        .help("Save added packages to optionalDependencies"),
                )
                .arg(
                    Arg::new("save-exact")
                        .short('E')
                        .long("save-exact")
                        .action(ArgAction::SetTrue)
                        .help("Save the exact installed version instead of a range"),
                )
                .arg(
                    Arg::new("no-cache")
                        .long("no-cache")
                        .action(ArgAction::SetTrue)
                        .help("Ignore cache and fetch from registry"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output; show spinner when busy"),
                )
                .arg(backend_arg.clone())
                .arg(
                    Arg::new("lockfile-only")
                        .long("lockfile-only")
                        .action(ArgAction::SetTrue)
                        .help("Only update lockfile, do not install to node_modules"),
                )
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue)
                        .help("Only use cache; fail if any package is missing (or set JHOL_OFFLINE=1)"),
                )
                .arg(
                    Arg::new("frozen")
                        .long("frozen-lockfile")
                        .visible_alias("frozen")
                        .action(ArgAction::SetTrue)
                        .help("Require lockfile and fail if out of sync with package.json"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages (from package.json workspaces)"),
                )
                .arg(
                    Arg::new("filter")
                        .long("filter")
                        .short('F')
                        .value_name("PATTERN")
                        .action(ArgAction::Append)
                        .help("Restrict a workspace run to packages matching NAME or @scope/* (repeatable)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output machine-readable JSON result"),
                )
                .arg(
                    Arg::new("native-only")
                        .long("native-only")
                        .action(ArgAction::SetTrue)
                        .help("Never use Bun/npm; fail if native install fails (default)"),
                )
                .arg(
                    Arg::new("omit")
                        .long("omit")
                        .value_name("CLASS[,...]")
                        .help("Leave out dependency classes: dev, optional, peer"),
                )
                .arg(
                    Arg::new("prod")
                        .long("prod")
                        .alias("production")
                        .action(ArgAction::SetTrue)
                        .help("Shorthand for --omit=dev (runtime dependencies only)"),
                )
                .arg(
                    Arg::new("include-dev")
                        .long("include-dev")
                        .action(ArgAction::SetTrue)
                        .help("Install devDependencies even when NODE_ENV=production"),
                )
                .arg(
                    Arg::new("engine-strict")
                        .long("engine-strict")
                        .action(ArgAction::SetTrue)
                        .help("Fail when a package's engines.node excludes this runtime"),
                )
                .arg(
                    Arg::new("as-of")
                        .long("as-of")
                        .value_name("DATE")
                        .help("Resolve as of a date (YYYY-MM-DD or RFC3339): only versions published by then are eligible"),
                )
                .arg(
                    Arg::new("allow-license")
                        .long("allow-license")
                        .value_name("LIST")
                        .help("Only resolve versions under these licenses (comma separated, e.g. MIT,Apache-2.0)"),
                )
                .arg(
                    Arg::new("license-allow-unknown")
                        .long("license-allow-unknown")
                        .action(ArgAction::SetTrue)
                        .help("With --allow-license, also allow versions that declare no license"),
                )
                .arg(
                    Arg::new("exclude-vuln")
                        .long("exclude-vuln")
                        .action(ArgAction::SetTrue)
                        .help("Do not resolve versions with known advisories (queries OSV, then re-resolves)"),
                )
                .arg(
                    Arg::new("global")
                        .short('g')
                        .long("global")
                        .action(ArgAction::SetTrue)
                        .help("Install machine-wide into the jhol global prefix, shimming bin entries"),
                )
                .arg(
                    Arg::new("fallback-backend")
                        .long("fallback-backend")
                        .action(ArgAction::SetTrue)
                        .help("On failure, fall back to Bun/npm for install"),
                )
                .arg(
                    Arg::new("no-scripts")
                        .long("no-scripts")
                        .action(ArgAction::SetTrue)
                        .help("Do not run lifecycle scripts in fallback backend installs (default true)"),
                )
                .arg(
                    Arg::new("scripts")
                        .long("scripts")
                        .action(ArgAction::SetTrue)
                        .help("Allow lifecycle scripts in fallback backend installs"),
                )
                .arg(
                    // Documented (AGENTS.md) and supported by the core
                    // (`InstallOptions.script_allowlist`, `JHOL_SCRIPT_ALLOWLIST`),
                    // but there was no CLI flag — so a user had no way to opt a
                    // specific package into lifecycle scripts for one install, only
                    // the env var or `.jholrc`. Scripts stay off by default; this is
                    // the per-invocation escape hatch.
                    Arg::new("script-allowlist")
                        .long("script-allowlist")
                        .value_name("PKG[,PKG...]")
                        .action(ArgAction::Append)
                        .help("Run lifecycle scripts for these packages (comma-separated; repeatable). Scripts stay opt-in."),
                )
                .arg(
                    Arg::new("resolver")
                        .long("resolver")
                        .value_parser(["pubgrub-v2", "pubgrub", "jagr", "jagr3", "jagr-infinite", "legacy"])
                        .help("Resolver name, kept as a compatibility alias: every value routes to JAGR-Infinite, the only primary resolver (a non-default value logs a note). If JAGR-Infinite fails, a PubGrub-named rescue runs only for manifests with <=25 direct dependencies"),
                )
                .arg(
                    Arg::new("strict-peer-deps")
                        .long("strict-peer-deps")
                        .action(ArgAction::SetTrue)
                        .help("Fail install when root peer dependency constraints conflict"),
                )
                .arg(
                    Arg::new("daemon")
                        .long("daemon")
                        .action(ArgAction::SetTrue)
                        .help("Send install request to running jhol daemon (Unix)"),
                )
                .arg(
                    Arg::new("fetch-timeout")
                        .long("fetch-timeout")
                        .value_parser(clap::value_parser!(u64))
                        .help("Timeout in seconds for individual package fetch operations"),
                )
                .arg(
                    Arg::new("resolve-timeout")
                        .long("resolve-timeout")
                        .value_parser(clap::value_parser!(u64))
                        .help("Timeout in seconds for dependency resolution"),
                )
                .arg(
                    Arg::new("install-timeout")
                        .long("install-timeout")
                        .value_parser(clap::value_parser!(u64))
                        .help("Timeout in seconds for the entire install operation"),
                )
                .arg(
                    Arg::new("prefer")
                        .long("prefer")
                        .value_parser(["minimal", "optimize", "latest"])
                        .help("Prefer minimal versions (minimal), optimal deduplication (optimize), or latest (default)"),
                ),
        )
        .subcommand(
            Command::new("ci")
                .about("Clean, deterministic install (npm ci-style): requires lockfile")
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue)
                        .help("Only use cache; fail if any package is missing"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("filter")
                        .long("filter")
                        .short('F')
                        .value_name("PATTERN")
                        .action(ArgAction::Append)
                        .help("Restrict a workspace run to packages matching NAME or @scope/* (repeatable)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output machine-readable JSON result"),
                )
                .arg(
                    Arg::new("jagr-check")
                        .long("jagr-check")
                        .action(ArgAction::SetTrue)
                        .help("After ci succeeds, run JAGR three-layer analysis on the manifest to confirm satisfiability"),
                )
                .arg(backend_arg.clone())
                .arg(
                    Arg::new("scripts")
                        .long("scripts")
                        .action(ArgAction::SetTrue)
                        .help("Run lifecycle scripts for allowlisted packages (off by default)"),
                )
                .arg(
                    Arg::new("no-scripts")
                        .long("no-scripts")
                        .action(ArgAction::SetTrue)
                        .help("Never run lifecycle scripts (the default)"),
                )
                .arg(
                    Arg::new("script-allowlist")
                        .long("script-allowlist")
                        .value_name("PKG[,PKG...]")
                        .action(ArgAction::Append)
                        .help("Allow lifecycle scripts for these packages (repeatable, comma-separated)"),
                )
                .arg(
                    Arg::new("strict-peer-deps")
                        .long("strict-peer-deps")
                        .action(ArgAction::SetTrue)
                        .help("Fail on a peer-dependency conflict instead of warning"),
                ),
        )
        .subcommand(
            Command::new("doctor")
                .about("Check and fix outdated dependencies")
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .action(ArgAction::SetTrue)
                        .help("Update outdated packages"),
                )
                .arg(
                    Arg::new("explain")
                        .long("explain")
                        .action(ArgAction::SetTrue)
                        .help("Explain lockfile/health/fallback diagnostics for this project"),
                )
                .arg(
                    Arg::new("jagr")
                        .long("jagr")
                        .action(ArgAction::SetTrue)
                        .help("Run JAGR three-layer resolver health check on this project"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                )
                .arg(backend_arg.clone())
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output machine-readable JSON"),
                ),
        )
        .subcommand(
            Command::new("import-lock")
                .about("Import/convert lockfile formats (bun.lock, yarn.lock, pnpm-lock.yaml -> package-lock.json)")
                .arg(
                    Arg::new("from")
                        .long("from")
                        .value_parser(["auto", "bun", "npm", "yarn", "pnpm"])
                        .default_value("auto")
                        .help("Source lockfile format (auto, bun, npm, yarn, pnpm)"),
                ),
        )
        .subcommand(
            Command::new("global-install")
                .about("Install jhol binary to PATH (e.g. /usr/local/bin)"),
        )
        .subcommand(
            Command::new("daemon")
                .about("Run persistent install daemon (Unix)")
                .arg(
                    Arg::new("serve")
                        .long("serve")
                        .action(ArgAction::SetTrue)
                        .help("Run daemon server loop"),
                )
                .arg(
                    Arg::new("stop")
                        .long("stop")
                        .action(ArgAction::SetTrue)
                        .help("Stop the daemon after accepted requests finish"),
                ),
        )
        .subcommand(
            Command::new("cache")
                .about("Manage local package cache")
                .subcommand(Command::new("list").about("List cached packages"))
                .subcommand(Command::new("size").about("Show cache size and tarball count"))
                .subcommand(
                    Command::new("prune")
                        .about("Remove unreferenced tarballs; optionally keep only N most recent")
                        .arg(
                            Arg::new("keep")
                                .long("keep")
                                .value_parser(clap::value_parser!(usize))
                                .help("Keep only N most recently used tarballs"),
                        ),
                )
                .subcommand(
                    Command::new("gc")
                        .about("Sweep store tarballs not referenced by any given lockfile")
                        .arg(
                            Arg::new("from")
                                .long("from")
                                .num_args(1..)
                                .value_name("LOCKFILE")
                                .help("Lockfiles that mark reachability (default: ./package-lock.json)"),
                        )
                        .arg(
                            Arg::new("dry-run")
                                .long("dry-run")
                                .num_args(0)
                                .help("Report what would be freed without deleting"),
                        ),
                )
                .subcommand(
                    Command::new("export")
                        .about("Export project deps from cache to directory (for offline)")
                        .arg(Arg::new("dir").required(true).help("Output directory")),
                )
                .subcommand(
                    Command::new("import")
                        .about("Import tarballs from directory into cache")
                        .arg(Arg::new("dir").required(true).help("Directory from jhol cache export")),
                )
                .subcommand(Command::new("clean").about("Remove all cached tarballs"))
                .subcommand(
                    Command::new("telemetry").about("Show native fallback telemetry summary"),
                )
                .subcommand(
                    Command::new("key")
                        .about("Print lockfile hash for CI cache key (same lockfile => same key)"),
                )
                .subcommand(
                    Command::new("mirror")
                        .about("Show JMR resolution mirror stats (entries, coverage, hits)")
                        .subcommand(Command::new("stats").about("Show mirror statistics"))
                        .subcommand(Command::new("prune").about("Remove expired entries from mirror"))
                        .subcommand(Command::new("clear").about("Clear all mirror entries"))
                        .subcommand(Command::new("lookup")
                            .about("Look up a package in the mirror")
                            .arg(Arg::new("package").required(true).help("Package name to look up (@scope/pkg@^range)"))
                        ),
                ),
        )
        .subcommand(
            Command::new("audit")
                .about("Check for known vulnerabilities")
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .action(ArgAction::SetTrue)
                        .help("Apply fixes where possible"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                )
                .arg(backend_arg.clone())
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output raw audit JSON"),
                )
                .arg(
                    Arg::new("gate")
                        .long("gate")
                        .action(ArgAction::SetTrue)
                        .help("Fail with non-zero exit if vulnerabilities are found (CI gate)"),
                ),
        )
        .subcommand(
            Command::new("prefetch")
                .about("Download lockfile dependencies into cache (no node_modules). Use before install --offline.")
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                ),
        )
        .subcommand(
            Command::new("run")
                .about("Run a script from package.json or a JS/TS file with Jhol runtime")
                .arg(
                    Arg::new("script")
                        .required(true)
                        .help("Script name from package.json, or JS/TS file path"),
                )
                .arg(
                    Arg::new("file")
                        .long("file")
                        .short('f')
                        .action(ArgAction::SetTrue)
                        .help("Treat argument as a JS/TS file path (run with Jhol runtime)"),
                )
                .arg(
                    Arg::new("watch")
                        .long("watch")
                        .short('w')
                        .action(ArgAction::SetTrue)
                        .help("Watch mode: restart on file changes"),
                )
                .arg(
                    Arg::new("engine")
                        .long("engine")
                        .default_value("auto")
                        .value_parser(["auto", "quickjs", "js-engine"])
                        .help("Select script engine"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                ),
        )
        .subcommand(
            Command::new("eval")
                .about("Evaluate JavaScript/TypeScript code inline")
                .arg(
                    Arg::new("code")
                        .required(true)
                        .help("JavaScript/TypeScript code to evaluate"),
                )
                .arg(
                    Arg::new("type")
                        .long("type")
                        .value_parser(["js", "ts", "jsx", "tsx"])
                        .default_value("js")
                        .help("File type for inline evaluation"),
                )
                .arg(
                    Arg::new("print")
                        .long("print")
                        .short('p')
                        .action(ArgAction::SetTrue)
                        .help("Print the value of the final expression (like node -p)"),
                ),
        )
        .subcommand(
            Command::new("build")
                .about("Bundle JavaScript/TypeScript/CSS for browser, Node.js, or Jhol")
                .arg(
                    Arg::new("entry")
                        .required(true)
                        .num_args(1..)
                        .help("Entry file(s) to bundle"),
                )
                .arg(
                    Arg::new("outdir")
                        .long("outdir")
                        .short('o')
                        .default_value("dist")
                        .help("Output directory"),
                )
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["esm", "cjs", "iife"])
                        .default_value("esm")
                        .help("Output format"),
                )
                .arg(
                    Arg::new("target")
                        .long("target")
                        .value_parser(["browser", "node", "jhol"])
                        .default_value("browser")
                        .help("Target platform"),
                )
                .arg(
                    Arg::new("minify")
                        .long("minify")
                        .short('m')
                        .action(ArgAction::SetTrue)
                        .help("Minify output"),
                )
                .arg(
                    Arg::new("watch")
                        .long("watch")
                        .short('w')
                        .action(ArgAction::SetTrue)
                        .help("Watch mode: rebuild on file changes"),
                )
                .arg(
                    Arg::new("sourcemap")
                        .long("sourcemap")
                        .value_parser(["none", "inline", "external", "both"])
                        .default_value("none")
                        .help("Source map generation"),
                )
                .arg(
                    Arg::new("external")
                        .long("external")
                        .num_args(0..)
                        .help("Modules to mark as external"),
                )
                .arg(
                    Arg::new("tree-shake")
                        .long("tree-shake")
                        .action(ArgAction::SetTrue)
                        .help("Enable tree shaking"),
                )
                .arg(
                    Arg::new("code-splitting")
                        .long("code-splitting")
                        .action(ArgAction::SetTrue)
                        .help("Enable code splitting"),
                ),
        )
        .subcommand(
            Command::new("fmt")
                .about("Format files in place (native Rust, cache-first; JS/TS/JSON/CSS + rustfmt/gofmt/ruff)")
                .arg(Arg::new("path").num_args(0..).help("Files or directories to format"))
                .arg(Arg::new("check").long("check").action(ArgAction::SetTrue).help("Check, write nothing (exit 1 if changes needed)"))
                .arg(Arg::new("list-different").long("list-different").action(ArgAction::SetTrue).help("Like --check, print only file names"))
                .arg(Arg::new("stdin-filepath").long("stdin-filepath").help("Format stdin as this file path and print"))
                .arg(Arg::new("init").long("init").action(ArgAction::SetTrue).help("Write .prettierrc.json defaults"))
                .arg(Arg::new("no-cache").long("no-cache").action(ArgAction::SetTrue).help("Bypass the format cache"))
                .arg(Arg::new("print-width").long("print-width").help("Wrap width (default 80)"))
                .arg(Arg::new("tab-width").long("tab-width").help("Spaces per indent level (default 2)"))
                .arg(Arg::new("use-tabs").long("use-tabs").action(ArgAction::SetTrue).help("Indent with tabs"))
                .arg(Arg::new("no-semi").long("no-semi").action(ArgAction::SetTrue).help("Omit semicolons where possible"))
                .arg(Arg::new("single-quote").long("single-quote").action(ArgAction::SetTrue).help("Use single quotes"))
                .arg(Arg::new("trailing-comma").long("trailing-comma").value_parser(["all", "es5", "none"]).help("Trailing commas (default all)"))
        )
        .subcommand(
            Command::new("exec")
                .visible_alias("x")
                .about("Run a binary from node_modules/.bin (no npx)")
                .arg(
                    Arg::new("binary")
                        .required(true)
                        .help("Binary name (e.g. eslint, tsc)"),
                )
                .arg(
                    Arg::new("args")
                        .num_args(0..)
                        // Everything after the binary name belongs to that binary:
                        // without these, jhol parsed the flags itself and
                        // `jhol exec tsc --version` failed with "unexpected argument".
                        .allow_hyphen_values(true)
                        .trailing_var_arg(true)
                        .help("Arguments to pass to the binary (flags included)"),
                ),
        )
        .subcommand(
            Command::new("cdn")
                .about("Print esm.sh URL for a package (optional: fetch to file)")
                .arg(
                    Arg::new("package")
                        .required(true)
                        .help("Package spec (e.g. lodash@4 or lodash)"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .help("Fetch ESM bundle to this file"),
                ),
        )
        .subcommand(
            Command::new("plan")
                .about("Inspect manifests and print a read-only environment/task plan (no installs)")
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Machine-readable JSON output"),
                )
                .arg(
                    Arg::new("path")
                        .long("path")
                        .help("Project root (default: current directory)"),
                ),
        )
        .subcommand(
            Command::new("diff")
                .about("Compare project state with a snapshot (default: latest), or two receipt JSON files")
                .arg(
                    Arg::new("before")
                        .required(false)
                        .help("Snapshot id, or an earlier receipt JSON path"),
                )
                .arg(
                    Arg::new("after")
                        .required(false)
                        .help("Later receipt JSON path (two-receipt mode)"),
                )
                .arg(
                    Arg::new("path")
                        .long("path")
                        .help("Project root (default: current directory)"),
                )
                .arg(
                    Arg::new("exit-code")
                        .long("exit-code")
                        .action(ArgAction::SetTrue)
                        .help("Exit 1 when `jhol sync --apply` is needed (for CI)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Emit DiffReport JSON"),
                ),
        )
        .subcommand(
            Command::new("sync")
                .about("Record native lock digests; optionally apply native ecosystem adapters")
                .arg(
                    Arg::new("path")
                        .long("path")
                        .help("Project root (default: current directory)"),
                )
                .arg(
                    Arg::new("apply")
                        .long("apply")
                        .action(ArgAction::SetTrue)
                        .help("Also run jhol install when package.json present (needs LocalPackageInstall)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Machine-readable JSON"),
                )
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue)
                        .help("Forbid network fetches in every adapter"),
                )
                .arg(
                    Arg::new("allow-build-scripts")
                        .long("allow-build-scripts")
                        .action(ArgAction::SetTrue)
                        .help("Approve build backends, Cargo build scripts, and procedural macros"),
                ),
        )
        .subcommand(
            Command::new("snapshot")
                .about("Create, list, or show portable project snapshots")
                .arg(
                    Arg::new("action")
                        .required(true)
                        .value_parser(["create", "list", "show"])
                        .help("create | list | show"),
                )
                .arg(Arg::new("id").help("Snapshot id (for show)"))
                .arg(
                    Arg::new("path")
                        .long("path")
                        .help("Project root (default: current directory)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("JSON output"),
                ),
        )
        .subcommand(
            Command::new("restore")
                .about("Restore jhol.project.lock from a snapshot (never auto-restores DBs)")
                .arg(Arg::new("id").required(true).help("Snapshot id"))
                .arg(
                    Arg::new("plan")
                        .long("plan")
                        .action(ArgAction::SetTrue)
                        .help("Show restore plan only"),
                )
                .arg(
                    Arg::new("apply")
                        .long("apply")
                        .action(ArgAction::SetTrue)
                        .help("Write jhol.project.lock from snapshot"),
                )
                .arg(
                    Arg::new("path")
                        .long("path")
                        .help("Project root (default: current directory)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("JSON output"),
                )
                .arg(
                    Arg::new("databases")
                        .long("databases")
                        .action(ArgAction::SetTrue)
                        .help("Request DB restore (always refused — fail closed)"),
                ),
        )
        .subcommand(
            Command::new("db")
                .about("Migration plan on disk + optional DATABASE_URL TCP probe")
                .subcommand(
                    Command::new("plan")
                        .about("Hash migrations/*.sql on disk (no live DB)")
                        .arg(
                            Arg::new("migrations")
                                .long("migrations")
                                .default_value("migrations")
                                .help("Migrations directory relative to project"),
                        )
                        .arg(
                            Arg::new("path")
                                .long("path")
                                .help("Project root"),
                        )
                        .arg(
                            Arg::new("json")
                                .long("json")
                                .action(ArgAction::SetTrue),
                        ),
                )
                .subcommand(
                    Command::new("probe")
                        .about("Optional TCP probe of DATABASE_URL (no SQL, no migrate)")
                        .arg(
                            Arg::new("timeout-ms")
                                .long("timeout-ms")
                                .default_value("500")
                                .help("Connect timeout in milliseconds"),
                        )
                        .arg(
                            Arg::new("json")
                                .long("json")
                                .action(ArgAction::SetTrue),
                        ),
                ),
        )
        .subcommand(
            Command::new("dev")
                .about("Plan declared services; optional --probe / --spawn")
                .arg(
                    Arg::new("path")
                        .long("path")
                        .help("Project root"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("probe")
                        .long("probe")
                        .action(ArgAction::SetTrue)
                        .help("TCP-probe ready_tcp endpoints (still does not spawn processes)"),
                )
                .arg(
                    Arg::new("spawn")
                        .long("spawn")
                        .action(ArgAction::SetTrue)
                        .help("Spawn services declared in jhol.toml, then hold and clean up"),
                )
                .arg(
                    Arg::new("hold-secs")
                        .long("hold-secs")
                        .default_value("1")
                        .help("Seconds to hold spawned services before cleanup"),
                ),
        )
        .subcommand(
            Command::new("ai")
                .about("Pin, verify, prepare, and run small model artifacts")
                .subcommand(
                    Command::new("pull")
                        .about("Fetch model files into the shared store and link them into the project")
                        .arg(Arg::new("source").long("source").required(true).value_name("hf://OWNER/REPO"))
                        .arg(Arg::new("revision").long("revision").default_value("main"))
                        .arg(
                            Arg::new("file")
                                .long("file")
                                .required(true)
                                .action(ArgAction::Append)
                                .value_name("PATH_IN_REPO"),
                        )
                        .arg(Arg::new("dest").long("dest").value_name("DIR"))
                        .arg(
                            Arg::new("max-bytes")
                                .long("max-bytes")
                                .value_name("N")
                                .help("Refuse a single file larger than this (default 536870912)"),
                        )
                        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("inspect")
                        .about("Describe a safetensors model from its header (no weights fetched)")
                        .arg(
                            Arg::new("path")
                                .long("path")
                                .value_name("FILE")
                                .help("Local .safetensors file"),
                        )
                        .arg(
                            Arg::new("source")
                                .long("source")
                                .value_name("hf://OWNER/REPO")
                                .help("Hugging Face repository, read over a byte range"),
                        )
                        .arg(Arg::new("revision").long("revision").value_name("REV"))
                        .arg(
                            Arg::new("file")
                                .long("file")
                                .value_name("PATH_IN_REPO")
                                .help("File inside the repository (e.g. model.safetensors)"),
                        )
                        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("pin")
                        .about("Hash local files into a revision-pinned model manifest")
                        .arg(Arg::new("name").long("name").required(true))
                        .arg(Arg::new("source").long("source").required(true))
                        .arg(Arg::new("revision").long("revision").required(true))
                        .arg(
                            Arg::new("file")
                                .long("file")
                                .required(true)
                                .action(ArgAction::Append),
                        )
                        .arg(
                            Arg::new("manifest")
                                .long("manifest")
                                .default_value("models/demo.jhol-model.json"),
                        )
                        .arg(Arg::new("path").long("path").help("Project root"))
                        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("pin-remote")
                        .about("Record trusted SHA-256 and size pins before any download")
                        .arg(Arg::new("name").long("name").required(true))
                        .arg(Arg::new("source").long("source").required(true))
                        .arg(Arg::new("revision").long("revision").required(true))
                        .arg(
                            Arg::new("file")
                                .long("file")
                                .required(true)
                                .action(ArgAction::Append),
                        )
                        .arg(
                            Arg::new("digest")
                                .long("digest")
                                .required(true)
                                .action(ArgAction::Append),
                        )
                        .arg(
                            Arg::new("size-bytes")
                                .long("size-bytes")
                                .required(true)
                                .action(ArgAction::Append),
                        )
                        .arg(
                            Arg::new("manifest")
                                .long("manifest")
                                .default_value("models/demo.jhol-model.json"),
                        )
                        .arg(Arg::new("path").long("path").help("Project root"))
                        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("verify")
                        .about("Fail closed if pinned files do not match digests")
                        .arg(
                            Arg::new("manifest")
                                .long("manifest")
                                .default_value("models/demo.jhol-model.json"),
                        )
                        .arg(Arg::new("path").long("path").help("Project root"))
                        .arg(
                            Arg::new("json")
                                .long("json")
                                .action(ArgAction::SetTrue),
                        ),
                )
                .subcommand(
                    Command::new("prepare")
                        .about("Verify cached files or fetch explicitly approved missing files")
                        .arg(
                            Arg::new("manifest")
                                .long("manifest")
                                .default_value("models/demo.jhol-model.json"),
                        )
                        .arg(Arg::new("path").long("path").help("Project root"))
                        .arg(
                            Arg::new("allow-network")
                                .long("allow-network")
                                .action(ArgAction::SetTrue)
                                .help("Allow HTTPS model downloads from the pinned source"),
                        )
                        .arg(
                            Arg::new("max-bytes")
                                .long("max-bytes")
                                .default_value("10737418240")
                                .help("Maximum total bytes to fetch (default 10 GiB)"),
                        )
                        .arg(
                            Arg::new("json")
                                .long("json")
                                .action(ArgAction::SetTrue),
                        ),
                )
                .subcommand(
                    Command::new("infer")
                        .about("Run a pinned linear classifier on Jhol's bounded CPU runtime")
                        .arg(
                            Arg::new("manifest")
                                .long("manifest")
                                .default_value("models/demo.jhol-model.json"),
                        )
                        .arg(
                            Arg::new("config")
                                .long("config")
                                .required(true)
                                .help("Pinned jhol.linear-classifier.v1 config file"),
                        )
                        .arg(
                            Arg::new("input")
                                .long("input")
                                .required(true)
                                .help("Comma-separated numeric feature vector"),
                        )
                        .arg(Arg::new("path").long("path").help("Project root"))
                        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                ),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove a package (optionally from package.json)")
                .arg(Arg::new("package").required(true).help("Package name to remove"))
                .arg(
                    Arg::new("save")
                        .long("save")
                        .action(ArgAction::SetTrue)
                        .hide(true)
                        .help("Kept for compatibility; removal persists by default"),
                )
                .arg(
                    Arg::new("no-save")
                        .long("no-save")
                        .action(ArgAction::SetTrue)
                        .help("Leave package.json untouched"),
                ),
        )
        .subcommand(
            Command::new("update")
                .about("Update packages to latest and refresh lockfile")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .num_args(0..)
                        .help("Package(s) to update; omit for full lockfile refresh"),
                )
                .arg(
                    Arg::new("save")
                        .long("save")
                        .action(ArgAction::SetTrue)
                        .help("Write updated versions back to package.json"),
                ),
        )
        .subcommand(
            Command::new("why")
                .about("Explain why a package is installed (dependency path)")
                .arg(Arg::new("package").required(true).help("Package name"))
                .arg(
                    Arg::new("jagr")
                        .long("jagr")
                        .action(ArgAction::SetTrue)
                        .help("Use JAGR three-layer analysis to produce a richer explanation (Tropical + Sheaf Cohomology + Max-SMT)"),
                )
                .arg(
                    Arg::new("manifest")
                        .long("manifest")
                        .short('m')
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json to analyze (used with --jagr)"),
                ),
        )
        .subcommand(
            Command::new("outdated")
                .about("Show outdated dependencies with current, wanted, and latest versions")
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("jagr")
                        .long("jagr")
                        .action(ArgAction::SetTrue)
                        .help("After listing outdated packages, also report JAGR topological impact (which upgrades would break SNF or raise β₁)"),
                ),
        )
        .subcommand(
            Command::new("init")
                .about("Initialize a new package.json in the current directory")
                .arg(
                    Arg::new("yes")
                        .short('y')
                        .long("yes")
                        .action(ArgAction::SetTrue)
                        .help("Accept defaults without prompting"),
                )
                .arg(
                    Arg::new("private")
                        .long("private")
                        .action(ArgAction::SetTrue)
                        .help("Mark package as private (not for publishing)"),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Alias for install: add packages to dependencies")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .num_args(0..)
                        .help("Package(s) to add"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                ),
        )
        .subcommand(
            Command::new("remove")
                .about("Alias for uninstall: remove packages from dependencies")
                .arg(Arg::new("package").required(true).help("Package name(s) to remove"))
                .arg(
                    Arg::new("save")
                        .long("save")
                        .action(ArgAction::SetTrue)
                        .hide(true)
                        .help("Kept for compatibility; removal persists by default"),
                )
                .arg(
                    Arg::new("no-save")
                        .long("no-save")
                        .action(ArgAction::SetTrue)
                        .help("Leave package.json untouched"),
                ),
        )
        .subcommand(
            Command::new("link")
                .about("Link a local package directory into node_modules")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .help("Package directory to link (default: current directory)"),
                ),
        )
        .subcommand(
            Command::new("unlink")
                .about("Remove a linked package")
                .arg(Arg::new("package").required(true).help("Package name to unlink")),
        )
        .subcommand(
            Command::new("approve-scripts")
                .about("Grant content-addressed consent for install scripts")
                .arg(
                    Arg::new("packages")
                        .num_args(0..)
                        .value_name("PACKAGE")
                        .help("Package names; omit with --all for every package with scripts"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .num_args(0)
                        .help("Approve every package that currently ships install scripts"),
                )
            )
            .subcommand(
                Command::new("replay")
                    .about("Reproduce a published environment from a replay spec (deps as of a date, plus models)")
                    .arg(Arg::new("spec").required(true).value_name("FILE.json"))
                    .arg(
                        Arg::new("execute")
                            .long("execute")
                            .action(ArgAction::SetTrue)
                            .help("Run the plan instead of printing it"),
                    ),
            )
            .subcommand(
                Command::new("env")
                    .about("Copy-on-write environment forks: try a change without touching the original")
                    .subcommand(
                        Command::new("fork")
                            .about("Fork the current environment into .jhol/envs/<name>")
                            .arg(Arg::new("name").required(true))
                            .arg(Arg::new("from").long("from").value_name("DIR").help("Source project (default: .)"))
                            .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                    )
                    .subcommand(Command::new("list").about("List forks").arg(Arg::new("json").long("json").action(ArgAction::SetTrue)))
                    .subcommand(
                        Command::new("diff")
                            .about("Compare the resolved versions of two forks")
                            .arg(Arg::new("a").required(true))
                            .arg(Arg::new("b").required(true)),
                    )
                    .subcommand(
                        Command::new("rm")
                            .about("Delete a fork")
                            .arg(Arg::new("name").required(true)),
                    ),
            )
            .subcommand(
                Command::new("attest")
                    .about("Record (or verify) a deterministic proof of this environment")
                    .arg(Arg::new("verify").long("verify").action(ArgAction::SetTrue))
                    .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
            )
            .subcommand(
                Command::new("axes")
                    .about("Show the hardware/ABI axes jhol uses for compatibility decisions")
                    .arg(Arg::new("json").long("json").action(ArgAction::SetTrue))
                    .arg(
                        Arg::new("python")
                            .long("python")
                            .value_name("X.Y")
                            .help("Assume this Python version instead of the detected one"),
                    ),
            )
            .subcommand(
                Command::new("pysolve")
                    .about("Resolve a Python dependency graph to artifacts that run on this machine")
                    .arg(
                        Arg::new("spec")
                            .required(true)
                            .value_name("PACKAGE[@VERSION]")
                            .help("e.g. requests or torch@2.4.1"),
                    )
                    .arg(Arg::new("python").long("python").value_name("X.Y"))
                    .arg(
                        Arg::new("allow-sdist")
                            .long("allow-sdist")
                            .action(ArgAction::SetTrue)
                            .help("Also accept releases with no compatible wheel"),
                    )
                    .arg(
                        Arg::new("pre")
                            .long("pre")
                            .action(ArgAction::SetTrue)
                            .help("Allow pre-release versions"),
                    )
                    .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
            )
            .subcommand(
                Command::new("wheels")
                    .about("Which wheels of a PyPI release would run on this machine, and why not")
                    .arg(
                        Arg::new("spec")
                            .required(true)
                            .value_name("PACKAGE[@VERSION]")
                            .help("e.g. torch@2.4.1 (version optional)"),
                    )
                    .arg(
                        Arg::new("python")
                            .long("python")
                            .value_name("X.Y")
                            .help("Assume this Python version"),
                    )
                    .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
            )
            .subcommand(
                Command::new("verify")
                    .about("Check that node_modules is exactly what the lockfile authorises")
                    .arg(
                        Arg::new("json")
                            .long("json")
                            .num_args(0)
                            .help("Machine-readable output"),
                    )
                    .arg(
                        Arg::new("prune")
                            .long("prune")
                            .num_args(0)
                            .help("Remove packages the lockfile does not authorise"),
                    )
                    .arg(
                        Arg::new("quiet")
                            .long("quiet")
                            .short('q')
                            .num_args(0)
                            .help("Only report problems (via the exit code)"),
                    ),
            )
            .subcommand(
                Command::new("why-not")
                    .about("Explain why a range of a package cannot be used, and what would allow it")
                    .arg(
                        Arg::new("spec")
                            .required(true)
                            .value_name("PACKAGE@RANGE")
                            .help("Package and range, e.g. chalk@^4"),
                    ),
            )
            .subcommand(
                Command::new("revoke-scripts")
                    .about("Revoke script consent for packages")
                    .arg(
                        Arg::new("packages")
                            .num_args(1..)
                            .required(true)
                            .value_name("PACKAGE")
                            .help("Package names to revoke (name or name@version)"),
                    )
            )
            .subcommand(
                Command::new("global")
                    .about("Manage globally installed packages")
                    .subcommand(Command::new("list").about("List globally installed packages"))
                    .subcommand(Command::new("path").about("Print the global prefix"))
                    .subcommand(
                        Command::new("remove")
                            .about("Remove a globally installed package")
                            .arg(Arg::new("package").required(true)),
                    ),
            )
            .subcommand(
                Command::new("login")
                    .about("Store a registry auth token in .npmrc (0600)")
                    .arg(
                        Arg::new("registry")
                            .long("registry")
                            .help("Registry URL (default: effective registry)"),
                    )
                    .arg(
                        Arg::new("token")
                            .long("token")
                            .required(true)
                            .help("Auth token to store"),
                    )
                    .arg(
                        Arg::new("global")
                            .long("global")
                            .num_args(0)
                            .help("Write ~/.npmrc instead of the project .npmrc"),
                    ),
            )
            .subcommand(
                Command::new("publish")
                    .about("Publish the package in this directory to the registry")
                    .arg(Arg::new("dir").default_value(".").help("Package directory"))
                    .arg(
                        Arg::new("dry-run")
                            .long("dry-run")
                            .num_args(0)
                            .help("Build the full request without sending it"),
                    )
                    .arg(
                        Arg::new("registry")
                            .long("registry")
                            .help("Override the registry URL"),
                    ),
            )
            .subcommand(
                Command::new("dedupe")
                    .about("Report packages installed at more than one version")
                    .arg(Arg::new("json").long("json").num_args(0)
                        .help("Machine-readable output")),
            )
            .subcommand(
                Command::new("patch")
                    .about("Persist local edits to a dependency (extract, edit, commit)")
                    .arg(
                        Arg::new("package")
                            .num_args(0..=1)
                            .value_name("PACKAGE")
                            .help("Package to patch; omit with --list"),
                    )
                    .arg(Arg::new("commit").long("commit").num_args(0)
                        .help("Store the edits made in the extracted copy"))
                    .arg(Arg::new("list").long("list").num_args(0)
                        .help("Show recorded patches"))
                    .arg(Arg::new("rollback").long("rollback").num_args(0)
                        .help("Forget a package's patch")),
            )
            .subcommand(
                Command::new("view")
                    .about("Inspect a package's registry metadata (versions, deps, integrity)")
                    .arg(Arg::new("package").required(true).help("name or name@version")),
            )
            .subcommand(
                Command::new("search")
                    .about("Search the registry")
                    .arg(Arg::new("term").required(true).help("Search terms"))
                    .arg(
                        Arg::new("limit")
                            .long("limit")
                            .value_parser(clap::value_parser!(usize))
                            .default_value("20")
                            .help("Max results"),
                    ),
            )
            .subcommand(
                Command::new("pack")
                    .about("Build a deterministic, reproducible package tarball")
                    .arg(Arg::new("dir").default_value(".").help("Package directory")),
            )
.subcommand(
            Command::new("sbom")
                .about("Generate Software Bill of Materials")
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["cyclonedx", "simple"])
                        .default_value("cyclonedx")
                        .help("SBOM format"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .help("Write to file (default: stdout)"),
                ),
        )
        .subcommand(
            Command::new("jagr")
                .about("Run the JAGR three-layer resolver analysis on the current project's package.json (Tropical + Sheaf Cohomology + Core-Guided MaxSAT)")
                .arg(
                    Arg::new("show-config")
                        .long("show-config")
                        .action(ArgAction::SetTrue)
                        .help("Print the merged JAGR config (.jholrc + env vars) and exit"),
                )
                .arg(
                    Arg::new("manifest")
                        .long("manifest")
                        .short('m')
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json manifest to analyze"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output the full JagrAnalysisReport as JSON"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output (strategy + selection only)"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .short('w')
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run the JAGR analysis in all workspace packages"),
                )
                .arg(
                    Arg::new("skip-layer2")
                        .long("skip-layer2")
                        .action(ArgAction::SetTrue)
                        .help("Skip Layer 2 (sheaf cohomology) and go directly to Layer 1"),
                )
                .arg(
                    Arg::new("skip-layer3")
                        .long("skip-layer3")
                        .action(ArgAction::SetTrue)
                        .help("Skip Layer 3 (tropical PTIME filter)"),
                )
                .arg(
                    Arg::new("run-all-layers")
                        .long("run-all-layers")
                        .action(ArgAction::SetTrue)
                        .help("Run all three layers even if an earlier layer already produced a definite answer (useful for benchmarking)"),
                )
                .arg(
                    Arg::new("graph")
                        .long("graph")
                        .value_parser(["dot", "mermaid"])
                        .help("Output the derivation/conflict graph in the given format (dot or mermaid)"),
                )
                .arg(
                    Arg::new("lockfile-only")
                        .long("lockfile-only")
                        .action(ArgAction::SetTrue)
                        .help("Resolve and write the lockfile, but don't install any packages (mirrors `jhol install --lockfile-only`)"),
                )
                .arg(
                    Arg::new("explain-each")
                        .long("explain-each")
                        .action(ArgAction::SetTrue)
                        .help("Show a per-package explanation chain (which layer picked each version and why)"),
                )
                .arg(
                    Arg::new("compare-resolvers")
                        .long("compare-resolvers")
                        .action(ArgAction::SetTrue)
                        .help("Run JAGR and the legacy resolvers (pubgrub, sat) on the same manifest and print a side-by-side time/selection comparison"),
                )
                .arg(
                    Arg::new("upgrade-plan")
                        .long("upgrade-plan")
                        .action(ArgAction::SetTrue)
                        .help("For each declared dep, run JAGR before/after a candidate version change and report topological impact (β₁, SNF, risk class)"),
                )
                .arg(
                    Arg::new("dry-run-upgrade")
                        .long("dry-run-upgrade")
                        .value_name("PKG@VERSION")
                        .action(ArgAction::Set)
                        .help("Simulate upgrading PKG to VERSION and report JAGR topological impact (no manifest is modified)"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .help("Write the analysis output to a file (default: stdout)"),
                ),
        )
        .subcommand(
            Command::new("audit-jagr")
                .about("Save / load / diff JAGR analysis reports for reproducibility and audit trails")
                .arg(
                    Arg::new("save")
                        .long("save")
                        .value_name("PATH")
                        .help("Run JAGR on the current manifest and save the report to PATH as JSON"),
                )
                .arg(
                    Arg::new("load")
                        .long("load")
                        .value_name("PATH")
                        .help("Load a JAGR report from PATH and print a summary"),
                )
                .arg(
                    Arg::new("diff")
                        .long("diff")
                        .value_name("OLD,NEW")
                        .help("Compare two JAGR audit JSON files (comma-separated paths) and print the diff"),
                )
                .arg(
                    Arg::new("manifest")
                        .long("manifest")
                        .short('m')
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json manifest to analyze (used with --save)"),
                ),
        )
        .subcommand(
            Command::new("betti-history")
                .about("Show the persistent Betti-number history of JAGR analyses for this project (set JHOL_BETTI_HISTORY=1 to record)")
                .arg(
                    Arg::new("file")
                        .long("file")
                        .value_name("PATH")
                        .default_value(".jagr_betti_history.jsonl")
                        .help("Path to the Betti history JSONL file"),
                )
                .arg(
                    Arg::new("last")
                        .long("last")
                        .value_name("N")
                        .help("Show only the last N entries (e.g. --last 5)"),
                ),
        )
        .subcommand(
            Command::new("jagr-impact")
                .about("Simulate upgrading a single package and report the JAGR topological impact (β₁, SNF, risk class). No manifest is modified.")
                .arg(
                    Arg::new("manifest")
                        .short('m')
                        .long("manifest")
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json manifest"),
                )
                .arg(
                    Arg::new("package")
                        .long("package")
                        .short('p')
                        .value_name("PKG")
                        .required_unless_present("target")
                        .help("Package name to simulate upgrading"),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .short('v')
                        .value_name("VER")
                        .required_unless_present("target")
                        .help("Target version to simulate"),
                )
                .arg(
                    Arg::new("target")
                        .long("target")
                        .value_name("PKG@VERSION")
                        .help("Single-value form: PKG@VERSION (e.g. left-pad@1.3.0)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output as JSON"),
                ),
        )
        .subcommand(
            Command::new("gui")
                .about("Open the jhol Terminal GUI — browse packages, view recent installs, navigate with arrow keys")
                .long_about(
                    "Launches the jhol Terminal GUI (TUI). \
                     The classic CLI is still available — this is purely additive.\n\n\
                     Keyboard shortcuts inside the GUI:\n  \
                     ↑↓ / j k   navigate\n  \
                     Tab / 1 2 3  switch panels\n  \
                     r / F5      refresh\n  \
                     q / Esc     quit\n  \
                     ?           help"
                ),
        )
        .subcommand(
            Command::new("mcp")
                .about("Run Model Context Protocol (MCP) server for AI coding agents (stdio)")
                .arg(
                    Arg::new("stdio")
                        .long("stdio")
                        .action(ArgAction::SetTrue)
                        .help("Run MCP server over standard I/O (default)"),
                ),
        )
        .subcommand(
            Command::new("merge-lock")
                .about("Resolve 3-way Git merge conflicts in package-lock.json using JAGR-Infinite")
                .arg(
                    Arg::new("base")
                        .long("base")
                        .value_name("FILE")
                        .default_value("package-lock.json")
                        .help("Common ancestor lockfile path"),
                )
                .arg(
                    Arg::new("ours")
                        .long("ours")
                        .value_name("FILE")
                        .default_value("package-lock.json")
                        .help("Current branch (HEAD) lockfile path"),
                )
                .arg(
                    Arg::new("theirs")
                        .long("theirs")
                        .value_name("FILE")
                        .default_value("package-lock.json")
                        .help("Incoming branch (MERGE_HEAD) lockfile path"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("FILE")
                        .default_value("package-lock.json")
                        .help("Output merged lockfile path"),
                ),
        )
        .subcommand(
            Command::new("bisect-deps")
                .about("Automated dependency bisection to isolate breaking package upgrades")
                .arg(
                    Arg::new("test")
                        .long("test")
                        .value_name("CMD")
                        .required(true)
                        .help("Test command to execute for verifying build health (e.g. 'npm test')"),
                ),
        )
        .subcommand(
            Command::new("git-hook")
                .about("Manage Git hooks for automatic sub-8ms dependency synchronization")
                .subcommand(
                    Command::new("install")
                        .about("Install post-checkout and post-merge hooks in .git/hooks/"),
                ),
        )
        .subcommand(
            Command::new("completions")
                .about("Generate shell completion scripts")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(["bash", "zsh", "fish", "powershell", "elvish"])
                        .help("Shell to generate completions for (bash, zsh, fish, powershell, elvish)"),
                ),
        )
}

/// Project-relative directory for the lock (`models/<repo>`), so the lock does not
/// depend on where the project lives.
fn cache_relative_dir(dest: &std::path::Path) -> String {
    let rel = dest.strip_prefix(std::path::Path::new(".")).unwrap_or(dest);
    rel.to_string_lossy().replace('\\', "/")
}

/// One model-lock entry from the pins a pull just materialised.
fn artifact_lock_entries(
    dir: &str,
    pins: &[jhol_ai::FilePin],
) -> jhol_core::model_lock::ModelEntry {
    jhol_core::model_lock::ModelEntry {
        source: String::new(),
        revision: String::new(),
        dir: dir.to_string(),
        files: pins
            .iter()
            .map(|p| jhol_core::model_lock::ModelFile {
                path: p.path.clone(),
                sha256: p.digest.clone(),
                size: p.size_bytes.unwrap_or(0),
            })
            .collect(),
    }
}

/// `jhol ai pull` — fetch model files once, keep them in the shared store, and link
/// them into the project.
///
/// Two projects pulling the same weights must not double the disk usage: files are
/// stored by SHA-256 and materialised by hard link, so the second project costs an
/// inode rather than a checkpoint. The manifest written alongside makes the pull
/// verifiable afterwards (`jhol ai verify`).
fn pull_model(m: &clap::ArgMatches) -> Result<String, String> {
    use jhol_ai::cas::ModelStore;
    use jhol_ai::{FilePin, ModelArtifact};

    let source = m
        .get_one::<String>("source")
        .ok_or("--source is required")?
        .clone();
    let revision = m
        .get_one::<String>("revision")
        .cloned()
        .unwrap_or_else(|| "main".to_string());
    let files: Vec<String> = m
        .get_many::<String>("file")
        .map(|v| v.cloned().collect())
        .unwrap_or_default();
    let cap: u64 = m
        .get_one::<String>("max-bytes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(512 * 1024 * 1024);
    let repo_name = source
        .strip_prefix("hf://")
        .and_then(|r| r.split('/').nth(1))
        .unwrap_or("model")
        .to_string();
    let dest = m
        .get_one::<String>("dest")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("models").join(&repo_name));

    let cache = std::path::PathBuf::from(jhol_core::utils::get_cache_dir()).join("model_store");
    let store = ModelStore::new(cache);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    // A manifest pins a commit, never a branch: `main` moves, and a pin that moves
    // is not a pin. Resolve the ref once (HF returns `X-Repo-Commit` for a file
    // request) and use the commit for the manifest and every download.
    let revision = if revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        revision
    } else {
        let probe = hf_resolve_url(&source, &revision, &files[0])?;
        let headers = jhol_core::http_client::head_headers(&probe)
            .map_err(|e| format!("resolving {source}@{revision}: {e}"))?;
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-repo-commit"))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| {
                format!(
                    "could not resolve {source}@{revision} to a commit; pass --revision <40-char commit>"
                )
            })?
    };

    let mut pins: Vec<FilePin> = Vec::new();
    let mut downloaded = 0usize;
    for file in &files {
        if file.starts_with('/') || file.contains("..") {
            return Err(format!("invalid file path in repo: {file}"));
        }
        let url = hf_resolve_url(&source, &revision, file)?;
        let target = dest.join(file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // The store is the authority, not the working copy. A file on disk that the
        // store does not know is either tampered or from elsewhere: re-fetch it.
        // (Trusting the local file made `pull` silently preserve a corrupted
        // checkpoint, which is the one thing a pull must never do.)
        let local_digest = if target.is_file() {
            Some(jhol_ai::digest_file(&target).map_err(|e| e.to_string())?)
        } else {
            None
        };
        let digest = if let Some(d) = local_digest.filter(|d| store.contains(d)) {
            d
        } else {
            // Streamed to disk with resume and hashing in one pass: a 7 GB
            // checkpoint must never be buffered in memory, and an interrupted pull
            // must continue rather than start over.
            let tmp = target.with_extension("part");
            let (_bytes, digest) = jhol_core::http_client::download_to_path(&url, &tmp, cap)
                .map_err(|e| format!("{url}: {e}"))?;
            store.put(&tmp).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&tmp);
            downloaded += 1;
            digest
        };
        // Materialise from the store so the project links to the shared copy.
        store
            .materialise(&digest, &target)
            .map_err(|e| e.to_string())?;
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        pins.push(FilePin {
            path: file.clone(),
            digest,
            size_bytes: Some(size),
        });
    }

    let cached = artifact_lock_entries(&cache_relative_dir(&dest), &pins);
    let mut lock = jhol_core::model_lock::ModelLock::load(std::path::Path::new("."))
        .unwrap_or_default();
    lock.insert(&repo_name, cached);
    lock.save(std::path::Path::new("."))
        .map_err(|e| format!("model lock: {e}"))?;

    let artifact = ModelArtifact::pin_remote_files(&repo_name, &source, &revision, pins)
        .map_err(|e| e.to_string())?;
    let manifest_path = dest.join(format!("{repo_name}.jhol-model.json"));
    artifact
        .write_manifest(&manifest_path)
        .map_err(|e| e.to_string())?;
    if !flag(m, "json") {
        return Ok(format!(
            "{} file(s), {} downloaded, {} bytes linked from {} -> {}\nmanifest: {}",
            artifact.files.len(),
            downloaded,
            artifact.files.iter().filter_map(|f| f.size_bytes).sum::<u64>(),
            store.root().display(),
            dest.display(),
            manifest_path.display()
        ));
    }
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "artifact": artifact,
        "downloaded": downloaded,
        "store": store.root().display().to_string(),
        "dest": dest.display().to_string(),
    }))
    .unwrap_or_default())
}

/// `jhol ai inspect` — describe a safetensors model without fetching its weights.
///
/// The whole point: a checkpoint's structure (parameter count, dtypes, shard layout,
/// per-tensor byte ranges) lives in a header of a few kilobytes, so a model can enter
/// dependency resolution with real metadata — as cheaply as an npm packument. The
/// header is fetched over a byte range, never the file.
fn inspect_model(m: &clap::ArgMatches) -> Result<String, String> {
    use jhol_ai::safetensors::{self, Header};

    let header: Header = if let Some(path) = m.get_one::<String>("path") {
        safetensors::parse_file(std::path::Path::new(path))
            .map_err(|e| format!("{}: {e}", path))?
    } else {
        let source = m
            .get_one::<String>("source")
            .ok_or("inspect needs --path FILE or --source hf://owner/repo --file PATH")?;
        let revision = m
            .get_one::<String>("revision")
            .map(|s| s.as_str())
            .unwrap_or("main");
        let file = m
            .get_one::<String>("file")
            .ok_or("--source needs --file (e.g. model.safetensors)")?;
        let url = hf_resolve_url(source, revision, file)?;
        // Two requests: the declared header length, then the header itself.
        let first = jhol_core::http_client::get_range(&url, 0, 7)
            .map_err(|e| format!("{url}: {e}"))?;
        let n = safetensors::header_len(&first).map_err(|e| e.to_string())?;
        let body = jhol_core::http_client::get_range(&url, 8, 8 + n - 1)
            .map_err(|e| format!("{url}: {e}"))?;
        let mut all = Vec::with_capacity(8 + body.len());
        all.extend_from_slice(&first);
        all.extend_from_slice(&body);
        safetensors::parse_header(&all).map_err(|e| e.to_string())?
    };

    if flag(m, "json") {
        let tensors: Vec<serde_json::Value> = header
            .tensors
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name, "dtype": t.dtype, "shape": t.shape,
                    "bytes": t.end.saturating_sub(t.begin),
                })
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "tensors": header.tensors.len(),
            "parameters": header.total_params,
            "dtypes": header.dtypes,
            "header_bytes": header.header_bytes,
            "data_start": header.data_start,
            "detail": tensors,
        }))
        .unwrap_or_default());
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{} tensors, {} parameters, header {} bytes (data starts at {})
",
        header.tensors.len(),
        group_digits(header.total_params),
        header.header_bytes,
        header.data_start
    ));
    let dtypes: Vec<String> = header
        .dtypes
        .iter()
        .map(|(d, n)| format!("{d} x{n}"))
        .collect();
    out.push_str(&format!("dtypes: {}
", dtypes.join(", ")));
    out.push_str("largest tensors:
");
    let mut by_size: Vec<&jhol_ai::safetensors::Tensor> = header.tensors.iter().collect();
    by_size.sort_by_key(|t| std::cmp::Reverse(t.end.saturating_sub(t.begin)));
    for t in by_size.iter().take(5) {
        out.push_str(&format!(
            "  {} {:?} [{:?}] {} bytes
",
            t.dtype,
            t.shape,
            t.dtype,
            t.end.saturating_sub(t.begin)
        ));
    }
    Ok(out)
}

/// `hf://owner/repo` + revision + path -> the resolve URL (no hub of our own).
fn hf_resolve_url(source: &str, revision: &str, file: &str) -> Result<String, String> {
    let repo = source
        .strip_prefix("hf://")
        .ok_or("source must be hf://owner/repository")?;
    if repo.split('/').count() != 2 || file.starts_with('/') || file.contains("..") {
        return Err(format!("invalid model reference: {source} / {file}"));
    }
    Ok(format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        repo, revision, file
    ))
}

fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn main() {
    // Rust ignores SIGPIPE, so writing to a closed pipe (`jhol cache size | head`)
    // panicked with "Broken pipe" — and release builds use panic=abort, so that
    // killed the process. Restore the default before any output.
    jhol_core::restore_default_sigpipe();

    // THE freshness authority, before any lane is chosen. See
    // tree_is_current_and_reported: five downstream gates each answer with
    // evidence too weak for the question, and which one runs decides the result.
    if plain_install_fully_settled() {
        return;
    }

    // One install at a time per project, held for the life of the process.
    // Acquired here rather than inside a lane because the lanes are many and
    // the CLI dispatches before any of them: two concurrent `jhol install` runs
    // interleave their writes and leave a tree that matches neither.
    let _install_lock = install_command_lock();

    // An engine name that resolves to nothing used to silently run the script
    // on a different backend, which then surfaced engine-specific errors (a raw
    // `JSException { .. }` debug dump) with no hint that the variable itself
    // was wrong. Reject typos before anything else happens.
    if let Ok(v) = std::env::var("JHOL_RUNTIME_ENGINE") {
        const KNOWN: [&str; 7] = ["", "auto", "qjs", "quickjs", "jsc", "jholvm", "vm"];
        if !KNOWN.iter().any(|k| v.eq_ignore_ascii_case(k)) {
            eprintln!(
                "jhol: unknown runtime engine {:?} in JHOL_RUNTIME_ENGINE.\n      \
                 expected one of: auto, jholvm, jsc, qjs",
                v
            );
            std::process::exit(2);
        }
    }

    // Pre-Clap bypass is restricted to exact plain install/i invocations and
    // requires the same lockfile-bound proof as the core install path.
    jhol_core::daemon_cache::try_exit_before_clap();

    // Eagerly prewarm HTTP/2 network connection in the background during CLI startup and argument parsing
    let raw_args: Vec<_> = std::env::args_os().collect();
    let is_offline = raw_args.iter().any(|a| {
        let s = a.to_string_lossy();
        s == "--offline" || s == "-h" || s == "--help" || s == "--version" || s == "-V"
    });
    if !is_offline && std::env::var("JHOL_OFFLINE").is_err() {
        std::thread::Builder::new()
            .name("jhol-network-early-prewarm".into())
            .spawn(|| {
                jhol_core::http_client::prewarm_network();
            })
            .ok();
    }

    if !use_color() {
        colored::control::set_override(false);
    }

    let code = match std::panic::catch_unwind(|| run()) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            print_error("", &e);
            1
        }
        Err(_) => {
            print_error("", "An unexpected error occurred. Please report this issue at https://github.com/bhuvanprakash/jhol/issues");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(all(test, unix))]
mod daemon_socket_tests {
    use super::{prepare_daemon_socket, DaemonSocketCleanup};
    use std::os::unix::net::UnixListener;

    // Hermetic per-test directory owned by `tempfile`, removed on drop. The
    // previous hand-rolled `create_dir` + `remove_dir_all` pair panicked inside
    // `create_dir` when the shared `/tmp` was under disk pressure, which is the
    // failure this replaces; a failing assertion also no longer leaks a
    // directory. Keep this hermetic rather than special-casing an error.
    fn isolated_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("jhol-daemon-socket-test-")
            .tempdir()
            .expect("create isolated socket directory")
    }

    #[test]
    fn active_daemon_socket_is_preserved() {
        let dir = isolated_dir();
        let socket = dir.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind active test socket");
        assert!(prepare_daemon_socket(&socket).is_err());
        assert!(std::fs::symlink_metadata(&socket).is_ok());
        drop(listener);
    }

    #[test]
    fn stale_daemon_socket_is_removed() {
        let dir = isolated_dir();
        let socket = dir.path().join("daemon.sock");
        drop(UnixListener::bind(&socket).expect("bind stale test socket"));
        prepare_daemon_socket(&socket).expect("remove stale socket");
        assert!(std::fs::symlink_metadata(&socket).is_err());
    }

    #[test]
    fn non_socket_daemon_path_is_preserved() {
        let dir = isolated_dir();
        let socket = dir.path().join("daemon.sock");
        std::fs::write(&socket, b"keep me").expect("create non-socket path");
        assert!(prepare_daemon_socket(&socket).is_err());
        assert_eq!(
            std::fs::read(&socket).expect("read preserved path"),
            b"keep me"
        );
    }

    #[test]
    fn daemon_cleanup_removes_its_bound_socket() {
        let dir = isolated_dir();
        let socket = dir.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind owned socket");
        let cleanup = DaemonSocketCleanup::capture(&socket).expect("capture socket identity");
        drop(listener);

        cleanup.remove_if_owned().expect("remove owned socket");
        assert!(std::fs::symlink_metadata(&socket).is_err());
        drop(cleanup);
    }

    #[test]
    fn daemon_cleanup_preserves_replacement_socket() {
        let dir = isolated_dir();
        let socket = dir.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind original socket");
        let cleanup = DaemonSocketCleanup::capture(&socket).expect("capture socket identity");
        std::fs::remove_file(&socket).expect("remove original socket path");

        let replacement = UnixListener::bind(&socket).expect("bind replacement socket");
        drop(listener);
        assert!(cleanup.remove_if_owned().is_err());
        assert!(std::fs::symlink_metadata(&socket).is_ok());
        drop(cleanup);
        drop(replacement);
    }
}
