//! Tests for Provenance Log

use crate::provenance_log::{InstallMetadata, InstallType, IPLEntry, ProvenanceLog};
use crate::mpt_store::ContentHash;

#[test]
fn test_append_and_get() {
    let mut log = ProvenanceLog::memory();
    
    let entry = IPLEntry {
        version: 1,
        timestamp: 0,
        sequence: 0, // Will be auto-assigned
        root_hash: ContentHash::new(b"root"),
        platform: "darwin-arm64".to_string(),
        node_version: "20.15.0".to_string(),
        registry: "npmjs.org".to_string(),
        install_type: InstallType::Fresh,
        integrity_hash: ContentHash::new(b"integrity"),
        prev_entry_hash: ContentHash::default(),
        metadata: InstallMetadata {
            project: Some("/test/project".to_string()),
            lockfile_hash: Some("abc123".to_string()),
            duration_ms: Some(100),
            packages_count: Some(10),
            user: None,
            hostname: None,
        },
        entry_hash: ContentHash::default(), // Will be computed
    };
    
    let seq = log.append(entry).unwrap();
    assert_eq!(seq, 1);
    
    let retrieved = log.get(seq).unwrap();
    assert!(retrieved.is_some());
    
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.sequence, 1);
    assert_eq!(retrieved.platform, "darwin-arm64");
}

#[test]
fn test_chain_integrity() {
    let mut log = ProvenanceLog::memory();
    
    // Add first entry
    let entry1 = IPLEntry {
        version: 1,
        timestamp: 1000,
        sequence: 0,
        root_hash: ContentHash::new(b"root1"),
        platform: "linux".to_string(),
        node_version: "20.0.0".to_string(),
        registry: "npmjs.org".to_string(),
        install_type: InstallType::Fresh,
        integrity_hash: ContentHash::new(b"int1"),
        prev_entry_hash: ContentHash::default(),
        metadata: InstallMetadata::default(),
        entry_hash: ContentHash::default(),
    };
    
    log.append(entry1).unwrap();
    
    // Add second entry
    let entry2 = IPLEntry {
        version: 1,
        timestamp: 2000,
        sequence: 0,
        root_hash: ContentHash::new(b"root2"),
        platform: "linux".to_string(),
        node_version: "20.0.0".to_string(),
        registry: "npmjs.org".to_string(),
        install_type: InstallType::Cached,
        integrity_hash: ContentHash::new(b"int2"),
        prev_entry_hash: ContentHash::default(), // Will be set by append
        metadata: InstallMetadata::default(),
        entry_hash: ContentHash::default(),
    };
    
    log.append(entry2).unwrap();
    
    // Verify chain
    let is_valid = log.verify_chain().unwrap();
    assert!(is_valid);
}

#[test]
fn test_recent_entries() {
    let mut log = ProvenanceLog::memory();
    
    for i in 0..5 {
        let entry = IPLEntry {
            version: 1,
            timestamp: i * 1000,
            sequence: 0,
            root_hash: ContentHash::new(&[i]),
            platform: "linux".to_string(),
            node_version: "20.0.0".to_string(),
            registry: "npmjs.org".to_string(),
            install_type: InstallType::Fresh,
            integrity_hash: ContentHash::new(&[i]),
            prev_entry_hash: ContentHash::default(),
            metadata: InstallMetadata::default(),
            entry_hash: ContentHash::default(),
        };
        log.append(entry).unwrap();
    }
    
    let recent = log.recent(3);
    assert_eq!(recent.len(), 3);
}

#[test]
fn test_attestation() {
    let mut log = ProvenanceLog::memory();
    
    let entry = IPLEntry {
        version: 1,
        timestamp: 1000,
        sequence: 0,
        root_hash: ContentHash::new(b"root"),
        platform: "darwin".to_string(),
        node_version: "20.0.0".to_string(),
        registry: "npmjs.org".to_string(),
        install_type: InstallType::Fresh,
        integrity_hash: ContentHash::new(b"integrity"),
        prev_entry_hash: ContentHash::default(),
        metadata: InstallMetadata::default(),
        entry_hash: ContentHash::default(),
    };
    
    log.append(entry).unwrap();
    
    let attestation = log.attest(1).unwrap();
    assert!(attestation.is_some());
    
    let att = attestation.unwrap();
    assert_eq!(att.entry_sequence, 1);
    assert!(log.verify_attestation(&att));
}