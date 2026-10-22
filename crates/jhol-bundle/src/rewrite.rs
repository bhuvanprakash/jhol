//! Canonical `require()` → `__require('<module-id>')` rewriting.
//!
//! This is the single implementation used by every bundle emitter. It replaces
//! two divergent, broken predecessors:
//!
//! - the main emitter simply never rewrote `require()` at all, so **any**
//!   CommonJS module crashed at runtime with `MODULE_NOT_FOUND`;
//! - `codegen::rewrite_cjs_imports` did `source.replace("require(", "__require(")`,
//!   which renamed occurrences inside strings and comments and, worse, kept the
//!   *path* as the argument (`__require("./lib.js")`) even though the bundle's
//!   `__require` is keyed by module id (`'__m3'`).
//!
//! The scanner here is string-, template-, comment- and interpolation-aware, so
//! `require("./x")` inside a template interpolation is rewritten while the same
//! text inside a string literal is not.

/// True when `text[pos..pos+len]` is not glued to identifier characters, so
/// `required(` / `myrequire(` are not mistaken for `require(`.
fn word_boundary(text: &str, pos: usize, len: usize) -> bool {
    let b = text.as_bytes();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    let before_ok = pos == 0 || !is_ident(b[pos - 1]);
    let after = pos + len;
    let after_ok = after >= b.len() || !is_ident(b[after]);
    before_ok && after_ok
}

/// Rewrite `require("<spec>")` to `__require('<id>')` using `resolve`.
///
/// `resolve` returns the bundle module id for a specifier, or `None` to leave the
/// call untouched — that keeps real `require` calls working for externals (Node
/// builtins, marked-external packages) instead of breaking them.
pub(crate) fn rewrite_require_calls(
    code: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> String {
    let bytes = code.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(code.len());
    let mut i = 0usize;

    while i < n {
        match bytes[i] {
            // String literal: copy verbatim (escapes included).
            q @ (b'\'' | b'"') => {
                let start = i;
                i += 1;
                while i < n {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == q {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                out.push_str(&code[start..i.min(n)]);
            }
            // Template literal: literal text is opaque, `${...}` is code.
            b'`' => {
                out.push('`');
                i += 1;
                let mut literal_start = i;
                while i < n {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'`' {
                        out.push_str(&code[literal_start..i]);
                        out.push('`');
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'$' && i + 1 < n && bytes[i + 1] == b'{' {
                        out.push_str(&code[literal_start..i]);
                        out.push_str("${");
                        i += 2;
                        let expr_start = i;
                        let mut depth = 1i32;
                        while i < n {
                            match bytes[i] {
                                b'{' => depth += 1,
                                b'}' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                q @ (b'\'' | b'"') => {
                                    i += 1;
                                    while i < n {
                                        if bytes[i] == b'\\' {
                                            i += 2;
                                            continue;
                                        }
                                        if bytes[i] == q {
                                            break;
                                        }
                                        i += 1;
                                    }
                                }
                                b'`' => {
                                    i += 1;
                                    while i < n {
                                        if bytes[i] == b'\\' {
                                            i += 2;
                                            continue;
                                        }
                                        if bytes[i] == b'`' {
                                            break;
                                        }
                                        i += 1;
                                    }
                                }
                                _ => {}
                            }
                            i += 1;
                        }
                        let expr_end = i.min(n);
                        out.push_str(&rewrite_require_calls(&code[expr_start..expr_end], resolve));
                        if i < n {
                            out.push('}');
                            i += 1;
                        }
                        literal_start = i;
                        continue;
                    }
                    i += 1;
                }
            }
            b'/' if i + 1 < n && bytes[i + 1] == b'/' => {
                let start = i;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
                out.push_str(&code[start..i]);
            }
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                let start = i;
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(n);
                out.push_str(&code[start..i]);
            }
            _ => {
                if code[i..].starts_with("require") && word_boundary(code, i, "require".len()) {
                    if let Some((spec, end)) = parse_require_call(code, i + "require".len()) {
                        if let Some(mid) = resolve(&spec) {
                            out.push_str(&format!("__require('{}')", mid));
                            i = end;
                            continue;
                        }
                    }
                }
                let ch_len = code[i..].chars().next().map(char::len_utf8).unwrap_or(1);
                out.push_str(&code[i..(i + ch_len).min(n)]);
                i += ch_len;
            }
        }
    }

    out
}

/// Parse `"(" ws* '<spec>' ws* ")"` starting at `from`. Returns the specifier and
/// the index just past the closing paren.
fn parse_require_call(code: &str, from: usize) -> Option<(String, usize)> {
    let bytes = code.as_bytes();
    let n = bytes.len();
    let mut j = from;
    while j < n && (bytes[j] as char).is_ascii_whitespace() {
        j += 1;
    }
    if j >= n || bytes[j] != b'(' {
        return None;
    }
    j += 1;
    while j < n && (bytes[j] as char).is_ascii_whitespace() {
        j += 1;
    }
    if j >= n || (bytes[j] != b'\'' && bytes[j] != b'"') {
        return None;
    }
    let quote = bytes[j];
    let spec_start = j + 1;
    let mut k = spec_start;
    while k < n {
        if bytes[k] == b'\\' {
            k += 2;
            continue;
        }
        if bytes[k] == quote {
            break;
        }
        k += 1;
    }
    if k >= n {
        return None;
    }
    let spec = code[spec_start..k].to_string();
    let mut end = k + 1;
    while end < n && (bytes[end] as char).is_ascii_whitespace() {
        end += 1;
    }
    if end < n && bytes[end] == b')' {
        end += 1;
    }
    Some((spec, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(spec: &str) -> Option<String> {
        match spec {
            "./b.js" => Some("__m1".to_string()),
            "./c.js" => Some("__m2".to_string()),
            _ => None,
        }
    }

    #[test]
    fn rewrites_require_to_module_id() {
        let out = rewrite_require_calls("const b = require('./b.js');", &resolver);
        assert_eq!(out, "const b = __require('__m1');");
    }

    #[test]
    fn handles_spacing_and_double_quotes() {
        let out = rewrite_require_calls("require  (  \"./b.js\"  )", &resolver);
        assert_eq!(out, "__require('__m1')");
    }

    #[test]
    fn leaves_externals_and_unknown_specs_alone() {
        let src = "require('fs'); require('./missing.js');";
        assert_eq!(rewrite_require_calls(src, &resolver), src);
    }

    #[test]
    fn ignores_strings_comments_and_identifiers() {
        let src = "const s = \"require('./b.js')\"; // require('./b.js')\n/* require('./b.js') */\nconst required = 1;\nmyrequire('./b.js');";
        assert_eq!(rewrite_require_calls(src, &resolver), src);
    }

    #[test]
    fn rewrites_inside_template_interpolation_only() {
        let src = "const a = `x${require('./b.js').name}y`;\nconst t = `require('./b.js')`;";
        let out = rewrite_require_calls(src, &resolver);
        assert!(out.contains("${__require('__m1').name}"), "out: {}", out);
        // the literal template text must be untouched
        assert!(out.contains("`require('./b.js')`"), "out: {}", out);
    }

    #[test]
    fn nested_template_and_multiple_requires() {
        let src = "const x = `a${ `b${require('./b.js')}` }c`; require('./c.js');";
        let out = rewrite_require_calls(src, &resolver);
        assert!(out.contains("${__require('__m1')}"), "out: {}", out);
        assert!(out.ends_with("__require('__m2');"), "out: {}", out);
    }

    #[test]
    fn handles_escaped_quotes_in_strings() {
        let src = "const s = 'it\\'s require(\"./b.js\")';\nrequire('./b.js');";
        let out = rewrite_require_calls(src, &resolver);
        assert!(out.starts_with("const s = 'it\\'s require(\"./b.js\")';"));
        assert!(out.contains("__require('__m1')"));
    }
}
