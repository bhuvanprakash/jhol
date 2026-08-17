use std::fs;
use std::path::{Path, PathBuf};

/// Manages pre-assembled package trees at ~/.jhol-cache/trees/<tree_hash>/
pub struct TreeCache {
    base_dir: PathBuf,
}

impl TreeCache {
    pub fn new(cache_dir: &Path) -> Self {
        let base_dir = cache_dir.join("trees");
        fs::create_dir_all(&base_dir).ok();
        Self { base_dir }
    }

    /// Path to a specific tree's directory
    pub fn tree_path(&self, tree_hash: &[u8; 32]) -> PathBuf {
        let hex = hex::encode(tree_hash);
        self.base_dir.join(&hex)
    }

    /// Check if a tree exists
    pub fn has_tree(&self, tree_hash: &[u8; 32]) -> bool {
        self.tree_path(tree_hash).is_dir()
    }

    /// Create the tree directory (empty)
    pub fn create_tree_dir(&self, tree_hash: &[u8; 32]) -> Result<PathBuf, String> {
        let path = self.tree_path(tree_hash);
        fs::create_dir_all(&path).map_err(|e| format!("create tree dir: {e}"))?;
        Ok(path)
    }

    /// List all tree hashes
    pub fn list_trees(&self) -> Result<Vec<String>, String> {
        let mut trees = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        trees.push(name.to_string());
                    }
                }
            }
        }
        trees.sort();
        Ok(trees)
    }

    /// Remove a specific tree
    pub fn remove_tree(&self, tree_hash: &[u8; 32]) -> Result<(), String> {
        let path = self.tree_path(tree_hash);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|e| format!("remove tree: {e}"))?;
        }
        Ok(())
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}
