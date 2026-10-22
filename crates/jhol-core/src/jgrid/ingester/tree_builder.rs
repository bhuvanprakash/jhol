use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::jgrid::format::{build_directory_tree, Manifest, StringTable, SwissTable, TreeDagEntry};
use crate::jgrid::store::GridStore;

/// Build a directory tree on disk from a package's manifest + JGrid blocks
pub fn materialize_tree_from_store(
    store: &GridStore,
    manifest: &Manifest,
    string_table: &StringTable,
    tree_path: &Path,
) -> Result<[u8; 32], String> {
    fs::create_dir_all(tree_path).map_err(|e| format!("create tree dir {tree_path:?}: {e}"))?;
    let mut seen_paths = HashSet::new();
    let mut symlink_paths = HashSet::new();
    let tree_entries: Vec<(&str, u16, [u8; 32])> = manifest
        .files
        .iter()
        .map(|file| {
            (
                string_table.get(file.path_offset),
                file.mode,
                file.content_hash,
            )
        })
        .collect();

    for file in &manifest.files {
        let path_str = string_table.get(file.path_offset);
        if !seen_paths.insert(path_str.to_string()) {
            return Err(format!("duplicate JGrid path: {path_str}"));
        }
        let is_symlink = file.mode & 0o170000 == 0o120000;
        let content = store
            .read_block(&file.content_hash)
            .ok_or_else(|| format!("content block not found for {path_str}"))?;
        if SwissTable::compute_hash(content) != file.content_hash {
            return Err(format!("content hash mismatch for JGrid path {path_str}"));
        }
        let link_target = if is_symlink {
            Some(
                String::from_utf8(content.to_vec())
                    .map_err(|_| format!("invalid symlink target UTF-8 for {path_str}"))?,
            )
        } else {
            None
        };
        crate::registry::validate_archive_path(
            path_str,
            tree_path,
            &[],
            if is_symlink {
                tar::EntryType::Symlink
            } else {
                tar::EntryType::Regular
            },
            link_target.as_deref().map(Path::new),
        )?;

        // An earlier entry may have created a symlink parent. Never follow one
        // while writing a later path, even when both paths are inside this tree.
        let mut ancestor = String::new();
        let mut parts = path_str.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                break;
            }
            if ancestor.is_empty() {
                ancestor.push_str(part);
            } else {
                ancestor.push('/');
                ancestor.push_str(part);
            }
            if symlink_paths.contains(&ancestor) {
                return Err(format!("JGrid path traverses symlink parent: {path_str}"));
            }
        }
        let target_path = tree_path.join(path_str);

        // Create parent directories
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create parent {parent:?}: {e}"))?;
        }

        if is_symlink {
            let target = link_target.as_deref().expect("validated symlink target");
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &target_path)
                .map_err(|e| format!("create symlink {target_path:?}: {e}"))?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(&target, &target_path)
                .map_err(|e| format!("create symlink {target_path:?}: {e}"))?;
            symlink_paths.insert(path_str.to_string());
        } else {
            fs::write(&target_path, content).map_err(|e| format!("write {target_path:?}: {e}"))?;
            set_permissions(&target_path, file.mode)?;
        }
    }

    Ok(Manifest::compute_tree_hash(&tree_entries))
}

/// Build a directory tree from a list of (path, content_hash, mode) triples
/// and content block reader function
pub fn build_tree_from_manifest(
    files: &[(&str, [u8; 32], u16)],
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
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target_str, &target_path)
                .map_err(|e| format!("symlink {target_path:?}: {e}"))?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(&target_str, &target_path)
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
    use crate::jgrid::ingester::ingest_raw_files;
    use crate::jgrid::store::GridBuilder;
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
    fn materialize_tree_preserves_symlinks_and_modes() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("store.jgrid");
        let tree_path = dir.path().join("tree");
        let mut builder = GridBuilder::create(&store_path).unwrap();
        ingest_raw_files(
            "pkg",
            "1.0.0",
            &[
                ("bin/cli.js", 0o755, b"#!/usr/bin/env node"),
                ("bin/cli", 0o120777, b"cli.js"),
            ],
            &mut builder,
        )
        .unwrap();
        builder.finalize().unwrap();
        let store = GridStore::open(&store_path).unwrap();
        let manifest = store.find_manifest("pkg", "1.0.0").unwrap();
        materialize_tree_from_store(&store, manifest, store.string_table(), &tree_path).unwrap();

        let metadata = fs::symlink_metadata(tree_path.join("bin/cli")).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(
            fs::read_link(tree_path.join("bin/cli")).unwrap(),
            Path::new("cli.js")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(tree_path.join("bin/cli.js"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn materialize_tree_rejects_parent_traversal() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("store.jgrid");
        let tree_path = dir.path().join("tree");
        let mut builder = GridBuilder::create(&store_path).unwrap();
        ingest_raw_files(
            "pkg",
            "1.0.0",
            &[("../outside", 0o644, b"nope")],
            &mut builder,
        )
        .unwrap();
        builder.finalize().unwrap();
        let store = GridStore::open(&store_path).unwrap();
        let manifest = store.find_manifest("pkg", "1.0.0").unwrap();
        assert!(
            materialize_tree_from_store(&store, manifest, store.string_table(), &tree_path)
                .is_err()
        );
        assert!(!dir.path().join("outside").exists());
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
