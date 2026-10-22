//! Task scheduling helpers (blueprint §11).
//!
//! Topological order for ordinary DAGs; service readiness is separate.

#![forbid(unsafe_code)]

use jhol_graph::{Edge, EdgeKind, Graph, GraphError, Node, NodeId, NodeKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub name: String,
    pub run: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub cache: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScheduleError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error("unknown dependency {0} required by {1}")]
    MissingDep(String, String),
}

/// Build a graph from named tasks and return topological execution order (names).
pub fn schedule_order(tasks: &[TaskSpec]) -> Result<Vec<String>, ScheduleError> {
    let mut g = Graph::new();
    let mut id_by_name: std::collections::HashMap<String, NodeId> =
        std::collections::HashMap::new();
    for t in tasks {
        let id = NodeId::compute(NodeKind::BuildAction, &t.name, "", "");
        id_by_name.insert(t.name.clone(), id.clone());
        g.insert_node(Node {
            id,
            kind: NodeKind::BuildAction,
            label: t.name.clone(),
        });
    }
    for t in tasks {
        let from = id_by_name.get(&t.name).unwrap().clone();
        for dep in &t.needs {
            let Some(to) = id_by_name.get(dep) else {
                return Err(ScheduleError::MissingDep(dep.clone(), t.name.clone()));
            };
            // Edge: dep must run before t → dep Requires? Actually t Requires dep means
            // t depends on dep, so edge from dep to t for topo (producer before consumer).
            g.add_edge(Edge {
                from: to.clone(),
                to: from.clone(),
                kind: EdgeKind::Requires,
            })?;
        }
    }
    g.validate_acyclic()?;

    // Kahn again for ordered names
    use std::collections::{HashMap, VecDeque};
    let mut indeg: HashMap<&str, usize> = tasks.iter().map(|t| (t.name.as_str(), 0)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in tasks {
        for dep in &t.needs {
            *indeg.get_mut(t.name.as_str()).unwrap() += 1;
            adj.entry(dep.as_str()).or_default().push(t.name.as_str());
        }
    }
    let mut q: VecDeque<&str> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    // stable-ish: sort ready set
    let mut order = Vec::new();
    while !q.is_empty() {
        let mut ready: Vec<&str> = q.drain(..).collect();
        ready.sort_unstable();
        for n in ready {
            order.push(n.to_string());
            if let Some(outs) = adj.get(n) {
                for t in outs {
                    let d = indeg.get_mut(*t).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        q.push_back(*t);
                    }
                }
            }
        }
    }
    if order.len() != tasks.len() {
        return Err(ScheduleError::Graph(GraphError::Cycle("task".into())));
    }
    Ok(order)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSpec {
    pub name: String,
    /// Command to start the service (left running until cancelled — caller owns process).
    pub run: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Other services that must report ready first.
    #[serde(default)]
    pub needs: Vec<String>,
    /// Optional TCP host:port probe for readiness (e.g. "127.0.0.1:5432").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_tcp: Option<String>,
}

#[derive(Debug, Error)]
pub enum ServiceConfigError {
    #[error("could not read jhol.toml: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid jhol.toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unsupported jhol.toml schema {0}; this Jhol build supports schema 1")]
    SchemaVersion(u32),
    #[error("invalid service declaration: {0}")]
    InvalidService(String),
    #[error("jhol.toml has no service with a non-empty `run` command")]
    NoRunnableServices,
}

#[derive(Debug, Deserialize)]
struct ProjectServiceConfig {
    name: String,
    #[allow(dead_code)]
    kind: String,
    #[serde(default)]
    run: Option<Vec<String>>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    needs: Vec<String>,
    #[serde(default)]
    ready_tcp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectConfig {
    schema: u32,
    #[serde(default)]
    service: Vec<ProjectServiceConfig>,
}

/// Read runnable service declarations from schema-versioned `jhol.toml`.
///
/// Service working directories are resolved under the project root and must
/// exist there. Commands remain argument arrays; no shell is inserted.
pub fn load_project_services(project_root: &Path) -> Result<Vec<ServiceSpec>, ServiceConfigError> {
    let root = fs::canonicalize(project_root)?;
    let text = fs::read_to_string(root.join("jhol.toml"))?;
    let project: ProjectConfig = toml::from_str(&text)?;
    if project.schema != 1 {
        return Err(ServiceConfigError::SchemaVersion(project.schema));
    }

    let mut seen_names = HashSet::new();
    for service in &project.service {
        if service.name.trim().is_empty() {
            return Err(ServiceConfigError::InvalidService(
                "service name cannot be empty".into(),
            ));
        }
        if !seen_names.insert(service.name.clone()) {
            return Err(ServiceConfigError::InvalidService(format!(
                "duplicate service name `{}`",
                service.name
            )));
        }
    }

    let mut services = Vec::new();
    for service in project.service {
        let Some(run) = service.run else {
            continue;
        };
        if run.is_empty() || run[0].trim().is_empty() {
            return Err(ServiceConfigError::InvalidService(format!(
                "service `{}` needs a non-empty `run` argument array",
                service.name
            )));
        }
        if run.iter().any(|arg| arg.contains('\0')) {
            return Err(ServiceConfigError::InvalidService(format!(
                "service `{}` command contains a NUL byte",
                service.name
            )));
        }

        let cwd = match service.cwd {
            None => root.clone(),
            Some(relative) => {
                if relative.is_absolute()
                    || relative.components().any(|part| {
                        matches!(
                            part,
                            Component::ParentDir | Component::RootDir | Component::Prefix(_)
                        )
                    })
                {
                    return Err(ServiceConfigError::InvalidService(format!(
                        "service `{}` cwd must stay inside the project root",
                        service.name
                    )));
                }
                let resolved = fs::canonicalize(root.join(relative))?;
                if !resolved.starts_with(&root) || !resolved.is_dir() {
                    return Err(ServiceConfigError::InvalidService(format!(
                        "service `{}` cwd must be an existing directory inside the project root",
                        service.name
                    )));
                }
                resolved
            }
        };

        if let Some(address) = &service.ready_tcp {
            if address.parse::<std::net::SocketAddr>().is_err() {
                return Err(ServiceConfigError::InvalidService(format!(
                    "service `{}` ready_tcp must be an IP address and port, got `{address}`",
                    service.name
                )));
            }
        }

        services.push(ServiceSpec {
            name: service.name,
            run,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            needs: service.needs,
            ready_tcp: service.ready_tcp,
        });
    }
    if services.is_empty() {
        return Err(ServiceConfigError::NoRunnableServices);
    }
    Ok(services)
}

/// Topological start order for services (same DAG rules as tasks).
pub fn service_boot_order(services: &[ServiceSpec]) -> Result<Vec<String>, ScheduleError> {
    let tasks: Vec<TaskSpec> = services
        .iter()
        .map(|s| TaskSpec {
            name: s.name.clone(),
            run: s.run.clone(),
            cwd: s.cwd.clone(),
            needs: s.needs.clone(),
            cache: false,
        })
        .collect();
    schedule_order(&tasks)
}

/// True when `host:port` accepts a TCP connect (best-effort readiness).
pub fn tcp_ready(addr: &str) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    TcpStream::connect_timeout(
        &match addr.parse() {
            Ok(a) => a,
            Err(_) => return false,
        },
        Duration::from_millis(200),
    )
    .is_ok()
}

/// Child process owned by the scheduler; killed on drop (cleanup).
pub struct SpawnedService {
    pub name: String,
    child: std::process::Child,
}

impl Drop for SpawnedService {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SpawnError {
    #[error("service {0} not in boot order")]
    Unknown(String),
    #[error("empty run command for {0}")]
    EmptyRun(String),
    #[error("spawn {name}: {detail}")]
    Io { name: String, detail: String },
    #[error("service {0} not ready within timeout ({1})")]
    NotReady(String, String),
    #[error("service {0} readiness endpoint is already accepting connections: {1}")]
    AlreadyReady(String, String),
}

/// Start services in `order`; wait for each `ready_tcp` (if set) before next.
/// Caller must keep the returned handles alive; drop kills children.
pub fn spawn_services(
    services: &[ServiceSpec],
    order: &[String],
    ready_timeout_ms: u64,
) -> Result<Vec<SpawnedService>, SpawnError> {
    use std::collections::HashMap;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    let by_name: HashMap<&str, &ServiceSpec> =
        services.iter().map(|s| (s.name.as_str(), s)).collect();
    let mut handles = Vec::new();
    for name in order {
        let spec = by_name
            .get(name.as_str())
            .ok_or_else(|| SpawnError::Unknown(name.clone()))?;
        if spec.run.is_empty() {
            return Err(SpawnError::EmptyRun(name.clone()));
        }
        if let Some(addr) = &spec.ready_tcp {
            if tcp_ready(addr) {
                return Err(SpawnError::AlreadyReady(name.clone(), addr.clone()));
            }
        }
        let mut cmd = Command::new(&spec.run[0]);
        if spec.run.len() > 1 {
            cmd.args(&spec.run[1..]);
        }
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        let child = cmd.spawn().map_err(|e| SpawnError::Io {
            name: name.clone(),
            detail: e.to_string(),
        })?;
        handles.push(SpawnedService {
            name: name.clone(),
            child,
        });
        if let Some(addr) = &spec.ready_tcp {
            let deadline = Instant::now() + Duration::from_millis(ready_timeout_ms);
            while Instant::now() < deadline {
                if tcp_ready(addr) {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if !tcp_ready(addr) {
                return Err(SpawnError::NotReady(name.clone(), addr.clone()));
            }
        }
    }
    Ok(handles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topo_respects_needs() {
        let tasks = vec![
            TaskSpec {
                name: "test".into(),
                run: vec!["pytest".into()],
                cwd: None,
                needs: vec!["build".into()],
                cache: false,
            },
            TaskSpec {
                name: "build".into(),
                run: vec!["cargo".into(), "build".into()],
                cwd: None,
                needs: vec![],
                cache: true,
            },
        ];
        let order = schedule_order(&tasks).unwrap();
        assert_eq!(order, vec!["build".to_string(), "test".to_string()]);
    }

    #[test]
    fn service_order_api_after_db() {
        let svcs = vec![
            ServiceSpec {
                name: "api".into(),
                run: vec!["uvicorn".into()],
                cwd: None,
                needs: vec!["db".into()],
                ready_tcp: Some("127.0.0.1:8000".into()),
            },
            ServiceSpec {
                name: "db".into(),
                run: vec!["postgres".into()],
                cwd: None,
                needs: vec![],
                ready_tcp: Some("127.0.0.1:5432".into()),
            },
        ];
        assert_eq!(
            service_boot_order(&svcs).unwrap(),
            vec!["db".to_string(), "api".to_string()]
        );
    }

    #[test]
    fn loads_only_project_declared_services_and_resolves_cwd_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("services/api")).unwrap();
        fs::write(
            dir.path().join("jhol.toml"),
            r#"
schema = 1
[project]
name = "sample"

[[service]]
name = "api"
kind = "other"
run = ["python", "-m", "http.server", "8000"]
cwd = "services/api"
needs = ["db"]
ready_tcp = "127.0.0.1:8000"

[[service]]
name = "db"
kind = "postgresql"
run = ["postgres", "-D", "data"]
ready_tcp = "[::1]:5432"
"#,
        )
        .unwrap();

        let services = load_project_services(dir.path()).unwrap();
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].run, ["python", "-m", "http.server", "8000"]);
        assert_eq!(
            Path::new(services[0].cwd.as_deref().unwrap()),
            dir.path().canonicalize().unwrap().join("services/api")
        );
        assert_eq!(
            service_boot_order(&services).unwrap(),
            vec!["db".to_string(), "api".to_string()]
        );
    }

    #[test]
    fn refuses_service_working_directory_outside_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("jhol.toml"),
            "schema = 1\n[[service]]\nname = \"bad\"\nkind = \"other\"\nrun = [\"echo\"]\ncwd = \"../outside\"\n",
        )
        .unwrap();

        let err = load_project_services(dir.path()).unwrap_err();
        assert!(matches!(err, ServiceConfigError::InvalidService(_)));
    }

    #[test]
    fn refuses_to_spawn_services_without_declared_commands() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("jhol.toml"),
            "schema = 1\n[[service]]\nname = \"db\"\nkind = \"postgresql\"\n",
        )
        .unwrap();

        assert!(matches!(
            load_project_services(dir.path()),
            Err(ServiceConfigError::NoRunnableServices)
        ));
    }

    #[test]
    fn rejects_unsupported_project_schema_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("jhol.toml"), "schema = 2\n").unwrap();

        assert!(matches!(
            load_project_services(dir.path()),
            Err(ServiceConfigError::SchemaVersion(2))
        ));
    }

    #[test]
    fn spawn_sleep_then_drop_cleans_up() {
        let svcs = vec![ServiceSpec {
            name: "sleeper".into(),
            run: vec!["sleep".into(), "30".into()],
            cwd: None,
            needs: vec![],
            ready_tcp: None,
        }];
        let order = service_boot_order(&svcs).unwrap();
        let handles = spawn_services(&svcs, &order, 500).unwrap();
        assert_eq!(handles.len(), 1);
        drop(handles); // must not hang
    }

    #[test]
    fn spawn_refuses_an_already_ready_endpoint() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let services = vec![ServiceSpec {
            name: "api".into(),
            run: vec!["sleep".into(), "30".into()],
            cwd: None,
            needs: vec![],
            ready_tcp: Some(addr.clone()),
        }];
        let order = service_boot_order(&services).unwrap();

        assert!(matches!(
            spawn_services(&services, &order, 500),
            Err(SpawnError::AlreadyReady(name, endpoint)) if name == "api" && endpoint == addr
        ));
        drop(listener);
    }
}
