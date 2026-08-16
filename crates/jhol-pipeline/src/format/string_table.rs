/// A deduplicated, packed string table for paths and names.
/// Strings are stored null-terminated, sorted for binary search prefix matching.
pub struct StringTable {
    bytes: Vec<u8>,
    offsets: Vec<(u32, u32)>, // (string_hash, offset) sorted by string_hash
}

impl StringTable {
    pub fn new() -> Self {
        let mut bytes = Vec::new();
        bytes.push(0);
        Self {
            bytes,
            offsets: Vec::new(),
        }
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let offsets = build_offset_index(&bytes);
        Self { bytes, offsets }
    }

    pub fn intern(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }

        let find_result = self.find(s);
        if let Some(_offset) = find_result.and_then(|(_, off)| {
            let at = off as usize;
            let end = self.bytes[at..]
                .iter()
                .position(|&b| b == 0)
                .map(|p| at + p)
                .unwrap_or(self.bytes.len());
            if self.bytes[at..end] == *s.as_bytes() {
                Some(off)
            } else {
                None
            }
        }) {
            return find_result.unwrap().1;
        }

        let offset = self.bytes.len() as u32;
        self.bytes.extend_from_slice(s.as_bytes());
        self.bytes.push(0);

        let h = string_hash(s);
        self.offsets.push((h, offset));

        // Keep sorted by hash for binary search
        let last = self.offsets.len() - 1;
        if last > 0 && self.offsets[last - 1].0 > h {
            self.offsets.sort_by_key(|k| k.0);
        }

        offset
    }

    pub fn find(&self, s: &str) -> Option<(u32, u32)> {
        let h = string_hash(s);
        let idx = self.offsets.binary_search_by_key(&h, |k| k.0).ok()?;
        Some(self.offsets[idx])
    }

    pub fn get(&self, offset: u32) -> &str {
        let start = offset as usize;
        if start >= self.bytes.len() {
            return "";
        }
        let end = self.bytes[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| start + p)
            .unwrap_or(self.bytes.len());
        std::str::from_utf8(&self.bytes[start..end]).unwrap_or("")
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.len() <= 1
    }

    /// Number of unique strings
    pub fn count(&self) -> usize {
        self.offsets.len()
    }
}

impl Default for StringTable {
    fn default() -> Self {
        Self::new()
    }
}

fn string_hash(s: &str) -> u32 {
    let bytes = s.as_bytes();
    let mut h: u32 = 5381;
    for &b in bytes {
        h = h.wrapping_mul(33).wrapping_add(b as u32);
    }
    h
}

fn build_offset_index(bytes: &[u8]) -> Vec<(u32, u32)> {
    let mut offsets = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] == 0 {
            pos += 1;
            continue;
        }
        let start = pos;
        while pos < bytes.len() && bytes[pos] != 0 {
            pos += 1;
        }
        let s = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
        if !s.is_empty() {
            offsets.push((string_hash(s), start as u32));
        }
    }
    offsets.sort_by_key(|k| k.0);
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_and_get() {
        let mut st = StringTable::new();
        let off = st.intern("index.js");
        assert_eq!(st.get(off), "index.js");
    }

    #[test]
    fn test_deduplication() {
        let mut st = StringTable::new();
        let off1 = st.intern("package.json");
        let off2 = st.intern("package.json");
        assert_eq!(off1, off2);
    }

    #[test]
    fn test_empty_string() {
        let mut st = StringTable::new();
        assert_eq!(st.intern(""), 0);
        assert_eq!(st.get(0), "");
    }

    #[test]
    fn test_from_bytes() {
        let mut st = StringTable::new();
        st.intern("a.js");
        st.intern("b.js");
        let bytes = st.into_bytes();

        let st2 = StringTable::from_bytes(bytes);
        assert_eq!(st2.count(), 2);
    }

    #[test]
    fn test_multiple_paths() {
        let mut st = StringTable::new();
        let paths = ["package.json", "index.js", "lib/index.js", "README.md"];
        for p in &paths {
            st.intern(p);
        }
        for p in &paths {
            let off = st.find(p).unwrap().1;
            assert_eq!(st.get(off), *p);
        }
    }
}
