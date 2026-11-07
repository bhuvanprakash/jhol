//! A compact binary index of the facts the resolver actually reads.
//!
//! ## Why
//!
//! Measured on a real corpus (275 packuments, 6,715 edges): after mask
//! compilation the chart build is ~15 ms, while **gunzip + JSON parse + prune is
//! 80–158 ms**. The resolver therefore spends 85–90% of its time turning gzipped
//! JSON into strings it reads once. The registry cache stores HTTP bodies because
//! that is what HTTP gives us — it is not a format for repeated resolution.
//!
//! This module extracts the resolver's needs *once* — sorted version strings, and
//! the dependency/peer declarations with the set of releases that declare them —
//! and writes them as a flat, length-prefixed little-endian blob. Reading it back
//! is a few allocations and no JSON, no gzip, no `serde_json::Value`.
//!
//! ## Format
//!
//! ```text
//! magic "JHOLJAG1" (8 bytes)
//! u32 package count
//! per package:
//!   str name
//!   u32 version count, then that many str
//!   u32 dep count, then per dep: str target, str spec, u8 optional, u32 idx count, u32 idx...
//!   u32 peer count, then per peer: str target, str spec, u32 idx count, u32 idx...
//! ```
//!
//! `str` = `u32` byte length + UTF-8 bytes. Release indices are offsets from the
//! package's `ordinal_lo` (its oldest version), which is how the chart assigns
//! ordinals, so the declaring-version sets survive the round trip unchanged.
//!
//! Determinism: packages must be encoded in sorted name order and edges in sorted
//! `(target, spec, optional)` order, so the same inputs produce the same bytes.
//! A resolver's cache key must be a function of its inputs, or "same manifest,
//! same tree" stops being true the moment the cache is warm.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// One dependency or peer declaration, with the releases that declare it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEdge {
    pub target: String,
    pub spec: String,
    pub optional: bool,
    /// Offsets from the package's oldest version, ascending.
    pub declaring: Vec<u32>,
}

/// Everything the resolver needs about one package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexPackage {
    pub name: String,
    /// Sorted ascending, as the chart's ordinals assume.
    pub versions: Vec<String>,
    pub deps: Vec<IndexEdge>,
    pub peers: Vec<IndexEdge>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolverIndex {
    pub packages: BTreeMap<String, IndexPackage>,
}

const MAGIC: &[u8; 8] = b"JHOLJAG1";

impl ResolverIndex {
    /// Build from packuments, keeping every declared target (not just the ones
    /// currently fetched): the reachable set is only known after propagation, and
    /// an index that dropped unknown targets would lose edges on a later install.
    pub fn build(packuments: &HashMap<String, serde_json::Value>) -> Self {
        let mut packages = BTreeMap::new();
        for (name, packument) in packuments {
            packages.insert(name.clone(), package_from_packument(name, packument));
        }
        Self { packages }
    }

    /// Chart inputs, in the shape `build_chart_from_constraints` expects.
    ///
    /// Returns `(versions, version_strings, deps, peers)` keyed by package id,
    /// with ids assigned by sorted name so they are a function of the input set.
    /// Chart inputs, in the shape `build_chart_from_constraints` expects.
    ///
    /// The index stores each edge's declaring releases, but the chart builder is
    /// still package-level (one edge per `(target, spec)`), so declaring sets are
    /// not passed through yet. They stay in the index because the per-version
    /// edge work and `why-not` cores both need them.
    #[allow(clippy::type_complexity)]
    pub fn chart_inputs(
        &self,
    ) -> (
        HashMap<String, u32>,
        HashMap<u32, Vec<crate::jagr_infinite::chart::VersionOrdinal>>,
        HashMap<u32, Vec<String>>,
        HashMap<u32, Vec<(u32, String, crate::jagr_infinite::chart::VersionInterval, bool)>>,
        HashMap<u32, Vec<(u32, String, crate::jagr_infinite::chart::VersionInterval)>>,
    ) {
        use crate::jagr_infinite::chart::VersionOrdinal;

        let name_to_id: HashMap<String, u32> = self
            .packages
            .keys()
            .enumerate()
            .map(|(i, n)| (n.clone(), i as u32))
            .collect();

        let mut versions = HashMap::new();
        let mut version_strings = HashMap::new();
        let mut deps = HashMap::new();
        let mut peers = HashMap::new();
        for (name, id) in &name_to_id {
            let Some(pkg) = self.packages.get(name) else {
                continue;
            };
            versions.insert(
                *id,
                (0..pkg.versions.len())
                    .map(|i| VersionOrdinal(i as u16))
                    .collect::<Vec<_>>(),
            );
            version_strings.insert(*id, pkg.versions.clone());
            let d: Vec<_> = pkg
                .deps
                .iter()
                .filter_map(|e| {
                    let target = *name_to_id.get(&e.target)?;
                    let interval = crate::jagr_infinite::precompute::spec_to_interval(&e.spec)?;
                    Some((target, e.spec.clone(), interval, e.optional))
                })
                .collect();
            let p: Vec<_> = pkg
                .peers
                .iter()
                .filter_map(|e| {
                    let target = *name_to_id.get(&e.target)?;
                    let interval = crate::jagr_infinite::precompute::spec_to_interval(&e.spec)?;
                    Some((target, e.spec.clone(), interval))
                })
                .collect();
            deps.insert(*id, d);
            peers.insert(*id, p);
        }
        (name_to_id, versions, version_strings, deps, peers)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 * self.packages.len() + 16);
        out.extend_from_slice(MAGIC);
        put_u32(&mut out, self.packages.len() as u32);
        for pkg in self.packages.values() {
            put_str(&mut out, &pkg.name);
            put_u32(&mut out, pkg.versions.len() as u32);
            for v in &pkg.versions {
                put_str(&mut out, v);
            }
            put_u32(&mut out, pkg.deps.len() as u32);
            for e in &pkg.deps {
                put_edge(&mut out, e);
            }
            put_u32(&mut out, pkg.peers.len() as u32);
            for e in &pkg.peers {
                put_edge(&mut out, e);
            }
        }
        out
    }

    /// Decode a blob produced by [`Self::encode`].
    ///
    /// Every read is bounds-checked and returns `None` on truncation or a bad
    /// magic: a corrupt cache entry must fall back to the packument, never panic
    /// a resolver.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 12 || &bytes[..8] != MAGIC {
            return None;
        }
        let mut r = Reader { b: bytes, pos: 8 };
        let count = r.u32()? as usize;
        let mut packages = BTreeMap::new();
        for _ in 0..count {
            let name = r.string()?;
            let nv = r.u32()? as usize;
            let mut versions = Vec::with_capacity(nv);
            for _ in 0..nv {
                versions.push(r.string()?);
            }
            let nd = r.u32()? as usize;
            let mut deps = Vec::with_capacity(nd);
            for _ in 0..nd {
                deps.push(r.edge()?);
            }
            let np = r.u32()? as usize;
            let mut peers = Vec::with_capacity(np);
            for _ in 0..np {
                peers.push(r.edge()?);
            }
            packages.insert(
                name.clone(),
                IndexPackage {
                    name,
                    versions,
                    deps,
                    peers,
                },
            );
        }
        if r.pos != bytes.len() {
            // Trailing bytes mean a version mismatch or corruption; refuse rather
            // than guess.
            return None;
        }
        Some(Self { packages })
    }
}

fn edge_specs(
    versions: &serde_json::Map<String, serde_json::Value>,
    sections: &[(&str, bool)],
    order: &BTreeMap<String, u32>,
) -> Vec<IndexEdge> {
    // (target, spec, optional) -> declaring offsets
    let mut declared: BTreeMap<(String, String, bool), BTreeSet<u32>> = BTreeMap::new();
    for (vstr, info) in versions {
        let Some(&idx) = order.get(vstr) else { continue };
        for (key, optional) in sections {
            let Some(map) = info.get(key).and_then(|v| v.as_object()) else {
                continue;
            };
            for (target, spec) in map {
                let spec = spec.as_str().unwrap_or("*").trim().to_string();
                declared
                    .entry((target.clone(), spec, *optional))
                    .or_default()
                    .insert(idx);
            }
        }
    }
    // A name declared as both required and optional is required (npm semantics).
    let required_targets: BTreeSet<String> = declared
        .keys()
        .filter(|(_, _, optional)| !*optional)
        .map(|(name, _, _)| name.clone())
        .collect();
    declared
        .into_iter()
        .map(|((target, spec, optional), idxs)| IndexEdge {
            // `required` wins over `optional` for the same target, so resolve the
            // flag before moving the name.
            optional: optional && !required_targets.contains(&target),
            target,
            spec,
            declaring: idxs.into_iter().collect(),
        })
        .collect()
}

pub fn package_from_packument(name: &str, packument: &serde_json::Value) -> IndexPackage {
    let raw = packument
        .get("versions")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    // Sorted semver order defines ordinals; unparsable keys are dropped exactly
    // as the chart builder drops them.
    let mut parsed: Vec<(semver::Version, String)> = raw
        .keys()
        .filter_map(|k| semver::Version::parse(k).ok().map(|v| (v, k.clone())))
        .collect();
    parsed.sort_by(|a, b| a.0.cmp(&b.0));
    let order: BTreeMap<String, u32> = parsed
        .iter()
        .enumerate()
        .map(|(i, (_, k))| (k.clone(), i as u32))
        .collect();
    let versions: Vec<String> = parsed.iter().map(|(v, _)| v.to_string()).collect();

    // Rebuild a version map keyed by the canonical string so indices line up.
    let keyed: serde_json::Map<String, serde_json::Value> = parsed
        .iter()
        .filter_map(|(v, k)| raw.get(k).map(|val| (v.to_string(), val.clone())))
        .collect();

    let mut deps = edge_specs(
        &keyed,
        &[("dependencies", false), ("optionalDependencies", true)],
        &order,
    );
    // required wins over optional for the same target+spec
    let required_targets: BTreeSet<String> = deps
        .iter()
        .filter(|e| !e.optional)
        .map(|e| e.target.clone())
        .collect();
    for e in deps.iter_mut() {
        if e.optional && required_targets.contains(&e.target) {
            e.optional = false;
        }
    }
    let peers = edge_specs(&keyed, &[("peerDependencies", false)], &order);

    IndexPackage {
        name: name.to_string(),
        versions,
        deps,
        peers,
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

fn put_edge(out: &mut Vec<u8>, e: &IndexEdge) {
    put_str(out, &e.target);
    put_str(out, &e.spec);
    out.push(u8::from(e.optional));
    put_u32(out, e.declaring.len() as u32);
    for i in &e.declaring {
        put_u32(out, *i);
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Option<u32> {
        let end = self.pos.checked_add(4)?;
        let slice = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(u32::from_le_bytes(slice.try_into().ok()?))
    }

    fn string(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let end = self.pos.checked_add(len)?;
        let slice = self.b.get(self.pos..end)?;
        self.pos = end;
        String::from_utf8(slice.to_vec()).ok()
    }

    fn edge(&mut self) -> Option<IndexEdge> {
        let target = self.string()?;
        let spec = self.string()?;
        let byte = *self.b.get(self.pos)?;
        self.pos += 1;
        let n = self.u32()? as usize;
        let mut declaring = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            declaring.push(self.u32()?);
        }
        Some(IndexEdge {
            target,
            spec,
            optional: byte != 0,
            declaring,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn packument() -> serde_json::Value {
        json!({
            "name": "a",
            "versions": {
                "2.0.0": { "dependencies": { "b": "^2" }, "peerDependencies": { "c": ">=1" } },
                "1.0.0": { "dependencies": { "b": "^1", "d": "~1.2.0" } },
                "1.1.0": { "dependencies": { "b": "^1" }, "optionalDependencies": { "e": "^3" } }
            }
        })
    }

    #[test]
    fn round_trip_is_exact() {
        let idx = ResolverIndex::build(&HashMap::from([("a".to_string(), packument())]));
        let bytes = idx.encode();
        let back = ResolverIndex::decode(&bytes).expect("decodes");
        assert_eq!(idx, back);
        // Deterministic bytes for identical inputs.
        assert_eq!(bytes, ResolverIndex::build(&HashMap::from([("a".to_string(), packument())])).encode());
    }

    #[test]
    fn extraction_matches_chart_semantics() {
        let pkg = package_from_packument("a", &packument());
        // Ordinals are sorted semver: 1.0.0, 1.1.0, 2.0.0.
        assert_eq!(pkg.versions, vec!["1.0.0", "1.1.0", "2.0.0"]);
        let b1 = pkg
            .deps
            .iter()
            .find(|e| e.target == "b" && e.spec == "^1")
            .expect("^1 edge");
        assert_eq!(b1.declaring, vec![0, 1], "declared by 1.0.0 and 1.1.0");
        let b2 = pkg
            .deps
            .iter()
            .find(|e| e.target == "b" && e.spec == "^2")
            .expect("^2 edge");
        assert_eq!(b2.declaring, vec![2], "declared only by 2.0.0");
        assert!(pkg
            .peers
            .iter()
            .any(|e| e.target == "c" && e.spec == ">=1" && e.declaring == vec![2]));
    }

    #[test]
    fn truncated_or_foreign_blobs_are_rejected_not_panicked() {
        let idx = ResolverIndex::build(&HashMap::from([("a".to_string(), packument())]));
        let bytes = idx.encode();
        for cut in [0, 4, 8, 12, bytes.len() / 2, bytes.len() - 1] {
            assert!(ResolverIndex::decode(&bytes[..cut]).is_none(), "cut {cut}");
        }
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        assert!(ResolverIndex::decode(&wrong).is_none());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(ResolverIndex::decode(&trailing).is_none());
    }

    /// The point of the index: the chart built from it must equal the chart built
    /// from JSON, edge for edge, so speed cannot change the answer.
    #[test]
    fn chart_from_index_equals_chart_from_packuments() {
        let packuments = HashMap::from([
            ("a".to_string(), packument()),
            (
                "b".to_string(),
                json!({ "name": "b", "versions": { "1.0.0": {}, "2.0.0": {} } }),
            ),
            ("c".to_string(), json!({ "name": "c", "versions": { "1.0.0": {} } })),
            ("d".to_string(), json!({ "name": "d", "versions": { "1.2.0": {} } })),
            ("e".to_string(), json!({ "name": "e", "versions": { "3.0.0": {} } })),
        ]);
        let mut names: Vec<&String> = packuments.keys().collect();
        names.sort();
        let name_to_id: HashMap<String, u32> = names
            .iter()
            .enumerate()
            .map(|(i, n)| ((*n).clone(), i as u32))
            .collect();

        let from_json =
            crate::jagr_infinite::precompute::build_full_chart(&packuments, &name_to_id);

        let idx = ResolverIndex::build(&packuments);
        let (idx_names, versions, version_strings, deps, peers) = idx.chart_inputs();
        assert_eq!(idx_names, name_to_id);
        let from_index = crate::jagr_infinite::atlas::build_chart_from_constraints(
            &(0..names.len() as u32)
                .map(|id| (id, crate::jagr_infinite::chart::VersionInterval::full()))
                .collect(),
            &versions,
            &version_strings,
            &deps,
            &peers,
        );

        assert_eq!(from_json.packages.len(), from_index.packages.len());
        for (id, a) in &from_json.packages {
            let b = from_index.packages.get(id).expect("same ids");
            assert_eq!(a.versions, b.versions, "package {id} versions differ");
            assert_eq!(
                a.allowed.words, b.allowed.words,
                "package {id} domain differs"
            );
            assert_eq!(a.deps.len(), b.deps.len(), "package {id} edge count");
            for (ea, eb) in a.deps.iter().zip(b.deps.iter()) {
                assert_eq!(ea.target_pkg, eb.target_pkg);
                assert_eq!(ea.spec, eb.spec);
                assert_eq!(ea.optional, eb.optional);
                assert_eq!(
                    ea.required_mask.as_ref().map(|m| &m.words),
                    eb.required_mask.as_ref().map(|m| &m.words),
                    "package {id} mask for {} differs",
                    ea.spec
                );
            }
        }
    }
}
