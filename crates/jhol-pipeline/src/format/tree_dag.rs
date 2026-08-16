use sha2::{Digest, Sha256};

/// A node in the content-addressed directory tree
#[derive(Clone, Debug)]
pub enum DirEntry {
    File {
        name: String,
        content_hash: [u8; 32],
        mode: u16,
    },
    Directory {
        name: String,
        entries: Vec<DirEntry>,
    },
    Symlink {
        name: String,
        target: String,
    },
}

impl DirEntry {
    pub fn name(&self) -> &str {
        match self {
            DirEntry::File { name, .. } => name,
            DirEntry::Directory { name, .. } => name,
            DirEntry::Symlink { name, .. } => name,
        }
    }

    pub fn set_name(&mut self, new_name: &str) {
        match self {
            DirEntry::File { ref mut name, .. } => *name = new_name.to_string(),
            DirEntry::Directory { ref mut name, .. } => *name = new_name.to_string(),
            DirEntry::Symlink { ref mut name, .. } => *name = new_name.to_string(),
        }
    }

    /// Compute the SHA-256 hash of this directory tree node recursively
    pub fn compute_hash(&self) -> [u8; 32] {
        match self {
            DirEntry::File {
                name,
                content_hash,
                mode,
            } => {
                let mut hasher = Sha256::new();
                hasher.update(b"file:");
                hasher.update(name.as_bytes());
                hasher.update(b":");
                hasher.update(mode.to_le_bytes());
                hasher.update(b":");
                hasher.update(content_hash);
                let result = hasher.finalize();
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&result);
                hash
            }
            DirEntry::Directory { name, entries } => {
                let mut hasher = Sha256::new();
                hasher.update(b"dir:");
                hasher.update(name.as_bytes());
                hasher.update(b":");

                let mut child_hashes: Vec<[u8; 32]> =
                    entries.iter().map(|e| e.compute_hash()).collect();
                child_hashes.sort();

                for ch in &child_hashes {
                    hasher.update(ch);
                }

                let result = hasher.finalize();
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&result);
                hash
            }
            DirEntry::Symlink { name, target } => {
                let mut hasher = Sha256::new();
                hasher.update(b"sym:");
                hasher.update(name.as_bytes());
                hasher.update(b":");
                hasher.update(target.as_bytes());
                let result = hasher.finalize();
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&result);
                hash
            }
        }
    }

    /// Serialize to bytes (for storage in the Tree DAG index)
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            DirEntry::File {
                name,
                content_hash,
                mode,
            } => {
                let mut bytes = vec![0u8]; // kind = File
                encode_len_prefixed_string(&mut bytes, name);
                bytes.extend_from_slice(content_hash);
                bytes.extend_from_slice(&mode.to_le_bytes());
                bytes
            }
            DirEntry::Directory { name, entries } => {
                let mut bytes = vec![1u8]; // kind = Directory
                encode_len_prefixed_string(&mut bytes, name);
                bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                for entry in entries {
                    bytes.extend_from_slice(&entry.serialize());
                }
                bytes
            }
            DirEntry::Symlink { name, target } => {
                let mut bytes = vec![2u8]; // kind = Symlink
                encode_len_prefixed_string(&mut bytes, name);
                encode_len_prefixed_string(&mut bytes, target);
                bytes
            }
        }
    }

    /// Deserialize from bytes
    pub fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        if *offset >= bytes.len() {
            return Err("unexpected end".into());
        }

        let kind = bytes[*offset];
        *offset += 1;

        match kind {
            0 => {
                let name = decode_len_prefixed_string(bytes, offset)?;
                if *offset + 32 + 2 > bytes.len() {
                    return Err("unexpected end for File entry".into());
                }
                let mut content_hash = [0u8; 32];
                content_hash.copy_from_slice(&bytes[*offset..*offset + 32]);
                *offset += 32;
                let mode = u16::from_le_bytes(bytes[*offset..*offset + 2].try_into().unwrap());
                *offset += 2;
                Ok(DirEntry::File {
                    name,
                    content_hash,
                    mode,
                })
            }
            1 => {
                let name = decode_len_prefixed_string(bytes, offset)?;
                if *offset + 4 > bytes.len() {
                    return Err("unexpected end for Directory entry".into());
                }
                let count =
                    u32::from_le_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
                *offset += 4;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    entries.push(DirEntry::deserialize(bytes, offset)?);
                }
                Ok(DirEntry::Directory { name, entries })
            }
            2 => {
                let name = decode_len_prefixed_string(bytes, offset)?;
                let target = decode_len_prefixed_string(bytes, offset)?;
                Ok(DirEntry::Symlink { name, target })
            }
            _ => Err(format!("unknown DirEntry kind: {kind}")),
        }
    }
}

fn encode_len_prefixed_string(bytes: &mut Vec<u8>, s: &str) {
    let len = s.len() as u32;
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(s.as_bytes());
}

fn decode_len_prefixed_string(bytes: &[u8], offset: &mut usize) -> Result<String, String> {
    if *offset + 4 > bytes.len() {
        return Err("unexpected end for string length".into());
    }
    let len = u32::from_le_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
    *offset += 4;
    if *offset + len > bytes.len() {
        return Err("unexpected end for string data".into());
    }
    let s = std::str::from_utf8(&bytes[*offset..*offset + len])
        .map_err(|e| format!("invalid UTF-8: {e}"))?
        .to_string();
    *offset += len;
    Ok(s)
}

/// An entry in the Tree DAG index: maps tree_hash → serialized DirEntry bytes
#[derive(Clone, Debug)]
pub struct TreeDagEntry {
    pub tree_hash: [u8; 32],
    pub root: DirEntry,
}

impl TreeDagEntry {
    pub fn new(root: DirEntry) -> Self {
        let tree_hash = root.compute_hash();
        Self { tree_hash, root }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let root_bytes = self.root.serialize();
        let mut bytes = Vec::with_capacity(32 + 4 + root_bytes.len());
        bytes.extend_from_slice(&self.tree_hash);
        bytes.extend_from_slice(&(root_bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&root_bytes);
        bytes
    }

    pub fn deserialize(bytes: &[u8], offset: &mut usize) -> Result<Self, String> {
        if *offset + 32 + 4 > bytes.len() {
            return Err("TreeDagEntry too short".into());
        }
        let mut tree_hash = [0u8; 32];
        tree_hash.copy_from_slice(&bytes[*offset..*offset + 32]);
        *offset += 32;
        let _root_len =
            u32::from_le_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
        *offset += 4;
        let root = DirEntry::deserialize(bytes, offset)?;

        // Verify tree hash matches
        let computed = root.compute_hash();
        if computed != tree_hash {
            return Err(format!(
                "tree hash mismatch: computed {} != stored {}",
                hex::encode(computed),
                hex::encode(tree_hash)
            ));
        }

        Ok(Self { tree_hash, root })
    }
}

/// Build a directory tree from a list of (path, content_hash, mode)
pub fn build_directory_tree(files: &[(Vec<&str>, [u8; 32], u16)]) -> DirEntry {
    let root = DirEntry::Directory {
        name: String::new(),
        entries: Vec::new(),
    };

    let mut root = insert_files_into_tree(root, files, 0);

    // Sort entries for deterministic hash
    sort_entries(&mut root);

    root
}

fn insert_files_into_tree<'a>(
    mut entry: DirEntry,
    files: &[(Vec<&'a str>, [u8; 32], u16)],
    depth: usize,
) -> DirEntry {
    match entry {
        DirEntry::Directory {
            name: _,
            ref mut entries,
        } => {
            // Group files by the next path component
            let mut subdirs: std::collections::BTreeMap<String, Vec<(Vec<&str>, [u8; 32], u16)>> =
                std::collections::BTreeMap::new();
            let mut files_here: Vec<(Vec<&str>, [u8; 32], u16)> = Vec::new();

            for file in files {
                if file.0.len() == depth + 1 {
                    // File is directly in this directory
                    files_here.push(file.clone());
                } else if file.0.len() > depth + 1 {
                    // File is in a subdirectory
                    let subdir_name = file.0[depth].to_string();
                    subdirs.entry(subdir_name).or_default().push(file.clone());
                }
            }

            // Add files
            for (path_parts, content_hash, mode) in &files_here {
                let name = path_parts[depth].to_string();
                entries.push(DirEntry::File {
                    name,
                    content_hash: *content_hash,
                    mode: *mode,
                });
            }

            // Add subdirectories recursively
            for (subdir_name, sub_files) in subdirs {
                let sub = DirEntry::Directory {
                    name: subdir_name.clone(),
                    entries: Vec::new(),
                };
                let sub = insert_files_into_tree(sub, &sub_files, depth + 1);
                entries.push(sub);
            }
        }
        _ => {}
    }

    entry
}

fn sort_entries(entry: &mut DirEntry) {
    if let DirEntry::Directory {
        ref mut entries, ..
    } = entry
    {
        entries.sort_by(|a, b| a.name().cmp(b.name()));

        // Fix the path parts to have depth-correct names
        let taken = std::mem::take(entries);
        for mut child in taken {
            sort_entries(&mut child);
            entries.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_hash_stability() {
        let hash1 = [1u8; 32];
        let file = DirEntry::File {
            name: "index.js".to_string(),
            content_hash: hash1,
            mode: 0o644,
        };
        let hash_a = file.compute_hash();
        let hash_b = file.compute_hash();
        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn test_directory_hash() {
        let dir = DirEntry::Directory {
            name: "lib".to_string(),
            entries: vec![DirEntry::File {
                name: "util.js".to_string(),
                content_hash: [1u8; 32],
                mode: 0o644,
            }],
        };
        let hash = dir.compute_hash();
        let non_zero = hash.iter().any(|&b| b != 0);
        assert!(non_zero);
    }

    #[test]
    fn test_serialize_roundtrip() {
        let dir = DirEntry::Directory {
            name: "root".to_string(),
            entries: vec![
                DirEntry::File {
                    name: "a.js".to_string(),
                    content_hash: [1u8; 32],
                    mode: 0o644,
                },
                DirEntry::Directory {
                    name: "sub".to_string(),
                    entries: vec![DirEntry::File {
                        name: "b.js".to_string(),
                        content_hash: [2u8; 32],
                        mode: 0o755,
                    }],
                },
                DirEntry::Symlink {
                    name: "link".to_string(),
                    target: "../target".to_string(),
                },
            ],
        };

        let dag = TreeDagEntry::new(dir);
        let ser = dag.serialize();
        let mut offset = 0;
        let dag2 = TreeDagEntry::deserialize(&ser, &mut offset).unwrap();
        assert_eq!(dag.tree_hash, dag2.tree_hash);

        // Verify structure
        match &dag2.root {
            DirEntry::Directory { entries, .. } => {
                assert_eq!(entries.len(), 3);
            }
            _ => panic!("expected directory"),
        }
    }

    #[test]
    fn test_build_tree_from_paths() {
        let files = vec![
            (vec!["package.json"], [1u8; 32], 0o644),
            (vec!["src", "index.js"], [2u8; 32], 0o644),
            (vec!["src", "util.js"], [3u8; 32], 0o644),
            (vec!["README.md"], [4u8; 32], 0o644),
        ];

        let root = build_directory_tree(&files);
        match &root {
            DirEntry::Directory { entries, .. } => {
                assert_eq!(entries.len(), 3);
                let names: Vec<&str> = entries.iter().map(|e| e.name()).collect();
                assert!(names.contains(&"package.json"));
                assert!(names.contains(&"README.md"));
                assert!(names.contains(&"src"));
            }
            _ => panic!("expected directory"),
        }
    }
}
