use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum OutputFormat {
    ESM,
    CJS,
    IIFE,
}

impl OutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::ESM => "esm",
            OutputFormat::CJS => "cjs",
            OutputFormat::IIFE => "iife",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "esm" | "module" => Some(OutputFormat::ESM),
            "cjs" | "commonjs" => Some(OutputFormat::CJS),
            "iife" => Some(OutputFormat::IIFE),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Hash)]
pub enum Target {
    Browser,
    Node,
    Jhol,
}

impl Target {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "browser" => Some(Target::Browser),
            "node" | "nodejs" => Some(Target::Node),
            "jhol" => Some(Target::Jhol),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum SourceMapOption {
    None,
    Inline,
    External,
    Both,
}

#[derive(Debug, Clone)]
pub struct BundleConfig {
    /// Entry point file paths
    pub entries: Vec<PathBuf>,

    /// Output directory
    pub out_dir: PathBuf,

    /// Output format
    pub format: OutputFormat,

    /// Target platform
    pub target: Target,

    /// Enable minification
    pub minify: bool,

    /// Source map generation
    pub sourcemap: SourceMapOption,

    /// External modules (not bundled)
    pub external: Vec<String>,

    /// Whether to bundle node_modules
    pub bundle_node_modules: bool,

    /// Whether to bundle CSS
    pub bundle_css: bool,

    /// Enable tree shaking
    pub tree_shake: bool,

    /// Enable code splitting
    pub code_splitting: bool,

    /// Chunk naming pattern (e.g. "[name]-[hash].js")
    pub chunk_pattern: String,

    /// Global variable name for IIFE
    pub global_name: Option<String>,

    /// Define/replace constants (like process.env.NODE_ENV)
    pub define: HashMap<String, String>,

    /// Whether to generate a single file
    pub single_file: bool,

    /// Platform-specific externals
    pub platform_externals: Vec<String>,

    /// Enable watch mode
    pub watch: bool,

    /// Print build stats
    pub stats: bool,

    /// Tsconfig path
    pub tsconfig: Option<PathBuf>,

    /// JSX runtime (automatic/classic)
    pub jsx_runtime: JsxRuntime,

    /// React import source (for automatic JSX runtime)
    pub jsx_import_source: String,

    /// Target ECMAScript version for output
    pub ecma_version: EcmaVersion,
}

#[derive(Debug, Clone, Hash)]
pub enum JsxRuntime {
    Automatic,
    Classic,
}

#[derive(Debug, Clone)]
pub enum EcmaVersion {
    Es2015,
    Es2016,
    Es2017,
    Es2018,
    Es2019,
    Es2020,
    Es2021,
    Es2022,
    Es2023,
    EsNext,
}

impl Default for BundleConfig {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            out_dir: PathBuf::from("dist"),
            format: OutputFormat::ESM,
            target: Target::Browser,
            minify: false,
            sourcemap: SourceMapOption::None,
            external: Vec::new(),
            bundle_node_modules: true,
            bundle_css: true,
            tree_shake: true,
            code_splitting: false,
            chunk_pattern: "[name]-[hash].js".to_string(),
            global_name: None,
            define: HashMap::new(),
            single_file: false,
            platform_externals: vec![
                "fs".to_string(),
                "path".to_string(),
                "os".to_string(),
                "crypto".to_string(),
                "http".to_string(),
                "https".to_string(),
                "stream".to_string(),
                "buffer".to_string(),
                "util".to_string(),
                "events".to_string(),
                "child_process".to_string(),
                "net".to_string(),
                "tls".to_string(),
                "url".to_string(),
                "querystring".to_string(),
                "zlib".to_string(),
                "assert".to_string(),
                "tty".to_string(),
                "dns".to_string(),
                "module".to_string(),
                "worker_threads".to_string(),
                "perf_hooks".to_string(),
                "async_hooks".to_string(),
                "v8".to_string(),
                "vm".to_string(),
                "string_decoder".to_string(),
                "readline".to_string(),
                "cluster".to_string(),
                "dgram".to_string(),
                "domain".to_string(),
                "punycode".to_string(),
                "repl".to_string(),
            ],
            watch: false,
            stats: false,
            tsconfig: None,
            jsx_runtime: JsxRuntime::Automatic,
            jsx_import_source: "react".to_string(),
            ecma_version: EcmaVersion::EsNext,
        }
    }
}

pub fn load_config_from_args() -> BundleConfig {
    BundleConfig::default()
}
