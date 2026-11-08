//! Replay specifications — a runnable description of a published environment.
//!
//! A paper (or a teammate's "it worked last June") declares: these dependencies, as of
//! this date, with these models, on hardware like this. `jhol replay` turns that into
//! the commands that reproduce it, and refuses when the spec and the project disagree
//! instead of quietly editing the project.
//!
//! Parsing is pure and deterministic so a bad spec is a message, not a half-applied
//! environment.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct Paper {
    #[serde(default = "one")]
    pub schema: u32,
    #[serde(default)]
    pub name: Option<String>,
    /// Resolve as of this date (`YYYY-MM-DD` or RFC3339).
    #[serde(default)]
    pub as_of: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<PaperModel>,
    /// Declared hardware, used for a warning rather than a refusal: a replayed
    /// environment on different hardware is a legitimate experiment.
    #[serde(default)]
    pub axes: BTreeMap<String, String>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct PaperModel {
    pub source: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

impl Paper {
    pub fn parse(text: &str) -> Result<Self, String> {
        let paper: Paper = serde_json::from_str(text)
            .map_err(|e| format!("replay spec is not valid JSON: {e}"))?;
        paper.validate()?;
        Ok(paper)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("unsupported replay schema {}, expected 1", self.schema));
        }
        if self.dependencies.is_empty() && self.models.is_empty() {
            return Err("replay spec declares neither dependencies nor models".into());
        }
        if let Some(as_of) = &self.as_of {
            if !looks_like_date(as_of) {
                return Err(format!("as_of is not a date: {as_of}"));
            }
        }
        for m in &self.models {
            if !m.source.starts_with("hf://") {
                return Err(format!("model source must be hf://owner/repo, got {}", m.source));
            }
            if m.files.is_empty() {
                return Err(format!("model {} lists no files", m.source));
            }
        }
        for (name, spec) in &self.dependencies {
            if name.trim().is_empty() || spec.trim().is_empty() {
                return Err("dependency entries need a name and a spec".into());
            }
        }
        Ok(())
    }

    /// How the project differs from the spec, as `(name, current, wanted)`.
    pub fn diff_manifest(&self, current: &BTreeMap<String, String>) -> Vec<(String, Option<String>, String)> {
        let mut out = Vec::new();
        for (name, want) in &self.dependencies {
            let have = current.get(name);
            if have.map(|h| h.trim()) != Some(want.trim()) {
                out.push((name.clone(), have.cloned(), want.clone()));
            }
        }
        for name in current.keys() {
            if !self.dependencies.contains_key(name) {
                out.push((name.clone(), current.get(name).cloned(), "<absent in spec>".into()));
            }
        }
        out.sort();
        out
    }

    /// The commands that reproduce this spec, in order.
    pub fn plan(&self) -> Vec<String> {
        let mut cmds = Vec::new();
        let mut install = String::from("jhol install");
        if let Some(d) = &self.as_of {
            install.push_str(&format!(" --as-of {d}"));
        }
        cmds.push(install);
        for m in &self.models {
            let mut pull = format!("jhol ai pull --source {}", m.source);
            if let Some(rev) = &m.revision {
                pull.push_str(&format!(" --revision {rev}"));
            }
            for f in &m.files {
                pull.push_str(&format!(" --file {f}"));
            }
            cmds.push(pull);
        }
        cmds
    }
}

fn looks_like_date(s: &str) -> bool {
    let s = s.trim();
    if s.len() == 10 {
        let parts: Vec<&str> = s.split('-').collect();
        return parts.len() == 3
            && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    }
    s.contains('T') && s.len() >= 19
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_plans() {
        let p = Paper::parse(
            r#"{
                "name": "reproduce-2024",
                "as_of": "2024-06-01",
                "dependencies": {"torch": "2.4.1"},
                "models": [{"source": "hf://meta/llama", "revision": "abc", "files": ["model.safetensors"]}],
                "axes": {"cuda": "12.4"}
            }"#,
        )
        .unwrap();
        let plan = p.plan();
        assert_eq!(plan[0], "jhol install --as-of 2024-06-01");
        assert!(plan[1].contains("--source hf://meta/llama"));
        assert!(plan[1].contains("--file model.safetensors"));
    }

    #[test]
    fn rejects_nonsense_specs() {
        assert!(Paper::parse(r#"{"schema": 1, "as_of": "last june", "dependencies": {"a": "^1"}}"#).is_err());
        assert!(Paper::parse(r#"{"schema": 1, "dependencies": {}}"#).is_err());
        assert!(Paper::parse(r#"{"schema": 1, "models": [{"source": "http://x", "files": ["a"]}]}"#).is_err());
        assert!(Paper::parse(r#"{"schema": 1, "models": [{"source": "hf://a/b"}]}"#).is_err());
        assert!(Paper::parse("not json").is_err());
    }

    #[test]
    fn reports_manifest_drift_both_ways() {
        let p = Paper::parse(r#"{"schema":1,"dependencies":{"a":"^1","b":"^2"}}"#).unwrap();
        let mut current = BTreeMap::new();
        current.insert("a".to_string(), "^1".to_string()); // matches
        current.insert("b".to_string(), "^3".to_string()); // differs
        current.insert("extra".to_string(), "^1".to_string()); // not in spec
        let diff = p.diff_manifest(&current);
        assert_eq!(diff.len(), 2, "{diff:?}");
        assert_eq!(diff[0].0, "b");
        assert_eq!(diff[0].2, "^2");
        assert_eq!(diff[1].0, "extra");
    }
}
