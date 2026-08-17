use std::fs;
use std::path::Path;

#[cfg(test)]
use crate::format::SwissTable;
use crate::format::{build_directory_tree, Manifest, StringTable, TreeDagEntry};
use crate::store::GridStore;

/// Build a directory tree on disk from a package's manifest + JGrid blocks
pub fn materialize_tree_from_store(
    store: &GridStore,
    manifest: &Manifest,
    string_table: &StringTable,
    tree_path: &Path,
) -> Result<[u8; 32], String> {
    fs::create_dir_all(tree_path).map_err(|e| format!("create tree dir {tree_path:?}: {e}"))?;

    for file in &manifest.files {
        let path_str = string_table.get(file.path_offset);
        let target_path = tree_path.join(path_str);

        // Create parent directories
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create parent {parent:?}: {e}"))?;
        }

        // Read content block
        let content = store
            .read_block(&file.content_hash)
            .ok_or_else(|| format!("content block not found for {path_str}"))?;

        // Write file
        fs::write(&target_path, content).map_err(|e| format!("write {target_path:?}: {e}"))?;

        // Set permissions
        set_permissions(&target_path, file.mode)?;
    }

    Ok(manifest.tree_hash)
}

/// Build a directory tree from a list of (path, content_hash, mode) triples
/// and content block reader function
pub fn build_tree_from_manifest<'a>(
    files: &[(&'a str, [u8; 32], u16)],
    mut read_block: impl FnMut(&[u8; 32]) -> Option<Vec<u8>>,
    tree_path: &Path,
) -> Result<[u8; 32], String> {
    fs::create_dir_all(tree_path).map_err(|e| format!("create {tree_path:?}: {e}"))?;

    for (path_str, content_hash, mode) in files {
        let target_path = tree_path.join(path_str);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create parent {parent:?}: {e}"))?;
        }

        // Symlink handling
        if *mode & 0o120000 == 0o120000 {
            let target = read_block(content_hash)
                .ok_or_else(|| format!("symlink target not found for {path_str}"))?;
            let target_str = String::from_utf8(target)
                .map_err(|_| format!("invalid symlink target UTF-8 for {path_str}"))?;
            std::os::unix::fs::symlink(&target_str, &target_path)
                .map_err(|e| format!("symlink {target_path:?}: {e}"))?;
        } else {
            let content = read_block(content_hash)
                .ok_or_else(|| format!("content not found for {path_str}"))?;
            fs::write(&target_path, &content).map_err(|e| format!("write {target_path:?}: {e}"))?;
            set_permissions(&target_path, *mode)?;
        }
    }

    // Compute tree hash by building the DAG
    let path_parts: Vec<(Vec<&str>, [u8; 32], u16)> = files
        .iter()
        .map(|(p, h, m)| (p.split('/').collect::<Vec<_>>(), *h, *m))
        .collect();
    let root = build_directory_tree(&path_parts);
    let dag = TreeDagEntry::new(root);
    Ok(dag.tree_hash)
}

fn set_permissions(path: &Path, mode: u16) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode as u32 & 0o777))
            .map_err(|e| format!("set permissions on {path:?}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingester::ingest_raw_files;
    use crate::store::GridBuilder;
    use tempfile::tempdir;

    #[test]
    fn test_materialize_tree() {
        let dir = tempdir().unwrap();
        let jgrid_path = dir.path().join("store.jgrid");
        let tree_path = dir.path().join("tree");

        let mut builder = GridBuilder::create(&jgrid_path).unwrap();
        let files = vec![
            ("index.js", 0o644, b"console.log('hi')" as &[u8]),
            ("package.json", 0o644, b"{\"name\":\"x\"}"),
        ];
        ingest_raw_files("pkg", "1.0.0", &files, &mut builder).unwrap();
        builder.finalize().unwrap();

        let store = GridStore::open(&jgrid_path).unwrap();
        let manifest = store.find_manifest("pkg", "1.0.0").unwrap();

        materialize_tree_from_store(&store, manifest, store.string_table(), &tree_path).unwrap();

        assert!(tree_path.join("index.js").exists());
        assert!(tree_path.join("package.json").exists());

        let content = std::fs::read(tree_path.join("index.js")).unwrap();
        assert_eq!(content, b"console.log('hi')");
    }

    #[test]
    fn test_build_tree_from_manifest_fn() {
        let dir = tempdir().unwrap();
        let tree_path = dir.path().join("output");

        let hash1 = SwissTable::compute_hash(b"content1");
        let hash2 = SwissTable::compute_hash(b"content2");

        let files = vec![("src/a.js", hash1, 0o644), ("src/b.js", hash2, 0o644)];

        let tree_hash = build_tree_from_manifest(
            &files,
            |hash| {
                if *hash == hash1 {
                    Some(b"content1".to_vec())
                } else if *hash == hash2 {
                    Some(b"content2".to_vec())
                } else {
                    None
                }
            },
            &tree_path,
        )
        .unwrap();

        assert!(tree_path.join("src/a.js").exists());
        assert!(tree_path.join("src/b.js").exists());

        let non_zero = tree_hash.iter().any(|&b| b != 0);
        assert!(non_zero);
    }
}
