fn main() {
    let p = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(&p).unwrap();
    let mut lx = jhol_vm::lexer::Lexer::new(&src);
    let _ = lx.run();
    for i in 0..lx.toks.len() {
        let a = lx.starts[i] as usize;
        let b = lx.starts.get(i + 1).copied().unwrap_or(src.len() as u32) as usize;
        let slice = &src[a.min(src.len())..b.min(src.len())];
        let quotes = slice.matches('"').count();
        let odd = quotes % 2 == 1;
        let is_str = matches!(lx.toks[i].0, jhol_vm::lexer::Tok::Str(_));
        if odd && !is_str {
            let sl: String = slice.chars().take(90).collect();
            println!("ODD tok{i} @{a} {:?}: {}", lx.toks[i].0, sl.escape_debug());
            if i > 0 {
                let pa = lx.starts[i - 1] as usize;
                let psl: String = src[pa..a.min(src.len())].chars().take(90).collect();
                println!("   prev: {}", psl.escape_debug());
            }
            break;
        }
    }
    println!("checked {}", lx.toks.len());
}
