//! Offline differential probes for JAGR-Infinite Paper IV.
use jhol_core::jagr_infinite::{
    chart::{VersionBitmap, VersionInterval, VersionOrdinal},
    select_versions,
};
use jhol_core::jagr_nest::{resolve_with_options, ResolveOptions};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

fn bitmap(hi: u16) -> VersionBitmap {
    VersionBitmap::from_interval(VersionInterval {
        lo: VersionOrdinal(0),
        hi: VersionOrdinal(hi),
    })
}

fn bitmap_probe() -> Value {
    let ends = [0, 62, 63, 64, 65, 126, 127, 128, 129];
    let mut differences = Vec::new();
    for left in ends {
        for right in ends {
            let mut actual = bitmap(left);
            actual.intersect(&bitmap(right));
            let expected = (0..=left.min(right)).collect::<Vec<_>>();
            let observed = (0..=left.max(right))
                .filter(|ordinal| actual.contains_ordinal(VersionOrdinal(*ordinal)))
                .collect::<Vec<_>>();
            if observed != expected {
                differences.push(json!({
                    "left_last_ordinal": left, "right_last_ordinal": right,
                    "expected_count": expected.len(), "observed_count": observed.len(),
                    "first_extra_ordinal": observed.iter().find(|ordinal| !expected.contains(ordinal)),
                }));
            }
        }
    }
    json!({"pairs": ends.len() * ends.len(), "differences": differences})
}

fn probe(world: HashMap<String, Value>, roots: HashMap<String, String>) -> Value {
    let flat = select_versions(&world, &roots)
        .map(|versions| versions.into_iter().collect::<BTreeMap<_, _>>());
    let nested = match resolve_with_options(&world, &roots, &ResolveOptions::default()) {
        Ok(plan) => {
            let mut instances: Vec<_> = plan
                .instances
                .into_iter()
                .map(|i| (i.package, i.installed_as, i.version))
                .collect();
            instances.sort();
            json!({"result": "selected", "instances": instances})
        }
        Err(e) => json!({
            "result": format!("{:?}", e.failure),
            "package": e.package, "range": e.spec, "dependent": e.required_by
        }),
    };
    json!({"flat": flat, "nested": nested})
}

fn main() {
    let greedy = HashMap::from([
        (
            "parent".into(),
            json!({"name":"parent", "versions":{
                "1.0.0":{"version":"1.0.0"},
                "1.1.0":{"version":"1.1.0","dependencies":{"child":"^2.0.0"}}
            }}),
        ),
        (
            "child".into(),
            json!({"name":"child", "versions":{
                "1.0.0":{"version":"1.0.0"}
            }}),
        ),
    ]);
    let multi = HashMap::from([
        (
            "left".into(),
            json!({"name":"left", "versions":{
                "1.0.0":{"version":"1.0.0","dependencies":{"child":"^1.0.0"}}
            }}),
        ),
        (
            "right".into(),
            json!({"name":"right", "versions":{
                "1.0.0":{"version":"1.0.0","dependencies":{"child":"^2.0.0"}}
            }}),
        ),
        (
            "child".into(),
            json!({"name":"child", "versions":{
                "1.0.0":{"version":"1.0.0"}, "2.0.0":{"version":"2.0.0"}
            }}),
        ),
    ]);
    let greedy_result = probe(greedy, HashMap::from([("parent".into(), "^1.0.0".into())]));
    let multi_result = probe(
        multi,
        HashMap::from([
            ("left".into(), "1.0.0".into()),
            ("right".into(), "1.0.0".into()),
        ]),
    );
    println!("{}", serde_json::to_string_pretty(&json!({
        "scope":"Two pure Rust resolver APIs on synthetic metadata; no CLI, network, placement, or install timing.",
        "bitmap_intersection": bitmap_probe(),
        "conditional_dependency": {
            "valid_witness":{"parent":"1.0.0"},
            "native":greedy_result,
        },
        "two_major_control": {
            "expected_instances":4,
            "native":multi_result,
        },
    })).unwrap());
}
