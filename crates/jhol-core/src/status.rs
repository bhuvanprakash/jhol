//! One screen of truth about a project.
//!
//! The CLI, the TUI and the web dashboard all render **this** struct. That is the point:
//! the TUI used to collect its own numbers, which is how two implementations of one fact
//! start disagreeing. Anything that wants to show project state reads this.
//!
//! Four questions an ML developer actually asks, and the fields that answer them:
//!
//! | question | field |
//! |---|---|
//! | will it run on *this* box? | [`Artifact::fits`], [`Artifact::reasons`] |
//! | what will it cost? | [`Plan`] — missing bytes and a time at a stated throughput |
//! | what am I getting? | [`Artifact::params`], [`Artifact::vram_bytes`] |
//! | can I prove it later? | [`Proof`], [`Artifact::digest`] |
//!
//! Nothing here touches the network. `collect` reads lockfiles and the filesystem; the
//! only expensive step is a safetensors header, which [`crate::status`] callers add
//! through `jhol-ai` so this crate stays free of model parsing.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default)]
pub struct Status {
    pub project: String,
    pub version: String,
    /// Absolute path of the project this status describes. A dashboard that does not say
    /// *which* directory it is showing invites the wrong-project mistake.
    pub root: String,
    /// Is there a `package.json` here at all? An unnamed project and no project are
    /// different screens.
    pub has_manifest: bool,
    /// Is the installed tree what the lockfile authorises?
    pub tree: Tree,
    /// Does the lockfile still describe `package.json`?
    pub manifest_matches_lockfile: bool,
    /// Policy in force, as reported by the environment the command runs under.
    pub policy: Policy,
    pub models: Models,
    pub python: Python,
    pub axes: Axes,
    pub proof: Proof,
    /// Wheels and weights: the things that decide whether this environment runs.
    pub artifacts: Vec<Artifact>,
    /// What is still missing, in bytes and seconds.
    pub plan: Plan,
    /// Environment forks under `.jhol/envs`.
    pub forks: Vec<ForkRow>,
    /// What an agent can ask this project to do, with a cost class.
    pub agent: Agent,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Tree {
    /// `match` | `missing` | `wrong` | `extraneous` | `unverified`
    pub state: String,
    /// One line naming the package when the state is not `match`.
    pub detail: String,
    pub packages: usize,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Policy {
    pub as_of: Option<String>,
    pub allowed_licenses: Option<Vec<String>>,
    pub exclude_vulnerable: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Models {
    pub pinned: usize,
    pub names: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Python {
    /// Distributions recorded in `.jhol/py/lock.json`.
    pub pinned: usize,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Axes {
    pub os: String,
    pub arch: String,
    pub libc: String,
    pub python: Option<String>,
    pub cuda_driver: Option<String>,
    pub sm: Vec<u32>,
    pub gpu_mem_mb: Option<u64>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Proof {
    pub snapshot_ready: bool,
    pub attested_packages: usize,
    pub attested_models: usize,
}

/// One thing that must exist for the environment to work.
///
/// `fits` is `None` when we cannot decide — a pure-python wheel with no platform tag, or a
/// weight file whose need depends on a GPU this tool cannot probe. `None` is information:
/// it is never reported as a pass.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Artifact {
    /// `wheel` | `model`
    pub kind: String,
    pub name: String,
    pub version: String,
    pub bytes: Option<u64>,
    pub present: bool,
    /// Digest from the lockfile, truncated for display by the renderers, not by us.
    pub digest: String,
    pub fits: Option<bool>,
    pub reasons: Vec<String>,
    /// Where the artifact lives, relative to the project root, when it lives there.
    pub path: Option<String>,
    /// For weights: parameter count read from the safetensors header.
    pub params: Option<u64>,
    /// For weights: estimated resident bytes.
    pub vram_bytes: Option<u64>,
    /// For weights: dtype -> tensor count.
    pub dtypes: BTreeMap<String, usize>,
    /// For weights: the heaviest tensors, largest first.
    pub top_tensors: Vec<(String, u64)>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Plan {
    pub items: Vec<PlanItem>,
    pub cached_bytes: u64,
    pub missing_bytes: u64,
    /// Seconds for `missing_bytes` at [`Plan::throughput_bytes_per_sec`].
    pub eta_seconds: u64,
    /// The rate the estimate assumes. Stated in every rendering: a number without its
    /// assumption is a guess dressed as a measurement.
    pub throughput_bytes_per_sec: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct PlanItem {
    pub kind: String,
    pub name: String,
    pub bytes: u64,
    pub present: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct ForkRow {
    pub name: String,
    /// Bytes the fork occupies beyond the base tree, when measurable.
    pub bytes: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Agent {
    pub tools: Vec<ToolInfo>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct ToolInfo {
    pub name: String,
    pub purpose: String,
    /// `read` — no writes and no network; `network` — may fetch; `write` — changes the tree.
    pub cost: String,
}

/// What an agent can ask for. Kept as a catalog next to the MCP server so a dashboard can
/// show it without starting a JSON-RPC conversation; a test asserts every name here is one
/// the MCP server actually answers, so the two cannot drift.
pub fn tool_catalog() -> Vec<ToolInfo> {
    let row = |name: &str, purpose: &str, cost: &str| ToolInfo {
        name: name.to_string(),
        purpose: purpose.to_string(),
        cost: cost.to_string(),
    };
    vec![
        row(
            "jhol_resolve_speculation",
            "resolve a spec in memory without touching disk",
            "read",
        ),
        row("jhol_explain_failure", "attribute an unsatisfiable spec", "read"),
        row("jhol_auto_heal", "reconcile the tree with the lockfile", "write"),
        row("jhol_typosquat_check", "compare a name against known packages", "read"),
        row("jhol_verify_environment", "verify tree, models and attestation", "read"),
        row("jhol_axes", "report host os/arch/python/cuda/sm", "read"),
        row("jhol_why_not", "minimal unsat core for a pinned version", "read"),
        row("jhol_plan_model", "bytes and time for a model plan", "network"),
        row("jhol_plan_python", "bytes and time for a wheel plan", "network"),
        row("jhol_audit_report", "licences and known vulnerabilities", "read"),
        row("jhol_fit", "size a model into a memory budget from its header", "read"),
        row("jhol_plan_readonly", "a plan with no writes and no network", "read"),
        row("jhol_sync_lock", "rewrite the lockfile to match the manifest", "write"),
    ]
}

fn read_manifest(root: &Path) -> (String, String) {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return (String::new(), String::new());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (String::new(), String::new());
    };
    (
        v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
        v.get("version").and_then(|n| n.as_str()).unwrap_or("").to_string(),
    )
}

/// Directory sizes are useful and must never be a stall: stop after `cap` entries and say
/// nothing rather than walk a million files in a dashboard refresh.
fn tree_bytes(path: &Path, cap: usize) -> u64 {
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
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

fn wheel_artifacts(host: &crate::axes::HostAxes, lock: &crate::pyinstall::PyLock) -> Vec<Artifact> {
    lock.distributions
        .iter()
        .map(|(name, dist)| {
            let mut reasons = Vec::new();
            let fits = match crate::axes::WheelTag::parse(&dist.artifact) {
                Some(tag) => match crate::axes::wheel_fits(&tag, host) {
                    Ok(()) => {
                        reasons.push(format!(
                            "{} {} {} fits this host",
                            tag.python, tag.abi, tag.platform
                        ));
                        Some(true)
                    }
                    Err(why) => {
                        reasons.push(why.to_string());
                        Some(false)
                    }
                },
                None => {
                    reasons.push("no wheel tag in the artefact name".to_string());
                    None
                }
            };
            Artifact {
                kind: "wheel".into(),
                name: name.clone(),
                version: dist.version.clone(),
                bytes: None,
                present: true,
                digest: dist.sha256.clone(),
                fits,
                reasons,
                path: Some(crate::pyinstall::SITE_PACKAGES.to_string()),
                params: None,
                vram_bytes: None,
                dtypes: BTreeMap::new(),
                top_tensors: Vec::new(),
            }
        })
        .collect()
}

/// Weights, with sizes from the lock. Whether they *fit* needs a header read, which the
/// binary adds through `jhol-ai` — this crate does not parse model files.
fn model_artifacts(root: &Path, lock: &crate::model_lock::ModelLock) -> (Vec<Artifact>, Plan) {
    let mut out = Vec::new();
    let mut plan = Plan {
        throughput_bytes_per_sec: crate::pyinstall::THROUGHPUT_BYTES_PER_SEC,
        ..Default::default()
    };
    for (name, entry) in &lock.models {
        let mut total = 0u64;
        let mut missing = 0u64;
        let mut present_all = true;
        for file in &entry.files {
            total += file.size;
            // `ModelFile::path` is relative to the model's own directory; joining the
            // project root twice produces `models/x/models/x/...`, which is exactly the
            // defect that made `verify` and `status` disagree about one file.
            let here = root.join(&entry.dir).join(&file.path).is_file();
            if here {
                plan.cached_bytes += file.size;
            } else {
                plan.missing_bytes += file.size;
                missing += file.size;
                present_all = false;
            }
            plan.items.push(PlanItem {
                kind: "model".into(),
                name: format!("{name}/{}", file.path),
                bytes: file.size,
                present: here,
            });
        }
        let mut reasons = vec![format!(
            "{} @ {} (pinned)",
            entry.source,
            entry.revision.chars().take(12).collect::<String>()
        )];
        if missing > 0 {
            reasons.push(format!(
                "{:.1} MB missing of {:.1} MB",
                missing as f64 / 1_048_576.0,
                total as f64 / 1_048_576.0
            ));
        }
        out.push(Artifact {
            kind: "model".into(),
            name: name.clone(),
            version: entry.revision.chars().take(12).collect(),
            bytes: Some(total),
            present: present_all,
            digest: entry
                .files
                .first()
                .map(|f| f.sha256.clone())
                .unwrap_or_default(),
            // Unknown until the header is read: a weight that does not fit is a runtime
            // out-of-memory, and we will not call that a pass.
            fits: None,
            reasons,
            path: Some(entry.dir.clone()),
            params: None,
            vram_bytes: None,
            dtypes: BTreeMap::new(),
            top_tensors: Vec::new(),
        });
    }
    (out, plan)
}

/// npm in the host's dialect: a lockfile says `darwin`/`win32` where the rest of the
/// tool says `macos`/`windows`, and `x64` where it says `x86_64`.
fn npm_os(host: &str) -> &str {
    match host {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn npm_arch(host: &str) -> &str {
    match host {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Native npm packages declare where they may run. Those are the only packages that can
/// fail for *this* machine, so they are the only ones worth a fitness row: a table of
/// 71 pure-JS packages that all fit is noise, and noise is how a warning stops working.
fn npm_artifacts(root: &Path, host: &crate::axes::HostAxes) -> Vec<Artifact> {
    let Ok(text) = std::fs::read_to_string(root.join("package-lock.json")) else {
        return Vec::new();
    };
    // Most projects declare no native packages at all. A substring check over the raw
    // text costs microseconds; parsing 172 KB of lockfile costs milliseconds. The guard is
    // not a heuristic — a platform constraint cannot exist without one of these keys.
    if !text.contains("\"os\"") && !text.contains("\"cpu\"") && !text.contains("\"libc\"") {
        return Vec::new();
    }
    let Ok(lock) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(packages) = lock.get("packages").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let want_os = npm_os(&host.os);
    let want_arch = npm_arch(&host.arch);
    let mut out = Vec::new();
    for (path, entry) in packages {
        let list = |field: &str| -> Vec<String> {
            entry
                .get(field)
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        let os_list = list("os");
        let cpu_list = list("cpu");
        let libc_list = list("libc");
        if os_list.is_empty() && cpu_list.is_empty() && libc_list.is_empty() {
            continue;
        }
        let name = path.rsplit("node_modules/").next().unwrap_or(path).to_string();
        let mut reasons = Vec::new();
        let mut fits = true;
        if !os_list.is_empty() {
            let ok = os_list.iter().any(|o| o == want_os);
            fits &= ok;
            reasons.push(format!("os {} (host {want_os})", os_list.join(",")));
        }
        if !cpu_list.is_empty() {
            let ok = cpu_list.iter().any(|c| c == want_arch);
            fits &= ok;
            reasons.push(format!("cpu {} (host {want_arch})", cpu_list.join(",")));
        }
        if !libc_list.is_empty() {
            let host_libc = host.libc.to_string();
            let ok = libc_list.iter().any(|l| host_libc.contains(l.as_str()));
            fits &= ok;
            reasons.push(format!("libc {} (host {host_libc})", libc_list.join(",")));
        }
        out.push(Artifact {
            kind: "package".into(),
            name: name.clone(),
            version: entry
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            bytes: None,
            present: root.join(path).is_dir(),
            digest: String::new(),
            fits: Some(fits),
            reasons,
            path: Some(path.clone()),
            params: None,
            vram_bytes: None,
            dtypes: BTreeMap::new(),
            top_tensors: Vec::new(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn forks(root: &Path) -> Vec<ForkRow> {
    let Ok(entries) = std::fs::read_dir(root.join(".jhol/envs")) else {
        return Vec::new();
    };
    let mut out: Vec<ForkRow> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| ForkRow {
            name: e.file_name().to_string_lossy().to_string(),
            // A fork is mostly shared inodes; the cap keeps this from becoming a scan.
            bytes: tree_bytes(&e.path(), 4096),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Everything a status depends on, as one opaque tag: file sizes and mtimes for the
/// manifests, the lockfiles, `node_modules` and `.jhol/envs`.
///
/// A caller that polls can compare tags and skip `collect` entirely — which is what the web
/// dashboard's `ETag` and the CLI's `--watch` both do. One definition, so "has anything
/// changed?" cannot mean two different things in two places.
pub fn fingerprint(root: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for rel in [
        "package.json",
        "package-lock.json",
        "jhol-lock.json",
        ".jhol/models.lock.json",
        ".jhol/py/lock.json",
        "node_modules",
        ".jhol/envs",
    ] {
        rel.hash(&mut h);
        match std::fs::metadata(root.join(rel)) {
            Ok(m) => {
                m.len().hash(&mut h);
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .hash(&mut h);
            }
            Err(_) => 0u8.hash(&mut h),
        }
    }
    format!("{:016x}", h.finish())
}

/// Collect everything, cheaply. No network, no resolution: one lockfile parse, one tree
/// walk, and the files the artifact layers already keep.
pub fn collect(root: &Path) -> Status {
    let prof = std::env::var_os("JHOL_PROFILE_STATUS").is_some();
    let mut t = std::time::Instant::now();
    let mut mark = |label: &str, t: &mut std::time::Instant| {
        if prof {
            eprintln!("[status] {label}: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
            *t = std::time::Instant::now();
        }
    };
    let _ = &mut mark;
    let (project, version) = read_manifest(root);

    // One walk answers both "is it right?" and "how many packages?".
    let report = crate::tree_state::inspect_tree(root);
    mark("tree", &mut t);
    let state = report.state.clone();
    let (label, detail) = match &state {
        crate::tree_state::TreeState::Match => ("match", String::new()),
        crate::tree_state::TreeState::Missing { path } => ("missing", format!("{path} is missing")),
        crate::tree_state::TreeState::Wrong { path, want, got } => (
            "wrong",
            format!("{path} is {got} but the lockfile pins {want}"),
        ),
        crate::tree_state::TreeState::Extraneous { path } => {
            ("extraneous", format!("{path} is not in the lockfile"))
        }
        crate::tree_state::TreeState::Unverified { reason } => ("unverified", reason.clone()),
    };

    let model_lock = crate::model_lock::ModelLock::load(root).unwrap_or_default();
    let models = Models {
        pinned: model_lock.models.len(),
        names: model_lock.models.keys().cloned().collect(),
    };

    mark("lockfile+models", &mut t);
    let py_lock: Option<crate::pyinstall::PyLock> =
        std::fs::read_to_string(root.join(crate::pyinstall::PY_LOCK_PATH))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());

    let host = crate::axes::HostAxes::detect();

    let (attested_packages, attested_models) = crate::attest::Attestation::load(root)
        .map(|a| (a.packages.len(), a.models.len()))
        .unwrap_or((0, 0));

    let mut artifacts = npm_artifacts(root, &host);
    mark("npm rows", &mut t);
    if let Some(lock) = &py_lock {
        artifacts.extend(wheel_artifacts(&host, lock));
    }
    let (model_rows, mut plan) = model_artifacts(root, &model_lock);
    artifacts.extend(model_rows);

    plan.eta_seconds = plan
        .missing_bytes
        .div_ceil(plan.throughput_bytes_per_sec.max(1));

    mark("pylock+axes+attest+plan", &mut t);
    Status {
        root: std::fs::canonicalize(root)
            .unwrap_or_else(|_| root.to_path_buf())
            .display()
            .to_string(),
        has_manifest: root.join("package.json").is_file(),
        project,
        version,
        manifest_matches_lockfile: crate::install::lockfile_matches_manifest(root),
        // (timed below)
        tree: Tree {
            state: label.to_string(),
            detail,
        packages: report.installed,
        },
        policy: Policy {
            as_of: std::env::var("JHOL_AS_OF").ok(),
            allowed_licenses: std::env::var("JHOL_ALLOW_LICENSE")
                .ok()
                .map(|v| v.split(',').map(|s| s.trim().to_string()).collect()),
            exclude_vulnerable: std::env::var("JHOL_EXCLUDE_VULN").is_ok(),
        },
        models,
        python: Python {
            pinned: py_lock.as_ref().map(|l| l.distributions.len()).unwrap_or(0),
        },
        axes: Axes {
            os: host.os.clone(),
            arch: host.arch.clone(),
            libc: host.libc.to_string(),
            python: host.python.clone(),
            cuda_driver: host.cuda_driver.clone(),
            sm: host.sm.clone(),
            gpu_mem_mb: host.gpu_mem_mb,
        },
        proof: Proof {
            snapshot_ready: crate::install::snapshot_ready(root),
            attested_packages,
            attested_models,
        },
        artifacts,
        plan,
        forks: forks(root),
        agent: Agent {
            tools: tool_catalog(),
        },
    }
}


/// What an action would do, as one sentence. The TUI and the web page both print **this**
/// text; before it existed each built its own, which is how two surfaces start telling a
/// user two different things about the same download.
#[derive(Serialize, Clone, Debug, Default)]
pub struct ActionPreview {
    pub action: String,
    /// Does it change the tree? Read-only actions need no confirmation.
    pub writes: bool,
    pub summary: String,
}

pub fn action_preview(action: &str, status: &Status) -> ActionPreview {
    let throughput = status.plan.throughput_bytes_per_sec.max(1);
    let cost = if status.plan.missing_bytes > 0 {
        format!(
            "would fetch {:.1} MB — ~{} s at {} MB/s",
            status.plan.missing_bytes as f64 / 1_048_576.0,
            status.plan.missing_bytes.div_ceil(throughput),
            throughput / 1_048_576
        )
    } else {
        "nothing to download; this re-materialises from the lockfile".to_string()
    };
    let (writes, summary) = match action {
        "verify" => (
            false,
            format!(
                "reads {} packages and compares versions with the lockfile — no writes",
                status.tree.packages
            ),
        ),
        // `prune` deletes directories: it is a write, and it must ask even though `verify`
        // inside it is a read. Treating it as read-only let a dashboard button delete a tree.
        "prune" => (
            true,
            match status.tree.state.as_str() {
                "extraneous" => format!(
                    "would remove what the lockfile does not authorise — {}",
                    status.tree.detail
                ),
                _ => "nothing to prune; the tree holds only authorised packages".to_string(),
            },
        ),
        "install" => (true, cost),
        "ci" => (
            true,
            format!("{cost}; ci refuses to write a lockfile that would drift"),
        ),
        "policy" => (
            true,
            format!("{cost}; policy in force: {}", policy_line(&status.policy)),
        ),
        "fork" => (
            true,
            "forks the environment; most files share inodes, so it costs milliseconds".to_string(),
        ),
        "promote" => (
            true,
            "makes the fork the project's tree — two renames, no copying".to_string(),
        ),
        other => (true, format!("{other}: no preview available")),
    };
    ActionPreview {
        action: action.to_string(),
        writes,
        summary,
    }
}

fn policy_line(p: &Policy) -> String {
    let mut parts = Vec::new();
    if let Some(at) = &p.as_of {
        parts.push(format!("as-of {at}"));
    }
    if let Some(lic) = &p.allowed_licenses {
        parts.push(format!("licences {}", lic.join(",")));
    }
    if p.exclude_vulnerable {
        parts.push("known CVEs excluded".to_string());
    }
    if parts.is_empty() {
        "latest, any licence".to_string()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    fn status(missing: u64, state: &str) -> Status {
        let mut s = Status::default();
        s.tree.packages = 71;
        s.tree.state = state.to_string();
        s.plan.missing_bytes = missing;
        s.plan.throughput_bytes_per_sec = 5 * 1_048_576;
        s
    }

    #[test]
    fn reads_never_ask_for_confirmation_and_writes_always_do() {
        assert!(!action_preview("verify", &status(0, "match")).writes);
        // prune removes directories, so it is a write even though its name starts with `verify`.
        for a in ["prune", "install", "ci", "policy", "fork", "promote"] {
            assert!(action_preview(a, &status(0, "match")).writes, "{a} must confirm");
        }
    }

    #[test]
    fn the_estimate_states_its_throughput_and_scales_with_the_plan() {
        let p = action_preview("install", &status(10 * 1_048_576, "match"));
        assert!(p.summary.contains("10.0 MB"), "{}", p.summary);
        assert!(p.summary.contains("2 s"), "{}", p.summary);
        assert!(p.summary.contains("5 MB/s"), "{}", p.summary);
        let free = action_preview("install", &status(0, "match"));
        assert!(free.summary.contains("nothing to download"));
    }

    #[test]
    fn prune_names_what_it_would_remove() {
        let mut s = status(0, "extraneous");
        s.tree.detail = "node_modules/x is not in the lockfile".into();
        let p = action_preview("prune", &s);
        assert!(p.summary.contains("node_modules/x"), "{}", p.summary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog a dashboard shows and the catalog the MCP server answers must be one
    /// list. Cheap check, strong invariant: every name here must appear in `mcp.rs`.
    #[test]
    fn agent_catalog_and_mcp_server_answer_the_same_tools() {
        let source = include_str!("mcp.rs");
        for tool in tool_catalog() {
            let needle = format!("\"{}\"", tool.name);
            assert!(
                source.contains(&needle),
                "catalog lists {} but the MCP server does not answer it",
                tool.name
            );
        }
        // And the other direction: a tool the server answers but the catalog hides is a
        // capability an agent has and a human cannot see.
        let served: Vec<&str> = source
            .match_indices("\"name\": \"jhol_")
            .map(|(i, _)| {
                let rest = &source[i + 9..];
                let end = rest.find('"').unwrap_or(0);
                &rest[..end]
            })
            .collect();
        for name in served {
            assert!(
                tool_catalog().iter().any(|t| t.name == name),
                "the MCP server answers {name}, which the catalog hides"
            );
        }
    }

    /// The lock stores a path relative to the model directory. Resolving it against the
    /// project root as well makes one file look like two different files, and `status`
    /// then claims present while `verify` reports missing.
    #[test]
    fn npm_names_are_translated_to_the_hosts_dialect() {
        assert_eq!(npm_os("macos"), "darwin");
        assert_eq!(npm_os("windows"), "win32");
        assert_eq!(npm_os("linux"), "linux");
        assert_eq!(npm_arch("x86_64"), "x64");
        assert_eq!(npm_arch("aarch64"), "arm64");
    }

    #[test]
    fn native_packages_get_a_fitness_row_and_pure_js_does_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/@esbuild/darwin-arm64")).unwrap();
        std::fs::write(
            dir.path().join("package-lock.json"),
            r#"{"packages":{
              "": {},
              "node_modules/express": {"version":"4.18.2"},
              "node_modules/@esbuild/darwin-arm64": {"version":"0.20.0","os":["darwin"],"cpu":["arm64"]},
              "node_modules/@esbuild/linux-x64": {"version":"0.20.0","os":["linux"],"cpu":["x64"]}
            }}"#,
        )
        .unwrap();
        let host = crate::axes::HostAxes {
            os: "macos".into(),
            arch: "arm64".into(),
            libc: crate::axes::Libc::Other,
            python: None,
            cuda_driver: None,
            sm: Vec::new(),
            gpu_mem_mb: None,
        };
        let rows = npm_artifacts(dir.path(), &host);
        assert_eq!(rows.len(), 2, "express must not produce a row");
        let darwin = rows.iter().find(|r| r.name.contains("darwin")).unwrap();
        let linux = rows.iter().find(|r| r.name.contains("linux")).unwrap();
        assert_eq!(darwin.fits, Some(true));
        assert!(darwin.present, "installed package must report present");
        assert_eq!(linux.fits, Some(false));
        assert!(!linux.present);
        assert!(darwin.reasons.iter().any(|r| r.contains("host darwin")));
    }

    #[test]
    fn a_weight_path_is_resolved_against_the_model_directory_once() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("models/w");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("model.safetensors"), b"x").unwrap();
        let lock = crate::model_lock::ModelLock {
            schema: 1,
            models: [(
                "w".to_string(),
                crate::model_lock::ModelEntry {
                    source: "hf://x".into(),
                    revision: "abc".into(),
                    dir: "models/w".into(),
                    files: vec![crate::model_lock::ModelFile {
                        path: "model.safetensors".into(),
                        sha256: "00".into(),
                        size: 1,
                    }],
                },
            )]
            .into_iter()
            .collect(),
        };
        let (rows, plan) = model_artifacts(dir.path(), &lock);
        assert!(rows[0].present, "dir-qualified path must resolve once");
        assert_eq!(plan.cached_bytes, 1);
        assert_eq!(plan.missing_bytes, 0);
    }

    #[test]
    fn the_status_says_which_directory_it_describes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"name":"here"}"#).unwrap();
        let s = collect(dir.path());
        assert!(s.has_manifest);
        assert!(
            s.root.ends_with(dir.path().file_name().unwrap().to_str().unwrap()),
            "root must name the project: {}",
            s.root
        );
    }

    #[test]
    fn a_missing_weight_file_is_a_plan_item_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let lock = crate::model_lock::ModelLock {
            schema: 1,
            models: [(
                "w".to_string(),
                crate::model_lock::ModelEntry {
                    source: "hf://x".into(),
                    revision: "abc".into(),
                    dir: "models/w".into(),
                    files: vec![crate::model_lock::ModelFile {
                        path: "model.safetensors".into(),
                        sha256: "00".into(),
                        size: 1_000_000,
                    }],
                },
            )]
            .into_iter()
            .collect(),
        };
        let (rows, plan) = model_artifacts(dir.path(), &lock);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].present, "absent file must not report present");
        assert_eq!(rows[0].fits, None, "unread weights are not a pass");
        assert_eq!(plan.missing_bytes, 1_000_000);
        assert_eq!(plan.cached_bytes, 0);
    }
}
