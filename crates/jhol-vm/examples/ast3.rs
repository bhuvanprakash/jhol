fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let _a = jhol_vm::arena::Arena::new();
    let prog = jhol_vm::parser::parse_program(&src).unwrap();
    for s in &prog.body {
        if let jhol_vm::ast::Stmt::Class(c) = s {
            println!("class {:?} extends={:?}", c.name, c.superclass.as_ref().map(|e| format!("{:?}", e)));
        }
    }
}
