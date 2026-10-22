//! Python adapter — uv first (blueprint §8). Pip mode remains a separate mode.

use crate::{
    execute_approved_plan, verify_receipt, AdapterCapabilities, AdapterError, AdapterIdentity,
    AdapterPlan, AdapterReceipt, ApprovedPlan, Declaration, EcosystemAdapter, NativeCommand,
    PlanRequest, ProjectContext, Verification,
};
use jhol_policy::Capability;

pub struct UvPythonAdapter {
    version: String,
    uv_binary: String,
    python_binary: String,
}

impl UvPythonAdapter {
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uv_binary: "uv".into(),
            python_binary: if cfg!(windows) { "python" } else { "python3" }.into(),
        }
    }

    pub fn with_uv_binary(mut self, path: impl Into<String>) -> Self {
        self.uv_binary = path.into();
        self
    }

    pub fn with_python_binary(mut self, path: impl Into<String>) -> Self {
        self.python_binary = path.into();
        self
    }
}

impl Default for UvPythonAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl EcosystemAdapter for UvPythonAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            id: "jhol-adapter-python".into(),
            version: self.version.clone(),
            ecosystem: "python".into(),
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
        for name in ["pyproject.toml", "requirements.txt", "uv.lock"] {
            let p = project.root.join(name);
            if p.is_file() {
                out.push(Declaration {
                    path: p,
                    kind: "python".into(),
                    summary: format!("{name} present"),
                });
            }
        }
        Ok(out)
    }

    fn plan(&self, request: &PlanRequest) -> Result<AdapterPlan, AdapterError> {
        let mut declared_inputs: Vec<String> = ["pyproject.toml", "requirements.txt", "uv.lock"]
            .into_iter()
            .filter(|name| request.root.join(name).is_file())
            .map(str::to_owned)
            .collect();
        if declared_inputs.is_empty() {
            return Err(AdapterError::Msg(
                "no Python declaration found — adapter has nothing to plan".into(),
            ));
        }
        let has_project = declared_inputs.iter().any(|name| name == "pyproject.toml");
        let has_requirements = declared_inputs
            .iter()
            .any(|name| name == "requirements.txt");
        let has_lock = declared_inputs.iter().any(|name| name == "uv.lock");
        if !has_project && !has_requirements {
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
                    "uv.lock without pyproject.toml is recorded but cannot drive uv sync".into(),
                ],
                human_summary: "Record an existing uv lockfile without changing the environment"
                    .into(),
            });
        }
        let mut limitations = vec![
            "the host Python interpreter is selected from PATH; interpreter and wheel ABI are recorded only by uv's resulting environment".into(),
            "the action cache records evidence but does not replay virtual environments".into(),
        ];
        if !has_project {
            limitations.push("project metadata is unavailable without pyproject.toml".into());
        }
        if has_project
            && declared_inputs
                .iter()
                .any(|name| name == "requirements.txt")
        {
            limitations.push(
                "requirements.txt is discovered but uv sync follows pyproject.toml and uv.lock"
                    .into(),
            );
            declared_inputs.retain(|name| name != "requirements.txt");
        }
        if !has_project {
            declared_inputs.retain(|name| name != "uv.lock");
        }
        let venv_python = if cfg!(windows) {
            request.root.join(".venv/Scripts/python.exe")
        } else {
            request.root.join(".venv/bin/python")
        };
        let mut actions = Vec::new();
        let mut expected_writes = vec![".venv/".into()];
        let expected_outputs = if has_project {
            if !has_lock {
                expected_writes.push("uv.lock".into());
            }
            vec!["uv.lock".into(), ".venv/pyvenv.cfg".into()]
        } else {
            actions.push(NativeCommand {
                program: self.uv_binary.clone(),
                args: vec![
                    "venv".into(),
                    "--no-managed-python".into(),
                    "--python".into(),
                    self.python_binary.clone(),
                    request.root.join(".venv").to_string_lossy().into_owned(),
                ],
                version_args: vec!["--version".into()],
            });
            let mut args = vec![
                "pip".into(),
                "sync".into(),
                "--python".into(),
                venv_python.to_string_lossy().into_owned(),
                request
                    .root
                    .join("requirements.txt")
                    .to_string_lossy()
                    .into_owned(),
            ];
            if request.offline {
                args.insert(2, "--offline".into());
            }
            actions.push(NativeCommand {
                program: self.uv_binary.clone(),
                args,
                version_args: vec!["--version".into()],
            });
            vec![".venv/pyvenv.cfg".into()]
        };
        if has_project {
            let mut args = vec![
                "sync".into(),
                "--project".into(),
                request.root.to_string_lossy().into_owned(),
            ];
            if has_lock {
                args.push("--locked".into());
            }
            if request.offline {
                args.push("--offline".into());
            }
            actions.push(NativeCommand {
                program: self.uv_binary.clone(),
                args,
                version_args: vec!["--version".into()],
            });
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
        Ok(AdapterPlan {
            adapter: self.identity(),
            project_root: request.root.clone(),
            declared_inputs,
            expected_writes,
            expected_outputs,
            actions,
            cancellation: "terminal interrupts reach uv; descendant cleanup is host-dependent"
                .into(),
            network_access: !request.offline,
            script_execution_possible: true,
            permissions,
            cacheable: false,
            limitations,
            human_summary: if has_project {
                "Run uv sync from pyproject.toml, honoring uv.lock when present".into()
            } else {
                "Create a project-local .venv and synchronize requirements.txt with uv".into()
            },
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
    fn plan_lists_only_present_python_inputs() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("requirements.txt"), "requests==2.32.0\n").unwrap();
        let plan = UvPythonAdapter::new()
            .plan(&PlanRequest {
                root: root.path().to_path_buf(),
                dry_run: true,
                offline: false,
            })
            .unwrap();
        assert_eq!(plan.declared_inputs, ["requirements.txt"]);
        assert_eq!(plan.expected_writes, [".venv/"]);
        assert!(plan.network_access);
        assert!(plan.script_execution_possible);
        assert!(plan
            .actions
            .iter()
            .any(|action| action.args.contains(&"sync".into())));
    }
}
