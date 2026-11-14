//! Node-keyed install layout: hoisting and nesting.
//!
//! The resolver produces a *flat* resolution keyed by package name (one version
//! per name). npm permits two majors of the same package side by side; that can
//! only be expressed in a node-keyed tree:
//!
//! ```text
//! node_modules/<name>                         <- hoisted (top-level) copy
//! node_modules/<dependent>/node_modules/<name> <- nested copy for a conflict
//! ```
//!
//! [`build_node_layout`] converts a flat resolution into such a tree. It is a
//! pure function of the flat resolution plus the packuments (needed to pick a
//! version for a nested copy when the hoisted version cannot satisfy a
//! dependent's range), so it is fully unit-testable without a network.
//!
//! The lockfile writer already normalizes node keys (`node_modules/a` and
//! nested `.../node_modules/a`), and the lockfile-driven materializer already
//! installs to `LockfileInstallEntry.install_path`, so emitting node keys here
//! is enough to make nesting work end to end.

use crate::lockfile_write::ResolvedPackage;
use std::collections::VecDeque;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Node path for a top-level package.
pub fn top_level_node(name: &str) -> String {
    format!("node_modules/{}", name)
}

/// Return the first provider in lookup order, including its canonical slot.
pub(crate) fn visible_dependency_at<'a, T: 'a>(
    parent: &str,
    child: &str,
    mut lookup: impl FnMut(&str) -> Option<&'a T>,
) -> Option<(String, &'a T)> {
    let mut ancestor = Some(parent);
    while let Some(path) = ancestor {
        let candidate = format!("{path}/node_modules/{child}");
        if let Some(found) = lookup(&candidate) {
            return Some((candidate, found));
        }
        ancestor = path.rsplit_once("/node_modules/").map(|(parent, _)| parent);
    }
    let candidate = top_level_node(child);
    lookup(&candidate).map(|found| (candidate, found))
}

pub(crate) fn visible_dependency<'a, T: 'a>(
    parent: &str,
    child: &str,
    lookup: impl FnMut(&str) -> Option<&'a T>,
) -> Option<&'a T> {
    visible_dependency_at(parent, child, lookup).map(|(_, found)| found)
}

/// A peer is provided alongside a package or by an ancestor scope.
pub(crate) fn visible_peer_at<'a, T: 'a>(
    consumer: &str,
    peer: &str,
    mut lookup: impl FnMut(&str) -> Option<&'a T>,
) -> Option<(String, &'a T)> {
    if let Some((parent, _)) = consumer.rsplit_once("/node_modules/") {
        visible_dependency_at(parent, peer, lookup)
    } else {
        let candidate = top_level_node(peer);
        lookup(&candidate).map(|found| (candidate, found))
    }
}

pub(crate) fn visible_peer<'a, T: 'a>(
    consumer: &str,
    peer: &str,
    lookup: impl FnMut(&str) -> Option<&'a T>,
) -> Option<&'a T> {
    visible_peer_at(consumer, peer, lookup).map(|(_, found)| found)
}

/// Highest published version in `packument` satisfying `range`.
fn highest_satisfying(packument: &serde_json::Value, range: &str) -> Option<String> {
    crate::packument::pick_version_json(packument, range)
}

/// Build a node-keyed layout from a flat resolution.
///
/// * `direct`     — root `package.json` dependencies (name -> range).
/// * `resolved`   — flat resolution (name -> package); used for hoisting.
/// * `packuments` — full packuments, used to resolve nested copies.
///
/// Algorithm (npm-style hoisting with nesting only where required):
/// 1. The flat resolution's version for a name is the "hoisted" version and is
///    placed at `node_modules/<name>`.
/// 2. For every dependency edge, if the hoisted child version satisfies the
///    declared range, the edge points at the top-level node. Otherwise a nested
///    copy is created under the dependent at
///    `<dependent-node>/node_modules/<child>` with the highest published
///    version satisfying the range, and that copy's own edges are expanded.
/// 3. Cycles are handled with a `visited` set keyed by node path.
pub fn build_node_layout(
    direct: &HashMap<String, String>,
    resolved: &HashMap<String, ResolvedPackage>,
    packuments: &HashMap<String, serde_json::Value>,
) -> BTreeMap<String, ResolvedPackage> {
    build_node_layout_with(direct, resolved, |name| packuments.get(name))
}

/// [`build_node_layout`], reading packuments through an accessor.
///
/// The install path keeps the packuments it already fetched behind a shared
/// pointer (the transitive metadata snapshot). Materialising them into a
/// `HashMap<String, serde_json::Value>` just to call the version above would
/// deep-clone every packument — the exact cost that made a large project outgrow
/// memory — so this variant borrows them in place.
pub fn build_node_layout_with<'a, F>(
    direct: &HashMap<String, String>,
    resolved: &HashMap<String, ResolvedPackage>,
    get_packument: F,
) -> BTreeMap<String, ResolvedPackage>
where
    F: Fn(&str) -> Option<&'a serde_json::Value>,
{
    let mut out: BTreeMap<String, ResolvedPackage> = BTreeMap::new();

    // Hoisted version per package name. If the flat resolution is missing a
    // direct dep, fall back to the highest published version satisfying it.
    let mut hoisted: HashMap<String, String> = resolved
        .iter()
        .map(|(name, pkg)| (name.clone(), pkg.version.clone()))
        .collect();
    for (name, range) in direct {
        let satisfies = hoisted
            .get(name)
            .map(|v| crate::registry::version_satisfies(range, v))
            .unwrap_or(false);
        if !satisfies {
            if let Some(v) = get_packument(name).and_then(|p| highest_satisfying(p, range)) {
                hoisted.insert(name.clone(), v);
            }
        }
    }

    // Seed the queue with the direct dependencies.
    let mut queue: VecDeque<(String, String, String)> = VecDeque::new();
    let mut direct_names: Vec<&String> = direct.keys().collect();
    direct_names.sort();
    for name in direct_names {
        let Some(version) = hoisted.get(name).cloned() else {
            continue;
        };
        let node = top_level_node(name);
        queue.push_back((node, name.clone(), version));
    }
    // Also hoist every package the flat resolution knows about so that
    // transitively-required packages keep their top-level placement even if the
    // walk below does not reach them (e.g. peer-only packages).
    let mut resolved_names: Vec<&String> = resolved.keys().collect();
    resolved_names.sort();
    for name in resolved_names {
        if direct.contains_key(name) {
            continue;
        }
        if let Some(version) = hoisted.get(name).cloned() {
            queue.push_back((top_level_node(name), name.clone(), version));
        }
    }

    let mut visited: HashSet<String> = HashSet::new();
    while let Some((node, name, version)) = queue.pop_front() {
        if !visited.insert(node.clone()) {
            continue;
        }
        let Some(packument) = get_packument(&name) else {
            // No metadata: still record what we already know (resolved copy if
            // hoisted, otherwise a bare minimal entry).
            if let Some(pkg) = resolved.get(&name) {
                if pkg.version == version {
                    out.insert(node, pkg.clone());
                    continue;
                }
            }
            out.insert(
                node,
                ResolvedPackage {
                    registry_name: None,
                    resolved: crate::lockfile::tarball_url_from_registry(&name, &version),
                    version,
                    integrity: None,
                    dependencies: HashMap::new(),
                    peer_dependencies: HashMap::new(),
                    peer_dependencies_meta: HashMap::new(),
                manifest: Default::default(),
                },
            );
            continue;
        };

        let declared = declared_edges(packument, &version);
        let mut pkg = resolved
            .get(&name)
            .cloned()
            .unwrap_or_else(|| ResolvedPackage {
                registry_name: None,
                version: version.clone(),
                resolved: String::new(),
                integrity: None,
                dependencies: HashMap::new(),
                peer_dependencies: HashMap::new(),
                peer_dependencies_meta: HashMap::new(),
                               manifest: Default::default(),
            });
        // Ensure the node reports its own version (a nested copy differs from
        // the hoisted `resolved` entry).
        pkg.version = version.clone();
        if pkg.resolved.is_empty() {
            pkg.resolved = crate::lockfile::tarball_url_from_registry(&name, &version);
        }
        // Enrich with registry-provided tarball URL + SRI integrity. The flat
        // resolution carries the URL but not the integrity, and `jhol ci`
        // (strict lockfile) refuses a lockfile whose entries lack integrity.
        if let Some(dist) = packument
            .get("versions")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(&version))
            .and_then(|vi| vi.get("dist"))
        {
            // Always trust the packument for this exact version. A nested copy
            // is cloned from the hoisted package, so its `resolved`/`integrity`
            // would otherwise point at the hoisted version's tarball.
            if let Some(t) = dist.get("tarball").and_then(|v| v.as_str()) {
                pkg.resolved = t.to_string();
            }
            if let Some(i) = dist.get("integrity").and_then(|v| v.as_str()) {
                pkg.integrity = Some(i.to_string());
            }
        }

        let mut dep_versions: HashMap<String, String> = HashMap::new();
        for (child, range) in declared {
            let hoisted_ok = hoisted
                .get(&child)
                .map(|v| crate::registry::version_satisfies(&range, v))
                .unwrap_or(false);
            if hoisted_ok {
                let child_version = hoisted[&child].clone();
                dep_versions.insert(child.clone(), child_version.clone());
                queue.push_back((top_level_node(&child), child.clone(), child_version));
            } else if let Some(nested_version) =
                get_packument(&child).and_then(|p| highest_satisfying(p, &range))
            {
                let nested_node = format!("{}/{}", node, top_level_node(&child));
                dep_versions.insert(child.clone(), nested_version.clone());
                queue.push_back((nested_node, child.clone(), nested_version));
            }
            // If nothing satisfies the range, leave the edge out; the packing
            // verifier reports the conflict.
        }
        pkg.dependencies = dep_versions;
        pkg.peer_dependencies = crate::registry::get_version_peer_dependencies(packument, &version);
        pkg.peer_dependencies_meta =
            crate::registry::get_version_peer_dependencies_meta(packument, &version);
        out.insert(node, pkg);
    }

    out
}

/// Check declared required edges against Node's nearest installed ancestor.
/// A missing candidate must not disappear from the layout's dependency map.
pub(crate) fn required_edges_satisfied<'a>(
    layout: &BTreeMap<String, ResolvedPackage>,
    get_packument: impl Fn(&str) -> Option<&'a serde_json::Value>,
) -> bool {
    for (node, pkg) in layout {
        let Some(name) = node.rsplit("node_modules/").next() else {
            return false;
        };
        let Some(meta) = get_packument(name) else {
            return false;
        };
        if meta
            .get("versions")
            .and_then(|v| v.get(&pkg.version))
            .is_none()
        {
            return false;
        }
        for (child, range) in crate::registry::get_version_dependencies_with_optional(meta, &pkg.version)
        {
            let target = visible_dependency(node, &child, |path| layout.get(path));
            if !target.is_some_and(|p| crate::registry::version_satisfies(&range, &p.version)) {
                return false;
            }
        }
    }
    true
}

/// Declared dependency edges of `version`, including non-optional peers.
fn declared_edges(packument: &serde_json::Value, version: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(ver_info) = packument
        .get("versions")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(version))
    else {
        return out;
    };
    if let Some(deps) = ver_info.get("dependencies").and_then(|v| v.as_object()) {
        for (name, range) in deps {
            out.push((name.clone(), range.as_str().unwrap_or("*").to_string()));
        }
    }
    if let Some(peers) = ver_info.get("peerDependencies").and_then(|v| v.as_object()) {
        let meta = ver_info
            .get("peerDependenciesMeta")
            .and_then(|v| v.as_object());
        for (name, range) in peers {
            let optional = meta
                .and_then(|m| m.get(name))
                .and_then(|p| p.get("optional"))
                .and_then(|o| o.as_bool())
                .unwrap_or(false);
            if !optional {
                out.push((name.clone(), range.as_str().unwrap_or("*").to_string()));
            }
        }
    }
    out
}

/// True when the layout contains two different versions of the same package.
/// Used by callers/tests to detect that nesting actually happened.
pub fn has_duplicate_versions(layout: &BTreeMap<String, ResolvedPackage>) -> bool {
    let mut seen_versions: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (node, pkg) in layout {
        let Some(idx) = node.rfind("node_modules/") else {
            continue;
        };
        let name = &node[idx + "node_modules/".len()..];
        seen_versions
            .entry(name)
            .or_default()
            .insert(pkg.version.as_str());
    }
    seen_versions.values().any(|v| v.len() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn pkg(version: &str, deps: &[(&str, &str)]) -> ResolvedPackage {
        ResolvedPackage {
            registry_name: None,
            version: version.to_string(),
            resolved: format!("https://registry.npmjs.org/x/-/x-{}.tgz", version),
            integrity: None,
            dependencies: deps
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            peer_dependencies: HashMap::new(),
            peer_dependencies_meta: HashMap::new(),
        manifest: Default::default(),
        }
    }

    #[test]
    fn visible_dependency_keeps_nearest_scoped_instance() {
        let values = HashMap::from([
            ("node_modules/@scope/shared", 1),
            ("node_modules/parent/node_modules/@scope/shared", 2),
            (
                "node_modules/parent/node_modules/consumer/node_modules/@scope/shared",
                3,
            ),
        ]);
        let lookup = |path: &str| values.get(path);
        assert_eq!(
            visible_dependency(
                "node_modules/parent/node_modules/consumer",
                "@scope/shared",
                lookup
            ),
            Some(&3)
        );
        assert_eq!(
            visible_dependency(
                "node_modules/parent/node_modules/other",
                "@scope/shared",
                lookup
            ),
            Some(&2)
        );
        assert_eq!(
            visible_dependency("node_modules/other", "@scope/shared", lookup),
            Some(&1)
        );
        assert_eq!(
            visible_dependency("node_modules/parent", "absent", lookup),
            None
        );
    }

    #[test]
    fn omitted_conflicting_edge_is_not_a_complete_layout() {
        let direct = HashMap::from([("debug".into(), "2.6.9".into())]);
        let resolved = HashMap::from([
            ("debug".into(), pkg("2.6.9", &[("ms", "2.0.0")])),
            ("ms".into(), pkg("2.1.3", &[])),
        ]);
        let mut metas = HashMap::from([
            (
                "debug".into(),
                json!({"versions":{"2.6.9":{"dependencies":{"ms":"2.0.0"}}}}),
            ),
            ("ms".into(), json!({"versions":{"2.1.3":{}}})),
        ]);
        let layout = build_node_layout(&direct, &resolved, &metas);
        assert!(layout["node_modules/debug"].dependencies.is_empty());
        assert!(!required_edges_satisfied(&layout, |n| metas.get(n)));
        metas.get_mut("ms").unwrap()["versions"]["2.0.0"] = json!({});
        let mut layout = build_node_layout(&direct, &resolved, &metas);
        assert_eq!(
            layout["node_modules/debug/node_modules/ms"].version,
            "2.0.0"
        );
        assert!(required_edges_satisfied(&layout, |n| metas.get(n)));
        layout
            .get_mut("node_modules/debug/node_modules/ms")
            .unwrap()
            .version = "2.1.3".into();
        assert!(!required_edges_satisfied(&layout, |n| metas.get(n)));
    }

    #[test]
    fn flat_graph_stays_flat_and_top_level() {
        // a@1 depends on b@1; no conflict.
        let direct = HashMap::from([("a".to_string(), "^1.0.0".to_string())]);
        let resolved = HashMap::from([
            ("a".to_string(), pkg("1.0.0", &[("b", "1.0.0")])),
            ("b".to_string(), pkg("1.0.0", &[])),
        ]);
        let packuments = HashMap::from([
            (
                "a".to_string(),
                json!({"versions": {"1.0.0": {"dependencies": {"b": "^1.0.0"}}}}),
            ),
            ("b".to_string(), json!({"versions": {"1.0.0": {}}})),
        ]);

        let layout = build_node_layout(&direct, &resolved, &packuments);
        assert!(layout.contains_key("node_modules/a"));
        assert!(layout.contains_key("node_modules/b"));
        assert!(!has_duplicate_versions(&layout));
    }

    #[test]
    fn conflicting_dependency_nests_the_non_hoisted_version() {
        // root deps: a (needs b ^2) and b ^1 (hoisted at top = b@1).
        // a's requirement b ^2 cannot be met by b@1 -> nested under a.
        let direct = HashMap::from([
            ("a".to_string(), "^1.0.0".to_string()),
            ("b".to_string(), "^1.0.0".to_string()),
        ]);
        let resolved = HashMap::from([
            ("a".to_string(), pkg("1.0.0", &[("b", "2.0.0")])),
            ("b".to_string(), pkg("1.0.0", &[])),
        ]);
        let packuments = HashMap::from([
            (
                "a".to_string(),
                json!({"versions": {"1.0.0": {"dependencies": {"b": "^2.0.0"}}}}),
            ),
            (
                "b".to_string(),
                json!({"versions": {
                    "1.0.0": {},
                    "2.1.0": {},
                    "2.0.0": {}
                }}),
            ),
        ]);

        let layout = build_node_layout(&direct, &resolved, &packuments);
        assert_eq!(layout["node_modules/b"].version, "1.0.0");
        let nested = &layout["node_modules/a/node_modules/b"];
        assert_eq!(nested.version, "2.1.0");
        assert_eq!(layout["node_modules/a"].dependencies["b"], "2.1.0");
        assert!(has_duplicate_versions(&layout));
    }

    #[test]
    fn layout_enriches_integrity_from_packument_dist() {
        let direct = HashMap::from([("a".to_string(), "^1.0.0".to_string())]);
        let mut a = pkg("1.0.0", &[]);
        a.resolved = String::new();
        let resolved = HashMap::from([("a".to_string(), a)]);
        let packuments = HashMap::from([(
            "a".to_string(),
            json!({"versions": {"1.0.0": {
                "dependencies": {},
                "dist": {
                    "tarball": "https://registry.npmjs.org/a/-/a-1.0.0.tgz",
                    "integrity": "sha512-AAA"
                }
            }}}),
        )]);
        let layout = build_node_layout(&direct, &resolved, &packuments);
        assert_eq!(
            layout["node_modules/a"].integrity.as_deref(),
            Some("sha512-AAA")
        );
        assert_eq!(
            layout["node_modules/a"].resolved,
            "https://registry.npmjs.org/a/-/a-1.0.0.tgz"
        );
    }

    #[test]
    fn scoped_names_are_supported() {
        let direct = HashMap::from([("@scope/a".to_string(), "^1.0.0".to_string())]);
        let resolved = HashMap::from([
            (
                "@scope/a".to_string(),
                pkg("1.0.0", &[("@scope/b", "1.0.0")]),
            ),
            ("@scope/b".to_string(), pkg("1.0.0", &[])),
        ]);
        let packuments = HashMap::from([
            (
                "@scope/a".to_string(),
                json!({"versions": {"1.0.0": {"dependencies": {"@scope/b": "^1.0.0"}}}}),
            ),
            ("@scope/b".to_string(), json!({"versions": {"1.0.0": {}}})),
        ]);
        let layout = build_node_layout(&direct, &resolved, &packuments);
        assert!(layout.contains_key("node_modules/@scope/a"));
        assert!(layout.contains_key("node_modules/@scope/b"));
    }

    #[test]
    fn cycles_terminate() {
        // a -> b -> a
        let direct = HashMap::from([("a".to_string(), "^1.0.0".to_string())]);
        let resolved = HashMap::from([
            ("a".to_string(), pkg("1.0.0", &[("b", "1.0.0")])),
            ("b".to_string(), pkg("1.0.0", &[("a", "1.0.0")])),
        ]);
        let packuments = HashMap::from([
            (
                "a".to_string(),
                json!({"versions": {"1.0.0": {"dependencies": {"b": "^1.0.0"}}}}),
            ),
            (
                "b".to_string(),
                json!({"versions": {"1.0.0": {"dependencies": {"a": "^1.0.0"}}}}),
            ),
        ]);
        let layout = build_node_layout(&direct, &resolved, &packuments);
        assert!(layout.contains_key("node_modules/a"));
        assert!(layout.contains_key("node_modules/b"));
        assert!(!has_duplicate_versions(&layout));
    }

    /// Property: nested layout always places a satisfying child for every
    /// dependency edge (blueprint Phase A layout invariant).
    #[test]
    fn property_every_dep_edge_has_satisfying_child() {
        let cases: Vec<(
            HashMap<String, String>,
            HashMap<String, ResolvedPackage>,
            HashMap<String, Value>,
        )> = vec![
            // flat
            (
                HashMap::from([("a".into(), "^1".into())]),
                HashMap::from([
                    ("a".into(), pkg("1.0.0", &[("b", "1.0.0")])),
                    ("b".into(), pkg("1.0.0", &[])),
                ]),
                HashMap::from([
                    (
                        "a".into(),
                        json!({"versions": {"1.0.0": {"dependencies": {"b": "^1.0.0"}}}}),
                    ),
                    ("b".into(), json!({"versions": {"1.0.0": {}}})),
                ]),
            ),
            // conflict nesting
            (
                HashMap::from([
                    ("cliui".into(), "^8".into()),
                    ("string-width".into(), "^5".into()),
                ]),
                HashMap::from([
                    ("cliui".into(), pkg("8.0.1", &[("string-width", "4.2.3")])),
                    ("string-width".into(), pkg("5.1.2", &[])),
                ]),
                HashMap::from([
                    (
                        "cliui".into(),
                        json!({
                            "versions": {
                                "8.0.1": {
                                    "dependencies": { "string-width": "^4.2.0" },
                                    "dist": {
                                        "tarball": "https://registry.npmjs.org/cliui/-/cliui-8.0.1.tgz",
                                        "integrity": "sha512-cliui"
                                    }
                                }
                            }
                        }),
                    ),
                    (
                        "string-width".into(),
                        json!({
                            "versions": {
                                "4.2.3": {
                                    "dist": {
                                        "tarball": "https://registry.npmjs.org/string-width/-/string-width-4.2.3.tgz",
                                        "integrity": "sha512-sw4"
                                    }
                                },
                                "5.1.2": {
                                    "dist": {
                                        "tarball": "https://registry.npmjs.org/string-width/-/string-width-5.1.2.tgz",
                                        "integrity": "sha512-sw5"
                                    }
                                }
                            }
                        }),
                    ),
                ]),
            ),
        ];

        for (direct, resolved, packuments) in cases {
            let layout = build_node_layout(&direct, &resolved, &packuments);
            for (node_path, pkg) in &layout {
                for (dep_name, range) in &pkg.dependencies {
                    let nested = format!("{node_path}/node_modules/{dep_name}");
                    let hoisted = format!("node_modules/{dep_name}");
                    let child = layout
                        .get(&nested)
                        .or_else(|| layout.get(&hoisted))
                        .unwrap_or_else(|| panic!("missing child {dep_name} for {node_path}"));
                    assert!(
                        crate::registry::version_satisfies(range, &child.version),
                        "{node_path} -> {dep_name}@{range} got {}",
                        child.version
                    );
                }
            }
        }
    }

    /// Corrupted packument versions map must not panic the layout builder.
    #[test]
    fn corrupted_packument_versions_do_not_panic() {
        let direct = HashMap::from([("a".to_string(), "^1.0.0".to_string())]);
        let resolved = HashMap::from([("a".to_string(), pkg("1.0.0", &[]))]);
        let packuments = HashMap::from([(
            "a".to_string(),
            json!({
                "versions": "not-an-object",
                "dist-tags": null
            }),
        )]);
        let layout = build_node_layout(&direct, &resolved, &packuments);
        assert!(layout.contains_key("node_modules/a"));
    }
}
