//! Ecosystem adapter protocol (blueprint §8).
//!
//! The current JavaScript, Python/uv, and Rust/Cargo adapters live as modules
//! here. Separate ecosystem crates remain deferred until their contracts and
//! release boundaries justify that split.

#![forbid(unsafe_code)]

pub mod js;
pub mod python;
pub mod rust_lang;

use jhol_policy::Capability;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterIdentity {
    pub id: String,
    pub version: String,
    pub ecosystem: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    pub discover: bool,
    pub plan: bool,
    pub apply: bool,
    pub verify: bool,
    pub mutate_database: bool,
    pub run_scripts: bool,
    pub network: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Declaration {
    pub path: PathBuf,
    pub kind: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRequest {
    pub root: PathBuf,
    /// When true, adapters must not mutate the filesystem or network.
    pub dry_run: bool,
    /// Propagate an offline constraint to native package tools.
    pub offline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeCommand {
    pub program: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterPlan {
    pub adapter: AdapterIdentity,
    pub project_root: PathBuf,
    pub declared_inputs: Vec<String>,
    pub expected_writes: Vec<String>,
    pub expected_outputs: Vec<String>,
    pub actions: Vec<NativeCommand>,
    /// Terminal interrupts reach the native child; descendant cleanup remains host-dependent.
    pub cancellation: String,
    pub network_access: bool,
    pub script_execution_possible: bool,
    pub permissions: Vec<Capability>,
    pub cacheable: bool,
    pub limitations: Vec<String>,
    pub human_summary: String,
}

#[derive(Debug, Clone)]
pub struct ApprovedPlan {
    plan: AdapterPlan,
    grants: HashSet<Capability>,
}

impl ApprovedPlan {
    /// Build an execution approval only after every planned capability passes policy.
    pub fn authorize(
        plan: AdapterPlan,
        policy: &jhol_policy::Policy,
    ) -> Result<Self, AdapterError> {
        policy
            .check_all(plan.permissions.iter())
            .map_err(|e| AdapterError::Msg(format!("plan policy denied: {e}")))?;
        Ok(Self {
            plan,
            grants: policy.allowed.clone(),
        })
    }

    pub fn plan(&self) -> &AdapterPlan {
        &self.plan
    }

    fn permits(&self, capability: Capability) -> bool {
        self.grants.contains(&capability)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterReceipt {
    pub adapter: AdapterIdentity,
    pub project_root: PathBuf,
    pub tool_versions: serde_json::Map<String, serde_json::Value>,
    pub outputs: Vec<jhol_receipt::DigestedPath>,
    pub exit_status: i32,
    pub warnings: Vec<String>,
    pub unverified_assumptions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub ok: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("{0}")]
    Msg(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Execute concrete commands only after their plan capabilities were approved.
/// Commands are passed as argument arrays and never interpreted by a shell.
pub fn execute_approved_plan(
    expected_adapter: &AdapterIdentity,
    approved: &ApprovedPlan,
) -> Result<AdapterReceipt, AdapterError> {
    let plan = &approved.plan;
    if plan.adapter.id != expected_adapter.id || plan.adapter.version != expected_adapter.version {
        return Err(AdapterError::Msg(
            "approved plan belongs to another adapter version".into(),
        ));
    }
    for capability in &plan.permissions {
        if !approved.permits(*capability) {
            return Err(AdapterError::Msg(format!(
                "approved plan is missing capability: {capability:?}"
            )));
        }
    }
    if !plan.project_root.is_dir() {
        return Err(AdapterError::Msg(format!(
            "project root is not a directory: {}",
            plan.project_root.display()
        )));
    }
    validate_planned_writes(plan)?;
    if plan.actions.is_empty() {
        return Err(AdapterError::Msg(
            "approved plan has no executable action".into(),
        ));
    }

    let mut tool_versions = serde_json::Map::new();
    for action in &plan.actions {
        let version = bounded_tool_version(action, &plan.project_root);
        let status = Command::new(&action.program)
            .args(&action.args)
            .current_dir(&plan.project_root)
            .status()
            .map_err(|e| {
                AdapterError::Msg(format!(
                    "could not start {}: {e}",
                    Path::new(&action.program)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("native tool")
                ))
            })?;
        if !status.success() {
            return Err(AdapterError::Msg(format!(
                "{} exited with status {}",
                Path::new(&action.program)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("native tool"),
                status.code().unwrap_or(-1)
            )));
        }
        if let Some(version) = version {
            let name = Path::new(&action.program)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&action.program);
            tool_versions.insert(name.to_string(), serde_json::Value::String(version));
        }
    }

    let mut outputs = Vec::new();
    for relative in &plan.expected_outputs {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
        {
            return Err(AdapterError::Msg(format!(
                "planned output escapes project root: {relative}"
            )));
        }
        let path = plan.project_root.join(relative);
        if !path.is_file() {
            return Err(AdapterError::Msg(format!(
                "expected output was not created: {relative}"
            )));
        }
        let digest = jhol_graph::digest_file(&path)
            .map_err(|e| AdapterError::Msg(format!("digest {relative}: {e}")))?;
        outputs.push(jhol_receipt::DigestedPath {
            path: relative.clone(),
            digest,
            verified: Some(true),
        });
    }

    Ok(AdapterReceipt {
        adapter: plan.adapter.clone(),
        project_root: plan.project_root.clone(),
        tool_versions,
        outputs,
        exit_status: 0,
        warnings: Vec::new(),
        unverified_assumptions: plan.limitations.clone(),
    })
}

fn validate_planned_writes(plan: &AdapterPlan) -> Result<(), AdapterError> {
    for relative in &plan.expected_writes {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(AdapterError::Msg(format!(
                "planned write escapes project root: {relative}"
            )));
        }
        let mut current = plan.project_root.clone();
        for component in relative_path.components() {
            current.push(component.as_os_str());
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(AdapterError::Msg(format!(
                        "planned write crosses a symlink: {}",
                        current.display()
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(AdapterError::Msg(format!(
                        "inspect {}: {error}",
                        current.display()
                    )))
                }
            }
        }
    }
    Ok(())
}

fn bounded_tool_version(action: &NativeCommand, root: &Path) -> Option<String> {
    if action.version_args.is_empty() {
        return None;
    }
    let mut child = Command::new(&action.program)
        .args(&action.version_args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::with_capacity(256);
        let _ = stdout.take(4096).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let bytes = reader.join().ok()?;
    String::from_utf8_lossy(&bytes)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Recheck the adapter identity and every file digest recorded in a receipt.
pub fn verify_receipt(
    expected_adapter: &AdapterIdentity,
    receipt: &AdapterReceipt,
) -> Verification {
    let mut notes = Vec::new();
    if receipt.adapter.id != expected_adapter.id
        || receipt.adapter.version != expected_adapter.version
    {
        notes.push("receipt adapter identity does not match".into());
    }
    if receipt.exit_status != 0 {
        notes.push(format!(
            "native command exited with status {}",
            receipt.exit_status
        ));
    }
    for output in &receipt.outputs {
        let path = receipt.project_root.join(&output.path);
        let digest = path
            .is_file()
            .then(|| jhol_graph::digest_file(&path).ok())
            .flatten();
        if digest.as_deref() != Some(output.digest.as_str()) || output.verified != Some(true) {
            notes.push(format!(
                "output digest verification failed: {}",
                output.path
            ));
        }
    }
    Verification {
        ok: notes.is_empty(),
        notes,
    }
}

pub trait EcosystemAdapter: Send + Sync {
    fn identity(&self) -> AdapterIdentity;
    fn capabilities(&self) -> AdapterCapabilities;
    fn discover(&self, project: &ProjectContext) -> Result<Vec<Declaration>, AdapterError>;
    fn plan(&self, request: &PlanRequest) -> Result<AdapterPlan, AdapterError>;
    fn apply(&self, approved: &ApprovedPlan) -> Result<AdapterReceipt, AdapterError>;
    fn verify(&self, receipt: &AdapterReceipt) -> Result<Verification, AdapterError>;
}

/// Registry of available adapters for a process.
#[derive(Default)]
pub struct AdapterRegistry {
    adapters: Vec<Box<dyn EcosystemAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: Box<dyn EcosystemAdapter>) {
        self.adapters.push(adapter);
    }

    pub fn default_local() -> Self {
        let mut r = Self::new();
        r.register(Box::new(js::JsNativeAdapter::new()));
        r.register(Box::new(python::UvPythonAdapter::new()));
        r.register(Box::new(rust_lang::CargoRustAdapter::new()));
        r
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn EcosystemAdapter> {
        self.adapters.iter().map(|a| a.as_ref())
    }

    pub fn discover_all(&self, root: &Path) -> Result<Vec<Declaration>, AdapterError> {
        let ctx = ProjectContext {
            root: root.to_path_buf(),
        };
        let mut out = Vec::new();
        for a in self.iter() {
            out.extend(a.discover(&ctx)?);
        }
        Ok(out)
    }

    pub fn plan_all(&self, root: &Path, dry_run: bool) -> Result<Vec<AdapterPlan>, AdapterError> {
        self.plan_all_with_options(root, dry_run, false)
    }

    pub fn plan_all_with_options(
        &self,
        root: &Path,
        dry_run: bool,
        offline: bool,
    ) -> Result<Vec<AdapterPlan>, AdapterError> {
        let req = PlanRequest {
            root: root.to_path_buf(),
            dry_run,
            offline,
        };
        let mut out = Vec::new();
        for a in self.iter() {
            // Skip adapters with nothing to discover for this root.
            let decls = a.discover(&ProjectContext {
                root: root.to_path_buf(),
            })?;
            if decls.is_empty() {
                continue;
            }
            out.push(a.plan(&req)?);
        }
        Ok(out)
    }

    pub fn authorize_all(
        plans: Vec<AdapterPlan>,
        policy: &jhol_policy::Policy,
    ) -> Result<Vec<ApprovedPlan>, AdapterError> {
        plans
            .into_iter()
            .map(|plan| ApprovedPlan::authorize(plan, policy))
            .collect()
    }

    /// Check all native executables before the first adapter can make changes.
    pub fn preflight(&self, approved: &ApprovedPlan) -> Result<(), AdapterError> {
        let adapter = self
            .iter()
            .find(|adapter| adapter.identity().id == approved.plan.adapter.id)
            .ok_or_else(|| {
                AdapterError::Msg(format!(
                    "adapter not registered: {}",
                    approved.plan.adapter.id
                ))
            })?;
        if adapter.identity().version != approved.plan.adapter.version {
            return Err(AdapterError::Msg(
                "approved plan belongs to another adapter version".into(),
            ));
        }
        if !approved.plan.project_root.is_dir() {
            return Err(AdapterError::Msg(format!(
                "project root is not a directory: {}",
                approved.plan.project_root.display()
            )));
        }
        validate_planned_writes(&approved.plan)?;
        for capability in &approved.plan.permissions {
            if !approved.permits(*capability) {
                return Err(AdapterError::Msg(format!(
                    "approved plan is missing capability: {capability:?}"
                )));
            }
        }
        for action in &approved.plan.actions {
            if action.version_args.is_empty()
                || bounded_tool_version(action, &approved.plan.project_root).is_none()
            {
                return Err(AdapterError::Msg(format!(
                    "could not verify native tool before apply: {}",
                    Path::new(&action.program)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("native tool")
                )));
            }
        }
        Ok(())
    }

    pub fn apply(&self, approved: &ApprovedPlan) -> Result<AdapterReceipt, AdapterError> {
        let adapter = self
            .iter()
            .find(|adapter| adapter.identity().id == approved.plan.adapter.id)
            .ok_or_else(|| {
                AdapterError::Msg(format!(
                    "adapter not registered: {}",
                    approved.plan.adapter.id
                ))
            })?;
        let receipt = adapter.apply(approved)?;
        let verification = adapter.verify(&receipt)?;
        if !verification.ok {
            return Err(AdapterError::Msg(format!(
                "{} output verification failed: {}",
                approved.plan.adapter.id,
                verification.notes.join("; ")
            )));
        }
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_package_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .unwrap();
        let reg = AdapterRegistry::default_local();
        let decls = reg.discover_all(dir.path()).unwrap();
        assert!(decls.iter().any(|d| d.kind == "javascript"));
    }

    #[test]
    fn authorize_all_denies_build_backends_before_any_apply() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\ndependencies = []\n",
        )
        .unwrap();
        let registry = AdapterRegistry::default_local();
        let plans = registry
            .plan_all_with_options(dir.path(), false, true)
            .unwrap();
        let mut policy = jhol_policy::Policy::deny_all();
        policy.allow(Capability::ReadOnlyInspect);
        policy.allow(Capability::LocalPackageInstall);
        policy.allow(Capability::ProcessLaunch);
        assert!(AdapterRegistry::authorize_all(plans, &policy).is_err());
        assert!(!dir.path().join(".venv").exists());
    }

    #[cfg(unix)]
    #[test]
    fn planned_writes_reject_symlinked_output_directories() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), r#"{"name":"demo"}"#).unwrap();
        symlink(outside.path(), dir.path().join("node_modules")).unwrap();
        let plan = js::JsNativeAdapter::new()
            .plan(&PlanRequest {
                root: dir.path().to_path_buf(),
                dry_run: true,
                offline: true,
            })
            .unwrap();
        assert!(validate_planned_writes(&plan)
            .unwrap_err()
            .to_string()
            .contains("symlink"));
    }

    #[test]
    fn empty_js_manifest_does_not_require_an_absent_lock_output() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), r#"{"name":"empty"}"#).unwrap();
        let plan = js::JsNativeAdapter::new()
            .plan(&PlanRequest {
                root: dir.path().to_path_buf(),
                dry_run: true,
                offline: true,
            })
            .unwrap();
        assert!(plan.expected_outputs.is_empty());
    }
}
