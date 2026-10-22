//! Rust / Cargo workspace adapter (blueprint §8).

use crate::{
    execute_approved_plan, verify_receipt, AdapterCapabilities, AdapterError, AdapterIdentity,
    AdapterPlan, AdapterReceipt, ApprovedPlan, Declaration, EcosystemAdapter, NativeCommand,
    PlanRequest, ProjectContext, Verification,
};
use jhol_policy::Capability;

pub struct CargoRustAdapter {
    version: String,
    cargo_binary: String,
}

impl CargoRustAdapter {
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            cargo_binary: "cargo".into(),
        }
    }

    pub fn with_cargo_binary(mut self, path: impl Into<String>) -> Self {
        self.cargo_binary = path.into();
        self
    }
}

impl Default for CargoRustAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl EcosystemAdapter for CargoRustAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            id: "jhol-adapter-rust".into(),
            version: self.version.clone(),
            ecosystem: "rust".into(),
        }
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            discover: true,
            plan: true,
            apply: true,
            verify: true,
            ..Default::default()
        }
    }

    fn discover(&self, project: &ProjectContext) -> Result<Vec<Declaration>, AdapterError> {
        let mut out = Vec::new();
        for name in ["Cargo.toml", "Cargo.lock"] {
            let p = project.root.join(name);
            if p.is_file() {
                out.push(Declaration {
                    path: p,
                    kind: "rust".into(),
                    summary: format!("{name} present"),
                });
            }
        }
        Ok(out)
    }

    fn plan(&self, request: &PlanRequest) -> Result<AdapterPlan, AdapterError> {
        let declared_inputs: Vec<String> = ["Cargo.toml", "Cargo.lock"]
            .into_iter()
            .filter(|name| request.root.join(name).is_file())
            .map(str::to_owned)
            .collect();
        if declared_inputs.is_empty() {
            return Err(AdapterError::Msg(
                "no Cargo manifest or lockfile found — adapter has nothing to plan".into(),
            ));
        }
        let has_manifest = declared_inputs.iter().any(|name| name == "Cargo.toml");
        let has_lock = declared_inputs.iter().any(|name| name == "Cargo.lock");
        if !has_manifest {
            return Ok(AdapterPlan {
                adapter: self.identity(),
                project_root: request.root.clone(),
                declared_inputs,
                expected_writes: Vec::new(),
                expected_outputs: Vec::new(),
                actions: Vec::new(),
                cancellation: "no native process is launched".into(),
                network_access: false,
                script_execution_possible: false,
                permissions: vec![Capability::ReadOnlyInspect],
                cacheable: false,
                limitations: vec![
                    "Cargo.lock exists without Cargo.toml; no build action is available".into(),
                ],
                human_summary: "Record the Cargo lockfile; a manifest is required before applying"
                    .into(),
            });
        }
        let limitations = vec![
            "the action builds Cargo's default target and feature set; target triples and explicit feature selection are not modeled".into(),
            "build scripts and procedural macros execute as part of Cargo build and require explicit approval".into(),
            "the action cache records evidence but does not replay target/".into(),
        ];
        let mut args = vec![
            "build".into(),
            "--workspace".into(),
            "--manifest-path".into(),
            request
                .root
                .join("Cargo.toml")
                .to_string_lossy()
                .into_owned(),
        ];
        if has_lock {
            args.push("--locked".into());
        }
        if request.offline {
            args.push("--offline".into());
        }
        let mut permissions = vec![
            Capability::ReadOnlyInspect,
            Capability::LocalPackageInstall,
            Capability::ProcessLaunch,
            Capability::LifecycleScript,
            Capability::ArbitraryExternalCommand,
        ];
        if !request.offline {
            permissions.push(Capability::NetworkFetch);
        }
        let mut expected_writes = vec!["target/".into()];
        if !has_lock {
            expected_writes.push("Cargo.lock".into());
        }
        Ok(AdapterPlan {
            adapter: self.identity(),
            project_root: request.root.clone(),
            declared_inputs,
            expected_writes,
            expected_outputs: vec!["Cargo.lock".into()],
            actions: vec![NativeCommand {
                program: self.cargo_binary.clone(),
                args,
                version_args: vec!["--version".into()],
            }],
            cancellation: "terminal interrupts reach Cargo; descendant cleanup is host-dependent"
                .into(),
            network_access: !request.offline,
            script_execution_possible: true,
            permissions,
            cacheable: false,
            limitations,
            human_summary: "Build the Cargo workspace with its native manifest and lock semantics"
                .into(),
        })
    }

    fn apply(&self, approved: &ApprovedPlan) -> Result<AdapterReceipt, AdapterError> {
        execute_approved_plan(&self.identity(), approved)
    }

    fn verify(&self, receipt: &AdapterReceipt) -> Result<Verification, AdapterError> {
        Ok(verify_receipt(&self.identity(), receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plan_records_lock_only_without_inventing_manifest() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("Cargo.lock"), "version = 4\n").unwrap();
        let plan = CargoRustAdapter::new()
            .plan(&PlanRequest {
                root: root.path().to_path_buf(),
                dry_run: true,
                offline: false,
            })
            .unwrap();
        assert_eq!(plan.declared_inputs, ["Cargo.lock"]);
        assert!(plan.expected_writes.is_empty());
        assert!(plan.actions.is_empty());
        assert!(plan
            .permissions
            .iter()
            .all(|cap| *cap == Capability::ReadOnlyInspect));
        assert!(plan
            .limitations
            .iter()
            .any(|lim| lim.contains("without Cargo.toml")));
    }
}
