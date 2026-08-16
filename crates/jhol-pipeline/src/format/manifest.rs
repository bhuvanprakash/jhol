use crate::format::string_table::StringTable;

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
        let mut manifests = Vec::with_capacity(count);
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
