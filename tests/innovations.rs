//! Phase innovations: binary lockfile, lock-free GRM, incremental hashing, distributed GRM, Markov, crypto, io_uring, FUSE.

#[cfg(test)]
mod binary_lockfile_tests {
    use jhol_core::binary_lockfile::BinaryLockfile;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct MockLockfile {
        name: String,
        version: String,
        deps: Vec<String>,
    }

    #[test]
    fn test_create_binary_lockfile() {
        let lockfile = MockLockfile {
            name: "lodash".into(),
            version: "4.17.21".into(),
            deps: vec!["@types/node".into()],
        };
        let result = BinaryLockfile::from_lockfile(&lockfile);
        assert!(result.is_ok());
    }

    #[test]
    fn test_binary_lockfile_roundtrip_and_persistence() {
        let lockfile = MockLockfile {
            name: "react".into(),
            version: "18.2.0".into(),
            deps: vec!["scheduler".into()],
        };
        let binary = BinaryLockfile::from_lockfile(&lockfile).unwrap();
        let decoded: MockLockfile = binary.to_lockfile().unwrap();
        assert_eq!(decoded, lockfile);
        assert_eq!(binary.header.version, 1);
        assert!(binary.header.compressed_size > 0);

        let tmp = tempfile::NamedTempFile::new().unwrap();
        binary.save(tmp.path()).unwrap();
        assert!(BinaryLockfile::is_binary_format(tmp.path()));
        let loaded = BinaryLockfile::load(tmp.path()).unwrap();
        let decoded_loaded: MockLockfile = loaded.to_lockfile().unwrap();
        assert_eq!(decoded_loaded, lockfile);
    }
}

#[cfg(test)]
mod lock_free_grm_tests {
    use jhol_core::grm::{ResolutionRecord, ResolutionSource};
    use jhol_core::lock_free_grm::LockFreeGRM;
    use jhol_core::mpt_store::ContentHash;

    fn record(spec: &str, version: &str) -> ResolutionRecord {
        ResolutionRecord {
            id: 0,
            package_spec: spec.into(),
            resolved_version: version.into(),
            content_hash: ContentHash::new(format!("{spec}@{version}").as_bytes()),
            dependency_hashes: vec![],
            platform: Some("linux".into()),
            node_version: Some("20".into()),
            resolution_hash: ContentHash::new(format!("resolution:{spec}@{version}").as_bytes()),
            provenance_sequence: None,
            cached_at: 1,
            hit_count: 0,
            source: ResolutionSource::GRM,
            cache_priority: 100,
            dependencies: Default::default(),
            timestamp: 0,
            ttl_secs: 0,
        }
    }

    #[test]
    fn test_create_lock_free_grm() {
        let grm = LockFreeGRM::new();
        assert_eq!(grm.stats().total_records, 0);
    }

    #[test]
    fn test_lock_free_grm_store_and_query() {
        let grm = LockFreeGRM::new();
        let rec = record("lodash", "4.17.21");
        let id = grm.store(rec.clone()).unwrap();
        assert_eq!(id, 1);

        let found = grm.query("lodash", Some("linux"), Some("20")).unwrap();
        assert_eq!(found.package_spec, rec.package_spec);
        assert_eq!(grm.stats().total_hits, 1);
    }
}

#[cfg(test)]
mod incremental_hashing_tests {
    use jhol_core::incremental_hashing::{
        clear_hash_cache, hash_directory_cached, hash_file_cached, IncrementalHasher,
    };
    use std::fs;

    #[test]
    fn test_create_incremental_hasher() {
        let hasher = IncrementalHasher::new(true);
        assert_eq!(hasher.cache_size(), 0);
    }

    #[test]
    fn test_incremental_hashing_cache_and_directory() {
        clear_hash_cache();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("package.json");
        fs::write(&file, b"{\"name\":\"demo\"}").unwrap();

        let first = hash_file_cached(&file).unwrap();
        let second = hash_file_cached(&file).unwrap();
        assert_eq!(first, second);

        let dir_hash = hash_directory_cached(dir.path()).unwrap();
        assert!(!dir_hash.as_str().is_empty());
    }
}

#[cfg(test)]
mod incremental_resolver_tests {
    use jhol_core::incremental_resolver::{get_resolved_global, IncrementalResolver, PackageNode};

    #[test]
    fn test_create_incremental_resolver() {
        let resolver = IncrementalResolver::new();
        assert!(resolver.get_resolved().is_empty());
    }

    #[test]
    fn test_incremental_resolver_add_update_remove() {
        let mut resolver = IncrementalResolver::new();
        resolver.add_package(PackageNode {
            name: "react".into(),
            version: "18.2.0".into(),
            dependencies: vec!["scheduler".into()],
        });
        resolver.update_package("react", "18.3.0");
        let resolved = resolver.get_resolved();
        assert_eq!(resolved[0].version, "18.3.0");
        resolver.remove_package("react");
        assert!(resolver.get_resolved().is_empty());
        let _ = get_resolved_global();
    }
}

#[cfg(test)]
mod crypto_tests {
    use jhol_core::crypto::{sign_package, verify_package, QuantumResistantSigner};

    #[test]
    fn test_create_quantum_resistant_signer() {
        let signer = QuantumResistantSigner::generate().unwrap();
        assert!(!signer.public_key().is_empty());
    }

    #[test]
    fn test_crypto_sign_and_verify() {
        let signer = QuantumResistantSigner::generate().unwrap();
        let content = b"package tarball bytes";
        let signature = signer.sign(content);
        assert!(signer.verify(content, &signature));
        assert!(verify_package(content, &signature, signer.public_key()));
        assert!(sign_package(content).unwrap().len() >= 32);
    }
}

#[cfg(test)]
mod distributed_grm_tests {
    use jhol_core::distributed_grm::{
        publish_resolution, query_distributed, DistributedGRM, P2PResolutionRecord,
    };

    #[test]
    fn test_create_distributed_grm() {
        let grm = DistributedGRM::new();
        assert!(grm.query_dht("lodash").is_none());
    }

    #[test]
    fn test_distributed_grm_publish_and_query() {
        let mut grm = DistributedGRM::new();
        let record = P2PResolutionRecord {
            package_spec: "lodash".into(),
            resolved_version: "4.17.21".into(),
            content_hash: "abc123".into(),
            platform: "linux".into(),
            node_version: Some("20".into()),
            peer_count: 7,
        };
        grm.publish_to_dht(record.clone());
        assert_eq!(grm.query_dht("lodash").unwrap().resolved_version, "4.17.21");
        publish_resolution(record);
        assert!(query_distributed("lodash").is_some());
    }
}

#[cfg(test)]
mod second_order_markov_tests {
    use jhol_core::second_order_markov::{predict_with_context, SecondOrderMarkov};

    #[test]
    fn test_create_second_order_markov() {
        let model = SecondOrderMarkov::new();
        assert!(model.predict_top("a", "b", 1).is_empty());
    }

    #[test]
    fn test_second_order_markov_learn_and_predict() {
        let mut model = SecondOrderMarkov::new();
        let seq = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        model.learn_from_sequence(&seq);
        assert_eq!(model.predict_top("a", "b", 1), vec!["c".to_string()]);

        predict_with_context("a", "b", 1);
    }
}

#[cfg(target_os = "linux")]
#[cfg(test)]
mod fuse_mount_tests {
    use jhol_core::fuse_mount::{instant_mount_node_modules, JholFS};

    #[test]
    fn test_create_fuse_mount() {
        let fs = JholFS::new();
        let _ = fs;
    }

    #[test]
    fn test_fuse_mount_manifest_and_mount() {
        let dir = tempfile::tempdir().unwrap();
        let mut fs = JholFS::new();
        fs.build_manifest(&dir.path().join("package-lock.json"))
            .unwrap();
        fs.mount(dir.path().join("node_modules")).unwrap();
        assert!(instant_mount_node_modules(dir.path()).is_ok());
    }
}
