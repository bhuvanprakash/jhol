# Knot v4 — Million-user scale + 4-layer verdict

**Date:** 2026-07-16  
**Status:** Experiment (original tree untouched)  
**Builds on:** Knot v3 (AC-soften, 9/9 parity)

---

## Problem reframed

Going against npm at **~10 lakh (1M) users** is not “make search faster forever.”

Three different caches:

| Cache | What it stores | Who pays if missing |
|-------|----------------|---------------------|
| **Tarball / CAS** | package bytes on disk | download + extract (npm/Bun fight here) |
| **Packument / metadata** | version lists + deps | registry RTT |
| **Resolution capsule** | fingerprint → certified assignment | Knot / OL3P search |

Knot v3 fixed **correctness** on truncated packuments.  
Knot v4 attacks **resolution capsule** warm path — the only cache Knot owns.  
Tarball CAS + GRM remain Jhol’s crown; Knot must not thrash them with unique cold resolves.

---

## Knot v4 upgrades (phenomenal = simple outside)

1. **Resolution capsules** — hash(graph topology + domains + supports) → `{objective, domain}`. Identical manifests across users = **O(1) lookup**.  
2. **Journal undo** — branch without cloning full domain vectors (memory under deep residual).  
3. **Shell-first** — residual empty + feasible → skip search.  
4. **Keep v3 AC-soften** — packument truncation still repaired before search.

Measured (experiment, 9 unique graphs, 100k warm requests):

| Metric | Value |
|--------|-------|
| Hit rate | **1.0** |
| Mean cold | ~0.038 ms |
| Mean warm | ~0.0013 ms |
| Warm vs cold | **~30×** |
| Strict OL3P↔Knot parity | **9/9** (held) |

This is a **stand-in** for many users on popular apps (Express, Next, etc.). Not a claim about install wall-clock vs npm.

---

## How 10 lakh users actually survive

```text
user install
  → local CAS / GRM hit?     # bytes already on machine (Jhol soul)
  → resolution capsule hit?  # Knot v4 / shared mirror
  → shell force-wave         # most packages forced
  → Knot on tiny residual Q  # rare expensive path
  → write capsule + lockfile
```

**Do not** put Layer 2/3 cubic topology on this path.  
**Do** put shared / device-local resolution mirrors (JMR/GRM) so user #500_000 resolving `express@4` does not re-search.

npm wins today on ecosystem + CDN + habit. Jhol wins only if:

1. Cold resolve is exact and small (Knot).  
2. Warm resolve is capsule/GRM (v4+).  
3. Warm install is CAS link/copy (existing Jhol), not re-download.

---

## 4-layer system + Knot — does it help?

**Short answer: Yes — if layers keep their contracts. No — if L2/L3 tax every install.**

| Layer | With Knot v3/v4 | Helps 1M users? |
|-------|-----------------|-----------------|
| **L0 structure** | Separators, work order, capsule/fingerprint inputs | **Yes** — better keys + order |
| **L1 Knot** | Sole exact authority on residual `Q` (replaces OL3P) | **Yes — mandatory** |
| **L2 sheaf / Betti** | Observatory: `jhol jagr`, doctor, history | **Yes for soul/debug**; **No on install hot path** |
| **L3 tropical** | Optional ordering hints only; never authorizes lockfile | **Same as L2** |

### What the 4 layers give that Knot alone does not

- **Identity / uniqueness** vs npm (JAGR story, not “another SAT solver”).  
- **Explainability** (`why`, doctor, Betti history) — retention, not first install speed.  
- **Evidence discipline** — reasons + verifier still Jhol’s differentiator.

### What they must not do at scale

- Full-complex `O(N³)` Betti on every `jhol install`.  
- Tropical matrix work authorizing SAT/optimum.  
- Re-introducing CNF/OL3P as Layer 1 identity.

### Verdict

```text
4-layer + Knot v3/v4  =  HELP
  iff  L1 = Knot (exact, cached)
  and  L2/L3 = observatory / optional hints
  and  GRM/CAS absorb repeated installs
```

If you run “all layers every install,” 10 lakh users will melt you **even with perfect Knot**.  
If you run **shell → Knot → capsule** on install and keep topology for `jagr`/doctor, the 4-layer system is an **advantage** against npm (soul + exactness + cache), not a liability.

---

## Still not done (honest)

1. Lockfile / selection parity vs production `jhol`  
2. Install stage-split vs Bun/npm (resolve share of wall clock)  
3. Persistent on-disk capsules + multi-machine GRM (experiment is in-process HashMap)  
4. Promote to `crates/` only after those gates  

**Knot does not beat npm by itself. Knot + CAS + capsules + layers-as-observatory can.**
