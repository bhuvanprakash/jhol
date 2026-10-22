fn main() {
    let p = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(&p).unwrap();
    match jhol_vm::formatter::format_source(&src, &p) {
        Ok(s) => print!("{}", s),
        Err(e) => eprintln!("fmt error: {}", e),
    }
}

#[test]
fn instr_size() {
    println!("Instr = {} bytes", std::mem::size_of::<jhol_vm::bytecode::Instr>());
    println!("Op    = {} bytes", std::mem::size_of::<jhol_vm::bytecode::Op>());
}
