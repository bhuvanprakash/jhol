// Does the resolver itself return optional deps? Walks the same entry point install uses.
fn main() {
    let pj = std::env::args().nth(1).unwrap_or_else(|| "package.json".into());
    match jhol_core::lockfile_write::resolve_full_tree_nested(std::path::Path::new(&pj)) {
        Ok(tree) => {
            let hits: Vec<&String> = tree.keys().filter(|k| k.contains("fsevents") || k.contains("haste-map")).collect();
            println!("entries={} matches={hits:?}", tree.len());
        }
        Err(e) => println!("resolver error: {e}"),
    }
}
