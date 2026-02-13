//! Bounded HTTP client: connection reuse via a single Agent, capped concurrency.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Condvar, Mutex};

const REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_CONCURRENCY: usize = 16;
const MAX_CONCURRENCY_CAP: usize = 32;
const DEFAULT_RETRY_COUNT: usize = 2;
const DEFAULT_RETRY_BACKOFF_MS: u64 = 250;

fn concurrency_from_env() -> usize {
    std::env::var("JHOL_NETWORK_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, MAX_CONCURRENCY_CAP))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 2).clamp(4, MAX_CONCURRENCY_CAP))
                .unwrap_or(DEFAULT_CONCURRENCY)
        })
}

fn retry_count_from_env() -> usize {
    std::env::var("JHOL_HTTP_RETRIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_RETRY_COUNT)
}

fn retry_backoff_ms_from_env() -> u64 {
    std::env::var("JHOL_HTTP_RETRY_BACKOFF_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETRY_BACKOFF_MS)
}

/// Semaphore-style limit: wait until a slot is free, then hold until guard is dropped.
struct ConcurrencyLimit {
    mutex: Mutex<usize>,
    condvar: Condvar,
    max: usize,
}

impl ConcurrencyLimit {
    fn new(max: usize) -> Self {
        Self {
            mutex: Mutex::new(0),
            condvar: Condvar::new(),
            max,
        }
    }

    fn acquire(&self) -> ConcurrencyGuard<'_> {
        let mut guard = self.mutex.lock().unwrap();
        while *guard >= self.max {
            guard = self.condvar.wait(guard).unwrap();
        }
        *guard += 1;
        ConcurrencyGuard(self)
    }
}

struct ConcurrencyGuard<'a>(&'a ConcurrencyLimit);

impl Drop for ConcurrencyGuard<'_> {
    fn drop(&mut self) {
        let mut guard = self.0.mutex.lock().unwrap();
        *guard = guard.saturating_sub(1);
        self.0.condvar.notify_one();
    }
}

/// HTTP client: one Agent (connection reuse), bounded concurrent requests.
pub struct HttpClient {
    agent: ureq::Agent,
    limit: ConcurrencyLimit,
}

impl HttpClient {
    pub fn new(max_concurrent: usize) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
            .build();
        Self {
            agent,
            limit: ConcurrencyLimit::new(max_concurrent),
        }
    }

    /// GET url and return body bytes.
    pub fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.get_with_accept(url, None)
    }

    /// GET url with optional Accept header (e.g. for abbreviated packument).
    pub fn get_with_accept(&self, url: &str, accept: Option<&str>) -> Result<Vec<u8>, String> {
        let _guard = self.limit.acquire();
        let resp = self.send_with_retry(|| {
            let req = self.agent.get(url);
            match accept {
                Some(h) => req.set("Accept", h).call(),
                None => req.call(),
            }
        })?;
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    /// POST body to url (e.g. JSON). Content-Type: application/json.
    pub fn post_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, String> {
        let _guard = self.limit.acquire();
        let resp = self.send_with_retry(|| {
            self.agent
                .post(url)
                .set("Content-Type", "application/json")
                .send_bytes(body)
        })?;
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    /// GET url and write body to file (for tarballs).
    pub fn get_to_file(&self, url: &str, dest: &Path) -> Result<(), String> {
        let _guard = self.limit.acquire();
        let resp = self.send_with_retry(|| self.agent.get(url).call())?;
        let mut out = File::create(dest).map_err(|e| e.to_string())?;
        let mut reader = resp.into_reader();
        std::io::copy(&mut reader, &mut out).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn send_with_retry<F>(&self, mut send: F) -> Result<ureq::Response, String>
    where
        F: FnMut() -> Result<ureq::Response, ureq::Error>,
    {
        let retries = retry_count_from_env();
        let mut attempt = 0usize;
        let mut backoff = retry_backoff_ms_from_env();
        loop {
            attempt += 1;
            match send() {
                Ok(resp) => {
                    if resp.status() == 200 {
                        return Ok(resp);
                    }
                    let status = resp.status();
                    if attempt <= retries && (status >= 500 || status == 429) {
                        std::thread::sleep(std::time::Duration::from_millis(backoff));
                        backoff = backoff.saturating_mul(2).min(5_000);
                        continue;
                    }
                    return Err(format!("HTTP {}", status));
                }
                Err(ureq::Error::Status(code, _resp)) => {
                    if attempt <= retries && (code >= 500 || code == 429) {
                        std::thread::sleep(std::time::Duration::from_millis(backoff));
                        backoff = backoff.saturating_mul(2).min(5_000);
                        continue;
                    }
                    return Err(format!("HTTP {}", code));
                }
                Err(e) => {
                    if attempt <= retries {
                        std::thread::sleep(std::time::Duration::from_millis(backoff));
                        backoff = backoff.saturating_mul(2).min(5_000);
                        continue;
                    }
                    return Err(e.to_string());
                }
            }
        }
    }
}

static CLIENT: std::sync::OnceLock<HttpClient> = std::sync::OnceLock::new();

fn get_global() -> &'static HttpClient {
    CLIENT.get_or_init(|| HttpClient::new(concurrency_from_env()))
}

/// GET url and return body (uses global bounded client).
pub fn get(url: &str) -> Result<Vec<u8>, String> {
    get_global().get(url)
}

/// GET url with optional Accept header (e.g. application/vnd.npm.install-v1+json for abbreviated packument).
pub fn get_with_accept(url: &str, accept: Option<&str>) -> Result<Vec<u8>, String> {
    get_global().get_with_accept(url, accept)
}

/// GET url and write to file (uses global bounded client).
pub fn get_to_file(url: &str, dest: &Path) -> Result<(), String> {
    get_global().get_to_file(url, dest)
}

/// POST JSON body to url (uses global bounded client).
pub fn post_json(url: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    get_global().post_json(url, body)
}
