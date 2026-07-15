# JAGR Certified Lazy Refinement Implementation Plan

> Execute in the live workspace because `crates/jhol-core/src/jagr/` is untracked and is not available in a Git worktree. Preserve all unrelated user changes.

**Goal:** Implement approved certified lazy JAGR Phases 1–6 without changing JAGR's name, four-layer identity, or resolution quality.

**Architecture:** Preserve current certified resolver as `off` baseline. Add monotone metadata knowledge, optimistic/certified objective bounds, persistent refinement state, obstruction-quotient features, and deterministic information-gain hydration. Candidate runs in shadow until parity and performance gates permit promotion.

**Tech Stack:** Rust 2021, serde JSON, Rayon, existing JAGR MaxSAT/tropical/sheaf modules, Cargo unit tests and release benchmarks. Inline execution only; user prohibited agents.

---

### Phase 1: Baseline, mode, telemetry, fallback

**Files:** `crates/jhol-core/src/lockfile_write.rs`

1. Write failing tests for `off`, `shadow`, `on`, invalid-value fallback, and telemetry counters.
2. Add `JagrRefinementMode`, `JagrRefinementTelemetry`, deterministic comparison, and automatic baseline fallback.
3. Preserve current binary and release benchmark output before candidate promotion.

### Phase 2: Monotone knowledge lattice

**Files:** `crates/jhol-core/src/lockfile_write.rs`, `crates/jhol-core/src/registry.rs`

1. Write failing transition tests for `Unknown < VersionKnown < ManifestKnown < ConstraintCertified`, including rejected backward transitions.
2. Build domains from abbreviated packuments.
3. Add bounded parallel specific-version manifest hydration using existing HTTP client/cache.
4. Never encode unknown manifests as dependency-free authoritative data.

### Phase 3: Dual-bound closure certificate

**Files:** `crates/jhol-core/src/lockfile_write.rs`

1. Write failing tests proving feasible-but-suboptimal assignments do not close and equal bounds do close.
2. Track optimistic lower bound and certified upper bound with exact integer preference ranks.
3. Require bound equality plus existing assignment certificate before success.
4. Require grounded authoritative constraints before final UNSAT.

### Phase 4: Persistent exact refinement

**Files:** `crates/jhol-core/src/jagr/core_guided_solver.rs`, `crates/jhol-core/src/jagr/ol3p_solver.rs`, `crates/jhol-core/src/lockfile_write.rs`

1. Write failing tests that add hard constraints between solve rounds and preserve lower bounds/formula state.
2. Add an incremental session API with monotonic clause addition and generation counters.
3. Reuse safe formula, relaxation, and bound state; reset trail state when clause addition invalidates it.
4. Keep deterministic results identical to one-shot solving.

### Phase 5: Four-layer obstruction quotient

**Files:** `crates/jhol-core/src/jagr/jagr_resolver.rs`, `crates/jhol-core/src/jagr/tropical_sheaf.rs`, `crates/jhol-core/src/lockfile_write.rs`

1. Write failing tests for deterministic spectral influence, tropical slack, and obstruction support.
2. Collapse certified vertices from unresolved frontier.
3. Recompute features only for affected components.
4. Keep all feature outputs advisory.

### Phase 6: Information-gain scheduler

**Files:** `crates/jhol-core/src/lockfile_write.rs`

1. Write failing ordering/tie tests for deterministic byte-aware hydration score.
2. Implement lazy heap reevaluation and bounded batch selection.
3. Integrate refinement loop: optimistic solve, select frontier, hydrate, add constraints, certify, repeat.
4. Worst-case fallback calls current authoritative full resolver.

### Verification and rollback gate

1. Format only touched files because workspace contains unrelated/deleted benchmark files.
2. Run focused red-green tests, full JAGR library tests, full core library tests, release build, parity fixtures, and before/after/Bun scenarios.
3. Reject any correctness difference. Revert phase for repeated median regression above 3% or P95 above 5%.
4. Keep `off` authoritative unless `on` passes all gates; shadow mismatch always falls back.
