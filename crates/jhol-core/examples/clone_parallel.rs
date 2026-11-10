//! Compare one recursive clonefile with cloning the top-level children concurrently.
//! clonefile copies metadata per file, so the kernel may parallelise independent
//! subtrees; if it does, wall time is the slowest child, not the sum.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let src = std::path::PathBuf::from(&args[0]);
    let serial = std::path::PathBuf::from("/tmp/cp_serial");
    let parallel = std::path::PathBuf::from("/tmp/cp_parallel");
    let _ = std::fs::remove_dir_all(&serial);
    let _ = std::fs::remove_dir_all(&parallel);

    let t = std::time::Instant::now();
    let _ = jhol_core::jgrid::materialize::Cloner::clone_directory(&src, &serial);
    let serial_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = std::time::Instant::now();
    std::fs::create_dir_all(&parallel).unwrap();
    let entries: Vec<_> = std::fs::read_dir(&src)
        .unwrap()
        .flatten()
        .map(|e| (e.path(), e.file_name()))
        .collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    for chunk in entries.chunks(entries.len().div_ceil(threads).max(1)) {
        std::thread::scope(|scope| {
            for (path, name) in chunk {
                let dst = parallel.join(name);
                scope.spawn(move || {
                    let _ = jhol_core::jgrid::materialize::Cloner::clone_directory(path, &dst);
                });
            }
        });
    }
    let parallel_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{} entries: serial {serial_ms:.1} ms | parallel({threads}) {parallel_ms:.1} ms",
        entries.len()
    );
    let _ = std::fs::remove_dir_all(&serial);
    let _ = std::fs::remove_dir_all(&parallel);
}
