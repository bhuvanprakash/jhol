//! JHOL Pre-Resolved Package Index
//!
//! Bundled index of top packages for instant resolution.

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn lookup_version<'a>(
    package: Option<&'a PreResolvedPackage>,
    version_req: &str,
) -> Option<&'a PreResolvedPackage> {
    let pkg = package?;
    let req = version_req.trim();
    if req.is_empty() || req == "latest" || req == "*" {
        return Some(pkg);
    }
    if let Ok(exact) = Version::parse(req) {
        let latest = Version::parse(&pkg.version).ok()?;
        return (latest == exact).then_some(pkg);
    }
    if let Ok(parsed_req) = VersionReq::parse(req) {
        let latest = Version::parse(&pkg.version).ok()?;
        return parsed_req.matches(&latest).then_some(pkg);
    }
    None
}

/// Pre-resolved package entry
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreResolvedPackage {
    /// Package name
    pub name: String,
    /// Latest version
    pub version: String,
    /// All available versions
    pub versions: Vec<String>,
    /// Tarball URL
    pub tarball_url: String,
    /// Integrity hash
    pub integrity: String,
    /// Direct dependencies
    pub dependencies: HashMap<String, String>,
    /// Resolved at timestamp
    pub resolved_at: u64,
}

/// Pre-resolved package index (bundled with JHOL)
pub struct PackageIndex {
    /// Embedded public-registry packages.
    packages: HashMap<String, PreResolvedPackage>,
    /// User entries partitioned by registry identity.
    user_packages: HashMap<String, PreResolvedPackage>,
    /// Old entries have no origin. Retain them on disk, but do not use as proof.
    legacy_user_packages: HashMap<String, PreResolvedPackage>,
    /// Index file path
    index_path: PathBuf,
}

impl PackageIndex {
    /// Create new package index
    pub fn new(index_path: PathBuf) -> Self {
        let mut index = Self {
            packages: HashMap::new(),
            user_packages: HashMap::new(),
            legacy_user_packages: HashMap::new(),
            index_path,
        };

        index.load_bundled_index();
        index.load_user_index();
        index
    }

    /// Load bundled index (shipped with JHOL)
    fn load_bundled_index(&mut self) {
        if let Some(embedded) = Self::load_embedded_index() {
            self.packages = embedded;
            return;
        }

        let bundled_path = self.index_path.join("bundled-index.json");
        if bundled_path.exists() {
            if let Ok(content) = fs::read_to_string(&bundled_path) {
                if let Ok(packages) =
                    serde_json::from_str::<HashMap<String, PreResolvedPackage>>(&content)
                {
                    self.packages = packages;
                }
            }
        }
    }

    /// Load embedded index (compiled into binary)
    fn load_embedded_index() -> Option<HashMap<String, PreResolvedPackage>> {
        const BUNDLED_INDEX: &[u8] = include_bytes!("../data/bundled-index.json");
        serde_json::from_slice(BUNDLED_INDEX).ok()
    }

    /// Load user index (updated by user)
    fn load_user_index(&mut self) {
        let user_path = self.index_path.join("user-index.json");
        if user_path.exists() {
            if let Ok(content) = fs::read_to_string(&user_path) {
                if let Ok(user_packages) =
                    serde_json::from_str::<HashMap<String, PreResolvedPackage>>(&content)
                {
                    for (key, package) in user_packages {
                        if key.starts_with("metadata-v2:") && key.contains('|') {
                            self.user_packages.insert(key, package);
                        } else {
                            self.legacy_user_packages.insert(key, package);
                        }
                    }
                }
            }
        }
    }

    /// Save user index
    pub fn save_user_index(&self) -> Result<(), String> {
        let user_path = self.index_path.join("user-index.json");

        let mut persisted = self.legacy_user_packages.clone();
        persisted.extend(self.user_packages.clone());
        let content = serde_json::to_string(&persisted)
            .map_err(|e| format!("Failed to serialize index: {}", e))?;

        fs::write(&user_path, content).map_err(|e| format!("Failed to write index: {}", e))?;
        Ok(())
    }

    /// Get all packages (for prefetch optimization)
    pub fn all_packages(&self) -> &HashMap<String, PreResolvedPackage> {
        &self.packages
    }

    /// Has package in index?
    pub fn has(&self, package: &str) -> bool {
        self.packages.contains_key(package)
    }

    /// Get package directly (for fast resolution)
    pub fn get_package(&self, package: &str) -> Option<&PreResolvedPackage> {
        self.packages.get(package)
    }

    /// Lookup package in index.
    /// Only returns when the *latest indexed version* satisfies `version_req`.
    pub fn lookup(&self, package: &str, version_req: &str) -> Option<&PreResolvedPackage> {
        lookup_version(self.packages.get(package), version_req)
    }

    pub(crate) fn lookup_for_origin(
        &self,
        package: &str,
        version_req: &str,
        origin_key: &str,
        allow_embedded: bool,
    ) -> Option<&PreResolvedPackage> {
        let key = format!("{origin_key}|{package}");
        let user = self.user_packages.get(&key);
        let embedded = if allow_embedded {
            self.packages.get(package)
        } else {
            None
        };
        lookup_version(user.or(embedded), version_req)
    }

    /// Add package to index
    pub fn add_package(&mut self, pkg: PreResolvedPackage) {
        self.packages.insert(pkg.name.clone(), pkg);
    }

    pub(crate) fn add_package_for_origin(&mut self, pkg: PreResolvedPackage, origin_key: &str) {
        self.user_packages
            .insert(format!("{origin_key}|{}", pkg.name), pkg);
    }

    /// Check if package exists in index
    pub fn contains(&self, package: &str) -> bool {
        self.packages.contains_key(package)
    }

    /// Get number of packages in index
    pub fn len(&self) -> usize {
        self.packages.len()
    }

    /// Check if index is empty
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Get top N packages by popularity
    pub fn top_packages(&self, n: usize) -> Vec<&PreResolvedPackage> {
        self.packages.values().take(n).collect()
    }

    /// Clear user index (keep bundled)
    pub fn clear_user_index(&mut self) {
        self.load_bundled_index();
        let _ = self.save_user_index();
    }
}

/// Resolve package from pre-resolved index (O(1))
pub fn resolve_from_index(
    index: &PackageIndex,
    package: &str,
    version_req: &str,
) -> Option<PreResolvedPackage> {
    index.lookup(package, version_req).cloned()
}

/// Build index entry from package metadata
pub fn build_index_entry(name: &str, metadata: &serde_json::Value) -> Option<PreResolvedPackage> {
    let version = metadata
        .get("dist-tags")
        .and_then(|t| t.get("latest"))
        .and_then(|v| v.as_str())?
        .to_string();

    let versions = metadata
        .get("versions")
        .and_then(|v| v.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    let latest_version = metadata
        .get("versions")
        .and_then(|v| v.get(&version))
        .and_then(|v| v.as_object())?;

    let tarball_url = latest_version
        .get("dist")
        .and_then(|d| d.get("tarball"))
        .and_then(|t| t.as_str())?
        .to_string();

    let integrity = latest_version
        .get("dist")
        .and_then(|d| d.get("integrity"))
        .and_then(|i| i.as_str())?
        .to_string();

    let dependencies = latest_version
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();

    Some(PreResolvedPackage {
        name: name.to_string(),
        version,
        versions,
        tarball_url,
        integrity,
        dependencies,
        resolved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed(name: &str, version: &str) -> PreResolvedPackage {
        PreResolvedPackage {
            name: name.to_string(),
            version: version.to_string(),
            versions: vec![version.to_string()],
            tarball_url: format!("https://example.test/{name}-{version}.tgz"),
            integrity: "sha512-fixture".to_string(),
            dependencies: HashMap::new(),
            resolved_at: 0,
        }
    }

    #[test]
    fn user_index_reuse_is_partitioned_by_origin_and_legacy_entries_are_misses() {
        let temp_dir = tempfile::tempdir().unwrap();
        let old_key = "legacy-private";
        let old_entry = indexed(old_key, "9.0.0");
        std::fs::write(
            temp_dir.path().join("user-index.json"),
            serde_json::to_vec(&HashMap::from([(old_key, old_entry)])).unwrap(),
        )
        .unwrap();

        let mut index = PackageIndex::new(temp_dir.path().to_path_buf());
        let first_origin = "metadata-v2:origin-a";
        let second_origin = "metadata-v2:origin-b";
        index.add_package_for_origin(indexed("private-pkg", "1.2.0"), first_origin);
        index.add_package_for_origin(indexed("private-pkg", "2.3.0"), second_origin);

        assert_eq!(
            index
                .lookup_for_origin("private-pkg", "*", first_origin, false)
                .map(|pkg| pkg.version.as_str()),
            Some("1.2.0")
        );
        assert_eq!(
            index
                .lookup_for_origin("private-pkg", "*", second_origin, false)
                .map(|pkg| pkg.version.as_str()),
            Some("2.3.0")
        );
        assert!(index
            .lookup_for_origin("legacy-private", "*", first_origin, false)
            .is_none());

        index.save_user_index().unwrap();
        let saved: HashMap<String, PreResolvedPackage> = serde_json::from_slice(
            &std::fs::read(temp_dir.path().join("user-index.json")).unwrap(),
        )
        .unwrap();
        assert!(saved.contains_key(old_key));
        assert!(saved.contains_key("metadata-v2:origin-a|private-pkg"));
        assert!(saved.contains_key("metadata-v2:origin-b|private-pkg"));
    }

    #[test]
    fn test_package_index() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = PackageIndex::new(temp_dir.path().to_path_buf());

        let pkg = PreResolvedPackage {
            name: "test-pkg".to_string(),
            version: "1.0.0".to_string(),
            versions: vec!["1.0.0".to_string()],
            tarball_url: "https://example.com/test-pkg-1.0.0.tgz".to_string(),
            integrity: "sha256-abc123".to_string(),
            dependencies: HashMap::new(),
            resolved_at: 0,
        };

        index.add_package(pkg);

        assert!(index.contains("test-pkg"));
        assert!(!index.is_empty());

        let found = index.lookup("test-pkg", "^1.0.0").unwrap();
        assert_eq!(found.name, "test-pkg");
        assert_eq!(found.version, "1.0.0");
    }

    #[test]
    fn lookup_respects_exact_version() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = PackageIndex::new(temp_dir.path().to_path_buf());
        index.add_package(PreResolvedPackage {
            name: "asynckit".to_string(),
            version: "0.5.0".to_string(),
            versions: vec!["0.5.0".to_string(), "0.4.0".to_string()],
            tarball_url: "https://example.com/asynckit-0.5.0.tgz".to_string(),
            integrity: "sha512-test".to_string(),
            dependencies: HashMap::new(),
            resolved_at: 0,
        });

        assert!(index.lookup("asynckit", "0.5.0").is_some());
        assert!(index.lookup("asynckit", "0.4.0").is_none());
    }

    #[test]
    fn lookup_respects_semver_range() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut index = PackageIndex::new(temp_dir.path().to_path_buf());
        index.add_package(PreResolvedPackage {
            name: "demo".to_string(),
            version: "2.1.0".to_string(),
            versions: vec!["2.1.0".to_string()],
            tarball_url: "https://example.com/demo-2.1.0.tgz".to_string(),
            integrity: "sha512-test".to_string(),
            dependencies: HashMap::new(),
            resolved_at: 0,
        });

        assert!(index.lookup("demo", "^2.0.0").is_some());
        assert!(index.lookup("demo", "^1.0.0").is_none());
    }
}
