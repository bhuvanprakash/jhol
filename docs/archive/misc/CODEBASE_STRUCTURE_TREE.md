# Jhol Codebase File Tree & Architectural Map

This document contains the complete, curated structural tree of every folder and source file in the **Jhol** repository.

## 1. Complete Repository Directory Tree

```text
jhol/
├── AGENTS.md
├── CHANGELOG.md
├── CODEBASE_STRUCTURE_TREE.md
├── CONTRIBUTING.md
├── Cargo.lock
├── Cargo.toml
├── Documentation/
│   ├── BENCHMARK_IDEAS_APPLIED.md
│   ├── BOTTLENECKS_AND_PRODUCTION_READINESS.md
│   ├── BOTTLENECKS_JHOL_VS_BUN.md
│   ├── JAGR2_SEMANTICS.md
│   ├── LOCKFILE_DESIGN.md
│   ├── README.md
│   ├── SCOPE_AND_STABILITY.md
│   └── main.md
├── GET_STARTED.md
├── INNOVATION_LOG.md
├── LICENSE
├── PRODUCTION_HARDENING_RECORD.md
├── QWEN.md
├── README-highdata.md
├── README.md
├── RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.pdf
├── RESULTS.md
├── ROADMAP.md
├── SECURITY.md
├── benchmarks/
│   ├── baseline.json
│   ├── benchmark-fair-true-cold-v1.json
│   ├── benchmark-fair-true-cold-v2-post-ocp-with-npm.json
│   ├── benchmark-latest-v10-parallel-manifest-conc32.json
│   ├── benchmark-latest-v10-parallel-manifest-conc32.md
│   ├── benchmark-latest-v11-hash-sri-singlepass-conc32.json
│   ├── benchmark-latest-v11-hash-sri-singlepass-conc32.md
│   ├── benchmark-latest-v12-streaming-download-conc32.json
│   ├── benchmark-latest-v12-streaming-download-conc32.md
│   ├── benchmark-latest-v13-recheck-conc32-r7.json
│   ├── benchmark-latest-v13-recheck-conc32-r7.md
│   ├── benchmark-latest-v14-ocp-profiler-conc32.json
│   ├── benchmark-latest-v14-ocp-profiler-conc32.md
│   ├── benchmark-latest-v2.json
│   ├── benchmark-latest-v2.md
│   ├── benchmark-latest-v3.json
│   ├── benchmark-latest-v3.md
│   ├── benchmark-latest-v4.json
│   ├── benchmark-latest-v4.md
│   ├── benchmark-latest-v5.json
│   ├── benchmark-latest-v5.md
│   ├── benchmark-latest-v6-conc32.json
│   ├── benchmark-latest-v6-conc32.md
│   ├── benchmark-latest-v7-reqwest-conc32.json
│   ├── benchmark-latest-v7-reqwest-conc32.md
│   ├── benchmark-latest-v7-ureq-conc32.json
│   ├── benchmark-latest-v7-ureq-conc32.md
│   ├── benchmark-latest-v8-indexfix-conc32.json
│   ├── benchmark-latest-v8-indexfix-conc32.md
│   ├── benchmark-latest-v9-manifestfastpath-conc32.json
│   ├── benchmark-latest-v9-manifestfastpath-conc32.md
│   ├── benchmark-latest.json
│   ├── benchmark-latest.md
│   ├── benchmark-next.json
│   ├── benchmark-next.md
│   ├── brutal/
│   │   ├── README.md
│   │   ├── bench.sh
│   │   └── brutal_v2.sh
│   ├── enterprise_guardrails.json
│   ├── fallback_trend_guardrails.json
│   ├── framework_guardrails.json
│   ├── framework_matrix.json
│   ├── legacy/
│   │   ├── baseline.json
│   │   ├── benchmark-fair-true-cold-v1.json
│   │   ├── benchmark-fair-true-cold-v2-post-ocp-with-npm.json
│   │   ├── benchmark-latest-v10-parallel-manifest-conc32.json
│   │   ├── benchmark-latest-v10-parallel-manifest-conc32.md
│   │   ├── benchmark-latest-v11-hash-sri-singlepass-conc32.json
│   │   ├── benchmark-latest-v11-hash-sri-singlepass-conc32.md
│   │   ├── benchmark-latest-v12-streaming-download-conc32.json
│   │   ├── benchmark-latest-v12-streaming-download-conc32.md
│   │   ├── benchmark-latest-v13-recheck-conc32-r7.json
│   │   ├── benchmark-latest-v13-recheck-conc32-r7.md
│   │   ├── benchmark-latest-v14-ocp-profiler-conc32.json
│   │   ├── benchmark-latest-v14-ocp-profiler-conc32.md
│   │   ├── benchmark-latest-v2.json
│   │   ├── benchmark-latest-v2.md
│   │   ├── benchmark-latest-v3.json
│   │   ├── benchmark-latest-v3.md
│   │   ├── benchmark-latest-v4.json
│   │   ├── benchmark-latest-v4.md
│   │   ├── benchmark-latest-v5.json
│   │   ├── benchmark-latest-v5.md
│   │   ├── benchmark-latest-v6-conc32.json
│   │   ├── benchmark-latest-v6-conc32.md
│   │   ├── benchmark-latest-v7-reqwest-conc32.json
│   │   ├── benchmark-latest-v7-reqwest-conc32.md
│   │   ├── benchmark-latest-v7-ureq-conc32.json
│   │   ├── benchmark-latest-v7-ureq-conc32.md
│   │   ├── benchmark-latest-v8-indexfix-conc32.json
│   │   ├── benchmark-latest-v8-indexfix-conc32.md
│   │   ├── benchmark-latest-v9-manifestfastpath-conc32.json
│   │   ├── benchmark-latest-v9-manifestfastpath-conc32.md
│   │   ├── benchmark-latest.json
│   │   ├── benchmark-latest.md
│   │   ├── benchmark-next.json
│   │   ├── benchmark-next.md
│   │   ├── enterprise_guardrails.json
│   │   ├── fallback_trend_guardrails.json
│   │   ├── framework_guardrails.json
│   │   ├── framework_matrix.json
│   │   ├── live-bench-20260218-200004.json
│   │   ├── live-bench-20260218-203813.json
│   │   ├── live-bench-20260307-194257.json
│   │   ├── live-bench-20260307-195151.json
│   │   ├── live-bench-20260702-013136.json
│   │   ├── resolver-parity-report-latest.json
│   │   ├── resolver_parity_guardrails.json
│   │   └── week1_guardrails.json
│   ├── live-bench-20260218-200004.json
│   ├── live-bench-20260218-203813.json
│   ├── macos-arm64/
│   │   ├── 2026-09-17-after.json
│   │   ├── 2026-09-17-before.json
│   │   └── 2026-09-17-final.json
│   ├── materialization.rs
│   ├── phase1_4_benchmarks.rs
│   ├── profiles/
│   │   ├── jhol-async-prefetch-joined-profile.txt
│   │   ├── jhol-async-prefetch-profile.txt
│   │   ├── jhol-cold-profile-after-artifact-plan.txt
│   │   ├── jhol-cold-profile-artifact-identity-verbose.txt
│   │   ├── jhol-cold-profile-artifact-identity.txt
│   │   ├── jhol-cold-profile-artifact-prune-fair.txt
│   │   ├── jhol-cold-profile-artifact-prune.txt
│   │   ├── jhol-cold-profile-math-dedup.txt
│   │   ├── jhol-cold-profile-math-queue.txt
│   │   ├── jhol-compact-summary-profile.txt
│   │   ├── jhol-current-root-bottleneck-profile.txt
│   │   ├── jhol-metadata-reqwest-profile.txt
│   │   ├── jhol-persistent-compact-cold-profile.txt
│   │   ├── jhol-persistent-compact-v2-cold-profile.txt
│   │   ├── jhol-persistent-compact-v2-warm-profile.txt
│   │   └── jhol-persistent-compact-warm-profile.txt
│   ├── resolver-parity-report-latest.json
│   ├── resolver_parity_guardrails.json
│   └── week1_guardrails.json
├── binary-packages/
│   ├── 000da189aa1588996e80d25512d213871eb4074bfaaca05a44e1687ddc74653c.jhol
│   ├── 01043e85c7e68ba459ea112a07ba23776dfb98cd6b2f1d0ce4328c0760fa5fe6.jhol
│   ├── 01766931e7f616e61d59ddaedfa23869e8d0df4641245d70ecbad2b415bf0d4b.jhol
│   └── ... [199 more pre-packed binary tarballs/entries]
├── build.rs
├── crates/
│   ├── jhol-bundle/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── cas.rs
│   │       ├── codegen.rs
│   │       ├── config.rs
│   │       ├── css.rs
│   │       ├── emit.rs
│   │       ├── errors.rs
│   │       ├── externals.rs
│   │       ├── graph.rs
│   │       ├── ingest.rs
│   │       ├── lib.rs
│   │       ├── minify.rs
│   │       ├── parse.rs
│   │       ├── parser.rs
│   │       ├── plugins.rs
│   │       ├── rewrite.rs
│   │       ├── sourcemap.rs
│   │       ├── split.rs
│   │       ├── transform.rs
│   │       ├── tree_shake.rs
│   │       └── watch.rs
│   ├── jhol-core/
│   │   ├── Cargo.toml
│   │   ├── Cargo.toml.bak
│   │   ├── build.rs
│   │   ├── data/
│   │   │   ├── binary-packages-ai-web-200.json
│   │   │   ├── binary-packages-hot-small.json
│   │   │   ├── binary-packages-top100.json
│   │   │   ├── binary-packages-top1000.json
│   │   │   └── bundled-index.json
│   │   ├── src/
│   │   │   ├── atomic_ring.rs
│   │   │   ├── audit.rs
│   │   │   ├── backend.rs
│   │   │   ├── bin_links.rs
│   │   │   ├── binary_cache.rs
│   │   │   ├── binary_lockfile.rs
│   │   │   ├── binary_manifest.rs
│   │   │   ├── bucket_vsids.rs
│   │   │   ├── capsule.rs
│   │   │   ├── cas/
│   │   │   │   ├── cas.rs
│   │   │   │   ├── hardlink.rs
│   │   │   │   ├── integrity.rs
│   │   │   │   └── mod.rs
│   │   │   ├── cdn.rs
│   │   │   ├── config.rs
│   │   │   ├── content_addressed/
│   │   │   │   └── tests.rs
│   │   │   ├── content_addressed_store.rs
│   │   │   ├── crypto.rs
│   │   │   ├── daemon_cache.rs
│   │   │   ├── decentralized_registry.rs
│   │   │   ├── distributed_grm.rs
│   │   │   ├── doctor.rs
│   │   │   ├── enterprise.rs
│   │   │   ├── error_handling.rs
│   │   │   ├── exec.rs
│   │   │   ├── fast_stream/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── ssdp.rs
│   │   │   │   ├── twm.rs
│   │   │   │   └── vrb.rs
│   │   │   ├── feature_graph.rs
│   │   │   ├── file_lock.rs
│   │   │   ├── fuse_mount.rs
│   │   │   ├── git_lifecycle.rs
│   │   │   ├── global_cache.rs
│   │   │   ├── grm/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── query.rs
│   │   │   │   ├── store/
│   │   │   │   │   └── tests.rs
│   │   │   │   ├── store.rs
│   │   │   │   └── sync.rs
│   │   │   ├── harvest.rs
│   │   │   ├── http2_download.rs
│   │   │   ├── http_client.rs
│   │   │   ├── hypergraph_resolver.rs
│   │   │   ├── incremental_hashing.rs
│   │   │   ├── incremental_resolution.rs
│   │   │   ├── incremental_resolver.rs
│   │   │   ├── install/
│   │   │   │   └── lockfile_patch.rs
│   │   │   ├── install.rs
│   │   │   ├── install_log.rs
│   │   │   ├── instant_lock/
│   │   │   │   └── tests.rs
│   │   │   ├── io_uring_wrapper.rs
│   │   │   ├── jagr/
│   │   │   │   ├── betti_numbers.rs
│   │   │   │   ├── conflict_explainer.rs
│   │   │   │   ├── core_guided_lp.rs
│   │   │   │   ├── core_guided_solver.rs
│   │   │   │   ├── jagr_resolver.rs
│   │   │   │   ├── knot.rs
│   │   │   │   ├── maxsat_solver.rs
│   │   │   │   ├── mayer_vietoris.rs
│   │   │   │   ├── mod.rs
│   │   │   │   ├── ol3p_solver.rs
│   │   │   │   ├── resolution_differential.rs
│   │   │   │   ├── simplicial_normal_form.rs
│   │   │   │   ├── tropical_math.rs
│   │   │   │   ├── tropical_max_atom.rs
│   │   │   │   ├── tropical_nullstellensatz.rs
│   │   │   │   ├── tropical_polyhedron.rs
│   │   │   │   ├── tropical_sheaf.rs
│   │   │   │   └── tropical_sheaf_analyzer.rs
│   │   │   ├── jagr_infinite.rs
│   │   │   ├── jagrs.rs
│   │   │   ├── jgrid/
│   │   │   │   ├── PLAN.md
│   │   │   │   ├── README.md
│   │   │   │   ├── format/
│   │   │   │   │   ├── header.rs
│   │   │   │   │   ├── manifest.rs
│   │   │   │   │   ├── mod.rs
│   │   │   │   │   ├── perfect_hash.rs
│   │   │   │   │   ├── string_table.rs
│   │   │   │   │   ├── swiss_table.rs
│   │   │   │   │   └── tree_dag.rs
│   │   │   │   ├── ingester/
│   │   │   │   │   ├── from_tarball.rs
│   │   │   │   │   ├── mod.rs
│   │   │   │   │   └── tree_builder.rs
│   │   │   │   ├── materialize/
│   │   │   │   │   ├── cloner.rs
│   │   │   │   │   ├── installer.rs
│   │   │   │   │   └── mod.rs
│   │   │   │   ├── mod.rs
│   │   │   │   ├── spec.md
│   │   │   │   └── store/
│   │   │   │       ├── builder.rs
│   │   │   │       ├── bundle_cache.rs
│   │   │   │       ├── gc.rs
│   │   │   │       ├── grid_store.rs
│   │   │   │       ├── mod.rs
│   │   │   │       └── tree_cache.rs
│   │   │   ├── js_engine_backend.rs
│   │   │   ├── lattice_cache.rs
│   │   │   ├── layout.rs
│   │   │   ├── lib.rs
│   │   │   ├── lifecycle.rs
│   │   │   ├── lock_free_grm.rs
│   │   │   ├── lockfile.rs
│   │   │   ├── lockfile_write.rs
│   │   │   ├── mcp.rs
│   │   │   ├── mpt_store/
│   │   │   │   ├── mod.rs
│   │   │   │   └── tests.rs
│   │   │   ├── offline_cache.rs
│   │   │   ├── optimized_download.rs
│   │   │   ├── osv.rs
│   │   │   ├── package_index.rs
│   │   │   ├── pool.rs
│   │   │   ├── predictive_prefetch.rs
│   │   │   ├── prefetch.rs
│   │   │   ├── progress.rs
│   │   │   ├── provenance_log/
│   │   │   │   └── tests.rs
│   │   │   ├── pubgrub/
│   │   │   │   ├── incompatibility.rs
│   │   │   │   ├── minimal.rs
│   │   │   │   ├── mod.rs
│   │   │   │   ├── partial_solution.rs
│   │   │   │   ├── solver.rs
│   │   │   │   ├── term.rs
│   │   │   │   ├── tests.rs
│   │   │   │   ├── version_set.rs
│   │   │   │   └── vsids.rs
│   │   │   ├── quantum_signing.rs
│   │   │   ├── registry.rs
│   │   │   ├── registry_tests.rs
│   │   │   ├── resolution_mirror.rs
│   │   │   ├── run.rs
│   │   │   ├── safe_file.rs
│   │   │   ├── sandbox.rs
│   │   │   ├── sat_resolver.rs
│   │   │   ├── sat_resolver_tests.rs
│   │   │   ├── second_order_markov.rs
│   │   │   ├── security_guard.rs
│   │   │   ├── selective_extract.rs
│   │   │   ├── soul_path.rs
│   │   │   ├── spec_grammar.rs
│   │   │   ├── spectral_sparsifier.rs
│   │   │   ├── task_queue/
│   │   │   │   ├── adaptive.rs
│   │   │   │   ├── mod.rs
│   │   │   │   ├── parallel.rs
│   │   │   │   ├── queue.rs
│   │   │   │   └── worker.rs
│   │   │   ├── utils.rs
│   │   │   ├── ux.rs
│   │   │   ├── workspaces.rs
│   │   │   └── zero_copy_metadata.rs
│   │   └── tests/
│   │       └── capsule_adversarial.rs
│   └── jhol-runtime/
│       ├── Cargo.toml
│       ├── README.md
│       ├── examples/
│       │   ├── bench.rs
│       │   └── bench_jsc.rs
│       └── src/
│           ├── bytecode_cache.rs
│           ├── engine.rs
│           ├── errors.rs
│           ├── jit.rs
│           ├── jsc_engine.rs
│           ├── lib.rs
│           ├── module_cache.rs
│           ├── modules/
│           │   ├── buffer.rs
│           │   ├── console.rs
│           │   ├── crypto.rs
│           │   ├── events.rs
│           │   ├── fs.rs
│           │   ├── http.rs
│           │   ├── mod.rs
│           │   ├── os.rs
│           │   ├── path.rs
│           │   ├── process.rs
│           │   ├── timers.rs
│           │   └── util.rs
│           ├── resolver.rs
│           ├── timer.rs
│           ├── transpile.rs
│           ├── transpile_cache.rs
│           ├── tropical_graph.rs
│           ├── tropical_graph_tests.rs
│           └── watch.rs
├── docs/
│   ├── BOTTLENECK_ANALYSIS.md
│   ├── JAGR1.md
│   ├── KNOT_RISING_STAR.md
│   ├── PHASE_1_4_COMPLETE.md
│   ├── PRODUCTION_CHECKLIST.md
│   ├── PRODUCTION_OPTIMIZATION_PLAN.md
│   ├── README.md
│   ├── RELEASE_NOTES_1.0.2.md
│   ├── WEEKLY_EXECUTION_BOARD.md
│   ├── archive/
│   │   ├── 2025-optimization/
│   │   │   ├── 4_WEEK_OPTIMIZATION_FINAL_RESULTS.md
│   │   │   ├── ALL_ISSUES_FIXED_FINAL.md
│   │   │   ├── BENCHMARK_VALIDATION_REPORT.md
│   │   │   ├── BEST_IN_CLASS_OPTIMIZATIONS_FINAL.md
│   │   │   ├── BRUTAL_HONEST_ARCHITECTURAL_ANALYSIS.md
│   │   │   ├── COMPLETE_FIX_REPORT.md
│   │   │   ├── COMPLETE_OPTIMIZATION_SUMMARY.md
│   │   │   ├── COMPLETE_OPTIMIZATION_SUMMARY_FINAL.md
│   │   │   ├── COMPLETE_VICTORY_REPORT.md
│   │   │   ├── COMPREHENSIVE_BENCHMARK_ANALYSIS.md
│   │   │   ├── COMPREHENSIVE_BENCHMARK_REPORT.md
│   │   │   ├── COMPREHENSIVE_OPTIMIZATION_AUDIT.md
│   │   │   ├── COMPREHENSIVE_OPTIMIZATION_SUMMARY.md
│   │   │   ├── COMPREHENSIVE_XXHASH_OPTIMIZATION_COMPLETE.md
│   │   │   ├── DASHMAP_IMPLEMENTATION_COMPLETE.md
│   │   │   ├── DEEP_DIVE_CRITICAL_ISSUES.md
│   │   │   ├── DEEP_DIVE_OPTIMIZATION_ANALYSIS.md
│   │   │   ├── FINAL_BENCHMARK_ANALYSIS_AND_BOTTLENECKS.md
│   │   │   ├── FINAL_BENCHMARK_RESULTS_LATEST.md
│   │   │   ├── FINAL_COMPREHENSIVE_ANALYSIS.md
│   │   │   ├── FINAL_IMPLEMENTATION_REPORT.md
│   │   │   ├── FINAL_OPTIMIZATION_REPORT_COMPLETE.md
│   │   │   ├── FINAL_VICTORY_REPORT.md
│   │   │   ├── FUNDAMENTAL_CORE_LEVEL_ANALYSIS.md
│   │   │   ├── FUNDAMENTAL_OPTIMIZATIONS_RESULTS.md
│   │   │   ├── GLOBAL_CACHE_FINAL_RESULTS.md
│   │   │   ├── IMPLEMENTATION_COMPLETE.md
│   │   │   ├── JAGR2_COMPLETE_IMPLEMENTATION.md
│   │   │   ├── JAGR2_FINAL_RESULTS.md
│   │   │   ├── JAGR2_IMPLEMENTATION_STATUS.md
│   │   │   ├── JAGR2_PHASE1_COMPLETE.md
│   │   │   ├── JAGR3_ALL_OPTIMIZATIONS_COMPLETE.md
│   │   │   ├── JAGR3_COMPLETE_FINAL_REPORT.md
│   │   │   ├── JAGR_IMPLEMENTATION_SUMMARY.md
│   │   │   ├── JHOL_BEATS_BUN_FINAL_VICTORY_REPORT.md
│   │   │   ├── JHOL_BINARY_REBUILT_FINAL_RESULTS.md
│   │   │   ├── JHOL_OPTIMIZATION_JOURNEY_SUMMARY.md
│   │   │   ├── JHOL_TRUE_PERFORMANCE_REVEALED.md
│   │   │   ├── JHOL_ULTIMATE_OPTIMIZATION_COMPLETE.md
│   │   │   ├── OPTIMIZATIONS_IMPLEMENTED.md
│   │   │   ├── OPTIMIZATION_COMPLETE_FINAL_STATUS.md
│   │   │   ├── OPTIMIZATION_FINAL_REPORT.md
│   │   │   ├── PERFORMANCE_ANALYSIS_REPORT.md
│   │   │   ├── PERFORMANCE_DEEP_DIVE.md
│   │   │   ├── PHASE_1_4_COMPLETION_REPORT.md
│   │   │   ├── SLOW_INTERNET_ANALYSIS_AND_FIXES.md
│   │   │   ├── ULTIMATE_VICTORY_REPORT.md
│   │   │   ├── WHAT_WE_DISCOVERED_AND_WHATS_NEXT.md
│   │   │   ├── XXHASH_OPTIMIZATION_100_PERCENT_COMPLETE.md
│   │   │   ├── benchmark-results-all.md
│   │   │   ├── benchmark-results-medium.md
│   │   │   ├── benchmark-results-step0.md
│   │   │   └── benchmark-results.md
│   │   ├── 2026-benchmarks/
│   │   ├── REORG_MANIFEST.json
│   │   ├── junk/
│   │   │   ├── 0.5
│   │   │   └── =
│   │   ├── misc/
│   │   │   ├── QWEN.md
│   │   │   ├── README-highdata.md
│   │   │   └── slow-to-fast-documentary.md
│   │   └── reports/
│   │       ├── 2025_RESEARCH_OPTIMIZATION_PLAN.md
│   │       ├── 4_WEEK_OPTIMIZATION_FINAL_RESULTS.md
│   │       ├── ALL_ISSUES_FIXED_FINAL.md
│   │       ├── BENCHMARK_VALIDATION_REPORT.md
│   │       ├── BEST_IN_CLASS_OPTIMIZATIONS_FINAL.md
│   │       ├── BRUTAL_HONEST_ARCHITECTURAL_ANALYSIS.md
│   │       ├── COMPLETE_FIX_REPORT.md
│   │       ├── COMPLETE_OPTIMIZATION_MASTER_PLAN.md
│   │       ├── COMPLETE_OPTIMIZATION_SUMMARY.md
│   │       ├── COMPLETE_OPTIMIZATION_SUMMARY_FINAL.md
│   │       ├── COMPLETE_VICTORY_REPORT.md
│   │       ├── COMPREHENSIVE_BENCHMARK_ANALYSIS.md
│   │       ├── COMPREHENSIVE_BENCHMARK_REPORT.md
│   │       ├── COMPREHENSIVE_OPTIMIZATION_AUDIT.md
│   │       ├── COMPREHENSIVE_OPTIMIZATION_SUMMARY.md
│   │       ├── COMPREHENSIVE_XXHASH_OPTIMIZATION_COMPLETE.md
│   │       ├── DASHMAP_IMPLEMENTATION_COMPLETE.md
│   │       ├── DEEP_DIVE_CRITICAL_ISSUES.md
│   │       ├── DEEP_DIVE_OPTIMIZATION_ANALYSIS.md
│   │       ├── FINAL_BENCHMARK_ANALYSIS_AND_BOTTLENECKS.md
│   │       ├── FINAL_BENCHMARK_RESULTS_LATEST.md
│   │       ├── FINAL_COMPREHENSIVE_ANALYSIS.md
│   │       ├── FINAL_IMPLEMENTATION_REPORT.md
│   │       ├── FINAL_OPTIMIZATION_REPORT_COMPLETE.md
│   │       ├── FINAL_VICTORY_REPORT.md
│   │       ├── FUNDAMENTAL_CORE_LEVEL_ANALYSIS.md
│   │       ├── FUNDAMENTAL_OPTIMIZATIONS_RESULTS.md
│   │       ├── GLOBAL_CACHE_FINAL_RESULTS.md
│   │       ├── IMPLEMENTATION_COMPLETE.md
│   │       ├── JAGR2_COMPLETE_IMPLEMENTATION.md
│   │       ├── JAGR2_FINAL_RESULTS.md
│   │       ├── JAGR2_IMPLEMENTATION_STATUS.md
│   │       ├── JAGR2_PHASE1_COMPLETE.md
│   │       ├── JAGR2_STRATEGIC_ROADMAP.md
│   │       ├── JAGR3_ALL_OPTIMIZATIONS_COMPLETE.md
│   │       ├── JAGR3_COMPLETE_FINAL_REPORT.md
│   │       ├── JAGR3_NEXT_GEN_PROPOSAL.md
│   │       ├── JAGR3_PURE_MATH_IMPROVEMENTS.md
│   │       ├── JAGR_IMPLEMENTATION_SUMMARY.md
│   │       ├── JAGR_INFINITE_PLAN.md
│   │       ├── JHOL_BEATS_BUN_FINAL_VICTORY_REPORT.md
│   │       ├── JHOL_BINARY_REBUILT_FINAL_RESULTS.md
│   │       ├── JHOL_OPTIMIZATION_JOURNEY_SUMMARY.md
│   │       ├── JHOL_TRUE_PERFORMANCE_REVEALED.md
│   │       ├── JHOL_ULTIMATE_OPTIMIZATION_COMPLETE.md
│   │       ├── KCC_KNOT_COHOMOLOGY_CLASSIFIER_PLAN.md
│   │       ├── OPTIMIZATIONS_IMPLEMENTED.md
│   │       ├── OPTIMIZATION_COMPLETE_FINAL_STATUS.md
│   │       ├── OPTIMIZATION_FINAL_REPORT.md
│   │       ├── PERFORMANCE_ANALYSIS_REPORT.md
│   │       ├── PERFORMANCE_DEEP_DIVE.md
│   │       ├── PHASE_1_4_COMPLETION_REPORT.md
│   │       ├── SLOW_INTERNET_ANALYSIS_AND_FIXES.md
│   │       ├── ULTIMATE_OPTIMIZATION_PLAN.md
│   │       ├── ULTIMATE_VICTORY_REPORT.md
│   │       ├── WHAT_WE_DISCOVERED_AND_WHATS_NEXT.md
│   │       ├── XXHASH_OPTIMIZATION_100_PERCENT_COMPLETE.md
│   │       ├── benchmark-results-all.md
│   │       ├── benchmark-results-medium.md
│   │       ├── benchmark-results-step0.md
│   │       ├── benchmark-results.md
│   │       └── implementation_plan.md
│   ├── design/
│   │   └── JHOL_CORE_INNOVATIONS.md
│   ├── engineering/
│   │   ├── BENCHMARK_IDEAS_APPLIED.md
│   │   ├── BOTTLENECKS_AND_PRODUCTION_READINESS.md
│   │   ├── BOTTLENECKS_JHOL_VS_BUN.md
│   │   ├── JAGR2_SEMANTICS.md
│   │   ├── JAGR_INFINITE_ARCHITECTURE.md
│   │   ├── LOCKFILE_DESIGN.md
│   │   ├── RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.md
│   │   ├── RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.pdf
│   │   ├── RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.tex
│   │   └── paper.html
│   ├── internal/
│   │   └── LOCAL_RECORDS.md
│   ├── paper-style.css
│   ├── phase4-optimization-plan.md
│   ├── plans/
│   │   ├── 2025_RESEARCH_OPTIMIZATION_PLAN.md
│   │   ├── COMPLETE_OPTIMIZATION_MASTER_PLAN.md
│   │   ├── JAGR2_STRATEGIC_ROADMAP.md
│   │   ├── JAGR3_NEXT_GEN_PROPOSAL.md
│   │   ├── JAGR3_PURE_MATH_IMPROVEMENTS.md
│   │   ├── JAGR_INFINITE_PLAN.md
│   │   ├── KCC_KNOT_COHOMOLOGY_CLASSIFIER_PLAN.md
│   │   ├── ROADMAP.md
│   │   ├── SMCC_DESIGN.md
│   │   ├── ULTIMATE_OPTIMIZATION_PLAN.md
│   │   ├── e145-instance-model.md
│   │   └── implementation_plan.md
│   ├── research/
│   │   ├── JAGR_INFINITE_RESEARCH_PAPER.md
│   │   └── JAGR_INFINITE_RESEARCH_PAPER.pdf
│   ├── superpowers/
│   │   ├── plans/
│   │   │   ├── 2026-05-28-phase-5-completion.md
│   │   │   ├── 2026-07-08-jhol-runtime-event-loop-and-watcher.md
│   │   │   ├── 2026-07-08-jhol-runtime-lockfree-state-and-cache.md
│   │   │   ├── 2026-07-08-jhol-runtime-sparse-tropical-graph.md
│   │   │   ├── 2026-07-08-jhol-runtime-transpile-native-io-deps.md
│   │   │   ├── 2026-07-10-jhol-runtime-perf.md
│   │   │   ├── 2026-07-10-js-engine.md
│   │   │   ├── 2026-07-14-complete-js-engine.md
│   │   │   ├── 2026-07-14-jagr-phase1-certified-constraints.md
│   │   │   ├── 2026-07-14-jagr-phase2-parallel-domain-compiler.md
│   │   │   ├── 2026-07-14-jagr-quality-preserving-performance.md
│   │   │   ├── 2026-07-14-project-fingerprint-lockfile-reuse.md
│   │   │   ├── 2026-07-24-atomic-wavefront-closure.md
│   │   │   └── 2026-09-13-jagr-infinite-law-layer.md
│   │   └── specs/
│   │       ├── 2026-07-06-jhol-bundle-cas-first-design.md
│   │       ├── 2026-07-08-jhol-runtime-performance-design.md
│   │       ├── 2026-07-14-jagr-quality-preserving-performance-design.md
│   │       ├── 2026-07-14-project-fingerprint-lockfile-reuse-design.md
│   │       ├── 2026-07-16-jagr-native-proof-engine-design.md
│   │       └── 2026-07-24-atomic-wavefront-closure-design.md
│   ├── user/
│   │   ├── SCOPE_AND_STABILITY.md
│   │   ├── for-windows.md
│   │   └── main.md
│   ├── v2-optimization-plan.md
│   └── warm-optimization-plan.md
├── experiment/
│   └── 2026-07-16-resolver-bakeoff/
│       ├── README.md
│       ├── backups/
│       │   └── jagr-pre-knot-l1/
│       │       ├── betti_numbers.rs
│       │       ├── conflict_explainer.rs
│       │       ├── core_guided_lp.rs
│       │       ├── core_guided_solver.rs
│       │       ├── jagr_resolver.rs
│       │       ├── maxsat_solver.rs
│       │       ├── mayer_vietoris.rs
│       │       ├── mod.rs
│       │       ├── ol3p_solver.rs
│       │       ├── resolution_differential.rs
│       │       ├── simplicial_normal_form.rs
│       │       ├── tropical_math.rs
│       │       ├── tropical_max_atom.rs
│       │       ├── tropical_nullstellensatz.rs
│       │       ├── tropical_polyhedron.rs
│       │       ├── tropical_sheaf.rs
│       │       └── tropical_sheaf_analyzer.rs
│       ├── fixtures/
│       │   ├── express-app/
│       │   │   └── package.json
│       │   ├── overrides-app/
│       │   │   └── package.json
│       │   └── peer-deps-app/
│       │       └── package.json
│       ├── harness/
│       │   ├── run_bakeoff.sh
│       │   └── topo_vs_pulse_microbench.py
│       ├── plans/
│       │   ├── jagr-knot-ol3p-replacement-design.md
│       │   ├── knot-v4-million-user-scale.md
│       │   ├── pulse-engine-design.md
│       │   ├── residual-certified-ol3p-design.md
│       │   └── yours-dual-geometry-design.md
│       ├── residual_ol3p/
│       │   ├── Cargo.lock
│       │   ├── Cargo.toml
│       │   └── src/
│       │       └── main.rs
│       └── work/
│           ├── domino-cold/
│           │   ├── cold2.err
│           │   ├── cold2.out
│           │   ├── hit.err
│           │   ├── hit.out
│           │   ├── home-cold1/
│           │   ├── home-cold2/
│           │   └── proj/
│           │       └── package.json
│           ├── domino-fair/
│           │   ├── bun.err
│           │   ├── bun.out
│           │   ├── coldA.err
│           │   ├── coldA.out
│           │   ├── coldB.err
│           │   ├── coldB.out
│           │   ├── hitA.err
│           │   ├── hitA.out
│           │   ├── homeA/
│           │   ├── homeB/
│           │   └── proj/
│           │       ├── bun.lock
│           │       └── package.json
│           └── domino-verbose/
│               ├── err
│               ├── home/
│               ├── out
│               └── proj/
│                   └── package.json
├── for-windows.md
├── graft/
├── graphify-out/
│   ├── GRAPH_REPORT.md
│   ├── cache/
│   │   └── ast/
│   │       ├── 002d98cce57c8ba4c35635055b5ffdcf3679d926b74963c139d91e15a02daad6.json
│   │       ├── 0071be666e715b549350529ee5ab608269ea6615fd3de5c49125432990e9fa82.json
│   │       ├── 00adbf24165c4b8d8e6aba20d4cfc12ae270a362215075c8f59d1ef634b5cc27.json
│   │       ├── 00cefab3bef8bece1fed20abb3c9b97749476e0eedf567230c1f8e754360c36d.json
│   │       ├── 00d98984852abd227aa08a9781f9689fc2af397d7d2492d9097fd806b1efd2b0.json
│   │       ├── 0181b1247f14db97a1afe6bbdd5cede301b828e7027ef1538da593b4909aa080.json
│   │       ├── 020919ec8792d42660d7f8dcedb1050c5e342f6644e25fde3873c03357736734.json
│   │       ├── 024b8b0a7203edb402c35b7130ed2f4e17d448acd95668492e5b6c7fb195a768.json
│   │       ├── 027472d481f79f447a1449baf09adfea4605345c86c472d544961de45d101446.json
│   │       ├── 02f7fe764c6e5b4d8b25c1ec116fa3588c89cd3055e2f5022cde01ef62308109.json
│   │       ├── 036e75ecbcbb3e353d281d8edbd915aefe57a205f2e945ecc1b06de31ea0a96a.json
│   │       ├── 039150152da420df061a364728754e93c415a2b2576ab00e4ece030783e7c94f.json
│   │       ├── 03a817bb45cf4bbcbf4cc8a4047eb9155a45a509b2f5f9ea8d6f9628081aec82.json
│   │       ├── 03d0fda0a3ef4a3f4ef9968a6ad853e2e1ca56944ef9ae7b05bc72adbabc555d.json
│   │       ├── 04620805e51c24b8e2698bb08bfc987e660eece5c0233839305273ad01020167.json
│   │       ├── 0486f50a9cea6a2251a8978fd03a20dc3cdeea4cf40b08717c8db16f0ea5d63a.json
│   │       ├── 04f63adbc3579acd79c0900a838a736d5b5695c6006604ec51fe1088ae0c394c.json
│   │       ├── 05c0930d0f3823d15eb65214542b64fe552cd0b397b6df89823011fcd1631ced.json
│   │       ├── 061353a6ae5f9542d251089f56b3c2fdd70e09d4993493665c5f293652488538.json
│   │       ├── 0660e40e09e2e4d09bb14982f81bb4f82e5f04a16c617051bb11fa8fa5ddf8a9.json
│   │       ├── 068feed12acf691e6f54422788e5157e399074e41c438c27d18f172730151ad4.json
│   │       ├── 0724ec8062b1b8086011ecdd537c016b14f59a489d6b0bdc932c6a39775d98a9.json
│   │       ├── 07ae1b1ef006173e672118d6ad1efc232071107b6cf2b0d17c0ed58aafa689b3.json
│   │       ├── 08486bfe8d877cfb119798ada171b976cf39402b2a2bd4bae1ab384a4f391813.json
│   │       ├── 088868602371a7af5463ab787fd5bdbaeca41ffec7e76563f47ffafef58395ec.json
│   │       ├── 08b4e147b0d5247e89960968184997a979c5498aba1527292b8c50d18a474ad8.json
│   │       ├── 097dcee10bee262c5f779fb342c75933402b20bf01399ecc75f8d9c8e5abb084.json
│   │       ├── 099060752c5e845989ca251e5d10b2bf0c15981a3cdeba7ef7598e8766e953df.json
│   │       ├── 09beeb8b078f9718c12a903e409e424ff7e5bd7d2b64a476f644fe9dbc1eb12c.json
│   │       ├── 09f766b4575f390ca767fec5137f66a61f42c88784eb4c7d189551335e45d608.json
│   │       ├── 0a1ca272abd51603e4d658303a41a257fd59e1a9a8d459a53e040985f6137f46.json
│   │       ├── 0a800f7ce079f0c60007c8bf6f02c37aeeb3c393ec18bd44a8850e9a0206094f.json
│   │       ├── 0abdcf86c72fbdc93395ea82a88712c0d1b68175ea4411e0c1872e93b85d5e17.json
│   │       ├── 0adfa5cb658e08ca954d3ccafffd93e187070dcdba9784863dbdaa6e51d3ff10.json
│   │       ├── 0b22c8651249e862239e519e9226b5b34cd7d8f4ea085046135e38e06639db77.json
│   │       ├── 0c00ef9c42c8af4b43cb7fa5900a8cf4dc7fddedd4ee84ef433828c554546ac3.json
│   │       ├── 0c421c94164d230b61c58e756129786427a81ec063919b34c68e9d285fb1a35a.json
│   │       ├── 0d37aad044a247b9f2f54171c56fd8a44af37a2d1f0016b1ea08f755de59fba2.json
│   │       ├── 0d99e5c369c953de71ac3124cb9e33e03676f77c054ca9ac82f45d6e1c8a7c2e.json
│   │       ├── 0f65c2eb3909bda58ab3bea947af2bc78fa574deb306cef853d017a304b76db1.json
│   │       ├── 0f6b112ee00b530918ba49e06a76aea72371bb96bab37b1964b12e167878d51e.json
│   │       ├── 0ff64381d1bd98105c5c5d941081d963bfd97be9720b89a0ee029fee1f8d078e.json
│   │       ├── 11be837f906381196d1d0316cffd8eaeb0a1b2b62039fa92f6fc1acec392ce0f.json
│   │       ├── 1225a1a9a05a54dbf757cd7c8ab783731a097523818a24685592eff297c3beda.json
│   │       ├── 123a505549da1d3354c12c6ac556bb7e33fa596cc9a1c92cfae502c2b1d75257.json
│   │       ├── 1300936e56cc2cbab5b685e6f48efb4e350dd017d34ae9a95f1b30496d61b867.json
│   │       ├── 13ad841f0c6296f5f14501d23561186fce08a389806207c2b74e19b20a0f5122.json
│   │       ├── 13ec3fa3cf9dc155281178e9f8e8b55c543c8e7dbadaffa8bd5ca8cfbf3defe9.json
│   │       ├── 141471f0e6438df88bfdd95b62d33d8586adc82dce854ee9bb8616ebb0f963fd.json
│   │       ├── 146ea61f6715d5755ea6f474e33c1c8789e4d00eac21b91961ef1b451b67cfaf.json
│   │       ├── 14af8b2e6776186197d8d5e2d11d356accb46d10eaad69c6582604b65bebc8ae.json
│   │       ├── 15e3f1e5c79504a2de2d74794520a3902a555b533d7f704c8d3c0c5f88b26b12.json
│   │       ├── 163b1daafccd5e90495f27622fdfb14ace2e047fb1a1d489b6f836fb779e9d7f.json
│   │       ├── 16acf69795e22b5c2de16f29effee82a461fb43d40a89aaa1c90b7f289551511.json
│   │       ├── 16d42fef8181e66078bf41e9e9600106c29cafbeb1d3f8622df3978b24182180.json
│   │       ├── 17018d9fb1a712d425dccbefa2ad7a15d645afb7b45998796a38ef6ba2bf33e1.json
│   │       ├── 175dc4cbd38c4d16dd0368195bd21966a75132e4c4a33de4ed04eba40c1b3516.json
│   │       ├── 176245471c070e736c6fa4133edc68623a88220848a8e181b9a7428a2667ada7.json
│   │       ├── 17fe996d6cb337897223c2d75eb6d006eeff85caaac1faa434ad9d06fa78b5cb.json
│   │       ├── 187948c7e7e1303b04a4bc29834ec3dc565ecfca2e956a41fb18d80cf2b7ec0f.json
│   │       ├── 18e00afeb3d8d69c3382b08dc51e4a093d909f3d8c26db0ecb6f2118a3d911c8.json
│   │       ├── 18edb3d9cd80636ed56ee37fb611f5b73b9f24da8ead1bcc07fc69d7413b5c19.json
│   │       ├── 192a8f28a3a8173a9e83ae5f0237e5b7d0b5af91c21e6057ecec418169101304.json
│   │       ├── 1947180a2f4121ca1963d5cc6202ece3afda370eda92535b2874522472be4e5c.json
│   │       ├── 19c370597a01921086176f424775b527d4290a7100f4c8a662f8ec2381081e1b.json
│   │       ├── 19f6d03f1520abc4dd0205ff1445076a8918227489e6b8d3a14e220bcfc040db.json
│   │       ├── 1a04d1cdaa8b951e9be20e5a602b4f0586ad285f67dac8038078f57a5c138eb0.json
│   │       ├── 1a09ba414b565c4a32a712dce7b49b4d5406bc51c814fee065f963c71034ecf9.json
│   │       ├── 1a36df15136f166fa972179c4807d0688a121fd5e5c41652b15d723586119209.json
│   │       ├── 1a68892fc162f3d6009b0b2adb03d56868a1684e773d21d79078f150cf091c12.json
│   │       ├── 1aebdb2f9e3839d861fbafe64a43dddeba9e64695aa623900330b56c5208ed7f.json
│   │       ├── 1b94c740cf2146c8f278b09f2fe1e9e918f800af45654243dcea97b7ad0aec26.json
│   │       ├── 1bdcbefb4446ee727774685addee64ff96ca1dc2dd22bf2b98af2ca3ef31b6d4.json
│   │       ├── 1bddebbb97bdaed2488825d844034622fd793cebfd22664a6c6be1d668aba859.json
│   │       ├── 1c0676924e84448453cf5ad009f8b315b1a6cdce01524ff378ed5eda328baf1f.json
│   │       ├── 1c72f7802286eee2e16d9304008244da338be28621b5e9ef01ba4fd4c1fa2468.json
│   │       ├── 1c733bd477289f2ead798f42d40c79842fdb0279dc897e11bae77ea996e53136.json
│   │       ├── 1d624827764df30c12068ef4ee7df1cebf7829636783491faea83c28d4fb366d.json
│   │       ├── 1dc17935b92f474f14a45f7a996a01e0edae8dd7a055a8a42fc0c052e79db5a0.json
│   │       ├── 1e3a317474f869ae73c470e5a59736ca12d5db0e1de6fbcfe0556829ff43a5c1.json
│   │       ├── 1f8d54ad29b6604d255b30b5a4332d96c7d7035fa5567692f524e6bcde02dc2d.json
│   │       ├── 1fe2745355d1e6c555b33c3b3cb5d615c90798112b5b3ac944f02a107e4c3824.json
│   │       ├── 203bfca86789e1a1ae12e606efbac9e9f4efd76e5fb0febb6ad693e82d02a94c.json
│   │       ├── 204c753e5de4458e53c7f4b0ad7161df16b975f7d0d331b78b6f4ac875f6c2ee.json
│   │       ├── 206db818c462cb16abe1af31f253daa3223aa1dff7789474cd36d6c980073e20.json
│   │       ├── 20fa08001f321cfec7eab90191b319619181aa1b7a3d0b6e09f63ecb17a1b61c.json
│   │       ├── 2151174cf502b862ec75bd3e92e4611282b18f2c84975419991cbed981fed06b.json
│   │       ├── 21e9dae58178ddf437d889496ca3b18ade57ba0a653ccf1e6e345d91163a6261.json
│   │       ├── 22c938a5ea21e5dca05657a332d52d2fd8bc4bbf809505d9c353c2cac4232741.json
│   │       ├── 22ce593153a323cc4943b4c4ab8c2edb9cc5579b14d05aa28d89ecd57acb384f.json
│   │       ├── 231193c7dee516e5b78699361f81e4d4e8b6a2b8afd9d61c283849f878c43b61.json
│   │       ├── 236294809920c4339120196c644cc2d51b70f4e69fbf2f41f16cf5b037b2a2fb.json
│   │       ├── 23f497292264d0d6a3a7a6573b78068bf1c5aa5d4e69c392d41a18454ea27432.json
│   │       ├── 244ec53cdc786a12301b40c478d6f535dbae4d8a455016e11c803226a84862c3.json
│   │       ├── 247a6168b8451515a9aeaba9c0d615557651f2351779d0ac57c3956581d686d7.json
│   │       ├── 24fbce23da668ac58b6480da2a0656ae6950c23ca603b18c254caccd41dc93ff.json
│   │       ├── 254a4da9d087f8fcb89e7df3fe6e08909980c9c8dbc9f247c740381cf10c6424.json
│   │       ├── 259ed7a80872a81d9619dc1c7feb9292f2998ede6f84b1384281151b4c81223c.json
│   │       ├── 27f6c975a324be6029a5f596be05159c517ef8d21953cd3df6a4d9a46f7c6452.json
│   │       ├── 280b6b0e136cdd0fad6c8fa11ce22dc4804062000f6d73d414572f8f0dcb72ac.json
│   │       ├── 2892a33873b7e8cf066073d86979c1cb4c3fe0d5542ddf239cc1a61ca1bb72ef.json
│   │       ├── 28ec1212240701419f02cde7c6eae9cc7d8027bbe05b98ca75fbab19faab04eb.json
│   │       ├── 294b765f45e94e455e48b6db02de26f1da6ac5ccfc96889376a21731cf2ca3d8.json
│   │       ├── 2987c38526d14cfb8fe91ad017be6a182ae145153988e83a397d9535c3d81a14.json
│   │       ├── 2a00f93e313df567ec63499d44c47598387ffabf220261daac3acba55424d75c.json
│   │       ├── 2a42d38931214589704b7bc1c9d607b4ee1aed8c4c533b82b27e6a96560a98f4.json
│   │       ├── 2a681620ad8ca17300677dd646425144e5d7376d37a583bc85198ff5083851aa.json
│   │       ├── 2a7d5045f3f809222c3314bbcf09e4005f5cb52196ea7770d7fedaa92d2d7a11.json
│   │       ├── 2bcbab205f8a4bddf174dac33ccc9096cd57c3574202a00e8dd249ef04bca238.json
│   │       ├── 2cb25bc019812c3e14781ea1dbe9ef9025e2771e82c0bab8150f7fc04c91d9ac.json
│   │       ├── 2ded5a7b9c3813dcb15cb255282c8b169486d079bbffed18ef84b75d64b0f454.json
│   │       ├── 2e2b0ec10dc7153f6f9c8218d527af93009f595a9bc8857d3652634db7a12c29.json
│   │       ├── 2eebbb8564fd16e1201979a1dc218172bfd144ca92897a8e4157741b7923a5ef.json
│   │       ├── 2f0a0771415ebc0cac3b3776ae03b630653c33b47474f2f369d4c3c322d45c8c.json
│   │       ├── 2f56d28e0ae8fa14dada0241e7ebf91f6f4298c6006baf640ad924679a89f8f1.json
│   │       ├── 2fbc41e4a75bd995b1434e0eb79cb64cc1c2cfc697b534c095b0e51f0f443725.json
│   │       ├── 30f4d35058ba83b05c6660495b86aeb486aa985540830f327e772b51c781f3c0.json
│   │       ├── 315e80704037541daffbca723806d5248604200bead4c014c4fdabbd4c08f45e.json
│   │       ├── 31c90b0ab9ff9d93f110ffbc842d6e1775e9c7ff804d414b1ac6c5ad0d314cc3.json
│   │       ├── 31fd143c679a3c837ab033601ce036d83282bf6ed76e615d91bff6a8ab4e67c9.json
│   │       ├── 325e5549c7ea043f73b686d01e84cbf1bde3b9e45f7e82d33181872548337e7c.json
│   │       ├── 328b5255efecd550c670f88c7db4ac63b90f67980095c00429390c44a5425fcd.json
│   │       ├── 32d8db3c86c79011312becdf16fa3b6684c12d116b792cbb7622f2d308ec172b.json
│   │       ├── 333d507e2ad3b638a1da6ad699e58a55a6f96573c8db6d0d03e6532b3db93d75.json
│   │       ├── 3371afd35b8e3f70840c1e8f27f729868d69944b38a808ad9834bcd4992bb3da.json
│   │       ├── 33ef39e2d1ede231846698cc24db8b6281dae5ab435eb13830e4bcc6c6b4a94b.json
│   │       ├── 34e22378dbd8c215b57a79e6a1961b11f7d7d093d8b6591d7a5091f252760251.json
│   │       ├── 350d53bcee7580ff80fc8152dc7d4dd1dfb9f03c793a0d30191fd5d21c4f3ab8.json
│   │       ├── 355ff5c4d658d405d996b72f999a4ac9dad60af5dcb6067cd57e369aefb04d0f.json
│   │       ├── 3575d5485fd5638e164aab7e59bcbfe5fb12c128270a614a55504690dbe4a554.json
│   │       ├── 35e65085002e176c0b88fd7837c9f1bbff809817862e58956e8b2eaefcfafe30.json
│   │       ├── 3633604e141156f80682553d098fcecfca0b75f58464527f08d6c104773d9321.json
│   │       ├── 3637f376aff6f1fe309e8858574917756cd9d7893789d1e821c372df06b878dc.json
│   │       ├── 36f5fc3a8d491d8356739e992cbb62f221945dfb8b172ac3de8e6f62aba362fa.json
│   │       ├── 3764d83f3fac9bc7382e6e34ea1b19a4bd4ebc8251e2f0dc175a86f2d748918a.json
│   │       ├── 37ce98c4e2edb86c3efe2ee5066c70d3c4290a05b11802752c4085ad52693ac9.json
│   │       ├── 3835c7d9b95b5ce949585826d308ffbda8733bc4aff361e8df4c312912a03aba.json
│   │       ├── 3837f6bdbb3da209427962264f64e4a694ad9994b8ccc46af404a14eaf426d3c.json
│   │       ├── 389ed20c02d6812561b89eb0dca0e8e50cc96fd97dc1742dc06a399930183c6b.json
│   │       ├── 3a3d60a92fb0ba46c5af827c31d3c22b51538cd02e659767fcad50e7a719741c.json
│   │       ├── 3b109a182a136513559e7c2b9abfb80b6c4031889433e69a93a97c2c3c707fc6.json
│   │       ├── 3b712f4ce43025f36c25fe416ea029f08e669a657659966f83a1b39d806fe914.json
│   │       ├── 3bafa366898e45febeca145d25abbedab36516d1882a633b5daf9061734d37e9.json
│   │       ├── 3bbf8f5ebb04c4caf0da4f0ade94ae3b70d0846bb1631f21fc0a0889522c83f2.json
│   │       ├── 3c061377dce9ce5618afdb19d8a595b2f434c0c6c07d5029e94f91ab4326301c.json
│   │       ├── 3c1de1be0c55472e1af307c73e46259f48ab7551bc8b5e75f62fd889b9ff7d98.json
│   │       ├── 3c4ed1ae92391c4a8e2e947f9ab1dc650031af2760d78a35c7e21a8a73ef0ad6.json
│   │       ├── 3df7d25a43f41fed1a4f7db1c4d34da054f1593f816dbd9b2a0ca794b46cf03f.json
│   │       ├── 3e6bf58a7202f44ea5e3ddd31ba747596779338d5bd2413d4c4e4fe27652abfc.json
│   │       ├── 3fed09e91b9daac75fbb3da5b487521aeab00b956d2c584c5383cc892cb62fa0.json
│   │       ├── 40482562078c56dcb97d1602645804f4515f9c605200e1c9263dfee18d63fbd3.json
│   │       ├── 405eb1da10b3a8909ba88ce5c3014da60be917110ca20c4ebff7794a95337a3e.json
│   │       ├── 40c2c5f75a21bc41258443c0cf51bee9a5b01793bc9b9121a85868b2e15410e6.json
│   │       ├── 40c8c718d756609b0e7f990cff8f333203e9ba0b460060d8739299cd508b5577.json
│   │       ├── 40dfda744c8bc27fff7b304e1152a96d7d32202fe5d0de48fedf5bc9cddc6206.json
│   │       ├── 41a62d587f7ca1ba47d50d1d6c6721ffeaa5c42aea77884e2675f123843b965b.json
│   │       ├── 41fa08bdbd846b54e8839866445b20fd4dd0bffa50674fdb63df73d60997d1c1.json
│   │       ├── 42d73d76ea664ac132d6d4bd83fe0920cbb6001a79c90b2f7be888284d9e3ae4.json
│   │       ├── 437d83524441d318b4aae7ec808e4abc483a69208747e28538af65550f230318.json
│   │       ├── 440e3654c59cf2ce217e62e0829d4f9df5ae185e8efeb6e70a01abffd16f079a.json
│   │       ├── 44c22a3c4b0446bca683ba5f4c70600f8b2100d2b5e6904c39b8f8912b381ac6.json
│   │       ├── 44d08224a8245b11a859ed14ad060f496aaf32096d3f0d1c52e743c2a0cf744e.json
│   │       ├── 456909e5da0e28aa08b5adaccefbc184c22d5f28276183806a02e00b7e0b2f2d.json
│   │       ├── 457cbf85eb7fa0cf1f4263644d0a6469f0ddc71e7f3b2edf27d690a94326e3aa.json
│   │       ├── 45a6d3f0474bd617c6757eab84d62d58513a68a59bc27d3c11f7b007522c1903.json
│   │       ├── 45b51fe1c27cefee4e87f7ed15fa6f32c911123bb2537b0c175a2d8536885179.json
│   │       ├── 460d72b39327cca4aa400c751accaafd0e398b829e4083e7efe3d8b8b2ab816c.json
│   │       ├── 4681c9d00b23a4aa535ccc4590b613da1fbf125063656314d1a38e66e14e7bbd.json
│   │       ├── 46d43617057bb9b8802c83860b28080de19d15ee2062300221fbc22c259aab82.json
│   │       ├── 47345e7e9e431ef958c9404233c1ce3a0b4cbc4acd9b184c083b7cf8cfd4c9ec.json
│   │       ├── 478092d409ddb6de59ad80e3aa20e0d0496cf1e323199eba9a3c41144ef520e1.json
│   │       ├── 47cc955bd78f900749cd3846eac81b69d93fe20d577ff9c28df779315c042fc6.json
│   │       ├── 48525427d4e572ac28ac952ddaa6b1ec06d0ece91414980d11d9e4b76834f85d.json
│   │       ├── 489da32a33c252941bd0368fcb60d57c29abb33dab153cf705dfa6e893be4481.json
│   │       ├── 48d2b7f3809f3ef526aa82a8716677b33b48d2c8d3986738cd75bba0aea72716.json
│   │       ├── 4a6193733b51261024f0cb59921e89be372be57ce6ea6607e1ca0232bf80eea1.json
│   │       ├── 4a93a841acc1fd387a50aa32f6efbf91d1d10778b78acdfc4ccd5664d1ddb5f7.json
│   │       ├── 4aff61edc9163be2c61b53f36ba7493be0faf82f239e3d96bf185f6eff296d4c.json
│   │       ├── 4b3368f114e65760dfacef974e3bc769a8deaf1afbb4c02d99b10e25378e5dec.json
│   │       ├── 4ba6416b864bd0e553032e2741996151a85cce7c719dc508b0d73554604573bc.json
│   │       ├── 4ba69f86f427d8500e9508a81bfd3f28d69b004b521ac3bc87a8780ca6bbc211.json
│   │       ├── 4ce140593c1c45b7edc43575288e7f38c0aa1855e1b6d1b049a5d60644558726.json
│   │       ├── 4ce61263a64de11a6c47a4de71bc9b5f222738a9ce6b9a803d55b3c3f8b179a3.json
│   │       ├── 4d3354ccce826864e06f8b04294aa5690ca00988f580dd37b739da6a1a649c43.json
│   │       ├── 4d50706c3f7298a58b23ddf592a3e5b54cec0a941321dd0965b3428048f5c1a6.json
│   │       ├── 4e9b4606b26062346f2b72d06580909a06207008331e3d30486c127d43b8abe6.json
│   │       ├── 4fb7681b2c90d40ae90d84cf2e4d7c4894c37a528ff737cc4376a1bd44509ea9.json
│   │       ├── 4ff0922f096249d317559a832a1caace80fd18c9a339318cfc0ee850112719d1.json
│   │       ├── 50e27a6a7b68085ce44fca1f77d6bddec5b9f7798d7ea69584bcc1fe91ed39ef.json
│   │       ├── 513ccdf2938eb4d91382acf5f7859a18f39332ca43f63705176abe4867fe8ee0.json
│   │       ├── 51f3579a4c07ba8951a107d6beb0b733e18e76556803020e55e01f7944f09404.json
│   │       ├── 51fa24915b029ad35d0025b124b20b52e6eae1b7dcd7714136cd7ddb7f23f5b5.json
│   │       ├── 522686defdad4e86d4ef06140052931c58b247a49f5b612a7d8953c5311cad91.json
│   │       ├── 52441b003f370935eddc615babbf243fc83ea977a9d3b2744117cb6eed016456.json
│   │       ├── 52701547f6f1a955218329b7ea857d936c17a81cf3af3c3c0e44a26989ef0cc7.json
│   │       ├── 52de2e079d5d5755c3dabad34495eee8620dce86b7d6c17bec3ad90e79cb99ed.json
│   │       ├── 53a30952bb79352130f525e03c51ce5ba905d517f2a01f1b40b17ad0231044b4.json
│   │       ├── 5418c04b8e1560441a43894d6ae4c58d3ec2a683ccb337cc6faa93959c509bec.json
│   │       ├── 54645df2e8aaebb4cdc9ff63bea6f26965b918ed55294ac680212e6ecab22750.json
│   │       ├── 547dc4a6e38918c2f60ab537845afd896dd4e75cf376788aa3dc59418d3d36fd.json
│   │       ├── 54a9bf59da95b3d064e73e08e79cc1b994d10ff5cd8bac8adfd9f803866e01c4.json
│   │       ├── 54df25fb99d0da53392af5a9953c075da0a3512dccb8adf0dc17886005c40515.json
│   │       ├── 55db97dc2787f94d95a5722de2cb7d31381667f9400a6d1faa5d201aa26a1f39.json
│   │       ├── 55f9bd9533819f38f637975f8c0f41daeb1b02cba2fda94d039e471c3cb6bdfe.json
│   │       ├── 5847479e4f46ade72f0d7cadfe7d1c69aa343b074ee5687e64466b38e060ce40.json
│   │       ├── 586b7e809a8f77d9184e6d1732954912f8e7b551cbe6c2bca72e63439563c0db.json
│   │       ├── 590d40679e668eb8d85fb1a66a2d68b6f61e8892a3763461ae8e0b86720bf3ff.json
│   │       ├── 59124539d71953c2d2b86ede3306f1fa5481ab7d74fe1e80b2d4edc4f1cc79e3.json
│   │       ├── 592b3d10b80a2099ef0e7c83116af12ef6b7f07a6f52a1b9d09d805e67ce37a8.json
│   │       ├── 59315fd6b0a778932141cac680b4291a22535dc172db0be2924e4a087d501031.json
│   │       ├── 593a3358ca630cc49539cdbbecf107adf6a78d7425a305ad8a9c56fa8717aefe.json
│   │       ├── 595499c01907b36f3649668fbf18f563bbb848a7a8245662633eecba14f064e9.json
│   │       ├── 59d0882e21cbe6cff6fa3f14e49e212e2e7de7d9b89baf8020944cff0eb8a5a2.json
│   │       ├── 5a84fa29f662dbfe75079b284bc190ab21c33e5d20ca2c1f44fed1af227f2589.json
│   │       ├── 5adef3088a6b133dff2916509668c4ac3a7917c0a6c8e6b4bc8e55eda2dbea75.json
│   │       ├── 5b6b6205b057e0d686469f43969280d6fb00d78df74c1aa9547c7100514f9d48.json
│   │       ├── 5c38b50cc8ff39d3af2970b574174001e7f6e1a203da6b4fc745944c4ee2d72a.json
│   │       ├── 5cd755a5f1f88ea1e24a510b68ade6c1c8210034d9c2af13d40bbce94f0de21e.json
│   │       ├── 5d633fd95869fb7134df811b4af53d2c6fa4300118c4ded623585033abc25a98.json
│   │       ├── 5d85c6f55858eb20fa32a7ba2b3873ce5d12479c7a461fea9fbe172d61d9964b.json
│   │       ├── 5da32c21f4c9cf0da0816cf3803b762497c916e900af1130ed5e9a96bb89a7b3.json
│   │       ├── 5daccceb54c01cd9de5ba1d6281b35f53688165d6b29d07102b78725d939322d.json
│   │       ├── 5df7374a5c38df4552f27b50888f354d169e69ee3a4f1a7082f0145040d106c2.json
│   │       ├── 5e2143938c95fb55929161e3ee2643f14a7cec03ab7c37abbda3e0c6c025fcde.json
│   │       ├── 5f576794fa7c4ae556abf27b375865c1f2992452e1a63dc3642618f0e3e08a02.json
│   │       ├── 5f9401792809dd89f5ff99b6ae79cc8923154b523293153325cf92eba5af7832.json
│   │       ├── 5f9471ab0da9908b330752fb708f06c9a236b0718dee82788aa515c27b323584.json
│   │       ├── 5fb9a8819d0ae9330736028b3c89315176fce668f996c937a9d8bfe3e7e7aeef.json
│   │       ├── 601204dc666df1a6df4ccc99cd4c2d2c8aff9e2d691b618d5d18847b5062a2d0.json
│   │       ├── 6023e238b356bfdae2edbf0ba3480b7943f9be6b3726816b773636a957ea2e05.json
│   │       ├── 60e596039b1383e50a9c462aea4a1b38a6b3102a81e8357cf5cf02d9c932486d.json
│   │       ├── 60eb28b67c286149da8ef04e36cd263a85f3d3aa20e3d4399f0917cecc2a07b2.json
│   │       ├── 6190267e3204aeb277b518f2ff932130e0cd19b900534adab7b92f6bb713c60e.json
│   │       ├── 61cf51c7a092108424bd1f32e795286e5df10ddaa490e390462571169f5e1b90.json
│   │       ├── 61d6533a10bb4c166e42ca170c6c7a6c1cb385a5b1d979ce8ad5d755f787fd2d.json
│   │       ├── 61e769eab29c414c1c23c771e0acf3457e999e5526fc65cc3dc3ffe0c57ae95a.json
│   │       ├── 627650ac701a6234ba8e2baaa70e76beacaa58d7e4ea15da39f1f23e4dce37c6.json
│   │       ├── 633d49e40e3a058fe6809c826f99ba0232c5131477934fca4244183c948bea3e.json
│   │       ├── 637800ec8891045ae1f11aec5ea04f7c8ca4dc032ea47ee4afe5345dee174b2c.json
│   │       ├── 638059876b1ae5c12e785f9c8ce778d40270a61013af0f71e2403ab103bbde54.json
│   │       ├── 63a0ff5ab661771c9452fcb16545c40b14ee7039131e1787eb7855b1eb6fdd98.json
│   │       ├── 6420270435bd8416c92e6d7273cb40b151400b555f803d83150749cbcec24a63.json
│   │       ├── 64756bd05870122648ba0e5ba29833bd6e19e5c77ad80f9f1d7f9260d139b51c.json
│   │       ├── 649897a3374742f695eb9360c024778492573f3eabcac1da2f53c90fc19660ce.json
│   │       ├── 64b2fb1a7a8a6c8ee90716e3634e322df560aca23ce0e2e97958e1cf8747aec5.json
│   │       ├── 6512f7c43e8bc49b51abcf96145c791e4a66216ebcc5a565f975e3b220b7ac70.json
│   │       ├── 652058967cfd436513de31eefc4f5a049b6bf9f630c1d57b2aeda93191acee17.json
│   │       ├── 65918141a5777d3c75387892672e29342d361077072e3921a16f0a119c7ec25e.json
│   │       ├── 6689fb9391f0c94a1845bd9d8dc4a3155fe51b88533a9a4582d8f240a615609b.json
│   │       ├── 66a2aa67a7186bf46939899cf4f21a54c7069855e76fc7ba048eed1a3377c01f.json
│   │       ├── 67e4713f5e87499191d7240e847b569c49cccac70e8472bc25a955fe955dfdb9.json
│   │       ├── 6849ed745b4e67fff97749079646eec4ae158ac7ea065d39251b53268361029a.json
│   │       ├── 6898485fc4213f67b9b281806acc9b75ebb8ca045c1a12b1c459d59f13210f12.json
│   │       ├── 68e5fb60052ebc4fd119314afad3534919c424d12cb2cb661b546f6d13ad5822.json
│   │       ├── 68fed97cfa1e1159802b72c056b2c431ae43f8b9a97dc67ed539bc36bfd5ae15.json
│   │       ├── 6a12762a4c13a9db1b1b9ba804d31022af96050752e437e557f22a60f3ede29c.json
│   │       ├── 6a17350f458829b88d4fd8e2a84054d7a606aba09724857a22a88e014bb36d08.json
│   │       ├── 6a378a932537a18bff7d645690d382740a5dff57d7a543eef540386445945470.json
│   │       ├── 6a71314c1e138959eeedd3a9cc743ba23412822442e5720e78efb259b14c7e3f.json
│   │       ├── 6b90f37d94d59ef57f0534b38dff08dc6c3099425671c5cb458fc3ba44c4ea8c.json
│   │       ├── 6c20e6d538cd973f7f590be84acf4e57c5d9b24f4131d3a2db7c0169e973944b.json
│   │       ├── 6c71e2af999db5a9d4f0444f16160080659d2179e11b62bf6c3219ad09e6df32.json
│   │       ├── 6cf5f6e39aa0a3d62d21813084eb528bec8109f6cf1944ffb39b4f62cedcf000.json
│   │       ├── 6d2a458d48766483a85ab0ad1fea350740b5e5d33da9725afd476a6ea5c119ff.json
│   │       ├── 6d3a36645bca67b38869a374e49f9ac2d0d766390571a1a624a26c10f4beb11d.json
│   │       ├── 6d71a2a2a8ee1c1c20a4b62c71b13710414f22ecf476723f6610f4f2bd748f7a.json
│   │       ├── 6d96dbf51d0c0977e577d15238c1fb78e7423e01108275362f216e359e53bc88.json
│   │       ├── 6de0207e1ce86aca730f16ee439632617b67b6f90347e5c697c7bb6426d6982f.json
│   │       ├── 6e0cab74dbcfaacc5ecca23d904db8113ae185aa4552b0acc79ca745f25b35af.json
│   │       ├── 6e2f0be1a27a6ee3ff9a98d2ce48961cf704319c644c28459564274e7872bc7d.json
│   │       ├── 6f343ef4b2f6ca3d1017cece6186f4fa6d264d207fb27849e813487cfe95375a.json
│   │       ├── 70d85ad7c328224100bf1df7fe04c5004f3b268934214711233323a470b25f51.json
│   │       ├── 70fa76a05d1fd7d2009bb6d76fc78a57b0e06c19728bd168ef02adaf0cb4e8c5.json
│   │       ├── 7169618e75030688e14518598f0e14698ba89b1772985a92f5fe7d015f11887f.json
│   │       ├── 716f1ec75df4928d7a72d4b5a1c515c4d0c79222db53541ba7c80448bd26b37a.json
│   │       ├── 71ced1c5bdbc3c1647666232a05ce85b0217b97ad1d2f61f8e437a2e8f595623.json
│   │       ├── 71ec7e0080bf9ca49a6a51a4ac102546cae3fce4edf508d04eb2e99a6fa58f3a.json
│   │       ├── 72175d8a3464ec37edfcd9a24bdecb037654ec4a4e3a2d95ccbcbb1deaa2a3a6.json
│   │       ├── 72cf5193565fe2ec3a04f00032319e09f59a8d774b8698da9aea45f4edde9ab8.json
│   │       ├── 72d22e2a88bdd558af6bcf6e91d26ebed347268226edb8829f21444670e5d89f.json
│   │       ├── 72dff183d3e93e6e43e0c6656b38899f895e53bd33328604d9feb4276a8a8114.json
│   │       ├── 7300a791c900b96536a3830b9ff46d230097593ed8329251b82d5d39a2621130.json
│   │       ├── 7320f2dae26cbbef521421f42aad8a66d42a69f2eb5cf4ebef8d56fd532b2521.json
│   │       ├── 73bdb02155ffd4484cd95852a985eb96274ba1a3ff8ddf145867a0a65c1919c4.json
│   │       ├── 7405b9f521a21b32b2e8cad612861f7894bd916cb71924203d5f5d21dbc6e8ca.json
│   │       ├── 74c20594f27bc5e4030781bd36feed2e70baabcc28dbdf1b3be3a2bb6b566784.json
│   │       ├── 754dfd899f10a2eb47d24d88d13f383fa3ef6a7da87904546e8ee8476cb0fd4b.json
│   │       ├── 7550a86735800f2bd09ca78a6d104512b7c03aab0b8246760bc5298e86b37f50.json
│   │       ├── 7564c96f258643aa894708fe54e863b7c15aa50d0d788cb253cdc904fdc25e98.json
│   │       ├── 7565119fb565604d56d0588fb9511d95d2e35454e0567e1567465e09d487bd1d.json
│   │       ├── 761672e17e60d085f8886c94127194468a8fbb6fc887ed9b1372d106b05e2669.json
│   │       ├── 766dae44c459b22fb6f689db978e26b93a9302d44f05961e4c58b5d0abd0aa4a.json
│   │       ├── 78876b70186d933e567ed6dd35364bf833346cfee1a17f35543daebf8fb113aa.json
│   │       ├── 79488b2200508988361ee4ca6140c5469ffcd903c572faf4dd2cefb45f4c9e3a.json
│   │       ├── 79db2f2bb6b2118c2db149fefa02daf1eb3e6732c118a3a82f205bd6b2d6e477.json
│   │       ├── 79df6b71209207025428eaaae2d9b8745d5f1bc43da012b388eb9e72daec7a0c.json
│   │       ├── 7a123e5db3d2d32573bcd206211204095832779a32286be27f451c567c3c6bec.json
│   │       ├── 7a1a2320e0e5619ad1fadcc78f5d57aece1b0b6102e67e2c4021cea12e67d1a8.json
│   │       ├── 7ac13b7f45a6dc5dafa6584ddaac33e64dba22a22bf3e438533727bbeea1157c.json
│   │       ├── 7acd5d124475976146f221b6f09afeb791bb7f62cba6a023949b0f475be863ac.json
│   │       ├── 7b37e00467318dff7a7cb90df8263edc73f54e9157fb1f188624d88ec1071c98.json
│   │       ├── 7b5e207d6d20484656014b590a828963c8849773fad85c4b7d66d71d6363a95c.json
│   │       ├── 7b7f5d8167b1acb5f8ece5a2e4e794462cb2212724d31a74121f4a258b925ca1.json
│   │       ├── 7c236cd854040165da6163a7d0e7addec4985b877a2e312f1f5f76a635a100dc.json
│   │       ├── 7ccc258085063c45c6c398d61feb9f09941de9045ca555ab6108bb9996aa409f.json
│   │       ├── 7d10b60158a26dbbd52efe97951a55c2f85490af0ea15aa7958666bc843c08ac.json
│   │       ├── 7d14de9218c55e1844e1a5503925394ca5a34257163015b23bd7b8df5f4bae1a.json
│   │       ├── 7d2ca24c88762d49d32ff1c6e04670ece8ce2b07f83e152d5c46e9e7f6857cc4.json
│   │       ├── 7d34d100e56b465188054532560766d6b4d6c6d9a876eab3fe84c8013d6a5f93.json
│   │       ├── 7d7d92628dedf3d02f027efaf4ea1a67cb83e3a05b79b43e1eb72b4d7a6ed7c0.json
│   │       ├── 7dd45b7238a150014a89c4e39330ef302bf3d370e6e43f242327705834eca631.json
│   │       ├── 7e889bab3df1bb1e467c08166ca2ee480bb29e5dd6a0b6a8dce8f25e99e17b92.json
│   │       ├── 7e8ccecc466541664fe6b5983db3e07fbc4704b08d8782d4dff560396c0fe918.json
│   │       ├── 7e9aa0a0ea24f8d1e4a7f34abeaff749fd5ded9bc5bdf5a2099dad6892fff83a.json
│   │       ├── 7ea95fbdcfd1c0d286f734ab1d26cc8e762ebcdee1f0610bb3f8c1bc059be723.json
│   │       ├── 7eb9e975e4ca1a63ec55efcf72c02061ce2450572ae49e6b40b26a593f4101c8.json
│   │       ├── 7efb7e95825126cd3ab92bb25978ac7bfbcf1bda2fff3a0cf4559c598b5d327e.json
│   │       ├── 7fadcd52c684f05f2235f94b5ec05a24f8b7d2e855710901c0ca9c6dc5aa8e8a.json
│   │       ├── 7fd46d28c17747ed3595e4f9cee1ba13647567bae6da9dde8a23276be67689d8.json
│   │       ├── 7fd7daff115b837d72db5a23bad44a276642207856c685ea0a024289f0a7469d.json
│   │       ├── 7fe2d6bd866631edda902fddde7828e0fa01b34363ca19b2f5f5a08fc9e5b508.json
│   │       ├── 7ff07810d3bb5b874739d423bcb89e554dd756d91e92278116def68779ad4b4a.json
│   │       ├── 800b6647a696ab6adba0c088c7c142058ce7db873d6d4ce5eaad5bb203e5f6c4.json
│   │       ├── 8094022344b4f0e8ab5becadc50ad7fa11e8adef87a2ae4d4333cea2f02c7024.json
│   │       ├── 80fe4b90c9048fd9e5867cced4d17c72b205eff1596d213ae1df2005903919d0.json
│   │       ├── 8152aee747aa0756511626b9eb757a9a04796c7bf178ba713cd5ea989ab56950.json
│   │       ├── 8230c7de780e59f3bd5e9e2cacee8d105fa76fd9eeb5e0f011ab957a590af854.json
│   │       ├── 824bd7bfbbd2cde57880eed7bc7021c2312c069f546c414b789f027cb89599b9.json
│   │       ├── 82be99b989e4def4e5e92012e63b2a298e5b1c1bde1376d3df7b4be87aa5d751.json
│   │       ├── 833fab4781dadad8b62ef280f8b25978f31a712905cfb28da12a1054f0f8dd66.json
│   │       ├── 83caca8a4e43f0e15d39353de9a0d40faefff76ec54b55672b8de5dc371dc259.json
│   │       ├── 841746abd3e968bb2a8792023965dbb374e49286e2d72f7f8ac37de6e6a069ca.json
│   │       ├── 84902a865efd7bb87f1437798e1ad6c7d72c2b3390695895d1ec4c1f48083646.json
│   │       ├── 85251072efef0ebe8f365288488ef34fdbc14d299b3494409f9158049fc06c9f.json
│   │       ├── 858e1932af2c84f89ab965e0702b2aa07a444aa32cdb9c7866af9d2a61c9f826.json
│   │       ├── 85a9ee146fc456edc5565ff04e3641d436d0609875970548446238b92d069389.json
│   │       ├── 85f342f296c9ce1028bc927c0cfe4200961d59d39571c599d001fa7ae84d7099.json
│   │       ├── 860a577c939e1d20ff13cbe05b0b5d3c823216e30f6b6622114641eb3d572e12.json
│   │       ├── 860f03000666415d33ec2b45af010976a30dc7426fe8381a57105355d596044f.json
│   │       ├── 86cf54a96382166346241c5e608f1fae0d154c7ccbc88cb510b8a6253538006a.json
│   │       ├── 87a7a566bd10478b0b4b10782c36a4d472a088e9c4b02dbd2715b9e7d52d933b.json
│   │       ├── 87d8e3c537d9843a26ed83a1e44862d921684112522bdcf228b03aff9213d95b.json
│   │       ├── 88b1eb695491892d61190c0490f5484b240a6bc47ba902c06ccfbe6028d6bb5e.json
│   │       ├── 88d07c2fa2103b638f9a8b8f7860338c674bbf49c173d5a075d3c0604681d2dd.json
│   │       ├── 898be30a941bb095914e53dc6e3ce2b4069cb75fb2f3acfedff904d108e7ea0d.json
│   │       ├── 899af9047bfccf7765627281db37eea131417dbe9b5eb84c70743f05f36318cf.json
│   │       ├── 899f62aaeb60f40f5f33c20d2f155cd08e382884110f36f6806e516052421031.json
│   │       ├── 89c84adb4e9e06e8f3789c5f2fa22d927803adc3d416629bd6accfa70b052158.json
│   │       ├── 89f388ec6f6019d9db3991b621981c0f733191ba23387d065b04e99e1a8b9222.json
│   │       ├── 8a8ef3803c0dfb766c9ea55976ebde077c0211b0cb2e53314dd3be1c9a9be5fc.json
│   │       ├── 8a983fdaf3711b0d99af1b76750ea34559a31896921a2158c80181b8b215a4c3.json
│   │       ├── 8ab44a946fec1191d9a8f5dc22cdb50c043f354b280297462cda26a877bef5a3.json
│   │       ├── 8acc9ee599a7251547df9298f48a2a4c0c0a37768451e4db3f332b026dd3cbb4.json
│   │       ├── 8b7bafc130cafbe1faf123b15d6a85f9ebc8e62742f7ddbed2710e486cea9d4a.json
│   │       ├── 8b9f187566b4b9c7d58de4a8a2f7a2e7094c9c3ee675fb9acb70b0520a7f3e6b.json
│   │       ├── 8ba2bc43c14bd4d0d52adedfc13fb6d5b405c59406610a621eba93b28244c10d.json
│   │       ├── 8bead098d9ef1995f623ff1c7e95adc6f5f5803be7ced1042a07e72b16740713.json
│   │       ├── 8c3efcdea3a4796c0d53fa2fe27618ee75b267b41d036a6f799e89ddac1ec021.json
│   │       ├── 8c8f5361b8a1dcf4f3b5466586d4988b249f734605df1a9a64e66e2fe3fea0e1.json
│   │       ├── 8d29cdb3a2be1f8435365c10c4944e0ab637f96fdf9479887b3b0d15e9b334d1.json
│   │       ├── 8dce6760ba01b67474b15449acdaf6f16bcdc58be173d716d1cfd875dbfdd464.json
│   │       ├── 8dde0b64b3953deb7f13dc6fea7167fd87bbb84be44fd980b01b999bc7ef3bc3.json
│   │       ├── 8e140d06e586f8f085bf36a4f05d6d90ab88ce3d7c95456c507c79f4eb833e63.json
│   │       ├── 8e8b17fb4f2489bd6d8bcbf0de267dea14670690bfbadd5098cc1e30206bbdd0.json
│   │       ├── 8e946ca193d644f69dc203f792994d669a0209586734892aa7ff29dd608e2960.json
│   │       ├── 8eead3e611787ec1578d178ab32f7b66950804066be8f790d6478803b4094cf8.json
│   │       ├── 8f642ecd5ee981ba6b9a81f40ef253e6a9f0b8d90eb27efe6f9573c80ca57d0f.json
│   │       ├── 8f7174090c99cba6245c4f1876eb2d8643bd442b98a72b04e5a21440851dc07e.json
│   │       ├── 8f9535cb63588c8f41124ad847c0d9200d955c92f1803a23b5e135d12ae92b5c.json
│   │       ├── 8fa4a46ddde4327cbbf2b4467ec58c9833816820012382d9d715cb7fd0b4a9fd.json
│   │       ├── 90501fdb979a3b48a32710d84eda261abf3c2f49a004fafd7abdcc4c7916a033.json
│   │       ├── 908848e8aefbbf73cfd8f7e06636468ffd39fc27629765076ce39085232732c8.json
│   │       ├── 924fb281abc79a58828b2e88f65fde8434b7371f93523cf6970ec2f7fd5cd435.json
│   │       ├── 92f92cdc701e13987dac2c6519fc10f49e3d4268e9765e153567bca956f113a2.json
│   │       ├── 93b20c7cab5fda8863409c65c316fee41b2c09219f83e544f24c04ae2b5d7733.json
│   │       ├── 945364388b4a2899d12ee80a1a830a2cf490159b284bc99a267576d01b930147.json
│   │       ├── 95856a5464ec61aaf1dfe76adf9cd876d9ab16c747119f2ed7ff2cf363690d64.json
│   │       ├── 964a3e1791296cb0bd6fcf6717320860d1d67eedef99e365b1336209dff0bccc.json
│   │       ├── 9671b6313a68c5b686ee24985a4f048a5b94ec82dfe4602290f378c35ef2438f.json
│   │       ├── 96e690376f23cafcdd887cd8cf0e7dabeb5d355230f7cde3f2b2b9460184853a.json
│   │       ├── 972739347f1982a773827f863a755114d4523cc25cfd4d81da42edc6b476f3c0.json
│   │       ├── 98b246d96a37f56c2ded22b331c0e04f6555d9c2a1ab91a97a5569b71106cdec.json
│   │       ├── 9924a6d9cda3185061804649d2dffa3b9ac71792427f3b7a894afb48c10f18a9.json
│   │       ├── 994e13fdfd167214f04393a7d10336de8c536e4a78ce0aa7053c73065fce1fe9.json
│   │       ├── 99f746eae6811c04f9356c52c6557d07a5d698c92dcddb485878407d1b406c60.json
│   │       ├── 9cd4ce99576b426e8a42eac669938e5362dfc3ed4315ac5f0d097d60fc7fc86c.json
│   │       ├── 9d1fa29ba2860a366d5e9af63db0f37d54e62e2ccea687b8149194d2d1274ba1.json
│   │       ├── 9d2a330f351a887d2aa0e7b69dae9c74c6d9a329f1b4664600b237eb8d78e1c7.json
│   │       ├── 9dec4a0bf0dfe2c87b899f47f453db3f6b8a486487456a627a20c3436524f373.json
│   │       ├── 9df56b9d0a5aecd30587cbc9c9349b703e438da45ddc64945221d456457277e8.json
│   │       ├── 9f415a798de5b4f9e8a2212bd01bb38407dcbfd58dc88bffcfef414dc2c616e9.json
│   │       ├── 9f7e0084efc2db0907600419169537560cb31b19887a864513e36448b6d95ba4.json
│   │       ├── 9f9445dd1a697cf4f19b30d4cf40241cff718c06d2d8503ccb62fffab50d1b4b.json
│   │       ├── a018c3ea464236b4f90be2d8c1ae36556130e08ddaa70e3eb7c4809820bf5707.json
│   │       ├── a039a08620a5611c1855fbfb00a544cd557f5aa3c0d7aadaa02e12f84873927c.json
│   │       ├── a050266a016ab17cddd8a657887cbba27a753a3c7bd50e5a84d5773b38439593.json
│   │       ├── a0522f8463f1bf0bd9f944d7c5c52c524481136cf1a1b3e58ab81611ff02dfb7.json
│   │       ├── a077554299d38d7e5b339a5de49ac3bc99430da1c22c35770676d08b7cfc9257.json
│   │       ├── a08987f08eb54cddbdb72706dd93d8cf9db3c072301a34ceed07767504a74ee2.json
│   │       ├── a0a268285de085fd2cecacd2720a9976c50bb987ef32b0c67270fc26cc9c34fd.json
│   │       ├── a1c84b42ec1f7ce011060c54412a03960b3fefcab5e85d7dc5d571388b490786.json
│   │       ├── a2027054484875a303d6cd65d90c7ab9bf48bce89b0316689aedcaeacf64a5f6.json
│   │       ├── a298e13493b67020c887d6a685e3196d7a6a25a0cb878c32bcc05a7eec519ad2.json
│   │       ├── a2a2cbf2f0a9902e1401df1d10b67f75fca61fe1d1c9ad5d4d79992a9abc38ba.json
│   │       ├── a38fcf70e673abd9cfb69073c0a5f8430d4ac391cd8fd1a6c03c9178aca81022.json
│   │       ├── a3da29fa7c03c4cd9abc7c9390dc58c8e42dc9ef650b51a25ea90b1eb9d12fc8.json
│   │       ├── a3f28e332b5fe5fd3b69e4b5fb2c549c8a52a57be3e0980b390eb1b535d10e62.json
│   │       ├── a426d76e6d454635977070e29ebb64b1478d30ff975429228c95a4c8e4330e03.json
│   │       ├── a4486b043d0a6fc19751d14c13bd3a9939c1de3ce3af2a13b1839b7d565ebe8d.json
│   │       ├── a46b205ba13554dd2ff25052400df438bed155996636addb880a261ebdfee038.json
│   │       ├── a4a935c0be542a015da4584dcaefa3d3bb10f46ed9005460bacf20bb90c3399a.json
│   │       ├── a4f6c1778c1ffa01462eb9eaea15959e753c092ad566e04845a779bf8e735725.json
│   │       ├── a5d38b92e49633d18fd6057317eea3e1cc7c33033f1c38dcb9b91f1ebe7f4af8.json
│   │       ├── a5ed1ef2334da1ccc6696b931d406d855763e7fe8d6867a94d0870e5c419f002.json
│   │       ├── a648ef124c74ec84a1fbca0ae44d31f10a7246708314ba309d597904ee2ef139.json
│   │       ├── a6a7adf1b375de838bd49fdcd09f0166f292908a57b44501d9c029ba3915bcbe.json
│   │       ├── a7634abe8cfeceb6d7fc68b0b6e3b1417a669bfe32b0c5f69a651201856a063a.json
│   │       ├── a7d34eff7ee6242a157f5b9747c20a9c0314a1c10b71ed136f16ff84c17bb974.json
│   │       ├── a807b77eeb4b5512cba31e3e9e3cb988c11241218fe9c5e9ca7e0cab78283335.json
│   │       ├── a829115bd1c5e49a64bd7f8b4c5d5819080299218771cdade9c2f27fb5407010.json
│   │       ├── a8709b6ae87eb5687dd4ee677b97843a523011697e287339915d6a48f65d1a17.json
│   │       ├── a8af8fa53acfaf6adfba1c87bf94b796ae4b9fe6a951dbe059601746bbb42663.json
│   │       ├── a940a2a343c3e6a144c1807bd0414d87a0f70bb1798f618121a166b9f5286557.json
│   │       ├── a9491dc0eab968bd702c876b1b48e1efb312e018dd9850c17e6e60154af2fb4a.json
│   │       ├── a950f25024c10e7fe70744954329347473fbb9289a2445f05e5169f62e1e2de0.json
│   │       ├── a97defce33ff3abbe8f3e7150ad4e4a4abdf14f8c83eddbdf1b48d97f47735b4.json
│   │       ├── a9aee9cdb2fed70d70fd351b6639f4e9deaa5da68abadc9024bf668b49fa6da5.json
│   │       ├── aa4db3ca246e4ae1dbfc3232d8252a6e462b3989a2dce015364a87e07cf8bf99.json
│   │       ├── aaa6efc12f74f30ffe9caf674403eacc60e45dc4c8bbc861576c8ce3e8febc5d.json
│   │       ├── aaa8ae72d363a9947fa5139626a5846257e4012d4f231d43e19d79bc1e3d8101.json
│   │       ├── ab2bc9d93ffe92e8594a5a2ac494b952c1ff5360e49dd613b6d5743366d80756.json
│   │       ├── ab9e871f565e437940e658b7b1760d726ec6a01ee267fb209dc9bdedb2358061.json
│   │       ├── aba73916fbf912dff623a8438c646acb4965484e4a98047941c75e84ca9e95fb.json
│   │       ├── abdb47cf8b45800685a62d7c3f5a2dc35c8ab9e336ab1d5b7660e73aafe9de8a.json
│   │       ├── ac06ecda8d562b3b467c812a065c8c5a37b7a85c9b489a542f8e9e96583b6554.json
│   │       ├── accb7de01d2ec70ab845d3a603b5809acff492288433325a1a2d99c1466cb9b3.json
│   │       ├── ad9c83a37d3938404dd687aa43dd226e6a1dd72491f0ca04d16e593f64031094.json
│   │       ├── ae18e92f32e157271ceb04d9b5bd8c4fc23f086dca70f78cc8814495303ac2db.json
│   │       ├── ae2d99c6ac5830949cda2da3d5187c26ba97618ef2a77e603a52118e9bccf23b.json
│   │       ├── ae39c900fc031ac373fac32040d960317ecd2b36c0a0b40ba2594da635c8efa1.json
│   │       ├── af170e885e9a80dba9135007720592bd80f129ad871d266f9fd07246ff0e89ab.json
│   │       ├── af1e291771d56b13ace8b51c990550329913d4f2a023671c4d2d83912991443e.json
│   │       ├── b100fe56d063caac0e123f045b87d971039ee95a631fe35d53d9529dea5a8b93.json
│   │       ├── b1441c17531ad8eb7a2ae1781af7d9450c7159d3758f2d6e25325fd75ae0e6f7.json
│   │       ├── b1af0bf24e930a42585451ba0ea727d28c5d1b3dda527685540aa1c1b6de553e.json
│   │       ├── b250eb085c0e0707414cb047f050a9cd4e4797a67f23038e75bbc44bb88142cc.json
│   │       ├── b272f527f962d95785ca247aeee94b9596668c1a9a1880404c29777c7ca00818.json
│   │       ├── b2cd9e30e87e74a2571f604cb487ad3970181f080557fea1c79cf1f6a157c86c.json
│   │       ├── b2f4f6f4bdee687edbb337778866ffd421f39e0ff1168feb7c71a78bc3d499d2.json
│   │       ├── b3317edb54bc8c2b76092e610fc88956b3261520d9f1500b97e3879ea38a3818.json
│   │       ├── b379c6e3dc0381e01d42cf48d89c2d0d78acab0037eee8696c5913f72fd390e1.json
│   │       ├── b3f0f38f18ec3d57db9fd10773495be6c42f7a75f02a1b5d3ba3d32bfa53d37e.json
│   │       ├── b50dd867483a47f24d1b74bd1aac9e44fb03b2b676c6b1d35fc6e8616722b5f8.json
│   │       ├── b5ce575b831786c6355acb15a8b59334b288d42e199b54bdd0e1b18fa027deeb.json
│   │       ├── b6f9307b4ed5ab7544ae900d33cbe7671a9e84924a9a6566aa326b35942be8bf.json
│   │       ├── b7aa8ae938ee1c0995389908f810d9641b5ff70fbad2025c15e4aa2eabbbef98.json
│   │       ├── b7c59c4af8ac9de6a3b0c34c0f45bb79b767218352472ee5bd0f2a4683bedb1d.json
│   │       ├── b83d66c5ed031a77fbc189637e42dcb714355ffd9b65d7f006ebf6b7f94dbf4d.json
│   │       ├── b83e5e1a57de2fc77893b81386a877693daa5ce35dfa20ffe71a0954a88e27b9.json
│   │       ├── b928e6d54eeaeca04fbe55989e3a5780387443a078a9f22dca49ccf34f8d6c2d.json
│   │       ├── b9338eb953fb3ecfb48f4c8c198c939be870d9424e2b67b2a6a8f88c290add72.json
│   │       ├── b9523fd15d28edd673fde9c4550b2183bbef4afb75ac82c996643d21b6c071e1.json
│   │       ├── b9b28d500074581c574c164c3861ddeaa4f77a4834fb6b3034d0ed5a7823ade6.json
│   │       ├── b9bd81ddac637d5ff408b688c34e992c403f78cd0f49a48911b17692663ee5df.json
│   │       ├── ba2284cc67a0a8634487f7f7b5c062f556e283884a710b0c0cbea4bbbbd99ad8.json
│   │       ├── ba4a14691d1e24a795ab165fad7ec630de425815e53adca2a30634d431135316.json
│   │       ├── ba5b7658a34c93e5046c6d3949c0b1a8cca5a1abc427e569696135a654b9f1e7.json
│   │       ├── bb2d2f4e6e4b494901c91d57457ce9c0b6d66e8a1f6c02956b42f653b4a9aca3.json
│   │       ├── bb3590c37de189d51be57244c420c2831932d2151920045d5bd4806de8540e88.json
│   │       ├── bb770c6f0b9750bb8de4913cf5429c4a9b4d90828f12e6ad2f1edb32ba33f69e.json
│   │       ├── bb8529674a6711996d7321af7aed0d8a5e9530d6aac80cebdb66c337a29bb34b.json
│   │       ├── bb9a248b1dacc6fb6e182e312db2d8381353ee7c47a89bfa3209d5de00022ecb.json
│   │       ├── bbe45de066d5ab9445593a555f0369f09b487395282ca250d895201f6c7120d1.json
│   │       ├── bc48b16ce0b188193a814f6e7643f41b887000342bbffaee3fcfd6a945660acf.json
│   │       ├── bc74c8381c4fc3686fc686fdf4054e87b55c03c7f27b2f8d67b8543b460da666.json
│   │       ├── bc7beaf233a97a2cdc5f684e32fe56b75ea7401758a1a18092d6f8e5adf140a4.json
│   │       ├── bcb8b374a94c8ec76d23a35f1cf0ff34083dc2f21de45e383563a8f7ace128ad.json
│   │       ├── bd0bc1c593d05b0d46a2382fbd7482702b8a82bebbceced15973ee74cd0ffea3.json
│   │       ├── bd6553d14f72d12d798db9fd123034e4843c91e108717b57be979f1e40fad879.json
│   │       ├── bd85cc8e440f41a3b00ca5c414f96a9ad2b1693b7d7ec6666255e7260e72d335.json
│   │       ├── bd8ce971440485b85cf9551b99a53437c45e042a186f145388a12b9d182a253d.json
│   │       ├── be0d69e9f4157336af2427ff4f3e3a581ed31289af4be12f53ea82537cdac369.json
│   │       ├── be13e8f9833650543686a23048f601e2b31524e8d33f46c9042878ef2366db2f.json
│   │       ├── bf3ac2ec33cb1fa7c3a115141cdce32687525e16878b11084c1c44c79a05b22e.json
│   │       ├── bf9bf0ff0e025bb100010d9b895af7a8cea141d86445eea754b8d46961e15b64.json
│   │       ├── bfc4f144dcea0f0b74befa7f5d7b2caffc742e17308ab19e72c0a5b3e5470510.json
│   │       ├── c027dffdbaf5a7f34a0a4caa60aff0e11c3dfc3328208bcc5a34453689c7cf4d.json
│   │       ├── c04061060ee0785cb9402454105e3d27c96f29ebcf6fc287aed2c875e4e9c5e6.json
│   │       ├── c1b9fab7e152e92f4d2923254754c08f3d274721060cf38d502483225a57cfad.json
│   │       ├── c2719794b06e1091135c747b7141d49db18d1d754c5d49ba07538f2278ebc28b.json
│   │       ├── c27c1b5c3db4393a1edadf34321f3a8b169e752d6471df45b06c28e2d6eaf3d8.json
│   │       ├── c2fb3ca1c5055cbf33cc7ebfedb170576106d51e2e02f35c0f3e5e06da30238a.json
│   │       ├── c305f02f5092617785225139fca4da0810fb6e7e08d7d15f3d7b1505269aa0eb.json
│   │       ├── c3c1cdf870a6eae8d47da5816c9dc16732bc95838ae7158d028442deaf45905c.json
│   │       ├── c3f3f855e87fef23b54c4286bccdc8cd119227bcdccc10b0d490d2513221d53e.json
│   │       ├── c40a1d2f46bb33e3a0cbf32e418861ae120ba0f2a11718d756ebeff0a9445271.json
│   │       ├── c40b08fba0644c0f6cbd94064c462c6101f1bfd06f785c1a5c1d7fc4c6754355.json
│   │       ├── c43ac4993659a618a9c353f8337f2b4774b72c76db86e958929b9a348143d79b.json
│   │       ├── c51f4819c07a4b1fd4abf28820970e38e0da1cc830227e1582bed36b725d00d0.json
│   │       ├── c56a006b5ce112fc9b553ba848bc5da860121b926dc6f85ecb4a0e2893e6741e.json
│   │       ├── c57e8ff117a6b5c8023c4e6096f0060c94b31478dc5413c64a07f72f193e5a88.json
│   │       ├── c5a379db1e7c5e03bbd7a9f93b21369feb4fb878bb34735d91a1ec32485f8e7c.json
│   │       ├── c62d2a5322a82a7e22b4d92e20ede2ef1828fe4419ef9549038b2e4429322fc4.json
│   │       ├── c6679bc4d18a18c9640b4e36b9a54672a3d6b0cac7a3888cc4e13a3c50492e0c.json
│   │       ├── c75a0e5fd28a7d0c0743cb2bd35e300e788d07dbfbe5d2c7c469a0a72c1b0386.json
│   │       ├── c77fd943f5b53c5ad8f38dfbbfbb321d8f74e8e20f774642ef335d86e04b5ca6.json
│   │       ├── c82bc1503a4ebbd9c53b70109ddebd5789161c0f133e8195f91aae364e4b842e.json
│   │       ├── ca2ed59b895a3aca98005f2292cbb719cb9cee853fe54aab4a48fe539762e765.json
│   │       ├── cbfbfc8b5b8a50ee88aa949ede73efac9beddcd18820cbfb55296144c1915699.json
│   │       ├── ccfdacafa51b90c0c30254f3bf716fd9c6044880ed84a6b57601768e6a529ee6.json
│   │       ├── cd630a742eb7e8f0dcd1fe8dbf56edcb3e759377f359e180adcb1719969edddc.json
│   │       ├── cd72ddc699c9da4430044142f4fbc929e2bdaf6c55403fde23256c6197827108.json
│   │       ├── cd855a6e020041993b1c7169dd3c9204213be12aa5ae1150a2b94bac4231d84d.json
│   │       ├── cdf772247000faabebf88c34d8f6d49078e89b5508e82085207a299272e68d5b.json
│   │       ├── cf022f3674ef39116c45b42dd1d2c6716174308e296fd012b8a69ca98f217347.json
│   │       ├── d18425d51db853a5cab41a944fe027ea7baf04edab805ff3e5e5265b8874cc59.json
│   │       ├── d192df02384f2ca4dd41b0b72fedabffcb185f1de3612c23f6753dfc848e23fc.json
│   │       ├── d25fb6b7a678550f738010a9f79c3e3df30cc54d810701f3c824635f9d1e47b3.json
│   │       ├── d2dbba9f40b518f2b8abbdcf7b9f08237b77dd2fd32c7da1c86e6378f45a2053.json
│   │       ├── d2e6a5a1250dcb9d587f558d9db40e0b8baf90b85e5ae7023df987f042f79628.json
│   │       ├── d3a217f378fd68e66210a24bec213d9814da9e046d5154eee6e924b3c5a14e65.json
│   │       ├── d4822b96c36be7dc4441e9e42fdd5a3b094dfae022233e64f06900e64090d31c.json
│   │       ├── d52e3cb07a85f45f1d6b35300a45762593f6404172d4cd7994cd5c90ef83841b.json
│   │       ├── d557bdbbf6f61e84872302b992c65d71855da3fc627a2c9209b11523ffb8cd06.json
│   │       ├── d5ba29dd7e9fa035fc2f8799560979f0d7a231316e053da966f7fd5691a50ad3.json
│   │       ├── d5c796f5ed2168656aee035179ac21d65de171bfb9d06ab653378e0e4f7cab14.json
│   │       ├── d65a17faa2bd6bbdcc32ba039cdac68317bf3177055f81433b691bc3b021b100.json
│   │       ├── d66d7367cc3f7dcb89710412b91a52b0e867a8a27cd53b7ad4eac773dd1ea82e.json
│   │       ├── d6f11db4639206c6c1958aa100d4df95e195e9e64688eb76f14a830fbc45fa5a.json
│   │       ├── d756cd3fc0aaaacf15b1b5b026fbfcd28d6b737f9968e726383d822d2e710157.json
│   │       ├── d7630393dfc54404362a51c8c6b076827996a82aa9e2dddbdd5d18694eb1a509.json
│   │       ├── d774e4f8938b33c3e5e2b17fad8e08600d847b5e1418f2d9a066dba1b0fc3c7d.json
│   │       ├── d7a3fb61af0652fe3a59d1aa27beaa1ca63d6477d2ad600c1f22d9be9e639348.json
│   │       ├── d7e549cf622872079b144e4b61ce7f121bcd17b319fec09ef64fc621818d8ef5.json
│   │       ├── d7ec11b17f8504808e01bf74e46ee2b51e0238c0fec177a61c33d28b1fd77670.json
│   │       ├── d8b25ee85de91d7d131e963f3f54e4e5d0f4862531eba941baeb1b0090dc6107.json
│   │       ├── d9a11181b8fe8404c49192e2bfed6aefb42c5d38f830e0be96021f08d35ec324.json
│   │       ├── d9bbe351674182b8f78e1a4814ea49de7cfe25ecc25c104dc34a1884047c3478.json
│   │       ├── d9bc06b37ed7e9c0761f952588ec00ae62c960124e4f428231f365a20069ca05.json
│   │       ├── d9ed5f424decdc4adbac3497c29cd73cae791306c634513614aba8ac484ed183.json
│   │       ├── db4ca54635a7fd443aef7791d8263b20bdd36b863f30845d60aa75b35c06bc48.json
│   │       ├── db8a457d7883aa58f0a2c09039a849613e137f1a5290a458e58dd072ee35c7e3.json
│   │       ├── dcb296386a336e15b15cfd0b97fd4ade18ce0229234acb7da75939822cee47bd.json
│   │       ├── dcd5da85b1f7b5221e10e755479e73e17297acafe06634a7572f81cd74018e86.json
│   │       ├── dcd84a58cd9c7ec6db6accdd5627a64e368c9ccb97ec8a0d06a989ccb37cc426.json
│   │       ├── dd20e85b52536a22e8bfe56e9d729a5bedda9da7230eabf422decdadd14c575c.json
│   │       ├── dd3d6e0c54b43298074e429f42142a22611344241da41d55bff70607d6a0fdff.json
│   │       ├── dd4a6a1e96b5d19c1f4a7296fc4ef3678103ac96422c468d64d6d303d456843b.json
│   │       ├── de23d94e154385c07380d833ed59536469f9e5181d324b51585b8ccc97bc2ca2.json
│   │       ├── dea6a6bb23909f4b5048462e30c68442eb65536f0975c81a318735a7a6a73468.json
│   │       ├── df0f51ff8d66df5622d91f53925006cec67f8ce717d184d2077f05d6c203245b.json
│   │       ├── df9e6635deb23bc446813dc520c3da542b0613a5a8611c1cd1ef4fc9124784e4.json
│   │       ├── dfcdbab42af59e9c34e0a3ad1453d97dec433cb88d7715c372f8a100566fef09.json
│   │       ├── e033fa26763738d8ce53e174d1e8d5eb7c94a91c20ee4911ad96671dba56055d.json
│   │       ├── e1bacd9fa30b8961cf256210979b80fad54c46d41e07df93c3824427d734bfbe.json
│   │       ├── e20e18dc88a8a2375121d75b5406489862ddb9d54b3f9614cda73bd877e448f2.json
│   │       ├── e24b47f4e3d0d99cc5d192238e7eb2dbbf02cba2c720ecd406126bdde27a163e.json
│   │       ├── e269c24ffe1d06bad404475e4a721b7f1502804bad9ed3349d36b062ae4752c0.json
│   │       ├── e2c3e3843f457e9eb90e442f0c7680d080af2d76a89a86f4edc4915320efa386.json
│   │       ├── e314c8f214745aa8b58d870038149200cd9119d6154c06ee5392689de20d3ece.json
│   │       ├── e32654d9067656a9e0ca071950a9290519287f9abefeae93cfd85bbda4c7ed28.json
│   │       ├── e4016e2580d4578486d4cbbaccd72e2a79a5600fed07244400c883b94b994503.json
│   │       ├── e4098aa46096d1e65ee43dc9e347a75aebeb1e0df2a5216d820d134105f0b9d9.json
│   │       ├── e4273a800162e8042df862dc32a268268bf1e620c80acd32412454206bf4aad2.json
│   │       ├── e50f00b4dc84276a624e063b39fa45d671b6c6879cb65a6599009466d57c8883.json
│   │       ├── e518d7b8adfc5c903d917aeacf1c56cf95c53f2c1a067c59869f58e08a536143.json
│   │       ├── e5835f49a75caeaab76aca0d955bd0bf0687d466560e3793462d252ec696d588.json
│   │       ├── e5ab2f258c8a92cec630a3889c06bb5289b1221e350720c1f3e64576a646d362.json
│   │       ├── e6c321a22b38133d1fe76767808cb19bae1d35793339e0749fc05213a909e0cd.json
│   │       ├── e6d4d9d3dbca72d90df6914db8136aad11d072fd7caefd7fe6d0f1513309dacb.json
│   │       ├── e72cf671ee385b3a252f3203107b4c8980ffe0326f334a0d79eaf6f8af3ee70c.json
│   │       ├── e7306ee1a58c5d5e88d4b5b3a18c533baa3b6f27e260bd42a069a0e7537700ff.json
│   │       ├── e7f90129ef2d3fb1fbdd2ad1ecd3a1f58f38ca2ce14842a2f0882c9ae8c2ba59.json
│   │       ├── e88aa81ce5d9a747d98050442e5a4183d8add52e7c62fb79c6a6877c49fc2947.json
│   │       ├── e8a019f8b7fb611b0947296f51ec20b3bc36c426df95a96569ec579303ca409a.json
│   │       ├── e935c0469ffdffbd08bd53e9c2a653db90b3501c493e99215f595082b535c33f.json
│   │       ├── e980099f903172ebf3244008570807e559c44eb4435431a6c50420fde6197de3.json
│   │       ├── e988cc2d1532e9014cf313a41d99c8e191d2849948f94bc04d2b06fc1d14c305.json
│   │       ├── ea0b991828f6cbf0e061b34b3e1cd8af0fc3898a43b13b1b01d854532f1dd5fb.json
│   │       ├── ea4d4a431052a905fa6ff5d125d82832e4cb38b7a846faaf587364cdce78e875.json
│   │       ├── eaa0e3a04b2f0d3da50534eb78f56b6fe39e5d48ce9ff6be7afcefd5ff570b4d.json
│   │       ├── eacb68af3964334774681d53b9c529becaddf71a5e3b08bf3cbe28628efdc795.json
│   │       ├── eb84817eeaf7677afd969cc469f22f311cd68c869638e0d0f8b427174b074977.json
│   │       ├── eba73cd83fbbf3ed89ca6aac20c25af8d85b089a9cc34ca6865abe8d12a6c20e.json
│   │       ├── ebe5ce7d056942d0e86789a84fb0aad499a227ea1bae18e46dc45cf874f52dc0.json
│   │       ├── ecc90c1f8d5639bf0364ab9fd7045f196587ffcfdb8aba0c67fa2d38e9ef0b62.json
│   │       ├── ed08f9a9210e64e968a2ee030056819146632d63a3bd37a2f0dd06de678fdfe1.json
│   │       ├── edcd6193f17051740b0d658d1c0224d7a93cced3ebf8ca93e8ef7fb25c2da0d8.json
│   │       ├── edef9ee552bdcf73254b35c80115c50c4a2cdc62fef5b0f6dfd533515928cd57.json
│   │       ├── ef889fae7b36b62b69cbc56f4b577d713286a2b60598007db56b7c51303e6e7d.json
│   │       ├── ef8c8c601b512fcc8fbe15c383e2aede984102d9c82e858a05d116bed3807e2b.json
│   │       ├── f01020ddec620ee3e2aa10c4844e10d05c97b34109a7386f42a459a5de8fd4fa.json
│   │       ├── f01e69056686f2f7c20790d99929f4ebfcc2e34d0d01a186ed73dd7e6ce41901.json
│   │       ├── f038c7fffaa12c62fee5026dd955ebc1c3440ae3afbfae9cafc6c8644b1093e8.json
│   │       ├── f09cc6747cdb85c1636c3404ec3b1c599714dfda453ed0fb5514b85867e73840.json
│   │       ├── f1d2f06200a4b9c2abb65b6a35bd16724868a28495736c6b3ff716dc03a29dab.json
│   │       ├── f1dfb2331469df41bf4c696b5a409a5c9c52b75581dbe12d736d0f6fdc46106b.json
│   │       ├── f2524c2d5445212619f04afc13959e8bc763d12b31a667ab5cc5eced832e1b8d.json
│   │       ├── f30a9ca1761235051d0121d2ea47af993e58b7d646ba0f0620f8f7e81e5e8b17.json
│   │       ├── f38b587afdfd671ceb383c574a8a9e770559e9ac56821ab588b525a903062a0b.json
│   │       ├── f3ca578e2cfe9cc5dcfb7d66c5aa1d3d19db966ac6e20ec39578b3ca2ffaf39a.json
│   │       ├── f42da213befcb45aae560d3d4132faf53f752ece087651d4081805b4f2319911.json
│   │       ├── f43233e8a6469c1ed2892a372ac837dd48d0cf5240dfa92756e6b4bcdb0398de.json
│   │       ├── f442b0f307ea758f279f6f5058d0daf2c730fc4d6067f86f56bd70000ba19e74.json
│   │       ├── f4bc13f4d2c00c00c57006d27d2491e08062b4516237618fefc193aba09fae15.json
│   │       ├── f55064e8ad87e5f7f13cde4d9626defe1773e44f42c4c96119fcdf247cb420c3.json
│   │       ├── f577c77b5caefa2f31ba26f9e78a44ca9fc4fe56f78e0824be89284cbd22255c.json
│   │       ├── f591200c037a2089473e958678abbaff650d77167c271251edbce03917db3a3a.json
│   │       ├── f5d0160631a6b1cad59af9d4aeac750ccc3e9aa20ac6a75f17ea4174bbc2f57e.json
│   │       ├── f64b26de986600114b4fd4c01fea7caec8a2803962f7e346022aeacc943b877e.json
│   │       ├── f6fefad0d3b4e8ed60480f6306870407ad5b8c75b27c0fc1342fef5040e86b63.json
│   │       ├── f7f490468c35472ad9e21158cd56efdc1c5618a159d15e196257084068f28ee8.json
│   │       ├── f80bd5957f4403fc38a10ae2ff781faf52bc661aed0f891d8afe03e0ca033bc7.json
│   │       ├── f822922ca44c947260e74c6a912eb6ce118a898cee26e46a2465aaa58d634883.json
│   │       ├── f84183fffab007fa6b69f905425b1db46fb32a633639ddeec2b2ed583d02463a.json
│   │       ├── f85020a804443fccd6f076eca5ec22a5e4ab976c1a650e6789e2f02d29edfb8b.json
│   │       ├── f870e41ff9854316c48dd5b5058b1df6001beab42632c3541ecd58b48165ee33.json
│   │       ├── f9a0cb680dd6d4db68be5d864ef3471b3127c97b09ba361c98a07edc3e97320a.json
│   │       ├── f9bdcc499dc76b6d85747ee878046b28630129d6f5a1eabb153bc53cd1f7ff84.json
│   │       ├── f9fe4987bab97dcdb7356c6707cbbffa03e1156c19a58b5f457eff53a11e9305.json
│   │       ├── fb53bd1f23f1b8fe6a4975ef1bdfeb23ceb22f365efcbdcca25f45a60aef5bc3.json
│   │       ├── fb69c0a9a8a1661e631238931fad783e5d39dfa63ec94fb126bb783143fd77af.json
│   │       ├── fcccc6c30b92a8706b0bc671250b14bd5551537918be9d16949e036f76d0d60f.json
│   │       ├── fd2c0d4bbbebb1b559e34d8461abae19b599b8a83d5e2cdf1e419abe2e32bebb.json
│   │       ├── fdc6242e2c0903afa210e8777783849add30d3f9eac569b0e29fe407d24c65be.json
│   │       ├── fdec000e138b99031a56299d3d66bbe6214e6e396be812b69e7c7d2d66a10d34.json
│   │       ├── fdfb3e72e2d33a1e7da0f4b9eaa48d22bba9d3158c53ca6150b3dc13114623d7.json
│   │       ├── fe06bc490c0a661957ea72beae7079fab3a32a2ae2b531796c689450ff96b272.json
│   │       ├── fea2a148073c148150217067d2a0eb0b645fa37e744bf3aa8e652915fabc61bb.json
│   │       ├── fecb32941e991c017b0d29b9333404ceefa69f9eacd1032e3818815abe757e03.json
│   │       ├── fee2f82aea1311852cd86f796bf6d940ca6f333cd250579a4737538977408d83.json
│   │       └── ff8305e91ddd13d0032ad3d60cac78cb4a67392126a56bd48a3d68339e55cf67.json
│   └── graph.json
├── install.ps1
├── install.sh
├── install_jhol.bat
├── package-lock.json
├── package.json
├── reasonix.toml
├── runtime-benchmarks/
│   ├── ENGINE_EXPERIMENT.md
│   ├── array.js
│   ├── bench_runtime.py
│   ├── fib.js
│   ├── json.js
│   ├── loop.js
│   ├── objects.js
│   ├── results.md
│   └── strings.js
├── scripts/
│   ├── FREE_DISK_README.md
│   ├── add_binary_from_npm_lock.py
│   ├── bench/
│   │   ├── README.md
│   │   ├── bench_jhol.py
│   │   ├── bench_vs_bun.sh
│   │   ├── benchmark_jagr_comparison.py
│   │   ├── benchmark_large_workload.py
│   │   ├── benchmark_repeat.py
│   │   ├── brutal-bench.sh
│   │   ├── deep_profiler.py
│   │   ├── fair_bench.py
│   │   ├── final_accurate_bench.py
│   │   ├── professional_bench.py
│   │   ├── profile_cold_install.py
│   │   ├── quick_bench.py
│   │   ├── run_jagr_benchmark.sh
│   │   ├── scientific_bench.py
│   │   ├── scientific_jhol_vs_bun.py
│   │   ├── simple_jagr_benchmark.py
│   │   └── ultra_clean_bench.py
│   ├── bench_daemon.py
│   ├── bench_scales.py
│   ├── benchmark.py
│   ├── benchmark.sh
│   ├── benchmark_jhol_vs_bun.py
│   ├── benchmark_offline.py
│   ├── benchmark_scientific_v2.py
│   ├── build_binary_bundle.py
│   ├── build_binary_packages.py
│   ├── build_pgo.sh
│   ├── build_release_safe.sh
│   ├── check_benchmark_regression.py
│   ├── check_fallback_trend.py
│   ├── check_guardrails.py
│   ├── clean_benchmark_artifacts.py
│   ├── collect_baseline.py
│   ├── compat.sh
│   ├── e145/
│   │   ├── apply.py
│   │   ├── model_texts.py
│   │   └── verify.sh
│   ├── enterprise_npmrc_report.py
│   ├── expand_binary_package_set.py
│   ├── framework_compat_report.py
│   ├── free-mac-disk-space.sh
│   ├── generate_codebase_tree.py
│   ├── generate_paper_pdf.py
│   ├── guardrails.sh
│   ├── live_bench.py
│   ├── live_bench.sh
│   ├── live_bench_jhol_vs_bun.py
│   ├── mem_guard.py
│   ├── performance_dashboard.py
│   ├── precompute_unpacked_binary_cache.py
│   ├── production_check.sh
│   ├── qa_matrix.sh
│   ├── reorganize_root.py
│   ├── resolver_fixture_report.py
│   ├── run_benchmarks.sh
│   ├── safe_edit.py
│   ├── snapshot_tree.sh
│   ├── tui_check.py
│   └── update_binary_profiles.py
├── slow-to-fast-documentary.md
├── src/
│   ├── bin/
│   │   └── jhol-fast.rs
│   ├── jhol.sh
│   ├── main.rs
│   └── tui.rs
├── test-project/
│   └── package.json
└── tests/
    ├── benchmark_suite.rs
    ├── cli.rs
    ├── compat_matrix.rs
    ├── fixtures/
    │   ├── expo-app/
    │   │   └── package.json
    │   ├── express-app/
    │   │   └── package.json
    │   ├── hard-peer-cycles/
    │   │   └── package.json
    │   ├── jagr-instance-bench/
    │   │   ├── bun.lock
    │   │   └── package.json
    │   ├── nest-app/
    │   │   └── package.json
    │   ├── next-app/
    │   │   └── package.json
    │   ├── nuxt-app/
    │   │   └── package.json
    │   ├── optional-deps-app/
    │   │   └── package.json
    │   ├── overrides-app/
    │   │   └── package.json
    │   ├── peer-conflict/
    │   │   └── package.json
    │   ├── peer-deps-app/
    │   │   └── package.json
    │   ├── react-app/
    │   │   └── package.json
    │   ├── turbo-app/
    │   │   └── package.json
    │   ├── typescript-app/
    │   │   └── package.json
    │   └── workspace-app/
    │       └── package.json
    ├── jagr3_grm_integration_test.rs
    ├── perf_maps.rs
    ├── phase1_4_integration_tests.rs
    ├── regression_bun_npm.rs
    └── resolver-snapshots/
        ├── expo-app.json
        ├── express-app.json
        ├── nest-app.json
        ├── next-app.json
        ├── nuxt-app.json
        ├── optional-deps-app.json
        ├── overrides-app.json
        ├── peer-conflict.json
        ├── peer-deps-app.json
        ├── react-app.json
        ├── turbo-app.json
        ├── typescript-app.json
        └── workspace-app.json
```

## 2. Categorized Functional Subsystem Map & File Breakdown

### A. CLI Binary Layer (`src/`)

- `src/main.rs` — Main CLI entrypoint, argument parsing (Clap), subcommand dispatch, Unix daemon service.

- `src/tui.rs` — Interactive Terminal UI (`jhol gui`) built with Ratatui.

- `src/bin/jhol-fast.rs` — Alternative fast-path CLI binary.

- `src/jhol.sh` — Helper wrapper shell script.


### B. Core Library Modules (`crates/jhol-core/src/`)

#### Top-Level Core Modules:

- `crates/jhol-core/src/atomic_ring.rs`
- `crates/jhol-core/src/audit.rs`
- `crates/jhol-core/src/backend.rs`
- `crates/jhol-core/src/bin_links.rs`
- `crates/jhol-core/src/binary_cache.rs`
- `crates/jhol-core/src/binary_lockfile.rs`
- `crates/jhol-core/src/binary_manifest.rs`
- `crates/jhol-core/src/bucket_vsids.rs`
- `crates/jhol-core/src/capsule.rs`
- `crates/jhol-core/src/cdn.rs`
- `crates/jhol-core/src/config.rs`
- `crates/jhol-core/src/content_addressed_store.rs`
- `crates/jhol-core/src/crypto.rs`
- `crates/jhol-core/src/daemon_cache.rs`
- `crates/jhol-core/src/decentralized_registry.rs`
- `crates/jhol-core/src/distributed_grm.rs`
- `crates/jhol-core/src/doctor.rs`
- `crates/jhol-core/src/enterprise.rs`
- `crates/jhol-core/src/error_handling.rs`
- `crates/jhol-core/src/exec.rs`
- `crates/jhol-core/src/feature_graph.rs`
- `crates/jhol-core/src/file_lock.rs`
- `crates/jhol-core/src/fuse_mount.rs`
- `crates/jhol-core/src/git_lifecycle.rs`
- `crates/jhol-core/src/global_cache.rs`
- `crates/jhol-core/src/harvest.rs`
- `crates/jhol-core/src/http2_download.rs`
- `crates/jhol-core/src/http_client.rs`
- `crates/jhol-core/src/hypergraph_resolver.rs`
- `crates/jhol-core/src/incremental_hashing.rs`
- `crates/jhol-core/src/incremental_resolution.rs`
- `crates/jhol-core/src/incremental_resolver.rs`
- `crates/jhol-core/src/install.rs`
- `crates/jhol-core/src/install_log.rs`
- `crates/jhol-core/src/io_uring_wrapper.rs`
- `crates/jhol-core/src/jagr_infinite.rs`
- `crates/jhol-core/src/jagrs.rs`
- `crates/jhol-core/src/js_engine_backend.rs`
- `crates/jhol-core/src/lattice_cache.rs`
- `crates/jhol-core/src/layout.rs`
- `crates/jhol-core/src/lib.rs`
- `crates/jhol-core/src/lifecycle.rs`
- `crates/jhol-core/src/lock_free_grm.rs`
- `crates/jhol-core/src/lockfile.rs`
- `crates/jhol-core/src/lockfile_write.rs`
- `crates/jhol-core/src/mcp.rs`
- `crates/jhol-core/src/offline_cache.rs`
- `crates/jhol-core/src/optimized_download.rs`
- `crates/jhol-core/src/osv.rs`
- `crates/jhol-core/src/package_index.rs`
- `crates/jhol-core/src/pool.rs`
- `crates/jhol-core/src/predictive_prefetch.rs`
- `crates/jhol-core/src/prefetch.rs`
- `crates/jhol-core/src/progress.rs`
- `crates/jhol-core/src/quantum_signing.rs`
- `crates/jhol-core/src/registry.rs`
- `crates/jhol-core/src/registry_tests.rs`
- `crates/jhol-core/src/resolution_mirror.rs`
- `crates/jhol-core/src/run.rs`
- `crates/jhol-core/src/safe_file.rs`
- `crates/jhol-core/src/sandbox.rs`
- `crates/jhol-core/src/sat_resolver.rs`
- `crates/jhol-core/src/sat_resolver_tests.rs`
- `crates/jhol-core/src/second_order_markov.rs`
- `crates/jhol-core/src/security_guard.rs`
- `crates/jhol-core/src/selective_extract.rs`
- `crates/jhol-core/src/soul_path.rs`
- `crates/jhol-core/src/spec_grammar.rs`
- `crates/jhol-core/src/spectral_sparsifier.rs`
- `crates/jhol-core/src/utils.rs`
- `crates/jhol-core/src/ux.rs`
- `crates/jhol-core/src/workspaces.rs`
- `crates/jhol-core/src/zero_copy_metadata.rs`

#### Core Subdirectories:

##### Subsystem `crates/jhol-core/src/cas/`:
- `crates/jhol-core/src/cas/cas.rs`
- `crates/jhol-core/src/cas/hardlink.rs`
- `crates/jhol-core/src/cas/integrity.rs`
- `crates/jhol-core/src/cas/mod.rs`

##### Subsystem `crates/jhol-core/src/content_addressed/`:
- `crates/jhol-core/src/content_addressed/tests.rs`

##### Subsystem `crates/jhol-core/src/fast_stream/`:
- `crates/jhol-core/src/fast_stream/mod.rs`
- `crates/jhol-core/src/fast_stream/ssdp.rs`
- `crates/jhol-core/src/fast_stream/twm.rs`
- `crates/jhol-core/src/fast_stream/vrb.rs`

##### Subsystem `crates/jhol-core/src/grm/`:
- `crates/jhol-core/src/grm/mod.rs`
- `crates/jhol-core/src/grm/query.rs`
- `crates/jhol-core/src/grm/store/`
  - `crates/jhol-core/src/grm/store/tests.rs`
- `crates/jhol-core/src/grm/store.rs`
- `crates/jhol-core/src/grm/sync.rs`

##### Subsystem `crates/jhol-core/src/install/`:
- `crates/jhol-core/src/install/lockfile_patch.rs`

##### Subsystem `crates/jhol-core/src/instant_lock/`:
- `crates/jhol-core/src/instant_lock/tests.rs`

##### Subsystem `crates/jhol-core/src/jagr/`:
- `crates/jhol-core/src/jagr/betti_numbers.rs`
- `crates/jhol-core/src/jagr/conflict_explainer.rs`
- `crates/jhol-core/src/jagr/core_guided_lp.rs`
- `crates/jhol-core/src/jagr/core_guided_solver.rs`
- `crates/jhol-core/src/jagr/jagr_resolver.rs`
- `crates/jhol-core/src/jagr/knot.rs`
- `crates/jhol-core/src/jagr/maxsat_solver.rs`
- `crates/jhol-core/src/jagr/mayer_vietoris.rs`
- `crates/jhol-core/src/jagr/mod.rs`
- `crates/jhol-core/src/jagr/ol3p_solver.rs`
- `crates/jhol-core/src/jagr/resolution_differential.rs`
- `crates/jhol-core/src/jagr/simplicial_normal_form.rs`
- `crates/jhol-core/src/jagr/tropical_math.rs`
- `crates/jhol-core/src/jagr/tropical_max_atom.rs`
- `crates/jhol-core/src/jagr/tropical_nullstellensatz.rs`
- `crates/jhol-core/src/jagr/tropical_polyhedron.rs`
- `crates/jhol-core/src/jagr/tropical_sheaf.rs`
- `crates/jhol-core/src/jagr/tropical_sheaf_analyzer.rs`

##### Subsystem `crates/jhol-core/src/jgrid/`:
- `crates/jhol-core/src/jgrid/PLAN.md`
- `crates/jhol-core/src/jgrid/README.md`
- `crates/jhol-core/src/jgrid/format/`
  - `crates/jhol-core/src/jgrid/format/header.rs`
  - `crates/jhol-core/src/jgrid/format/manifest.rs`
  - `crates/jhol-core/src/jgrid/format/mod.rs`
  - `crates/jhol-core/src/jgrid/format/perfect_hash.rs`
  - `crates/jhol-core/src/jgrid/format/string_table.rs`
  - `crates/jhol-core/src/jgrid/format/swiss_table.rs`
  - `crates/jhol-core/src/jgrid/format/tree_dag.rs`
- `crates/jhol-core/src/jgrid/ingester/`
  - `crates/jhol-core/src/jgrid/ingester/from_tarball.rs`
  - `crates/jhol-core/src/jgrid/ingester/mod.rs`
  - `crates/jhol-core/src/jgrid/ingester/tree_builder.rs`
- `crates/jhol-core/src/jgrid/materialize/`
  - `crates/jhol-core/src/jgrid/materialize/cloner.rs`
  - `crates/jhol-core/src/jgrid/materialize/installer.rs`
  - `crates/jhol-core/src/jgrid/materialize/mod.rs`
- `crates/jhol-core/src/jgrid/mod.rs`
- `crates/jhol-core/src/jgrid/spec.md`
- `crates/jhol-core/src/jgrid/store/`
  - `crates/jhol-core/src/jgrid/store/builder.rs`
  - `crates/jhol-core/src/jgrid/store/bundle_cache.rs`
  - `crates/jhol-core/src/jgrid/store/gc.rs`
  - `crates/jhol-core/src/jgrid/store/grid_store.rs`
  - `crates/jhol-core/src/jgrid/store/mod.rs`
  - `crates/jhol-core/src/jgrid/store/tree_cache.rs`

##### Subsystem `crates/jhol-core/src/mpt_store/`:
- `crates/jhol-core/src/mpt_store/mod.rs`
- `crates/jhol-core/src/mpt_store/tests.rs`

##### Subsystem `crates/jhol-core/src/provenance_log/`:
- `crates/jhol-core/src/provenance_log/tests.rs`

##### Subsystem `crates/jhol-core/src/pubgrub/`:
- `crates/jhol-core/src/pubgrub/incompatibility.rs`
- `crates/jhol-core/src/pubgrub/minimal.rs`
- `crates/jhol-core/src/pubgrub/mod.rs`
- `crates/jhol-core/src/pubgrub/partial_solution.rs`
- `crates/jhol-core/src/pubgrub/solver.rs`
- `crates/jhol-core/src/pubgrub/term.rs`
- `crates/jhol-core/src/pubgrub/tests.rs`
- `crates/jhol-core/src/pubgrub/version_set.rs`
- `crates/jhol-core/src/pubgrub/vsids.rs`

##### Subsystem `crates/jhol-core/src/task_queue/`:
- `crates/jhol-core/src/task_queue/adaptive.rs`
- `crates/jhol-core/src/task_queue/mod.rs`
- `crates/jhol-core/src/task_queue/parallel.rs`
- `crates/jhol-core/src/task_queue/queue.rs`
- `crates/jhol-core/src/task_queue/worker.rs`


### C. Integration Test Suites & Fixtures (`tests/`)

- `tests/benchmark_suite.rs`
- `tests/cli.rs`
- `tests/compat_matrix.rs`
- `tests/fixtures/`
  - `tests/fixtures/expo-app`
  - `tests/fixtures/express-app`
  - `tests/fixtures/hard-peer-cycles`
  - `tests/fixtures/jagr-instance-bench`
  - `tests/fixtures/nest-app`
  - `tests/fixtures/next-app`
  - `tests/fixtures/nuxt-app`
  - `tests/fixtures/optional-deps-app`
  - `tests/fixtures/overrides-app`
  - `tests/fixtures/peer-conflict`
  - `tests/fixtures/peer-deps-app`
  - `tests/fixtures/react-app`
  - `tests/fixtures/turbo-app`
  - `tests/fixtures/typescript-app`
  - `tests/fixtures/workspace-app`
- `tests/jagr3_grm_integration_test.rs`
- `tests/perf_maps.rs`
- `tests/phase1_4_integration_tests.rs`
- `tests/regression_bun_npm.rs`
- `tests/resolver-snapshots/`
  - `tests/resolver-snapshots/expo-app.json`
  - `tests/resolver-snapshots/express-app.json`
  - `tests/resolver-snapshots/nest-app.json`
  - `tests/resolver-snapshots/next-app.json`
  - `tests/resolver-snapshots/nuxt-app.json`
  - `tests/resolver-snapshots/optional-deps-app.json`
  - `tests/resolver-snapshots/overrides-app.json`
  - `tests/resolver-snapshots/peer-conflict.json`
  - `tests/resolver-snapshots/peer-deps-app.json`
  - `tests/resolver-snapshots/react-app.json`
  - `tests/resolver-snapshots/turbo-app.json`
  - `tests/resolver-snapshots/typescript-app.json`
  - `tests/resolver-snapshots/workspace-app.json`

### D. Documentation (`docs/`, `Documentation/`, Root Docs)

- `AGENTS.md`
- `CHANGELOG.md`
- `CODEBASE_STRUCTURE_TREE.md`
- `CONTRIBUTING.md`
- `GET_STARTED.md`
- `INNOVATION_LOG.md`
- `PRODUCTION_HARDENING_RECORD.md`
- `QWEN.md`
- `README-highdata.md`
- `README.md`
- `RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.pdf`
- `RESULTS.md`
- `ROADMAP.md`
- `SECURITY.md`
- `for-windows.md`
- `slow-to-fast-documentary.md`
- `docs/BOTTLENECK_ANALYSIS.md`
- `docs/JAGR1.md`
- `docs/KNOT_RISING_STAR.md`
- `docs/PHASE_1_4_COMPLETE.md`
- `docs/PRODUCTION_CHECKLIST.md`
- `docs/PRODUCTION_OPTIMIZATION_PLAN.md`
- `docs/README.md`
- `docs/RELEASE_NOTES_1.0.2.md`
- `docs/WEEKLY_EXECUTION_BOARD.md`
- `docs/paper-style.css`
- `docs/phase4-optimization-plan.md`
- `docs/v2-optimization-plan.md`
- `docs/warm-optimization-plan.md`
- `docs/archive/REORG_MANIFEST.json`
- `docs/archive/2025-optimization/4_WEEK_OPTIMIZATION_FINAL_RESULTS.md`
- `docs/archive/2025-optimization/ALL_ISSUES_FIXED_FINAL.md`
- `docs/archive/2025-optimization/BENCHMARK_VALIDATION_REPORT.md`
- `docs/archive/2025-optimization/BEST_IN_CLASS_OPTIMIZATIONS_FINAL.md`
- `docs/archive/2025-optimization/BRUTAL_HONEST_ARCHITECTURAL_ANALYSIS.md`
- `docs/archive/2025-optimization/COMPLETE_FIX_REPORT.md`
- `docs/archive/2025-optimization/COMPLETE_OPTIMIZATION_SUMMARY.md`
- `docs/archive/2025-optimization/COMPLETE_OPTIMIZATION_SUMMARY_FINAL.md`
- `docs/archive/2025-optimization/COMPLETE_VICTORY_REPORT.md`
- `docs/archive/2025-optimization/COMPREHENSIVE_BENCHMARK_ANALYSIS.md`
- `docs/archive/2025-optimization/COMPREHENSIVE_BENCHMARK_REPORT.md`
- `docs/archive/2025-optimization/COMPREHENSIVE_OPTIMIZATION_AUDIT.md`
- `docs/archive/2025-optimization/COMPREHENSIVE_OPTIMIZATION_SUMMARY.md`
- `docs/archive/2025-optimization/COMPREHENSIVE_XXHASH_OPTIMIZATION_COMPLETE.md`
- `docs/archive/2025-optimization/DASHMAP_IMPLEMENTATION_COMPLETE.md`
- `docs/archive/2025-optimization/DEEP_DIVE_CRITICAL_ISSUES.md`
- `docs/archive/2025-optimization/DEEP_DIVE_OPTIMIZATION_ANALYSIS.md`
- `docs/archive/2025-optimization/FINAL_BENCHMARK_ANALYSIS_AND_BOTTLENECKS.md`
- `docs/archive/2025-optimization/FINAL_BENCHMARK_RESULTS_LATEST.md`
- `docs/archive/2025-optimization/FINAL_COMPREHENSIVE_ANALYSIS.md`
- `docs/archive/2025-optimization/FINAL_IMPLEMENTATION_REPORT.md`
- `docs/archive/2025-optimization/FINAL_OPTIMIZATION_REPORT_COMPLETE.md`
- `docs/archive/2025-optimization/FINAL_VICTORY_REPORT.md`
- `docs/archive/2025-optimization/FUNDAMENTAL_CORE_LEVEL_ANALYSIS.md`
- `docs/archive/2025-optimization/FUNDAMENTAL_OPTIMIZATIONS_RESULTS.md`
- `docs/archive/2025-optimization/GLOBAL_CACHE_FINAL_RESULTS.md`
- `docs/archive/2025-optimization/IMPLEMENTATION_COMPLETE.md`
- `docs/archive/2025-optimization/JAGR2_COMPLETE_IMPLEMENTATION.md`
- `docs/archive/2025-optimization/JAGR2_FINAL_RESULTS.md`
- `docs/archive/2025-optimization/JAGR2_IMPLEMENTATION_STATUS.md`
- `docs/archive/2025-optimization/JAGR2_PHASE1_COMPLETE.md`
- `docs/archive/2025-optimization/JAGR3_ALL_OPTIMIZATIONS_COMPLETE.md`
- `docs/archive/2025-optimization/JAGR3_COMPLETE_FINAL_REPORT.md`
- `docs/archive/2025-optimization/JAGR_IMPLEMENTATION_SUMMARY.md`
- `docs/archive/2025-optimization/JHOL_BEATS_BUN_FINAL_VICTORY_REPORT.md`
- `docs/archive/2025-optimization/JHOL_BINARY_REBUILT_FINAL_RESULTS.md`
- `docs/archive/2025-optimization/JHOL_OPTIMIZATION_JOURNEY_SUMMARY.md`
- `docs/archive/2025-optimization/JHOL_TRUE_PERFORMANCE_REVEALED.md`
- `docs/archive/2025-optimization/JHOL_ULTIMATE_OPTIMIZATION_COMPLETE.md`
- `docs/archive/2025-optimization/OPTIMIZATIONS_IMPLEMENTED.md`
- `docs/archive/2025-optimization/OPTIMIZATION_COMPLETE_FINAL_STATUS.md`
- `docs/archive/2025-optimization/OPTIMIZATION_FINAL_REPORT.md`
- `docs/archive/2025-optimization/PERFORMANCE_ANALYSIS_REPORT.md`
- `docs/archive/2025-optimization/PERFORMANCE_DEEP_DIVE.md`
- `docs/archive/2025-optimization/PHASE_1_4_COMPLETION_REPORT.md`
- `docs/archive/2025-optimization/SLOW_INTERNET_ANALYSIS_AND_FIXES.md`
- `docs/archive/2025-optimization/ULTIMATE_VICTORY_REPORT.md`
- `docs/archive/2025-optimization/WHAT_WE_DISCOVERED_AND_WHATS_NEXT.md`
- `docs/archive/2025-optimization/XXHASH_OPTIMIZATION_100_PERCENT_COMPLETE.md`
- `docs/archive/2025-optimization/benchmark-results-all.md`
- `docs/archive/2025-optimization/benchmark-results-medium.md`
- `docs/archive/2025-optimization/benchmark-results-step0.md`
- `docs/archive/2025-optimization/benchmark-results.md`
- `docs/archive/junk/0.5`
- `docs/archive/junk/=`
- `docs/archive/misc/QWEN.md`
- `docs/archive/misc/README-highdata.md`
- `docs/archive/misc/slow-to-fast-documentary.md`
- `docs/archive/reports/2025_RESEARCH_OPTIMIZATION_PLAN.md`
- `docs/archive/reports/4_WEEK_OPTIMIZATION_FINAL_RESULTS.md`
- `docs/archive/reports/ALL_ISSUES_FIXED_FINAL.md`
- `docs/archive/reports/BENCHMARK_VALIDATION_REPORT.md`
- `docs/archive/reports/BEST_IN_CLASS_OPTIMIZATIONS_FINAL.md`
- `docs/archive/reports/BRUTAL_HONEST_ARCHITECTURAL_ANALYSIS.md`
- `docs/archive/reports/COMPLETE_FIX_REPORT.md`
- `docs/archive/reports/COMPLETE_OPTIMIZATION_MASTER_PLAN.md`
- `docs/archive/reports/COMPLETE_OPTIMIZATION_SUMMARY.md`
- `docs/archive/reports/COMPLETE_OPTIMIZATION_SUMMARY_FINAL.md`
- `docs/archive/reports/COMPLETE_VICTORY_REPORT.md`
- `docs/archive/reports/COMPREHENSIVE_BENCHMARK_ANALYSIS.md`
- `docs/archive/reports/COMPREHENSIVE_BENCHMARK_REPORT.md`
- `docs/archive/reports/COMPREHENSIVE_OPTIMIZATION_AUDIT.md`
- `docs/archive/reports/COMPREHENSIVE_OPTIMIZATION_SUMMARY.md`
- `docs/archive/reports/COMPREHENSIVE_XXHASH_OPTIMIZATION_COMPLETE.md`
- `docs/archive/reports/DASHMAP_IMPLEMENTATION_COMPLETE.md`
- `docs/archive/reports/DEEP_DIVE_CRITICAL_ISSUES.md`
- `docs/archive/reports/DEEP_DIVE_OPTIMIZATION_ANALYSIS.md`
- `docs/archive/reports/FINAL_BENCHMARK_ANALYSIS_AND_BOTTLENECKS.md`
- `docs/archive/reports/FINAL_BENCHMARK_RESULTS_LATEST.md`
- `docs/archive/reports/FINAL_COMPREHENSIVE_ANALYSIS.md`
- `docs/archive/reports/FINAL_IMPLEMENTATION_REPORT.md`
- `docs/archive/reports/FINAL_OPTIMIZATION_REPORT_COMPLETE.md`
- `docs/archive/reports/FINAL_VICTORY_REPORT.md`
- `docs/archive/reports/FUNDAMENTAL_CORE_LEVEL_ANALYSIS.md`
- `docs/archive/reports/FUNDAMENTAL_OPTIMIZATIONS_RESULTS.md`
- `docs/archive/reports/GLOBAL_CACHE_FINAL_RESULTS.md`
- `docs/archive/reports/IMPLEMENTATION_COMPLETE.md`
- `docs/archive/reports/JAGR2_COMPLETE_IMPLEMENTATION.md`
- `docs/archive/reports/JAGR2_FINAL_RESULTS.md`
- `docs/archive/reports/JAGR2_IMPLEMENTATION_STATUS.md`
- `docs/archive/reports/JAGR2_PHASE1_COMPLETE.md`
- `docs/archive/reports/JAGR2_STRATEGIC_ROADMAP.md`
- `docs/archive/reports/JAGR3_ALL_OPTIMIZATIONS_COMPLETE.md`
- `docs/archive/reports/JAGR3_COMPLETE_FINAL_REPORT.md`
- `docs/archive/reports/JAGR3_NEXT_GEN_PROPOSAL.md`
- `docs/archive/reports/JAGR3_PURE_MATH_IMPROVEMENTS.md`
- `docs/archive/reports/JAGR_IMPLEMENTATION_SUMMARY.md`
- `docs/archive/reports/JAGR_INFINITE_PLAN.md`
- `docs/archive/reports/JHOL_BEATS_BUN_FINAL_VICTORY_REPORT.md`
- `docs/archive/reports/JHOL_BINARY_REBUILT_FINAL_RESULTS.md`
- `docs/archive/reports/JHOL_OPTIMIZATION_JOURNEY_SUMMARY.md`
- `docs/archive/reports/JHOL_TRUE_PERFORMANCE_REVEALED.md`
- `docs/archive/reports/JHOL_ULTIMATE_OPTIMIZATION_COMPLETE.md`
- `docs/archive/reports/KCC_KNOT_COHOMOLOGY_CLASSIFIER_PLAN.md`
- `docs/archive/reports/OPTIMIZATIONS_IMPLEMENTED.md`
- `docs/archive/reports/OPTIMIZATION_COMPLETE_FINAL_STATUS.md`
- `docs/archive/reports/OPTIMIZATION_FINAL_REPORT.md`
- `docs/archive/reports/PERFORMANCE_ANALYSIS_REPORT.md`
- `docs/archive/reports/PERFORMANCE_DEEP_DIVE.md`
- `docs/archive/reports/PHASE_1_4_COMPLETION_REPORT.md`
- `docs/archive/reports/SLOW_INTERNET_ANALYSIS_AND_FIXES.md`
- `docs/archive/reports/ULTIMATE_OPTIMIZATION_PLAN.md`
- `docs/archive/reports/ULTIMATE_VICTORY_REPORT.md`
- `docs/archive/reports/WHAT_WE_DISCOVERED_AND_WHATS_NEXT.md`
- `docs/archive/reports/XXHASH_OPTIMIZATION_100_PERCENT_COMPLETE.md`
- `docs/archive/reports/benchmark-results-all.md`
- `docs/archive/reports/benchmark-results-medium.md`
- `docs/archive/reports/benchmark-results-step0.md`
- `docs/archive/reports/benchmark-results.md`
- `docs/archive/reports/implementation_plan.md`
- `docs/design/JHOL_CORE_INNOVATIONS.md`
- `docs/engineering/BENCHMARK_IDEAS_APPLIED.md`
- `docs/engineering/BOTTLENECKS_AND_PRODUCTION_READINESS.md`
- `docs/engineering/BOTTLENECKS_JHOL_VS_BUN.md`
- `docs/engineering/JAGR2_SEMANTICS.md`
- `docs/engineering/JAGR_INFINITE_ARCHITECTURE.md`
- `docs/engineering/LOCKFILE_DESIGN.md`
- `docs/engineering/RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.md`
- `docs/engineering/RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.pdf`
- `docs/engineering/RESEARCH_PAPER_HOLOGRAPHIC_DEPENDENCY_RESOLUTION.tex`
- `docs/engineering/paper.html`
- `docs/internal/LOCAL_RECORDS.md`
- `docs/plans/2025_RESEARCH_OPTIMIZATION_PLAN.md`
- `docs/plans/COMPLETE_OPTIMIZATION_MASTER_PLAN.md`
- `docs/plans/JAGR2_STRATEGIC_ROADMAP.md`
- `docs/plans/JAGR3_NEXT_GEN_PROPOSAL.md`
- `docs/plans/JAGR3_PURE_MATH_IMPROVEMENTS.md`
- `docs/plans/JAGR_INFINITE_PLAN.md`
- `docs/plans/KCC_KNOT_COHOMOLOGY_CLASSIFIER_PLAN.md`
- `docs/plans/ROADMAP.md`
- `docs/plans/SMCC_DESIGN.md`
- `docs/plans/ULTIMATE_OPTIMIZATION_PLAN.md`
- `docs/plans/e145-instance-model.md`
- `docs/plans/implementation_plan.md`
- `docs/research/JAGR_INFINITE_RESEARCH_PAPER.md`
- `docs/research/JAGR_INFINITE_RESEARCH_PAPER.pdf`
- `docs/superpowers/plans/2026-05-28-phase-5-completion.md`
- `docs/superpowers/plans/2026-07-08-jhol-runtime-event-loop-and-watcher.md`
- `docs/superpowers/plans/2026-07-08-jhol-runtime-lockfree-state-and-cache.md`
- `docs/superpowers/plans/2026-07-08-jhol-runtime-sparse-tropical-graph.md`
- `docs/superpowers/plans/2026-07-08-jhol-runtime-transpile-native-io-deps.md`
- `docs/superpowers/plans/2026-07-10-jhol-runtime-perf.md`
- `docs/superpowers/plans/2026-07-10-js-engine.md`
- `docs/superpowers/plans/2026-07-14-complete-js-engine.md`
- `docs/superpowers/plans/2026-07-14-jagr-phase1-certified-constraints.md`
- `docs/superpowers/plans/2026-07-14-jagr-phase2-parallel-domain-compiler.md`
- `docs/superpowers/plans/2026-07-14-jagr-quality-preserving-performance.md`
- `docs/superpowers/plans/2026-07-14-project-fingerprint-lockfile-reuse.md`
- `docs/superpowers/plans/2026-07-24-atomic-wavefront-closure.md`
- `docs/superpowers/plans/2026-09-13-jagr-infinite-law-layer.md`
- `docs/superpowers/specs/2026-07-06-jhol-bundle-cas-first-design.md`
- `docs/superpowers/specs/2026-07-08-jhol-runtime-performance-design.md`
- `docs/superpowers/specs/2026-07-14-jagr-quality-preserving-performance-design.md`
- `docs/superpowers/specs/2026-07-14-project-fingerprint-lockfile-reuse-design.md`
- `docs/superpowers/specs/2026-07-16-jagr-native-proof-engine-design.md`
- `docs/superpowers/specs/2026-07-24-atomic-wavefront-closure-design.md`
- `docs/user/SCOPE_AND_STABILITY.md`
- `docs/user/for-windows.md`
- `docs/user/main.md`
- `Documentation/BENCHMARK_IDEAS_APPLIED.md`
- `Documentation/BOTTLENECKS_AND_PRODUCTION_READINESS.md`
- `Documentation/BOTTLENECKS_JHOL_VS_BUN.md`
- `Documentation/JAGR2_SEMANTICS.md`
- `Documentation/LOCKFILE_DESIGN.md`
- `Documentation/README.md`
- `Documentation/SCOPE_AND_STABILITY.md`
- `Documentation/main.md`

### E. Benchmarks & Scripts (`benchmarks/`, `scripts/`, `runtime-benchmarks/`)

- `scripts/FREE_DISK_README.md`
- `scripts/add_binary_from_npm_lock.py`
- `scripts/bench/`
  - `scripts/bench/README.md`
  - `scripts/bench/bench_jhol.py`
  - `scripts/bench/bench_vs_bun.sh`
  - `scripts/bench/benchmark_jagr_comparison.py`
  - `scripts/bench/benchmark_large_workload.py`
  - `scripts/bench/benchmark_repeat.py`
  - `scripts/bench/brutal-bench.sh`
  - `scripts/bench/deep_profiler.py`
  - `scripts/bench/fair_bench.py`
  - `scripts/bench/final_accurate_bench.py`
  - `scripts/bench/professional_bench.py`
  - `scripts/bench/profile_cold_install.py`
  - `scripts/bench/quick_bench.py`
  - `scripts/bench/run_jagr_benchmark.sh`
  - `scripts/bench/scientific_bench.py`
  - `scripts/bench/scientific_jhol_vs_bun.py`
  - `scripts/bench/simple_jagr_benchmark.py`
  - `scripts/bench/ultra_clean_bench.py`
- `scripts/bench_daemon.py`
- `scripts/bench_scales.py`
- `scripts/benchmark.py`
- `scripts/benchmark.sh`
- `scripts/benchmark_jhol_vs_bun.py`
- `scripts/benchmark_offline.py`
- `scripts/benchmark_scientific_v2.py`
- `scripts/build_binary_bundle.py`
- `scripts/build_binary_packages.py`
- `scripts/build_pgo.sh`
- `scripts/build_release_safe.sh`
- `scripts/check_benchmark_regression.py`
- `scripts/check_fallback_trend.py`
- `scripts/check_guardrails.py`
- `scripts/clean_benchmark_artifacts.py`
- `scripts/collect_baseline.py`
- `scripts/compat.sh`
- `scripts/e145/`
  - `scripts/e145/apply.py`
  - `scripts/e145/model_texts.py`
  - `scripts/e145/verify.sh`
- `scripts/enterprise_npmrc_report.py`
- `scripts/expand_binary_package_set.py`
- `scripts/framework_compat_report.py`
- `scripts/free-mac-disk-space.sh`
- `scripts/generate_codebase_tree.py`
- `scripts/generate_paper_pdf.py`
- `scripts/guardrails.sh`
- `scripts/live_bench.py`
- `scripts/live_bench.sh`
- `scripts/live_bench_jhol_vs_bun.py`
- `scripts/mem_guard.py`
- `scripts/performance_dashboard.py`
- `scripts/precompute_unpacked_binary_cache.py`
- `scripts/production_check.sh`
- `scripts/qa_matrix.sh`
- `scripts/reorganize_root.py`
- `scripts/resolver_fixture_report.py`
- `scripts/run_benchmarks.sh`
- `scripts/safe_edit.py`
- `scripts/snapshot_tree.sh`
- `scripts/tui_check.py`
- `scripts/update_binary_profiles.py`