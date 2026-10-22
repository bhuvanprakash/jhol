# JAGR-3 Semantics

This note describes the current dependency semantics implemented by Jhol's primary resolver, `jagr3`.

Jhol's canonical internal model is an instance-aware graph stored in `.jhol/state`, with `.jhol.lock` as the native reproducible lock export. `node_modules` and `package-lock.json` remain compatibility outputs, not the source of truth for successful native installs.

## Core model

- **Root requirement**: A direct requirement from `package.json`, such as `react` in `^18.3.1`.
- **Package instance**: A concrete package placed at a specific install slot, such as `node_modules/react` or `node_modules/send/node_modules/ms`.
- **Instance graph**: The resolved graph of package instances, their dependency edges, and peer visibility context.
- **Peer environment**: The set of ancestors and visible peers that constrain where an instance may be reused versus where a new specialized instance must be created.

## Constraint types

1. **Root / direct**
   Root dependencies come from `package.json` and define the initial instance requirements.

2. **Dependency / transitive**
   When package instance `P@V` depends on `Q` with range `R`, Jhol resolves a visible compatible instance of `Q` or creates a nested instance in the requesting slot's subtree.

3. **Peer dependency / ancestor-visible**
   Peers are resolved against visible ancestors, not by forcing one global version of a package name across the entire tree.
   If a compatible visible peer does not exist, resolution may require a different placement or fail if the peer is mandatory and unsatisfied.

4. **Overrides / resolutions**
   Exact overrides still take precedence over inferred ranges.

## Duplicate versions

Jhol permits multiple versions of the same package name in one graph when the instance paths differ and hoisting/reuse cannot satisfy all constraints with a single visible instance.

Examples:
- `node_modules/ms`
- `node_modules/send/node_modules/ms`
- `node_modules/debug/node_modules/ms`

These are distinct package instances even though they share the same package name.

## Determinism

`jagr3` is deterministic for a fixed registry state and fixed root requirements:
- root frontier is sorted
- dependency frontier is sorted
- peer frontier is sorted
- slot selection order is deterministic

This ensures stable graph fingerprints and reproducible `.jhol/state/graph.json` output.

## Compatibility outputs

- `.jhol/state/graph.json` records the canonical resolved graph.
- `.jhol/state/layout.json` records materialization state for warm reuse.
- `.jhol.lock` records the canonical graph in a stable, commit-friendly format.
- `package-lock.json` is exported from the canonical graph.
- `node_modules` is rendered from the canonical graph for ecosystem compatibility.

Successful graph-backed installs should not need to rediscover dependency structure from `node_modules`.

## Guarantees

- **Instance-aware correctness**: valid duplicate-version trees are representable.
- **Determinism**: equivalent inputs and registry state produce the same graph shape.
- **Compatibility**: lockfile and `node_modules` outputs remain npm-compatible where supported by Jhol's install model.
- **Fallback boundary**: only `pubgrub-v2` remains as a resolver fallback; Bun/npm remain install compatibility fallbacks, not primary resolution paths.

## References

- Package Calculus: A Formal Model of Dependency Resolution (arXiv 2602.18602)
- Solving Package Management via Hypergraph Dependency Resolution (HyperRes, 2025)
- FourierCSP (2025), used only as heuristic inspiration for ranking/pruning
