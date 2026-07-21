//! Tests for Content-Addressed Store

use crate::content_addressed::{
    ContentAddressedStore, ContentHash, ExportEntry, ExportType, IntegrityMetadata, PackageMetadata,
};
use std::collections::HashMap;

#[test]
fn test_store_and_retrieve() {
    let mut store = ContentAddressedStore::memory();
    
    let content = b"package content here";
    let metadata = PackageMetadata {
        name: "lodash".to_string(),
        version: "4.17.21".to_string(),
        description: Some("Utility library".to_string()),
        main: Some("lodash.js".to_string()),
        module: None,
        types: None,
        exports: None,
        dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
        optional_dependencies: HashMap::new(),
        dev_dependencies: HashMap::new(),
    };
    
    let integrity = IntegrityMetadata {
        registry: "npmjs.org".to_string(),
        registry_integrity: Some("sha512-abc123".to_string()),
        shasum: Some("def456".to_string()),
        signatures: vec![],
    };
    
    let exports = vec![
        ExportEntry {
            path: "lodash.js".to_string(),
            content_hash: ContentHash::new(b"export"),
            size_bytes: 100,
            exports_type: ExportType::CommonJS,
        },
    ];
    
    let tarball_hash = ContentHash::new(content);
    
    let content_hash = store.store(
        content,
        metadata.clone(),
        integrity,
        exports,
        tarball_hash,
        1000,
        5,
    ).unwrap();
    
    let retrieved = store.get(&content_hash).unwrap();
    assert!(retrieved.is_some());
    
    let pkg = retrieved.unwrap();
    assert_eq!(pkg.metadata.name, "lodash");
    assert_eq!(pkg.metadata.version, "4.17.21");
}

#[test]
fn test_verify_integrity() {
    use crate::content_addressed::{compute_integrity, verify_integrity};
    
    let data = b"test package content";
    let integrity = compute_integrity(data);
    
    assert!(integrity.starts_with("sha256-"));
    assert!(verify_integrity(data, &integrity));
}

#[test]
fn test_hash_tarball() {
    use crate::content_addressed::hash_tarball;
    
    let tarball = b"tarball content";
    let hash = hash_tarball(tarball);
    
    assert_eq!(hash.as_str().len(), 64);
    
    // Same content = same hash
    let hash2 = hash_tarball(tarball);
    assert_eq!(hash, hash2);
}

#[test]
fn test_deduplication_by_content() {
    let mut store = ContentAddressedStore::memory();
    
    let content1 = b"identical content";
    let content2 = b"identical content"; // Same!
    
    let metadata1 = PackageMetadata {
        name: "pkg-a".to_string(),
        version: "1.0.0".to_string(),
        description: None,
        main: None,
        module: None,
        types: None,
        exports: None,
        dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
        optional_dependencies: HashMap::new(),
        dev_dependencies: HashMap::new(),
    };
    
    let metadata2 = PackageMetadata {
        name: "pkg-b".to_string(),
        version: "1.0.0".to_string(),
        description: None,
        main: None,
        module: None,
        types: None,
        exports: None,
        dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
        optional_dependencies: HashMap::new(),
        dev_dependencies: HashMap::new(),
    };
    
    let integrity = IntegrityMetadata {
        registry: "npmjs.org".to_string(),
        registry_integrity: None,
        shasum: None,
        signatures: vec![],
    };
    
    let hash1 = store.store(content1, metadata1.clone(), integrity.clone(), vec![], ContentHash::new(content1), 100, 1).unwrap();
    let hash2 = store.store(content2, metadata2.clone(), integrity, vec![], ContentHash::new(content2), 100, 1).unwrap();
    
    // Same content = same content hash
    assert_eq!(hash1, hash2);
    
    // Only one package stored
    assert_eq!(store.stats().total_packages, 1);
}