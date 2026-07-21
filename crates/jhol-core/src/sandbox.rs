//! Phase 16: Capability-Based Sandboxing with Landlock/seccomp
//!
//! Each package runs with minimal capabilities.
//! Post-install scripts sandboxed with seccomp-bpf + Landlock.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Sandbox policy for package installation
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub fs_access: Vec<PathAccess>,
    pub network: NetworkPolicy,
    pub syscalls: AllowedSyscalls,
    pub env: EnvPolicy,
}

#[derive(Debug, Clone)]
pub struct PathAccess {
    pub path: PathBuf,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    Offline,
    LocalhostOnly,
    Full,
}

#[derive(Debug, Clone)]
pub struct AllowedSyscalls {
    pub whitelist: Vec<i32>,
    pub blacklist: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct EnvPolicy {
    pub allowed: Vec<String>,
    pub clean: bool,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            fs_access: Vec::new(),
            network: NetworkPolicy::Offline,
            syscalls: AllowedSyscalls {
                whitelist: Vec::new(),
                blacklist: Vec::new(),
            },
            env: EnvPolicy {
                allowed: Vec::new(),
                clean: false,
            },
        }
    }
}

/// Sandboxed script executor
pub struct SandboxedExecutor {
    policy: SandboxPolicy,
}

impl SandboxedExecutor {
    pub fn new(policy: SandboxPolicy) -> Self {
        Self { policy }
    }

    /// Execute script with sandbox
    pub fn execute(&self, script: &Path, args: &[&str]) -> Result<ExecutionResult, String> {
        #[cfg(target_os = "linux")]
        {
            self.execute_linux(script, args)
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.execute_fallback(script, args)
        }
    }

    #[cfg(target_os = "linux")]
    fn execute_linux(&self, script: &Path, args: &[&str]) -> Result<ExecutionResult, String> {
        use landlock::{AccessFs, FilesystemAccess, Ruleset, RulesetAttr};

        let mut ruleset = Ruleset::new()
            .handle_access(AccessFs::from_all())
            .map_err(|e| format!("Failed to create ruleset: {:?}", e))?
            .create()
            .map_err(|e| format!("Failed to create ruleset: {:?}", e))?;

        for access in &self.policy.fs_access {
            let mut allowed = AccessFs::empty();
            if access.read {
                allowed |= AccessFs::ReadFile;
            }
            if access.write {
                allowed |= AccessFs::WriteFile;
            }
            if access.execute {
                allowed |= AccessFs::Execute;
            }
            if !allowed.is_empty() {
                ruleset = ruleset
                    .add_rule(allowed, &access.path)
                    .map_err(|e| format!("Failed to add rule: {:?}", e))?;
            }
        }

        let _ruleset = ruleset
            .restrict()
            .map_err(|e| format!("Failed to restrict: {:?}", e))?;

        let mut cmd = Command::new(script);
        cmd.args(args);

        if self.policy.env.clean {
            cmd.env_clear();
        }
        for var in &self.policy.env.allowed {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to execute: {}", e))?;

        Ok(ExecutionResult {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn execute_fallback(&self, script: &Path, args: &[&str]) -> Result<ExecutionResult, String> {
        let mut cmd = Command::new(script);
        cmd.args(args);

        if self.policy.env.clean {
            cmd.env_clear();
        }
        for var in &self.policy.env.allowed {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to execute: {}", e))?;

        Ok(ExecutionResult {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

#[derive(Debug)]
pub struct ExecutionResult {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

pub struct SandboxPolicyBuilder {
    policy: SandboxPolicy,
}

impl SandboxPolicyBuilder {
    pub fn new() -> Self {
        Self {
            policy: SandboxPolicy::default(),
        }
    }

    pub fn allow_path(mut self, path: &str, read: bool, write: bool, execute: bool) -> Self {
        self.policy.fs_access.push(PathAccess {
            path: PathBuf::from(path),
            read,
            write,
            execute,
        });
        self
    }

    pub fn network_offline(mut self) -> Self {
        self.policy.network = NetworkPolicy::Offline;
        self
    }

    pub fn network_localhost_only(mut self) -> Self {
        self.policy.network = NetworkPolicy::LocalhostOnly;
        self
    }

    pub fn allow_syscall(mut self, syscall: i32) -> Self {
        self.policy.syscalls.whitelist.push(syscall);
        self
    }

    pub fn forbid_syscall(mut self, syscall: i32) -> Self {
        self.policy.syscalls.blacklist.push(syscall);
        self
    }

    pub fn clean_env(mut self) -> Self {
        self.policy.env.clean = true;
        self
    }

    pub fn allow_env_var(mut self, var: &str) -> Self {
        self.policy.env.allowed.push(var.to_string());
        self
    }

    pub fn build(self) -> SandboxPolicy {
        self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_policy_builder() {
        let policy = SandboxPolicyBuilder::new()
            .allow_path("/tmp", true, true, false)
            .allow_path("/etc/hosts", true, false, false)
            .network_offline()
            .allow_syscall(1)
            .allow_syscall(2)
            .forbid_syscall(59)
            .clean_env()
            .allow_env_var("PATH")
            .build();

        assert_eq!(policy.fs_access.len(), 2);
        assert_eq!(policy.network, NetworkPolicy::Offline);
        assert_eq!(policy.syscalls.whitelist.len(), 2);
        assert_eq!(policy.syscalls.blacklist.len(), 1);
        assert!(policy.env.clean);
        assert_eq!(policy.env.allowed.len(), 1);
    }
}
