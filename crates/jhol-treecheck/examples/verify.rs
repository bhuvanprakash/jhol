fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let p = std::path::Path::new(&dir);
    let state = jhol_treecheck::verify_tree(p);
    println!("{:?}\n{}", state, state.describe());
    let rec = jhol_treecheck::reconcile_tree(p);
    println!("after reconcile: {:?}", rec);
}
