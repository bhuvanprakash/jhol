fn main() {
    let mut e = jhol_vm::vm::Engine::new();
    e.ensure_started();
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let v = e.eval_source(&src, "t.js").unwrap();
    println!("completion: {:?}", e.vm.to_string(v));
}
