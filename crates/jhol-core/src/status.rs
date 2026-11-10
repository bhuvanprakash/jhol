//! One screen of truth about a project.
//!
//! The CLI, the TUI and the web dashboard all render **this** struct. That is the point:
//! the TUI used to collect its own numbers, which is how two implementations of one fact
//! start disagreeing. Anything that wants to show project state reads this.

use std::path::Path;

use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default)]
pub struct Status {
    pub project: String,
    pub version: String,
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
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Proof {
    pub snapshot_ready: bool,
    pub attested_packages: usize,
    pub attested_models: usize,
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

/// Collect everything, cheaply. No network, no resolution: one lockfile parse, one tree
/// walk, and the files the artifact layers already keep.
pub fn collect(root: &Path) -> Status {
    let (project, version) = read_manifest(root);

    let state = crate::tree_state::verify_tree(root);
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

    let models = crate::model_lock::ModelLock::load(root)
        .map(|lock| Models {
            pinned: lock.models.len(),
            names: lock.models.keys().cloned().collect(),
        })
        .unwrap_or_default();

    let python = std::fs::read_to_string(root.join(".jhol/py/lock.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|v| {
            let n = v
                .get("distributions")
                .and_then(|d| d.as_object())
                .map(|d| d.len())
                .or_else(|| v.as_array().map(|a| a.len()))
                .unwrap_or(0);
            Python { pinned: n }
        })
        .unwrap_or_default();

    let axes = crate::axes::HostAxes::detect();

    let (attested_packages, attested_models) = crate::attest::Attestation::load(root)
        .map(|a| (a.packages.len(), a.models.len()))
        .unwrap_or((0, 0));

    Status {
        project,
        version,
        manifest_matches_lockfile: crate::install::lockfile_matches_manifest(root),
        tree: Tree {
            state: label.to_string(),
            detail,
            packages: crate::count_installed_packages(root),
        },
        policy: Policy {
            as_of: std::env::var("JHOL_AS_OF").ok(),
            allowed_licenses: std::env::var("JHOL_ALLOW_LICENSE")
                .ok()
                .map(|v| v.split(',').map(|s| s.trim().to_string()).collect()),
            exclude_vulnerable: std::env::var("JHOL_EXCLUDE_VULN").is_ok(),
        },
        models,
        python,
        axes: Axes {
            os: axes.os.clone(),
            arch: axes.arch.clone(),
            libc: axes.libc.to_string(),
            python: axes.python.clone(),
            cuda_driver: axes.cuda_driver.clone(),
        },
        proof: Proof {
            snapshot_ready: crate::install::snapshot_ready(root),
            attested_packages,
            attested_models,
        },
    }
}
