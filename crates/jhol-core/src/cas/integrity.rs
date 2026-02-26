//! Integrity hash verification (SRI - Subresource Integrity)

use sha2::{Digest, Sha256, Sha384, Sha512};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

/// Integrity hash in SRI format (e.g., "sha256-abc123...")
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityHash {
    /// Hash algorithm
    pub algorithm: HashAlgorithm,
    /// Base64-encoded hash
    pub hash: String,
    /// Original SRI string
    pub sri: String,
}

/// Supported hash algorithms
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    /// Get the name of the algorithm
    pub fn name(&self) -> &'static str {
        match self {
            HashAlgorithm::Sha256 => "sha256",
            HashAlgorithm::Sha384 => "sha384",
            HashAlgorithm::Sha512 => "sha512",
        }
    }
    
    /// Compute hash of content
    pub fn compute(&self, content: &[u8]) -> String {
        match self {
            HashAlgorithm::Sha256 => {
                let hash = Sha256::digest(content);
                BASE64.encode(hash)
            }
            HashAlgorithm::Sha384 => {
                let hash = Sha384::digest(content);
                BASE64.encode(hash)
            }
            HashAlgorithm::Sha512 => {
                let hash = Sha512::digest(content);
                BASE64.encode(hash)
            }
        }
    }
    
    /// Verify content against expected hash
    pub fn verify(&self, content: &[u8], expected_hash: &str) -> bool {
        let computed = self.compute(content);
        computed == expected_hash
    }
}

impl IntegrityHash {
    /// Parse an SRI string (e.g., "sha256-abc123...")
    pub fn parse(sri: &str) -> Option<Self> {
        let sri = sri.trim();
        
        // Handle multiple hashes (take the first one)
        let sri = sri.split_whitespace().next().unwrap_or(sri);
        
        let parts: Vec<&str> = sri.split('-').collect();
        if parts.len() != 2 {
            return None;
        }
        
        let algorithm = match parts[0] {
            "sha256" => HashAlgorithm::Sha256,
            "sha384" => HashAlgorithm::Sha384,
            "sha512" => HashAlgorithm::Sha512,
            _ => return None,
        };
        
        Some(Self {
            algorithm,
            hash: parts[1].to_string(),
            sri: sri.to_string(),
        })
    }
    
    /// Create a new integrity hash
    pub fn new(algorithm: HashAlgorithm, content: &[u8]) -> Self {
        let hash = algorithm.compute(content);
        let sri = format!("{}-{}", algorithm.name(), hash);
        
        Self {
            algorithm,
            hash,
            sri,
        }
    }
    
    /// Compute SHA256 integrity hash
    pub fn sha256(content: &[u8]) -> Self {
        Self::new(HashAlgorithm::Sha256, content)
    }
    
    /// Compute SHA512 integrity hash
    pub fn sha512(content: &[u8]) -> Self {
        Self::new(HashAlgorithm::Sha512, content)
    }
    
    /// Verify content against this integrity hash
    pub fn verify(&self, content: &[u8]) -> bool {
        self.algorithm.verify(content, &self.hash)
    }
    
    /// Get the SRI string
    pub fn as_sri(&self) -> &str {
        &self.sri
    }
}

/// Compute integrity hash for content
pub fn compute_integrity(content: &[u8]) -> String {
    IntegrityHash::sha256(content).as_sri().to_string()
}

/// Compute SHA512 integrity hash (npm default)
pub fn compute_integrity_sha512(content: &[u8]) -> String {
    IntegrityHash::sha512(content).as_sri().to_string()
}

/// Verify content against expected integrity hash
pub fn verify_integrity(content: &[u8], expected_integrity: &str) -> bool {
    // Handle multiple hashes (space-separated)
    for sri in expected_integrity.split_whitespace() {
        if let Some(integrity) = IntegrityHash::parse(sri) {
            if integrity.verify(content) {
                return true;
            }
        }
    }
    
    false
}

/// Verify content against expected integrity hash with specific algorithm
pub fn verify_integrity_strict(content: &[u8], expected_integrity: &str) -> Result<bool, String> {
    let integrity = IntegrityHash::parse(expected_integrity)
        .ok_or_else(|| format!("Invalid integrity hash: {}", expected_integrity))?;
    
    Ok(integrity.verify(content))
}

/// Compare two integrity hashes (handles different algorithms)
pub fn compare_integrity(integrity1: &str, integrity2: &str) -> bool {
    if integrity1 == integrity2 {
        return true;
    }
    
    // Parse both and compare hashes (even if different algorithms)
    let parsed1 = IntegrityHash::parse(integrity1);
    let parsed2 = IntegrityHash::parse(integrity2);
    
    match (parsed1, parsed2) {
        (Some(i1), Some(i2)) => i1.hash == i2.hash,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_integrity() {
        let sri = "sha256-abc123==";
        let integrity = IntegrityHash::parse(sri).unwrap();
        
        assert_eq!(integrity.algorithm, HashAlgorithm::Sha256);
        assert_eq!(integrity.hash, "abc123==");
        assert_eq!(integrity.sri, sri);
    }

    #[test]
    fn test_compute_integrity() {
        let content = b"test content";
        let integrity = compute_integrity(content);
        
        assert!(integrity.starts_with("sha256-"));
        
        // Verify
        assert!(verify_integrity(content, &integrity));
        
        // Wrong content should fail
        assert!(!verify_integrity(b"wrong content", &integrity));
    }

    #[test]
    fn test_multiple_hashes() {
        let content = b"test content";
        let sha256 = IntegrityHash::sha256(content).as_sri();
        let sha512 = IntegrityHash::sha512(content).as_sri();
        
        // Multiple hashes (space-separated)
        let combined = format!("{} {}", sha256, sha512);
        
        // Should verify with either hash
        assert!(verify_integrity(content, &combined));
    }

    #[test]
    fn test_compare_integrity() {
        let content = b"test content";
        let sha256_1 = compute_integrity(content);
        let sha256_2 = compute_integrity(content);
        
        // Same content, same hash
        assert!(compare_integrity(&sha256_1, &sha256_2));
        
        // Different algorithms, same content
        let sha512 = compute_integrity_sha512(content);
        assert!(compare_integrity(&sha256_1, &sha512));
    }
}
