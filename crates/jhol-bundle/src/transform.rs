//! Jhol Bundle — Transform Module (CAS-first)
//!
//! Transforms ParseManifests into output JavaScript. Results are stored in CAS
//! keyed by (source_content_hash, config_hash).  The same source with the same
//! config is transformed exactly once — on subsequent builds, the result is
//! returned from CAS instantly.
//!
//! Transform pipeline (each step cached independently):
//!   1. TypeScript strip (remove type annotations) — swc
//!   2. JSX transform (React.createElement / automatic runtime) — swc
//!   3. Env define replacement (process.env.NODE_ENV, etc.) — string replace
//!   4. Minify (if config.minify) — swc minifier
//!
//! CSS and JSON are passed through unchanged (no transform needed).

use std::collections::HashMap;
use std::path::Path;

use rayon::prelude::*;

use crate::cas::ParseManifest;
use crate::cas::{bundle_cas_store, TransformedOutput};
use crate::config::BundleConfig;
use crate::errors::{BundleError, BundleResult};
use crate::ingest::FileType;

/// Compute a config hash — changes when transform output would differ.
/// Includes: minify, target, jsx_runtime, jsx_import_source, define keys.
pub fn compute_config_hash(config: &BundleConfig) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    config.minify.hash(&mut h);
    config.target.hash(&mut h);
    config.jsx_runtime.hash(&mut h);
    config.jsx_import_source.hash(&mut h);
    // Include sorted define keys (values don't affect structure)
    let mut def_keys: Vec<_> = config.define.keys().collect();
    def_keys.sort();
    def_keys.hash(&mut h);
    h.finish()
}

/// Transform a ParseManifest into TransformedOutput.
/// Checks CAS first; only transforms if not cached.
pub fn transform_manifest(
    manifest: &ParseManifest,
    config: &BundleConfig,
) -> BundleResult<Arc<TransformedOutput>> {
    let source_hash = manifest.content_hash;
    let config_hash = compute_config_hash(config);

    // CAS lookup first — the key insight of CAS-first bundling
    if let Some(cas_guard) = bundle_cas_store().read().unwrap().as_ref() {
        if let Some(cached) = cas_guard.get_transformed(source_hash, config_hash) {
            return Ok(cached);
        }
    }

    let file_type = FileType::from_path(Path::new(&manifest.path));

    // Transform based on file type
    let code = match file_type {
        FileType::Json => {
            // JSON → JS export default
            format!("export default {};", manifest.source)
        }
        FileType::Css => {
            // CSS is handled by the CSS module — return as-is
            manifest.source.clone()
        }
        FileType::TypeScript | FileType::Tsx => {
            #[cfg(feature = "swc-backend")]
            {
                transform_typescript(&manifest.source, file_type, config)?
            }
            #[cfg(not(feature = "swc-backend"))]
            {
                return Err(BundleError::TransformError(
                    manifest.path.to_string_lossy().into_owned(),
                    "TypeScript transformation requires swc-backend feature".to_string(),
                ));
            }
        }
        FileType::Jsx => {
            #[cfg(feature = "swc-backend")]
            {
                transform_jsx(&manifest.source, config)?
            }
            #[cfg(not(feature = "swc-backend"))]
            {
                transform_jsx_simple(&manifest.source)?
            }
        }
        FileType::JavaScript | FileType::Other => apply_defines(&manifest.source, config),
        FileType::Wasm => {
            return Err(BundleError::TransformError(
                manifest.path.to_string_lossy().into_owned(),
                "WASM files must be handled as binary".to_string(),
            ));
        }
    };

    let final_code = if config.minify {
        #[cfg(feature = "swc-backend")]
        {
            minify_javascript(&code, &manifest.path)?
        }
        #[cfg(not(feature = "swc-backend"))]
        {
            crate::minify::minify_js(&code, &*manifest.path.to_string_lossy())?
        }
    } else {
        code
    };

    let output = TransformedOutput {
        source_hash,
        config_hash,
        code: final_code,
        sourcemap: None,
    };

    // Store in CAS
    if let Some(cas_guard) = bundle_cas_store().read().unwrap().as_ref() {
        let _ = cas_guard.put_transformed(&output);
    }

    Ok(Arc::new(output))
}

/// Transform all manifests in parallel using Rayon.
/// Returns a HashMap from content_hash → TransformedOutput.
pub fn transform_all(
    manifests: &[ParseManifest],
    config: &BundleConfig,
) -> BundleResult<HashMap<u64, Arc<TransformedOutput>>> {
    let config_hash = compute_config_hash(config);
    let mut results: HashMap<u64, Arc<TransformedOutput>> = HashMap::new();
    let mut needs_transform: Vec<u64> = Vec::new();
    let mut pending: Vec<(usize, &ParseManifest)> = Vec::new();

    // Phase 1: check CAS for all manifests in parallel
    if let Some(cas_guard) = bundle_cas_store().read().unwrap().as_ref() {
        for manifest in manifests {
            let key = crate::cas::CasStore::composite_key(manifest.content_hash, config_hash);
            if let Some(cached) = cas_guard.get_transformed(manifest.content_hash, config_hash) {
                results.insert(manifest.content_hash, cached);
            } else {
                needs_transform.push(manifest.content_hash);
                pending.push((results.len(), manifest));
            }
        }
    }

    // Phase 2: transform the ones not in CAS in parallel
    let transformed: Vec<Arc<TransformedOutput>> = pending
        .par_iter()
        .map(|(_, manifest)| transform_manifest(manifest, config))
        .collect::<Result<Vec<_>, _>>()?;

    for (i, (_, manifest)) in pending.into_iter().enumerate() {
        let output = transformed[i].clone();
        results.insert(manifest.content_hash, output);
    }

    Ok(results)
}

// ── TypeScript strip ─────────────────────────────────────────────────────────

#[cfg(feature = "swc-backend")]
fn transform_typescript(
    source: &str,
    file_type: FileType,
    config: &BundleConfig,
) -> BundleResult<String> {
    use swc_core::common::{errors::Handler, FileName, Globals, SourceMap, GLOBALS};
    use swc_core::ecma::{
        codegen::{text_writer::JsWriter as CodeGenWriter, Emitter},
        parser::{Parser, StringInput, Syntax, TsConfig},
        transforms::base::fixer::fixer,
        transforms::typescript::strip,
    };

    let cm = Arc::<SourceMap>::default();
    let fm = cm.new_source_file(FileName::Custom("inline".into()), source.to_string());

    let syntax = Syntax::Typescript(TsConfig {
        tsx: matches!(file_type, FileType::Tsx),
        decorators: true,
        ..Default::default()
    });

    let handler = Handler::with_tty_emitter(
        swc_core::common::errors::ColorConfig::Never,
        false,
        false,
        Some(cm.clone()),
    );
    let mut parser = Parser::new(syntax, StringInput::from(&*fm), Some(&handler));
    let module = parser.parse_module().map_err(|e| BundleError::ParseError {
        path: "inline".to_string(),
        line: e.loc().map(|l| l.row + 1).unwrap_or(1),
        column: e.loc().map(|l| l.col + 1).unwrap_or(1),
        message: format!("{:?}", e),
    })?;

    let result = GLOBALS.set(&Globals::new(), || {
        let module = module.fold_with(&mut strip::strip_with_config(
            swc_core::ecma::transforms::typescript::Config {
                import_not_used_as_values:
                    swc_core::ecma::transforms::typescript::ImportsNotUsedAsValues::Remove,
                ..Default::default()
            },
            Default::default(),
        ));

        // Apply JSX transform if TSX
        let module = if matches!(file_type, FileType::Tsx) {
            module.fold_with(&mut jsx_transform(cm.clone(), config))
        } else {
            module
        };

        let module = module.fold_with(&mut fixer(None));

        let mut buf = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: swc_core::ecma::codegen::Config::default(),
                cm: cm.clone(),
                comments: None,
                wr: Box::new(CodeGenWriter::new(cm.clone(), "\n", &mut buf, None)),
            };
            emitter.emit_module(&module).map_err(|e| {
                BundleError::TransformError("inline".to_string(), format!("codegen: {:?}", e))
            })?;
        }

        let code = String::from_utf8(buf).unwrap_or_default();
        Ok(apply_defines(&code, config))
    });

    result
}

#[cfg(feature = "swc-backend")]
fn jsx_transform(cm: Arc<SourceMap>, config: &BundleConfig) -> impl swc_core::ecma::visit::Fold {
    use swc_core::ecma::ast::Module;
    use swc_core::ecma::transforms::react::jsx;

    let runtime = match config.jsx_runtime {
        crate::config::JsxRuntime::Automatic => swc_core::ecma::transforms::react::Jsx::Automatic {
            import_source: config.jsx_import_source.clone(),
        },
        crate::config::JsxRuntime::Classic => swc_core::ecma::transforms::react::Jsx::Classic {
            import_source: config.jsx_import_source.clone(),
            ..Default::default()
        },
    };

    jsx(
        cm,
        None,
        swc_core::ecma::transforms::react::Options {
            runtime,
            development: false,
            ..Default::default()
        },
    )
}

#[cfg(feature = "swc-backend")]
fn transform_jsx(source: &str, config: &BundleConfig) -> BundleResult<String> {
    use swc_core::common::{errors::Handler, FileName, Globals, SourceMap, GLOBALS};
    use swc_core::ecma::{
        codegen::{text_writer::JsWriter as CodeGenWriter, Emitter},
        parser::{EsConfig, Parser, StringInput, Syntax},
        transforms::base::fixer::fixer,
    };

    let cm = Arc::<SourceMap>::default();
    let fm = cm.new_source_file(FileName::Custom("inline".into()), source.to_string());

    let mut parser = Parser::new(
        Syntax::Es(EsConfig {
            jsx: true,
            ..Default::default()
        }),
        StringInput::from(&*fm),
        None,
    );
    let module = parser.parse_module().map_err(|e| BundleError::ParseError {
        path: "inline".to_string(),
        line: e.loc().map(|l| l.row + 1).unwrap_or(1),
        column: e.loc().map(|l| l.col + 1).unwrap_or(1),
        message: format!("{:?}", e),
    })?;

    let result = GLOBALS.set(&Globals::new(), || {
        let module = module.fold_with(&mut jsx_transform(cm.clone(), config));
        let module = module.fold_with(&mut fixer(None));

        let mut buf = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: swc_core::ecma::codegen::Config::default(),
                cm: cm.clone(),
                comments: None,
                wr: Box::new(CodeGenWriter::new(cm.clone(), "\n", &mut buf, None)),
            };
            emitter.emit_module(&module).map_err(|e| {
                BundleError::TransformError("inline".to_string(), format!("codegen: {:?}", e))
            })?;
        }

        let code = String::from_utf8(buf).unwrap_or_default();
        Ok(apply_defines(&code, config))
    });

    result
}

// ── Fallback JSX transform (no swc) ───────────────────────────────────────────

#[cfg(not(feature = "swc-backend"))]
fn transform_jsx(source: &str, _config: &BundleConfig) -> BundleResult<String> {
    transform_jsx_simple(source)
}

fn transform_jsx_simple(source: &str) -> BundleResult<String> {
    let mut result = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if i + 1 < chars.len()
            && chars[i] == '<'
            && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '/')
        {
            // Skip comparison operators
            if i > 0 && matches!(chars[i - 1], '=' | '!' | '<' | '>') {
                result.push(chars[i]);
                i += 1;
                continue;
            }

            let is_closing = i + 1 < chars.len() && chars[i + 1] == '/';
            let tag_start = if is_closing { i + 2 } else { i + 1 };
            let mut tag_end = tag_start;
            while tag_end < chars.len()
                && (chars[tag_end].is_ascii_alphanumeric()
                    || chars[tag_end] == '_'
                    || chars[tag_end] == '-'
                    || chars[tag_end] == '.')
            {
                tag_end += 1;
            }
            let tag: String = chars[tag_start..tag_end].iter().collect();

            if is_closing {
                result.push_str(")"); // close React.createElement
                i = tag_end + 1;
                continue;
            }

            // Collect props until '>'
            let mut props_start = tag_end;
            let mut self_closing = false;
            let mut props = String::new();

            while props_start < chars.len() {
                if props_start + 1 < chars.len()
                    && chars[props_start] == '/'
                    && chars[props_start + 1] == '>'
                {
                    self_closing = true;
                    props_start += 2;
                    break;
                }
                if chars[props_start] == '>' {
                    props_start += 1;
                    break;
                }
                props.push(chars[props_start]);
                props_start += 1;
            }

            let props_str = props.trim();
            let props_obj = if props_str.is_empty() {
                "null".to_string()
            } else {
                format!("{{{}}}", props_str)
            };

            if self_closing {
                result.push_str(&format!("React.createElement(\"{}\", {})", tag, props_obj));
            } else {
                result.push_str(&format!("React.createElement(\"{}\", {}, ", tag, props_obj));
            }

            i = props_start;
            continue;
        }
        result.push(chars[i]);
        i += 1;
    }

    Ok(result)
}

// ── Minification ──────────────────────────────────────────────────────────────

#[cfg(feature = "swc-backend")]
fn minify_javascript(source: &str, path: &str) -> BundleResult<String> {
    use swc_core::common::{errors::Handler, FileName, Globals, SourceMap, GLOBALS};
    use swc_core::ecma::{
        codegen::{text_writer::JsWriter as CodeGenWriter, Emitter},
        minifier::{compressor::Compressor, option::CompressOptions},
        parser::{EsConfig, Parser, StringInput, Syntax},
    };

    let cm = Arc::<SourceMap>::default();
    let fm = cm.new_source_file(FileName::Custom(path.to_string()), source.to_string());

    let mut parser = Parser::new(
        Syntax::Es(EsConfig::default()),
        StringInput::from(&*fm),
        None,
    );
    let module = parser.parse_module().map_err(|e| {
        BundleError::TransformError(path.to_string(), format!("minify parse: {:?}", e))
    })?;

    GLOBALS.set(&Globals::new(), || {
        let module = module.fold_with(&mut Compressor::new(CompressOptions::default()));
        let mut buf = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: swc_core::ecma::codegen::Config {
                    minify: true,
                    ..Default::default()
                },
                cm: cm.clone(),
                comments: None,
                wr: Box::new(CodeGenWriter::new(cm.clone(), "\n", &mut buf, None)),
            };
            emitter.emit_module(&module).map_err(|e| {
                BundleError::TransformError(path.to_string(), format!("minify: {:?}", e))
            })?;
        }
        Ok(String::from_utf8(buf).unwrap_or_default())
    })
}

// ── Define replacement ────────────────────────────────────────────────────────

/// Replace define constants in source (e.g. process.env.NODE_ENV → "production").
/// Applied after TypeScript/JSX transform, before minification.
fn apply_defines(source: &str, config: &BundleConfig) -> String {
    if config.define.is_empty() {
        return source.to_string();
    }

    let mut result = source.to_string();
    for (key, value) in &config.define {
        result = result.replace(key, value);
    }
    result
}

use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_defines() {
        let source = "const env = process.env.NODE_ENV;";
        let mut config = BundleConfig::default();
        config
            .define
            .insert("process.env.NODE_ENV".into(), "\"production\"".into());
        let result = apply_defines(source, &config);
        assert_eq!(result, "const env = \"production\";");
    }

    #[test]
    fn test_config_hash_changes_with_minify() {
        let mut config1 = BundleConfig::default();
        let mut config2 = BundleConfig::default();
        config2.minify = true;
        let h1 = compute_config_hash(&config1);
        let h2 = compute_config_hash(&config2);
        assert_ne!(h1, h2);
    }
}
