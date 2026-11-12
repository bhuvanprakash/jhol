//! Probe what the metadata layer actually returns for a package: version count, whether a
//! specific string is present, and the dist-tags. Used to separate "the chooser is wrong" from
//! "the document we hold is incomplete".
fn main() {
    let pkg = std::env::args().nth(1).unwrap_or_else(|| "@babel/preset-modules".into());
    match jhol_core::registry::fetch_metadata(&pkg) {
        Ok(meta) => {
            let versions: Vec<String> = meta
                .get("versions")
                .and_then(|v| v.as_object())
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let want = std::env::args().nth(2);
            println!("package {pkg}: {} versions", versions.len());
            println!("last 6: {:?}", &versions[versions.len().saturating_sub(6)..]);
            println!("tags: {:?}", meta.get("dist-tags"));
            if let Some(w) = want {
                println!("contains {w}: {}", versions.iter().any(|v| *v == w));
            }
        }
        Err(e) => println!("fetch failed: {e}"),
    }
}
