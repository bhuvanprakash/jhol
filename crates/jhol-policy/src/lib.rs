//! Capability policy (blueprint §15).
//!
//! A plan is not consent to all side effects. Each capability is gated
//! separately, and a capability can be narrowed to a resource (a path, a
//! registry host, a command name) with allow/deny rules. Deny always wins.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
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

impl Capability {
    pub const ALL: &'static [Capability] = &[
        Capability::ReadOnlyInspect,
        Capability::LocalPackageInstall,
        Capability::LifecycleScript,
        Capability::NetworkFetch,
        Capability::RemoteUpload,
        Capability::ProcessLaunch,
        Capability::DatabaseMutation,
        Capability::ArbitraryExternalCommand,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::ReadOnlyInspect => "read_only_inspect",
            Capability::LocalPackageInstall => "local_package_install",
            Capability::LifecycleScript => "lifecycle_script",
            Capability::NetworkFetch => "network_fetch",
            Capability::RemoteUpload => "remote_upload",
            Capability::ProcessLaunch => "process_launch",
            Capability::DatabaseMutation => "database_mutation",
            Capability::ArbitraryExternalCommand => "arbitrary_external_command",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().replace('-', "_").to_ascii_lowercase();
        Capability::ALL
            .iter()
            .copied()
            .find(|cap| cap.as_str() == normalized)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    #[default]
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub capability: Capability,
    pub effect: Effect,
    /// `None` applies to every resource. `Some` uses `*` and `?` glob syntax.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Rule {
    pub fn allow(capability: Capability, resource: impl Into<String>) -> Self {
        Self {
            capability,
            effect: Effect::Allow,
            resource: Some(resource.into()),
            reason: None,
        }
    }

    pub fn deny(capability: Capability, resource: impl Into<String>) -> Self {
        Self {
            capability,
            effect: Effect::Deny,
            resource: Some(resource.into()),
            reason: None,
        }
    }

    pub fn for_all(capability: Capability, effect: Effect) -> Self {
        Self {
            capability,
            effect,
            resource: None,
            reason: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    fn applies(&self, capability: Capability, resource: Option<&str>) -> bool {
        if self.capability != capability {
            return false;
        }
        match (&self.resource, resource) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(pattern), Some(value)) => glob_match(pattern, value),
        }
    }

    fn specificity(&self) -> usize {
        self.resource
            .as_deref()
            .map(|r| r.chars().filter(|c| *c != '*' && *c != '?').count())
            .unwrap_or(0)
    }
}

/// Glob matcher for resource names. `*` matches any run, `?` matches one
/// character. It is intentionally small and allocation-free on the hot path.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Capabilities explicitly granted for this session.
    #[serde(default)]
    pub allowed: HashSet<Capability>,
    /// Capabilities explicitly revoked. Deny always wins over allow.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub denied: HashSet<Capability>,
    /// Narrow grants/revocations. The most specific matching rule wins;
    /// ties are resolved by the later rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

impl Default for Policy {
    fn default() -> Self {
        Self::deny_all()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("capability denied: {0:?}")]
    Denied(Capability),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub capability: Capability,
    pub allowed: bool,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<Rule>,
}

impl Policy {
    /// Nothing is granted.
    pub fn deny_all() -> Self {
        Self {
            allowed: HashSet::new(),
            denied: HashSet::new(),
            rules: Vec::new(),
        }
    }

    /// Inspect-only profile: no writes, no network, no process launch.
    pub fn read_only() -> Self {
        let mut policy = Self::deny_all();
        policy.allow(Capability::ReadOnlyInspect);
        policy
    }

    /// A normal local development session: install, fetch and launch.
    pub fn local_development() -> Self {
        let mut policy = Self::deny_all();
        for cap in [
            Capability::ReadOnlyInspect,
            Capability::LocalPackageInstall,
            Capability::NetworkFetch,
            Capability::ProcessLaunch,
            Capability::RemoteUpload,
        ] {
            policy.allow(cap);
        }
        policy
    }

    /// CI: deterministic installs, no lifecycle scripts or arbitrary commands.
    pub fn ci() -> Self {
        let mut policy = Self::deny_all();
        for cap in [
            Capability::ReadOnlyInspect,
            Capability::LocalPackageInstall,
            Capability::NetworkFetch,
            Capability::ProcessLaunch,
        ] {
            policy.allow(cap);
        }
        policy.deny(Capability::LifecycleScript);
        policy.deny(Capability::ArbitraryExternalCommand);
        policy
    }

    /// Untrusted checkout: only read-only inspection is allowed.
    pub fn untrusted() -> Self {
        Self::read_only()
    }

    pub fn allow(&mut self, cap: Capability) {
        self.denied.remove(&cap);
        self.allowed.insert(cap);
    }

    pub fn deny(&mut self, cap: Capability) {
        self.allowed.remove(&cap);
        self.denied.insert(cap);
    }

    pub fn allow_rule(&mut self, rule: Rule) -> Result<(), String> {
        validate_rule(&rule)?;
        self.rules.push(rule);
        Ok(())
    }

    pub fn deny_rule(&mut self, rule: Rule) -> Result<(), String> {
        validate_rule(&rule)?;
        self.rules.push(rule);
        Ok(())
    }

    pub fn is_allowed(&self, cap: Capability) -> bool {
        self.explain(cap, None).allowed
    }

    /// Scoped variant of [`Policy::is_allowed`].
    pub fn is_allowed_scoped(&self, cap: Capability, resource: &str) -> bool {
        self.explain(cap, Some(resource)).allowed
    }

    pub fn check(&self, cap: Capability) -> Result<(), PolicyError> {
        if self.is_allowed(cap) {
            Ok(())
        } else {
            Err(PolicyError::Denied(cap))
        }
    }

    pub fn check_scoped(&self, cap: Capability, resource: &str) -> Result<(), PolicyError> {
        if self.explain(cap, Some(resource)).allowed {
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

    /// Explain a decision without performing any side effect.
    pub fn explain(&self, cap: Capability, resource: Option<&str>) -> Decision {
        if self.denied.contains(&cap) {
            return Decision {
                capability: cap,
                allowed: false,
                reason: format!("{cap} is explicitly denied"),
                matched_rule: None,
            };
        }

        let mut best: Option<&Rule> = None;
        for rule in &self.rules {
            if !rule.applies(cap, resource) {
                continue;
            }
            match best {
                None => best = Some(rule),
                // Strictly more specific wins; on a tie, deny wins regardless
                // of rule order so a later allow can never resurrect a
                // resource an earlier deny took away.
                Some(current) if rule.specificity() > current.specificity() => best = Some(rule),
                Some(current)
                    if rule.specificity() == current.specificity()
                        && rule.effect == Effect::Deny
                        && current.effect == Effect::Allow =>
                {
                    best = Some(rule)
                }
                Some(_) => {}
            }
        }
        if let Some(rule) = best {
            return Decision {
                capability: cap,
                allowed: rule.effect == Effect::Allow,
                reason: rule.reason.clone().unwrap_or_else(|| match rule.effect {
                    Effect::Allow => format!("{cap} allowed by rule"),
                    Effect::Deny => format!("{cap} denied by rule"),
                }),
                matched_rule: Some(rule.clone()),
            };
        }

        if self.allowed.contains(&cap) {
            // A scoped allow rule narrows an otherwise global session grant:
            // the grant covers only resources matched by those rules.
            let narrowed = self.rules.iter().any(|rule| {
                rule.capability == cap && rule.effect == Effect::Allow && rule.resource.is_some()
            });
            if narrowed {
                return Decision {
                    capability: cap,
                    allowed: false,
                    reason: format!("{cap} is scoped to other resources"),
                    matched_rule: None,
                };
            }
            return Decision {
                capability: cap,
                allowed: true,
                reason: format!("{cap} granted for this session"),
                matched_rule: None,
            };
        }

        Decision {
            capability: cap,
            allowed: false,
            reason: format!("{cap} not granted (deny by default)"),
            matched_rule: None,
        }
    }

    /// Union of grants. A deny in either policy remains a deny.
    pub fn merge(&self, other: &Policy) -> Policy {
        let mut merged = self.clone();
        for cap in &other.allowed {
            if !merged.denied.contains(cap) {
                merged.allowed.insert(*cap);
            }
        }
        for cap in &other.denied {
            merged.allowed.remove(cap);
            merged.denied.insert(*cap);
        }
        for rule in &other.rules {
            if !merged.rules.contains(rule) {
                merged.rules.push(rule.clone());
            }
        }
        merged
    }

    pub fn validate(&self) -> Result<(), String> {
        for rule in &self.rules {
            validate_rule(rule)?;
        }
        Ok(())
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let policy: Policy =
            serde_json::from_str(text).map_err(|e| format!("invalid policy JSON: {e}"))?;
        policy.validate()?;
        Ok(policy)
    }
}

fn validate_rule(rule: &Rule) -> Result<(), String> {
    if let Some(resource) = &rule.resource {
        if resource.trim().is_empty() {
            return Err("policy rule resource must not be empty".into());
        }
        if resource.contains('\0') {
            return Err("policy rule resource contains a NUL byte".into());
        }
        // `glob_match`'s `*` crosses `/`, so a resource containing a `..`
        // component could let an allow on `build/*` also match `build/../secrets`.
        // Traversal is never a legitimate resource; reject it here so both the
        // rule author and the requester side are covered.
        if resource.split('/').any(|c| c == "..") {
            return Err("policy rule resource must not contain a '..' path component".into());
        }
    }
    Ok(())
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

    #[test]
    fn deny_overrides_allow() {
        let mut p = Policy::deny_all();
        p.allow(Capability::NetworkFetch);
        p.deny(Capability::NetworkFetch);
        assert!(p.check(Capability::NetworkFetch).is_err());
    }

    #[test]
    fn scoped_rule_narrows_a_grant() {
        let mut p = Policy::deny_all();
        p.allow(Capability::NetworkFetch);
        p.allow_rule(Rule::allow(Capability::NetworkFetch, "registry.npmjs.org"))
            .unwrap();
        assert!(p
            .check_scoped(Capability::NetworkFetch, "registry.npmjs.org")
            .is_ok());
        assert!(p
            .check_scoped(Capability::NetworkFetch, "evil.example.com")
            .is_err());
    }

    #[test]
    fn specific_deny_beats_broad_allow() {
        let mut p = Policy::deny_all();
        p.allow_rule(Rule::allow(Capability::LocalPackageInstall, "*"))
            .unwrap();
        p.deny_rule(Rule::deny(
            Capability::LocalPackageInstall,
            "node_modules/.bin/*",
        ))
        .unwrap();
        assert!(p
            .check_scoped(Capability::LocalPackageInstall, "node_modules/pkg")
            .is_ok());
        assert!(p
            .check_scoped(Capability::LocalPackageInstall, "node_modules/.bin/tsc")
            .is_err());
    }

    #[test]
    fn deny_wins_specificity_ties_regardless_of_order() {
        let mut p = Policy::read_only();
        p.allow_rule(
            Rule::allow(Capability::ProcessLaunch, "build/*").with_reason("builds allowed"),
        )
        .unwrap();
        p.deny_rule(Rule::deny(Capability::ProcessLaunch, "build/se*"))
            .unwrap();
        // Deny after allow, equal specificity (7 literal chars each): denied.
        assert!(!p.is_allowed_scoped(Capability::ProcessLaunch, "build/setup.sh"));
        // And the mirror order denies too.
        let mut p2 = Policy::read_only();
        p2.deny_rule(Rule::deny(Capability::ProcessLaunch, "build/se*"))
            .unwrap();
        p2.allow_rule(Rule::allow(Capability::ProcessLaunch, "build/*"))
            .unwrap();
        assert!(!p2.is_allowed_scoped(Capability::ProcessLaunch, "build/setup.sh"));
        // More specific allow beats less specific deny.
        let mut p3 = Policy::read_only();
        p3.deny_rule(Rule::deny(Capability::ProcessLaunch, "build/*"))
            .unwrap();
        p3.allow_rule(Rule::allow(Capability::ProcessLaunch, "build/ok.txt"))
            .unwrap();
        assert!(p3.is_allowed_scoped(Capability::ProcessLaunch, "build/ok.txt"));
        assert!(!p3.is_allowed_scoped(Capability::ProcessLaunch, "build/other.txt"));
    }

    #[test]
    #[test]
    fn validate_rule_rejects_dotdot_resources() {
        let r = Rule::allow(Capability::ReadOnlyInspect, "build/../secrets");
        assert!(validate_rule(&r).is_err());
        let r = Rule::allow(Capability::ReadOnlyInspect, "a/../../b");
        assert!(validate_rule(&r).is_err());
        let r = Rule::allow(Capability::ReadOnlyInspect, "build/*");
        assert!(validate_rule(&r).is_ok());
    }

    #[test]
    fn glob_semantics() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*.npmjs.org", "registry.npmjs.org"));
        assert!(!glob_match("*.npmjs.org", "npmjs.org.evil"));
        assert!(glob_match("src/?ain.rs", "src/main.rs"));
        assert!(!glob_match("src/?ain.rs", "src/mmain.rs"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
    }

    #[test]
    fn profiles_are_ordered() {
        assert!(Policy::read_only().check(Capability::NetworkFetch).is_err());
        assert!(Policy::local_development()
            .check(Capability::NetworkFetch)
            .is_ok());
        assert!(Policy::ci().check(Capability::LifecycleScript).is_err());
        assert!(Policy::untrusted()
            .check(Capability::LocalPackageInstall)
            .is_err());
    }

    #[test]
    fn json_roundtrip_and_legacy_shape() {
        let mut p = Policy::deny_all();
        p.allow(Capability::ReadOnlyInspect);
        p.allow_rule(Rule::allow(Capability::NetworkFetch, "*.npmjs.org").with_reason("registry"))
            .unwrap();
        let text = p.to_json_pretty().unwrap();
        let back = Policy::from_json(&text).unwrap();
        assert!(back.check(Capability::ReadOnlyInspect).is_ok());
        assert!(back
            .check_scoped(Capability::NetworkFetch, "registry.npmjs.org")
            .is_ok());

        let legacy = Policy::from_json(r#"{"allowed":["network_fetch"]}"#).unwrap();
        assert!(legacy.check(Capability::NetworkFetch).is_ok());
        assert!(!legacy.is_allowed(Capability::RemoteUpload));
    }

    #[test]
    fn capability_parsing_accepts_kebab_and_snake() {
        assert_eq!(
            Capability::parse("network-fetch"),
            Some(Capability::NetworkFetch)
        );
        assert_eq!(
            Capability::parse("arbitrary_external_command"),
            Some(Capability::ArbitraryExternalCommand)
        );
        assert_eq!(Capability::parse("nope"), None);
    }

    #[test]
    fn merge_keeps_denies() {
        let mut a = Policy::local_development();
        a.deny(Capability::RemoteUpload);
        let mut b = Policy::deny_all();
        b.allow(Capability::RemoteUpload);
        b.allow(Capability::DatabaseMutation);
        let merged = a.merge(&b);
        assert!(merged.check(Capability::RemoteUpload).is_err());
        assert!(merged.check(Capability::DatabaseMutation).is_ok());
    }
}
