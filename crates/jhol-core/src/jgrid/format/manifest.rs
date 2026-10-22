use crate::jgrid::format::string_table::StringTable;
use sha2::{Digest, Sha256};

/// A single file entry within a package manifest
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path_offset: u32,
    pub mode: u16,
    pub content_hash: [u8; 32],
}

/// A package manifest: maps package@version → list of (path, mode, content_hash)
#[derive(Clone, Debug)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub files: Vec<FileEntry>,
    pub tree_hash: [u8; 32],
}

impl Manifest {
    /// Hash package paths, modes, and contents into a stable, unambiguous tree identity.
    pub fn compute_tree_hash(entries: &[(&str, u16, [u8; 32])]) -> [u8; 32] {
        let mut sorted = entries.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(&b.1)));
        let mut hasher = Sha256::new();
        hasher.update(b"jhol-jgrid-tree-v2\0");
        for (path, mode, content_hash) in sorted {
            hasher.update((path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
            hasher.update(mode.to_le_bytes());
            hasher.update(content_hash);
        }
        hasher.finalize().into()
    }

    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            files: Vec::new(),
            tree_hash: [0u8; 32],
        }
    }

    pub fn add_file(&mut self, path_offset: u32, mode: u16, content_hash: [u8; 32]) {
        self.files.push(FileEntry {
            path_offset,
            mode,
            content_hash,
        });
    }

    pub fn set_tree_hash(&mut self, hash: [u8; 32]) {
        self.tree_hash = hash;
    }

    pub fn serialize(&self, string_table: &StringTable) -> Result<Vec<u8>, String> {
        let name_off = string_table
            .find(&self.name)
            .map(|(_, off)| off)
            .unwrap_or(0);
        let version_off = string_table
            .find(&self.version)
            .map(|(_, off)| off)
            .unwrap_or(0);

        if name_off == 0 && !self.name.is_empty() {
            return Err(format!("name '{}' not found in string table", self.name));
        }
        if version_off == 0 && !self.version.is_empty() {
            return Err(format!(
                "version '{}' not found in string table",
                self.version
            ));
        }

        let mut bytes = Vec::with_capacity(72 + self.files.len() * 38);
        bytes.extend_from_slice(&name_off.to_le_bytes());
        bytes.extend_from_slice(&version_off.to_le_bytes());
        bytes.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.tree_hash);

        for file in &self.files {
            bytes.extend_from_slice(&file.path_offset.to_le_bytes());
            bytes.extend_from_slice(&file.mode.to_le_bytes());
            bytes.extend_from_slice(&file.content_hash);
        }

        Ok(bytes)
    }

    pub fn deserialize(bytes: &[u8], string_table: &StringTable) -> Result<(Self, usize), String> {
        if bytes.len() < 12 + 32 {
            return Err("manifest too short".into());
        }

        let mut offset = 0;
        let name_off = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let version_off = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let file_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        offset += 4;

        if bytes.len() < offset + 32 + (file_count as usize) * 38 {
            return Err("manifest too short for entries".into());
        }

        let mut tree_hash = [0u8; 32];
        tree_hash.copy_from_slice(&bytes[offset..offset + 32]);
        offset += 32;

        let name = string_table.get(name_off).to_string();
        let version = string_table.get(version_off).to_string();

        let mut files = Vec::with_capacity(file_count as usize);
        for _ in 0..file_count {
            if bytes.len() < offset + 6 + 32 {
                return Err("manifest truncated".into());
            }
            let path_offset = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let mode = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
            offset += 2;
            let mut content_hash = [0u8; 32];
            content_hash.copy_from_slice(&bytes[offset..offset + 32]);
            offset += 32;
            files.push(FileEntry {
                path_offset,
                mode,
                content_hash,
            });
        }

        Ok((
            Self {
                name,
                version,
                files,
                tree_hash,
            },
            offset,
        ))
    }

    pub fn serialize_index(
        manifests: &[Manifest],
        string_table: &StringTable,
    ) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(manifests.len() as u32).to_le_bytes());

        for manifest in manifests {
            let entry_bytes = manifest.serialize(string_table)?;
            bytes.extend_from_slice(&(entry_bytes.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&entry_bytes);
        }

        Ok(bytes)
    }

    pub fn deserialize_index(
        bytes: &[u8],
        string_table: &StringTable,
    ) -> Result<Vec<Manifest>, String> {
        if bytes.len() < 4 {
            return Ok(Vec::new());
        }

        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        // `count` comes straight from the file. Each entry needs at least a 4-byte
        // length prefix, so anything larger than the remaining bytes is corruption;
        // reserving it verbatim would allocate gigabytes and abort the process.
        let max_entries = bytes.len().saturating_sub(4) / 4 + 1;
        let mut manifests = Vec::with_capacity(count.min(max_entries));
        let mut offset = 4;

        for _ in 0..count {
            if bytes.len() < offset + 4 {
                return Err("index truncated at entry length".into());
            }
            let entry_len =
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            if bytes.len() < offset + entry_len {
                return Err("index truncated at entry data".into());
            }

            let (manifest, read) =
                Manifest::deserialize(&bytes[offset..offset + entry_len], string_table)?;
            manifests.push(manifest);
            offset += read;
        }

        Ok(manifests)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn deserialize_index_rejects_absurd_count_without_huge_allocation() {
        // `count` is read straight from the store. A corrupt/hostile value must
        // produce an error rather than reserving ~4 billion `Manifest`s (which
        // aborts the process under `panic = "abort"`).
        let mut bytes = u32::MAX.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 8]);
        let st = super::StringTable::new();
        let res = super::Manifest::deserialize_index(&bytes, &st);
        assert!(
            res.is_err(),
            "expected an error, got {:?}",
            res.map(|v| v.len())
        );
    }

    #[test]
    fn deserialize_rejects_truncated_and_short_input() {
        let st = super::StringTable::new();
        assert!(super::Manifest::deserialize(&[], &st).is_err());
        assert!(super::Manifest::deserialize(&[0u8; 8], &st).is_err());
        // Claims 3 files but has no entry bytes.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_le_bytes()); // name
        bytes.extend_from_slice(&0u32.to_le_bytes()); // version
        bytes.extend_from_slice(&3u32.to_le_bytes()); // file_count = 3
        bytes.extend_from_slice(&[0u8; 32]); // tree hash
        assert!(super::Manifest::deserialize(&bytes, &st).is_err());
    }

    use super::*;

    #[test]
    fn test_manifest_roundtrip() {
        let mut st = StringTable::new();
        st.intern("lodash");
        st.intern("4.17.21");
        st.intern("index.js");
        st.intern("package.json");

        let mut m = Manifest::new("lodash", "4.17.21");
        m.add_file(st.intern("index.js"), 0o644, [1u8; 32]);
        m.add_file(st.intern("package.json"), 0o644, [2u8; 32]);
        m.set_tree_hash([3u8; 32]);

        let ser = m.serialize(&st).unwrap();
        let (m2, _) = Manifest::deserialize(&ser, &st).unwrap();
        assert_eq!(m2.name, "lodash");
        assert_eq!(m2.version, "4.17.21");
        assert_eq!(m2.files.len(), 2);
        assert_eq!(m2.files[0].mode, 0o644);
        assert_eq!(m2.files[1].mode, 0o644);
    }

    #[test]
    fn test_index_roundtrip() {
        let mut st = StringTable::new();
        st.intern("a");
        st.intern("1.0.0");
        st.intern("b");
        st.intern("2.0.0");
        st.intern("f.js");

        let m1 = {
            let mut m = Manifest::new("a", "1.0.0");
            m.add_file(st.intern("f.js"), 0o644, [1u8; 32]);
            m
        };
        let m2 = {
            let mut m = Manifest::new("b", "2.0.0");
            m.add_file(st.intern("f.js"), 0o755, [2u8; 32]);
            m
        };

        let ser = Manifest::serialize_index(&[m1, m2], &st).unwrap();
        let deser = Manifest::deserialize_index(&ser, &st).unwrap();
        assert_eq!(deser.len(), 2);
        assert_eq!(deser[0].name, "a");
        assert_eq!(deser[1].name, "b");
    }
}
