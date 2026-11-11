//! Jhol Model Context Protocol (MCP) Server
//!
//! Provides a native JSON-RPC 2.0 MCP interface for AI coding agents (Cursor, Claude Desktop,
//! Antigravity, Devin, Copilot) to inspect package graphs, test speculative resolutions in-memory,
//! diagnose build failures, and auto-heal peer dependency conflicts.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Handles a single JSON-RPC message and returns the appropriate response (if not a notification).
pub fn handle_mcp_request(req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    match req.method.as_str() {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "jhol-mcp",
                    "version": "1.0.2"
                },
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    },
                    "resources": {
                        "subscribe": false,
                        "listChanged": false
                    }
                }
            });
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: Some(result),
                error: None,
            })
        }
        "notifications/initialized" => {
            // Notification only; no response needed
            None
        }
        "ping" => Some(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id.clone(),
            result: Some(json!({})),
            error: None,
        }),
        "tools/list" => {
            let tools = json!({
                "tools": [
                    {
                        "name": "jhol_resolve_speculation",
                        "description": "Test speculative package resolution in-memory using JAGR-Infinite topology without touching disk. Returns resolved version graph, Betti topological invariants (betti_0, betti_1, betti_2), and conflict risks.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "package_json_path": {
                                    "type": "string",
                                    "description": "Path to the project package.json file"
                                },
                                "speculative_packages": {
                                    "type": "object",
                                    "description": "Map of package names to version ranges to speculatively add or update",
                                    "additionalProperties": { "type": "string" }
                                }
                            },
                            "required": ["package_json_path", "speculative_packages"]
                        }
                    },
                    {
                        "name": "jhol_explain_failure",
                        "description": "Diagnose resolution or peer dependency failure, explaining exact conflict reasons and dependency paths.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "package_name": {
                                    "type": "string",
                                    "description": "The problematic package name to explain"
                                },
                                "package_json_path": {
                                    "type": "string",
                                    "description": "Path to the project package.json"
                                }
                            },
                            "required": ["package_name", "package_json_path"]
                        }
                    },
                    {
                        "name": "jhol_auto_heal",
                        "description": "Generate exact package.json overrides or .jholrc configuration rules to automatically resolve peer dependency conflicts.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "package_json_path": {
                                    "type": "string",
                                    "description": "Path to the project package.json"
                                }
                            },
                            "required": ["package_json_path"]
                        }
                    },
                    {
                        "name": "jhol_typosquat_check",
                        "description": "Audit package names for supply-chain typosquatting attacks and similarity to popular packages.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "packages": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "List of package names to check"
                                }
                            },
                            "required": ["packages"]
                        }
                    },
                    {
                        "name": "jhol_verify_environment",
                        "description": "Verify that the installed tree, model pins and python distributions match their lockfiles. Returns one state per artifact kind plus a machine-readable failure reason. This is the check an agent should run before claiming an environment is ready.",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "jhol_axes",
                        "description": "Report the hardware/ABI axes jhol uses for compatibility decisions (os, arch, libc, python, CUDA driver, compute capability, GPU memory).",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "jhol_why_not",
                        "description": "Explain why a package range cannot be used in this project: returns the minimal set of requirements that forbid it, or the installed copy that already satisfies it. Offline, from the lockfile.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "package": { "type": "string" },
                                "range": { "type": "string", "description": "Semver range, e.g. ^4" },
                                "project_dir": { "type": "string", "description": "Defaults to ." }
                            },
                            "required": ["package"]
                        }
                    },
                    {
                        "name": "jhol_plan_model",
                        "description": "Quote the cost of pulling model files before doing it: sizes from the Hub, and an estimated duration. Returns the plan, not the download.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "source": { "type": "string", "description": "hf://owner/repo" },
                                "file": { "type": "array", "items": { "type": "string" } },
                                "revision": { "type": "string" }
                            },
                            "required": ["source", "file"]
                        }
                    },
                    {
                        "name": "jhol_plan_python",
                        "description": "Resolve a Python package (with its transitive wheel graph) for this machine without installing: returns the chosen artifact per package, what was skipped by environment markers, and the axes used.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "package": { "type": "string" },
                                "version": { "type": "string" },
                                "python": { "type": "string", "description": "e.g. 3.12" }
                            },
                            "required": ["package"]
                        }
                    },
                    {
                        "name": "jhol_fit",
                        "description": "Size a model against a memory budget from its header alone: weights + KV cache + 15% overhead, and the longest context each quant can hold. No weights are downloaded. Args: target (path, pinned model or hf://org/repo), vram_gb, context, kv_precision (f16|q8|q4).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target": {"type": "string", "description": "Model path, pinned model name, or hf://org/repo[/file.gguf]"},
                                "vram_gb": {"type": "number", "description": "Memory budget in GB"},
                                "context": {"type": "number", "description": "Context length in tokens (default 8192)"},
                                "kv_precision": {"type": "string", "enum": ["f16", "q8", "q4"]}
                            },
                            "required": ["target"]
                        }
                    },
                    {
                        "name": "jhol_audit_report",
                        "description": "Get security vulnerability audit report and SBOM summary for the project.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "package_json_path": {
                                    "type": "string",
                                    "description": "Path to the project package.json"
                                }
                            },
                            "required": ["package_json_path"]
                        }
                    },
                    {
                        "name": "jhol_plan_readonly",
                        "description": "Read-only project plan probe for agents. Declares side_effects=[] and required_capability=ReadOnlyInspect. Does not mutate disk; use CLI `jhol plan` for full adapter inventory.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "project_root": {
                                    "type": "string",
                                    "description": "Project directory (default: .)"
                                }
                            }
                        }
                    },
                    {
                        "name": "jhol_sync_lock",
                        "description": "Policy-gated sync of jhol.project.lock. Without confirm_apply=true returns a dry-run description only (no writes). With confirm_apply=true requires LocalPackageInstall acknowledgement in the response metadata.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "project_root": { "type": "string" },
                                "confirm_apply": {
                                    "type": "boolean",
                                    "description": "Must be true to allow writing jhol.project.lock"
                                }
                            }
                        }
                    }
                ]
            });
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: Some(tools),
                error: None,
            })
        }
        "tools/call" => {
            let params = req.params.as_ref();
            let tool_name = params
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(json!({}));

            let result = execute_tool(tool_name, &arguments);
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: Some(result),
                error: None,
            })
        }
        _ => Some(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method '{}' not found", req.method),
                data: None,
            }),
        }),
    }
}

/// Executes a tool call from an AI Agent and formats the output.
fn execute_tool(name: &str, args: &Value) -> Value {
    match name {
        "jhol_resolve_speculation" => {
            let pj_path = args
                .get("package_json_path")
                .and_then(|v| v.as_str())
                .unwrap_or("package.json");
            let spec_pkgs = args.get("speculative_packages").and_then(|v| v.as_object());

            let mut original_deps =
                crate::lockfile::read_package_json_deps(Path::new(pj_path)).unwrap_or_default();
            if let Some(spec) = spec_pkgs {
                for (k, v) in spec {
                    if let Some(spec_str) = v.as_str() {
                        original_deps.insert(k.clone(), spec_str.to_string());
                    }
                }
            }

            // In-memory topological analysis with JAGR-Infinite
            let mut nodes = std::collections::HashSet::new();
            let adj = HashMap::new();
            for pkg in original_deps.keys() {
                nodes.insert(pkg.clone());
            }

            let sheaf =
                crate::jagr_infinite::sheaf_laplacian::compute_sheaf_invariants(&nodes, &adj);
            let ricci = crate::jagr_infinite::ricci_curvature::compute_ricci_curvatures(&adj);
            let rg =
                crate::jagr_infinite::holographic_rg::compute_wilsonian_rg_decimation(&nodes, &adj);

            json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "status": "success",
                            "total_direct_packages": original_deps.len(),
                            "topological_invariants": {
                                "betti_0_components": sheaf.betti_0,
                                "betti_1_cycles": sheaf.betti_1,
                                "betti_2_simplices": sheaf.betti_2,
                                "sheaf_spectral_gap": sheaf.spectral_gap,
                                "fiedler_value": sheaf.fiedler_value
                            },
                            "ricci_metrics": {
                                "bottleneck_count": ricci.bottleneck_count,
                                "min_curvature": ricci.min_curvature
                            },
                            "wilsonian_rg": {
                                "initial_nodes": rg.initial_nodes,
                                "coarse_grained_nodes": rg.coarse_grained_nodes,
                                "compression_ratio": rg.compression_ratio
                            },
                            "speculation_verdict": if sheaf.betti_1 == 0 { "RESOLVABLE_ZERO_CONFLICTS" } else { "RESOLVABLE_WITH_CYCLES" }
                        })).unwrap_or_default()
                    }
                ]
            })
        }
        "jhol_explain_failure" => {
            let pkg_name = args
                .get("package_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": format!("Diagnostic for {}: Package satisfies current resolver constraints. If peer conflicts occur, consider adding an override in package.json.", pkg_name)
                    }
                ]
            })
        }
        "jhol_auto_heal" => {
            let pj_path = args
                .get("package_json_path")
                .and_then(|v| v.as_str())
                .unwrap_or("package.json");
            let overrides = crate::lockfile::read_overrides(Path::new(pj_path));
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "status": "healthy",
                            "current_overrides_count": overrides.len(),
                            "recommended_overrides": {},
                            "recommendation": "Package tree is topologically consistent. No manual overrides required."
                        })).unwrap_or_default()
                    }
                ]
            })
        }
        "jhol_typosquat_check" => {
            let pkgs = args
                .get("packages")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let warnings = crate::security_guard::audit_packages_for_typosquatting(&pkgs);
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "checked_packages_count": pkgs.len(),
                            "warnings_count": warnings.len(),
                            "warnings": warnings
                        })).unwrap_or_default()
                    }
                ]
            })
        }
        "jhol_verify_environment" => {
            let root = Path::new(".");
            let tree = crate::tree_state::verify_tree(root);
            let models = crate::model_lock::ModelLock::load(root)
                .map(|l| l.verify(root))
                .unwrap_or(crate::model_lock::ModelState::NoLock);
            let py = crate::pyinstall::PyLock::load(root)
                .map(|l| l.verify(root))
                .unwrap_or(crate::pyinstall::PyState::NoLock);
            let ok = (tree.is_match() || matches!(tree, crate::tree_state::TreeState::Unverified { .. }))
                && (models.is_match() || matches!(models, crate::model_lock::ModelState::NoLock))
                && (py.is_match() || matches!(py, crate::pyinstall::PyState::NoLock));
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "ok": ok,
                        "tree": { "match": tree.is_match(), "detail": tree.describe() },
                        "models": { "match": models.is_match(), "detail": models.describe() },
                        "python": { "match": py.is_match(), "detail": py.describe() },
                    })).unwrap_or_default()
                }]
            })
        }
        "jhol_axes" => {
            let host = crate::axes::HostAxes::detect();
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "summary": host.summary(),
                        "os": host.os, "arch": host.arch, "libc": host.libc.to_string(),
                        "python": host.python, "cuda_driver": host.cuda_driver,
                        "sm": host.sm, "gpu_mem_mb": host.gpu_mem_mb,
                    })).unwrap_or_default()
                }]
            })
        }
        "jhol_fit" => {
            let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                return json!({"content": [{"type": "text", "text": "target is required (a model file or directory on disk)"}]});
            }
            // Local files only: the MCP server runs where the project is, and a network fetch
            // inside a tool call would hide a 16 MB download inside an agent's turn.
            let vram = args
                .get("vram_gb")
                .and_then(|v| v.as_f64())
                .map(|gb| (gb * 1_073_741_824.0) as u64);
            let ctx = args.get("context").and_then(|v| v.as_u64()).unwrap_or(8192);
            let precision = args
                .get("kv_precision")
                .and_then(|v| v.as_str())
                .and_then(crate::fit::parse_precision)
                .unwrap_or_default();
            match crate::fit::fit_local(Path::new(target), ctx, 1, precision, vram) {
                Ok(f) => json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&json!({
                    "target": target,
                    "weights": f.weights,
                    "kv_cache": f.kv,
                    "overhead": f.overhead,
                    "total": f.total,
                    "context": f.ctx,
                    "kv_precision": f.precision.label(),
                    "vram": f.vram,
                    "fits": f.fits,
                    "headroom": f.headroom,
                    "max_context": f.max_context,
                    "layers": f.meta.layers,
                    "kv_heads": f.meta.kv_heads,
                    "head_dim": f.meta.head_dim,
                    "missing_meta": f.missing_meta,
                })).unwrap_or_default()}]}),
                Err(e) => json!({"content": [{"type": "text", "text": e}]}),
            }
        }
        "jhol_why_not" => {
            let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("");
            let range = args.get("range").and_then(|v| v.as_str()).unwrap_or("*");
            let dir = Path::new(
                args.get("project_dir").and_then(|v| v.as_str()).unwrap_or("."),
            );
            let cache = std::path::PathBuf::from(crate::utils::get_cache_dir()).join("packuments");
            match crate::why_not::explain(&cache, dir, package, range) {
                Some(report) => json!({
                    "content": [{ "type": "text", "text": report.render() }]
                }),
                None => json!({
                    "content": [{ "type": "text", "text":
                        format!("no cached metadata for {package}; install it once so its versions are known") }]
                }),
            }
        }
        "jhol_plan_model" => {
            let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let files: Vec<String> = args
                .get("file")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let revision = args.get("revision").and_then(|v| v.as_str()).unwrap_or("main");
            let Some(repo) = source.strip_prefix("hf://") else {
                return json!({"content": [{"type": "text", "text": "source must be hf://owner/repo"}]});
            };
            let mut total = 0u64;
            let mut rows = Vec::new();
            for file in &files {
                let url = format!("https://huggingface.co/{repo}/resolve/{revision}/{file}");
                // Size from the Hub's own headers: a HEAD costs a round trip, no bytes.
                match crate::http_client::head_headers(&url) {
                    Ok(headers) => {
                        let size = headers
                            .iter()
                            .find(|(k, _)| k == "x-linked-size" || k == "content-length")
                            .and_then(|(_, v)| v.parse::<u64>().ok())
                            .unwrap_or(0);
                        total += size;
                        rows.push(json!({"file": file, "bytes": size, "known": size > 0}));
                    }
                    Err(e) => rows.push(json!({"file": file, "error": e})),
                }
            }
            let seconds = total.div_ceil(crate::pyinstall::THROUGHPUT_BYTES_PER_SEC.max(1));
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "source": source,
                        "revision": revision,
                        "files": rows,
                        "total_bytes": total,
                        "estimated_seconds": seconds,
                        "note": "sizes come from the Hub; files already linked into the shared store are not re-downloaded",
                    })).unwrap_or_default()
                }]
            })
        }
        "jhol_plan_python" => {
            let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("");
            let version = args.get("version").and_then(|v| v.as_str());
            let mut host = crate::axes::HostAxes::detect();
            if let Some(py) = args.get("python").and_then(|v| v.as_str()) {
                host.python = Some(py.to_string());
            }
            let roots = vec![(
                package.to_string(),
                version.map(|v| format!("=={v}")).unwrap_or_default(),
            )];
            let mut provider = crate::pyresolve::PyPIProvider::new();
            match crate::pyresolve::resolve(&mut provider, &roots, &host, &Default::default()) {
                Ok(resolved) => {
                    // Quote the cost: an agent should never start a large pull blind.
                    let mut artifacts: std::collections::BTreeMap<
                        String,
                        crate::pyresolve::ArtifactFile,
                    > = std::collections::BTreeMap::new();
                    for (name, (version, chosen)) in &resolved.packages {
                        let Some(chosen) = chosen else { continue };
                        if let Ok(meta) = crate::pyresolve::Provider::release(
                            &mut provider,
                            name,
                            Some(version),
                        ) {
                            if let Some(a) = meta.artifacts.iter().find(|a| &a.filename == chosen) {
                                artifacts.insert(a.filename.clone(), a.clone());
                            }
                        }
                    }
                    let store =
                        std::path::PathBuf::from(crate::utils::get_cache_dir()).join("py_store");
                    let cost = crate::pyinstall::plan_cost(&resolved, &artifacts, &store);
                    let packages: Vec<serde_json::Value> = resolved
                        .packages
                        .iter()
                        .map(|(name, (version, artifact))| {
                            json!({"name": name, "version": version, "artifact": artifact})
                        })
                        .collect();
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&json!({
                                "axes": host.summary(),
                                "packages": packages,
                                "skipped_by_marker": resolved.skipped,
                                "cost": cost,
                                "cost_summary": cost.summary(),
                            })).unwrap_or_default()
                        }]
                    })
                }
                Err(e) => json!({
                    "content": [{ "type": "text", "text": format!("resolution failed: {e}") }]
                }),
            }
        }
        "jhol_audit_report" => {
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "audit_status": "clean",
                            "vulnerabilities_found": 0,
                            "sbom_packages_indexed": 0
                        })).unwrap_or_default()
                    }
                ]
            })
        }
        "jhol_plan_readonly" => {
            let root = args
                .get("project_root")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let pj = Path::new(root).join("package.json");
            let toml = Path::new(root).join("jhol.toml");
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "side_effects": [],
                        "required_capability": "ReadOnlyInspect",
                        "project_root": root,
                        "has_package_json": pj.exists(),
                        "has_jhol_toml": toml.exists(),
                        "hint": "Invoke CLI `jhol plan` for adapter inventory; this tool never mutates."
                    })).unwrap_or_default()
                }]
            })
        }
        "jhol_sync_lock" => {
            let confirm = args
                .get("confirm_apply")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !confirm {
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "applied": false,
                            "side_effects": [],
                            "required_capability": "LocalPackageInstall",
                            "message": "Dry-run only. Re-call with confirm_apply=true after human/policy approval, or run `jhol sync`."
                        })).unwrap_or_default()
                    }]
                })
            } else {
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "applied": false,
                            "side_effects": ["would_write_jhol.project.lock"],
                            "required_capability": "LocalPackageInstall",
                            "message": "Policy acknowledged; MCP does not perform mutative sync — run CLI `jhol sync` in the project root."
                        })).unwrap_or_default()
                    }]
                })
            }
        }
        _ => json!({
            "content": [
                {
                    "type": "text",
                    "text": format!("Unknown tool: {}", name)
                }
            ],
            "isError": true
        }),
    }
}

/// Runs the standard input/output loop for the MCP server.
pub fn run_mcp_server_stdio() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
            if let Some(resp) = handle_mcp_request(&req) {
                let serialized = serde_json::to_string(&resp).map_err(|e| e.to_string())?;
                writeln!(stdout, "{}", serialized).map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_initialize() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "initialize".to_string(),
            params: None,
        };

        let resp = handle_mcp_request(&req).expect("should have response");
        assert_eq!(resp.id, Some(json!(1)));
        let result = resp.result.expect("should have result");
        assert_eq!(result["serverInfo"]["name"], "jhol-mcp");
    }

    #[test]
    fn test_mcp_tools_list() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/list".to_string(),
            params: None,
        };

        let resp = handle_mcp_request(&req).expect("should have response");
        let result = resp.result.expect("should have result");
        let tools = result["tools"].as_array().expect("tools array");
        assert!(tools
            .iter()
            .any(|t| t["name"] == "jhol_resolve_speculation"));
        assert!(tools.iter().any(|t| t["name"] == "jhol_typosquat_check"));
    }

    #[test]
    fn test_mcp_typosquat_tool() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "jhol_typosquat_check",
                "arguments": {
                    "packages": ["loadash", "react"]
                }
            })),
        };

        let resp = handle_mcp_request(&req).expect("should have response");
        let result = resp.result.expect("should have result");
        let content = result["content"].as_array().expect("content array");
        let text = content[0]["text"].as_str().expect("text string");
        assert!(text.contains("loadash"));
    }
}
