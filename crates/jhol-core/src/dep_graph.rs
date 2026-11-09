/// Append-only binary index of dependency edges, extracted from store tarballs.
///
/// After a tarball lands in `store_unpacked/<shard>/<hash>/`, we read its
/// `package.json`, extract `dependencies`/`peerDependencies`/`optionalDependencies`,
/// and append one block per (name,version) to the index. On the NEXT install,
/// `expand_with_transitive_dependencies` reads the local graph instead of fetching
/// packuments from the network — turning a 10s expansion into ~100ms.
///
/// Format (append-only binary):
/// ```text
/// [magic: 8 bytes] "JDEPGRAPH"
/// [version: u32 LE]
/// [entry_count: u64 LE]
/// --- entries (repeated entry_count times) ---
/// [name_len: u16 LE][name: UTF-8][version_len: u16 LE][version: UTF-8]
/// [deps_count: u16 LE]
///   [dep_name_len: u16 LE][dep_name: UTF-8][spec_len: u16 LE][spec: UTF-8]
///   ... (deps_count times)
/// ```
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::RwLock;

const MAGIC: &[u8; 10] = b"JDEPGRAPH\0";
const VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct DepEdge {
    pub child: String,
    pub spec: String,
}

struct DepGraphInner {
    edges: HashMap<(String, String), Vec<DepEdge>>,
    path: PathBuf,
}

/// Lazy-loaded global dependency graph.
/// Maps (package_name, version) -> list of (child_name, semver_spec).
static GRAPH: OnceLock<RwLock<DepGraphInner>> = OnceLock::new();

pub struct DepGraph;

/// Look up the deps of a specific name@version. O(1) hash lookup.
pub fn deps_of(name: &str, version: &str) -> Option<Vec<DepEdge>> {
    DepGraph::deps_of_inner(name, version)
}

/// Find the highest version of `name` in the graph whose deps we can use.
/// Returns (version, deps) for the best match, or None if the package has
/// never been installed on this machine.
pub fn best_version_deps(name: &str, constraints: &[String]) -> Option<(String, Vec<DepEdge>)> {
    DepGraph::best_version_deps_inner(name, constraints)
}

/// Append edges for one package. Called after tarball extraction.
pub fn append(name: &str, version: &str, deps: Vec<DepEdge>) {
    DepGraph::append_inner(name, version, deps)
}

/// Recursively expand all known dependencies for a set of root packages in a single read lock.
/// Returns a map of package_name -> (best_version, direct_deps).
pub fn expand_subgraph_in_memory(
    roots: &[(&str, &[String])],
) -> HashMap<String, (String, Vec<DepEdge>)> {
    DepGraph::expand_subgraph_inner(roots)
}

impl DepGraph {
    fn get() -> &'static RwLock<DepGraphInner> {
        GRAPH.get_or_init(|| {
            let path = Self::graph_path();
            let mut edges = HashMap::new();
            if path.exists() {
                let _ = Self::read_index(&path, &mut edges);
            }
            RwLock::new(DepGraphInner { edges, path })
        })
    }

    fn graph_path() -> PathBuf {
        let cache_dir = crate::utils::get_cache_dir();
        PathBuf::from(cache_dir).join("dep-graph.jbin")
    }

    /// Look up the deps of a specific name@version. O(1) hash lookup.
    fn deps_of_inner(name: &str, version: &str) -> Option<Vec<DepEdge>> {
        let graph = Self::get();
        let inner = graph.read();
        inner
            .edges
            .get(&(name.to_string(), version.to_string()))
            .cloned()
    }

    /// Append edges for one package. Called after tarball extraction.
    fn append_inner(name: &str, version: &str, deps: Vec<DepEdge>) {
        let graph = Self::get();
        {
            let inner = graph.read();
            if inner
                .edges
                .contains_key(&(name.to_string(), version.to_string()))
            {
                return; // already indexed
            }
        }
        let path = {
            let inner = graph.read();
            inner.path.clone()
        };
        let _ = Self::write_entry(&path, name, version, &deps);
        {
            let mut inner = graph.write();
            inner
                .edges
                .insert((name.to_string(), version.to_string()), deps);
        }
    }

    /// Find the highest version of `name` in the graph that satisfies all constraints.
    /// Returns (version, deps) for the best match.
    fn best_version_deps_inner(
        name: &str,
        constraints: &[String],
    ) -> Option<(String, Vec<DepEdge>)> {
        if constraints.is_empty() {
            return None;
        }
        let graph = Self::get();
        let inner = graph.read();
        let mut best: Option<(String, Vec<DepEdge>)> = None;
        for ((pkg_name, ver), deps) in inner.edges.iter() {
            if pkg_name != name {
                continue;
            }
            let dominated = constraints
                .iter()
                .all(|c| crate::registry::version_satisfies(c, ver));
            if !dominated {
                continue;
            }
            match &best {
                Some((best_ver, _)) => {
                    let is_higher = match (
                        semver::Version::parse(ver),
                        semver::Version::parse(best_ver),
                    ) {
                        (Ok(v_new), Ok(v_cur)) => v_new > v_cur,
                        _ => ver > best_ver,
                    };
                    if is_higher {
                        best = Some((ver.clone(), deps.clone()));
                    }
                }
                None => {
                    best = Some((ver.clone(), deps.clone()));
                }
            }
        }
        best
    }

    /// In-memory BFS traversal of the dependency DAG in a single read lock.
    fn expand_subgraph_inner(
        roots: &[(&str, &[String])],
    ) -> HashMap<String, (String, Vec<DepEdge>)> {
        let graph = Self::get();
        let inner = graph.read();
        let mut resolved: HashMap<String, (String, Vec<DepEdge>)> = HashMap::new();
        let mut queue: std::collections::VecDeque<(String, Vec<String>)> =
            std::collections::VecDeque::new();

        for (pkg, constraints) in roots {
            queue.push_back((pkg.to_string(), constraints.to_vec()));
        }

        while let Some((pkg_name, constraints)) = queue.pop_front() {
            if resolved.contains_key(&pkg_name) {
                continue;
            }

            let mut best: Option<(String, Vec<DepEdge>)> = None;
            for ((name, ver), deps) in inner.edges.iter() {
                if name != &pkg_name {
                    continue;
                }
                let dominated = constraints.is_empty()
                    || constraints
                        .iter()
                        .all(|c| crate::registry::version_satisfies(c, ver));
                if !dominated {
                    continue;
                }
                match &best {
                    Some((best_ver, _)) => {
                        let is_higher = match (
                            semver::Version::parse(ver),
                            semver::Version::parse(best_ver),
                        ) {
                            (Ok(v_new), Ok(v_cur)) => v_new > v_cur,
                            _ => ver > best_ver,
                        };
                        if is_higher {
                            best = Some((ver.clone(), deps.clone()));
                        }
                    }
                    None => {
                        best = Some((ver.clone(), deps.clone()));
                    }
                }
            }

            if let Some((best_ver, deps)) = best {
                for edge in &deps {
                    if !resolved.contains_key(&edge.child) {
                        queue.push_back((edge.child.clone(), vec![edge.spec.clone()]));
                    }
                }
                resolved.insert(pkg_name, (best_ver, deps));
            }
        }

        resolved
    }

    fn write_entry(
        path: &Path,
        name: &str,
        version: &str,
        deps: &[DepEdge],
    ) -> std::io::Result<()> {
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;

        // Check if we need to write the header (file is empty or new)
        let meta = f.metadata()?;
        if meta.len() == 0 {
            f.write_all(MAGIC)?;
            f.write_all(&VERSION.to_le_bytes())?;
            f.write_all(&0u64.to_le_bytes())?; // entry_count placeholder (we don't update it)
        }

        // Write entry
        write_string(&mut f, name)?;
        write_string(&mut f, version)?;
        f.write_all(&(deps.len() as u16).to_le_bytes())?;
        for dep in deps {
            write_string(&mut f, &dep.child)?;
            write_string(&mut f, &dep.spec)?;
        }
        f.flush()
    }

    fn read_index(
        path: &Path,
        edges: &mut HashMap<(String, String), Vec<DepEdge>>,
    ) -> std::io::Result<()> {
        let file = std::fs::File::open(path)?;
        let mut reader = BufReader::new(file);

        let mut magic = [0u8; 10];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad magic",
            ));
        }

        let mut ver_buf = [0u8; 4];
        reader.read_exact(&mut ver_buf)?;
        let _version = u32::from_le_bytes(ver_buf);

        let mut count_buf = [0u8; 8];
        reader.read_exact(&mut count_buf)?;

        // Read entries until EOF (ignore the header count — it may be 0)
        loop {
            let name = match read_string(&mut reader) {
                Ok(s) => s,
                Err(_) => break, // EOF
            };
            let version = match read_string(&mut reader) {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut deps_count_buf = [0u8; 2];
            if reader.read_exact(&mut deps_count_buf).is_err() {
                break;
            }
            let deps_count = u16::from_le_bytes(deps_count_buf) as usize;

            let mut deps = Vec::with_capacity(deps_count);
            for _ in 0..deps_count {
                let child = match read_string(&mut reader) {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let spec = match read_string(&mut reader) {
                    Ok(s) => s,
                    Err(_) => break,
                };
                deps.push(DepEdge { child, spec });
            }
            edges.insert((name, version), deps);
        }

        Ok(())
    }
}

fn write_string(w: &mut impl Write, s: &str) -> std::io::Result<()> {
    let bytes = s.as_bytes();
    w.write_all(&(bytes.len() as u16).to_le_bytes())?;
    w.write_all(bytes)
}

fn read_string(r: &mut impl Read) -> std::io::Result<String> {
    let mut len_buf = [0u8; 2];
    r.read_exact(&mut len_buf)?;
    let len = u16::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dep_graph_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dep-graph.jbin");

        // Write
        let edges = vec![
            DepEdge {
                child: "debug".to_string(),
                spec: "^4.0.0".to_string(),
            },
            DepEdge {
                child: "ms".to_string(),
                spec: "^2.1.0".to_string(),
            },
        ];
        DepGraph::write_entry(&path, "express", "5.2.1", &edges).unwrap();
        DepGraph::write_entry(
            &path,
            "react",
            "18.2.0",
            &vec![DepEdge {
                child: "scheduler".to_string(),
                spec: "0.23.0".to_string(),
            }],
        )
        .unwrap();

        // Read
        let mut loaded = HashMap::new();
        DepGraph::read_index(&path, &mut loaded).unwrap();

        assert_eq!(loaded.len(), 2);
        let express_deps = loaded
            .get(&("express".to_string(), "5.2.1".to_string()))
            .unwrap();
        assert_eq!(express_deps.len(), 2);
        assert_eq!(express_deps[0].child, "debug");
        assert_eq!(express_deps[0].spec, "^4.0.0");

        let react_deps = loaded
            .get(&("react".to_string(), "18.2.0".to_string()))
            .unwrap();
        assert_eq!(react_deps[0].child, "scheduler");
        assert_eq!(react_deps[0].spec, "0.23.0");
    }

    #[test]
    fn dep_graph_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dep-graph.jbin");

        // Writing to a new file creates the header
        let edges = vec![DepEdge {
            child: "x".to_string(),
            spec: "^1.0.0".to_string(),
        }];
        DepGraph::write_entry(&path, "pkg", "1.0.0", &edges).unwrap();

        let mut loaded = HashMap::new();
        DepGraph::read_index(&path, &mut loaded).unwrap();
        assert_eq!(loaded.len(), 1);
    }
}
