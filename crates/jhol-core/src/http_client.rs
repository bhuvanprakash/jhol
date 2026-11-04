//! Bounded HTTP client: connection reuse via a single Agent, capped concurrency.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_CONCURRENCY: usize = 64; // Increased from 32 to 64
const MAX_CONCURRENCY_CAP: usize = 128; // Increased from 64 to 128
const DEFAULT_RETRY_COUNT: usize = 2;
const DEFAULT_RETRY_BACKOFF_MS: u64 = 250;
#[allow(dead_code)]
const TCP_KEEPALIVE_SECS: u64 = 30; // Added TCP keepalive
#[allow(dead_code)]
const IDLE_TIMEOUT_SECS: u64 = 90; // Added idle timeout for connection reuse
const MAX_IDLE_PER_HOST: usize = 32; // Added max idle connections per host

fn concurrency_from_env() -> usize {
    std::env::var("JHOL_NETWORK_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, MAX_CONCURRENCY_CAP))
        .unwrap_or_else(|| {
            // Network is latency-bound, not CPU-bound: allow more in-flight
            // requests than cores (4x, clamped) so a round of packument fetches
            // completes in roughly one RTT.
            std::thread::available_parallelism()
                .map(|n| {
                    let cores = n.get();
                    // Small machines: 2x cores (a 10-core laptop gets 20, not 40)
                    // so an install does not fight itself for memory and threads.
                    // Big machines keep the generous latency-hiding window.
                    if cores <= 8 {
                        (cores * 2).clamp(4, 16)
                    } else {
                        (cores * 4).clamp(16, 32)
                    }
                })
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

fn request_timeout_ms() -> u64 {
    std::env::var("JHOL_FETCH_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|s| s.saturating_mul(1000))
        .unwrap_or(REQUEST_TIMEOUT_MS)
}

/// Semaphore-style limit: wait until a slot is free, then hold until guard is dropped.
///
/// The cap is *adaptive*: the registry throttles aggressively when too many
/// large transfers are in flight (observed aggregate throughput collapse and
/// multi-second stalls beyond ~6 concurrent downloads), so a 429/5xx response
/// halves the in-flight cap, and each success lets it creep back toward the
/// configured maximum. This is AIMD for the HTTP plane.
struct ConcurrencyLimit {
    mutex: Mutex<usize>,
    condvar: Condvar,
    max: AtomicUsize,
    configured_max: usize,
}

impl ConcurrencyLimit {
    fn new(max: usize) -> Self {
        Self {
            mutex: Mutex::new(0),
            condvar: Condvar::new(),
            max: AtomicUsize::new(max),
            configured_max: max,
        }
    }

    fn acquire(&self) -> ConcurrencyGuard<'_> {
        let mut guard = self
            .mutex
            .lock()
            .expect("concurrency_limit: mutex lock failed (poisoned)");
        while *guard >= self.max.load(Ordering::Relaxed) {
            guard = self
                .condvar
                .wait(guard)
                .expect("concurrency_limit: condition wait failed (poisoned)");
        }
        *guard += 1;
        ConcurrencyGuard(self)
    }

    /// Throttling detected (429/5xx or a connection error): halve the in-flight
    /// cap, never below 2.
    fn back_off(&self) {
        let cur = self.max.load(Ordering::Relaxed);
        let new = (cur / 2).max(2);
        if new < cur {
            self.max.store(new, Ordering::Relaxed);
        }
    }

    /// A successful request: allow one more in-flight request, up to the
    /// configured maximum.
    fn recover(&self) {
        let cur = self.max.load(Ordering::Relaxed);
        if cur < self.configured_max {
            // Proportional recovery. Additive `+1` cannot recover from a halving
            // within one install: the whole budget is 30-60 requests, so the
            // window collapsed toward the floor and stayed there. That is why
            // raising JHOL_DOWNLOAD_CONCURRENCY measured almost no effect while
            // 30 tarballs totalling 1.1 MB took ~950 ms at ~4-way effective
            // parallelism.
            let step = (cur / 8).max(1);
            let _ = self.max.compare_exchange(
                cur,
                (cur + step).min(self.configured_max),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }
}

struct ConcurrencyGuard<'a>(&'a ConcurrencyLimit);

impl Drop for ConcurrencyGuard<'_> {
    fn drop(&mut self) {
        let mut guard = self
            .0
            .mutex
            .lock()
            .expect("concurrency_limit: mutex lock failed (poisoned) in drop");
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
        self.bytes_downloaded
            .fetch_add(bytes_down, Ordering::Relaxed);
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
        // key: reuse many connections per host. ureq defaults
        // `max_idle_connections_per_host` to 1, so with N concurrent requests
        // only one connection was kept alive and every later round (the resolver
        // fetches packuments in dependency-level rounds) had to re-open ~N
        // connections, paying a TLS handshake each time. Keep a pool instead.
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_millis(request_timeout_ms()))
            .max_idle_connections(MAX_IDLE_PER_HOST * 2)
            .max_idle_connections_per_host(MAX_IDLE_PER_HOST)
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

    /// PUT a JSON body with an optional bearer token (npm publish protocol:
    /// `PUT {registry}/{name}` with `Authorization: Bearer <token>`).
    /// Accepts any 2xx; the response body is returned for the caller to
    /// interpret (`{"ok":true}` on success).
    pub fn put_json_with_bearer(
        &self,
        url: &str,
        body: &[u8],
        token: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let _guard = self.limit.acquire();
        let resp = self.send_with_retry(|| {
            let mut req = self
                .agent
                .put(url)
                .set("Content-Type", "application/json")
                .set("Accept", "application/json");
            if let Some(t) = token {
                if !t.is_empty() {
                    req = req.set("Authorization", &format!("Bearer {}", t));
                }
            }
            req.send_bytes(body)
        })?;
        let status = resp.status();
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) {
            let msg = String::from_utf8_lossy(&buf);
            return Err(format!(
                "registry rejected publish (HTTP {}): {}",
                status,
                msg.chars().take(400).collect::<String>()
            ));
        }
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
        if std::env::var("JHOL_HTTP_METRICS").is_ok() {
            eprintln!("[HTTP] {}", url);
        }
        let auth_header = bearer_token
            .filter(|t| !t.is_empty())
            .map(|t| format!("Bearer {}", t));
        let t0 = std::time::Instant::now();
        let resp = match self.send_with_retry(|| {
            let req = self.agent.get(url);
            match &auth_header {
                Some(h) => req.set("Authorization", h).call(),
                None => req.call(),
            }
        }) {
            Ok(r) => r,
            Err(e) => {
                self.metrics.record_request(false, 0, 0);
                return Err(e);
            }
        };
        let ttfb = t0.elapsed();
        let hint = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut out = File::create(dest).map_err(|e| e.to_string())?;
        let mut reader = resp.into_reader();
        let n = std::io::copy(&mut reader, &mut out).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        if hint > 0 && (n as usize) < hint {
            let _ = std::fs::remove_file(dest);
            return Err(format!(
                "truncated download for {}: expected {} bytes, got {}",
                url, hint, n
            ));
        }
        trace_request(url, t0, ttfb, hint, n as usize);
        self.metrics.record_request(true, n, 0);
        Ok(())
    }

    /// Consume a bearer-authenticated response body while its pooled request
    /// permit is held. The consumer must read to EOF before returning success.
    pub fn get_with_bearer_reader<T, F>(
        &self,
        url: &str,
        bearer_token: Option<&str>,
        mut consume: F,
    ) -> Result<T, String>
    where
        F: FnMut(&mut dyn Read) -> Result<T, String>,
    {
        let _guard = self.limit.acquire();
        let auth_header = bearer_token
            .filter(|t| !t.is_empty())
            .map(|t| format!("Bearer {}", t));
        let t0 = std::time::Instant::now();
        let response = self.send_with_retry(|| {
            let request = self.agent.get(url);
            match &auth_header {
                Some(header) => request.set("Authorization", header).call(),
                None => request.call(),
            }
        })?;
        let ttfb = t0.elapsed();
        let hint = response
            .header("Content-Length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut reader = response.into_reader();
        let mut counted = CountedBody {
            inner: &mut reader,
            bytes_read: 0,
            expected: (hint > 0).then_some(hint as u64),
        };
        let result = consume(&mut counted);
        let bytes_read = counted.bytes_read;
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.metrics.record_request(false, bytes_read, 0);
                return Err(error);
            }
        };
        if hint > 0 && (bytes_read as usize) < hint {
            self.metrics.record_request(false, bytes_read, 0);
            return Err(format!(
                "truncated download for {}: expected {} bytes, got {}",
                url, hint, bytes_read
            ));
        }
        trace_request(url, t0, ttfb, hint, bytes_read as usize);
        self.metrics.record_request(true, bytes_read, 0);
        Ok(value)
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
        // Timed acquire. If this blocks, the concurrency limiter is throttling the
        // download phase - a cost that is invisible in wall time alone.
        let t_wait = std::time::Instant::now();
        let _guard = self.limit.acquire();
        let permit_wait_ms = t_wait.elapsed().as_millis();
        let t_req = std::time::Instant::now();
        let auth_header = bearer_token
            .filter(|t| !t.is_empty())
            .map(|t| format!("Bearer {}", t));
        let resp = self.send_with_retry(|| {
            let req = self.agent.get(url);
            match &auth_header {
                Some(h) => req.set("Authorization", h).call(),
                None => req.call(),
            }
        })?;
        // Pre-allocate from Content-Length if present to reduce reallocs.
        let hint = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut buf = Vec::with_capacity(crate::utils::clamp_prealloc(if hint > 0 {
            hint
        } else {
            256 * 1024
        }));
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        if hint > 0 && buf.len() < hint {
            return Err(format!(
                "truncated response for {}: expected {} bytes, got {}",
                url,
                hint,
                buf.len()
            ));
        }
        // The tarball path never reached `trace_request` before, despite that
        // function's doc comment claiming it covered tarballs - so a
        // lockfile-driven cold install (which is *only* tarballs) printed zero
        // timing lines and every diagnosis of it was blind. Measured here.
        if std::env::var("JHOL_HTTP_TIMING").is_ok() {
            eprintln!(
                "[TB-T] total {:>7}ms  wait {:>6}ms  {:>10}B  {}",
                t_req.elapsed().as_millis(),
                permit_wait_ms,
                buf.len(),
                url
            );
        }
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
        if std::env::var("JHOL_HTTP_METRICS").is_ok() {
            eprintln!("[HTTP] {}", url);
        }
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        let t0 = std::time::Instant::now();
        match req.call() {
            Ok(resp) => {
                let ttfb = t0.elapsed();
                let status = resp.status();
                let etag = resp
                    .header("ETag")
                    .or_else(|| resp.header("etag"))
                    .map(|s| s.to_string());
                let hint = resp
                    .header("Content-Length")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = Vec::with_capacity(crate::utils::clamp_prealloc(if hint > 0 {
                    hint
                } else {
                    256 * 1024
                }));
                resp.into_reader()
                    .read_to_end(&mut body)
                    .map_err(|e| e.to_string())?;
                // `Content-Length` is the on-the-wire (usually gzipped) size while
                // `body` has been decoded, so the pair shows how much gzip saved and
                // whether the cost was latency (ttfb) or transfer.
                if std::env::var("JHOL_HTTP_TIMING").is_ok() {
                    eprintln!(
                        "[HTTP-T] total {:>7}ms  ttfb {:>6}ms  body {:>6}ms  wire {:>9}B  decoded {:>10}B  {}",
                        t0.elapsed().as_millis(),
                        ttfb.as_millis(),
                        t0.elapsed().as_millis().saturating_sub(ttfb.as_millis()),
                        hint,
                        body.len(),
                        url
                    );
                }
                self.metrics.record_request(true, body.len() as u64, 0);
                Ok((status, body, etag))
            }
            Err(ureq::Error::Status(304, resp)) => {
                let etag = resp
                    .header("ETag")
                    .or_else(|| resp.header("etag"))
                    .map(|s| s.to_string());
                self.metrics.record_request(true, 0, 0);
                Ok((304, Vec::new(), etag))
            }
            Err(ureq::Error::Status(code, _)) => {
                self.metrics.record_request(false, 0, 0);
                Err(format!("HTTP {}", code))
            }
            Err(e) => {
                self.metrics.record_request(false, 0, 0);
                Err(e.to_string())
            }
        }
    }

    fn send_with_retry<F>(&self, mut send: F) -> Result<ureq::Response, String>
    where
        F: FnMut() -> Result<ureq::Response, ureq::Error>,
    {
        // Offline misses must fail in one attempt. Measured: offline installs of
        // bundled packages spent ~4 s in retry backoff against a registry that is
        // never reachable in offline mode, per failed package.
        let retries = if std::env::var("JHOL_OFFLINE").is_ok() {
            0
        } else {
            retry_count_from_env()
        };
        let mut attempt = 0usize;
        let mut backoff = retry_backoff_ms_from_env();
        loop {
            attempt += 1;
            match send() {
                Ok(resp) => {
                    // Any 2xx is success: npm's publish answers 201 Created,
                    // and the old exact-200 check reported a successful
                    // publish as a failure.
                    if (200..300).contains(&resp.status()) {
                        self.limit.recover();
                        return Ok(resp);
                    }
                    let status = resp.status();
                    if attempt <= retries && (status >= 500 || status == 429) {
                        // Only a 429 is a congestion signal. A 5xx is a server fault:
                        // retry it, but do not permanently lower our own throughput.
                        if status == 429 {
                            self.limit.back_off();
                        }

                        std::thread::sleep(std::time::Duration::from_millis(backoff));
                        backoff = backoff.saturating_mul(2).min(1_500);
                        continue;
                    }
                    return Err(format!("HTTP {}", status));
                }
                Err(ureq::Error::Status(code, _resp)) => {
                    if attempt <= retries && (code >= 500 || code == 429) {
                        // Only a 429 is a congestion signal. A 5xx is a server fault:
                        // retry it, but do not permanently lower our own throughput.
                        if code == 429 {
                            self.limit.back_off();
                        }

                        std::thread::sleep(std::time::Duration::from_millis(backoff));
                        backoff = backoff.saturating_mul(2).min(1_500);
                        continue;
                    }
                    return Err(format!("HTTP {}", code));
                }
                Err(e) => {
                    if attempt <= retries {
                        self.limit.back_off();
                        std::thread::sleep(std::time::Duration::from_millis(backoff));
                        backoff = backoff.saturating_mul(2).min(1_500);
                        continue;
                    }
                    return Err(e.to_string());
                }
            }
        }
    }
}

struct CountedBody<'a> {
    inner: &'a mut dyn Read,
    bytes_read: u64,
    expected: Option<u64>,
}

impl Read for CountedBody<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        self.bytes_read = self.bytes_read.saturating_add(n as u64);
        if n == 0 && self.expected.is_some_and(|length| self.bytes_read < length) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated response body",
            ));
        }
        Ok(n)
    }
}

static CLIENT: std::sync::OnceLock<HttpClient> = std::sync::OnceLock::new();

#[cfg(test)]
#[test]
fn reader_rejects_short_eof_before_consumer_success() {
    let mut input = &b"body"[..];
    let mut body = CountedBody {
        inner: &mut input,
        bytes_read: 0,
        expected: Some(8),
    };
    assert_eq!(body.read(&mut []).unwrap(), 0);
    let mut bytes = Vec::new();
    assert_eq!(
        body.read_to_end(&mut bytes).unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

fn get_global() -> &'static HttpClient {
    CLIENT.get_or_init(|| HttpClient::new(concurrency_from_env()))
}

/// GET url and return body (uses global bounded client).
pub fn get(url: &str) -> Result<Vec<u8>, String> {
    get_global().get(url)
}

/// GET url with optional Accept header (e.g. application/vnd.npm.install-v1+json for abbreviated packument).
/// Env-gated per-request timing line (`JHOL_HTTP_TIMING=1`).
///
/// One helper for every request path, so a cold-install profile accounts for *all* of the
/// traffic — packuments, manifests and tarballs. Previously only one path printed, which made
/// the network share of a cold install look tiny purely because the other requests were silent.
/// `wire` is the on-the-wire (gzipped) size and `decoded` the body we received, so the pair shows
/// both what compression saved and whether the cost was latency (ttfb) or transfer (body).
fn trace_request(
    url: &str,
    t0: std::time::Instant,
    ttfb: std::time::Duration,
    wire: usize,
    decoded: usize,
) {
    if std::env::var("JHOL_HTTP_TIMING").is_ok() {
        eprintln!(
            "[HTTP-T] total {:>7}ms  ttfb {:>6}ms  body {:>6}ms  wire {:>9}B  decoded {:>10}B  {}",
            t0.elapsed().as_millis(),
            ttfb.as_millis(),
            t0.elapsed().as_millis().saturating_sub(ttfb.as_millis()),
            wire,
            decoded,
            url
        );
    }
}

pub fn get_with_accept(url: &str, accept: Option<&str>) -> Result<Vec<u8>, String> {
    #[cfg(feature = "http-reqwest")]
    {
        if std::env::var("JHOL_NO_HTTP2").is_err() {
            let headers: Vec<(&str, &str)> = match accept {
                Some(acc) => vec![("Accept", acc)],
                None => vec![],
            };
            let r = crate::http2_download::download_with_headers(url, &headers);
            if let Ok(b) = r {
                return Ok(b);
            }
        }
    }
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

/// Prefer the pooled HTTP/2 path for large streamed downloads, then fall back
/// to the bounded HTTP/1.1 client if HTTP/2 fails.
pub fn get_to_file_with_bearer_streaming(
    url: &str,
    dest: &Path,
    bearer_token: Option<&str>,
) -> Result<(), String> {
    #[cfg(feature = "http-reqwest")]
    if std::env::var("JHOL_NO_HTTP2").is_err() {
        let auth = bearer_token
            .filter(|token| !token.is_empty())
            .map(|token| format!("Bearer {token}"));
        let headers = auth
            .as_deref()
            .map(|value| vec![("Authorization", value)])
            .unwrap_or_default();
        if crate::http2_download::download_to_file_with_headers(url, dest, &headers).is_ok() {
            return Ok(());
        }
    }

    get_global().get_to_file_with_bearer(url, dest, bearer_token)
}

/// Stream a response body into a caller-owned pipeline, preferring the pooled
/// HTTP/2 transport and falling back to the bounded HTTP/1.1 client. The
/// callback can be invoked twice if the HTTP/2 attempt fails, so it must reset
/// any private staging output before each attempt and publish only after success.
pub fn get_with_bearer_streaming<T, F>(
    url: &str,
    bearer_token: Option<&str>,
    // `mut` is used by the `http-reqwest` branch below; clippy --fix, run
    // without that feature active, strips it and breaks the release build.
    #[allow(unused_mut)] mut consume: F,
) -> Result<T, String>
where
    F: FnMut(&mut dyn Read) -> Result<T, String>,
{
    #[cfg(feature = "http-reqwest")]
    if std::env::var("JHOL_NO_HTTP2").is_err() {
        let auth = bearer_token
            .filter(|token| !token.is_empty())
            .map(|token| format!("Bearer {token}"));
        let headers = auth
            .as_deref()
            .map(|value| vec![("Authorization", value)])
            .unwrap_or_default();
        if let Ok(value) =
            crate::http2_download::download_with_headers_reader(url, &headers, &mut consume)
        {
            return Ok(value);
        }
    }

    get_global().get_with_bearer_reader(url, bearer_token, consume)
}

/// GET url with optional bearer token, return body as bytes (for one-pass hash+extract).
///
/// Routed through the HTTP/2 client pool when the `http-reqwest` feature is on.
///
/// Measured reason: on 30 real tarballs a reference client did 533 ms over 30 h1.1
/// connections but 245 ms over ONE multiplexed h2 connection (2.2x). jhol's own
/// instrumentation agrees from the other side - at `JHOL_DOWNLOAD_CONCURRENCY=1` a
/// single reused connection served 25 requests at 47 ms each, while at `conc>=4`
/// every request cost 400-670 ms because concurrent *new* connections get
/// throttled. HTTP/2 keeps the low per-request cost of one connection *and*
/// overlaps the requests, which is exactly what h1.1 could not do.
///
/// `JHOL_NO_HTTP2=1` forces the old h1.1/ureq path, so both can be compared in one
/// binary without rebuilding.
pub fn get_bytes_with_bearer(url: &str, bearer_token: Option<&str>) -> Result<Vec<u8>, String> {
    #[cfg(feature = "http-reqwest")]
    {
        if std::env::var("JHOL_NO_HTTP2").is_err() {
            let t0 = std::time::Instant::now();
            let r = match bearer_token.filter(|t| !t.is_empty()) {
                Some(t) => {
                    let auth = format!("Bearer {}", t);
                    crate::http2_download::download_with_headers(
                        url,
                        &[("Authorization", auth.as_str())],
                    )
                }
                None => crate::http2_download::download_to_bytes(url),
            };
            if std::env::var("JHOL_HTTP_TIMING").is_ok() {
                match &r {
                    Ok(b) => eprintln!(
                        "[TB-T] total {:>7}ms  wait {:>6}ms  {:>10}B  {}  (h2)",
                        t0.elapsed().as_millis(),
                        0,
                        b.len(),
                        url
                    ),
                    Err(e) => eprintln!("[TB-T] h2 FAILED {}: {}", url, e),
                }
            }
            // Any h2 failure falls back to h1.1 rather than failing the install:
            // a proxy or firewall that blocks h2 must not break downloads.
            if r.is_ok() {
                return r;
            }
        }
    }
    get_global().get_bytes_with_bearer(url, bearer_token)
}

/// GET url with custom request headers. Returns (status, body, etag).
/// 304 is returned as-is (empty body); caller handles cached response.
/// Uses the global shared Agent — all threads reuse TCP connections.
pub fn get_raw_with_headers(
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(u16, Vec<u8>, Option<String>), String> {
    #[cfg(feature = "http-reqwest")]
    {
        if std::env::var("JHOL_NO_HTTP2").is_err() {
            let r = crate::http2_download::download_raw_with_headers(url, headers);
            if let Ok(res) = r {
                return Ok(res);
            }
        }
    }
    get_global().get_raw_with_headers(url, headers)
}

/// POST JSON body to url (uses global bounded client).
pub fn post_json(url: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    get_global().post_json(url, body)
}

/// Eagerly prewarm the HTTP connection pool (starts TLS/H2 handshake before graph expansion).
pub fn prewarm_network() {
    #[cfg(feature = "http-reqwest")]
    {
        if std::env::var("JHOL_NO_HTTP2").is_err() {
            crate::http2_download::prewarm();
        }
    }
}

/// Get HTTP metrics for monitoring and debugging
pub fn get_http_metrics() -> (u64, u64, u64, u64, u64) {
    // `mut` is required by the `http-reqwest` branch below; clippy --fix, run
    // without that feature active, strips it and breaks the release build.
    #[allow(unused_mut)]
    let (mut total, mut success, mut failed, mut bytes_down, bytes_up) =
        get_global().metrics.get_stats();
    #[cfg(feature = "http-reqwest")]
    if let Some(h2) = crate::http2_download::get_metrics_if_initialized() {
        total = total.saturating_add(h2.requests_total as u64);
        success = success.saturating_add(h2.requests_success as u64);
        failed = failed.saturating_add(h2.requests_failed as u64);
        bytes_down = bytes_down.saturating_add(h2.bytes_downloaded as u64);
    }
    (total, success, failed, bytes_down, bytes_up)
}

/// Print HTTP metrics to stdout
pub fn print_http_metrics() {
    let (total, success, failed, bytes_down, bytes_up) = get_http_metrics();
    println!("HTTP Metrics:");
    println!("  Total requests: {}", total);
    println!("  Successful: {}", success);
    println!("  Failed: {}", failed);
    println!(
        "  Success rate: {:.2}%",
        if total > 0 {
            (success as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    );
    println!("  Bytes downloaded: {} bytes", bytes_down);
    println!("  Bytes uploaded: {} bytes", bytes_up);
}

/// Optional CDN / ESM-style fetch helpers (was `cdn.rs`).
pub mod cdn {
    //! Optional CDN / ESM-style fetch: produce esm.sh URLs for packages (no full install).
    //! Does not replace the main install path; no native addons or full npm semantics.

    /// Build esm.sh URL for a package (e.g. lodash@4 or lodash -> https://esm.sh/lodash@4 or latest).
    pub fn esm_sh_url(package: &str, version: Option<&str>) -> String {
        const ESM_SH: &str = "https://esm.sh";
        let spec = match version {
            Some(v) => format!("{}@{}", package.trim(), v.trim()),
            None => package.trim().to_string(),
        };
        format!("{}/{}", ESM_SH.trim_end_matches('/'), spec)
    }

    /// Fetch ESM bundle from URL to a file (e.g. for one-off scripts). Uses http_client.
    pub fn fetch_esm_to_file(url: &str, dest: &std::path::Path) -> Result<(), String> {
        crate::http_client::get_to_file(url, dest).map(|_| ())
    }
}
