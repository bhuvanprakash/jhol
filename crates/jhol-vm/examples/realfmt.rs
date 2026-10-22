//! Parse + format + verify a directory of real-world JavaScript.
//!
//! Every file must (1) parse, (2) format without error, (3) re-parse, and
//! (4) format idempotently. Failures are printed as they happen so a crash
//! names the file responsible.

use std::fs;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            walk(&p, out);
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("js" | "mjs" | "cjs")
        ) {
            out.push(p);
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(usize::MAX);
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    files.sort();
    files.truncate(limit);

    let (mut parsed, mut parse_fail, mut fmt_fail, mut reparse_fail, mut non_idem) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for f in &files {
        let src = match fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => continue, // non-UTF8: the runtime rejects these already
        };
        let prog = match jhol_vm::parser::parse_program(&src) {
            Ok(p) => p,
            Err(e) => {
                parse_fail += 1;
                if parse_fail <= 15 {
                    println!("PARSE  {}: {}", f.display(), e);
                }
                continue;
            }
        };
        parsed += 1;
        let _ = prog;

        let out = match jhol_vm::formatter::format_source(&src, &f.to_string_lossy()) {
            Ok(s) => s,
            Err(e) => {
                fmt_fail += 1;
                if fmt_fail <= 15 {
                    println!("FMT    {}: {}", f.display(), e);
                }
                continue;
            }
        };
        // The formatted output must itself parse.
        if let Err(e) = jhol_vm::parser::parse_program(&out) {
            reparse_fail += 1;
            if reparse_fail <= 15 {
                println!("REPARSE {}: {}", f.display(), e);
            }
            continue;
        }
        // Formatting twice must be a no-op.
        match jhol_vm::formatter::format_source(&out, &f.to_string_lossy()) {
            Ok(out2) if out2 == out => {}
            _ => {
                non_idem += 1;
                if non_idem <= 15 {
                    println!("IDEM   {}", f.display());
                }
            }
        }
    }
    println!(
        "files={} parsed={} parse_fail={} fmt_fail={} reparse_fail={} non_idempotent={}",
        files.len(),
        parsed,
        parse_fail,
        fmt_fail,
        reparse_fail,
        non_idem
    );
}
