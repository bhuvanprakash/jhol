use std::collections::HashMap;
use std::path::Path;

use crate::errors::{BundleError, BundleResult};

/// Plugin hook: called before module transformation
pub type BeforeTransformFn = Box<dyn Fn(&str, &str) -> BundleResult<String> + Send + Sync>;

/// Plugin hook: called after module transformation
pub type AfterTransformFn = Box<dyn Fn(&str, &str) -> BundleResult<String> + Send + Sync>;

/// Plugin hook: called before code generation
pub type BeforeCodegenFn =
    Box<dyn Fn(&HashMap<String, String>) -> BundleResult<HashMap<String, String>> + Send + Sync>;

/// Plugin hook: called after code generation
pub type AfterCodegenFn =
    Box<dyn Fn(&HashMap<String, String>, &[u8]) -> BundleResult<Vec<u8>> + Send + Sync>;

/// Plugin trait for extending the bundler
pub trait BundlePlugin: Send + Sync {
    fn name(&self) -> &'static str;

    fn on_load(&self, _path: &Path) -> BundleResult<Option<String>> {
        Ok(None)
    }

    fn on_transform(&self, _source: &str, _path: &Path) -> BundleResult<Option<String>> {
        Ok(None)
    }

    fn on_generate(&self, _output: &str, _format: &str) -> BundleResult<Option<String>> {
        Ok(None)
    }

    fn on_start(&self) -> BundleResult<()> {
        Ok(())
    }

    fn on_end(&self, _result: &BundleResultInfo) -> BundleResult<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct BundleResultInfo {
    pub files_created: Vec<String>,
    pub total_size: u64,
    pub duration_ms: u64,
    pub module_count: usize,
}

/// Plugin registry
pub struct PluginRegistry {
    plugins: Vec<Box<dyn BundlePlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn BundlePlugin>) {
        self.plugins.push(plugin);
    }

    pub fn run_on_start(&self) -> BundleResult<()> {
        for plugin in &self.plugins {
            plugin.on_start()?;
        }
        Ok(())
    }

    pub fn run_on_load(&self, path: &Path) -> BundleResult<Option<String>> {
        for plugin in &self.plugins {
            if let Some(result) = plugin.on_load(path)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    pub fn run_on_transform(&self, source: &str, path: &Path) -> BundleResult<Option<String>> {
        for plugin in &self.plugins {
            if let Some(result) = plugin.on_transform(source, path)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    pub fn run_on_generate(&self, output: &str, format: &str) -> BundleResult<Option<String>> {
        for plugin in &self.plugins {
            if let Some(result) = plugin.on_generate(output, format)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    pub fn run_on_end(&self, info: &BundleResultInfo) -> BundleResult<()> {
        for plugin in &self.plugins {
            plugin.on_end(info)?;
        }
        Ok(())
    }
}

/// Built-in plugin: JSON import support
pub struct JsonPlugin;

impl BundlePlugin for JsonPlugin {
    fn name(&self) -> &'static str {
        "json"
    }

    fn on_load(&self, path: &Path) -> BundleResult<Option<String>> {
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let content = std::fs::read_to_string(path).map_err(|e| BundleError::IoError(e))?;
            let parsed: serde_json::Value =
                serde_json::from_str(&content).map_err(|e| BundleError::JsonError(e))?;
            Ok(Some(format!("export default {};", parsed)))
        } else {
            Ok(None)
        }
    }
}

/// Built-in plugin: CSS import support
pub struct CssPlugin;

impl BundlePlugin for CssPlugin {
    fn name(&self) -> &'static str {
        "css"
    }

    fn on_load(&self, path: &Path) -> BundleResult<Option<String>> {
        if path.extension().and_then(|e| e.to_str()) == Some("css") {
            let content = std::fs::read_to_string(path).map_err(|e| BundleError::IoError(e))?;
            // Wrap CSS in a style injection script
            Ok(Some(format!(
                "const style = document.createElement('style');\n\
                 style.textContent = {};\n\
                 document.head.appendChild(style);\n\
                 export default style;",
                serde_json::to_string(&content).map_err(|e| BundleError::JsonError(e))?
            )))
        } else {
            Ok(None)
        }
    }
}
