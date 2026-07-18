//! Tests for GRM

use crate::grm::{GRMQuery, GRMStore, FreshnessRequirement, ResolutionRecord, ResolutionSource};
use crate::mpt_store::ContentHash;
use std::time::Duration;

fn make_record(package_spec: &str, version: &str, content_hash: &[u8]) -> ResolutionRecord {
    ResolutionRecord {
        id: 0,
        package_spec: package_spec.to_string(),
        resolved_version: version.to_string(),
        content_hash: ContentHash::new(content_hash),
        dependency_hashes: vec![],
        platform: Some("darwin-arm64".to_string()),
        node_version: Some("20.15.0".to_string()),
        resolution_hash: ContentHash::default(),
        cached_at: 0,
        hit_count: 1,
        source: ResolutionSource::GRM,
        cache_priority: 100,
    }
}

#[test]
fn test_store_and_query() {
    let mut store = GRMStore::memory();
    
    let record = make_record("lodash", "4.17.21", b"content");
    let id = store.store(record).unwrap();
    assert_eq!(id, 1);
    
    // Query
    let found = store.query("lodash", Some("darwin-arm64"), Some("20.15.0"));
    assert!(found.is_some());
    
    let found = found.unwrap();
    assert_eq!(found.resolved_version, "4.17.21");
}

#[test]
fn test_query_full() {
    let mut store = GRMStore::memory();
    
    let record1 = make_record("react", "18.2.0", b"react-content");
    let record2 = make_record("lodash", "4.17.21", b"lodash-content");
    
    store.store(record1).unwrap();
    store.store(record2).unwrap();
    
    let query = GRMQuery::new("react");
    let results = store.query_full(&query);
    assert!(!results.is_empty());
    assert_eq!(results[0].package_spec, "react");
}

#[test]
fn test_freshness_requirement() {
    let fr = FreshnessRequirement::Within(Duration::from_secs(3600));
    assert!(fr.is_fresh(1000)); // 1 second old
    assert!(!fr.is_fresh(4 * 3600 * 1000)); // 4 hours old
    
    let fr2 = FreshnessRequirement::RecentHits(5);
    assert!(fr2.has_hits(10));
    assert!(!fr2.has_hits(3));
}

#[test]
fn test_stats() {
    let mut store = GRMStore::memory();
    
    let record1 = make_record("pkg-a", "1.0.0", b"content1");
    let record2 = make_record("pkg-b", "2.0.0", b"content2");
    
    store.store(record1).unwrap();
    store.store(record2).unwrap();
    
    let stats = store.stats();
    assert_eq!(stats.total_records, 2);
    assert_eq!(stats.unique_packages, 2);
}

#[test]
fn test_get_all_versions() {
    let mut store = GRMStore::memory();
    
    let record1 = make_record("lodash", "4.17.20", b"content");
    let record2 = make_record("lodash", "4.17.21", b"content");
    let record3 = make_record("lodash", "4.18.0", b"content3");
    
    store.store(record1).unwrap();
    store.store(record2).unwrap();
    store.store(record3).unwrap();
    
    let versions = store.get_all_versions("lodash");
    assert_eq!(versions.len(), 3);
}
