//! Capability policy (blueprint §15).
//!
//! A plan is not consent to all side effects. Each capability is gated separately.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadOnlyInspect,
    LocalPackageInstall,
    LifecycleScript,
    NetworkFetch,
    RemoteUpload,
    ProcessLaunch,
    DatabaseMutation,
    ArbitraryExternalCommand,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Policy {
    /// Capabilities already approved for this session / invocation.
    pub allowed: HashSet<Capability>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("capability denied: {0:?}")]
    Denied(Capability),
}

impl Policy {
    pub fn deny_all() -> Self {
        Self::default()
    }

    pub fn allow(&mut self, cap: Capability) {
        self.allowed.insert(cap);
    }

    pub fn check(&self, cap: Capability) -> Result<(), PolicyError> {
        if self.allowed.contains(&cap) {
            Ok(())
        } else {
            Err(PolicyError::Denied(cap))
        }
    }

    pub fn check_all<'a>(
        &self,
        caps: impl IntoIterator<Item = &'a Capability>,
    ) -> Result<(), PolicyError> {
        for c in caps {
            self.check(*c)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_by_default() {
        let p = Policy::deny_all();
        assert_eq!(
            p.check(Capability::NetworkFetch),
            Err(PolicyError::Denied(Capability::NetworkFetch))
        );
    }

    #[test]
    fn allow_then_pass() {
        let mut p = Policy::deny_all();
        p.allow(Capability::ReadOnlyInspect);
        assert!(p.check(Capability::ReadOnlyInspect).is_ok());
    }
}
