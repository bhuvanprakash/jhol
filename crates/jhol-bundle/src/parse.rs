//! Jhol Bundle — Parse Module (CAS-first)
//!
//! Produces [ParseManifest] from source content. Every unique content hash is
//! parsed exactly once — the result is stored in CAS and reused.
//!
//! Parsing strategy:
//!   - `.js` / `.mjs` / `.cjs` / `.jsx` (without TypeScript features):
//!     Fast regex-based import/export extraction. No full AST needed for
//!     bundling purposes. Covers 90%+ of npm packages.
//!   - `.ts` / `.tsx` / `.jsx` (with TypeScript/JSX features):
//!     Full swc AST parse for accurate import/export resolution and syntax
//!     validation. Falls back to regex if swc feature is disabled.
//!   - `.css` / `.json`: Minimal parsing, no swc needed.
//!
//! The ParseManifest is a minimal bundle-focused representation:
//!   - No full AST (too large to serialize to CAS)
//!   - Only import/export/dep records (needed for graph building)
//!   - Side effect flags (needed for tree shaking)
//!   - Source text retained for transform and emit

use std::path::PathBuf;

use crate::cas::{ImportKind, ImportRecord as CasImportRecord, ParseManifest};
use crate::config::BundleConfig;
use crate::errors::BundleResult;
use crate::ingest::FileType;

/// Parse source content and produce a ParseManifest.
/// This is the main entry point for Layer 2.
/// Every unique (content_hash, file_type) pair is parsed exactly once.
pub fn parse_to_manifest(
    resolved_path: PathBuf,
    source: String,
    file_type: FileType,
    content_hash: u64,
    is_entry: bool,
    _config: &BundleConfig,
) -> BundleResult<ParseManifest> {
    let (imports, reexports, dynamic_imports, requires) =
        extract_import_records(&source, file_type)?;
    let (exports, ast_hash) = extract_export_records(&source, file_type)?;
    let has_side_effects = detect_side_effects(&source, &imports);
    let dependency_hashes = Vec::new(); // resolved by graph builder

    Ok(ParseManifest {
        content_hash,
        ast_hash,
        path: PathBuf::from(resolved_path),
        file_type: file_type.as_str().to_string(),
        source,
        imports,
        reexports,
        dynamic_imports,
        requires,
        exports,
        dependency_hashes,
        has_side_effects,
        is_entry,
    })
}

// ── Import/Export extraction ──────────────────────────────────────────────────

/// Extract all import records from source using the right parser.
fn extract_import_records(
    source: &str,
    file_type: FileType,
) -> BundleResult<(
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
)> {
    match file_type {
        FileType::TypeScript | FileType::Tsx => extract_for_typescript(source),
        FileType::Jsx => extract_for_jsx(source),
        // Plain JS — the hot path for most npm packages — goes through
        // JholVM's own parser (exact statement boundaries; the regex
        // fallback's false positives inside strings/comments/regex literals
        // are gone).
        _ => extract_with_jhol_parser(source),
    }
}

/// Extract import records with JholVM's parser. Module syntax arrives as
/// `Stmt::Raw` (the parser round-trips import/export verbatim), so the
/// statement has ALREADY been isolated from comments, strings and regex
/// literals — pulling the specifier out of that one statement is exact, not
/// a heuristic. Dynamic `import()` and `require()` calls are walked in the
/// expression tree.
fn extract_with_jhol_parser(
    source: &str,
) -> BundleResult<(
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
)> {
    let program = jhol_vm::parser::parse_program(source).map_err(|e| {
        crate::errors::BundleError::ParseError {
            path: "inline".to_string(),
            line: 0,
            column: 0,
            message: e,
        }
    })?;

    let mut imports = Vec::new();
    let mut reexports = Vec::new();
    let mut dynamic_imports = Vec::new();
    let mut requires = Vec::new();

    let record = |text: &str, kind: ImportKind| -> Option<CasImportRecord> {
        let spec = raw_module_specifier(text)?;
        let start = source.find(text).unwrap_or(0);
        Some(CasImportRecord {
            specifier: spec,
            kind,
            start,
            end: start + text.len(),
        })
    };

    let mut walk_expr = |e: &jhol_vm::ast::Expr,
                         dyn_i: &mut Vec<CasImportRecord>,
                         req: &mut Vec<CasImportRecord>| {
        walk_call_exprs(e, dyn_i, req);
    };
    let _ = &mut walk_expr;

    for stmt in &program.body {
        collect_stmt(
            stmt,
            &mut imports,
            &mut reexports,
            &mut dynamic_imports,
            &mut requires,
            &record,
        );
    }
    Ok((imports, reexports, dynamic_imports, requires))
}

/// Specifier out of one verbatim module statement. Handles the shapes the
/// parser emits for import/export: the specifier is the last (or only)
/// string literal in the statement text.
fn raw_module_specifier(text: &str) -> Option<String> {
    let mut spec = None;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                let mut j = i + 1;
                let mut val = String::new();
                while j < bytes.len() && bytes[j] != quote {
                    if bytes[j] == b'\\' && j + 1 < bytes.len() {
                        j += 1;
                    }
                    val.push(bytes[j] as char);
                    j += 1;
                }
                if j >= bytes.len() {
                    return None;
                }
                spec = Some(val);
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    spec.filter(|_| text.starts_with("import") || text.starts_with("export"))
}

fn collect_stmt(
    stmt: &jhol_vm::ast::Stmt,
    imports: &mut Vec<CasImportRecord>,
    reexports: &mut Vec<CasImportRecord>,
    dynamic_imports: &mut Vec<CasImportRecord>,
    requires: &mut Vec<CasImportRecord>,
    record: &dyn Fn(&str, ImportKind) -> Option<CasImportRecord>,
) {
    match stmt {
        jhol_vm::ast::Stmt::Raw(text) => {
            let t = text.trim_start();
            if t.starts_with("import") {
                // `export ... from` never starts with import; a bare
                // `import "x"` side-effect import has no braces/default.
                let is_reexport = t.contains(" export ") || t.contains("* as") && false;
                let _ = is_reexport;
                if let Some(rec) = record(t, ImportKind::StaticImport) {
                    imports.push(rec);
                }
            } else if t.starts_with("export") {
                let is_reexport = t.contains(" from ") || t.contains("*");
                let kind = if is_reexport {
                    ImportKind::ReExport
                } else {
                    ImportKind::StaticImport
                };
                if let Some(rec) = record(t, kind) {
                    if is_reexport {
                        reexports.push(rec);
                    } else {
                        imports.push(rec);
                    }
                }
            }
        }
        jhol_vm::ast::Stmt::Expr(e) => {
            walk_call_exprs(e, dynamic_imports, requires);
        }
        jhol_vm::ast::Stmt::Var(_, pairs) => {
            for (_, init) in pairs {
                if let Some(e) = init {
                    walk_call_exprs(e, dynamic_imports, requires);
                }
            }
        }
        jhol_vm::ast::Stmt::Block(body) => {
            for s in body {
                collect_stmt(s, imports, reexports, dynamic_imports, requires, record);
            }
        }
        jhol_vm::ast::Stmt::If(_, body, els) => {
            collect_stmt(body, imports, reexports, dynamic_imports, requires, record);
            if let Some(els) = els {
                collect_stmt(els, imports, reexports, dynamic_imports, requires, record);
            }
        }
        _ => {}
    }
}

fn walk_call_exprs(
    e: &jhol_vm::ast::Expr,
    dynamic_imports: &mut Vec<CasImportRecord>,
    requires: &mut Vec<CasImportRecord>,
) {
    match e {
        jhol_vm::ast::Expr::Call { callee, args, .. } => {
            let is_import = matches!(**callee, jhol_vm::ast::Expr::Ident(ref n) if n == "import");
            let is_require = matches!(**callee, jhol_vm::ast::Expr::Ident(ref n) if n == "require");
            if (is_import || is_require) && args.len() == 1 {
                if let jhol_vm::ast::Expr::Str(spec) = &args[0] {
                    let rec = CasImportRecord {
                        specifier: spec.clone(),
                        kind: if is_import {
                            ImportKind::DynamicImport
                        } else {
                            ImportKind::Require
                        },
                        start: 0,
                        end: 0,
                    };
                    if is_import {
                        dynamic_imports.push(rec);
                    } else {
                        requires.push(rec);
                    }
                }
            }
            walk_call_exprs(callee, dynamic_imports, requires);
            for a in args {
                walk_call_exprs(a, dynamic_imports, requires);
            }
        }
        jhol_vm::ast::Expr::Member { obj, .. } => walk_call_exprs(obj, dynamic_imports, requires),
        jhol_vm::ast::Expr::Seq(xs) => {
            for x in xs {
                walk_call_exprs(x, dynamic_imports, requires);
            }
        }
        jhol_vm::ast::Expr::Array(xs) => {
            for x in xs.iter().flatten() {
                walk_call_exprs(x, dynamic_imports, requires);
            }
        }
        _ => {}
    }
}

fn extract_for_typescript(
    source: &str,
) -> BundleResult<(
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
)> {
    #[cfg(feature = "swc-backend")]
    {
        extract_with_swc(source, FileType::TypeScript)
    }
    #[cfg(not(feature = "swc-backend"))]
    {
        extract_with_regex(source)
    }
}

fn extract_for_jsx(
    source: &str,
) -> BundleResult<(
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
)> {
    #[cfg(feature = "swc-backend")]
    {
        extract_with_swc(source, FileType::Jsx)
    }
    #[cfg(not(feature = "swc-backend"))]
    {
        extract_with_regex(source)
    }
}

/// Fast regex-based import/export extraction for plain JS files.
/// This is the hot path: most npm packages are plain JS.
fn extract_with_regex(
    source: &str,
) -> BundleResult<(
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
)> {
    let mut imports = Vec::new();
    let mut reexports = Vec::new();
    let mut dynamic_imports = Vec::new();
    let mut requires = Vec::new();

    // Static imports: import ... from "..."
    // Supports: import "x"; import x from "x"; import {a,b} from "x"; import * as x from "x"
    for cap in find_specifiers(source, SpecScan::StaticImport) {
        let start = cap.start;
        let end = cap.end;
        if let Some(spec) = extract_quoted(&source[cap.start..cap.end]) {
            imports.push(CasImportRecord {
                specifier: spec,
                start,
                end,
                kind: crate::cas::ImportKind::StaticImport,
            });
        }
    }

    // Re-exports: export ... from "..."
    for cap in find_specifiers(source, SpecScan::ReExport) {
        let start = cap.start;
        let end = cap.end;
        if let Some(spec) = extract_quoted(&source[cap.start..cap.end]) {
            reexports.push(CasImportRecord {
                specifier: spec,
                start,
                end,
                kind: crate::cas::ImportKind::ReExport,
            });
        }
    }

    // Dynamic imports: import("...")
    for cap in find_specifiers(source, SpecScan::DynamicImport) {
        let start = cap.start;
        let end = cap.end;
        if let Some(spec) = extract_quoted(&source[cap.start..cap.end]) {
            dynamic_imports.push(CasImportRecord {
                specifier: spec,
                start,
                end,
                kind: crate::cas::ImportKind::DynamicImport,
            });
        }
    }

    // CJS requires: require("...")
    for cap in find_specifiers(source, SpecScan::Require) {
        let start = cap.start;
        let end = cap.end;
        if let Some(spec) = extract_quoted(&source[cap.start..cap.end]) {
            requires.push(CasImportRecord {
                specifier: spec,
                start,
                end,
                kind: crate::cas::ImportKind::Require,
            });
        }
    }

    Ok((imports, reexports, dynamic_imports, requires))
}

/// Extract the first quoted string from a substring.
fn extract_quoted(slice: &str) -> Option<String> {
    for (i, ch) in slice.char_indices() {
        if ch == '\'' || ch == '"' {
            if let Some(end) = slice[i + 1..].find(ch) {
                return Some(slice[i + 1..i + 1 + end].to_string());
            }
        }
    }
    None
}

#[cfg(feature = "swc-backend")]
fn extract_with_swc(
    source: &str,
    file_type: FileType,
) -> BundleResult<(
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
)> {
    use crate::errors::BundleError;
    use swc_core::common::{comments::Comments, sync::Lrc, FileName, SourceMap, Spanned};
    use swc_core::ecma::parser::{EsSyntax, Parser, StringInput, Syntax, TsSyntax};

    let cm = Lrc::<SourceMap>::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom("inline".into())),
        source.to_string(),
    );

    let syntax = match file_type {
        FileType::TypeScript => Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: true,
            ..Default::default()
        }),
        FileType::Tsx => Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: true,
            ..Default::default()
        }),
        FileType::Jsx => Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        _ => unreachable!(),
    };

    let mut parser = Parser::new(syntax, StringInput::from(&*fm), None::<&dyn Comments>);
    let module = parser.parse_module().map_err(|e| BundleError::ParseError {
        path: "inline".to_string(),
        line: cm.lookup_char_pos(e.span().lo).line,
        column: cm.lookup_char_pos(e.span().lo).col_display + 1,
        message: format!("{:?}", e),
    })?;

    let mut imports = Vec::new();
    let mut reexports = Vec::new();
    let mut dynamic_imports = Vec::new();
    let mut requires = Vec::new();

    for item in &module.body {
        match item {
            swc_core::ecma::ast::ModuleItem::ModuleDecl(decl) => match decl {
                swc_core::ecma::ast::ModuleDecl::Import(imp) => {
                    let specifier = imp.src.value.to_string_lossy().to_string();
                    let start = imp.span.lo.0 as usize;
                    let end = imp.span.hi.0 as usize;
                    imports.push(CasImportRecord {
                        specifier,
                        start,
                        end,
                        kind: crate::cas::ImportKind::StaticImport,
                    });
                }
                swc_core::ecma::ast::ModuleDecl::ExportNamed(named) => {
                    if let Some(src) = &named.src {
                        let specifier = src.value.to_string_lossy().to_string();
                        let start = named.span.lo.0 as usize;
                        let end = named.span.hi.0 as usize;
                        reexports.push(CasImportRecord {
                            specifier,
                            start,
                            end,
                            kind: crate::cas::ImportKind::ReExport,
                        });
                    }
                }
                swc_core::ecma::ast::ModuleDecl::ExportAll(all) => {
                    let specifier = all.src.value.to_string_lossy().to_string();
                    let start = all.span.lo.0 as usize;
                    let end = all.span.hi.0 as usize;
                    reexports.push(CasImportRecord {
                        specifier,
                        start,
                        end,
                        kind: crate::cas::ImportKind::ReExport,
                    });
                }
                swc_core::ecma::ast::ModuleDecl::ExportDefaultDecl(_)
                | swc_core::ecma::ast::ModuleDecl::ExportDefaultExpr(_)
                | swc_core::ecma::ast::ModuleDecl::TsExportAssignment(_) => {}
                _ => {}
            },
            swc_core::ecma::ast::ModuleItem::Stmt(stmt) => {
                // Check for dynamic import in expression statements
                if let swc_core::ecma::ast::Stmt::Expr(expr_stmt) = stmt {
                    if let swc_core::ecma::ast::Expr::Call(call) = &*expr_stmt.expr {
                        if matches!(call.callee, swc_core::ecma::ast::Callee::Import(_)) {
                            if let Some(arg) = call.args.first() {
                                if let swc_core::ecma::ast::Expr::Lit(
                                    swc_core::ecma::ast::Lit::Str(s),
                                ) = &*arg.expr
                                {
                                    let specifier = s.value.to_string_lossy().to_string();
                                    let start = call.span.lo.0 as usize;
                                    let end = call.span.hi.0 as usize;
                                    dynamic_imports.push(CasImportRecord {
                                        specifier,
                                        start,
                                        end,
                                        kind: crate::cas::ImportKind::DynamicImport,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok((imports, reexports, dynamic_imports, requires))
}

#[cfg(not(feature = "swc-backend"))]
fn extract_with_swc(
    _source: &str,
    _file_type: FileType,
) -> BundleResult<(
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
    Vec<CasImportRecord>,
)> {
    extract_with_regex(_source)
}

// ── Export extraction ─────────────────────────────────────────────────────────

/// Extract export records and compute an AST hash (used for transform invalidation).
fn extract_export_records(
    source: &str,
    file_type: FileType,
) -> BundleResult<(Vec<crate::cas::ExportRecord>, u64)> {
    let mut exports = Vec::new();
    let mut export_source = String::new();

    for decl in find_export_declarations(source) {
        exports.push(crate::cas::ExportRecord {
            name: decl.name.clone(),
            alias: decl.alias.clone(),
            source: decl.source.clone(),
        });
        export_source.push_str(&decl.name);
        if let Some(alias) = &decl.alias {
            export_source.push_str(" as ");
            export_source.push_str(alias);
        }
        if let Some(src) = &decl.source {
            export_source.push_str(" from ");
            export_source.push_str(src);
        }
        export_source.push('\n');
    }

    // AST hash: combine content hash with export structure. Changes when exports
    // change even if the content is otherwise identical.
    let ast_hash = xxhash_rust::xxh3::xxh3_64(export_source.as_bytes());

    let _ = file_type;
    Ok((exports, ast_hash))
}

// ── Side effect detection ──────────────────────────────────────────────────────

/// Detect if a module has top-level side effects.
/// Conservative: returns true unless we can prove the module is pure.
/// A module is pure if:
///   - All exports are simple values (no side-effect imports)
///   - No top-level calls to potentially-effectful APIs
const SIDE_EFFECT_PATTERNS: &[&str] = &[
    "new ",
    "require(",
    "import(",
    "fetch(",
    "console.",
    "document.",
    "window.",
    "globalThis.",
    "localStorage",
    "sessionStorage",
    "setTimeout",
    "setInterval",
    "requestAnimationFrame",
    "process.",
    "Buffer.",
];

fn detect_side_effects(source: &str, imports: &[CasImportRecord]) -> bool {
    // Side-effect-only imports (no named imports, just `import 'foo'`)
    for import in imports {
        let import_text = &source[import.start.min(source.len())..import.end.min(source.len())];
        if !import_text.contains('{') && !import_text.contains(" as ") {
            // Could be `import 'foo'` — conservatively treat as side effect
            // unless we can verify it's a bare specifier with no bindings
            let has_from = import_text.contains(" from ");
            if !has_from {
                return true;
            }
        }
    }

    // Scan for side-effect patterns
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("import ")
            || trimmed.starts_with("export ")
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
        {
            continue;
        }
        for pattern in SIDE_EFFECT_PATTERNS {
            if trimmed.contains(pattern) {
                return true;
            }
        }
    }

    false
}

// ── Regex helper ──────────────────────────────────────────────────────────────

#[derive(Debug)]
struct RegexMatch {
    start: usize,
    end: usize,
}

/// Which construct a specifier scan is looking for. The kind determines the
/// *structure* that must appear between the keyword and the quoted specifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecScan {
    /// `import "s"` (bare) or `import x from "s"`.
    StaticImport,
    /// `export { a } from "s"` / `export * from "s"`.
    ReExport,
    /// `import("s")`.
    DynamicImport,
    /// `require("s")`.
    Require,
}

/// A single export declaration found in source.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportDecl {
    name: String,
    alias: Option<String>,
    source: Option<String>,
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// True when `text[pos..pos+len]` is not glued to an identifier character, so
/// `important`/`exports`/`required` are not mistaken for keywords.
fn word_boundary(text: &str, pos: usize, len: usize) -> bool {
    let b = text.as_bytes();
    let before_ok = pos == 0 || !is_ident_byte(b[pos - 1]);
    let after = pos + len;
    let after_ok = after >= b.len() || !is_ident_byte(b[after]);
    before_ok && after_ok
}

/// Byte mask: `true` where a byte is real source code. String bodies, template
/// *literal* text and comments are `false`; an *opening* quote stays `true` so
/// scanners can locate specifier starts. Without this, keywords inside strings or
/// comments (`const s = "import x"`) were treated as imports.
///
/// Template interpolations are code: `` `${import("./x")}` `` yields a dynamic
/// import, while the literal text around it (`` `import "nope"` ``) does not.
fn code_mask(text: &str) -> Vec<bool> {
    let mut mask = vec![true; text.len()];
    let mut i = 0usize;
    scan_code(text, &mut i, None, &mut mask);
    mask
}

/// Scan code until `terminator` (a closing quote) or EOF.
///
/// With `terminator = None` this is top-level/interpolation code, where strings,
/// templates and comments are handled recursively.
fn scan_code(text: &str, i: &mut usize, terminator: Option<u8>, mask: &mut [bool]) {
    let bytes = text.as_bytes();
    let n = bytes.len();

    if let Some(t) = terminator {
        while *i < n {
            if bytes[*i] == b'\\' {
                mask[*i] = false;
                *i += 1;
                if *i < n {
                    mask[*i] = false;
                    *i += 1;
                }
                continue;
            }
            if bytes[*i] == t {
                mask[*i] = false;
                *i += 1;
                return;
            }
            mask[*i] = false;
            *i += 1;
        }
        return;
    }

    while *i < n {
        match bytes[*i] {
            quote @ (b'\'' | b'"') => {
                *i += 1;
                scan_code(text, i, Some(quote), mask);
            }
            b'`' => {
                *i += 1;
                scan_template(text, i, mask);
            }
            b'/' if *i + 1 < n && bytes[*i + 1] == b'/' => {
                while *i < n && bytes[*i] != b'\n' {
                    mask[*i] = false;
                    *i += 1;
                }
            }
            b'/' if *i + 1 < n && bytes[*i + 1] == b'*' => {
                mask[*i] = false;
                mask[*i + 1] = false;
                *i += 2;
                while *i < n {
                    if *i + 1 < n && bytes[*i] == b'*' && bytes[*i + 1] == b'/' {
                        mask[*i] = false;
                        mask[*i + 1] = false;
                        *i += 2;
                        break;
                    }
                    mask[*i] = false;
                    *i += 1;
                }
            }
            _ => *i += 1,
        }
    }
}

/// Scan a template literal body, treating `${ ... }` interpolations as code.
fn scan_template(text: &str, i: &mut usize, mask: &mut [bool]) {
    let bytes = text.as_bytes();
    let n = bytes.len();
    while *i < n {
        match bytes[*i] {
            b'\\' => {
                mask[*i] = false;
                *i += 1;
                if *i < n {
                    mask[*i] = false;
                    *i += 1;
                }
            }
            b'`' => {
                mask[*i] = false;
                *i += 1;
                return;
            }
            b'$' if *i + 1 < n && bytes[*i + 1] == b'{' => {
                mask[*i] = false; // the `$` is literal template text
                *i += 1; // the `{` stays code
                let mut depth: i32 = 0;
                let mut j = *i;
                while j < n {
                    let c = bytes[j];
                    match c {
                        b'{' => {
                            depth += 1;
                            j += 1;
                        }
                        b'}' => {
                            depth -= 1;
                            j += 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        quote @ (b'\'' | b'"') => {
                            j += 1;
                            scan_code(text, &mut j, Some(quote), mask);
                        }
                        b'`' => {
                            j += 1;
                            scan_template(text, &mut j, mask);
                        }
                        b'/' if j + 1 < n && bytes[j + 1] == b'/' => {
                            while j < n && bytes[j] != b'\n' {
                                mask[j] = false;
                                j += 1;
                            }
                        }
                        b'/' if j + 1 < n && bytes[j + 1] == b'*' => {
                            mask[j] = false;
                            mask[j + 1] = false;
                            j += 2;
                            while j < n {
                                if j + 1 < n && bytes[j] == b'*' && bytes[j + 1] == b'/' {
                                    mask[j] = false;
                                    mask[j + 1] = false;
                                    j += 2;
                                    break;
                                }
                                mask[j] = false;
                                j += 1;
                            }
                        }
                        _ => j += 1,
                    }
                }
                *i = j;
            }
            _ => {
                mask[*i] = false;
                *i += 1;
            }
        }
    }
}

/// Find import specifiers, honouring the construct's real structure.
///
/// The previous implementation searched for the bare keyword and then took the
/// first quoted string after it, so `export const c = "dead";` was read as a
/// re-export of `dead` (module not found), string literals became imports, and
/// every static import was also collected as a dynamic import.
fn find_specifiers(text: &str, scan: SpecScan) -> Vec<RegexMatch> {
    let mask = code_mask(text);
    let is_code = |i: usize| mask.get(i).copied().unwrap_or(false);
    let bytes = text.as_bytes();

    let (keyword, allow_bare, allow_from, allow_call) = match scan {
        // `import "s"` or `import x from "s"`
        SpecScan::StaticImport => ("import", true, true, false),
        // `export ... from "s"` (never bare)
        SpecScan::ReExport => ("export", false, true, false),
        // `import("s")`
        SpecScan::DynamicImport => ("import", false, false, true),
        // `require("s")`
        SpecScan::Require => ("require", false, false, true),
    };

    let mut results = Vec::new();
    let mut search_start = 0usize;
    while let Some(kw_pos) = text[search_start..].find(keyword) {
        let abs_pos = search_start + kw_pos;
        search_start = abs_pos + keyword.len();

        if !is_code(abs_pos) || !word_boundary(text, abs_pos, keyword.len()) {
            continue;
        }

        // Walk forward to the first code-position quote, recording whether the
        // structural token (`from` / `(`) appeared first.
        let mut i = abs_pos + keyword.len();
        let limit = (i + 1024).min(text.len());
        let mut saw_from = false;
        let mut saw_call = false;
        let mut saw_other_code = false;
        let mut quote_at = None;
        let mut depth: i32 = 0;
        while i < limit {
            let b = bytes[i];
            if b == b'\'' || b == b'"' {
                if is_code(i) {
                    quote_at = Some(i);
                }
                break;
            }
            if !is_code(i) {
                // Entered a comment or an unrelated string — stop scanning.
                break;
            }
            match b {
                b'(' => {
                    depth += 1;
                    saw_call = true;
                }
                b')' => depth -= 1,
                b'{' => depth += 1,
                b'}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                b';' if depth == 0 => break,
                b'\n' if depth == 0 && saw_from => break,
                _ => {}
            }
            if !saw_from && text[i..].starts_with("from") && word_boundary(text, i, 4) {
                saw_from = true;
            }
            if !b.is_ascii_whitespace() {
                saw_other_code = true;
            }
            i += 1;
        }

        let Some(quote) = quote_at else { continue };
        let bare = !saw_other_code;
        if !((allow_bare && bare) || (allow_from && saw_from) || (allow_call && saw_call)) {
            continue;
        }

        // Find the closing quote (honouring escapes).
        let quote_byte = bytes[quote];
        let mut close = quote + 1;
        let mut found_close = None;
        while close < text.len() {
            if bytes[close] == b'\\' {
                close += 2;
                continue;
            }
            if bytes[close] == quote_byte {
                found_close = Some(close);
                break;
            }
            close += 1;
        }
        let Some(close) = found_close else { continue };

        // Statements extend to their terminator so the transformer can replace
        // the whole construct (same behaviour as the old scanner).
        let end = if matches!(scan, SpecScan::Require) {
            close + 1
        } else {
            let rest = &text[close + 1..];
            rest.find(';')
                .map(|p| close + 1 + p + 1)
                .or_else(|| rest.find('\n').map(|p| close + 1 + p))
                .unwrap_or(close + 1)
                .min(text.len())
        };

        results.push(RegexMatch {
            start: abs_pos,
            end,
        });
        search_start = close + 1;
    }

    results
}

/// Find export declarations: `export default ...`, `export <decl> NAME`, and
/// `export { a, b as c } [from "s"]`.
///
/// The old implementation reused the import scanner, which required a quoted
/// string — so for plain JS files *no* exports were ever detected, leaving the
/// tree-shaker with an empty export set.
fn find_export_declarations(text: &str) -> Vec<ExportDecl> {
    let mask = code_mask(text);
    let is_code = |i: usize| mask.get(i).copied().unwrap_or(false);
    let mut out = Vec::new();
    let mut search_start = 0usize;

    while let Some(rel) = text[search_start..].find("export") {
        let pos = search_start + rel;
        search_start = pos + "export".len();
        if !is_code(pos) || !word_boundary(text, pos, "export".len()) {
            continue;
        }

        let rest = text[pos + "export".len()..].trim_start();
        let off = text.len() - rest.len();

        // export default ...
        if rest.starts_with("default") && word_boundary(text, off, 7) {
            out.push(ExportDecl {
                name: "default".to_string(),
                alias: None,
                source: None,
            });
            continue;
        }

        // export { a, b as c } [from "s"]
        if rest.starts_with('{') {
            let Some(close) = rest.find('}').map(|p| off + p) else {
                continue;
            };
            let source = {
                let after = text[close + 1..].trim_start();
                if let Some(s) = after.strip_prefix("from") {
                    let a = s.trim_start();
                    if let Some(q) = a.find(['\'', '"']) {
                        let quote_byte = a.as_bytes()[q];
                        let body = &a[q + 1..];
                        body.find(quote_byte as char).map(|e| body[..e].to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            for part in text[off + 1..close].split(',') {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                let (name, alias) = match p.split_once(" as ") {
                    Some((n, a)) => (n.trim().to_string(), Some(a.trim().to_string())),
                    None => (p.to_string(), None),
                };
                out.push(ExportDecl {
                    name,
                    alias,
                    source: source.clone(),
                });
            }
            continue;
        }

        // export [async] const|let|var|function|class|interface|type|enum NAME
        let mut decl_rest = rest;
        if let Some(r) = decl_rest.strip_prefix("async ") {
            decl_rest = r.trim_start();
        }
        for kw in [
            "const",
            "let",
            "var",
            "function",
            "class",
            "interface",
            "type",
            "enum",
        ] {
            if decl_rest.starts_with(kw)
                && word_boundary(text, text.len() - decl_rest.len(), kw.len())
            {
                let after = decl_rest[kw.len()..].trim_start();
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                    .collect();
                if !name.is_empty() {
                    out.push(ExportDecl {
                        name,
                        alias: None,
                        source: None,
                    });
                }
                break;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_imports_js() {
        let source = r#"
            import React from 'react';
            import { useState, useEffect } from 'react';
            import * as _ from 'lodash';
            import './styles.css';
            export const x = 1;
        "#;
        let (imports, reexports, dynamic, requires) = extract_with_regex(source).unwrap();
        assert_eq!(imports.len(), 4);
        assert_eq!(reexports.len(), 0);
        assert_eq!(dynamic.len(), 0); // no dynamic imports in this test input
        assert_eq!(requires.len(), 0);
        assert!(imports.iter().any(|i| i.specifier == "react"));
        assert!(imports.iter().any(|i| i.specifier == "lodash"));
        assert!(imports.iter().any(|i| i.specifier == "./styles.css"));
    }

    #[test]
    fn template_interpolation_is_code() {
        // Dynamic import inside `${}` must be found...
        let source = "const m = `value: ${await import('./dyn.js')}`;";
        let (_, _, dynamic, _) = extract_with_regex(source).unwrap();
        assert_eq!(dynamic.len(), 1, "dynamic: {:?}", dynamic);
        assert_eq!(dynamic[0].specifier, "./dyn.js");

        // ...but template *literal* text must not be scanned as code.
        let literal = "const t = `import x from 'nope';`;\nconst s = 1;";
        let (imports, reexports, dynamic, requires) = extract_with_regex(literal).unwrap();
        assert!(imports.is_empty(), "imports: {:?}", imports);
        assert!(reexports.is_empty());
        assert!(dynamic.is_empty(), "dynamic: {:?}", dynamic);
        assert!(requires.is_empty());
    }

    #[test]
    fn nested_template_and_string_interpolations() {
        let source = "const a = `x${ `y${import('./deep.js')}` }z`;\nconst b = 'it\\'s';\nconst c = import(\"plain\");";
        let (_, _, dynamic, _) = extract_with_regex(source).unwrap();
        let specs: Vec<&str> = dynamic.iter().map(|d| d.specifier.as_str()).collect();
        assert!(specs.contains(&"./deep.js"), "specs: {:?}", specs);
        assert!(specs.contains(&"plain"), "specs: {:?}", specs);
        assert_eq!(specs.len(), 2, "specs: {:?}", specs);
    }

    #[test]
    fn string_literal_is_not_an_import() {
        // Regression: `export const c = "dead"` used to be read as a re-export of
        // `dead`, and bare strings used to become imports.
        let source = "export const c=\"dead\";\nconst note = 'import x from \"nope\"';\n";
        let (imports, reexports, dynamic, requires) = extract_with_regex(source).unwrap();
        assert!(imports.is_empty(), "imports: {:?}", imports);
        assert!(reexports.is_empty(), "reexports: {:?}", reexports);
        assert!(dynamic.is_empty());
        assert!(requires.is_empty());
    }

    #[test]
    fn keyword_inside_comment_or_identifier_is_ignored() {
        let source = "// import x from 'commented'\nconst important = 1;\nexports.foo = 2;\n/* require('block') */\n";
        let (imports, reexports, dynamic, requires) = extract_with_regex(source).unwrap();
        assert!(imports.is_empty(), "imports: {:?}", imports);
        assert!(reexports.is_empty());
        assert!(dynamic.is_empty(), "dynamic: {:?}", dynamic);
        assert!(requires.is_empty(), "requires: {:?}", requires);
    }

    #[test]
    fn distinguishes_static_dynamic_and_require() {
        let source = "import a from './a.js';\nconst b = await import('./b.js');\nconst c = require('./c.js');\nexport { a } from './d.js';\n";
        let (imports, reexports, dynamic, requires) = extract_with_regex(source).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].specifier, "./a.js");
        assert_eq!(dynamic.len(), 1, "dynamic: {:?}", dynamic);
        assert_eq!(dynamic[0].specifier, "./b.js");
        assert_eq!(requires.len(), 1);
        assert_eq!(requires[0].specifier, "./c.js");
        assert_eq!(reexports.len(), 1);
        assert_eq!(reexports[0].specifier, "./d.js");
    }

    #[test]
    fn extracts_exports_from_plain_js() {
        // Regression: the export scanner previously required a quoted string, so
        // plain-JS exports were never collected.
        let source = "export const a = 1;\nexport function b() {}\nexport default 42;\nexport { c, d as e };\nconst z = 1;\nexport { z } from './z.js';\n";
        let (exports, _hash) = extract_export_records(source, FileType::JavaScript).unwrap();
        let names: Vec<&str> = exports.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a"), "names: {:?}", names);
        assert!(names.contains(&"b"), "names: {:?}", names);
        assert!(names.contains(&"default"), "names: {:?}", names);
        assert!(names.contains(&"c"), "names: {:?}", names);
        assert!(names.contains(&"d"), "names: {:?}", names);
        assert!(names.contains(&"z"), "names: {:?}", names);
        let aliased = exports.iter().find(|e| e.name == "d").unwrap();
        assert_eq!(aliased.alias.as_deref(), Some("e"));
        let from = exports.iter().find(|e| e.name == "z").unwrap();
        assert_eq!(from.source.as_deref(), Some("./z.js"));
    }

    #[test]
    fn test_extract_dynamic_import() {
        let source = r#"const mod = await import('./dynamic.js');"#;
        let (imports, _, dynamic, _) = extract_with_regex(source).unwrap();
        assert_eq!(dynamic.len(), 1); // regex correctly finds await import()
        assert!(dynamic.iter().any(|i| i.specifier == "./dynamic.js"));
    }

    #[test]
    fn test_side_effect_detection() {
        let pure = "const x = 1; export { x };";
        assert!(!detect_side_effects(pure, &[]));

        let impure = "console.log('hi'); const x = 1; export { x };";
        assert!(detect_side_effects(impure, &[]));
    }

    #[test]
    fn test_regex_find_iter() {
        let source = "import React from 'react'; import { x } from 'y';";
        let matches = find_specifiers(source, SpecScan::StaticImport);
        assert_eq!(matches.len(), 2);
        assert_eq!(
            &source[matches[0].start..matches[0].end],
            "import React from 'react';"
        );
        assert_eq!(
            &source[matches[1].start..matches[1].end],
            "import { x } from 'y';"
        );
    }
}
