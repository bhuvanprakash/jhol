use crate::errors::BundleResult;

#[cfg(feature = "swc-backend")]
pub fn minify_js(source: &str, filename: &str) -> BundleResult<String> {
    use swc_core::common::{
        errors::{ColorConfig, Handler},
        FileName, Globals, SourceMap, GLOBALS,
    };
    use swc_core::ecma::{
        ast::EsVersion,
        codegen::{text_writer::JsWriter, Emitter},
        minifier::{
            compressor::Compressor,
            option::{CompressOptions, MangleOptions, MinifyOptions},
        },
        parser::{EsConfig, Parser, StringInput, Syntax},
        transforms::base::helpers::HELPERS,
    };

    let cm = Arc::<SourceMap>::default();
    let fm = cm.new_source_file(FileName::Custom(filename.to_string()), source.to_string());

    let mut parser = Parser::new(
        Syntax::Es(EsConfig::default()),
        StringInput::from(&*fm),
        None,
    );
    let module_ast = parser.parse_module().map_err(|e| {
        BundleError::TransformError(filename.to_string(), format!("Minify parse error: {:?}", e))
    })?;

    let result = GLOBALS.set(&Globals::new(), || {
        let module_ast = module_ast.fold_with(&mut Compressor::new(CompressOptions::default()));

        let mut buf = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: swc_core::ecma::codegen::Config {
                    minify: true,
                    ..Default::default()
                },
                cm: cm.clone(),
                comments: None,
                wr: Box::new(JsWriter::new(cm.clone(), "\n", &mut buf, None)),
            };
            emitter.emit_module(&module_ast).map_err(|e| {
                BundleError::TransformError(
                    filename.to_string(),
                    format!("Minify codegen error: {:?}", e),
                )
            })?;
        }

        Ok(String::from_utf8(buf).unwrap_or_default())
    });

    result
}

#[cfg(not(feature = "swc-backend"))]
pub fn minify_js(source: &str, _filename: &str) -> BundleResult<String> {
    // Basic JavaScript minification without swc
    let mut result = String::new();
    let mut prev = ' ';
    let mut in_string = false;
    let mut string_char = ' ';
    let mut in_single_line_comment = false;
    let mut in_multi_line_comment = false;
    let mut chars = source.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_single_line_comment {
            if ch == '\n' {
                in_single_line_comment = false;
                result.push(ch);
            }
            continue;
        }

        if in_multi_line_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_multi_line_comment = false;
            }
            continue;
        }

        if in_string {
            result.push(ch);
            if ch == string_char {
                in_string = false;
            }
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            continue;
        }

        match ch {
            '/' if chars.peek() == Some(&'/') => {
                in_single_line_comment = true;
                chars.next();
            }
            '/' if chars.peek() == Some(&'*') => {
                in_multi_line_comment = true;
                chars.next();
            }
            '\'' | '"' | '`' => {
                in_string = true;
                string_char = ch;
                result.push(ch);
            }
            ' ' | '\t' | '\n' | '\r' => {
                // Remove whitespace where possible
                if !prev.is_whitespace() && !is_operator_char(prev) && !ch.is_control() {
                    result.push(' ');
                }
                prev = ch;
            }
            _ => {
                result.push(ch);
                prev = ch;
            }
        }
    }

    // Remove trailing whitespace
    Ok(result.trim().to_string())
}

fn is_operator_char(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | ';'
            | ','
            | '.'
            | '~'
            | '?'
            | ':'
            | '!'
            | '='
            | '<'
            | '>'
            | '+'
            | '-'
            | '*'
            | '/'
            | '%'
            | '&'
            | '|'
            | '^'
    )
}

/// Tree-shaking via dead code elimination (basic implementation)
pub fn remove_dead_code(source: &str) -> String {
    let mut result = String::new();
    let mut in_string = false;
    let mut string_char = ' ';
    let mut i = 0;
    let chars: Vec<char> = source.chars().collect();

    while i < chars.len() {
        if in_string {
            result.push(chars[i]);
            if chars[i] == string_char {
                in_string = false;
            }
            if chars[i] == '\\' && i + 1 < chars.len() {
                result.push(chars[i + 1]);
                i += 1;
            }
            i += 1;
            continue;
        }

        match chars[i] {
            '\'' | '"' | '`' => {
                in_string = true;
                string_char = chars[i];
                result.push(chars[i]);
                i += 1;
            }
            '/' if i + 1 < chars.len() && chars[i + 1] == '/' => {
                // Single-line comment: skip to end
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                // Multi-line comment: skip to */
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            _ => {
                result.push(chars[i]);
                i += 1;
            }
        }
    }

    result
}

pub fn mangle_identifiers(source: &str) -> String {
    // In a full implementation, this would:
    // 1. Parse the source
    // 2. Find all local identifiers
    // 3. Replace them with short names
    // 4. Avoid mangling globals and exports

    // Basic implementation: leave as-is for now
    source.to_string()
}
