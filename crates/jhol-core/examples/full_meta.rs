fn main() {
    for name in std::env::args().skip(1) {
        match jhol_core::registry::fetch_metadata_full(&name) {
            Ok(doc) => {
                let versions = doc.get("versions").and_then(|v| v.as_object()).map(|m| m.len()).unwrap_or(0);
                let times = doc.get("time").and_then(|t| t.as_object()).map(|m| m.len()).unwrap_or(0);
                let sample = doc.get("time").and_then(|t| t.get(
                    doc.get("dist-tags").and_then(|d| d.get("latest")).and_then(|v| v.as_str()).unwrap_or("")
                )).and_then(|v| v.as_str()).unwrap_or("?");
                println!("{name}: versions={versions} time_entries={times} latest_published={sample}");
            }
            Err(e) => println!("{name}: ERROR {e}"),
        }
    }
}
