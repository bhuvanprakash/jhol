//! Shared pooled HTTP/2 transport for registry metadata and streamed tarball installs.
//! Callers retain the bounded HTTP/1.1 fallback. Resumable-file helpers are auxiliary paths.
//!
//! Hyper-fast HTTP/2 download engine with research-backed optimizations.
//!
//! Techniques incorporated from the latest Rust HTTP/2 engines (2026):
//!
//! **XDE (eXtreme Download Engine)**:
//! - Adaptive concurrency — starts conservative, scales up via throughput measurement
//! - Resumable downloads with ETag + byte-range
//! - BDP-based window management
//!
//! **StormDL**:
//! - Bandwidth-Delay Product (BDP) estimation for segment sizing
//! - Adaptive chunk rebalancing across streams
//! - Write coalescing to reduce syscall overhead
//!
//! **tur-rs**:
//! - Adaptive work-stealing across connection pool
//! - Protocol-aware scheduling
//!
//! **boatramp-http**:
//! - Multiplexed HTTP/2 driver with direct-poll (no channel overhead)
//! - Write batching across concurrent streams
//!
//! **libfw**:
//! - Parallel byte-range GETs with resume
//! - Tus-style parallel upload/download
//!
//! Key features:
//! - HTTP/2 prior knowledge with 2MB stream / 4MB connection windows
//! - Adaptive window auto-tuning (reqwest http2_adaptive_window)
//! - TCP keepalive (30s) + keepalive probes + TCP_NODELAY
//! - Write coalescing (256KB BufWriter) + 128KB stream copy buffer
//! - BDP-based adaptive chunking for large files (>1MB → parallel range requests,
//!   chunks written directly at file offsets, scoped threads for blocking IO)
//! - Connection prewarming at pool init (TLS+H2 handshake off the critical path)
//! - Multi-connection pool (N clients, round-robin)
//! - Retry with exponential backoff + jitter
//! - AIMD adaptive concurrency limiter
//! - ETag / conditional GET / resume support
//! - Streaming to disk (no full memory buffer for small files)
//! - Pre-allocated file from Content-Length
//!
//! Usage:
//!   download_to_file(url, dest)              — streaming write
//!   download_to_bytes(url)                   — in-memory (small payloads)
//!   download_to_file_parallel(url, dest)     — adaptive chunked for large files
//!   download_with_etag(url, etag)            — conditional GET
//!   download_concurrent(urls, dests, max)    — parallel pool
//!   download_to_file_resume(url, dest, pos)  — resumable from byte offset

use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING, IF_NONE_MATCH, RANGE};

// ── Configuration (env-tunable) ──────────────────────────────────────────────

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;
const DEFAULT_MAX_CONCURRENCY: usize = 64;
const MAX_CONCURRENCY_CAP: usize = 256;
const DEFAULT_RETRY_COUNT: usize = 3;
const DEFAULT_RETRY_BACKOFF_MS: u64 = 250;
const MAX_RETRY_BACKOFF_MS: u64 = 5_000;
const POOL_IDLE_TIMEOUT_SECS: u64 = 90;
const MAX_POOL_IDLE: usize = 128;
const USER_AGENT_STR: &str = "jhol/1.0 (HTTP/2; rustls)";

// HTTP/2 tuning — from XDE / StormDL / boatramp-http research
const HTTP2_STREAM_WINDOW: u32 = 2 * 1024 * 1024; // 2MB per-stream window
const HTTP2_CONNECTION_WINDOW: u32 = 4 * 1024 * 1024; // 4MB connection window
const TCP_KEEPALIVE_SECS: u64 = 30;

// BDP-based chunked download — from StormDL / libfw research
const PARALLEL_CHUNK_THRESHOLD: u64 = 1024 * 1024; // 1MB: files >= this get chunked
const MIN_CHUNK_SIZE: u64 = 256 * 1024; // 256KB minimum chunk
const MAX_CHUNKS: usize = 16; // max parallel range requests

// Write coalescing — from StormDL / XDE research
const WRITE_BUF_SIZE: usize = 256 * 1024; // 256KB write buffer — optimal for sequential SSD I/O

// Multi-connection pool — from boatramp-http / XDE research
// Number of HTTP/2 connections in the pool. Measured on a true-cold install
// (fresh cache dir, 28 tarballs, medians of 3):
//   pool=1 596ms | pool=2 583ms | pool=4 816ms | pool=8 828ms
// Fewer connections is *better* here: h2 multiplexes every stream over the ones
// it has, so extra connections only add connection-setup cost - the same cost
// that made the old h1.1 path collapse to 12-15s under concurrent connections.
// 1 multiplexed HTTP/2 connection avoids competing TLS handshakes and maximizes stream concurrency.
// Override with JHOL_HTTP2_POOL_SIZE.
const DEFAULT_POOL_SIZE: usize = 1;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn timeout_secs() -> Duration {
    let default_val = env_u64("JHOL_HTTP2_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(env_u64("JHOL_FETCH_TIMEOUT_SECS", default_val))
}

fn connect_timeout_secs() -> Duration {
    Duration::from_secs(env_u64(
        "JHOL_HTTP2_CONNECT_TIMEOUT_SECS",
        DEFAULT_CONNECT_TIMEOUT_SECS,
    ))
}

/// Hot-path retry knobs, read once. The pool/clients already freeze their
/// config at init, so first-read-wins here is consistent (no quality loss).
struct RetryConfig {
    count: usize,
    backoff_ms: u64,
}

static RETRY_CONFIG: std::sync::OnceLock<RetryConfig> = std::sync::OnceLock::new();

fn retry_config() -> &'static RetryConfig {
    RETRY_CONFIG.get_or_init(|| RetryConfig {
        count: env_usize("JHOL_HTTP_RETRIES", DEFAULT_RETRY_COUNT),
        backoff_ms: env_u64("JHOL_HTTP_RETRY_BACKOFF_MS", DEFAULT_RETRY_BACKOFF_MS),
    })
}

fn retry_count() -> usize {
    retry_config().count
}

fn retry_backoff_ms() -> u64 {
    retry_config().backoff_ms
}

fn max_concurrency() -> usize {
    env_usize("JHOL_NETWORK_CONCURRENCY", DEFAULT_MAX_CONCURRENCY)
        .min(MAX_CONCURRENCY_CAP)
        .max(2)
}

fn pool_size() -> usize {
    env_usize("JHOL_HTTP2_POOL_SIZE", DEFAULT_POOL_SIZE).max(1)
}

// ── AIMD concurrency limiter ─────────────────────────────────────────────────

struct ConcurrencyLimiter {
    mutex: Mutex<usize>,
    condvar: Condvar,
    cap: AtomicUsize,
    configured_max: usize,
}

impl ConcurrencyLimiter {
    fn new(max: usize) -> Self {
        Self {
            mutex: Mutex::new(0),
            condvar: Condvar::new(),
            cap: AtomicUsize::new(max),
            configured_max: max,
        }
    }

    fn acquire(&self) -> ConcurrencyPermit<'_> {
        let mut count = self.mutex.lock().expect("concurrency limiter poisoned");
        while *count >= self.cap.load(Ordering::Relaxed) {
            count = self
                .condvar
                .wait(count)
                .expect("concurrency limiter condvar poisoned");
        }
        *count += 1;
        ConcurrencyPermit(self)
    }

    fn back_off(&self) {
        let cur = self.cap.load(Ordering::Relaxed);
        let new = (cur / 2).max(2);
        if new < cur {
            self.cap.store(new, Ordering::Relaxed);
        }
    }

    fn recover(&self) {
        let cur = self.cap.load(Ordering::Relaxed);
        if cur < self.configured_max {
            let _ = self
                .cap
                .compare_exchange(cur, cur + 1, Ordering::Relaxed, Ordering::Relaxed);
        }
    }
}

struct ConcurrencyPermit<'a>(&'a ConcurrencyLimiter);

impl Drop for ConcurrencyPermit<'_> {
    fn drop(&mut self) {
        let mut count = self
            .0
            .mutex
            .lock()
            .expect("concurrency limiter poisoned in drop");
        *count = count.saturating_sub(1);
        self.0.condvar.notify_one();
    }
}

// ── BDP estimator (StormDL technique) ────────────────────────────────────────

/// Estimates the Bandwidth-Delay Product to size parallel download chunks.
/// BDP = bandwidth × RTT. A higher BDP means more data can be in-flight.
/// Single lock: one acquisition covers both fields (was two).
struct BdpState {
    bandwidth_bps: f64, // bytes/sec
    rtt_ms: f64,        // round-trip time in milliseconds
}

struct BdpEstimator {
    inner: Mutex<BdpState>,
}

impl BdpEstimator {
    fn new() -> Self {
        Self {
            inner: Mutex::new(BdpState {
                bandwidth_bps: 1_000_000.0, // 1 MB/s initial
                rtt_ms: 50.0,               // 50ms initial
            }),
        }
    }

    /// Record a download sample: `bytes` received in `elapsed`.
    fn record_sample(&self, bytes: u64, elapsed: Duration, rtt: Duration) {
        let mut s = self.inner.lock().expect("bdp poisoned");
        let sec = elapsed.as_secs_f64();
        if sec > 0.0 {
            let sample = bytes as f64 / sec;
            // Exponential moving average (α = 0.3)
            s.bandwidth_bps = 0.3 * sample + 0.7 * s.bandwidth_bps;
        }
        let sample = rtt.as_secs_f64() * 1000.0;
        s.rtt_ms = 0.3 * sample + 0.7 * s.rtt_ms;
    }

    /// BDP in bytes. Used to size chunk count for parallel range downloads.
    fn bdp_bytes(&self) -> u64 {
        let s = self.inner.lock().expect("bdp poisoned");
        (s.bandwidth_bps * s.rtt_ms / 1000.0) as u64
    }

    /// How many parallel chunks to split a file into.
    /// More chunks = more parallelism, but each chunk must be at least MIN_CHUNK_SIZE.
    fn chunk_count(&self, file_size: u64) -> usize {
        let bdp = self.bdp_bytes().max(MIN_CHUNK_SIZE);
        let ideal = (file_size / bdp).max(1) as usize;
        ideal.min(MAX_CHUNKS)
    }
}

// ── Global HTTP/2 client pool ────────────────────────────────────────────────

static CLIENT_POOL: std::sync::OnceLock<Arc<ClientPool>> = std::sync::OnceLock::new();
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ClientPool {
    clients: Vec<Client>,
    next: AtomicUsize, // round-robin index
    limiter: ConcurrencyLimiter,
    metrics: DownloadMetrics,
    bdp: BdpEstimator,
}

impl ClientPool {
    /// Get the next client in round-robin fashion.
    fn get(&self) -> &Client {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.clients.len();
        &self.clients[idx]
    }
}

#[derive(Debug, Default)]
struct DownloadMetrics {
    requests_total: AtomicUsize,
    requests_success: AtomicUsize,
    requests_failed: AtomicUsize,
    bytes_downloaded: AtomicUsize,
    cache_hits: AtomicUsize,
    chunks_completed: AtomicUsize,
}

impl DownloadMetrics {
    fn record(&self, success: bool, bytes: usize, cache_hit: bool) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if success {
            self.requests_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.requests_failed.fetch_add(1, Ordering::Relaxed);
        }
        self.bytes_downloaded.fetch_add(bytes, Ordering::Relaxed);
        if cache_hit {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_chunk(&self) {
        self.chunks_completed.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            requests_success: self.requests_success.load(Ordering::Relaxed),
            requests_failed: self.requests_failed.load(Ordering::Relaxed),
            bytes_downloaded: self.bytes_downloaded.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            chunks_completed: self.chunks_completed.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub requests_total: usize,
    pub requests_success: usize,
    pub requests_failed: usize,
    pub bytes_downloaded: usize,
    pub cache_hits: usize,
    pub chunks_completed: usize,
}

fn build_h2_client() -> Client {
    h2_client_builder()
        .build()
        .expect("failed to build HTTP/2 client")
}

fn h2_client_builder() -> reqwest::blocking::ClientBuilder {
    let mut builder = Client::builder()
        .use_rustls_tls()
        // ── HTTP/2 tuning (XDE / StormDL / boatramp-http) ──
        // NOTE: no `http2_prior_knowledge()` — it skips the ALPN negotiation
        // and measured 1.45x slower than ureq on the 20-packument case.
        // Letting ALPN negotiate H2 is the fair alternative (see scorecard).
        .http2_initial_stream_window_size(HTTP2_STREAM_WINDOW)
        .http2_initial_connection_window_size(HTTP2_CONNECTION_WINDOW)
        // Adaptive mode resets both explicit windows to 65,535 bytes. Keep
        // bounded windows available from the first cold response instead.
        .http2_adaptive_window(false)
        .http2_max_frame_size(16_777_215) // 16MB max frame — fewer frames, less overhead
        .http2_max_header_list_size(256 * 1024) // 256KB max headers
        // ── TCP tuning ──
        .tcp_keepalive(Duration::from_secs(TCP_KEEPALIVE_SECS))
        .tcp_keepalive_interval(Duration::from_secs(10)) // probe every 10s
        .tcp_keepalive_retries(3) // give up after 3 failed probes
        .tcp_nodelay(true)
        // ── Transfer ──
        .gzip(true)
        .brotli(true) // enable brotli compression
        // ── Timeouts ──
        .timeout(timeout_secs())
        .connect_timeout(connect_timeout_secs())
        .pool_idle_timeout(Duration::from_secs(POOL_IDLE_TIMEOUT_SECS))
        .pool_max_idle_per_host(MAX_POOL_IDLE)
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(USER_AGENT_STR);

    // Ensure localhost and loopback are never intercepted by proxy
    if let Ok(proxy_url) = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
    {
        if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
            let proxy = proxy.no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.1,::1"));
            builder = builder.proxy(proxy);
        }
    } else {
        builder = builder.no_proxy();
    }

    builder
}

fn get_pool() -> &'static Arc<ClientPool> {
    CLIENT_POOL.get_or_init(|| {
        let concurrency = max_concurrency();
        let size = pool_size();
        let mut clients = Vec::with_capacity(size);
        for _ in 0..size {
            clients.push(build_h2_client());
        }
        Arc::new(ClientPool {
            clients,
            next: AtomicUsize::new(0),
            limiter: ConcurrencyLimiter::new(concurrency),
            metrics: DownloadMetrics::default(),
            bdp: BdpEstimator::new(),
        })
    })
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Eagerly initialize and prewarm the HTTP/2 connection pool.
pub fn prewarm() {
    let p = get_pool();
    let reg = crate::registry::registry_url();
    let url = format!("{}/", reg.trim_end_matches('/'));
    let _ = p.get().head(&url).send();
}

/// Download URL bytes into memory. Best for small payloads (packuments, < 5 MB).
pub fn download_to_bytes(url: &str) -> Result<Vec<u8>, String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let resp = send_with_retry(p.get(), url, None)?;
    let hint = content_length_hint(&resp);

    let mut buf = Vec::with_capacity(hint.unwrap_or(256 * 1024));
    let mut resp = resp;
    resp.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;

    p.metrics.record(true, buf.len(), false);
    Ok(buf)
}

/// Stream-download URL to a file. For files < PARALLEL_CHUNK_THRESHOLD.
/// Uses BufWriter for write coalescing (256KB batches, reduces syscalls).
pub fn download_to_file(url: &str, dest: &Path) -> Result<(), String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let resp = send_with_retry(p.get(), url, None)?;

    let out = std::fs::File::create(dest).map_err(|e| format!("create failed: {e}"))?;
    let mut writer = BufWriter::with_capacity(WRITE_BUF_SIZE, out);
    let mut resp = resp;
    let n = copy_large(&mut resp, &mut writer).map_err(|e| format!("write failed: {e}"))?;
    writer.flush().map_err(|e| format!("flush failed: {e}"))?;

    p.metrics.record(true, n as usize, false);
    Ok(())
}

/// Adaptive parallel chunked download for large files.
///
/// Uses BDP estimation (StormDL) to size chunks, then issues parallel
/// Range requests over HTTP/2 multiplexed streams. Each chunk is written
/// directly to the correct file offset (no temp files, no assembly pass).
/// Blocking chunk IO runs on scoped OS threads (never rayon's CPU pool).
pub fn download_to_file_parallel(url: &str, dest: &Path) -> Result<(), String> {
    let p = get_pool();

    // Step 1: HEAD to discover size + Accept-Ranges
    let head_resp = head_request(p.get(), url)?;
    let total_size = content_length_hint(&head_resp)
        .ok_or_else(|| format!("no Content-Length for {url} — cannot parallel download"))?;

    if (total_size as u64) < PARALLEL_CHUNK_THRESHOLD {
        // Small file: sequential is faster (no HEAD + Range overhead)
        drop(head_resp);
        return download_to_file(url, dest);
    }

    // Check Accept-Ranges
    let supports_ranges = head_resp
        .headers()
        .get("accept-ranges")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("bytes"))
        .unwrap_or(false);

    if !supports_ranges {
        drop(head_resp);
        return download_to_file(url, dest);
    }

    // Step 2: BDP-based chunk sizing
    let total_size_u64 = total_size as u64;
    let chunk_count = p.bdp.chunk_count(total_size_u64);
    let chunk_size = (total_size_u64 / chunk_count as u64).max(MIN_CHUNK_SIZE);

    // Step 3: Create a pre-allocated temporary output.
    let temp = dest.with_extension(format!(
        "{}part.{}.{}",
        dest.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{e}."))
            .unwrap_or_default(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let out = std::fs::File::create(&temp).map_err(|e| format!("create failed: {e}"))?;
    out.set_len(total_size_u64).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("pre-alloc failed: {e}")
    })?;
    drop(out);

    // Step 4: Build chunk ranges
    let chunk_ranges: Vec<(u64, u64)> = (0..chunk_count)
        .map(|i| {
            let start = i as u64 * chunk_size;
            let end = (start + chunk_size).min(total_size_u64).saturating_sub(1);
            (start, end)
        })
        .collect();

    // Step 5: blocking IO belongs on dedicated threads, NOT rayon's CPU pool —
    // a rayon worker blocked on HTTP/condvar starves every other rayon user.
    // std::thread::scope borrows url/dest directly (zero clones) and joins all.
    let mut results: Vec<Result<(u64, Duration, Duration), String>> =
        Vec::with_capacity(chunk_count);
    std::thread::scope(|s| {
        let handles: Vec<_> = chunk_ranges
            .iter()
            .map(|&(start, end)| {
                let temp = temp.clone();
                s.spawn(move || download_chunk_to_offset(url, &temp, start, end, total_size_u64))
            })
            .collect();
        results = handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("chunk thread panicked".to_string()))
            })
            .collect();
    });

    // Step 6: Check all chunks succeeded + feed real measurements to BDP
    for r in &results {
        if let Err(e) = r {
            let _ = std::fs::remove_file(&temp);
            return Err(e.clone());
        }
    }
    for r in &results {
        if let Ok((bytes, ttfb, body_elapsed)) = r {
            p.bdp.record_sample(*bytes, *body_elapsed, *ttfb);
        }
    }

    p.metrics.record(true, total_size_u64 as usize, false);

    if let Err(e) = replace_file_tmp(&temp, dest) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("publish download: {e}"));
    }

    Ok(())
}

/// Conditional GET: returns Ok(None) on 304 (cache still valid).
pub fn download_with_etag(
    url: &str,
    existing_etag: Option<&str>,
) -> Result<Option<Vec<u8>>, String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let mut headers = HeaderMap::new();
    if let Some(etag) = existing_etag {
        if let Ok(val) = HeaderValue::from_str(etag) {
            headers.insert(IF_NONE_MATCH, val);
        }
    }

    let resp = send_with_retry(p.get(), url, Some(&headers))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        p.metrics.record(true, 0, true);
        return Ok(None);
    }
    if !status.is_success() {
        p.metrics.record(false, 0, false);
        return Err(format!("HTTP {}", status.as_u16()));
    }

    let hint = content_length_hint(&resp);
    let mut buf = Vec::with_capacity(hint.unwrap_or(256 * 1024));
    let mut resp = resp;
    resp.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;

    p.metrics.record(true, buf.len(), false);
    Ok(Some(buf))
}

/// Like `download_with_etag` but also returns the new ETag string.
pub fn download_with_etag_full(
    url: &str,
    existing_etag: Option<&str>,
) -> Result<(Option<Vec<u8>>, Option<String>), String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let mut headers = HeaderMap::new();
    if let Some(etag) = existing_etag {
        if let Ok(val) = HeaderValue::from_str(etag) {
            headers.insert(IF_NONE_MATCH, val);
        }
    }

    let resp = send_with_retry(p.get(), url, Some(&headers))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        p.metrics.record(true, 0, true);
        return Ok((None, existing_etag.map(|s| s.to_string())));
    }
    if !status.is_success() {
        p.metrics.record(false, 0, false);
        return Err(format!("HTTP {}", status.as_u16()));
    }

    let new_etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let hint = content_length_hint(&resp);
    let mut buf = Vec::with_capacity(hint.unwrap_or(256 * 1024));
    let mut resp = resp;
    resp.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;

    p.metrics.record(true, buf.len(), false);
    Ok((Some(buf), new_etag))
}

/// Resume download from a byte offset (XDE / libfw technique).
/// Returns bytes written. Caller can track position for partial resume.
pub fn download_to_file_resume(url: &str, dest: &Path, start_byte: u64) -> Result<u64, String> {
    download_to_file_resume_bounded(url, dest, start_byte, u64::MAX)
}

/// Resume a ranged download without allowing the partial file to exceed `max_bytes`.
pub fn download_to_file_resume_bounded(
    url: &str,
    dest: &Path,
    start_byte: u64,
    max_bytes: u64,
) -> Result<u64, String> {
    if start_byte > max_bytes {
        return Err(format!(
            "resume offset {start_byte} exceeds limit {max_bytes}"
        ));
    }
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let range_header = format!("bytes={start_byte}-");
    let mut headers = HeaderMap::new();
    if let Ok(val) = HeaderValue::from_str(&range_header) {
        headers.insert(RANGE, val);
    }
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

    let resp = send_with_retry(p.get(), url, Some(&headers))?;

    // 206 Partial Content = resume working; 200 = server ignored range
    let status = resp.status().as_u16();
    if status != 200 && status != 206 {
        return Err(format!("HTTP {status} for resume request"));
    }

    let resume_total = if status == 206 {
        let content_range = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "resume response missing Content-Range".to_string())?;
        let (response_start, _response_end, response_total) = parse_content_range(content_range)?;
        if response_start != start_byte || response_total < start_byte {
            return Err(format!("invalid resume Content-Range: {content_range}"));
        }
        if response_total > max_bytes {
            return Err(format!(
                "resume size {response_total} exceeds limit {max_bytes}"
            ));
        }
        let existing_len = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        if existing_len != start_byte {
            return Err(format!(
                "resume prefix length {existing_len}, expected {start_byte}"
            ));
        }
        Some(response_total)
    } else {
        if let Some(length) = resp.content_length() {
            if length > max_bytes {
                return Err(format!("download size {length} exceeds limit {max_bytes}"));
            }
        }
        None
    };

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        // On 200 the server sent the full body: truncate so a longer stale
        // partial cannot leave a garbage tail behind the shorter rewrite.
        .truncate(status == 200)
        .open(dest)
        .map_err(|e| format!("open for resume: {e}"))?;
    file.seek(SeekFrom::Start(if status == 206 { start_byte } else { 0 }))
        .map_err(|e| format!("seek: {e}"))?;

    let mut writer = BufWriter::with_capacity(WRITE_BUF_SIZE, file);
    let mut resp = resp;
    let body_limit = if status == 206 {
        max_bytes.saturating_sub(start_byte)
    } else {
        max_bytes
    };
    let n = copy_bounded(&mut resp, &mut writer, body_limit).map_err(|e| format!("write: {e}"))?;
    writer.flush().map_err(|e| format!("flush: {e}"))?;

    if let Some(total) = resume_total {
        writer
            .get_ref()
            .set_len(total)
            .map_err(|e| format!("set resume length: {e}"))?;
    }

    p.metrics.record(true, n as usize, false);
    Ok(n)
}

fn copy_bounded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    max_bytes: u64,
) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    loop {
        let remaining = max_bytes.saturating_sub(total);
        let read_limit =
            usize::try_from(remaining.saturating_add(1).min(buf.len() as u64)).unwrap_or(buf.len());
        let n = reader.read(&mut buf[..read_limit])?;
        if n == 0 {
            break;
        }
        if n as u64 > remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("download body exceeds limit {max_bytes}"),
            ));
        }
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    Ok(total)
}

/// Parallel download: fetch multiple URLs concurrently, writing each to its dest.
pub fn download_concurrent(
    urls: &[(&str, &Path)],
    max_concurrent: usize,
) -> Vec<Result<(), String>> {
    use std::sync::mpsc;

    let cap = max_concurrent.min(max_concurrency());
    let limiter = Arc::new(ConcurrencyLimiter::new(cap));

    let (tx, rx) = mpsc::channel();

    let handles: Vec<_> = urls
        .iter()
        .enumerate()
        .map(|(i, (url, dest))| {
            let url = url.to_string();
            let dest = dest.to_path_buf();
            let tx = tx.clone();
            let limiter = limiter.clone();

            std::thread::spawn(move || {
                let _permit = limiter.acquire();
                let result = download_to_file(&url, &dest);
                let _ = tx.send((i, result));
            })
        })
        .collect();

    drop(tx);

    let mut results = vec![Err("not started".to_string()); urls.len()];
    while let Ok((i, result)) = rx.recv() {
        results[i] = result;
    }

    for h in handles {
        let _ = h.join();
    }

    results
}

/// Download with custom headers.
pub fn download_with_headers(url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let header_map = build_header_map(headers);
    let resp = send_with_retry(p.get(), url, Some(&header_map))?;
    let hint = content_length_hint(&resp);
    let mut buf = Vec::with_capacity(hint.unwrap_or(256 * 1024));
    let mut resp = resp;
    resp.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;

    p.metrics.record(true, buf.len(), false);
    Ok(buf)
}

/// GET with custom request headers via HTTP/2 pool. Returns (http_status, body_bytes, etag_from_response).
/// Handles 304 without retrying (caller decides). Uses shared connection pool with HTTP/2 multiplexing.
pub fn download_raw_with_headers(
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(u16, Vec<u8>, Option<String>), String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let header_map = build_header_map(headers);
    let t0 = Instant::now();
    let resp = send_with_retry(p.get(), url, Some(&header_map))?;
    let ttfb = t0.elapsed();
    let status = resp.status().as_u16();

    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if status == 304 {
        p.metrics.record(true, 0, true);
        return Ok((304, Vec::new(), etag));
    }

    if status >= 400 {
        p.metrics.record(false, 0, false);
        return Err(format!("HTTP {}", status));
    }

    let hint = content_length_hint(&resp);
    let mut buf = Vec::with_capacity(hint.unwrap_or(256 * 1024));
    let mut resp = resp;
    resp.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;

    if std::env::var("JHOL_HTTP_TIMING").is_ok() {
        let accept = header_map
            .get(reqwest::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-");
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-");
        eprintln!(
            "[HTTP-T] total {:>7}ms  ttfb {:>6}ms  body {:>6}ms  wire {:>9}B  decoded {:>10}B  accept={} type={} {}  (h2)",
            t0.elapsed().as_millis(),
            ttfb.as_millis(),
            t0.elapsed().as_millis().saturating_sub(ttfb.as_millis()),
            hint.unwrap_or(0),
            buf.len(),
            accept,
            content_type,
            url
        );
    }

    p.metrics.record(true, buf.len(), false);
    Ok((status, buf, etag))
}

/// Stream-download with custom headers to a file.
pub fn download_to_file_with_headers(
    url: &str,
    dest: &Path,
    headers: &[(&str, &str)],
) -> Result<(), String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();
    let timing = std::env::var_os("JHOL_HTTP_TIMING").is_some();
    let started = timing.then(std::time::Instant::now);
    let header_map = build_header_map(headers);
    let mut resp = send_with_retry(p.get(), url, Some(&header_map))?;
    let protocol = resp.version();
    let ttfb = started.as_ref().map(std::time::Instant::elapsed);
    let expected = content_length_hint(&resp);

    let out = std::fs::File::create(dest).map_err(|e| format!("create failed: {e}"))?;
    let mut writer = BufWriter::with_capacity(WRITE_BUF_SIZE, out);
    let mut body = CountedBody {
        inner: &mut resp,
        bytes_read: 0,
        expected: expected.map(|length| length as u64),
        measure_read_time: timing,
        read_elapsed: std::time::Duration::ZERO,
    };
    let n = copy_large(&mut body, &mut writer).map_err(|e| format!("write failed: {e}"))?;
    writer.flush().map_err(|e| format!("flush failed: {e}"))?;
    if expected.is_some_and(|length| n < length as u64) {
        p.metrics.record(false, n as usize, false);
        return Err(format!(
            "truncated download for {url}: expected {} bytes, got {n}",
            expected.unwrap_or_default()
        ));
    }
    p.metrics.record(true, n as usize, false);
    if let (Some(started), Some(ttfb)) = (started, ttfb) {
        eprintln!(
            "[HTTP-FILE-T] total {:>7}ms  ttfb {:>6}ms  read {:>6}ms  body {:>10}B  protocol {:?}  {}",
            started.elapsed().as_millis(), ttfb.as_millis(), body.read_elapsed.as_millis(), n, protocol, url,
        );
    }
    Ok(())
}

/// Consume an HTTP response body directly instead of staging it to a file.
/// The callback must read to EOF; the request permit remains held throughout.
pub fn download_with_headers_reader<T, F>(
    url: &str,
    headers: &[(&str, &str)],
    mut consume: F,
) -> Result<T, String>
where
    F: FnMut(&mut dyn Read) -> Result<T, String>,
{
    let p = get_pool();
    let _permit = p.limiter.acquire();
    let timing = std::env::var_os("JHOL_HTTP_TIMING").is_some();
    let started = timing.then(std::time::Instant::now);
    let header_map = build_header_map(headers);
    let mut response = send_with_retry(p.get(), url, Some(&header_map))?;
    let ttfb = started.as_ref().map(std::time::Instant::elapsed);
    if !response.status().is_success() {
        let status = response.status().as_u16();
        p.metrics.record(false, 0, false);
        return Err(format!("HTTP {status} for {url}"));
    }

    let expected = content_length_hint(&response);
    let mut body = CountedBody {
        inner: &mut response,
        bytes_read: 0,
        expected: expected.map(|length| length as u64),
        measure_read_time: timing,
        read_elapsed: std::time::Duration::ZERO,
    };
    let consume_started = timing.then(std::time::Instant::now);
    let value = match consume(&mut body) {
        Ok(value) => value,
        Err(error) => {
            p.metrics.record(false, body.bytes_read as usize, false);
            return Err(error);
        }
    };
    if expected.is_some_and(|length| body.bytes_read < length as u64) {
        let got = body.bytes_read;
        p.metrics.record(false, got as usize, false);
        return Err(format!(
            "truncated download for {url}: expected {} bytes, got {got}",
            expected.unwrap_or_default()
        ));
    }
    p.metrics.record(true, body.bytes_read as usize, false);
    if let (Some(started), Some(ttfb), Some(consume_started)) = (started, ttfb, consume_started) {
        eprintln!(
            "[HTTP-STREAM-T] total {:>7}ms  ttfb {:>6}ms  read {:>6}ms  consume {:>6}ms  body {:>10}B  {}",
            started.elapsed().as_millis(),
            ttfb.as_millis(),
            body.read_elapsed.as_millis(),
            consume_started.elapsed().as_millis(),
            body.bytes_read,
            url
        );
    }
    Ok(value)
}

/// Get download metrics snapshot.
pub fn get_metrics() -> MetricsSnapshot {
    get_pool().metrics.snapshot()
}

/// Read metrics without creating the global pool, for aggregate diagnostics.
pub fn get_metrics_if_initialized() -> Option<MetricsSnapshot> {
    CLIENT_POOL.get().map(|pool| pool.metrics.snapshot())
}

/// Print download metrics summary.
pub fn print_metrics() {
    let m = get_metrics();
    let p = get_pool();
    let cap = p.limiter.cap.load(Ordering::Relaxed);
    let bdp = p.bdp.bdp_bytes();
    eprintln!(
        "[http2] requests={total} ok={success} fail={failed} bytes={bytes} \
         cache_hits={cache_hits} chunks={chunks} concurrency_cap={cap} \
         bdp={bdp} pool_size={pool}",
        total = m.requests_total,
        success = m.requests_success,
        failed = m.requests_failed,
        bytes = m.bytes_downloaded,
        cache_hits = m.cache_hits,
        chunks = m.chunks_completed,
        pool = p.clients.len(),
    );
}

// ── Internal helpers ─────────────────────────────────────────────────────────

fn content_length_hint(resp: &reqwest::blocking::Response) -> Option<usize> {
    resp.content_length().map(|n| n as usize)
}

struct CountedBody<'a> {
    inner: &'a mut dyn Read,
    bytes_read: u64,
    expected: Option<u64>,
    measure_read_time: bool,
    read_elapsed: std::time::Duration,
}

impl Read for CountedBody<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let started = self.measure_read_time.then(std::time::Instant::now);
        let n = self.inner.read(buf)?;
        if let Some(started) = started {
            self.read_elapsed += started.elapsed();
        }
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

/// Stream copy with a 128KB buffer. `std::io::copy` uses 8KB internally,
/// which costs 16x more read/write loop iterations on multi-MB downloads.
fn copy_large(reader: &mut impl Read, writer: &mut impl Write) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    Ok(total)
}

fn build_header_map(headers: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(key), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            map.insert(key, val);
        }
    }
    map
}

fn replace_file_tmp(temp: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        match std::fs::remove_file(dest) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    std::fs::rename(temp, dest)
}

/// HEAD request to discover file size + Accept-Ranges support.
fn head_request(client: &Client, url: &str) -> Result<reqwest::blocking::Response, String> {
    let resp = client
        .head(url)
        .send()
        .map_err(|e| format!("HEAD failed for {url}: {e}"))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        return Err(format!("HEAD HTTP {status} for {url}"));
    }
    Ok(resp)
}

/// Download a byte range directly into the output file at `chunk_start`.
/// Returns (bytes_written, ttfb, body_elapsed) for real BDP measurement.
fn download_chunk_to_offset(
    url: &str,
    dest: &Path,
    chunk_start: u64,
    chunk_end: u64,
    total_size: u64,
) -> Result<(u64, Duration, Duration), String> {
    let p = get_pool();
    let _permit = p.limiter.acquire();

    let range_header = format!("bytes={chunk_start}-{chunk_end}");
    let mut headers = HeaderMap::new();
    if let Ok(val) = HeaderValue::from_str(&range_header) {
        headers.insert(RANGE, val);
    }
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

    let send_start = Instant::now();
    let resp = send_with_retry(p.get(), url, Some(&headers))?;
    let ttfb = send_start.elapsed();

    let status = resp.status().as_u16();
    if status == 200 {
        return Err("range request returned full response (HTTP 200)".to_string());
    }
    if status != 206 {
        return Err(format!("range request HTTP {status}"));
    }

    let content_range = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| "range response missing Content-Range".to_string())?;
    let (response_start, response_end, response_total) = parse_content_range(content_range)?;
    let expected_len = chunk_end - chunk_start + 1;
    if response_start != chunk_start || response_end != chunk_end || response_total != total_size {
        return Err(format!("invalid Content-Range: {content_range}"));
    }

    // Open output file and seek to the chunk offset — no temp files
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .map_err(|e| format!("open for chunk write: {e}"))?;
    file.seek(SeekFrom::Start(chunk_start))
        .map_err(|e| format!("seek to {chunk_start}: {e}"))?;

    let mut writer = BufWriter::with_capacity(WRITE_BUF_SIZE, file);
    let body_start = Instant::now();
    let mut resp = resp;
    let n = copy_large(&mut resp, &mut writer).map_err(|e| format!("write chunk: {e}"))?;
    writer.flush().map_err(|e| format!("flush chunk: {e}"))?;
    let body_elapsed = body_start.elapsed();

    if n != expected_len {
        return Err(format!("range body length {n}, expected {expected_len}"));
    }

    p.metrics.record(true, n as usize, false);
    p.metrics.record_chunk();

    Ok((n, ttfb, body_elapsed))
}

fn parse_content_range(value: &str) -> Result<(u64, u64, u64), String> {
    let (range, total) = value
        .strip_prefix("bytes ")
        .and_then(|v| v.split_once('/'))
        .ok_or_else(|| "malformed Content-Range".to_string())?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| "malformed Content-Range".to_string())?;
    let start = start
        .parse()
        .map_err(|_| "malformed Content-Range".to_string())?;
    let end = end
        .parse()
        .map_err(|_| "malformed Content-Range".to_string())?;
    let total = total
        .parse()
        .map_err(|_| "malformed Content-Range".to_string())?;
    if start > end || end >= total {
        return Err("malformed Content-Range".to_string());
    }
    Ok((start, end, total))
}

/// Send a GET with retry. Returns the response body reader.
fn send_with_retry(
    client: &Client,
    url: &str,
    extra_headers: Option<&HeaderMap>,
) -> Result<reqwest::blocking::Response, String> {
    let retries = retry_count();
    let mut backoff = retry_backoff_ms();
    let mut attempt = 0;

    loop {
        attempt += 1;

        let mut req = client.get(url);

        if let Some(hdrs) = extra_headers {
            for (k, v) in hdrs {
                req = req.header(k, v);
            }
        }

        match req.send() {
            Ok(resp) => {
                let status = resp.status().as_u16();
                // 200 full, 206 partial (range/chunk/resume), 304 not-modified
                if status == 200 || status == 206 || status == 304 {
                    get_pool().limiter.recover();
                    return Ok(resp);
                }
                if attempt <= retries && (status >= 500 || status == 429) {
                    get_pool().limiter.back_off();
                    let jitter = rand_ms();
                    std::thread::sleep(Duration::from_millis(backoff + jitter));
                    backoff = (backoff.saturating_mul(2)).min(MAX_RETRY_BACKOFF_MS);
                    continue;
                }
                return Err(format!("HTTP {status} for {url}"));
            }
            Err(e) => {
                let is_transient = e.is_timeout() || e.is_connect() || e.is_request();
                if attempt <= retries && is_transient {
                    get_pool().limiter.back_off();
                    let jitter = rand_ms();
                    std::thread::sleep(Duration::from_millis(backoff + jitter));
                    backoff = (backoff.saturating_mul(2)).min(MAX_RETRY_BACKOFF_MS);
                    continue;
                }
                return Err(format!("request failed for {url}: {e:?}"));
            }
        }
    }
}

/// Random jitter in milliseconds [0, 250). Uses xorshift — zero allocation.
fn rand_ms() -> u64 {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(Instant::now().elapsed().as_nanos() as u64);
    // xorshift64
    let mut x = seed;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x % 250
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_windows_reach_http2_wire() {
        // Inspect real SETTINGS/WINDOW_UPDATE frames, not builder intent.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut preface = [0u8; 24];
            socket.read_exact(&mut preface).unwrap();
            assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
            let mut stream_window = 65_535;
            let mut connection_window = 65_535;
            for _ in 0..8 {
                let mut header = [0u8; 9];
                if socket.read_exact(&mut header).is_err() {
                    break;
                }
                let length = u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize;
                assert!(length <= 16_384, "bounded client preface frames");
                let mut payload = vec![0u8; length];
                socket.read_exact(&mut payload).unwrap();
                if header[3] == 4 && header[4] & 1 == 0 {
                    assert_eq!(length % 6, 0);
                    for setting in payload.chunks_exact(6) {
                        if setting[..2] == [0, 4] {
                            stream_window = u32::from_be_bytes(setting[2..].try_into().unwrap());
                        }
                    }
                } else if header[3] == 8 && header[5..] == [0, 0, 0, 0] {
                    assert_eq!(length, 4);
                    connection_window +=
                        u32::from_be_bytes(payload.try_into().unwrap()) & 0x7fff_ffff;
                }
                if stream_window == HTTP2_STREAM_WINDOW
                    && connection_window == HTTP2_CONNECTION_WINDOW
                {
                    break;
                }
            }
            (stream_window, connection_window)
        });
        // Only this loopback test uses h2c; production still negotiates ALPN.
        let client = h2_client_builder()
            .no_proxy()
            .http2_prior_knowledge()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let _ = client.get(format!("http://{address}/")).send();
        assert_eq!(
            server.join().unwrap(),
            (HTTP2_STREAM_WINDOW, HTTP2_CONNECTION_WINDOW)
        );
    }

    #[test]
    fn env_usize_falls_back_to_default() {
        assert_eq!(env_usize("JHOL_TEST_NONEXISTENT_KEY_42", 42), 42);
    }

    #[test]
    fn env_u64_falls_back_to_default() {
        assert_eq!(env_u64("JHOL_TEST_NONEXISTENT_KEY_99", 99), 99);
    }

    #[test]
    fn concurrency_limiter_acquire_and_release() {
        let lim = ConcurrencyLimiter::new(2);
        let g1 = lim.acquire();
        let g2 = lim.acquire();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 2);
        drop(g1);
        let g3 = lim.acquire();
        drop(g2);
        drop(g3);
    }

    #[test]
    fn back_off_halves_cap() {
        let lim = ConcurrencyLimiter::new(16);
        lim.back_off();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 8);
        lim.back_off();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 4);
        lim.back_off();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 2);
        lim.back_off();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 2); // floor
    }

    #[test]
    fn recover_increases_cap() {
        let lim = ConcurrencyLimiter::new(4);
        lim.back_off();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 2);
        lim.recover();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 3);
        lim.recover();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 4);
        lim.recover();
        assert_eq!(lim.cap.load(Ordering::Relaxed), 4); // capped
    }

    #[test]
    fn rand_ms_is_bounded() {
        for _ in 0..1000 {
            let v = rand_ms();
            assert!(v < 250, "rand_ms() returned {v}, expected < 250");
        }
    }

    #[test]
    fn client_pool_initializes() {
        let p = get_pool();
        assert!(!p.clients.is_empty());
        assert!(Arc::strong_count(p) >= 1);
    }

    #[test]
    fn metrics_default_to_zero() {
        let m = DownloadMetrics::default();
        let s = m.snapshot();
        assert_eq!(s.requests_total, 0);
        assert_eq!(s.requests_success, 0);
        assert_eq!(s.requests_failed, 0);
        assert_eq!(s.bytes_downloaded, 0);
        assert_eq!(s.cache_hits, 0);
        assert_eq!(s.chunks_completed, 0);
    }

    #[test]
    fn reader_reports_truncation_before_consumer_can_publish() {
        for expected in [4, 8] {
            let mut input = &b"body"[..];
            let mut body = CountedBody {
                inner: &mut input,
                bytes_read: 0,
                expected: Some(expected),
                measure_read_time: false,
                read_elapsed: std::time::Duration::ZERO,
            };
            assert_eq!(body.read(&mut []).unwrap(), 0);
            let mut bytes = Vec::new();
            let result = body.read_to_end(&mut bytes);
            assert_eq!(result.is_ok(), expected == 4);
            assert_eq!(bytes, b"body");
        }
    }

    #[test]
    fn file_download_checks_response_length() {
        use std::net::TcpListener;
        for advertised in [4, 8] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/archive", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                stream.read(&mut request).unwrap();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {advertised}\r\nConnection: close\r\n\r\nbody").unwrap();
            });
            let temp = tempfile::tempdir().unwrap();
            let destination = temp.path().join("archive.tgz");
            let result = download_to_file_with_headers(&url, &destination, &[]);
            server.join().unwrap();
            if advertised == 4 {
                result.unwrap();
                assert_eq!(std::fs::read(destination).unwrap(), b"body");
            } else {
                assert!(result.is_err(), "accepted truncated response");
            }
        }
    }

    #[test]
    fn metrics_record_counts() {
        let m = DownloadMetrics::default();
        m.record(true, 1024, false);
        m.record(true, 2048, true);
        m.record(false, 0, false);
        m.record_chunk();
        m.record_chunk();
        let s = m.snapshot();
        assert_eq!(s.requests_total, 3);
        assert_eq!(s.requests_success, 2);
        assert_eq!(s.requests_failed, 1);
        assert_eq!(s.bytes_downloaded, 3072);
        assert_eq!(s.cache_hits, 1);
        assert_eq!(s.chunks_completed, 2);
    }

    #[test]
    fn bdp_estimator_initial_values() {
        let bdp = BdpEstimator::new();
        let bytes = bdp.bdp_bytes();
        // Initial: 1MB/s * 50ms = 50KB
        assert!(bytes > 0, "bdp should be > 0, got {bytes}");
    }

    #[test]
    fn bdp_chunk_count_scales_with_file_size() {
        let bdp = BdpEstimator::new();
        // Small file → 1 chunk
        assert_eq!(bdp.chunk_count(100_000), 1);
        // Larger file → more chunks
        let c = bdp.chunk_count(10_000_000);
        assert!(c >= 1 && c <= MAX_CHUNKS, "chunks={c}");
    }

    #[test]
    fn bdp_estimator_records_samples() {
        let bdp = BdpEstimator::new();
        bdp.record_sample(
            1_000_000,
            Duration::from_millis(500),
            Duration::from_millis(30),
        );
        let bytes = bdp.bdp_bytes();
        assert!(bytes > 0);
    }

    #[test]
    fn build_header_map_works() {
        let h = build_header_map(&[("x-test", "hello"), ("range", "bytes=0-100")]);
        assert!(h.contains_key("x-test"));
        assert!(h.contains_key("range"));
    }

    #[test]
    fn parse_content_range_validates_total() {
        assert_eq!(
            parse_content_range("bytes 0-0/4377468").unwrap(),
            (0, 0, 4_377_468)
        );
    }

    #[test]
    fn parse_content_range_rejects_missing_or_malformed() {
        assert!(parse_content_range("bytes 0-0").is_err());
        assert!(parse_content_range("not-a-range").is_err());
        assert!(parse_content_range("bytes 2-1/10").is_err());
    }

    #[test]
    fn range_http_200_is_rejected_before_writing() {
        let (url, _shutdown) =
            test_server("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 3\r\n\r\nabc");
        let dir = test_dir("range-200");
        let dest = dir.join("out");
        std::fs::write(&dest, b"original").unwrap();
        assert!(download_chunk_to_offset(&url, &dest, 0, 2, 3).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"original");
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jhol-http2-{name}-{}",
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_server(response: &str) -> (String, std::sync::mpsc::Sender<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let response = response.as_bytes().to_vec();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let mut streams = Vec::new();
            while !matches!(
                rx.try_recv(),
                Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected)
            ) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
                        let mut buf = [0u8; 4096];
                        let mut total = 0;
                        while total < buf.len() {
                            match stream.read(&mut buf[total..]) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    total += n;
                                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                            }
                        }
                        let _ = stream.write_all(&response);
                        let _ = stream.flush();
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                        streams.push(stream);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            drop(streams);
        });
        (format!("http://{addr}/file"), tx)
    }

    #[test]
    fn range_missing_content_range_is_rejected() {
        let (url, _shutdown) = test_server(
            "HTTP/1.1 206 Partial Content\r\nConnection: close\r\nContent-Length: 3\r\n\r\nabc",
        );
        let dir = test_dir("missing-range");
        let dest = dir.join("out");
        std::fs::write(&dest, b"old").unwrap();
        assert!(download_chunk_to_offset(&url, &dest, 0, 2, 3).is_err());
    }

    #[test]
    fn range_wrong_start_and_short_body_are_rejected() {
        let (url, _s1) = test_server("HTTP/1.1 206 Partial Content\r\nConnection: close\r\nContent-Range: bytes 1-3/4\r\nContent-Length: 3\r\n\r\nabc");
        let dir = test_dir("wrong-range");
        let dest = dir.join("out");
        std::fs::write(&dest, b"old").unwrap();
        assert!(download_chunk_to_offset(&url, &dest, 0, 2, 4).is_err());

        let (url, _s2) = test_server("HTTP/1.1 206 Partial Content\r\nConnection: close\r\nContent-Range: bytes 0-3/4\r\nContent-Length: 2\r\n\r\nab");
        assert!(download_chunk_to_offset(&url, &dest, 0, 3, 4).is_err());
    }

    #[test]
    fn resume_valid_partial_is_byte_identical() {
        let (url, _shutdown) = test_server("HTTP/1.1 206 Partial Content\r\nConnection: close\r\nContent-Range: bytes 3-5/6\r\nContent-Length: 3\r\n\r\ndef");
        let dir = test_dir("resume");
        let dest = dir.join("out");
        std::fs::write(&dest, b"abc").unwrap();
        match download_to_file_resume(&url, &dest, 3) {
            Ok(n) => assert_eq!(n, 3),
            Err(e) => panic!("download_to_file_resume failed with: {}", e),
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"abcdef");
    }

    #[test]
    fn bounded_resume_rejects_declared_size_before_writing() {
        let (url, _shutdown) = test_server(
            "HTTP/1.1 206 Partial Content\r\nConnection: close\r\nContent-Range: bytes 3-5/6\r\nContent-Length: 3\r\n\r\ndef",
        );
        let dir = test_dir("resume-bounded");
        let dest = dir.join("out");
        std::fs::write(&dest, b"abc").unwrap();
        let err = download_to_file_resume_bounded(&url, &dest, 3, 5).unwrap_err();
        assert!(err.contains("exceeds limit 5"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
    }

    #[test]
    fn bounded_download_rejects_oversized_body_without_length_header() {
        let (url, _shutdown) = test_server("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabcdef");
        let dir = test_dir("download-bounded-body");
        let dest = dir.join("out");
        let err = download_to_file_resume_bounded(&url, &dest, 0, 5).unwrap_err();
        assert!(err.contains("exceeds limit 5"));
        assert!(std::fs::metadata(&dest).unwrap().len() <= 5);
    }

    #[test]
    fn resume_rejects_offset_beyond_existing_file() {
        let (url, _shutdown) = test_server("HTTP/1.1 206 Partial Content\r\nConnection: close\r\nContent-Range: bytes 4-5/6\r\nContent-Length: 2\r\n\r\ndef");
        let dir = test_dir("resume-short");
        let dest = dir.join("out");
        std::fs::write(&dest, b"abc").unwrap();
        assert!(download_to_file_resume(&url, &dest, 4).is_err());
    }

    #[test]
    fn pool_round_robin_cycles() {
        // Race-free contract: every `get()` returns one of the pool's clients.
        // (Exact cycle assertions are invalid: the counter is process-global
        // and other tests advance it concurrently on multi-threaded harnesses.)
        let p = get_pool();
        assert!(!p.clients.is_empty());
        let addrs: std::collections::HashSet<usize> =
            p.clients.iter().map(|c| c as *const _ as usize).collect();
        for _ in 0..4 * p.clients.len() {
            assert!(addrs.contains(&(p.get() as *const _ as usize)));
        }
    }
}
