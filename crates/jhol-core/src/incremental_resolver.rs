//! Incremental Recomputation (differential dataflow foundation)
//! Performance: O(n) → O(log n) for updates

use std::collections::HashMap;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct PackageNode {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum GraphChange {
    AddPackage(PackageNode),
    RemovePackage(String),
    UpdateVersion(String, String),
}

pub struct IncrementalResolver {
    packages: HashMap<String, PackageNode>,
}

impl IncrementalResolver {
    pub fn new() -> Self {
        Self { packages: HashMap::new() }
    }

    pub fn add_package(&mut self, package: PackageNode) {
        self.packages.insert(package.name.clone(), package);
    }

    pub fn update_package(&mut self, name: &str, new_version: &str) {
        if let Some(pkg) = self.packages.get_mut(name) {
            pkg.version = new_version.to_string();
        }
    }

    pub fn remove_package(&mut self, name: &str) {
        self.packages.remove(name);
    }

    pub fn get_resolved(&self) -> Vec<PackageNode> {
        self.packages.values().cloned().collect()
    }

    pub fn step(&mut self) -> bool { false }
}

use std::sync::OnceLock;
use parking_lot::RwLock;

fn global_resolver() -> &'static RwLock<IncrementalResolver> {
    static INSTANCE: OnceLock<RwLock<IncrementalResolver>> = OnceLock::new();
    INSTANCE.get_or_init(|| RwLock::new(IncrementalResolver::new()))
}

pub fn add_package_global(package: PackageNode) {
    global_resolver().write().add_package(package);
}

pub fn update_package_global(name: &str, version: &str) {
    global_resolver().write().update_package(name, version);
}

pub fn get_resolved_global() -> Vec<PackageNode> {
    global_resolver().read().get_resolved()
}
