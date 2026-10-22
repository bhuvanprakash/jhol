//! Native JavaScript adapter — runs Jhol's existing install path as an approved
//! child command, with scripts disabled and output digests verified.

use crate::{
    execute_approved_plan, verify_receipt, AdapterCapabilities, AdapterError, AdapterIdentity,
    AdapterPlan, AdapterReceipt, ApprovedPlan, Declaration, EcosystemAdapter, NativeCommand,
    PlanRequest, ProjectContext, Verification,
};
use jhol_policy::Capability;
use std::path::Path;

pub struct JsNativeAdapter {
    version: String,
    jhol_binary: String,
}

impl JsNativeAdapter {
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            jhol_binary: std::env::current_exe()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "jhol".into()),
        }
    }

    pub fn with_binary(mut self, path: impl Into<String>) -> Self {
        self.jhol_binary = path.into();
        self
    }
}

impl Default for JsNativeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl EcosystemAdapter for JsNativeAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            id: "jhol-adapter-js".into(),
            version: self.version.clone(),
            ecosystem: "javascript".into(),
        }
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            discover: true,
            plan: true,
            apply: true,
            verify: true,
            mutate_database: false,
            run_scripts: true,
            network: true,
        }
    }

    fn discover(&self, project: &ProjectContext) -> Result<Vec<Declaration>, AdapterError> {
        let mut out = Vec::new();
        let pkg = project.root.join("package.json");
        if pkg.is_file() {
            out.push(Declaration {
                path: pkg,
                kind: "javascript".into(),
                summary: "package.json present".into(),
            });
        }
        for name in [
            "package-lock.json",
            "bun.lock",
            "yarn.lock",
            "pnpm-lock.yaml",
        ] {
            let p = project.root.join(name);
            if p.is_file() {
                out.push(Declaration {
                    path: p,
                    kind: "javascript-lock".into(),
                    summary: format!("{name} present"),
                });
            }
        }
        Ok(out)
    }

    fn plan(&self, request: &PlanRequest) -> Result<AdapterPlan, AdapterError> {
        let has_pkg = request.root.join("package.json").is_file();
        let mut declared_inputs = Vec::new();
        if has_pkg {
            declared_inputs.push("package.json".to_string());
        }
        for name in [
            "package-lock.json",
            "bun.lock",
            "yarn.lock",
            "pnpm-lock.yaml",
        ] {
            if request.root.join(name).is_file() {
                declared_inputs.push(name.into());
            }
        }
        if !has_pkg {
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
                limitations: vec!["a JavaScript lockfile without package.json is indexed but cannot drive an install".into()],
                human_summary: "Record existing JavaScript lockfiles without changing the environment".into(),
            });
        }
        let mut args = vec![
            "install".into(),
            "--native-only".into(),
            "--no-scripts".into(),
        ];
        if request.offline {
            args.push("--offline".into());
        }
        let mut permissions = vec![
            Capability::ReadOnlyInspect,
            Capability::LocalPackageInstall,
            Capability::ProcessLaunch,
        ];
        if !request.offline {
            permissions.push(Capability::NetworkFetch);
        }
        let lock_is_expected = request.root.join("package-lock.json").is_file()
            || package_declares_dependencies(&request.root.join("package.json"));
        Ok(AdapterPlan {
            adapter: self.identity(),
            project_root: request.root.clone(),
            declared_inputs,
            expected_writes: vec!["node_modules/".into(), "package-lock.json".into()],
            expected_outputs: if lock_is_expected {
                vec!["package-lock.json".into()]
            } else {
                Vec::new()
            },
            actions: vec![NativeCommand {
                program: self.jhol_binary.clone(),
                args,
                version_args: vec!["--version".into()],
            }],
            cancellation: "terminal interrupts reach the native process; descendant cleanup is host-dependent".into(),
            network_access: !request.offline,
            script_execution_possible: false,
            permissions,
            cacheable: false,
            limitations: vec![
                "JavaScript install uses Jhol's native resolver and lockfile; lifecycle scripts are disabled for this sync action".into(),
                "the action cache records evidence but does not replay installed trees".into(),
            ],
            human_summary: "Run Jhol's native install in the project root with scripts disabled".into(),
        })
    }

    fn apply(&self, approved: &ApprovedPlan) -> Result<AdapterReceipt, AdapterError> {
        execute_approved_plan(&self.identity(), approved)
    }

    fn verify(&self, receipt: &AdapterReceipt) -> Result<Verification, AdapterError> {
        Ok(verify_receipt(&self.identity(), receipt))
    }
}

fn package_declares_dependencies(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ]
    .iter()
    .any(|key| {
        manifest
            .get(key)
            .and_then(serde_json::Value::as_object)
            .is_some_and(|dependencies| !dependencies.is_empty())
    })
}

/// Helper for tests / future wiring.
pub fn looks_like_js_project(root: &Path) -> bool {
    root.join("package.json").is_file()
}
