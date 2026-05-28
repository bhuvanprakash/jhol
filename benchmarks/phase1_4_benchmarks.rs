use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use jhol_core::binary_lockfile::BinaryLockfile;
use jhol_core::crypto::{QuantumResistantSigner, sign_package, verify_package};
use jhol_core::grm::{ResolutionRecord, ResolutionSource};
use jhol_core::incremental_hashing::{clear_hash_cache, hash_file_cached, IncrementalHasher};
use jhol_core::incremental_resolver::{IncrementalResolver, PackageNode};
use jhol_core::lock_free_grm::LockFreeGRM;
use jhol_core::mpt_store::ContentHash;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MockLockfile {
    name: String,
    version: String,
    deps: Vec<String>,
}

fn sample_record(spec: &str, version: &str) -> ResolutionRecord {
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
    }
}

fn benchmark_binary_lockfile(c: &mut Criterion) {
    let lockfile = MockLockfile {
        name: "react".into(),
        version: "18.2.0".into(),
        deps: (0..256).map(|i| format!("dep-{i}")).collect(),
    };

    c.bench_function("binary_lockfile_create", |b| {
        b.iter(|| BinaryLockfile::from_lockfile(black_box(&lockfile)).unwrap())
    });

    let binary = BinaryLockfile::from_lockfile(&lockfile).unwrap();
    c.bench_function("binary_lockfile_serialize_roundtrip", |b| {
        b.iter(|| {
            let decoded: MockLockfile = black_box(&binary).to_lockfile().unwrap();
            black_box(decoded)
        })
    });
}

fn benchmark_lock_free_grm(c: &mut Criterion) {
    let grm = LockFreeGRM::new();
    let records: Vec<_> = (0..512)
        .map(|i| sample_record(&format!("pkg-{i}"), "1.0.0"))
        .collect();
    for record in records.clone() {
        grm.store(record).unwrap();
    }

    c.bench_function("lock_free_grm_query_throughput", |b| {
        b.iter_batched(
            || records.clone(),
            |batch| {
                for record in batch {
                    black_box(grm.query(&record.package_spec, Some("linux"), Some("20")));
                }
            },
            BatchSize::SmallInput,
        )
    });
}

fn benchmark_incremental_hashing(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("package.json");
    fs::write(&file, b"{\"name\":\"demo\",\"deps\":[\"a\",\"b\",\"c\"]}").unwrap();

    clear_hash_cache();
    c.bench_function("incremental_hashing_cached_file", |b| {
        b.iter(|| black_box(hash_file_cached(black_box(&file)).unwrap()))
    });

    let mut hasher = IncrementalHasher::new(true);
    c.bench_function("incremental_hashing_directory", |b| {
        b.iter(|| black_box(hasher.hash_directory(black_box(dir.path())).unwrap()))
    });
}

fn benchmark_incremental_resolver(c: &mut Criterion) {
    let mut resolver = IncrementalResolver::new();
    for i in 0..2_000 {
        resolver.add_package(PackageNode {
            name: format!("pkg-{i}"),
            version: "1.0.0".into(),
            dependencies: vec!["dep-a".into(), "dep-b".into()],
        });
    }

    c.bench_function("incremental_resolver_update_speed", |b| {
        b.iter(|| {
            resolver.update_package("pkg-1000", black_box("1.0.1"));
            black_box(resolver.get_resolved())
        })
    });
}

fn benchmark_crypto(c: &mut Criterion) {
    let signer = QuantumResistantSigner::generate().unwrap();
    let content = vec![7u8; 64 * 1024];

    c.bench_function("crypto_encrypt_decrypt_latency", |b| {
        b.iter(|| {
            let signature = signer.sign(black_box(&content));
            black_box(signer.verify(&content, &signature))
        })
    });

    c.bench_function("crypto_package_sign_verify", |b| {
        b.iter(|| {
            let signature = sign_package(black_box(&content)).unwrap();
            black_box(verify_package(&content, &signature, signer.public_key()))
        })
    });
}

criterion_group!(
    phase1_4_benches,
    benchmark_binary_lockfile,
    benchmark_lock_free_grm,
    benchmark_incremental_hashing,
    benchmark_incremental_resolver,
    benchmark_crypto
);
criterion_main!(phase1_4_benches);
