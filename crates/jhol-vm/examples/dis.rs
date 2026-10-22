fn main() {
    let _a = jhol_vm::arena::Arena::new();
    _a.install();
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let prog = jhol_vm::parser::parse_program(&src).unwrap();
    let m = jhol_vm::compiler::compile(&prog, "t").unwrap();
    println!("entry={}", m.entry);
    for (i, p) in m.protos.iter().enumerate() {
        println!("=== proto {} `{}` slots={} params={}", i, p.name, p.nslots, p.nparams);
        for (j, ins) in p.code.iter().enumerate() {
            println!("  {:3} {:?} a={} b={} c={} ickey={:?}", j, ins.op, ins.a, ins.b, ins.c, p.ics[j].key);
        }
        println!("  consts: {:?}", p.consts);
    }
}
