# JAGR-∞ Implementation Plan

## Architecture: New Resolver Module (No Touch to Existing)

```
crates/jhol-core/src/
├── jagr_infinite/
│   ├── mod.rs                    # Public API: resolve_full_tree_jagr_infinite
│   ├── atlas.rs                  # Global Sheaf Atlas (Merkle DAG of charts)
│   ├── knot_invariants.rs        # Alexander/Jones polynomial + strategy map
│   ├── homotopy.rs               # Tropical homotopy continuation
│   ├── rvm/
│   │   ├── mod.rs                # RVM bytecode format + interpreter
│   │   ├── inst.rs               # Instruction set
│   │   ├── compile.rs            # package.json → bytecode compiler
│   │   └── exec.rs               # Interpreter + JIT stub
│   ├── chart.rs                  # Chart structure + transition maps
│   ├── precompute.rs             # Registry → Atlas offline build
│   └── persistence.rs            # .jhol-atlas format (Merkle DAG)
└── lockfile_write.rs             # ADD: "jagr-infinite" match arm only
```

## Wiring (Minimal Touch)

**lockfile_write.rs:305-335** - Add one match arm:
```rust
"jagr-infinite" => resolve_full_tree_jagr_infinite(package_json_path).or_else(|e| {
    if strict_resolver { return Err(e); }
    // fallback chain same as jagr3
})
```

**lib.rs** - Re-export: `pub use jagr_infinite::resolve_full_tree_jagr_infinite;`

## Milestone M1: Sheaf Atlas + Knot Invariants (Core)

### Files to Create (in order)

1. **`jagr_infinite/chart.rs`** - Chart = package set × version intervals + trivial cohomology
   - `Chart { id: u64, packages: Vec<ChartPackage>, transitions: HashMap<u64, TransitionMap>, betti: BettiNumbers }`
   - `TransitionMap { from_chart: u64, to_chart: u64, compatibility: CompatibilityFn }`
   - SIMD interval operations

2. **`jagr_infinite/atlas.rs`** - Global Sheaf Atlas (Merkle DAG)
   - `Atlas { charts: HashMap<u64, Chart>, merkle_root: Hash, version: u64 }`
   - `lookup(constraint_set) -> Chart` via semantic hash
   - `stitch(charts) -> Resolution` with exactness proof

3. **`jagr_infinite/knot_invariants.rs`** - Alexander/Jones polynomial
   - Build link diagram from SCC dependency cycle
   - `alexander_polynomial(link) -> Poly`, `jones_polynomial(link) -> Poly`
   - `classify_knot(invariants) -> Strategy { Greedy | MaxSAT2Var | Portfolio }`

4. **`jagr_infinite/precompute.rs`** - Registry → Atlas offline
   - Parallel chart construction from registry packuments
   - Merkle DAG serialization to `.jhol-atlas`
   - Incremental update on registry snapshot

5. **`jagr_infinite/mod.rs`** - Public API
   - `pub fn resolve_full_tree_jagr_infinite(path: &Path) -> Result<HashMap<String, ResolvedPackage>, String>`
   - Wire: atlas lookup → chart refinement → verify_deps → stitch

## Milestone M2: Tropical Homotopy + RVM Core

6. **`jagr_infinite/homotopy.rs`** - Lockfile → resolution continuation
   - Tropical variety from lockfile + constraints
   - Path following with tropical gradient descent
   - Singularity detection via Betti number change

7. **`jagr_infinite/rvm/inst.rs`** - Instruction set enum (typed, exhaustive)
7. **`jagr_infinite/rvm/compile.rs`** - package.json + atlas → bytecode
7. **`jagr_infinite/rvm/exec.rs`** - Interpreter + verify_deps gate

## Milestone M3: JIT + LCC Persistence

8. **`jagr_infinite/rvm/jit.rs`** - Cranelift JIT for hot paths
9. **`jagr_infinite/persistence.rs`** - Topological LCC (persistence diagram distance)
10. **`jagr_infinite/atlas_sync.rs`** - Merkle atlas incremental sync

## Integration Points (Existing Code Used)

| JAGR-∞ Component | Uses Existing |
|---|---|
| Chart intervals | `lattice_cache.rs` (semver ordinals) |
| Knot invariants | `jagr/knot.rs` (OL3P/OL3PS structures) |
| Tropical ops | `jagr/tropical_math.rs`, `tropical_sheaf.rs` |
| Sheaf cohomology | `jagr/tropical_sheaf.rs`, `simplicial_normal_form.rs` |
| MaxSAT | `jagr/jagr_resolver.rs` Layer 1 (OL3P/OL3PS) |
| Verify deps | `jagr/jagr_resolver.rs` verify_deps |
| Betti/SNF | `jagr/simplicial_normal_form.rs` |
| Registry fetch | `registry.rs` fetch_metadata* |
| Binary format | `binary_manifest.rs`, `.jhol` encode/decode |
| GRM | `grm/mod.rs`, `lock_free_grm.rs` |

## Quality Gates (Never Lose)

- Every fast path gated by `verify_deps` (exact)
- Sheaf exactness = mathematical global consistency
- Fallback chain identical to jagr3
- All existing tests must pass

## CLI Flag

`--resolver jagr-infinite` (adds to clap enum in main.rs)
`JHOL_RESOLVER=jagr-infinite` env var

---

## Start Order

1. Create `crates/jhol-core/src/jagr_infinite/` directory
2. `chart.rs` - interval algebra + chart structure
3. `knot_invariants.rs` - Alexander/Jones pure math
4. `atlas.rs` - Merkle DAG + lookup
5. `precompute.rs` - offline build
5. `mod.rs` - public resolve function
6. Wire in `lockfile_write.rs` + `lib.rs` + `main.rs` (clap)
7. Test: `JHOL_RESOLVER=jagr-infinite cargo test --test cli`