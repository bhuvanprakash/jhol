//! jhol fmt — native, cache-first formatter engines.
//!
//! Design: engine adapters over mature Rust parsers, not a mega-parser.
//! swc for JS/TS/JSX/TSX, serde_json for JSON, external ecosystem formatters
//! (rustfmt/gofmt/ruff) for the rest.
//!
//! Prettier option *names* are respected because they are the de-facto spec;
//! the engines, the cache-first design, and the normalization passes are
//! jhol's own.

use crate::errors::{BundleError, BundleResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Engine identity: crate version AND a semantic digest of the passes.
/// Bumping the crate version alone is NOT enough — the cache must die when
/// output semantics change without a release (proven: comment/quote/spacing
/// passes were added while a stale cache kept serving old-forever output).
pub const FMT_ENGINE: &str = concat!("jhol-fmt-", env!("CARGO_PKG_VERSION"), ".sem6");

// ── Config ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FmtConfig {
    pub print_width: usize,
    pub tab_width: usize,
    pub use_tabs: bool,
    pub semi: bool,
    pub single_quote: bool,
    pub trailing_comma: String,
}

impl Default for FmtConfig {
    fn default() -> Self {
        Self {
            print_width: 80,
            tab_width: 2,
            use_tabs: false,
            semi: true,
            single_quote: false,
            trailing_comma: "all".to_string(),
        }
    }
}

impl FmtConfig {
    /// Stable digest of every option, so the fmt-cache invalidates on any
    /// config change (the engine string alone can't see option edits).
    pub fn digest(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.print_width.hash(&mut h);
        self.tab_width.hash(&mut h);
        self.use_tabs.hash(&mut h);
        self.semi.hash(&mut h);
        self.single_quote.hash(&mut h);
        self.trailing_comma.hash(&mut h);
        h.finish()
    }
}

pub fn load_config(dir: &Path) -> FmtConfig {
    let mut cfg = FmtConfig::default();
    for name in [".prettierrc", ".prettierrc.json"] {
        if let Some(c) = read_json_config(&dir.join(name)) {
            merge_json(&mut cfg, &c);
        }
    }
    if let Some(c) = read_json_config(&dir.join(".jholrc"))
        .and_then(|v| v.get("fmt").and_then(|f| serde_json::to_value(f).ok()))
    {
        merge_json(&mut cfg, &c);
    }
    cfg
}

fn read_json_config(path: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn merge_json(cfg: &mut FmtConfig, v: &serde_json::Value) {
    let Some(obj) = v.as_object() else {
        return;
    };
    for (k, val) in obj {
        if let Some(n) = val.as_u64() {
            match k.as_str() {
                "printWidth" | "print_width" => cfg.print_width = n as usize,
                "tabWidth" | "tab_width" => cfg.tab_width = n as usize,
                _ => {}
            }
        }
        if let Some(b) = val.as_bool() {
            match k.as_str() {
                "useTabs" | "use_tabs" => cfg.use_tabs = b,
                "semi" => cfg.semi = b,
                "singleQuote" | "single_quote" => cfg.single_quote = b,
                _ => {}
            }
        }
        if let Some(s) = val.as_str() {
            if k == "trailingComma" || k == "trailing_comma" {
                cfg.trailing_comma = s.to_string();
            }
        }
    }
}

// ── JS / TS ────────────────────────────────────────────────────────────

#[cfg(feature = "swc-backend")]
pub fn format_js(source: &str, filename: &str, cfg: &FmtConfig) -> BundleResult<String> {
    use swc_core::common::{
        comments::SingleThreadedComments, sync::Lrc, FileName, Globals, SourceMap, GLOBALS,
    };
    use swc_core::ecma::{
        codegen::{text_writer::JsWriter, Emitter},
        parser::{EsSyntax, Parser, StringInput, Syntax, TsSyntax},
    };

    let cm = Lrc::<SourceMap>::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom(filename.to_string())),
        source.to_string(),
    );

    // Comments must be captured by the parser AND handed to the emitter, or
    // swc drops every comment on re-print (Prettier/Bun preserve them).
    let comments: Lrc<SingleThreadedComments> = Default::default();
    let syntax = syntax_for(filename);
    let mut parser = Parser::new(syntax, StringInput::from(&*fm), Some(&comments));
    let program = parser.parse_program().map_err(|e| {
        BundleError::TransformError(filename.to_string(), format!("Fmt parse error: {:?}", e))
    })?;

    GLOBALS.set(&Globals::new(), || {
        let mut buf = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: swc_core::ecma::codegen::Config::default(),
                cm: cm.clone(),
                comments: Some(&comments),
                wr: Box::new(JsWriter::new(cm.clone(), "\n", &mut buf, None)),
            };
            match program {
                swc_core::ecma::ast::Program::Module(m) => emitter.emit_module(&m),
                swc_core::ecma::ast::Program::Script(s) => emitter.emit_script(&s),
            }
            .map_err(|e| {
                BundleError::TransformError(
                    filename.to_string(),
                    format!("Fmt codegen error: {:?}", e),
                )
            })?;
        }
        let raw = String::from_utf8(buf).unwrap_or_default();
        let formatted = post_process_js(&apply_indent(&raw, cfg), cfg, &mut source_facts(source));
        Ok(join_pure_annotations(&normalize_var_breaks(
            &formatted,
            cfg.print_width,
        )))
    })
}

/// Keep `/* @__PURE__ */` and `/*#__PURE__*/` on the same line as the
/// expression they annotate.
///
/// swc's emitter and this file's own line splitting can both move a block
/// comment onto its own line. For a tree-shaking annotation that is not
/// cosmetic: bundlers match the annotation to the *following* call expression,
/// and a detached comment silently disables dead-code elimination for that call.
fn join_pure_annotations(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_end();
        let is_pure = trimmed.ends_with("*/")
            && (trimmed.contains("@__PURE__") || trimmed.contains("#__PURE__"));
        if is_pure && i + 1 < lines.len() {
            let next = lines[i + 1].trim_start();
            if !next.is_empty() {
                out.push_str(trimmed);
                out.push(' ');
                out.push_str(next);
                out.push('\n');
                i += 2;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    out
}

#[cfg(not(feature = "swc-backend"))]
pub fn format_js(_source: &str, filename: &str, _cfg: &FmtConfig) -> BundleResult<String> {
    Err(BundleError::TransformError(
        filename.to_string(),
        "swc-backend feature disabled; build with default features".to_string(),
    ))
}

#[cfg(feature = "swc-backend")]
fn syntax_for(filename: &str) -> swc_core::ecma::parser::Syntax {
    use swc_core::ecma::parser::{EsSyntax, TsSyntax};
    if filename.ends_with(".ts") || filename.ends_with(".mts") || filename.ends_with(".cts") {
        swc_core::ecma::parser::Syntax::Typescript(TsSyntax::default())
    } else if filename.ends_with(".tsx") {
        swc_core::ecma::parser::Syntax::Typescript(TsSyntax {
            tsx: true,
            ..TsSyntax::default()
        })
    } else if filename.ends_with(".jsx") {
        // JSX lives in EsSyntax on this swc version; without the flag a .jsx
        // file fails to parse and `jhol fmt` reports an error instead of
        // formatting it.
        swc_core::ecma::parser::Syntax::Es(EsSyntax {
            jsx: true,
            ..EsSyntax::default()
        })
    } else {
        swc_core::ecma::parser::Syntax::Es(EsSyntax::default())
    }
}

/// Facts extracted from the ORIGINAL source that swc's codegen destroys
/// and Prettier/Bun semantics depend on:
/// - objects whose `{` is followed by a newline stay expanded (never collapse)
/// - comments that start their own source line stay on their own line
/// - blank lines between top-level statements are preserved (max one)
#[derive(Debug, Default)]
pub struct SourceFacts {
    multiline_obj_prefixes: std::collections::HashSet<String>,
    /// comment text -> whether a blank line preceded it in the source
    own_line_comments: std::collections::HashMap<String, bool>,
    /// first ~24 chars of top-level statements that followed a blank line
    blank_before_stmts: std::collections::HashSet<String>,
}

/// Scan raw source for multiline object prefixes, own-line comments, and
/// blank-line-preceded top-level statements.
fn source_facts(source: &str) -> SourceFacts {
    let mut f = SourceFacts::default();
    let mut in_block = false;
    let mut was_blank = false;
    let mut depth = 0i32;
    for line in source.lines() {
        let t = line.trim();
        if in_block {
            if let Some(end) = line.find("*/") {
                in_block = line[end + 2..].contains("/*");
            }
            was_blank = false;
            continue;
        }
        if t.is_empty() {
            was_blank = true;
            continue;
        }
        if t.starts_with("//") {
            f.own_line_comments.insert(t.to_string(), was_blank);
        } else if t.starts_with("/*") {
            match t.find("*/") {
                Some(e) => {
                    f.own_line_comments
                        .insert(t[..e + 2].to_string(), was_blank);
                }
                None => {
                    f.own_line_comments.insert(t.to_string(), was_blank);
                    in_block = true;
                }
            }
        }
        if depth == 0 && was_blank && !t.starts_with("//") && !t.starts_with("/*") {
            let key: String = t.chars().take(24).collect();
            f.blank_before_stmts.insert(key);
        }
        if let Some(brace) = line.rfind('{') {
            let after = line[brace + 1..].trim();
            if after.is_empty() || after.starts_with("//") || after.starts_with("/*") {
                let prefix = line[..brace].trim();
                if !prefix.is_empty() {
                    f.multiline_obj_prefixes.insert(prefix.to_string());
                }
            }
        }
        for ch in line.chars() {
            match ch {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth -= 1,
                _ => {}
            }
        }
        was_blank = false;
    }
    f
}

// ── Post-processing passes ──────────────────────────────────────────────

/// swc emits objects broken across lines (or always-inline); re-collapse the
/// ones that fit in `print_width` (Prettier/Bun style), and add trailing
/// commas to multiline objects when `trailing_comma != "none"`.
fn post_process_js(src: &str, cfg: &FmtConfig, facts: &mut SourceFacts) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_end();
        if trimmed.ends_with('{') && is_object_literal_start(&lines, i) {
            if let Some(end) = find_matching_brace(&lines, i) {
                let obj_lines: Vec<&str> = lines[i..=end].to_vec();
                if let Some(collapsed) = try_collapse_object(&obj_lines, cfg.print_width) {
                    // Prettier/Bun preserve source-multiline objects: if the
                    // prefix had a newline after `{` in the original source,
                    // keep it expanded (fall through to trailing-comma path).
                    if !facts
                        .multiline_obj_prefixes
                        .contains(prefix_of(&obj_lines[0]))
                    {
                        out.push(collapsed);
                        i = end + 1;
                        continue;
                    }
                }
                // Too wide to collapse: ensure trailing comma before close
                if end > i && cfg.trailing_comma != "none" {
                    let close = obj_lines.last().unwrap().trim_start();
                    if close == "}" || close == "};" {
                        let mut props: Vec<String> = Vec::new();
                        for l in &obj_lines[..obj_lines.len() - 1] {
                            if !l.trim().is_empty() {
                                props.push(l.trim_end().to_string());
                            }
                        }
                        let mut ok = true;
                        if let Some(last) = props.last_mut() {
                            if !last.ends_with(',') {
                                last.push(',');
                            } else {
                                ok = false;
                            }
                        } else {
                            ok = false;
                        }
                        if ok {
                            out.extend(props);
                            out.push((*obj_lines.last().unwrap()).to_string());
                            i = end + 1;
                            continue;
                        }
                    }
                }
            }
        }
        out.push(lines[i].to_string());
        i += 1;
    }

    let mut result = out.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    normalize_spacing(&result, facts)
}

/// Prefix text of a swc object-open line (everything before the final `{`).
fn prefix_of(first: &str) -> &str {
    let cur = first.trim_end();
    cur[..cur.len() - 1].trim()
}

/// Object-start detection: `=`, `(`, `,`, `:`, `return`, `=>`, or `[`
/// precedes the final `{` (expression context, not a block).
fn is_object_literal_start(lines: &[&str], i: usize) -> bool {
    let cur = lines[i].trim_end();
    let before = cur[..cur.len() - 1].trim_end();
    before.ends_with('=')
        || before.ends_with('(')
        || before.ends_with(',')
        || before.ends_with(':')
        || before.ends_with("return")
        || before.ends_with("=>")
        || before.ends_with('[')
}

/// Find the matching close brace line for an object starting at `start`.
fn find_matching_brace(lines: &[&str], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(offset);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Collapse a multi-line object into one line if it fits within `max_width`.
/// Handles nested multi-line objects recursively (one level per call).
fn try_collapse_object(obj_lines: &[&str], max_width: usize) -> Option<String> {
    if obj_lines.len() < 3 {
        return None;
    }
    let first = obj_lines[0].trim_end();
    if !first.ends_with('{') {
        return None;
    }
    let prefix = first[..first.len() - 1].trim();

    let last = obj_lines.last().unwrap().trim();
    if !last.starts_with('}') {
        return None;
    }
    let leading = obj_lines
        .last()
        .unwrap()
        .chars()
        .take_while(|c| *c == ' ')
        .count();
    let indent = " ".repeat(leading);
    let suffix = if last.len() > 1 { &last[1..] } else { "" };

    let mut props: Vec<String> = Vec::new();
    let mut idx = 1;
    while idx < obj_lines.len() - 1 {
        let t = obj_lines[idx].trim();
        if t.is_empty() {
            return None;
        }
        if t.ends_with('{') {
            // Nested object: collapse it first, then splice inline
            let sub_end = find_matching_brace(obj_lines, idx)?;
            if sub_end >= obj_lines.len() - 1 {
                return None;
            }
            let sub = &obj_lines[idx..=sub_end];
            let mut collapsed_sub =
                try_collapse_object(sub, max_width.saturating_sub(indent.len() + 6))?
                    .trim_start()
                    .to_string();
            if collapsed_sub.ends_with(',') {
                collapsed_sub.pop();
            }
            props.push(collapsed_sub);
            idx = sub_end + 1;
        } else {
            if t.contains('[') || t.contains(']') {
                return None;
            }
            if !t.contains(':') {
                return None;
            }
            let mut p = t.to_string();
            if p.ends_with(',') {
                p.pop();
            }
            props.push(p);
            idx += 1;
        }
    }

    let body = props.join(", ");
    let joined = if body.is_empty() {
        format!("{} {{}}{}", prefix, suffix)
    } else {
        format!("{} {{ {} }}{}", prefix, body, suffix)
    };
    if indent.len() + joined.len() <= max_width {
        Some(format!("{}{}", indent, joined))
    } else {
        None
    }
}

/// swc's codegen has style quirks Prettier/Bun don't: missing space after
/// keywords (`for(`), missing space before `{` (`){`), single quotes, and
/// block comments followed by code on the same line.
fn normalize_spacing(src: &str, facts: &mut SourceFacts) -> String {
    let mut out: Vec<String> = Vec::with_capacity(src.lines().count());
    for line in src.lines() {
        // Split a comment that started its own line in the source but was
        // glued to the statement by swc. Comment text is emitted verbatim
        // (Prettier never rewrites comment contents).
        let indent = line.chars().take_while(|c| *c == ' ').count();
        let indent_s = " ".repeat(indent);
        let mut split: Option<(String, String, bool)> = None;
        {
            let b = line.as_bytes();
            let mut in_str = false;
            let mut j = 0;
            while j < b.len() {
                match b[j] {
                    b'"' => in_str = !in_str,
                    b'/' if !in_str && j + 1 < b.len() && b[j + 1] == b'/' => {
                        let text = line[j..].trim_end().to_string();
                        let code = line[..j].trim_end();
                        if !code.is_empty() {
                            if let Some(blank_before) = facts.own_line_comments.remove(&text) {
                                split = Some((code.to_string(), text, blank_before));
                            }
                        }
                        break;
                    }
                    b'/' if !in_str && j + 1 < b.len() && b[j + 1] == b'*' => {
                        if let Some(end) = line[j + 2..].find("*/") {
                            let text = line[j..j + 2 + end + 2].to_string();
                            let code = line[..j].trim_end();
                            if !code.is_empty() {
                                if let Some(blank_before) = facts.own_line_comments.remove(&text) {
                                    split = Some((code.to_string(), text, blank_before));
                                }
                            }
                        }
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        if let Some((code, comment, blank_before)) = split {
            if blank_before && out.last().map_or(false, |l| !l.is_empty()) {
                out.push(String::new());
            }
            out.push(code);
            out.push(format!("{}{}", indent_s, comment));
            continue;
        }

        // Prettier preserves one blank line before comments/statements that
        // followed a blank line in the source (swc drops all blank lines).
        let prev_ends_stmt = out.last().map_or(false, |l| {
            let lt = l.trim_end();
            !lt.is_empty() && (lt.ends_with(';') || lt.ends_with('}') || lt.ends_with('{'))
        });
        if let Some(blank_before) = take_own_line_comment(line, facts) {
            if blank_before && prev_ends_stmt {
                out.push(String::new());
            }
        } else {
            let key: String = line.trim_start().chars().take(24).collect();
            if !key.is_empty() && facts.blank_before_stmts.contains(&key) && prev_ends_stmt {
                out.push(String::new());
            }
        }

        let mut l = line.to_string();
        // swc prints block-comment continuation lines with a leading space
        // (` */`, ` * foo`); Prettier keeps them flush with the comment indent.
        if l.starts_with(' ') && l.trim_start().starts_with('*') {
            l = l[1..].to_string();
        }

        for kw in [
            "for", "if", "while", "switch", "catch", "return", "do", "else", "new", "typeof",
            "await", "yield", "throw", "delete", "void", "function",
        ] {
            let pat = format!("{}(", kw);
            if let Some(idx) = l.find(&pat) {
                let before_ok = idx == 0
                    || !l[..idx]
                        .chars()
                        .next_back()
                        .map_or(false, |c| c.is_alphanumeric() || c == '_' || c == '$');
                if before_ok {
                    l.replace_range(idx..idx + pat.len(), &format!("{} (", kw));
                }
            }
        }

        if line.matches('"').count() % 2 == 0 && line.matches('\'').count() % 2 == 0 {
            l = l.replace("){", ") {");
            l = l.replace("}else", "} else");
            l = l.replace("}catch", "} catch");
            l = l.replace(")=> {", ") => {"); // keep canonical arrow spacing
        }

        if let Some(pos) = l.find("*/ ") {
            let (head, tail) = l.split_at(pos + 2);
            let indent = line.chars().take_while(|c| *c == ' ').collect::<String>();
            l = format!("{}\n{}{}", head.trim_end(), indent, tail.trim_start());
        }

        let single = line.matches('\'').count();
        let double = line.matches('"').count();
        if single % 2 == 0 && single > 0 && double == 0 && !line.contains("\\'") {
            l = l.replace('\'', "\"");
        }

        out.push(l);
    }
    let mut r = out.join("\n");
    if !r.ends_with('\n') {
        r.push('\n');
    }
    r
}

/// True when the line is a comment start whose text is an own-line comment
/// in the source map (consuming it so a later identical trailing comment
/// isn't also split).
fn take_own_line_comment(line: &str, facts: &mut SourceFacts) -> Option<bool> {
    let t = line.trim_start();
    if t.starts_with("//") {
        return facts.own_line_comments.remove(t.trim_end());
    }
    if t.starts_with("/*") {
        if let Some(e) = t.find("*/") {
            return facts.own_line_comments.remove(&t[..e + 2]);
        }
        return facts.own_line_comments.remove(t.trim_end());
    }
    None
}

/// Prettier breaks multi-declarator `var`/`let`/`const` statements that
/// exceed print-width: one declarator per line, continuation-indented at
/// `indent + tab_width` (default 2 — hanging, NOT keyword-aligned).
fn normalize_var_breaks(src: &str, max_width: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        let indent = line.chars().take_while(|c| *c == ' ').count();
        let is_decl = trimmed.starts_with("var ")
            || trimmed.starts_with("let ")
            || trimmed.starts_with("const ");
        let over = line.len() > max_width;
        let plain = trimmed.matches('"').count() % 2 == 0
            && trimmed.matches('\'').count() % 2 == 0
            && !trimmed.contains("=>");
        if is_decl && over && plain {
            let mut depth = 0i32;
            let mut current = String::new();
            let mut parts: Vec<String> = Vec::new();
            for ch in trimmed.chars() {
                match ch {
                    '[' | '{' => {
                        depth += 1;
                        current.push(ch);
                    }
                    ']' | '}' => {
                        depth -= 1;
                        current.push(ch);
                    }
                    ',' if depth == 0 => {
                        parts.push(current.clone());
                        current.clear();
                    }
                    _ => current.push(ch),
                }
            }
            parts.push(current);
            if parts.len() >= 2 {
                let cont = " ".repeat(indent + 2);
                let mut broken = String::new();
                for (n, part) in parts.iter().enumerate() {
                    if n == 0 {
                        broken.push_str(part);
                    } else {
                        // parts keep the space after the comma; Prettier uses a
                        // hanging indent, so strip it.
                        broken.push_str(&format!(",\n{}{}", cont, part.trim_start()));
                    }
                }
                out.push(broken);
                continue;
            }
        }
        out.push(line.to_string());
    }
    let mut r = out.join("\n");
    if !r.ends_with('\n') {
        r.push('\n');
    }
    r
}

/// Re-indent swc output from its hardcoded 4-space units to the config's
/// tab width (or tabs). swc's codegen always emits 4-space steps.
fn apply_indent(src: &str, cfg: &FmtConfig) -> String {
    if cfg.tab_width == 4 && !cfg.use_tabs {
        return src.to_string();
    }
    let ends_nl = src.ends_with('\n');
    let body = src.trim_end_matches('\n');
    let unit = 4usize;
    let out: String = body
        .lines()
        .map(|l| {
            let n = l.chars().take_while(|c| *c == ' ').count();
            if n == 0 || n % unit != 0 {
                l.to_string()
            } else {
                let levels = n / unit;
                let indent = if cfg.use_tabs {
                    "\t".repeat(levels)
                } else {
                    " ".repeat(levels * cfg.tab_width.max(1))
                };
                format!("{}{}", indent, &l[n..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if ends_nl {
        format!("{}\n", out)
    } else {
        out
    }
}

// ── JSON ────────────────────────────────────────────────────────────────

pub fn format_json(source: &str, cfg: &FmtConfig) -> BundleResult<String> {
    let v: serde_json::Value = serde_json::from_str(source).map_err(|e| {
        BundleError::TransformError("json".to_string(), format!("JSON parse: {}", e))
    })?;
    let indent = if cfg.use_tabs {
        "\t".to_string()
    } else {
        " ".repeat(cfg.tab_width.max(1))
    };
    let fmt = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    serde::Serialize::serialize(&v, &mut ser)
        .map_err(|e| BundleError::TransformError("json".to_string(), e.to_string()))?;
    let pretty = String::from_utf8(buf)
        .map_err(|e| BundleError::TransformError("json".to_string(), e.to_string()))?;
    Ok(format!("{}\n", pretty))
}

// ── CSS ─────────────────────────────────────────────────────────────────

/// CSS is deliberately conservative: this pass never reflows declarations or
/// drops anything it does not fully understand. It normalises the trailing
/// newline and trims trailing spaces, which keeps `jhol fmt` non-destructive on
/// `.css`, `.scss` and `.less` even without the optional lightningcss engine.
/// CSS/SCSS/LESS formatter: structure and spacing, nothing clever.
///
/// What it does: one rule per block, one declaration per line, stable
/// indentation, `prop: value;` spacing, blank line between top-level blocks,
/// comments preserved where they appear, and strings / `url(...)` payloads
/// copied byte-for-byte (so `content: "a;b"` and `url(data:...;base64,...)`
/// survive).
///
/// ponytail: no line wrapping — long values are never split. Wrapping is the
/// part that needs a real parser; not wrapping cannot corrupt a file, while a
/// naive splitter can. Add wrapping when a stylesheet actually needs it.
pub fn format_css(source: &str, cfg: &FmtConfig) -> BundleResult<String> {
    let indent_unit: String = if cfg.use_tabs {
        "\t".to_string()
    } else {
        " ".repeat(cfg.tab_width.max(1))
    };
    let mut out = String::with_capacity(source.len() + 64);
    let mut prelude = String::new(); // selector or at-rule prelude
    let mut decl = String::new(); // declaration text between ; and ;
    let mut depth: usize = 0;
    let mut chars = source.chars().peekable();
    let mut pending_blank = false;

    fn pad(out: &mut String, unit: &str, depth: usize) {
        for _ in 0..depth {
            out.push_str(unit);
        }
    }
    fn flush_blank(out: &mut String, pending: &mut bool) {
        if *pending && !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        *pending = false;
    }
    /// `prop:value` -> `prop: value`, keeping everything after the first colon.
    fn normalize_decl(raw: &str) -> String {
        let t = raw.trim();
        if t.is_empty() {
            return String::new();
        }
        match t.find(':') {
            Some(i) => {
                let (k, v) = t.split_at(i);
                let v = v[1..].trim();
                let k = k.trim();
                if v.is_empty() {
                    format!("{k};")
                } else {
                    format!("{k}: {v};")
                }
            }
            None => format!("{t};"),
        }
    }

    while let Some(c) = chars.next() {
        // ---- comment: copy verbatim onto its own line -------------------
        if c == '/' && chars.peek() == Some(&'*') {
            let mut buf = String::from("/*");
            chars.next();
            while let Some(x) = chars.next() {
                buf.push(x);
                if x == '*' && chars.peek() == Some(&'/') {
                    buf.push(chars.next().unwrap());
                    break;
                }
            }
            let at_line_start = out.ends_with('\n') || out.is_empty();
            if !at_line_start {
                out.push('\n');
            }
            flush_blank(&mut out, &mut pending_blank);
            pad(&mut out, &indent_unit, depth);
            out.push_str(buf.trim());
            out.push('\n');
            continue;
        }

        // ---- strings: copy verbatim ------------------------------------
        if c == '"' || c == '\'' {
            let quote = c;
            let mut buf = String::new();
            buf.push(c);
            while let Some(x) = chars.next() {
                buf.push(x);
                if x == '\\' {
                    if let Some(n) = chars.next() {
                        buf.push(n);
                    }
                    continue;
                }
                if x == quote {
                    break;
                }
            }
            if depth > 0 {
                decl.push_str(&buf);
            } else {
                prelude.push_str(&buf);
            }
            continue;
        }

        // ---- url( ... ): copy verbatim until the matching ) -------------
        if c == '(' && decl.to_ascii_lowercase().ends_with("url") {
            let mut buf = String::from(c);
            let mut parens = 1usize;
            while let Some(x) = chars.next() {
                buf.push(x);
                match x {
                    '(' => parens += 1,
                    ')' => {
                        parens -= 1;
                        if parens == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if depth > 0 {
                decl.push_str(&buf)
            } else {
                prelude.push_str(&buf)
            }
            continue;
        }

        match c {
            '{' => {
                // A block can open on a selector (top level or nested) or on an
                // at-rule; in both cases whatever has accumulated is the head.
                let head =
                    normalize_selector_commas(format!("{}{}", prelude.trim(), decl.trim()).trim());
                prelude.clear();
                decl.clear();
                flush_blank(&mut out, &mut pending_blank);
                pad(&mut out, &indent_unit, depth);
                if head.is_empty() {
                    out.push_str("{\n");
                } else {
                    out.push_str(&head);
                    out.push_str(" {\n");
                }
                depth += 1;
            }
            '}' => {
                let pending = normalize_decl(&decl);
                decl.clear();
                if !pending.is_empty() {
                    pad(&mut out, &indent_unit, depth);
                    out.push_str(&pending);
                    out.push('\n');
                }
                depth = depth.saturating_sub(1);
                pad(&mut out, &indent_unit, depth);
                out.push_str("}\n");
                if depth == 0 {
                    pending_blank = true;
                }
            }
            ';' => {
                if depth == 0 {
                    let stmt = prelude.trim().to_string();
                    prelude.clear();
                    if !stmt.is_empty() {
                        flush_blank(&mut out, &mut pending_blank);
                        out.push_str(&stmt);
                        out.push_str(";\n");
                        pending_blank = true;
                    }
                } else {
                    let d = normalize_decl(&decl);
                    decl.clear();
                    if !d.is_empty() {
                        pad(&mut out, &indent_unit, depth);
                        out.push_str(&d);
                        out.push('\n');
                    }
                }
            }
            '\n' | '\r' => {
                // newline separates tokens; blank line becomes a pending blank
                let at_decl_end =
                    depth > 0 && !decl.trim().is_empty() && decl.trim_end().ends_with(',');
                if depth == 0 && prelude.trim().is_empty() {
                    pending_blank = true;
                }
                if at_decl_end {
                    decl.push(' ');
                } else {
                    decl.push(' ');
                }
            }
            c if c.is_whitespace() => {
                if depth > 0 {
                    if !decl.ends_with(' ') {
                        decl.push(' ');
                    }
                } else if !prelude.ends_with(' ') {
                    prelude.push(' ');
                }
            }
            _ => {
                if depth > 0 {
                    decl.push(c)
                } else {
                    prelude.push(c)
                }
            }
        }
    }

    // trailing statement without terminator
    let rest = normalize_decl(&decl);
    if !rest.is_empty() {
        pad(&mut out, &indent_unit, depth);
        out.push_str(&rest);
        out.push('\n');
    }
    let tail = prelude.trim();
    if !tail.is_empty() {
        out.push_str(tail);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    Ok(out)
}

// ── Dispatch ────────────────────────────────────────────────────────────

/// Comment forms whose text or position changes what a build does: bundler
/// tree-shaking annotations, lint / TypeScript directives, license banners.
/// The native printer does not preserve them, so a file carrying any of these
/// takes the swc path (which keeps comments) or refuses to format.
fn has_build_annotations(source: &str) -> bool {
    const MARKERS: &[&str] = &[
        "@__PURE__",
        "#__PURE__",
        "@ts-ignore",
        "@ts-expect-error",
        "@ts-nocheck",
        "eslint-disable",
        "eslint-enable",
        "prettier-ignore",
        "@license",
        "@preserve",
        "@vitest-environment",
    ];
    MARKERS.iter().any(|m| source.contains(m))
}

pub fn format_source(source: &str, filename: &str, cfg: &FmtConfig) -> BundleResult<String> {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".json") {
        return format_json(source, cfg);
    }
    if lower.ends_with(".css") || lower.ends_with(".scss") || lower.ends_with(".less") {
        return format_css(source, cfg);
    }

    // JholVM's parser covers JavaScript only. TypeScript and JSX exist on the
    // swc backend, so route them there instead of skipping the file silently.
    let ts_family = lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".jsx")
        || lower.ends_with(".mts")
        || lower.ends_with(".cts");

    #[cfg(feature = "swc-backend")]
    {
        if ts_family || has_build_annotations(source) {
            // swc captures comments into a map and hands them to the emitter, so
            // `/* @__PURE__ */` survives here. The native printer drops it.
            return format_js(source, filename, cfg);
        }
    }
    #[cfg(not(feature = "swc-backend"))]
    {
        if ts_family {
            return Err(BundleError::TransformError(
                filename.to_string(),
                "TypeScript/JSX formatting needs the swc backend; build with default features"
                    .to_string(),
            ));
        }
        if has_build_annotations(source) {
            return Err(BundleError::TransformError(
                filename.to_string(),
                "refusing to format: this file carries build/lint annotations \
                 (@__PURE__, eslint-disable, @ts-ignore, ...) that the native printer drops. \
                 Build with default features (swc backend) to preserve them."
                    .to_string(),
            ));
        }
    }

    #[cfg(feature = "native-fmt")]
    {
        // JholVM's own parser formats the language JholVM runs — no
        // third-party codegen and no regex patch-up pass to corrupt it.
        let opts = jhol_vm::formatter::FormatOptions {
            print_width: cfg.print_width,
            tab_width: cfg.tab_width,
            use_tabs: cfg.use_tabs,
            semi: cfg.semi,
            single_quote: cfg.single_quote,
            trailing_comma: cfg.trailing_comma.clone(),
        };
        return jhol_vm::formatter::format_source_with_options(source, filename, &opts)
            .map_err(|e| BundleError::TransformError(filename.to_string(), e));
    }
    #[cfg(not(feature = "native-fmt"))]
    {
        format_js(source, filename, cfg)
    }
}

pub fn content_hash(source: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(source.as_bytes());
    format!("{:x}", h.finalize())
}

/// `a,b` → `a, b` in a selector list, without touching commas inside strings,
/// parentheses or brackets (`[data-x="a,b"]`, `:is(a,b)`).
fn normalize_selector_commas(prelude: &str) -> String {
    let mut out = String::with_capacity(prelude.len() + 4);
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut prev_comma = false;
    for c in prelude.chars() {
        if let Some(q) = in_str {
            out.push(c);
            if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                in_str = Some(c);
                out.push(c);
            }
            '(' | '[' => {
                depth += 1;
                out.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                out.push(c);
            }
            ',' if depth == 0 => {
                out.push(',');
                prev_comma = true;
            }
            ' ' if prev_comma => {}
            other => {
                if prev_comma {
                    out.push(' ');
                    prev_comma = false;
                }
                out.push(other);
            }
        }
    }
    out
}

pub fn is_supported(path: &Path) -> bool {
    let lower = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let js = if cfg!(feature = "native-fmt") && cfg!(feature = "swc-backend") {
        // JavaScript goes through JholVM's parser; TypeScript and JSX go through
        // the swc backend (see `format_source`). Both are real, so claim both.
        lower.ends_with(".js")
            || lower.ends_with(".mjs")
            || lower.ends_with(".cjs")
            || lower.ends_with(".jsx")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".mts")
            || lower.ends_with(".cts")
    } else if cfg!(feature = "native-fmt") {
        // JavaScript only: JholVM's parser cannot read TypeScript or JSX, and
        // there is no swc fallback compiled in.
        lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs")
    } else {
        lower.ends_with(".js")
            || lower.ends_with(".jsx")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".mjs")
            || lower.ends_with(".cjs")
            || lower.ends_with(".mts")
            || lower.ends_with(".cts")
    };
    js || lower.ends_with(".json")
        || lower.ends_with(".css")
        || lower.ends_with(".scss")
        || lower.ends_with(".less")
        || lower.ends_with(".rs")
        || lower.ends_with(".go")
        || lower.ends_with(".py")
}

/// External ecosystem formatter for a file extension: (binary, base args).
/// The caller appends the file path after `args`.
pub fn external_tool(ext: &str) -> Option<(&'static str, Vec<&'static str>)> {
    match ext {
        "rs" => Some(("rustfmt", vec!["--edition", "2021"])),
        "go" => Some(("gofmt", vec!["-w"])),
        "py" => Some(("ruff", vec!["format"])),
        _ => None,
    }
}

// ── Ignore rules ────────────────────────────────────────────────────────

const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".jhol",
    "archive",
];
const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "jhol.project.lock",
];

pub fn is_ignored(path: &Path) -> bool {
    for part in path.components() {
        let s = part.as_os_str().to_string_lossy();
        if IGNORED_DIRS.contains(&s.as_ref()) {
            return true;
        }
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if LOCKFILES.contains(&name) {
            return true;
        }
        if name.ends_with(".min.js") || name.ends_with(".min.css") {
            return true;
        }
    }
    false
}

// ── Persistent skip-cache ───────────────────────────────────────────────

/// Persistent skip-cache: path -> content hash, guarded by engine + config
/// digest so option or output-semantics changes invalidate entries.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct FmtCache {
    pub engine: String,
    pub config: u64,
    pub files: HashMap<String, String>,
    /// Directory-walk snapshots, keyed by the root the user named (`.` for none). Lets a
    /// front door prove "the walk would return the same files" without walking; see
    /// `Walk`. Ignored by older readers.
    #[serde(default)]
    pub walks: HashMap<String, Walk>,
    #[serde(skip)]
    pub dirty: bool,
}

/// What one `jhol fmt <dir>` walk saw: every directory that can influence the result with
/// its mtime in ns (read **before** the directory was listed, so an entry added during
/// the walk can only make the snapshot stale, never wrong), and the final file list under
/// the exact keys the cache uses. A directory's mtime changes when an entry is added,
/// removed or renamed; an in-place edit is caught by the file's content hash instead.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Walk {
    pub dirs: Vec<(String, u64)>,
    pub files: Vec<String>,
}

impl FmtCache {
    pub fn load(dir: &Path, engine: &str, config_digest: u64) -> Self {
        let path = dir.join(".jhol").join("fmt-cache.json");
        let mut c: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<FmtCache>(&raw).ok())
            .unwrap_or_default();
        if c.engine != engine || c.config != config_digest {
            c.files.clear();
            c.walks.clear();
        }
        c.engine = engine.to_string();
        c.config = config_digest;
        c
    }

    pub fn set_walk(&mut self, key: &str, walk: Walk) {
        if self.walks.get(key) != Some(&walk) {
            self.walks.insert(key.to_string(), walk);
            self.dirty = true;
        }
    }

    pub fn is_fresh(&self, rel: &str, hash: &str) -> bool {
        self.files.get(rel).map(|h| h == hash).unwrap_or(false)
    }

    pub fn record(&mut self, rel: &str, hash: &str) {
        match self.files.get(rel) {
            Some(h) if h == hash => {}
            _ => {
                self.files.insert(rel.to_string(), hash.to_string());
                self.dirty = true;
            }
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let jhol_dir = dir.join(".jhol");
        std::fs::create_dir_all(&jhol_dir)?;
        let path = jhol_dir.join("fmt-cache.json");
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(temp, path)
    }
}

#[cfg(test)]
mod fmt_tests {
    use super::*;

    #[test]
    fn test_content_hash_stable() {
        assert_eq!(content_hash("x"), content_hash("x"));
    }

    #[test]
    fn test_cache_fresh() {
        let mut c = FmtCache::default();
        c.engine = "e".into();
        c.config = 1;
        c.record("a.js", "deadbeef");
        assert!(c.is_fresh("a.js", "deadbeef"));
        assert!(!c.is_fresh("a.js", "other"));
    }

    #[test]
    fn test_var_break() {
        let src = "var baseCreate = require(\"./_baseCreate\"), getPrototype = require(\"./_getPrototype\"), isPrototype = require(\"./_isPrototype\");\n";
        let out = normalize_var_breaks(src, 80);
        assert!(
            out.contains(",\n  "),
            "want 2-space continuation, got:\n{}",
            out
        );
    }

    #[test]
    fn test_quote_normalize() {
        let src = "var x = 'hello';\n";
        let out = normalize_spacing(src, &mut SourceFacts::default());
        assert!(out.contains("\"hello\""));
    }

    #[test]
    fn test_collapse() {
        let src = "const x = {\n  a: 1,\n  b: 2\n};\n";
        let out = post_process_js(src, &FmtConfig::default(), &mut SourceFacts::default());
        assert_eq!(out.trim(), "const x = { a: 1, b: 2 };");
    }

    #[test]
    fn json_indent_does_not_rewrite_string_contents() {
        let cfg = FmtConfig::default();
        let src = "{\"s\":\"a    b\",\"n\":{\"x\":1}}";
        let out = format_json(src, &cfg).expect("format json");
        assert!(out.contains("\"a    b\""), "string spaces preserved: {out}");
        assert!(out.contains("\n  \"n\""), "object indented: {out}");
        let again = format_json(&out, &cfg).expect("reformat json");
        assert_eq!(out, again, "json formatting must be idempotent");
    }

    #[test]
    fn css_rules_get_structure_and_indentation() {
        let cfg = FmtConfig::default();
        let src = "a{color:red;background:blue}b{color:green}";
        let out = format_css(src, &cfg).expect("css");
        assert_eq!(
            out,
            "a {\n  color: red;\n  background: blue;\n}\n\nb {\n  color: green;\n}\n"
        );
        let again = format_css(&out, &cfg).expect("css again");
        assert_eq!(out, again, "css formatting must be idempotent");
    }

    #[test]
    fn css_preserves_strings_and_url_payloads() {
        let cfg = FmtConfig::default();
        let src = "a{content:\"x;y{z}\";background:url(data:image/png;base64,AA==)}";
        let out = format_css(src, &cfg).expect("css");
        assert!(out.contains("content: \"x;y{z}\";"), "{out}");
        assert!(out.contains("url(data:image/png;base64,AA==)"), "{out}");
    }

    #[test]
    fn css_nested_at_rules_indent_and_close() {
        let cfg = FmtConfig::default();
        let src = "@media (max-width:600px){a{color:red}}";
        let out = format_css(src, &cfg).expect("css");
        assert!(out.starts_with("@media (max-width:600px) {"), "{out}");
        assert!(out.contains("\n  a {"), "{out}");
        assert!(out.contains("\n    color: red;"), "{out}");
    }

    #[test]
    fn css_selector_lists_get_comma_spacing() {
        let cfg = FmtConfig::default();
        let out = format_css(".a,.b{color:red}", &cfg).expect("css");
        assert!(out.starts_with(".a, .b {"), "{out}");
        // a comma inside a string or :is() is untouched
        let out2 = format_css("[data-x=\"a,b\"],:is(.c,.d){color:red}", &cfg).expect("css");
        assert!(out2.contains("[data-x=\"a,b\"], :is(.c,.d) {"), "{out2}");
    }

    #[test]
    fn css_keeps_comments() {
        let cfg = FmtConfig::default();
        let out = format_css("/* head */\na{color:red}", &cfg).expect("css");
        assert!(out.contains("/* head */"), "{out}");
    }

    #[cfg(feature = "swc-backend")]
    #[test]
    fn typescript_and_jsx_are_formatted_not_skipped() {
        let cfg = FmtConfig::default();
        let ts = "interface P<T=string>{a:T}\nconst f=<T,>(x:T):T[]=>[x];\n";
        let out = format_source(ts, "t.ts", &cfg).expect("format ts");
        assert!(out.contains("interface P"), "{out}");
        let jsx = "export const A=({t})=><div className=\"x\">{t}</div>;\n";
        let out2 = format_source(jsx, "t.jsx", &cfg).expect("format jsx");
        assert!(out2.contains("<div"), "{out2}");
    }

    #[cfg(feature = "swc-backend")]
    #[test]
    fn build_annotations_survive_formatting() {
        let cfg = FmtConfig::default();
        let src = "const keep = /* @__PURE__ */ make();\nconst v = /*#__PURE__*/ factory();\n";
        let out = format_source(src, "t.js", &cfg).expect("format annotated js");
        assert!(out.contains("@__PURE__"), "spelled annotation kept: {out}");
        assert!(out.contains("#__PURE__"), "hash annotation kept: {out}");
    }

    #[test]
    fn css_formatting_never_returns_empty_for_nonempty_input() {
        let src = "a {  color: red;  }\n\nb { color: blue; }\n";
        let out = format_css(src, &FmtConfig::default()).expect("css");
        assert!(!out.is_empty());
        assert!(out.contains("color: red;"));
    }

    #[test]
    fn js_options_reach_the_native_formatter() {
        let mut cfg = FmtConfig::default();
        cfg.single_quote = true;
        cfg.semi = false;
        let out = format_source("const s = \"hi\";\n", "t.js", &cfg).expect("format js");
        assert!(out.contains("'hi'"), "{out}");
        assert!(!out.contains(";"), "{out}");
    }

    #[test]
    fn test_apply_indent_2space() {
        let src = "function f() {\n    return 1;\n}\n";
        let out = apply_indent(src, &FmtConfig::default());
        assert_eq!(out, "function f() {\n  return 1;\n}\n");
    }
}
