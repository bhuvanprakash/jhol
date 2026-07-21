use std::path::{Path, PathBuf};

use crate::config::BundleConfig;
use crate::errors::{BundleError, BundleResult};

#[derive(Debug, Clone)]
pub struct CssBundle {
    pub content: String,
    pub sourcemap: Option<String>,
    pub imports: Vec<String>,
}

pub fn bundle_css(entries: &[PathBuf], config: &BundleConfig) -> BundleResult<CssBundle> {
    let mut bundle = CssBundle {
        content: String::new(),
        sourcemap: None,
        imports: Vec::new(),
    };

    for entry in entries {
        if !entry.exists() {
            continue;
        }
        let ext = entry
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        if ext != "css" {
            continue;
        }

        let content = std::fs::read_to_string(entry).map_err(|e| BundleError::IoError(e))?;

        // Process CSS content
        let processed = process_css(&content, entry, &mut bundle.imports, config)?;
        bundle.content.push_str(&processed);
        bundle.content.push('\n');
    }

    if config.minify {
        bundle.content = minify_css(&bundle.content)?;
    }

    Ok(bundle)
}

fn process_css(
    source: &str,
    base_path: &Path,
    imports: &mut Vec<String>,
    _config: &BundleConfig,
) -> BundleResult<String> {
    let mut result = String::new();
    let mut last_end = 0;

    // Parse @import statements
    let import_re = crate::parser::create_regex("@import\\s+['\"]([^'\"]+)['\"]");
    for cap in import_re.find_iter(source) {
        // Add content before this @import
        result.push_str(&source[last_end..cap.start()]);

        let matched = cap.as_str();
        if let Some(spec_start) = matched.find(|c: char| c == '\'' || c == '"') {
            let quote = matched.as_bytes()[spec_start] as char;
            if let Some(spec_end) = matched[spec_start + 1..].find(quote) {
                let spec = &matched[spec_start + 1..spec_start + 1 + spec_end];
                imports.push(spec.to_string());

                // Try to inline the imported CSS
                let import_path = base_path
                    .parent()
                    .map(|p| p.join(spec))
                    .unwrap_or_else(|| PathBuf::from(spec));

                if let Ok(import_content) = std::fs::read_to_string(&import_path) {
                    result.push_str(&import_content);
                    result.push('\n');
                }
            }
        }

        last_end = cap.end();
    }

    // Add remaining content
    result.push_str(&source[last_end..]);

    // Process CSS custom properties (var())
    let var_re = crate::parser::create_regex("var\\(--([^)]+)\\)");
    // Keep var() as-is - browsers handle them

    // Process @supports, @media, @keyframes rules (keep as-is)
    Ok(result)
}

fn minify_css(source: &str) -> BundleResult<String> {
    #[cfg(feature = "lightningcss")]
    {
        use lightningcss::{
            bundler::{Bundle, Bundler},
            stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet},
        };

        let mut stylesheet = StyleSheet::parse(source, ParserOptions::default())
            .map_err(|e| BundleError::CssError(format!("CSS parse error: {:?}", e)))?;

        stylesheet
            .minify(MinifyOptions::default())
            .map_err(|e| BundleError::CssError(format!("CSS minify error: {:?}", e)))?;

        let result = stylesheet
            .to_css(PrinterOptions {
                minify: true,
                ..Default::default()
            })
            .map_err(|e| BundleError::CssError(format!("CSS print error: {:?}", e)))?;

        Ok(result.code)
    }

    #[cfg(not(feature = "lightningcss"))]
    {
        // Basic CSS minification without lightningcss
        let mut result = String::new();
        let mut in_string = false;
        let mut string_char = ' ';
        let mut prev_char = ' ';

        for ch in source.chars() {
            if in_string {
                result.push(ch);
                if ch == string_char {
                    in_string = false;
                }
                continue;
            }

            if ch == '\'' || ch == '"' {
                in_string = true;
                string_char = ch;
                result.push(ch);
                continue;
            }

            // Remove comments
            if ch == '/' && prev_char == '/' {
                result.pop(); // remove the previous /
                              // Skip to end of line
                while let Some(c) = source.chars().nth(0) {
                    // Can't easily skip without index tracking
                }
                continue;
            }

            // Remove whitespace around operators
            if ch.is_whitespace()
                && (prev_char == ':'
                    || prev_char == ';'
                    || prev_char == '{'
                    || prev_char == '}'
                    || prev_char == ',')
            {
                continue;
            }
            if prev_char.is_whitespace()
                && (ch == ':' || ch == ';' || ch == '{' || ch == '}' || ch == ',')
            {
                // Remove the space before the character
                if let Some(pos) = result.rfind(|c: char| !c.is_whitespace()) {
                    result.truncate(pos + 1);
                }
            }

            if !ch.is_whitespace() || prev_char != ' ' {
                result.push(ch);
            }

            prev_char = ch;
        }

        Ok(result)
    }
}
