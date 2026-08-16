use criterion::{black_box, criterion_group, criterion_main, Criterion};
use jhol_pipeline::{JGridPipeline, ResolvedPackage};
use tempfile::tempdir;

fn swiss_table_lookup(c: &mut Criterion) {
    let table = jhol_pipeline::SwissTable::new(256, 42);
    let hash = jhol_pipeline::SwissTable::compute_hash(b"hello world");
    c.bench_function("swiss_table_lookup", |b| {
        b.iter(|| {
            black_box(table.lookup(&hash));
        });
    });
}

fn build_dir_tree(c: &mut Criterion) {
    const N: usize = 100;
    let paths: Vec<String> = (0..N).map(|i| format!("src/lib/module_{i}.js")).collect();
    let path_segs: Vec<Vec<&str>> = paths.iter().map(|p| p.split('/').collect()).collect();
    let files: Vec<(Vec<&str>, [u8; 32], u16)> = path_segs
        .into_iter()
        .enumerate()
        .map(|(i, segs)| (segs, [i as u8; 32], 0o644))
        .collect();

    c.bench_function("build_directory_tree_100_files", |b| {
        b.iter(|| {
            black_box(jhol_pipeline::build_directory_tree(&files));
        });
    });
}

fn setup_pipeline(
    dir: &std::path::Path,
    name: &str,
    version: &str,
    nfiles: usize,
) -> JGridPipeline {
    let mut pipeline = JGridPipeline::new(dir).unwrap();
    let entries: Vec<(String, u16, Vec<u8>)> = (0..nfiles)
        .map(|i| {
            (
                format!("src/mod_{i}.js"),
                0o644,
                format!("content_{i}").into_bytes(),
            )
        })
        .collect();
    let entries_refs: Vec<(&str, u16, &[u8])> = entries
        .iter()
        .map(|(p, m, c)| (p.as_str(), *m, c.as_slice()))
        .collect();
    pipeline.ingest(name, version, &entries_refs).unwrap();
    pipeline
}

/// L0: clone-ready bundle — materialize via cloning an existing tree
fn l0_bundle_materialization(c: &mut Criterion) {
    c.bench_function("l0_bundle_clone", |b| {
        b.iter_custom(|iters| {
            // Fresh pipeline per sample so L0 always hits a hot bundle
            let dir = tempdir().unwrap();
            let pipeline = setup_pipeline(dir.path(), "test-pkg", "1.0.0", 50);

            // Warmup: builds bundle
            pipeline
                .materialize(
                    &[ResolvedPackage {
                        name: "test-pkg".into(),
                        version: "1.0.0".into(),
                    }],
                    &dir.path().join("warm"),
                )
                .unwrap();

            let mut total = std::time::Duration::ZERO;
            for i in 0..iters {
                let target = dir.path().join(format!("nm_{i}"));
                let start = std::time::Instant::now();
                pipeline
                    .materialize(
                        &[ResolvedPackage {
                            name: "test-pkg".into(),
                            version: "1.0.0".into(),
                        }],
                        &target,
                    )
                    .unwrap();
                total += start.elapsed();
            }
            total
        });
    });
}

/// L1: tree cache — materialize per-package pre-assembled trees
fn l1_tree_cache(c: &mut Criterion) {
    c.bench_function("l1_tree_cache", |b| {
        b.iter_custom(|iters| {
            let dir = tempdir().unwrap();
            let pipeline = setup_pipeline(dir.path(), "cache-pkg", "2.0.0", 10);

            // Warmup: builds tree cache
            pipeline
                .materialize(
                    &[ResolvedPackage {
                        name: "cache-pkg".into(),
                        version: "2.0.0".into(),
                    }],
                    &dir.path().join("warm"),
                )
                .unwrap();

            let mut total = std::time::Duration::ZERO;
            for i in 0..iters {
                let target = dir.path().join(format!("nm_{i}"));
                let start = std::time::Instant::now();
                pipeline
                    .materialize(
                        &[ResolvedPackage {
                            name: "cache-pkg".into(),
                            version: "2.0.0".into(),
                        }],
                        &target,
                    )
                    .unwrap();
                total += start.elapsed();
            }
            total
        });
    });
}

/// L2: JGrid content blocks — build from mmap'd content-addressed blocks
fn l2_jgrid_blocks(c: &mut Criterion) {
    c.bench_function("l2_block_materialize", |b| {
        b.iter_custom(|iters| {
            let dir = tempdir().unwrap();
            let pipeline = setup_pipeline(dir.path(), "blocks-pkg", "3.0.0", 5);

            let mut total = std::time::Duration::ZERO;
            for i in 0..iters {
                let target = dir.path().join(format!("nm_{i}"));
                let start = std::time::Instant::now();
                pipeline
                    .materialize(
                        &[ResolvedPackage {
                            name: "blocks-pkg".into(),
                            version: "3.0.0".into(),
                        }],
                        &target,
                    )
                    .unwrap();
                total += start.elapsed();
            }
            total
        });
    });
}

/// L2 performance at scale: 1000 blocks from JGrid
fn l2_scale(c: &mut Criterion) {
    c.bench_function("l2_scale_1000_blocks", |b| {
        b.iter_custom(|iters| {
            let dir = tempdir().unwrap();
            let pipeline = setup_pipeline(dir.path(), "big-pkg", "4.0.0", 1000);

            let mut total = std::time::Duration::ZERO;
            for i in 0..iters {
                let target = dir.path().join(format!("nm_{i}"));
                let start = std::time::Instant::now();
                pipeline
                    .materialize(
                        &[ResolvedPackage {
                            name: "big-pkg".into(),
                            version: "4.0.0".into(),
                        }],
                        &target,
                    )
                    .unwrap();
                total += start.elapsed();
            }
            total
        });
    });
}

criterion_group!(
    benches,
    swiss_table_lookup,
    build_dir_tree,
    l0_bundle_materialization,
    l1_tree_cache,
    l2_jgrid_blocks,
    l2_scale,
);
criterion_main!(benches);
