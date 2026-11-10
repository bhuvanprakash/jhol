//! Time a single recursive `clonefile` of a directory tree.
//!
//! `cp -c` clones per file; this is one kernel call, which is what an APFS snapshot
//! actually costs. Used to settle whether a whole-tree prepared snapshot can beat
//! per-package materialisation.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(src) = args.first() else {
        eprintln!("usage: clone_probe <src-dir> <dst-dir>");
        std::process::exit(2);
    };
    let dst = args.get(1).cloned().unwrap_or_else(|| format!("{src}.clone"));
    let dst = std::path::PathBuf::from(dst);
    let _ = std::fs::remove_dir_all(&dst);
    let files = walk_count(std::path::Path::new(src));
    let started = std::time::Instant::now();
    match jhol_core::jgrid::materialize::Cloner::clone_directory(std::path::Path::new(src), &dst) {
        Ok(()) => println!(
            "single clonefile of {files} files: {} ms",
            started.elapsed().as_millis()
        ),
        Err(e) => println!("clonefile failed: {e}"),
    }
}

fn walk_count(root: &std::path::Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += walk_count(&p);
            } else {
                n += 1;
            }
        }
    }
    n
}
