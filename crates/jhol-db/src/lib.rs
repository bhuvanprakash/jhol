//! Database adapter — read-only inspect first (blueprint §13 / Phase C).
//! Never applies migrations unless explicitly asked later; this crate plans only.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationFile {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaPlan {
    pub migrations_dir: String,
    pub migrations: Vec<MigrationFile>,
    pub drift_notes: Vec<String>,
    pub limitations: Vec<String>,
}

fn digest_file(path: &Path) -> Result<String, DbError> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Discover `migrations/*.sql` (or given dir) and hash each file. No DB connection.
pub fn plan_migrations(project: &Path, migrations_dir: &str) -> Result<SchemaPlan, DbError> {
    let dir = project.join(migrations_dir);
    let mut migrations = Vec::new();
    let mut drift_notes = Vec::new();
    if !dir.is_dir() {
        drift_notes.push(format!("no migrations directory at {migrations_dir}"));
        return Ok(SchemaPlan {
            migrations_dir: migrations_dir.into(),
            migrations,
            drift_notes,
            limitations: vec![
                "read-only filesystem plan — no live PostgreSQL probe in this build".into(),
            ],
        });
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort();
    for p in entries {
        let rel = p
            .strip_prefix(project)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        migrations.push(MigrationFile {
            path: rel,
            digest: digest_file(&p)?,
        });
    }
    Ok(SchemaPlan {
        migrations_dir: migrations_dir.into(),
        migrations,
        drift_notes,
        limitations: vec![
            "filesystem plan only — use `jhol db probe` for optional live TCP check".into(),
            "apply requires explicit future `jhol db migrate --apply`".into(),
        ],
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbProbeResult {
    pub database_url_set: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub reachable: Option<bool>,
    pub note: String,
}

/// Parse `postgres://user:pass@host:port/db` (or host:port) for a TCP probe.
/// Never logs credentials. No SQL — connectivity only.
pub fn probe_database_url(url: &str, timeout_ms: u64) -> DbProbeResult {
    let (host, port) = match parse_pg_host_port(url) {
        Some(hp) => hp,
        None => {
            return DbProbeResult {
                database_url_set: !url.is_empty(),
                host: None,
                port: None,
                reachable: None,
                note: "could not parse host/port from DATABASE_URL".into(),
            };
        }
    };
    let addr = format!("{host}:{port}");
    use std::net::ToSocketAddrs;
    let reachable = match addr.to_socket_addrs() {
        Ok(mut iter) => iter.any(|sa| {
            std::net::TcpStream::connect_timeout(&sa, std::time::Duration::from_millis(timeout_ms))
                .is_ok()
        }),
        Err(_) => false,
    };
    DbProbeResult {
        database_url_set: true,
        host: Some(host),
        port: Some(port),
        reachable: Some(reachable),
        note: if reachable {
            "TCP accept on host:port (no SQL executed)".into()
        } else {
            "TCP unreachable — start disposable Postgres (see examples/blueprint/docker-compose.db.yml)"
                .into()
        },
    }
}

pub fn probe_from_env(timeout_ms: u64) -> DbProbeResult {
    match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => probe_database_url(&url, timeout_ms),
        _ => DbProbeResult {
            database_url_set: false,
            host: None,
            port: None,
            reachable: None,
            note: "DATABASE_URL unset — filesystem plan only".into(),
        },
    }
}

fn parse_pg_host_port(url: &str) -> Option<(String, u16)> {
    let s = url.trim();
    if let Some(rest) = s
        .strip_prefix("postgres://")
        .or_else(|| s.strip_prefix("postgresql://"))
    {
        let after_at = rest.split('@').nth(1).unwrap_or(rest);
        let hostport = after_at.split('/').next().unwrap_or(after_at);
        let hostport = hostport.split('?').next().unwrap_or(hostport);
        if let Some((h, p)) = hostport.rsplit_once(':') {
            let port: u16 = p.parse().ok()?;
            return Some((h.to_string(), port));
        }
        return Some((hostport.to_string(), 5432));
    }
    if let Some((h, p)) = s.rsplit_once(':') {
        let port: u16 = p.parse().ok()?;
        return Some((h.to_string(), port));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sql_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let mig = dir.path().join("migrations");
        fs::create_dir_all(&mig).unwrap();
        fs::write(mig.join("001_init.sql"), b"create table t(id int);").unwrap();
        let plan = plan_migrations(dir.path(), "migrations").unwrap();
        assert_eq!(plan.migrations.len(), 1);
        assert!(plan.migrations[0].path.contains("001_init.sql"));
    }

    #[test]
    fn parses_postgres_url_host_port() {
        assert_eq!(
            parse_pg_host_port("postgres://u:p@127.0.0.1:5433/app"),
            Some(("127.0.0.1".into(), 5433))
        );
        assert_eq!(
            parse_pg_host_port("postgresql://localhost/db"),
            Some(("localhost".into(), 5432))
        );
    }

    #[test]
    fn probe_bad_url() {
        let r = probe_database_url("not-a-url", 50);
        assert!(r.host.is_none());
        assert!(r.note.contains("could not parse"));
    }
}
