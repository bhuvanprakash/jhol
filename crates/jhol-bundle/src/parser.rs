use std::path::{Path, PathBuf};

#[cfg(feature = "swc-backend")]
use swc_core::common::{
    errors::{ColorConfig, Handler},
    FileName, Globals, SourceMap, GLOBALS,
};
#[cfg(feature = "swc-backend")]
use swc_core::ecma::{
    ast::{EsVersion, Program},
    parser::{EsConfig, Parser, StringInput, Syntax, TsConfig},
};

use crate::errors::BundleResult;
use regex_lite::Regex; // our inline module below

#[derive(Debug, Clone)]
pub enum FileType {
    JavaScript,
    TypeScript,
    Jsx,
    Tsx,
    Css,
    Json,
    Wasm,
    Other(String),
}

impl FileType {
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "js" | "mjs" | "cjs" => FileType::JavaScript,
            "jsx" => FileType::Jsx,
            "ts" | "mts" => FileType::TypeScript,
            "tsx" => FileType::Tsx,
            "css" => FileType::Css,
            "json" => FileType::Json,
            "wasm" => FileType::Wasm,
            other => FileType::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportRecord {
    pub specifier: String,
    pub kind: ImportKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportKind {
    StaticImport,
    DynamicImport,
    Require,
    ReExport,
}

#[derive(Debug, Clone)]
pub struct ExportRecord {
    pub name: String,
    pub alias: Option<String>,
    pub source: Option<String>,
    pub kind: ExportKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportKind {
    Named,
    Default,
    All,
}

#[derive(Debug, Clone)]
pub struct ParsedModule {
    pub path: PathBuf,
    pub file_type: FileType,
    pub source: String,
    pub imports: Vec<ImportRecord>,
    pub exports: Vec<ExportRecord>,
    pub ast: ParsedAst,
}

#[derive(Debug, Clone)]
pub enum ParsedAst {
    #[cfg(feature = "swc-backend")]
    SwcModule(Box<swc_core::ecma::ast::Module>),
    Text(String),
    Json(serde_json::Value),
    Css(String),
}

pub fn parse_file(path: &Path, source: &str, file_type: &FileType) -> BundleResult<ParsedModule> {
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    match file_type {
        FileType::JavaScript | FileType::Jsx | FileType::TypeScript | FileType::Tsx => {
            parse_imports_exports(source, &mut imports, &mut exports);
        }
        FileType::Css => {
            parse_css_imports(source, &mut imports);
        }
        FileType::Json => {
            // JSON doesn't have imports/exports in the traditional sense
        }
        _ => {}
    }

    Ok(ParsedModule {
        path: path.to_path_buf(),
        file_type: file_type.clone(),
        source: source.to_string(),
        imports,
        exports,
        ast: match file_type {
            FileType::Json => {
                let v: serde_json::Value =
                    serde_json::from_str(source).unwrap_or(serde_json::Value::Null);
                ParsedAst::Json(v)
            }
            FileType::Css => ParsedAst::Css(source.to_string()),
            _ => ParsedAst::Text(source.to_string()),
        },
    })
}

fn parse_imports_exports(
    source: &str,
    imports: &mut Vec<ImportRecord>,
    exports: &mut Vec<ExportRecord>,
) {
    // Static imports: import ... from "..."
    let import_re = Regex::new("import\\s+(?:(?:\\{[^}]*\\}|\\*\\s+as\\s+\\w+|\\w+(?:\\s*,\\s*(?:\\{[^}]*\\}|\\*\\s+as\\s+\\w+))?)\\s+from\\s+)?['\"]([^'\"]+)['\"]\\s*;?");
    for cap in import_re.find_iter(source) {
        let matched = cap.as_str();
        let start = cap.start();
        let end = cap.end();

        // Extract the module specifier
        if let Some(spec_start) = matched.find(|c: char| c == '\'' || c == '"') {
            let quote = matched.as_bytes()[spec_start] as char;
            if let Some(spec_end) = matched[spec_start + 1..].find(quote) {
                let spec = &matched[spec_start + 1..spec_start + 1 + spec_end];
                let kind = if matched.starts_with("import ") {
                    ImportKind::StaticImport
                } else if matched.starts_with("export ") {
                    ImportKind::ReExport
                } else {
                    ImportKind::StaticImport
                };
                imports.push(ImportRecord {
                    specifier: spec.to_string(),
                    kind,
                    start,
                    end,
                });
            }
        }
    }

    // Dynamic imports: import(...)
    let dynamic_re = Regex::new("import\\s*\\(\\s*['\"]([^'\"]+)['\"]\\s*\\)");
    for cap in dynamic_re.find_iter(source) {
        let matched = cap.as_str();
        let start = cap.start();
        let end = cap.end();
        if let Some(spec_start) = matched.find(|c: char| c == '\'' || c == '"') {
            let quote = matched.as_bytes()[spec_start] as char;
            if let Some(spec_end) = matched[spec_start + 1..].find(quote) {
                let spec = &matched[spec_start + 1..spec_start + 1 + spec_end];
                imports.push(ImportRecord {
                    specifier: spec.to_string(),
                    kind: ImportKind::DynamicImport,
                    start,
                    end,
                });
            }
        }
    }

    // require calls: require("...")
    let require_re = Regex::new("require\\s*\\(\\s*['\"]([^'\"]+)['\"]\\s*\\)");
    for cap in require_re.find_iter(source) {
        let matched = cap.as_str();
        let start = cap.start();
        let end = cap.end();
        if let Some(spec_start) = matched.find(|c: char| c == '\'' || c == '"') {
            let quote = matched.as_bytes()[spec_start] as char;
            if let Some(spec_end) = matched[spec_start + 1..].find(quote) {
                let spec = &matched[spec_start + 1..spec_start + 1 + spec_end];
                imports.push(ImportRecord {
                    specifier: spec.to_string(),
                    kind: ImportKind::Require,
                    start,
                    end,
                });
            }
        }
    }

    // Exports: export default, export const, export function, export class
    let export_re = Regex::new("export\\s+(?:default\\s+)?(?:const|let|var|function|class|interface|type|enum|abstract|async)\\s+(\\w+)");
    for cap in export_re.find_iter(source) {
        let matched = cap.as_str();
        if let Some(name_start) = matched.rfind(|c: char| c == ' ' || c == '\t') {
            let name = matched[name_start + 1..].trim();
            let is_default = matched.contains("default");
            exports.push(ExportRecord {
                name: name.to_string(),
                alias: if is_default {
                    Some("default".to_string())
                } else {
                    None
                },
                source: None,
                kind: if is_default {
                    ExportKind::Default
                } else {
                    ExportKind::Named
                },
            });
        }
    }

    // Re-exports: export ... from "..."
    let reexport_re = Regex::new("export\\s+(?:\\{[^}]*\\}|\\*)\\s+from\\s+['\"]([^'\"]+)['\"]");
    for cap in reexport_re.find_iter(source) {
        let matched = cap.as_str();
        if let Some(spec_start) = matched.find(|c: char| c == '\'' || c == '"') {
            let quote = matched.as_bytes()[spec_start] as char;
            if let Some(spec_end) = matched[spec_start + 1..].find(quote) {
                let spec = &matched[spec_start + 1..spec_start + 1 + spec_end];
                imports.push(ImportRecord {
                    specifier: spec.to_string(),
                    kind: ImportKind::ReExport,
                    start: cap.start(),
                    end: cap.end(),
                });
            }
        }
    }
}

fn parse_css_imports(source: &str, imports: &mut Vec<ImportRecord>) {
    let import_re = Regex::new("@import\\s+['\"]([^'\"]+)['\"]");
    for cap in import_re.find_iter(source) {
        let matched = cap.as_str();
        if let Some(spec_start) = matched.find(|c: char| c == '\'' || c == '"') {
            let quote = matched.as_bytes()[spec_start] as char;
            if let Some(spec_end) = matched[spec_start + 1..].find(quote) {
                let spec = &matched[spec_start + 1..spec_start + 1 + spec_end];
                imports.push(ImportRecord {
                    specifier: spec.to_string(),
                    kind: ImportKind::StaticImport,
                    start: cap.start(),
                    end: cap.end(),
                });
            }
        }
    }
}

// Simple regex-lite implementation
mod regex_lite {
    pub struct Match {
        start: usize,
        end: usize,
        text: String,
    }

    impl Match {
        pub fn start(&self) -> usize {
            self.start
        }
        pub fn end(&self) -> usize {
            self.end
        }
        pub fn as_str(&self) -> &str {
            &self.text
        }
    }

    pub struct Regex {
        pattern: String,
    }

    impl Regex {
        pub fn new(pattern: &str) -> Self {
            Self {
                pattern: pattern.to_string(),
            }
        }

        pub fn find_iter<'a>(&'a self, text: &'a str) -> RegexIter<'a> {
            RegexIter {
                pattern: &self.pattern,
                text,
                pos: 0,
            }
        }
    }

    pub struct RegexIter<'a> {
        pattern: &'a str,
        text: &'a str,
        pos: usize,
    }

    impl<'a> Iterator for RegexIter<'a> {
        type Item = Match;

        fn next(&mut self) -> Option<Self::Item> {
            // Simple substring matching for the patterns we use
            // Patterns we support:
            // 1. import "...": literal "import" followed by optional stuff and quoted string
            // 2. import(...): "import(" followed by quoted string
            // 3. require(...): "require(" followed by quoted string
            // 4. export...from "...": "from" + quoted string
            // 5. @import "...": "@import" + quoted string

            let remaining = &self.text[self.pos..];

            // Pattern: static import/re-export
            if self.pattern.contains("import") || self.pattern.contains("export") {
                // Look for import/export keyword
                let keywords = ["import", "export", "require", "@import"];
                for kw in &keywords {
                    if let Some(idx) = remaining.find(kw) {
                        let start = self.pos + idx;
                        let after_kw = &remaining[idx..];

                        // Find the first quoted string after the keyword
                        let mut in_quote = false;
                        let mut quote_char = ' ';
                        let mut spec_start = 0;
                        let mut spec_end = 0;

                        for (i, ch) in after_kw.char_indices() {
                            if !in_quote && (ch == '\'' || ch == '"') {
                                in_quote = true;
                                quote_char = ch;
                                spec_start = self.pos + idx + i + 1;
                            } else if in_quote && ch == quote_char {
                                spec_end = self.pos + idx + i;
                                in_quote = false;

                                let end = spec_end + 1;
                                let text = self.text[start..end].to_string();
                                self.pos = end;
                                return Some(Match { start, end, text });
                            }
                        }

                        // If we found a keyword but no quoted string, skip past it
                        self.pos = self.pos + idx + kw.len();
                        return self.next();
                    }
                }
            }

            None
        }
    }
}

pub fn create_regex(pattern: &str) -> regex_lite::Regex {
    regex_lite::Regex::new(pattern)
}
