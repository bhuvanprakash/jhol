//! Database adapter — read-only inspect first (blueprint §13 / Phase C).
//!
//! This crate plans and verifies migrations; it never applies them and never
//! executes SQL. A database URL is parsed only to derive a TCP target, and
//! credentials are redacted before any value leaves this module.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const LEDGER_FORMAT: u32 = 1;
pub const LEDGER_REL_PATH: &str = ".jhol/db-migrations.json";

#[derive(Debug, Error)]
pub enum DbError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationFile {
    pub path: String,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Numeric prefix parsed from the filename, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaPlan {
    pub migrations_dir: String,
    pub migrations: Vec<MigrationFile>,
    pub drift_notes: Vec<String>,
    pub limitations: Vec<String>,
    #[serde(default)]
    pub total_bytes: u64,
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

fn digest_text(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    hex::encode(h.finalize())
}

/// Parse a leading numeric migration order (`001_init.sql` → 1).
pub fn migration_order(file_name: &str) -> Option<u64> {
    let digits: String = file_name
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn sort_key(path: &Path) -> (u64, String) {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    (migration_order(&name).unwrap_or(u64::MAX), name)
}

/// Discover `*.sql` in `migrations_dir`, hash each file, and order naturally.
/// No database connection is made.
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
                "read-only filesystem plan — no live database probe in this build".into(),
            ],
            total_bytes: 0,
        });
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort_by_key(|p| sort_key(p));
    let mut total_bytes = 0u64;
    for p in entries {
        let rel = p
            .strip_prefix(project)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        let size_bytes = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        total_bytes = total_bytes.saturating_add(size_bytes);
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        migrations.push(MigrationFile {
            path: rel,
            digest: digest_file(&p)?,
            size_bytes: Some(size_bytes),
            order: migration_order(name),
        });
    }
    if migrations.iter().any(|m| m.order.is_none()) {
        drift_notes.push(
            "some migration filenames have no numeric prefix; they sort after numbered files"
                .into(),
        );
    }
    let mut seen = std::collections::BTreeMap::new();
    for m in &migrations {
        if let Some(order) = m.order {
            if let Some(first) = seen.insert(order, m.path.clone()) {
                drift_notes.push(format!(
                    "duplicate migration order {order}: {} and {}",
                    first, m.path
                ));
            }
        }
    }
    Ok(SchemaPlan {
        migrations_dir: migrations_dir.into(),
        migrations,
        drift_notes,
        limitations: vec![
            "filesystem plan only — use `jhol db probe` for an optional TCP check".into(),
            "apply requires an explicit future `jhol db migrate --apply`".into(),
        ],
        total_bytes,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationLedger {
    pub format_version: u32,
    pub migrations_dir: String,
    pub files: Vec<MigrationFile>,
}

pub fn ledger_path(project: &Path) -> PathBuf {
    project.join(LEDGER_REL_PATH)
}

pub fn read_ledger(project: &Path) -> Result<Option<MigrationLedger>, DbError> {
    let path = ledger_path(project);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&text)?))
}

/// Write the reviewed migration order and digests. This is a plan artifact,
/// not a database mutation.
pub fn write_ledger(project: &Path, plan: &SchemaPlan) -> Result<PathBuf, DbError> {
    let path = ledger_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let ledger = MigrationLedger {
        format_version: LEDGER_FORMAT,
        migrations_dir: plan.migrations_dir.clone(),
        files: plan.migrations.clone(),
    };
    fs::write(&path, serde_json::to_string_pretty(&ledger)?)?;
    Ok(path)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DriftReport {
    pub migrations_dir: String,
    pub clean: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
    pub reordered: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Pure comparison of a reviewed ledger with the current plan.
pub fn compare_ledger(ledger: &[MigrationFile], current: &[MigrationFile]) -> DriftReport {
    use std::collections::BTreeMap;
    let old: BTreeMap<&str, &MigrationFile> = ledger.iter().map(|m| (m.path.as_str(), m)).collect();
    let new: BTreeMap<&str, &MigrationFile> =
        current.iter().map(|m| (m.path.as_str(), m)).collect();

    let mut report = DriftReport::default();
    for path in new.keys() {
        match old.get(path) {
            None => report.added.push((*path).to_string()),
            Some(before) => {
                if before.digest != new[path].digest {
                    report.modified.push((*path).to_string());
                }
            }
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            report.removed.push((*path).to_string());
        }
    }

    let old_order: Vec<&str> = ledger
        .iter()
        .filter(|m| new.contains_key(m.path.as_str()))
        .map(|m| m.path.as_str())
        .collect();
    let new_order: Vec<&str> = current
        .iter()
        .filter(|m| old.contains_key(m.path.as_str()))
        .map(|m| m.path.as_str())
        .collect();
    if old_order != new_order {
        report.reordered = new_order
            .iter()
            .zip(old_order.iter())
            .filter(|(a, b)| a != b)
            .map(|(a, _)| (*a).to_string())
            .collect();
    }

    report.clean = report.added.is_empty()
        && report.removed.is_empty()
        && report.modified.is_empty()
        && report.reordered.is_empty();
    report
}

/// Verify the current migrations directory against `.jhol/db-migrations.json`.
pub fn verify_migrations(project: &Path, migrations_dir: &str) -> Result<DriftReport, DbError> {
    let plan = plan_migrations(project, migrations_dir)?;
    let Some(ledger) = read_ledger(project)? else {
        let mut report = DriftReport {
            migrations_dir: migrations_dir.into(),
            clean: false,
            ..DriftReport::default()
        };
        report
            .added
            .extend(plan.migrations.iter().map(|m| m.path.clone()));
        report.notes.push(format!(
            "no migration ledger at {LEDGER_REL_PATH}; run `jhol db plan --write-lock` after review"
        ));
        return Ok(report);
    };
    let mut report = compare_ledger(&ledger.files, &plan.migrations);
    report.migrations_dir = migrations_dir.into();
    if ledger.format_version != LEDGER_FORMAT {
        report.notes.push(format!(
            "ledger format {} differs from supported {}",
            ledger.format_version, LEDGER_FORMAT
        ));
    }
    if ledger.migrations_dir != migrations_dir {
        report.notes.push(format!(
            "ledger was written for `{}`, verifying `{}`",
            ledger.migrations_dir, migrations_dir
        ));
    }
    report.notes.extend(plan.drift_notes);
    Ok(report)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationAnalysis {
    pub path: String,
    pub digest: String,
    pub statements: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destructive: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_transactional: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Strip comments and string literals so statement scanning is not fooled by
/// text such as `-- drop table` or `select 'drop table'`.
///
/// Handles line/block comments, single-quoted strings (with `''` escapes),
/// double-quoted identifiers (with `""` escapes) and Postgres dollar-quoted
/// bodies (`$$…$$`, `$tag$…$tag$`) — a `;` or `DROP TABLE` inside any of
/// those must not split or flag statements. Non-ASCII bytes are decoded as
/// UTF-8, not pushed through Latin-1.
fn scrub_sql(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            out.push(' ');
        } else if bytes[i] == b'\'' || bytes[i] == b'"' {
            // Quoted string or identifier: keep the quotes, drop the body.
            let quote = bytes[i];
            out.push(quote as char);
            out.push(quote as char);
            i += 1;
            while i < bytes.len() {
                if bytes[i] == quote {
                    if i + 1 < bytes.len() && bytes[i + 1] == quote {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if bytes[i] == b'$' {
            // Postgres dollar quote: `$`, `$tag$` … same delimiter closes it.
            let mut tag_end = i + 1;
            while tag_end < bytes.len()
                && (bytes[tag_end].is_ascii_alphanumeric() || bytes[tag_end] == b'_')
            {
                tag_end += 1;
            }
            if tag_end < bytes.len() && bytes[tag_end] == b'$' {
                let open = &sql[i..=tag_end];
                out.push_str(open);
                out.push(' ');
                let rest = &sql[tag_end + 1..];
                if let Some(close_at) = rest.find(open) {
                    i = tag_end + 1 + close_at + open.len();
                    out.push_str(open);
                } else {
                    i = bytes.len(); // unterminated: swallow the rest
                }
            } else {
                push_char(&mut out, sql, &mut i);
            }
        } else {
            push_char(&mut out, sql, &mut i);
        }
    }
    out
}

/// Push one UTF-8 char starting at `*i`, advancing past it.
fn push_char(out: &mut String, sql: &str, i: &mut usize) {
    match sql[*i..].chars().next() {
        Some(ch) => {
            out.push(ch);
            *i += ch.len_utf8();
        }
        None => *i += 1,
    }
}

fn split_statements(sql: &str) -> Vec<String> {
    scrub_sql(sql)
        .split(';')
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect()
}

fn has_where(statement: &str) -> bool {
    statement
        .split_whitespace()
        .any(|word| word.eq_ignore_ascii_case("where"))
}

/// Static safety analysis. This is advisory text, never an execution gate.
pub fn analyze_migration(path: &Path, sql: &str) -> MigrationAnalysis {
    let statements = split_statements(sql);
    let mut destructive = Vec::new();
    let mut non_transactional = Vec::new();
    let mut notes = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        let upper = statement.to_ascii_uppercase();
        let label = format!("#{} {}", index + 1, truncate(statement, 80));
        if upper.starts_with("DROP TABLE")
            || upper.starts_with("DROP DATABASE")
            || upper.starts_with("DROP SCHEMA")
            || upper.starts_with("TRUNCATE")
        {
            destructive.push(label.clone());
        } else if upper.starts_with("DELETE FROM") && !has_where(statement) {
            destructive.push(format!("{label} (DELETE without WHERE)"));
        } else if upper.starts_with("UPDATE ") && !has_where(statement) {
            destructive.push(format!("{label} (UPDATE without WHERE)"));
        } else if upper.starts_with("ALTER TABLE") && upper.contains(" DROP ") {
            destructive.push(format!("{label} (ALTER ... DROP)"));
        }
        if upper.starts_with("CREATE INDEX CONCURRENTLY")
            || upper.starts_with("CREATE DATABASE")
            || upper.starts_with("VACUUM")
        {
            non_transactional.push(label.clone());
        }
        if upper.contains(" ALTER TYPE ") && upper.contains(" ADD VALUE") {
            non_transactional.push(format!("{label} (ALTER TYPE ADD VALUE)"));
        }
        if upper.starts_with("BEGIN")
            || upper.starts_with("COMMIT")
            || upper.starts_with("ROLLBACK")
        {
            notes.push(format!("{label} controls its own transaction boundary"));
        }
    }
    if statements.is_empty() {
        notes.push("migration contains no SQL statements".into());
    }
    MigrationAnalysis {
        path: path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string(),
        digest: digest_text(sql),
        statements: statements.len(),
        destructive,
        non_transactional,
        notes,
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Analyze every `*.sql` file in natural order.
pub fn scan_migrations(
    project: &Path,
    migrations_dir: &str,
) -> Result<Vec<MigrationAnalysis>, DbError> {
    let dir = project.join(migrations_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    paths.sort_by_key(|p| sort_key(p));
    let mut analyses = Vec::new();
    for path in paths {
        let sql = fs::read_to_string(&path)?;
        analyses.push(analyze_migration(&path, &sql));
    }
    Ok(analyses)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DbEngine {
    Postgres,
    Mysql,
    Sqlite,
    Redis,
    Unknown,
}

impl DbEngine {
    pub fn default_port(self) -> Option<u16> {
        match self {
            DbEngine::Postgres => Some(5432),
            DbEngine::Mysql => Some(3306),
            DbEngine::Redis => Some(6379),
            DbEngine::Sqlite | DbEngine::Unknown => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DbEngine::Postgres => "postgres",
            DbEngine::Mysql => "mysql",
            DbEngine::Sqlite => "sqlite",
            DbEngine::Redis => "redis",
            DbEngine::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParsedDbUrl {
    pub engine: DbEngine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub redacted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbProbeResult {
    pub database_url_set: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_url: Option<String>,
}

/// Parse postgres/mysql/redis/sqlite URLs, or a bare `host:port`.
pub fn parse_database_url(url: &str) -> Option<ParsedDbUrl> {
    let s = url.trim();
    if s.is_empty() {
        return None;
    }
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => {
            if s.ends_with(".db") || s.ends_with(".sqlite") || s.ends_with(".sqlite3") {
                return Some(ParsedDbUrl {
                    engine: DbEngine::Sqlite,
                    host: None,
                    port: None,
                    database: Some(s.to_string()),
                    user: None,
                    redacted: redact_database_url(s),
                });
            }
            let (host, port) = s.rsplit_once(':')?;
            let port: u16 = port.parse().ok()?;
            if host.is_empty() {
                return None;
            }
            return Some(ParsedDbUrl {
                engine: DbEngine::Unknown,
                host: Some(host.to_string()),
                port: Some(port),
                database: None,
                user: None,
                redacted: redact_database_url(s),
            });
        }
    };

    let engine = match scheme.as_str() {
        "postgres" | "postgresql" => DbEngine::Postgres,
        "mysql" | "mariadb" => DbEngine::Mysql,
        "redis" | "rediss" => DbEngine::Redis,
        "sqlite" | "sqlite3" => DbEngine::Sqlite,
        _ => DbEngine::Unknown,
    };

    if engine == DbEngine::Sqlite {
        let path = if rest.is_empty() { ":memory:" } else { rest };
        return Some(ParsedDbUrl {
            engine,
            host: None,
            port: None,
            database: Some(path.to_string()),
            user: None,
            redacted: redact_database_url(s),
        });
    }

    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, authority),
    };
    let user = userinfo
        .and_then(|u| u.split(':').next())
        .filter(|u| !u.is_empty());
    let (host, port) = match hostport.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            let port: u16 = port.parse().ok()?;
            (host.to_string(), Some(port))
        }
        _ if !hostport.is_empty() => (hostport.to_string(), engine.default_port()),
        _ => return None,
    };
    let database = rest
        .split('/')
        .nth(1)
        .map(|d| d.split('?').next().unwrap_or(d))
        .filter(|d| !d.is_empty())
        .map(str::to_string);

    Some(ParsedDbUrl {
        engine,
        host: Some(host),
        port,
        database,
        user: user.map(str::to_string),
        redacted: redact_database_url(s),
    })
}

/// Remove passwords and secret query parameters from a connection URL.
pub fn redact_database_url(url: &str) -> String {
    let mut out = url.to_string();
    if let Some((scheme, rest)) = out.clone().split_once("://") {
        let (authority, tail) = match rest.split_once('/') {
            Some((authority, tail)) => (authority, format!("/{tail}")),
            None => (rest, String::new()),
        };
        let redacted_authority = match authority.rsplit_once('@') {
            Some((userinfo, host)) => match userinfo.split_once(':') {
                Some((user, _password)) => format!("{user}:***@{host}"),
                None => format!("{userinfo}@{host}"),
            },
            None => authority.to_string(),
        };
        out = format!("{scheme}://{redacted_authority}{tail}");
    }
    redact_query_secrets(&out)
}

fn redact_query_secrets(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let mut parts = Vec::new();
    for pair in query.split('&') {
        let (key, _) = pair.split_once('=').unwrap_or((pair, ""));
        if is_secret_query_key(key) {
            parts.push(format!("{key}=***"));
        } else {
            parts.push(pair.to_string());
        }
    }
    format!("{base}?{}", parts.join("&"))
}

fn is_secret_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "password", "passwd", "token", "secret", "apikey", "api_key", "auth",
    ]
    .iter()
    .any(|needle| key == *needle || key.ends_with(needle))
}

/// Connectivity-only probe. No SQL is executed.
pub fn probe_database_url(url: &str, timeout_ms: u64) -> DbProbeResult {
    let Some(parsed) = parse_database_url(url) else {
        return DbProbeResult {
            database_url_set: !url.is_empty(),
            engine: None,
            host: None,
            port: None,
            reachable: None,
            note: "could not parse host/port from DATABASE_URL".into(),
            redacted_url: Some(redact_database_url(url)),
        };
    };
    if parsed.engine == DbEngine::Sqlite {
        let exists = parsed
            .database
            .as_deref()
            .map(|p| p == ":memory:" || Path::new(p).exists())
            .unwrap_or(false);
        return DbProbeResult {
            database_url_set: true,
            engine: Some(parsed.engine.as_str().into()),
            host: None,
            port: None,
            reachable: None,
            note: if exists {
                "sqlite file target exists (no connection opened)".into()
            } else {
                "sqlite file target does not exist (no connection opened)".into()
            },
            redacted_url: Some(parsed.redacted),
        };
    }
    let (Some(host), Some(port)) = (parsed.host.clone(), parsed.port) else {
        return DbProbeResult {
            database_url_set: true,
            engine: Some(parsed.engine.as_str().into()),
            host: parsed.host,
            port: None,
            reachable: None,
            note: "could not derive host/port from DATABASE_URL".into(),
            redacted_url: Some(parsed.redacted),
        };
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
        engine: Some(parsed.engine.as_str().into()),
        host: Some(host),
        port: Some(port),
        reachable: Some(reachable),
        note: if reachable {
            "TCP accept on host:port (no SQL executed)".into()
        } else {
            "TCP unreachable — start a disposable database (see examples/blueprint/docker-compose.db.yml)"
                .into()
        },
        redacted_url: Some(parsed.redacted),
    }
}

pub fn probe_from_env(timeout_ms: u64) -> DbProbeResult {
    match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => probe_database_url(&url, timeout_ms),
        _ => DbProbeResult {
            database_url_set: false,
            engine: None,
            host: None,
            port: None,
            reachable: None,
            note: "DATABASE_URL unset — filesystem plan only".into(),
            redacted_url: None,
        },
    }
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
        assert_eq!(plan.migrations[0].order, Some(1));
        assert!(plan.total_bytes > 0);
    }

    #[test]
    fn natural_migration_order_not_lexicographic() {
        let dir = tempfile::tempdir().unwrap();
        let mig = dir.path().join("migrations");
        fs::create_dir_all(&mig).unwrap();
        for name in ["10_ten.sql", "2_two.sql", "001_first.sql"] {
            fs::write(mig.join(name), "select 1;").unwrap();
        }
        let plan = plan_migrations(dir.path(), "migrations").unwrap();
        let names: Vec<&str> = plan
            .migrations
            .iter()
            .map(|m| m.path.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(names, ["001_first.sql", "2_two.sql", "10_ten.sql"]);
    }

    #[test]
    fn parses_postgres_url_host_port() {
        assert_eq!(
            parse_database_url("postgres://u:p@127.0.0.1:5433/app")
                .unwrap()
                .host
                .as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            parse_database_url("postgresql://localhost/db")
                .unwrap()
                .port,
            Some(5432)
        );
        assert_eq!(
            parse_database_url("mysql://root@db.internal/app")
                .unwrap()
                .port,
            Some(3306)
        );
        assert_eq!(
            parse_database_url("redis://cache:6380/0").unwrap().port,
            Some(6380)
        );
    }

    #[test]
    fn parses_sqlite_and_redacts_passwords() {
        let parsed = parse_database_url("sqlite:///var/lib/app.db").unwrap();
        assert_eq!(parsed.engine, DbEngine::Sqlite);
        assert_eq!(parsed.database.as_deref(), Some("/var/lib/app.db"));
        assert_eq!(
            redact_database_url("postgres://alice:hunter2@db:5432/app?sslmode=require"),
            "postgres://alice:***@db:5432/app?sslmode=require"
        );
        assert_eq!(
            redact_database_url("mysql://root@db/app?password=hunter2&x=1"),
            "mysql://root@db/app?password=***&x=1"
        );
        assert_eq!(
            redact_database_url("postgres://u:p@h/db?token=abc"),
            "postgres://u:***@h/db?token=***"
        );
    }

    #[test]
    fn probe_bad_url() {
        let r = probe_database_url("not-a-url", 50);
        assert!(r.host.is_none());
        assert!(r.note.contains("could not parse"));
        assert_eq!(r.redacted_url.as_deref(), Some("not-a-url"));
    }

    #[test]
    fn probe_sqlite_is_offline() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.db");
        fs::write(&db, b"").unwrap();
        let url = format!("sqlite://{}", db.display());
        let r = probe_database_url(&url, 10);
        assert_eq!(r.engine.as_deref(), Some("sqlite"));
        assert!(!r.note.contains("TCP"));
        assert!(r.reachable.is_none());
    }

    #[test]
    fn ledger_detects_add_modify_remove_and_reorder() {
        let dir = tempfile::tempdir().unwrap();
        let mig = dir.path().join("migrations");
        fs::create_dir_all(&mig).unwrap();
        fs::write(mig.join("1_a.sql"), "create table a(id int);").unwrap();
        fs::write(mig.join("2_b.sql"), "create table b(id int);").unwrap();
        let plan = plan_migrations(dir.path(), "migrations").unwrap();
        write_ledger(dir.path(), &plan).unwrap();
        assert!(verify_migrations(dir.path(), "migrations").unwrap().clean);

        fs::write(mig.join("3_c.sql"), "create table c(id int);").unwrap();
        fs::write(mig.join("1_a.sql"), "create table a(id bigint);").unwrap();
        fs::remove_file(mig.join("2_b.sql")).unwrap();
        let drift = verify_migrations(dir.path(), "migrations").unwrap();
        assert!(!drift.clean);
        assert!(drift.added.iter().any(|p| p.contains("3_c.sql")));
        assert!(drift.modified.iter().any(|p| p.contains("1_a.sql")));
        assert!(drift.removed.iter().any(|p| p.contains("2_b.sql")));
    }

    #[test]
    fn analysis_ignores_commented_and_quoted_drops() {
        let sql = "-- drop table users;\nselect 'drop table users';\n/* delete from x */\ncreate table t(id int);";
        let a = analyze_migration(Path::new("001.sql"), sql);
        assert!(a.destructive.is_empty());
        assert_eq!(a.statements, 2);
    }

    #[test]
    fn analysis_flags_destructive_and_non_transactional() {
        let sql = "drop table users; delete from accounts; update x set y = 1; create index concurrently idx on t(x);";
        let a = analyze_migration(Path::new("002.sql"), sql);
        assert_eq!(a.destructive.len(), 3);
        assert_eq!(a.non_transactional.len(), 1);
    }

    #[test]
    fn dollar_quoted_bodies_do_not_split_or_flag() {
        let sql = "CREATE FUNCTION bump() RETURNS trigger AS $$
BEGIN
  DELETE FROM audit_old; -- inside the body
  DROP TABLE IF EXISTS scratch;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TABLE t (id int);";
        let a = analyze_migration(Path::new("x.sql"), sql);
        assert_eq!(a.statements, 2, "body must not split: {:?}", a.statements);
        assert!(
            a.destructive.is_empty(),
            "body contents must not be flagged: {:?}",
            a.destructive
        );
    }

    #[test]
    fn quoted_identifiers_with_semicolons_stay_whole() {
        let sql = "CREATE TABLE \"we;ird\" (id int);\nSELECT 1;";
        let a = analyze_migration(Path::new("x.sql"), sql);
        assert_eq!(a.statements, 2);
    }

    #[test]
    fn unicode_sql_survives_scrubbing() {
        let sql = "INSERT INTO notes (text) VALUES ('café'); -- naïve comment";
        let a = analyze_migration(Path::new("x.sql"), sql);
        assert_eq!(a.statements, 1);
        assert!(a.destructive.is_empty());
    }

    #[test]
    fn missing_ledger_is_reported_not_silently_clean() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("migrations")).unwrap();
        fs::write(dir.path().join("migrations/1_a.sql"), "select 1;").unwrap();
        let drift = verify_migrations(dir.path(), "migrations").unwrap();
        assert!(!drift.clean);
        assert!(drift
            .notes
            .iter()
            .any(|n| n.contains("no migration ledger")));
    }
}
