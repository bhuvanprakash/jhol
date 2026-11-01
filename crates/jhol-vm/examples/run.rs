fn main() {
    let mut e = jhol_vm::vm::Engine::new();
    let p = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(&p).unwrap();
    match e.eval_source(&src, &p) {
        Ok(_) => {}
        Err(er) => match &er {
            jhol_vm::errors::VmError::Exception(t) => {
                let s = e.vm.to_string(t.value).unwrap_or_else(|_| "?".into());
                eprintln!("ERR: {}", s);
            }
            other => eprintln!("ERR: {}", other),
        },
    }
}
