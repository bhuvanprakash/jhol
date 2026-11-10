//! Why is a snapshot not being used? Print the predicates `restore_snapshot` checks.
fn main() {
    let root = std::path::Path::new(".");
    println!("node_modules exists: {}", root.join("node_modules").exists());
    println!("snapshot_ready:      {}", jhol_core::install::snapshot_ready(root));
    for i in [0usize, 1] {
        let dir = root.join(format!(".jhol/prepared-{}", i + 1));
        let mark = root.join(format!(".jhol/prepared-{}.json", i + 1));
        println!(
            "slot {i}: dir={} marker={} content={:?}",
            dir.is_dir(),
            mark.is_file(),
            std::fs::read_to_string(&mark).ok().map(|s| s.trim().to_string())
        );
    }
}
