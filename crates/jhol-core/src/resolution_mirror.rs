//! JMR: JAGR Mirror Resolution — stub for compatibility.
pub fn global_mirror() -> Mirror { Mirror }
pub struct Mirror;
impl Mirror {
    pub fn stats(&self) -> MirrorStats { MirrorStats::default() }
    pub fn prune(&self) -> Result<usize, String> { Ok(0) }
    pub fn clear(&self) -> Result<(), String> { Ok(()) }
    pub fn lookup(&self, _name: &str, _range: &str) -> Option<MirrorEntry> { None }
    pub fn predict_full_install(&self, _packages: &[(String, String)]) -> Prediction { Prediction::default() }
}
#[derive(Default)]
pub struct MirrorStats {
    pub total_entries: usize,
    pub fresh_entries: usize,
    pub expired_entries: usize,
}
pub struct MirrorEntry {
    pub resolved_version: String,
    pub dep_signature: String,
    pub dependencies: std::collections::HashMap<String, String>,
    pub tarball_url: Option<String>,
    pub integrity: Option<String>,
}
#[derive(Default)]
pub struct Prediction {
    pub coverage: f64,
    pub predicted_selection: std::collections::HashMap<String, String>,
    pub missing_packages: Vec<String>,
    pub predicted_tarball_urls: std::collections::HashMap<String, Option<String>>,
    pub predicted_integrity: std::collections::HashMap<String, Option<String>>,
}
