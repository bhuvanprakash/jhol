//! Optional config from .jholrc or ~/.jholrc (JSON). Merged with env and CLI.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{OnceLock, RwLock};
use std::time::SystemTime;

use crate::Backend;

/// Optional config from file. CLI and env override these.
#[derive(Default, Clone)]
pub struct Config {
    // --- Existing fields ---
    pub backend: Option<Backend>,
    pub cache_dir: Option<String>,
    pub offline: Option<bool>,
    pub frozen: Option<bool>,

    // --- Feature 1: install section ---
    pub no_scripts: Option<bool>,
    pub script_allowlist: Option<Vec<String>>,
    pub fallback_backend: Option<bool>,
    pub concurrency: Option<usize>,
    pub fetch_timeout: Option<u64>,
    pub install_timeout: Option<u64>,
    pub engine_backend: Option<String>,

    // --- Feature 2: registry section ---
    pub registry_default: Option<String>,
    pub registry_scopes: Option<HashMap<String, String>>,
    pub registry_auth_tokens: Option<HashMap<String, String>>,

    // --- Feature 3: audit section ---
    pub audit_gate: Option<bool>,
    pub audit_min_severity: Option<String>,
    pub audit_ignore_advisories: Option<Vec<String>>,

    // --- Feature 4: cache section ---
    pub cache_max_size_gb: Option<f64>,
    pub cache_keep_recent: Option<usize>,
    pub cache_auto_prewarm: Option<bool>,

    // --- Feature 5: workspaces section ---
    pub workspaces_parallel: Option<bool>,
    pub workspaces_bail: Option<bool>,

    // --- Feature 6: hooks section ---
    pub hook_pre_install: Option<String>,
    pub hook_post_install: Option<String>,
    pub hook_pre_audit: Option<String>,
    pub hook_post_audit: Option<String>,

    // --- Feature 7: trustedDependencies ---
    pub trusted_dependencies: Option<Vec<String>>,

    // --- Feature 8: overrides and resolutions ---
    pub overrides: Option<HashMap<String, String>>,
    pub resolutions: Option<HashMap<String, String>>,

    // --- Feature 9: env-specific overrides (internal: raw JSON for deep-merge) ---
    pub env_overrides: Option<HashMap<String, serde_json::Value>>,

    // --- Feature 10: output section ---
    pub output_quiet: Option<bool>,
    pub output_no_color: Option<bool>,
    pub output_json_log: Option<bool>,

    // --- Feature 11: telemetry ---
    pub telemetry: Option<bool>,
}

/// Registry configuration from the `.jholrc` `registry` section.
#[derive(Default, Clone, Debug)]
pub struct JholrcRegistryConfig {
    pub default: Option<String>,
    pub scopes: HashMap<String, String>,
    pub auth_tokens: HashMap<String, String>,
}

/// Load the `.jholrc` registry section from `dir`, then `~/.jholrc`.
pub fn load_jholrc_registry(dir: &Path) -> Option<JholrcRegistryConfig> {
    static CACHE: std::sync::OnceLock<
        std::sync::RwLock<Option<(std::time::SystemTime, JholrcRegistryConfig)>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::RwLock::new(None));

    let home = dirs_home();
    let candidates = [
        dir.join(".jholrc"),
        home.map(|h| h.join(".jholrc"))
            .unwrap_or_else(|| dir.join(".none")),
    ];

    // Find first existing candidate and its mtime
    let (path, mtime) = match candidates.iter().find_map(|p| {
        let m = std::fs::metadata(p).ok()?.modified().ok()?;
        Some((p.clone(), m))
    }) {
        Some(v) => v,
        None => return None,
    };

    // Check cache
    if let Ok(guard) = cache.read() {
        if let Some((cached_mtime, ref cached)) = *guard {
            if cached_mtime == mtime {
                return Some(cached.clone());
            }
        }
    }

    // Read and parse
    let s = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let reg = v.get("registry")?.as_object()?;
    let mut out = JholrcRegistryConfig::default();
    if let Some(d) = reg.get("default").and_then(|x| x.as_str()) {
        out.default = Some(d.trim_end_matches('/').to_string());
    }
    if let Some(scopes) = reg.get("scopes").and_then(|x| x.as_object()) {
        for (k, v) in scopes {
            if let Some(url) = v.as_str() {
                out.scopes
                    .insert(k.clone(), url.trim_end_matches('/').to_string());
            }
        }
    }
    if let Some(tokens) = reg.get("authTokens").and_then(|x| x.as_object()) {
        for (host, tok_val) in tokens {
            if let Some(raw) = tok_val.as_str() {
                let resolved = interpolate_env_vars(raw);
                out.auth_tokens.insert(host.clone(), resolved);
            }
        }
    }
    if let Ok(mut guard) = cache.write() {
        *guard = Some((mtime, out.clone()));
    }
    Some(out)
}

/// Interpolate `${VAR_NAME}` patterns in a string with environment variable values.
pub fn interpolate_env_vars(s: &str) -> String {
    if !s.contains("${") {
        return s.to_string();
    }
    let mut result = s.to_string();
    let mut search_from = 0;
    loop {
        let Some(start) = result[search_from..].find("${") else {
            break;
        };
        let abs_start = search_from + start;
        let Some(end) = result[abs_start..].find('}') else {
            break;
        };
        let abs_end = abs_start + end;
        let var_name = &result[abs_start + 2..abs_end];
        if let Ok(val) = std::env::var(var_name) {
            result.replace_range(abs_start..=abs_end, &val);
            search_from = abs_start + val.len();
        } else {
            search_from = abs_end + 1;
        }
    }
    result
}

/// Minimal .npmrc settings used by Jhol native registry paths.
#[derive(Default, Clone, Debug)]
pub struct NpmRcConfig {
    pub registry: Option<String>,
    pub auth_token: Option<String>,
    pub scoped_registries: HashMap<String, String>,
    pub auth_tokens_by_host_prefix: HashMap<String, String>,
    pub always_auth: Option<bool>,
    pub proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub strict_ssl: Option<bool>,
    pub cafile: Option<String>,
}

static NPMRC_CACHE: OnceLock<RwLock<HashMap<String, NpmRcConfig>>> = OnceLock::new();

fn npmrc_cache_key(dir: &Path) -> String {
    dir.to_string_lossy().to_string()
}

/// Load config from .jholrc in dir, then ~/.jholrc. Missing or invalid file = default.
pub fn load_config(dir: &Path) -> Config {
    load_config_cached(dir)
}

/// Load the config for the current working directory (cached). Used by
/// registry/network layers that need cheap config access without a dir.
pub fn global_config() -> Option<Config> {
    Some(load_config(Path::new(".")))
}

fn load_config_uncached(dir: &Path) -> Config {
    let mut cfg = Config::default();
    let home = dirs_home();
    let candidates = [
        dir.join(".jholrc"),
        home.map(|h| h.join(".jholrc"))
            .unwrap_or_else(|| dir.join(".none")),
    ];
    for path in &candidates {
        if path.is_file() {
            if let Ok(s) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    // ── Existing top-level fields ──────────────────────────
                    if let Some(b) = v.get("backend").and_then(|x| x.as_str()) {
                        cfg.backend = match b {
                            "bun" => Some(Backend::Bun),
                            "npm" => Some(Backend::Npm),
                            _ => None,
                        };
                    }
                    if let Some(c) = v.get("cacheDir").and_then(|x| x.as_str()) {
                        cfg.cache_dir = Some(c.to_string());
                    }
                    if let Some(o) = v.get("offline").and_then(|x| x.as_bool()) {
                        cfg.offline = Some(o);
                    }
                    if let Some(f) = v.get("frozen").and_then(|x| x.as_bool()) {
                        cfg.frozen = Some(f);
                    }

                    // ── Feature 1: install section ─────────────────────────
                    // Support top-level keys for backward compat AND nested install section.
                    {
                        let inst = v.get("install");
                        // no_scripts
                        let ns = inst
                            .and_then(|x| x.get("noScripts"))
                            .and_then(|x| x.as_bool())
                            .or_else(|| v.get("noScripts").and_then(|x| x.as_bool()));
                        if let Some(ns) = ns {
                            cfg.no_scripts = Some(ns);
                        }
                        // script_allowlist
                        let sal = inst
                            .and_then(|x| x.get("scriptAllowlist"))
                            .and_then(|x| x.as_array())
                            .or_else(|| v.get("scriptAllowlist").and_then(|x| x.as_array()));
                        if let Some(arr) = sal {
                            cfg.script_allowlist = Some(
                                arr.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect(),
                            );
                        }
                        // fallback_backend
                        let fb = inst
                            .and_then(|x| x.get("fallbackBackend"))
                            .and_then(|x| x.as_bool())
                            .or_else(|| v.get("fallbackBackend").and_then(|x| x.as_bool()));
                        if let Some(fb) = fb {
                            cfg.fallback_backend = Some(fb);
                        }
                        // concurrency
                        let con = inst
                            .and_then(|x| x.get("concurrency"))
                            .and_then(|x| x.as_u64())
                            .or_else(|| v.get("concurrency").and_then(|x| x.as_u64()));
                        if let Some(n) = con {
                            cfg.concurrency = Some(n as usize);
                        }
                        // fetch_timeout
                        let ft = inst
                            .and_then(|x| x.get("fetchTimeout"))
                            .and_then(|x| x.as_u64())
                            .or_else(|| v.get("fetchTimeout").and_then(|x| x.as_u64()));
                        if let Some(n) = ft {
                            cfg.fetch_timeout = Some(n);
                        }
                        // install_timeout
                        let it = inst
                            .and_then(|x| x.get("installTimeout"))
                            .and_then(|x| x.as_u64())
                            .or_else(|| v.get("installTimeout").and_then(|x| x.as_u64()));
                        if let Some(n) = it {
                            cfg.install_timeout = Some(n);
                        }
                        if let Some(engine) = inst
                            .and_then(|x| x.get("engineBackend"))
                            .and_then(|x| x.as_str())
                            .or_else(|| v.get("engineBackend").and_then(|x| x.as_str()))
                        {
                            cfg.engine_backend = Some(engine.to_string());
                        }
                    }

                    // ── Feature 2: registry section ────────────────────────
                    if let Some(reg) = v.get("registry").and_then(|x| x.as_object()) {
                        if let Some(d) = reg.get("default").and_then(|x| x.as_str()) {
                            cfg.registry_default = Some(d.trim_end_matches('/').to_string());
                        }
                        if let Some(scopes) = reg.get("scopes").and_then(|x| x.as_object()) {
                            let mut map = HashMap::new();
                            for (k, val) in scopes {
                                if let Some(url) = val.as_str() {
                                    map.insert(k.clone(), url.trim_end_matches('/').to_string());
                                }
                            }
                            cfg.registry_scopes = Some(map);
                        }
                        if let Some(tokens) = reg.get("authTokens").and_then(|x| x.as_object()) {
                            let mut map = HashMap::new();
                            for (host, tok_val) in tokens {
                                if let Some(raw) = tok_val.as_str() {
                                    map.insert(host.clone(), interpolate_env_vars(raw));
                                }
                            }
                            cfg.registry_auth_tokens = Some(map);
                        }
                    }

                    // ── Feature 3: audit section ───────────────────────────
                    if let Some(audit) = v.get("audit").and_then(|x| x.as_object()) {
                        if let Some(g) = audit.get("gate").and_then(|x| x.as_bool()) {
                            cfg.audit_gate = Some(g);
                        }
                        if let Some(sev) = audit.get("minSeverity").and_then(|x| x.as_str()) {
                            cfg.audit_min_severity = Some(sev.to_string());
                        }
                        if let Some(arr) = audit.get("ignoreAdvisories").and_then(|x| x.as_array())
                        {
                            cfg.audit_ignore_advisories = Some(
                                arr.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect(),
                            );
                        }
                    }

                    // ── Feature 4: cache section ───────────────────────────
                    if let Some(cache) = v.get("cache").and_then(|x| x.as_object()) {
                        if let Some(n) = cache.get("maxSizeGb").and_then(|x| x.as_f64()) {
                            cfg.cache_max_size_gb = Some(n);
                        }
                        if let Some(n) = cache.get("keepRecent").and_then(|x| x.as_u64()) {
                            cfg.cache_keep_recent = Some(n as usize);
                        }
                        if let Some(b) = cache.get("autoPrewarm").and_then(|x| x.as_bool()) {
                            cfg.cache_auto_prewarm = Some(b);
                        }
                    }

                    // ── Feature 5: workspaces section ─────────────────────
                    if let Some(ws) = v.get("workspaces").and_then(|x| x.as_object()) {
                        if let Some(b) = ws.get("parallel").and_then(|x| x.as_bool()) {
                            cfg.workspaces_parallel = Some(b);
                        }
                        if let Some(b) = ws.get("bail").and_then(|x| x.as_bool()) {
                            cfg.workspaces_bail = Some(b);
                        }
                    }

                    // ── Feature 6: hooks section ───────────────────────────
                    if let Some(hooks) = v.get("hooks").and_then(|x| x.as_object()) {
                        if let Some(s) = hooks.get("preInstall").and_then(|x| x.as_str()) {
                            cfg.hook_pre_install = Some(s.to_string());
                        }
                        if let Some(s) = hooks.get("postInstall").and_then(|x| x.as_str()) {
                            cfg.hook_post_install = Some(s.to_string());
                        }
                        if let Some(s) = hooks.get("preAudit").and_then(|x| x.as_str()) {
                            cfg.hook_pre_audit = Some(s.to_string());
                        }
                        if let Some(s) = hooks.get("postAudit").and_then(|x| x.as_str()) {
                            cfg.hook_post_audit = Some(s.to_string());
                        }
                    }

                    // ── Feature 7: trustedDependencies ────────────────────
                    if let Some(arr) = v.get("trustedDependencies").and_then(|x| x.as_array()) {
                        cfg.trusted_dependencies = Some(
                            arr.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect(),
                        );
                    }

                    // ── Feature 8: overrides and resolutions ───────────────
                    if let Some(ov) = v.get("overrides").and_then(|x| x.as_object()) {
                        let mut map = HashMap::new();
                        for (k, val) in ov {
                            if let Some(ver) = val.as_str() {
                                map.insert(k.clone(), ver.to_string());
                            }
                        }
                        cfg.overrides = Some(map);
                    }
                    if let Some(res) = v.get("resolutions").and_then(|x| x.as_object()) {
                        let mut map = HashMap::new();
                        for (k, val) in res {
                            if let Some(ver) = val.as_str() {
                                map.insert(k.clone(), ver.to_string());
                            }
                        }
                        cfg.resolutions = Some(map);
                    }

                    // ── Feature 9: env section (store raw for later deep-merge) ──
                    if let Some(env_section) = v.get("env").and_then(|x| x.as_object()) {
                        let mut map = HashMap::new();
                        for (env_name, env_val) in env_section {
                            map.insert(env_name.clone(), env_val.clone());
                        }
                        cfg.env_overrides = Some(map);
                    }

                    // ── Feature 10: output section ─────────────────────────
                    if let Some(out) = v.get("output").and_then(|x| x.as_object()) {
                        if let Some(b) = out.get("quiet").and_then(|x| x.as_bool()) {
                            cfg.output_quiet = Some(b);
                        }
                        if let Some(b) = out.get("noColor").and_then(|x| x.as_bool()) {
                            cfg.output_no_color = Some(b);
                        }
                        if let Some(b) = out.get("jsonLog").and_then(|x| x.as_bool()) {
                            cfg.output_json_log = Some(b);
                        }
                    }

                    // ── Feature 11: telemetry ───────────────────────────────
                    if let Some(b) = v.get("telemetry").and_then(|x| x.as_bool()) {
                        cfg.telemetry = Some(b);
                    }

                    // ── Feature 9: apply env-specific overrides ────────────
                    // After all base fields are loaded, check JHOL_ENV / NODE_ENV and
                    // deep-merge the matching env block.
                    let active_env = std::env::var("JHOL_ENV")
                        .ok()
                        .or_else(|| std::env::var("NODE_ENV").ok());
                    if let Some(env_name) = active_env {
                        if let Some(ref env_map) = cfg.env_overrides.clone() {
                            if let Some(overrides) = env_map.get(&env_name) {
                                apply_env_overrides(&mut cfg, overrides);
                            }
                        }
                    }
                }
            }
            break;
        }
    }
    cfg
}

static CONFIG_CACHE: OnceLock<RwLock<Option<(SystemTime, Config)>>> = OnceLock::new();

fn load_config_cached(dir: &Path) -> Config {
    let cache = CONFIG_CACHE.get_or_init(|| RwLock::new(None));
    // Check mtime of candidate files
    let home = dirs_home();
    let candidates = [
        dir.join(".jholrc"),
        home.map(|h| h.join(".jholrc"))
            .unwrap_or_else(|| dir.join(".none")),
    ];
    let latest_mtime = candidates
        .iter()
        .filter_map(|p| p.is_file().then(|| p.metadata().ok()?.modified().ok()))
        .flatten()
        .max();

    // Read lock — if cache is fresh, return cached copy
    if let Ok(guard) = cache.read() {
        if let Some(ref cached) = *guard {
            if cached.0 == latest_mtime.unwrap_or(SystemTime::UNIX_EPOCH) {
                return cached.1.clone();
            }
        }
    }

    // Cache miss — load fresh
    let cfg = load_config_uncached(dir);
    if let Ok(mut guard) = cache.write() {
        *guard = Some((latest_mtime.unwrap_or(SystemTime::now()), cfg.clone()));
    }
    cfg
}

/// Invalidate config cache (called when .jholrc changes via external tools).
pub fn invalidate_config_cache() {
    if let Ok(mut guard) = CONFIG_CACHE.get_or_init(|| RwLock::new(None)).write() {
        *guard = None;
    }
}

/// Deep-merge an env-specific block into the config.
/// Keys in the env block override the corresponding top-level config fields.
fn apply_env_overrides(cfg: &mut Config, overrides: &serde_json::Value) {
    let Some(obj) = overrides.as_object() else {
        return;
    };
    // Top-level scalars
    if let Some(b) = obj.get("frozen").and_then(|x| x.as_bool()) {
        cfg.frozen = Some(b);
    }
    if let Some(b) = obj.get("offline").and_then(|x| x.as_bool()) {
        cfg.offline = Some(b);
    }
    if let Some(s) = obj.get("cacheDir").and_then(|x| x.as_str()) {
        cfg.cache_dir = Some(s.to_string());
    }
    if let Some(b) = obj.get("backend").and_then(|x| x.as_str()) {
        cfg.backend = match b {
            "bun" => Some(Backend::Bun),
            "npm" => Some(Backend::Npm),
            _ => cfg.backend.clone(),
        };
    }
    // install sub-object
    if let Some(inst) = obj.get("install").and_then(|x| x.as_object()) {
        if let Some(b) = inst.get("noScripts").and_then(|x| x.as_bool()) {
            cfg.no_scripts = Some(b);
        }
        if let Some(arr) = inst.get("scriptAllowlist").and_then(|x| x.as_array()) {
            cfg.script_allowlist = Some(
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect(),
            );
        }
        if let Some(b) = inst.get("fallbackBackend").and_then(|x| x.as_bool()) {
            cfg.fallback_backend = Some(b);
        }
        if let Some(n) = inst.get("concurrency").and_then(|x| x.as_u64()) {
            cfg.concurrency = Some(n as usize);
        }
        if let Some(n) = inst.get("fetchTimeout").and_then(|x| x.as_u64()) {
            cfg.fetch_timeout = Some(n);
        }
        if let Some(n) = inst.get("installTimeout").and_then(|x| x.as_u64()) {
            cfg.install_timeout = Some(n);
        }
        if let Some(engine) = inst.get("engineBackend").and_then(|x| x.as_str()) {
            cfg.engine_backend = Some(engine.to_string());
        }
    }
    // audit sub-object
    if let Some(audit) = obj.get("audit").and_then(|x| x.as_object()) {
        if let Some(b) = audit.get("gate").and_then(|x| x.as_bool()) {
            cfg.audit_gate = Some(b);
        }
        if let Some(s) = audit.get("minSeverity").and_then(|x| x.as_str()) {
            cfg.audit_min_severity = Some(s.to_string());
        }
        if let Some(arr) = audit.get("ignoreAdvisories").and_then(|x| x.as_array()) {
            cfg.audit_ignore_advisories = Some(
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect(),
            );
        }
    }
    // output sub-object
    if let Some(out) = obj.get("output").and_then(|x| x.as_object()) {
        if let Some(b) = out.get("quiet").and_then(|x| x.as_bool()) {
            cfg.output_quiet = Some(b);
        }
        if let Some(b) = out.get("noColor").and_then(|x| x.as_bool()) {
            cfg.output_no_color = Some(b);
        }
        if let Some(b) = out.get("jsonLog").and_then(|x| x.as_bool()) {
            cfg.output_json_log = Some(b);
        }
    }
    // telemetry
    if let Some(b) = obj.get("telemetry").and_then(|x| x.as_bool()) {
        cfg.telemetry = Some(b);
    }
}

/// Returns true if telemetry should be sent, considering both config and environment.
/// Auto-disables in CI or offline environments regardless of config setting.
pub fn telemetry_enabled(config: &Config) -> bool {
    if config.telemetry == Some(false) {
        return false;
    }
    if std::env::var("CI").is_ok() {
        return false;
    }
    if std::env::var("JHOL_OFFLINE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")) {
        return false;
    }
    true
}

/// JAGR-specific configuration. Read from the `jagr` section of `.jholrc`
/// (project root or `~/.jholrc`). All fields are optional; missing values
/// fall back to the JAGR defaults.
#[derive(Default, Clone, Debug)]
pub struct JagrConfig {
    /// Whether to use the LP relaxation in Layer 1 (OL3P/OL3P-S).
    /// Defaults to `true`; can be overridden by `JHOL_JAGR_LP=0`.
    pub use_lp: Option<bool>,
    /// Per-layer timeout in milliseconds. Defaults to 30_000 (30 s).
    pub timeout_ms: Option<u64>,
    /// Whether to skip Layer 3 (tropical PTIME filter).
    pub skip_layer3: Option<bool>,
    /// Whether to skip Layer 2 (sheaf cohomology).
    pub skip_layer2: Option<bool>,
    /// Whether to run all three layers even on early termination.
    pub run_all_layers: Option<bool>,
    /// Whether to record the Betti-number history when running JAGR.
    /// Equivalent to setting `JHOL_BETTI_HISTORY=1` permanently for this project.
    pub betti_history: Option<bool>,
    /// Optional path to the Betti history JSONL file.
    pub betti_history_dir: Option<String>,
    /// Max candidate versions per package to feed into the resolver.
    /// Defaults to 512. Lower = faster, higher = more complete.
    pub max_versions: Option<usize>,
    /// When true, JAGR will fail loudly on any UNSAT rather than falling
    /// back to the old resolvers.
    pub strict: Option<bool>,
}

impl JagrConfig {
    /// Apply this config to a `JagrResolver`, returning the configured resolver.
    pub fn apply_to_resolver(&self) -> crate::jagr::JagrResolver {
        let mut r = crate::jagr::JagrResolver::new();
        if let Some(use_lp) = self.use_lp {
            r = r.with_lp(use_lp);
        }
        if let Some(timeout_ms) = self.timeout_ms {
            r = r.with_timeout(timeout_ms);
        }
        if self.skip_layer3.unwrap_or(false) {
            r = r.with_skip_layer3(true);
        }
        if self.skip_layer2.unwrap_or(false) {
            r = r.with_skip_layer2(true);
        }
        if self.run_all_layers.unwrap_or(false) {
            r = r.with_run_all_layers(true);
        }
        r
    }

    /// Return the merged config: this config overlaid with environment variables.
    /// Environment variables take precedence.
    pub fn merged_with_env(&self) -> Self {
        let mut c = self.clone();
        if let Ok(v) = std::env::var("JHOL_JAGR_LP") {
            c.use_lp = Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
        if let Ok(v) = std::env::var("JHOL_JAGR_TIMEOUT_MS") {
            if let Ok(n) = v.parse::<u64>() {
                c.timeout_ms = Some(n);
            }
        }
        if let Ok(v) = std::env::var("JHOL_JAGR_SKIP_LAYER3") {
            c.skip_layer3 = Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
        if let Ok(v) = std::env::var("JHOL_JAGR_SKIP_LAYER2") {
            c.skip_layer2 = Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
        if let Ok(v) = std::env::var("JHOL_JAGR_RUN_ALL_LAYERS") {
            c.run_all_layers = Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
        if let Ok(v) = std::env::var("JHOL_BETTI_HISTORY") {
            c.betti_history = Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
        if let Ok(v) = std::env::var("JHOL_BETTI_HISTORY_DIR") {
            c.betti_history_dir = Some(v);
        }
        if let Ok(v) = std::env::var("JHOL_JAGR_MAX_VERSIONS") {
            if let Ok(n) = v.parse::<usize>() {
                c.max_versions = Some(n);
            }
        }
        if let Ok(v) = std::env::var("JHOL_JAGR_STRICT") {
            c.strict = Some(v == "1" || v.eq_ignore_ascii_case("true"));
        }
        c
    }
}

/// Load JAGR config from the `jagr` section of `.jholrc` in `dir`, then
/// `~/.jholrc`. Missing file = default config. Invalid section = default
/// config (no error).
pub fn load_jagr_config(dir: &Path) -> JagrConfig {
    let mut cfg = JagrConfig::default();
    let home = dirs_home();
    let candidates = [
        dir.join(".jholrc"),
        home.map(|h| h.join(".jholrc"))
            .unwrap_or_else(|| dir.join(".none")),
    ];
    for path in &candidates {
        if path.is_file() {
            if let Ok(s) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(j) = v.get("jagr").and_then(|x| x.as_object()) {
                        if let Some(b) = j.get("useLp").and_then(|x| x.as_bool()) {
                            cfg.use_lp = Some(b);
                        }
                        if let Some(n) = j.get("timeoutMs").and_then(|x| x.as_u64()) {
                            cfg.timeout_ms = Some(n);
                        }
                        if let Some(b) = j.get("skipLayer3").and_then(|x| x.as_bool()) {
                            cfg.skip_layer3 = Some(b);
                        }
                        if let Some(b) = j.get("skipLayer2").and_then(|x| x.as_bool()) {
                            cfg.skip_layer2 = Some(b);
                        }
                        if let Some(b) = j.get("runAllLayers").and_then(|x| x.as_bool()) {
                            cfg.run_all_layers = Some(b);
                        }
                        if let Some(b) = j.get("bettiHistory").and_then(|x| x.as_bool()) {
                            cfg.betti_history = Some(b);
                        }
                        if let Some(s) = j.get("bettiHistoryDir").and_then(|x| x.as_str()) {
                            cfg.betti_history_dir = Some(s.to_string());
                        }
                        if let Some(n) = j.get("maxVersions").and_then(|x| x.as_u64()) {
                            cfg.max_versions = Some(n as usize);
                        }
                        if let Some(b) = j.get("strict").and_then(|x| x.as_bool()) {
                            cfg.strict = Some(b);
                        }
                    }
                }
            }
            break;
        }
    }
    cfg.merged_with_env()
}

fn dirs_home() -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    {
        std::env::var("HOME").ok().map(std::path::PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE")
            .ok()
            .map(std::path::PathBuf::from)
    }
}

fn read_npmrc(path: &Path) -> NpmRcConfig {
    let mut out = NpmRcConfig::default();
    let Ok(s) = std::fs::read_to_string(path) else {
        return out;
    };
    for raw in s.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let mut value = v.trim().to_string();
        if value.starts_with("${") && value.ends_with('}') && value.len() > 3 {
            let env_key = &value[2..value.len() - 1];
            if let Ok(env_val) = std::env::var(env_key) {
                value = env_val;
            }
        }
        if key == "registry" {
            out.registry = Some(value.trim_end_matches('/').to_string());
            continue;
        }
        if let Some(scope) = key.strip_suffix(":registry") {
            if scope.starts_with('@') && !scope.trim().is_empty() {
                out.scoped_registries
                    .insert(scope.to_string(), value.trim_end_matches('/').to_string());
            }
            continue;
        }
        if key.ends_with(":_authToken") {
            out.auth_token = Some(value.clone());
            out.auth_tokens_by_host_prefix.insert(
                key.trim_end_matches(":_authToken")
                    .trim_start_matches("//")
                    .trim_end_matches('/')
                    .to_string(),
                value,
            );
            continue;
        }
        if key == "always-auth" {
            out.always_auth = match value.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            continue;
        }
        if key == "proxy" {
            out.proxy = Some(value);
            continue;
        }
        if key == "https-proxy" || key == "https_proxy" {
            out.https_proxy = Some(value);
            continue;
        }
        if key == "noproxy" || key == "no-proxy" || key == "no_proxy" {
            out.no_proxy = Some(value);
            continue;
        }
        if key == "strict-ssl" {
            out.strict_ssl = match value.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            continue;
        }
        if key == "cafile" {
            out.cafile = Some(value);
            continue;
        }
    }
    out
}

/// Load .npmrc from project then home, with project taking precedence.
pub fn load_npmrc(dir: &Path) -> NpmRcConfig {
    let cache_key = npmrc_cache_key(dir);
    if let Some(cache) = NPMRC_CACHE.get() {
        if let Ok(guard) = cache.read() {
            if let Some(cached) = guard.get(&cache_key) {
                return cached.clone();
            }
        }
    }

    let home = dirs_home();
    let project = read_npmrc(&dir.join(".npmrc"));
    let home_cfg = home
        .as_ref()
        .map(|h| read_npmrc(&h.join(".npmrc")))
        .unwrap_or_default();
    let mut scoped_registries = home_cfg.scoped_registries;
    scoped_registries.extend(project.scoped_registries);

    let mut auth_tokens_by_host_prefix = home_cfg.auth_tokens_by_host_prefix;
    auth_tokens_by_host_prefix.extend(project.auth_tokens_by_host_prefix);

    let merged = NpmRcConfig {
        registry: project.registry.or(home_cfg.registry),
        auth_token: project.auth_token.or(home_cfg.auth_token),
        scoped_registries,
        auth_tokens_by_host_prefix,
        always_auth: project.always_auth.or(home_cfg.always_auth),
        proxy: project.proxy.or(home_cfg.proxy),
        https_proxy: project.https_proxy.or(home_cfg.https_proxy),
        no_proxy: project.no_proxy.or(home_cfg.no_proxy),
        strict_ssl: project.strict_ssl.or(home_cfg.strict_ssl),
        cafile: project.cafile.or(home_cfg.cafile),
    };

    let cache = NPMRC_CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Ok(mut guard) = cache.write() {
        guard.insert(cache_key, merged.clone());
    }

    merged
}

/// Effective registry URL used by native metadata/tarball flows.
pub fn effective_registry_url(dir: &Path) -> String {
    if let Ok(v) = std::env::var("NPM_CONFIG_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }
    if let Ok(v) = std::env::var("JHOL_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }
    // Feature 2: check .jholrc registry.default before .npmrc
    if let Some(jholrc_reg) = load_jholrc_registry(dir) {
        if let Some(d) = jholrc_reg.default {
            return d;
        }
    }
    load_npmrc(dir)
        .registry
        .unwrap_or_else(|| "https://registry.npmjs.org".to_string())
}

/// Resolve registry URL for a package, supporting scoped registry entries in .npmrc.
pub fn effective_registry_url_for_package(dir: &Path, package_name: &str) -> String {
    // Feature 2: check .jholrc scopes first (after env vars but before .npmrc)
    if package_name.starts_with('@') {
        if let Some((scope, _)) = package_name.split_once('/') {
            // Check .npmrc scoped registries first (they exist in the existing cfg load)
            let cfg = load_npmrc(dir);
            if let Some(url) = cfg.scoped_registries.get(scope) {
                return url.trim_end_matches('/').to_string();
            }
        }
    }

    if let Ok(v) = std::env::var("NPM_CONFIG_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }
    if let Ok(v) = std::env::var("JHOL_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }

    // Feature 2: check .jholrc scopes, then .jholrc default
    if let Some(jholrc_reg) = load_jholrc_registry(dir) {
        if package_name.starts_with('@') {
            if let Some((scope, _)) = package_name.split_once('/') {
                if let Some(url) = jholrc_reg.scopes.get(scope) {
                    return url.clone();
                }
            }
        }
        if let Some(d) = jholrc_reg.default {
            return d;
        }
    }

    load_npmrc(dir)
        .registry
        .unwrap_or_else(|| "https://registry.npmjs.org".to_string())
}

/// Best-effort auth token for registry API/tarball requests.
pub fn registry_auth_token(dir: &Path) -> Option<String> {
    if let Ok(v) = std::env::var("NODE_AUTH_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::env::var("NPM_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    load_npmrc(dir).auth_token
}

/// Resolve best auth token for a given registry/tarball URL using .npmrc host-specific tokens.
pub fn registry_auth_token_for_url(dir: &Path, url: &str) -> Option<String> {
    if let Ok(v) = std::env::var("NODE_AUTH_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::env::var("NPM_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }

    // Feature 2: check .jholrc auth tokens (takes precedence over .npmrc tokens)
    let normalized = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');

    if let Some(jholrc_reg) = load_jholrc_registry(dir) {
        let mut best: Option<(usize, String)> = None;
        for (host, token) in &jholrc_reg.auth_tokens {
            let h = host.trim().trim_end_matches('/');
            if !h.is_empty() && normalized.starts_with(h) {
                let score = h.len();
                match &best {
                    Some((s, _)) if *s >= score => {}
                    _ => best = Some((score, token.clone())),
                }
            }
        }
        if let Some((_, token)) = best {
            return Some(token);
        }
    }

    let cfg = load_npmrc(dir);

    let mut best: Option<(usize, String)> = None;
    for (prefix, token) in &cfg.auth_tokens_by_host_prefix {
        let p = prefix.trim().trim_end_matches('/');
        if p.is_empty() {
            continue;
        }
        if normalized.starts_with(p) {
            let score = p.len();
            match &best {
                Some((best_score, _)) if *best_score >= score => {}
                _ => best = Some((score, token.clone())),
            }
        }
    }
    best.map(|(_, t)| t).or(cfg.auth_token)
}

/// Apply enterprise network-related npmrc settings to process env (best-effort).
pub fn apply_enterprise_network_env(dir: &Path) {
    let cfg = load_npmrc(dir);
    if let Some(v) = cfg.https_proxy {
        if std::env::var("HTTPS_PROXY").is_err() && std::env::var("https_proxy").is_err() {
            std::env::set_var("HTTPS_PROXY", v.clone());
            std::env::set_var("https_proxy", v);
        }
    }
    if let Some(v) = cfg.proxy {
        if std::env::var("HTTP_PROXY").is_err() && std::env::var("http_proxy").is_err() {
            std::env::set_var("HTTP_PROXY", v.clone());
            std::env::set_var("http_proxy", v);
        }
    }
    if let Some(v) = cfg.no_proxy {
        if std::env::var("NO_PROXY").is_err() && std::env::var("no_proxy").is_err() {
            std::env::set_var("NO_PROXY", v.clone());
            std::env::set_var("no_proxy", v);
        }
    }
    if matches!(cfg.strict_ssl, Some(false)) {
        // SECURITY: We do NOT disable TLS verification even if strict_ssl=false is set.
        // Disabling TLS verification would expose auth tokens to MITM attacks.
        // Use a proper CA bundle (cafile) instead.
        eprintln!("SECURITY WARNING: strict_ssl=false in .npmrc is ignored; TLS verification remains enabled. Configure cafile for custom CAs.");
    }
    if let Some(v) = cfg.cafile {
        if std::env::var("SSL_CERT_FILE").is_err() {
            std::env::set_var("SSL_CERT_FILE", v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_npmrc_scoped_registry_and_token_resolution() {
        let tmp = std::env::temp_dir().join(format!(
            "jhol-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("SystemTime before UNIX epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).expect("Failed to create temp dir");
        std::fs::write(
            tmp.join(".npmrc"),
            "registry=https://registry.npmjs.org/\n@myco:registry=https://npm.myco.local/\n//npm.myco.local/:_authToken=abc123\n",
        )
        .expect("Failed to write test .npmrc");

        let cfg = load_npmrc(&tmp);
        assert_eq!(cfg.registry.as_deref(), Some("https://registry.npmjs.org"));
        assert_eq!(
            cfg.scoped_registries.get("@myco").map(String::as_str),
            Some("https://npm.myco.local")
        );

        let scoped = effective_registry_url_for_package(&tmp, "@myco/foo");
        assert_eq!(scoped, "https://npm.myco.local");

        assert_eq!(
            cfg.auth_tokens_by_host_prefix
                .get("npm.myco.local")
                .map(String::as_str),
            Some("abc123")
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
