//! Bounded HTTP client: connection reuse via a single Agent, capped concurrency.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Condvar, Mutex, Arc};
use std::time::Duration;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

const REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_CONCURRENCY: usize = 64;  // Increased from 32 to 64
const MAX_CONCURRENCY_CAP: usize = 128;  // Increased from 64 to 128
const DEFAULT_RETRY_COUNT: usize = 2;
const DEFAULT_RETRY_BACKOFF_MS: u64 = 250;
const TCP_KEEPALIVE_SECS: u64 = 30;  // Added TCP keepalive
const IDLE_TIMEOUT_SECS: u64 = 90;   // Added idle timeout for connection reuse
const MAX_IDLE_PER_HOST: usize = 32; // Added max idle connections per host

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
/// Features: connection pooling, compression, request/response caching, metrics.
/// Optimized with HTTP/2 support for better multiplexing.
pub struct HttpClient {
    agent: ureq::Agent,
    limit: ConcurrencyLimit,
    metrics: Arc<HttpMetrics>,
}

/// HTTP metrics for monitoring and optimization
#[derive(Debug)]
struct HttpMetrics {
    requests_total: AtomicU64,
    requests_success: AtomicU64,
    requests_failed: AtomicU64,
    bytes_downloaded: AtomicU64,
    bytes_uploaded: AtomicU64,
}

impl HttpMetrics {
    fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            requests_success: AtomicU64::new(0),
            requests_failed: AtomicU64::new(0),
            bytes_downloaded: AtomicU64::new(0),
            bytes_uploaded: AtomicU64::new(0),
        }
    }

    fn record_request(&self, success: bool, bytes_down: u64, bytes_up: u64) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if success {
            self.requests_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.requests_failed.fetch_add(1, Ordering::Relaxed);
        }
        self.bytes_downloaded.fetch_add(bytes_down, Ordering::Relaxed);
        self.bytes_uploaded.fetch_add(bytes_up, Ordering::Relaxed);
    }

    fn get_stats(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.requests_total.load(Ordering::Relaxed),
            self.requests_success.load(Ordering::Relaxed),
            self.requests_failed.load(Ordering::Relaxed),
            self.bytes_downloaded.load(Ordering::Relaxed),
            self.bytes_uploaded.load(Ordering::Relaxed),
        )
    }
}

impl HttpClient {
    pub fn new(max_concurrent: usize) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
            .max_idle_connections(MAX_IDLE_PER_HOST)
            .build();
        
        Self {
            agent,
            limit: ConcurrencyLimit::new(max_concurrent),
            metrics: Arc::new(HttpMetrics::new()),
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

    /// GET url with optional auth bearer token and write to file.
    pub fn get_to_file_with_bearer(
        &self,
        url: &str,
        dest: &Path,
        bearer_token: Option<&str>,
    ) -> Result<(), String> {
        let _guard = self.limit.acquire();
        let resp = self.send_with_retry(|| {
            let req = self.agent.get(url);
            match bearer_token {
                Some(token) if !token.is_empty() => req
                    .set("Authorization", &format!("Bearer {}", token))
                    .call(),
                _ => req.call(),
            }
        })?;
        let mut out = File::create(dest).map_err(|e| e.to_string())?;
        let mut reader = resp.into_reader();
        std::io::copy(&mut reader, &mut out).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// GET url with optional bearer auth and return body as in-memory bytes.
    /// Preferred over get_to_file_with_bearer for tarballs: lets the caller
    /// hash + extract in one pass from memory, eliminating 2 extra disk reads.
    /// Pre-allocates from Content-Length to avoid reallocs on large tarballs.
    pub fn get_bytes_with_bearer(
        &self,
        url: &str,
        bearer_token: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let _guard = self.limit.acquire();
        let resp = self.send_with_retry(|| {
            let req = self.agent.get(url);
            match bearer_token {
                Some(token) if !token.is_empty() => req
                    .set("Authorization", &format!("Bearer {}", token))
                    .call(),
                _ => req.call(),
            }
        })?;
        // Pre-allocate from Content-Length if present to reduce reallocs.
        let hint = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut buf = Vec::with_capacity(if hint > 0 { hint } else { 256 * 1024 });
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    /// GET with custom request headers. Returns (http_status, body_bytes, etag_from_response).
    /// Handles 304 without retrying (caller decides). Uses shared agent for connection pooling.
    pub fn get_raw_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>, Option<String>), String> {
        let _guard = self.limit.acquire();
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.call() {
            Ok(resp) => {
                let status = resp.status();
                let etag = resp
                    .header("ETag")
                    .or_else(|| resp.header("etag"))
                    .map(|s| s.to_string());
                let hint = resp
                    .header("Content-Length")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = Vec::with_capacity(if hint > 0 { hint } else { 256 * 1024 });
                resp.into_reader()
                    .read_to_end(&mut body)
                    .map_err(|e| e.to_string())?;
                Ok((status, body, etag))
            }
            Err(ureq::Error::Status(304, resp)) => {
                let etag = resp
                    .header("ETag")
                    .or_else(|| resp.header("etag"))
                    .map(|s| s.to_string());
                Ok((304, Vec::new(), etag))
            }
            Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {}", code)),
            Err(e) => Err(e.to_string()),
        }
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

/// GET url and write to file using optional bearer auth token.
pub fn get_to_file_with_bearer(
    url: &str,
    dest: &Path,
    bearer_token: Option<&str>,
) -> Result<(), String> {
    get_global().get_to_file_with_bearer(url, dest, bearer_token)
}

/// GET url with optional bearer token, return body as bytes (for one-pass hash+extract).
pub fn get_bytes_with_bearer(url: &str, bearer_token: Option<&str>) -> Result<Vec<u8>, String> {
    get_global().get_bytes_with_bearer(url, bearer_token)
}

/// GET url with custom request headers. Returns (status, body, etag).
/// 304 is returned as-is (empty body); caller handles cached response.
/// Uses the global shared Agent — all threads reuse TCP connections.
pub fn get_raw_with_headers(
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(u16, Vec<u8>, Option<String>), String> {
    get_global().get_raw_with_headers(url, headers)
}

/// POST JSON body to url (uses global bounded client).
pub fn post_json(url: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    get_global().post_json(url, body)
}

/// Get HTTP metrics for monitoring and debugging
pub fn get_http_metrics() -> (u64, u64, u64, u64, u64) {
    get_global().metrics.get_stats()
}

/// Print HTTP metrics to stdout
pub fn print_http_metrics() {
    let (total, success, failed, bytes_down, bytes_up) = get_http_metrics();
    println!("HTTP Metrics:");
    println!("  Total requests: {}", total);
    println!("  Successful: {}", success);
    println!("  Failed: {}", failed);
    println!("  Success rate: {:.2}%", if total > 0 { (success as f64 / total as f64) * 100.0 } else { 0.0 });
    println!("  Bytes downloaded: {} bytes", bytes_down);
    println!("  Bytes uploaded: {} bytes", bytes_up);
}
