use std::path::Path;
use std::sync::OnceLock;

use crate::errors::RuntimeResult;

#[cfg_attr(feature = "swc-transpile", allow(dead_code))]
fn import_type_re() -> &'static regex_lite::Regex {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex_lite::Regex::new(
            r#"(?m)^\s*import\s+type\s+\{[^}]*\}\s*from\s*['\"][^'\"]*['\"]\s*;?\s*$"#,
        )
        .unwrap()
    })
}

#[cfg_attr(feature = "swc-transpile", allow(dead_code))]
fn export_type_re() -> &'static regex_lite::Regex {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex_lite::Regex::new(r#"(?m)^\s*export\s+type\s+\{[^}]*\}\s*;?\s*$"#).unwrap()
    })
}

#[cfg_attr(feature = "swc-transpile", allow(dead_code))]
fn type_decl_re() -> &'static regex_lite::Regex {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| regex_lite::Regex::new(r#"(?m)^\s*(export\s+)?(type|interface|enum)\s+\w+"#).unwrap())
}

/// Returns true if the source likely contains TypeScript-specific syntax.
#[cfg_attr(feature = "swc-transpile", allow(dead_code))]
fn needs_ts_processing(source: &str, filename: &str) -> bool {
    let lower = filename.to_lowercase();
    let is_ts = lower.ends_with(".ts") || lower.ends_with(".tsx");
    let is_jsx = lower.ends_with(".jsx") || lower.ends_with(".tsx");

    if !is_ts && !is_jsx {
        return false;
    }
    if is_ts
        && (source.contains(":")
            || source.contains("interface ")
            || source.contains("type ")
            || source.contains("enum ")
            || source.contains("as "))
    {
        return true;
    }
    if is_jsx && source.contains('<') {
        return true;
    }
    false
}

#[cfg(feature = "swc-transpile")]
mod swc_backend {
    use swc_core::common::{
        comments::NoopComments,
        errors::{ColorConfig, Handler},
        sync::Lrc,
        FileName, Globals, Mark, SourceMap, GLOBALS,
    };
    use swc_core::ecma::ast::{Pass, Program};
    use swc_core::ecma::transforms::base::helpers::{Helpers, HELPERS};
    use swc_core::ecma::{
        codegen::{text_writer::JsWriter as CodeGenWriter, Emitter},
        parser::{EsSyntax, Parser, StringInput, Syntax, TsSyntax},
        transforms::{base::fixer::fixer, react::jsx, typescript::strip},
    };

    use crate::errors::{RuntimeError, RuntimeResult};

    #[allow(clippy::needless_update)]
    pub fn transpile_ts_to_js(source: &str, filename: &str) -> RuntimeResult<String> {
        let cm = Lrc::<SourceMap>::default();
        let _handler =
            Handler::with_tty_emitter(ColorConfig::Always, false, false, Some(cm.clone()));
        let fm = cm.new_source_file(FileName::Custom(filename.to_string()).into(), source.to_string());

        let is_tsx = filename.ends_with(".tsx");
        let is_ts = filename.ends_with(".ts");
        let is_jsx = filename.ends_with(".jsx");

        let syntax = if is_ts || is_tsx {
            Syntax::Typescript(TsSyntax {
                tsx: is_tsx,
                decorators: true,
                dts: false,
                no_early_errors: false,
                disallow_ambiguous_jsx_like: false,
                ..Default::default()
            })
        } else {
            Syntax::Es(EsSyntax {
                jsx: is_jsx,
                ..Default::default()
            })
        };

        let mut parser = Parser::new(syntax, StringInput::from(&*fm), None);
        let module = parser.parse_module().map_err(|e| {
            RuntimeError::TranspileError(format!("Parse error in {}: {:?}", filename, e))
        })?;

        let mut program = Program::Module(module);

        GLOBALS.set(&Globals::new(), || {
            HELPERS.set(&Helpers::new(true), || {
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();

                strip(unresolved_mark, top_level_mark).process(&mut program);

                if is_tsx || is_jsx {
                    jsx::jsx(
                        cm.clone(),
                        Some(NoopComments),
                        swc_core::ecma::transforms::react::Options {
                            development: Some(false),
                            ..Default::default()
                        },
                        top_level_mark,
                        unresolved_mark,
                    )
                    .process(&mut program);
                }

                fixer(None).process(&mut program);
            })
        });

        let module = program.expect_module();

        let mut buf = Vec::new();
        {
            let mut cfg = swc_core::ecma::codegen::Config::default();
            cfg.minify = false;
            let mut emitter = Emitter {
                cfg,
                cm: cm.clone(),
                comments: None,
                wr: Box::new(CodeGenWriter::new(cm.clone(), "\n", &mut buf, None)),
            };
            emitter
                .emit_module(&module)
                .map_err(|e| RuntimeError::TranspileError(format!("Codegen error: {:?}", e)))?;
        }

        let js = String::from_utf8(buf)
            .map_err(|e| RuntimeError::TranspileError(format!("UTF-8 error: {}", e)))?;

        Ok(js)
    }
}

/// Compile TypeScript/JSX to JavaScript.
pub fn transpile(source: &str, filename: &str) -> RuntimeResult<String> {
    let lower = filename.to_lowercase();
    if lower.ends_with(".ts") || lower.ends_with(".tsx") || lower.ends_with(".jsx") {
        #[cfg(feature = "swc-transpile")]
        {
            swc_backend::transpile_ts_to_js(source, filename)
        }
        #[cfg(not(feature = "swc-transpile"))]
        {
            fast_strip_ts(source, filename)
        }
    } else {
        Ok(source.to_string())
    }
}

/// Fast regex-based TS type stripper. Handles common TS code.
#[cfg_attr(feature = "swc-transpile", allow(dead_code))]
fn fast_strip_ts(source: &str, filename: &str) -> RuntimeResult<String> {
    if !needs_ts_processing(source, filename) {
        return Ok(source.to_string());
    }

    let is_jsx = filename.ends_with(".jsx") || filename.ends_with(".tsx");

    // 1. Strip `import type { ... } from '...'` and `export type { ... }`
    let mut result = import_type_re().replace_all(source, "").to_string();
    result = export_type_re().replace_all(&result, "").to_string();

    // 2. Strip `type Foo = ...;`, `interface Foo { ... }`, `enum Foo { ... }`
    //    including their `export` variants.
    result = type_decl_re().replace_all(&result, "").to_string();

    // 3. Walk the result and remove type/interface/enum blocks using brace depth.
    let mut out = String::with_capacity(result.len());
    let bytes = result.as_bytes();
    let mut i = 0;
    let mut brace_depth: i32 = 0;
    let mut skip_until_depth: Option<i32> = None;

    while i < bytes.len() {
        if skip_until_depth.is_none() {
            let line_start = i == 0 || bytes[i - 1] == b'\n';
            if line_start {
                let rest = &result[i..];
                let kw = [
                    "type ",
                    "interface ",
                    "enum ",
                    "export type ",
                    "export interface ",
                    "export enum ",
                ]
                .iter()
                .find(|kw| rest.starts_with(*kw));
                if kw.is_some() {
                    skip_until_depth = Some(brace_depth);
                    i += kw.unwrap().len();
                    continue;
                }
            }
        }

        let c = bytes[i];
        match c {
            b'{' => brace_depth += 1,
            b'}' => {
                brace_depth -= 1;
                if skip_until_depth == Some(brace_depth) {
                    skip_until_depth = None;
                    i += 1;
                    continue;
                }
            }
            _ => {}
        }

        if skip_until_depth.is_none() {
            out.push(c as char);
        }
        i += 1;
    }

    // 4. JSX transform (minimal) — only for .tsx/.jsx
    if is_jsx {
        out = transform_jsx(&out);
    }

    Ok(out)
}

/// Minimal JSX transform: returns source unchanged.
/// For correct JSX, build with the `swc-transpile` feature.
#[cfg_attr(feature = "swc-transpile", allow(dead_code))]
fn transform_jsx(source: &str) -> String {
    if !source.contains('<') {
        return source.to_string();
    }
    source.to_string()
}

pub fn needs_transpile(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "ts" | "tsx" | "jsx")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_type_imports() {
        let src = "import type { Foo } from './foo';\nconst x = 1;";
        let out = fast_strip_ts(src, "test.ts").unwrap();
        assert!(!out.contains("import type"));
        assert!(out.contains("const x = 1"));
    }

    #[test]
    fn strip_interface_block() {
        let src = "interface Foo { a: number; }\nconst x = 1;";
        let out = fast_strip_ts(src, "test.ts").unwrap();
        assert!(!out.contains("interface Foo"));
        assert!(out.contains("const x = 1"));
    }

    #[test]
    fn plain_js_unchanged() {
        let src = "const x = 1;\nfunction f() { return x; }";
        let out = fast_strip_ts(src, "test.js").unwrap();
        assert_eq!(out, src);
    }
}
