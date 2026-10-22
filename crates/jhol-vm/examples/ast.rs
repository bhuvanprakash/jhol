fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let _a = jhol_vm::arena::Arena::new();
    let prog = jhol_vm::parser::parse_program(&src).unwrap();
    for s in &prog.body {
        if let jhol_vm::ast::Stmt::Class(c) = s {
            println!("class {:?} members={}", c.name, c.members.len());
            for m in &c.members {
                println!("  member kind={}", match &m.kind {
                    jhol_vm::ast::ClassMemberKind::Method(_, s) => format!("method static={}", s),
                    jhol_vm::ast::ClassMemberKind::Field(_) => "field".to_string(),
                    jhol_vm::ast::ClassMemberKind::StaticField(_) => "static field".to_string(),
                });
            }
        }
    }
}
