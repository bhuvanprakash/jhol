//! Ignored benchmark/stress tests for JAGR-3, GRM, and installer comparisons.
//!
//! These tests intentionally shell out to package managers and may use the network.
//! Run with:
//!   cargo test --test stress -- --ignored --nocapture

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[derive(Debug, Clone)]
struct BenchResult {
    scenario: String,
    tool: String,
    phase: String,
    status: String,
    elapsed_ms: u128,
    peak_rss_kb: Option<u64>,
    stdout_tail: String,
    stderr_tail: String,
}

fn jhol_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_jhol"))
}

fn command_exists(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tail_text(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes).replace('\n', "\\n");
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        text
    } else {
        chars[chars.len() - max_chars..].iter().collect()
    }
}

fn fixture_package_json(name: &str, deps: &[(&str, &str)]) -> String {
    let dep_lines = deps
        .iter()
        .map(|(name, version)| format!(r#"    "{name}": "{version}""#))
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        r#"{{
  "name": "{name}",
  "version": "1.0.0",
  "private": true,
  "dependencies": {{
{dep_lines}
  }}
}}
"#
    )
}

fn create_project(path: &Path, package_json: &str) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(path.join("package.json"), package_json).unwrap();
}

fn remove_install_outputs(project: &Path) {
    let _ = std::fs::remove_dir_all(project.join("node_modules"));
    let _ = std::fs::remove_file(project.join("package-lock.json"));
    let _ = std::fs::remove_file(project.join("bun.lockb"));
    let _ = std::fs::remove_file(project.join("bun.lock"));
    let _ = std::fs::remove_file(project.join("pnpm-lock.yaml"));
    let _ = std::fs::remove_file(project.join("jhol-lock.json"));
}

fn run_command(mut command: Command, scenario: &str, tool: &str, phase: &str) -> BenchResult {
    let started = Instant::now();
    let output = command.output().expect("benchmark command should start");
    BenchResult {
        scenario: scenario.to_string(),
        tool: tool.to_string(),
        phase: phase.to_string(),
        status: if output.status.success() {
            "ok"
        } else {
            "failed"
        }
        .to_string(),
        elapsed_ms: started.elapsed().as_millis(),
        peak_rss_kb: None,
        stdout_tail: tail_text(&output.stdout, 240),
        stderr_tail: tail_text(&output.stderr, 240),
    }
}

fn jhol_install_command(project: &Path, cache: &Path, resolver: &str) -> Command {
    let mut c = Command::new(jhol_bin());
    c.current_dir(project)
        .env("JHOL_CACHE_DIR", cache)
        .env("JHOL_RESOLVER", resolver)
        .env("JHOL_QUIET", "1")
        .args(["install", "--resolver", resolver, "--no-scripts", "--quiet"]);
    c
}

fn npm_install_command(project: &Path) -> Command {
    let mut c = Command::new("npm");
    c.current_dir(project)
        .args(["install", "--ignore-scripts", "--no-audit", "--no-fund"]);
    c
}

fn pnpm_install_command(project: &Path) -> Command {
    let mut c = Command::new("pnpm");
    c.current_dir(project).args(["install", "--ignore-scripts"]);
    c
}

fn bun_install_command(project: &Path) -> Command {
    let mut c = Command::new("bun");
    c.current_dir(project).args(["install", "--ignore-scripts"]);
    c
}

fn print_markdown_results(results: &[BenchResult]) {
    eprintln!("| scenario | tool | phase | status | elapsed_ms | peak_rss_kb |");
    eprintln!("|---|---|---|---:|---:|---:|");
    for r in results {
        eprintln!(
            "| {} | {} | {} | {} | {} | {} |",
            r.scenario,
            r.tool,
            r.phase,
            r.status,
            r.elapsed_ms,
            r.peak_rss_kb
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        );
        if r.status != "ok" {
            eprintln!(
                "{} {} {} failed: stdout={} stderr={}",
                r.scenario, r.tool, r.phase, r.stdout_tail, r.stderr_tail
            );
        }
    }
}

fn run_cold_hot_matrix(scenario: &str, package_json: &str) -> Vec<BenchResult> {
    let td = tempdir().unwrap();
    let cache = td.path().join("jhol-cache");
    std::fs::create_dir_all(&cache).unwrap();
    let mut results = Vec::new();

    let tools: Vec<(&str, Box<dyn Fn(&Path) -> Option<Command>>)> = vec![
        (
            "jhol-jagr3",
            Box::new(|project| {
                Some(jhol_install_command(
                    project,
                    &project.join(".jhol-cache"),
                    "jagr3",
                ))
            }),
        ),
        (
            "jhol-pubgrub-v2",
            Box::new(|project| {
                Some(jhol_install_command(
                    project,
                    &project.join(".jhol-cache"),
                    "pubgrub-v2",
                ))
            }),
        ),
        (
            "npm",
            Box::new(|project| command_exists("npm").then(|| npm_install_command(project))),
        ),
        (
            "pnpm",
            Box::new(|project| command_exists("pnpm").then(|| pnpm_install_command(project))),
        ),
        (
            "bun",
            Box::new(|project| command_exists("bun").then(|| bun_install_command(project))),
        ),
    ];

    for (tool, build) in tools {
        let project = td.path().join(format!("{scenario}-{tool}"));
        create_project(&project, package_json);
        let Some(command) = build(&project) else {
            results.push(BenchResult {
                scenario: scenario.to_string(),
                tool: tool.to_string(),
                phase: "cold".to_string(),
                status: "skipped-missing-tool".to_string(),
                elapsed_ms: 0,
                peak_rss_kb: None,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
            });
            continue;
        };
        results.push(run_command(command, scenario, tool, "cold"));
        remove_install_outputs(&project);
        if let Some(command) = build(&project) {
            results.push(run_command(command, scenario, tool, "hot"));
        }
    }

    results
}

#[test]
#[ignore = "networked benchmark comparing cold/hot installs across package managers"]
fn benchmark_simple_project_cold_hot_across_package_managers() {
    let package_json = fixture_package_json(
        "bench-final-simple",
        &[
            ("left-pad", "1.3.0"),
            ("is-number", "7.0.0"),
            ("kleur", "4.1.5"),
        ],
    );
    let results = run_cold_hot_matrix("simple", &package_json);
    print_markdown_results(&results);
    assert!(results
        .iter()
        .any(|r| r.tool == "jhol-jagr3" && r.status == "ok"));
}

#[test]
#[ignore = "networked React dependency benchmark"]
fn benchmark_react_project_create_react_app_dependencies() {
    let package_json = fixture_package_json(
        "test-react",
        &[
            ("@testing-library/jest-dom", "^5.17.0"),
            ("@testing-library/react", "^13.4.0"),
            ("@testing-library/user-event", "^13.5.0"),
            ("react", "^18.3.1"),
            ("react-dom", "^18.3.1"),
            ("react-scripts", "5.0.1"),
            ("web-vitals", "^2.1.4"),
        ],
    );
    let results = run_cold_hot_matrix("react-cra", &package_json);
    print_markdown_results(&results);
}

#[test]
#[ignore = "networked Next.js dependency benchmark"]
fn benchmark_next_project_dependencies() {
    let package_json = fixture_package_json(
        "test-next",
        &[
            ("next", "^14.2.30"),
            ("react", "^18.3.1"),
            ("react-dom", "^18.3.1"),
        ],
    );
    let results = run_cold_hot_matrix("next", &package_json);
    print_markdown_results(&results);
}

#[test]
#[ignore = "networked complex peer dependency benchmark"]
fn benchmark_angular_complex_peer_dependencies() {
    let package_json = fixture_package_json(
        "test-angular-peers",
        &[
            ("@angular/animations", "^17.3.12"),
            ("@angular/common", "^17.3.12"),
            ("@angular/compiler", "^17.3.12"),
            ("@angular/core", "^17.3.12"),
            ("@angular/forms", "^17.3.12"),
            ("@angular/platform-browser", "^17.3.12"),
            ("rxjs", "^7.8.1"),
            ("tslib", "^2.6.2"),
            ("zone.js", "^0.14.10"),
        ],
    );
    let results = run_cold_hot_matrix("angular-peers", &package_json);
    print_markdown_results(&results);
}

#[test]
#[ignore = "workspace install benchmark"]
fn benchmark_jhol_workspace_monorepo_install() {
    let td = tempdir().unwrap();
    let repo = td.path().join("test-monorepo");
    std::fs::create_dir_all(repo.join("packages/a")).unwrap();
    std::fs::create_dir_all(repo.join("packages/b")).unwrap();
    std::fs::write(
        repo.join("package.json"),
        r#"{"name":"test-monorepo","private":true,"workspaces":["packages/*"],"dependencies":{"typescript":"^5.4.5"}}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("packages/a/package.json"),
        fixture_package_json("pkg-a", &[("react", "^18.3.1")]),
    )
    .unwrap();
    std::fs::write(
        repo.join("packages/b/package.json"),
        fixture_package_json("pkg-b", &[("lodash", "^4.17.21")]),
    )
    .unwrap();

    let result = run_command(
        {
            let mut c = Command::new(jhol_bin());
            c.current_dir(&repo)
                .env("JHOL_CACHE_DIR", td.path().join("cache"))
                .args([
                    "install",
                    "--all-workspaces",
                    "--resolver",
                    "jagr3",
                    "--no-scripts",
                    "--quiet",
                ]);
            c
        },
        "workspace",
        "jhol-jagr3",
        "cold",
    );
    print_markdown_results(&[result.clone()]);
    assert_eq!(result.status, "ok");
}

#[test]
#[ignore = "stress benchmark for large dependency graph"]
fn stress_large_dependency_tree_500_package_specs() {
    let common = [
        "@babel/core",
        "@babel/parser",
        "@babel/traverse",
        "@babel/types",
        "accepts",
        "ansi-colors",
        "argparse",
        "axios",
        "balanced-match",
        "brace-expansion",
        "chalk",
        "chokidar",
        "commander",
        "debug",
        "dotenv",
        "esbuild",
        "eslint",
        "express",
        "fast-glob",
        "fs-extra",
        "glob",
        "jest",
        "json5",
        "lodash",
        "micromatch",
        "minimist",
        "ms",
        "postcss",
        "prettier",
        "react",
        "react-dom",
        "resolve",
        "rimraf",
        "semver",
        "source-map",
        "supports-color",
        "tar",
        "typescript",
        "uuid",
        "webpack",
        "yargs",
    ];
    let deps: Vec<(String, String)> = (0..500)
        .map(|i| (common[i % common.len()].to_string(), "latest".to_string()))
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect();
    let borrowed: Vec<(&str, &str)> = deps.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let results = run_cold_hot_matrix(
        "large-500-specs",
        &fixture_package_json("large-500", &borrowed),
    );
    print_markdown_results(&results);
}

#[test]
#[ignore = "parallel process stress test"]
fn stress_concurrent_jhol_installs_use_isolated_projects_and_shared_cache() {
    let td = tempdir().unwrap();
    let cache = td.path().join("shared-cache");
    std::fs::create_dir_all(&cache).unwrap();
    let package_json = fixture_package_json(
        "concurrent",
        &[("left-pad", "1.3.0"), ("is-number", "7.0.0")],
    );
    let mut children = Vec::new();
    for idx in 0..4 {
        let project = td.path().join(format!("project-{idx}"));
        create_project(&project, &package_json);
        let mut child = Command::new(jhol_bin())
            .current_dir(project)
            .env("JHOL_CACHE_DIR", &cache)
            .args(["install", "--resolver", "jagr3", "--no-scripts", "--quiet"])
            .spawn()
            .unwrap();
        children.push(child.wait().unwrap());
    }

    assert!(
        children.iter().all(|s| s.success()),
        "all concurrent installs should succeed: {children:?}"
    );
}

#[test]
#[ignore = "offline/network-failure stress path"]
fn stress_offline_mode_fails_fast_on_empty_cache() {
    let td = tempdir().unwrap();
    let project = td.path().join("offline");
    create_project(
        &project,
        &fixture_package_json("offline", &[("left-pad", "1.3.0")]),
    );
    let result = run_command(
        {
            let mut c = Command::new(jhol_bin());
            c.current_dir(&project)
                .env("JHOL_CACHE_DIR", td.path().join("empty-cache"))
                .env("JHOL_BINARY_PACKAGES_DIR", td.path().join("no-bundle"))
                .args(["install", "--offline", "--resolver", "jagr3", "--quiet"]);
            c
        },
        "offline-empty-cache",
        "jhol-jagr3",
        "failure-path",
    );
    print_markdown_results(&[result.clone()]);
    assert_eq!(result.status, "failed");
    assert!(result.elapsed_ms < Duration::from_secs(20).as_millis());
}
