//! Time `verify_tree` in-process, without CLI startup in the number.
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = std::path::Path::new(&dir);
    let n = std::env::args()
        .nth(2)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20);
    // warm the page cache
    let _ = jhol_core::tree_state::verify_tree(root);
    let started = std::time::Instant::now();
    for _ in 0..n {
        let _ = jhol_core::tree_state::verify_tree(root);
    }
    let per = started.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("verify_tree: {per:.2} ms/call over {n} calls ({dir})");
}
