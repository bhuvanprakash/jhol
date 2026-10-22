fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let p = jhol_vm::parser::parse_program(&src).unwrap();
    for (i, s) in p.body.iter().enumerate() {
        match s {
            jhol_vm::ast::Stmt::Comment { text, trailing } => {
                println!("{i}: trailing={trailing} text={text:?}")
            }
            _ => println!("{i}: stmt"),
        }
    }
}
