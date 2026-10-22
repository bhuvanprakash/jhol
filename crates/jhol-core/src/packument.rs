//! E60 — typed packument record.
//!
//! `serde_json::Value` costs one enum slot plus a heap allocation per JSON node
//! (measured on the resolution fixture: 22.4 MB of packument bytes → a 257.8 MB
//! `Value` tree, 11.5x; E60's measured ceiling for the typed shape is 3.2x). The
//! JAGR-∞ resolution path therefore parses the registry bytes straight into
//! [`PackumentRecord`] and never materialises a `Value` tree.
//!
//! Consumers outside that path (`install.rs`, `doctor.rs`, `prefetch`,
//! `package_index`, the legacy `jagr3` / `pubgrub_v2` solvers) keep the
//! `serde_json::Value` API in [`crate::registry`] unchanged, so no feature is
//! gated behind the migration.
//!
//! # Field contract
//!
//! The record carries exactly the fields the migrated consumers read (mapping
//! table in `docs/engineering/PACKUMENT_RECORD.md`). Unknown keys are ignored —
//! that is where the memory win comes from. Every field is `#[serde(default)]`
//! and every custom type has a tolerant deserializer, so a packument that omits
//! or mistypes a field cannot make the parse fail where the `Value` path
//! succeeded: a failed parse would drop a package from the graph, which is
//! exactly the lost-functionality failure this round must not have.

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

/// A dependency spec string. `None` means the JSON value was *not* a string —
/// the same thing `Value::as_str()` yields. Consumers keep their original
/// handling: [`spec_or_star`] for the `unwrap_or("*")` sites, and skipping the
/// entry where the old code did `if let Some(s) = spec.as_str()`.
pub type Spec = Option<String>;

/// `dependencies` / `optionalDependencies` / `peerDependencies` map.
pub type SpecMap = BTreeMap<String, Spec>;

/// The old `spec.as_str().unwrap_or("*")`.
pub fn spec_or_star(spec: &Spec) -> &str {
    spec.as_deref().unwrap_or("*")
}

/// `dist` — only the fields the resolution path reads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DistRecord {
    #[serde(deserialize_with = "de_spec")]
    pub tarball: Spec,
    #[serde(deserialize_with = "de_spec")]
    pub integrity: Spec,
    #[serde(deserialize_with = "de_spec")]
    pub shasum: Spec,
}

/// One published version's metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VersionRecord {
    #[serde(deserialize_with = "de_spec")]
    pub name: Spec,
    #[serde(deserialize_with = "de_spec")]
    pub version: Spec,
    #[serde(deserialize_with = "de_spec_map")]
    pub dependencies: SpecMap,
    #[serde(rename = "optionalDependencies", deserialize_with = "de_spec_map")]
    pub optional_dependencies: SpecMap,
    #[serde(rename = "peerDependencies", deserialize_with = "de_spec_map")]
    pub peer_dependencies: SpecMap,
    /// Kept as raw JSON: the lockfile copies `peerDependenciesMeta` verbatim
    /// (`selection_to_resolved` clones each value), and the objects are tiny.
    #[serde(rename = "peerDependenciesMeta", deserialize_with = "de_opt_value_map")]
    pub peer_dependencies_meta: BTreeMap<String, serde_json::Value>,
    #[serde(deserialize_with = "de_opt_dist")]
    pub dist: Option<DistRecord>,
}

impl VersionRecord {
    /// `dependencies` then `optionalDependencies`, in the map order the old
    /// `for key in ["dependencies", "optionalDependencies"]` loop used.
    pub fn dep_specs(&self) -> impl Iterator<Item = (&String, &Spec)> {
        self.dependencies
            .iter()
            .chain(self.optional_dependencies.iter())
    }

    pub fn peer_specs(&self) -> impl Iterator<Item = (&String, &Spec)> {
        self.peer_dependencies.iter()
    }

    /// `peerDependenciesMeta[peer].optional`, defaulting to false.
    pub fn peer_optional(&self, peer: &str) -> bool {
        self.peer_dependencies_meta
            .get(peer)
            .and_then(|m| m.get("optional"))
            .and_then(|o| o.as_bool())
            .unwrap_or(false)
    }
}

/// A whole packument.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PackumentRecord {
    #[allow(dead_code)]
    #[serde(deserialize_with = "de_spec")]
    name: Spec,
    #[serde(rename = "dist-tags", deserialize_with = "de_opt_value_map")]
    dist_tags: BTreeMap<String, serde_json::Value>,
    /// `None` when the JSON had no usable `versions` object (`Value` path:
    /// `.get("versions").and_then(as_object)` yielded `None`).
    #[serde(deserialize_with = "de_opt_versions")]
    versions: Option<BTreeMap<String, VersionRecord>>,
    /// Marker fields: present-or-not is all the old `Value::get(..).is_some()`
    /// check looked at, so any JSON value (including `null`) counts.
    #[serde(rename = "_vortex_manifest_synth", deserialize_with = "de_marker")]
    vortex_manifest_synth: bool,
    #[serde(rename = "_bundled_index_synth", deserialize_with = "de_marker")]
    bundled_index_synth: bool,
}

fn empty_versions() -> &'static BTreeMap<String, VersionRecord> {
    static EMPTY: std::sync::OnceLock<BTreeMap<String, VersionRecord>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeMap::new)
}

impl PackumentRecord {
    /// The published versions (empty when the JSON had no `versions` object).
    pub fn versions(&self) -> &BTreeMap<String, VersionRecord> {
        match self.versions {
            Some(ref v) => v,
            None => empty_versions(),
        }
    }

    pub fn version_count(&self) -> usize {
        self.versions().len()
    }

    pub fn version(&self, version: &str) -> Option<&VersionRecord> {
        self.versions().get(version)
    }

    /// The old `has_versions` test: `versions` present-and-non-empty, or the key
    /// absent entirely (the historical `.unwrap_or(true)` for non-packuments).
    pub fn has_versions(&self) -> bool {
        self.versions
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(true)
    }

    pub fn is_synth(&self) -> bool {
        self.vortex_manifest_synth || self.bundled_index_synth
    }

    pub fn latest_tag(&self) -> Option<&str> {
        self.dist_tags.get("latest").and_then(|v| v.as_str())
    }

    /// Highest published version satisfying **every** constraint (the old
    /// `synth_satisfies_all`).
    pub fn satisfies_all(&self, constraints: &[String]) -> bool {
        if constraints.is_empty() {
            return false;
        }
        self.versions().keys().any(|v| {
            constraints
                .iter()
                .all(|c| crate::registry::version_satisfies(c, v))
        })
    }

    /// Highest published version satisfying one spec.
    pub fn highest_satisfying(&self, spec: &str) -> Option<String> {
        self.versions()
            .keys()
            .filter(|v| crate::registry::version_satisfies(spec, v))
            .max_by(|a, b| {
                semver::Version::parse(a)
                    .ok()
                    .zip(semver::Version::parse(b).ok())
                    .map(|(x, y)| x.cmp(&y))
                    .unwrap_or_else(|| a.cmp(b))
            })
            .cloned()
    }

    /// Newest published version by semver.
    ///
    /// Non-semver keys (npm metadata does carry junk tags in `versions`) are
    /// skipped whenever at least one key parses, because a mixed semver/string
    /// comparator is not a total order: `max_by` with one settled on
    /// `"not-semver"` for `["1.0.0","1.5.0","2.0.0","not-semver"]`, which would
    /// hand the resolver a version that cannot be fetched. When *nothing*
    /// parses the keys are compared textually, which is the behaviour of the
    /// `newest` closure in `jagr_infinite::collect_dep_names_for` for the
    /// inputs it actually sees (it is only reached with `constraints.is_empty()`
    /// and returns a version used to expand the dependency tree).
    pub fn newest_version(&self) -> Option<String> {
        let parsed = self
            .versions()
            .keys()
            .filter_map(|k| semver::Version::parse(k).ok().map(|v| (v, k.clone())))
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, k)| k);
        parsed.or_else(|| self.versions().keys().max().cloned())
    }

    /// The old `prune_packument_versions` version filter. `time` is not carried
    /// by the record (nothing on the resolution path reads it), so only the
    /// version map is retained.
    pub fn retain_versions(&mut self, keep: &HashSet<String>) {
        if let Some(versions) = self.versions.as_mut() {
            versions.retain(|key, _| keep.contains(key));
        }
    }

    /// Parse a small synthesized packument (bundled-index / manifest synth).
    /// Never used to rebuild a full packument — that is the E67 trap.
    pub fn from_value(value: serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }
}

// ── tolerant deserializers ──────────────────────────────────────────────────
// Each accepts any JSON shape and degrades to the same value the `Value` path
// would have produced, instead of failing the whole packument.

fn de_spec<'de, D: Deserializer<'de>>(d: D) -> Result<Spec, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Spec;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a registry spec string")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Spec, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<Spec, E> {
            Ok(Some(v))
        }
        fn visit_none<E: de::Error>(self) -> Result<Spec, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Spec, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Spec, D2::Error> {
            de_spec(d)
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Spec, E> {
            Ok(None)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Spec, E> {
            Ok(None)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Spec, E> {
            Ok(None)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Spec, E> {
            Ok(None)
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut s: A) -> Result<Spec, A::Error> {
            while s.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut m: A) -> Result<Spec, A::Error> {
            while m.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(None)
        }
    }
    d.deserialize_any(V)
}

/// Seed so a tolerant [`Spec`] can be pulled out of a `MapAccess`.
struct SpecSeed;

impl<'de> de::DeserializeSeed<'de> for SpecSeed {
    type Value = Spec;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Spec, D::Error> {
        de_spec(d)
    }
}

fn de_spec_map<'de, D: Deserializer<'de>>(d: D) -> Result<SpecMap, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = SpecMap;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a name -> spec object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<SpecMap, A::Error> {
            let mut out = SpecMap::new();
            while let Some(k) = m.next_key::<String>()? {
                out.insert(k, m.next_value_seed(SpecSeed)?);
            }
            Ok(out)
        }
        fn visit_unit<E: de::Error>(self) -> Result<SpecMap, E> {
            Ok(SpecMap::new())
        }
        fn visit_none<E: de::Error>(self) -> Result<SpecMap, E> {
            Ok(SpecMap::new())
        }
    }
    d.deserialize_any(V)
}

fn de_opt_value_map<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = BTreeMap<String, serde_json::Value>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<A: MapAccess<'de>>(
            self,
            mut m: A,
        ) -> Result<BTreeMap<String, serde_json::Value>, A::Error> {
            let mut out = BTreeMap::new();
            while let Some(k) = m.next_key::<String>()? {
                out.insert(k, m.next_value::<serde_json::Value>()?);
            }
            Ok(out)
        }
        fn visit_unit<E: de::Error>(self) -> Result<BTreeMap<String, serde_json::Value>, E> {
            Ok(BTreeMap::new())
        }
        fn visit_none<E: de::Error>(self) -> Result<BTreeMap<String, serde_json::Value>, E> {
            Ok(BTreeMap::new())
        }
    }
    d.deserialize_any(V)
}

fn de_opt_dist<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DistRecord>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<DistRecord>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a dist object or null")
        }
        fn visit_map<A: MapAccess<'de>>(self, m: A) -> Result<Option<DistRecord>, A::Error> {
            DistRecord::deserialize(de::value::MapAccessDeserializer::new(m)).map(Some)
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<DistRecord>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Option<DistRecord>, E> {
            Ok(None)
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<Option<DistRecord>, E> {
            Ok(None)
        }
        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut s: A,
        ) -> Result<Option<DistRecord>, A::Error> {
            while s.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Option<DistRecord>, E> {
            Ok(None)
        }
    }
    d.deserialize_any(V)
}

fn de_opt_versions<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<BTreeMap<String, VersionRecord>>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<BTreeMap<String, VersionRecord>>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a versions object or null")
        }
        fn visit_map<A: MapAccess<'de>>(
            self,
            mut m: A,
        ) -> Result<Option<BTreeMap<String, VersionRecord>>, A::Error> {
            let mut out = BTreeMap::new();
            while let Some(k) = m.next_key::<String>()? {
                out.insert(k, m.next_value::<VersionRecord>()?);
            }
            Ok(Some(out))
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<BTreeMap<String, VersionRecord>>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Option<BTreeMap<String, VersionRecord>>, E> {
            Ok(None)
        }
        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut s: A,
        ) -> Result<Option<BTreeMap<String, VersionRecord>>, A::Error> {
            while s.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }
        fn visit_str<E: de::Error>(
            self,
            _: &str,
        ) -> Result<Option<BTreeMap<String, VersionRecord>>, E> {
            Ok(None)
        }
    }
    d.deserialize_any(V)
}

/// Present-or-not marker: any JSON shape (including `null`) sets it.
fn de_marker<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = bool;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("any JSON value")
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<bool, E> {
            Ok(true)
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<bool, E> {
            Ok(true)
        }
        fn visit_unit<E: de::Error>(self) -> Result<bool, E> {
            Ok(true)
        }
        fn visit_none<E: de::Error>(self) -> Result<bool, E> {
            Ok(true)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<bool, E> {
            Ok(true)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<bool, E> {
            Ok(true)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<bool, E> {
            Ok(true)
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut s: A) -> Result<bool, A::Error> {
            while s.next_element::<IgnoredAny>()?.is_some() {}
            Ok(true)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<bool, A::Error> {
            while m.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(true)
        }
    }
    d.deserialize_any(V)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(json: &str) -> PackumentRecord {
        PackumentRecord::from_slice(json.as_bytes()).expect("record parse")
    }

    #[test]
    fn absent_versions_object_means_no_versions_and_non_empty_default() {
        let absent = rec(r#"{"name":"a","dist-tags":{"latest":"1.0.0"}}"#);
        assert!(absent.versions().is_empty());
        // The old `has_versions` used `.unwrap_or(true)`: a response with no
        // `versions` key at all counted as usable.
        assert!(absent.has_versions());
        let empty = rec(r#"{"name":"a","versions":{}}"#);
        assert!(!empty.has_versions());
        let not_an_object = rec(r#"{"name":"a","versions":[]}"#);
        assert!(not_an_object.versions().is_empty());
        assert!(not_an_object.has_versions());
    }

    #[test]
    fn non_string_specs_are_none_not_a_parse_failure() {
        let r = rec(r#"{"versions":{"1.0.0":{
                "dependencies":{"ok":"^1.0.0","num":3,"obj":{"a":1},"nul":null},
                "peerDependencies":{"host":true},
                "peerDependenciesMeta":{"host":{"optional":true}},
                "dist":{"integrity":"sha512-x","tarball":7}
            }}}"#);
        let v = r.version("1.0.0").expect("version");
        assert_eq!(v.dependencies.get("ok"), Some(&Some("^1.0.0".to_string())));
        assert_eq!(v.dependencies.get("num"), Some(&None));
        assert_eq!(v.dependencies.get("obj"), Some(&None));
        assert_eq!(v.dependencies.get("nul"), Some(&None));
        assert_eq!(spec_or_star(&v.dependencies["num"]), "*");
        // `selection_to_resolved` skipped non-string peers; `collect_dep_names_for`
        // used "*". The record preserves the distinction for both.
        assert_eq!(v.peer_dependencies.get("host"), Some(&None));
        assert!(v.peer_optional("host"));
        assert_eq!(
            v.dist.as_ref().and_then(|d| d.integrity.clone()),
            Some("sha512-x".into())
        );
        assert_eq!(v.dist.as_ref().and_then(|d| d.tarball.clone()), None);
    }

    #[test]
    fn synth_markers_and_round_trip() {
        let r = rec(r#"{"versions":{"1.0.0":{}},"_vortex_manifest_synth":true}"#);
        assert!(r.is_synth());
        let json = serde_json::to_vec(&r).expect("serialize");
        let back = PackumentRecord::from_slice(&json).expect("round trip");
        assert!(back.is_synth());
        assert_eq!(back.version_count(), 1);
        assert!(!rec(r#"{"versions":{"1.0.0":{}}}"#).is_synth());
        assert!(rec(r#"{"_bundled_index_synth":null}"#).is_synth());
    }

    #[test]
    fn satisfies_all_and_highest_satisfying_match_the_value_rules() {
        let r = rec(r#"{"versions":{"1.0.0":{},"1.5.0":{},"2.0.0":{},"not-semver":{}}}"#);
        assert!(r.satisfies_all(&["^1.0.0".to_string()]));
        assert!(!r.satisfies_all(&["^3.0.0".to_string()]));
        assert!(!r.satisfies_all(&[]));
        assert_eq!(r.highest_satisfying("^1.0.0"), Some("1.5.0".to_string()));
        assert_eq!(r.newest_version(), Some("2.0.0".to_string()));
    }

    #[test]
    fn retain_versions_keeps_only_the_named_keys() {
        let mut r = rec(r#"{"versions":{"1.0.0":{},"2.0.0":{}}}"#);
        let keep: HashSet<String> = ["2.0.0".to_string()].into_iter().collect();
        r.retain_versions(&keep);
        assert_eq!(r.version_count(), 1);
        assert!(r.version("2.0.0").is_some());
    }
}
