//! `jhol-cloud` — offline control-plane domain model (blueprint §10 / Phase D).
//!
//! This crate is deliberately a *library*, not a server. It encodes the
//! contracts a future hosted control plane needs: tenants, scoped tokens,
//! quota accounting, deterministic storage routing, audits, and a small
//! request/response state machine. Everything runs in memory with no network
//! or filesystem access, so tests and emergency drills never touch production.
//!
//! ## Security notes
//!
//! Token material comes from the operating system's CSPRNG via [`random_hex`]
//! (the earlier hash-of-timestamp generator was not suitable for secrets and
//! has been replaced). The protocol handler still assumes a trusted
//! transport; authentication is enforced, but transport encryption is outside
//! this crate.

#![forbid(unsafe_code)]

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use thiserror::Error;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const TOKEN_PREFIX: &str = "jhc1";
pub const DEFAULT_BUCKET_PREFIX: &str = "jhol-cas";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum CloudError {
    #[error("invalid tenant id: {0}")]
    InvalidTenant(String),
    #[error("tenant not found: {0}")]
    TenantNotFound(String),
    #[error("tenant is {0}")]
    TenantInactive(String),
    #[error("tenant already exists: {0}")]
    Conflict(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("token expired")]
    TokenExpired,
    #[error("token revoked")]
    TokenRevoked,
    #[error("scope denied: {0}")]
    ScopeDenied(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("artifact digest is invalid: {0}")]
    InvalidDigest(String),
    #[error("unsupported protocol version {0}")]
    Protocol(u32),
}

impl CloudError {
    pub fn status(&self) -> u16 {
        match self {
            CloudError::InvalidTenant(_)
            | CloudError::BadRequest(_)
            | CloudError::InvalidDigest(_)
            | CloudError::Protocol(_) => 400,
            CloudError::Unauthorized | CloudError::TokenExpired | CloudError::TokenRevoked => 401,
            CloudError::ScopeDenied(_) | CloudError::TenantInactive(_) => 403,
            CloudError::TenantNotFound(_) => 404,
            CloudError::Conflict(_) => 409,
            CloudError::QuotaExceeded(_) => 429,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            CloudError::InvalidTenant(_) => "invalid_tenant",
            CloudError::TenantNotFound(_) => "tenant_not_found",
            CloudError::TenantInactive(_) => "tenant_inactive",
            CloudError::Conflict(_) => "conflict",
            CloudError::Unauthorized => "unauthorized",
            CloudError::TokenExpired => "token_expired",
            CloudError::TokenRevoked => "token_revoked",
            CloudError::ScopeDenied(_) => "scope_denied",
            CloudError::QuotaExceeded(_) => "quota_exceeded",
            CloudError::BadRequest(_) => "bad_request",
            CloudError::InvalidDigest(_) => "invalid_digest",
            CloudError::Protocol(_) => "unsupported_protocol",
        }
    }
}

// ---------------------------------------------------------------------------
// Tenants and plans
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(String);

impl TenantId {
    pub fn parse(value: impl Into<String>) -> Result<Self, CloudError> {
        let value = value.into();
        let valid_len = (3..=63).contains(&value.len());
        let valid_chars = value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let clean_edges = !value.starts_with('-') && !value.ends_with('-') && !value.contains("--");
        if valid_len && valid_chars && clean_edges {
            Ok(TenantId(value))
        } else {
            Err(CloudError::InvalidTenant(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanTier {
    #[default]
    Free,
    Team,
    Enterprise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quotas {
    pub requests_per_minute: u64,
    pub upload_bytes_per_day: u64,
    pub storage_bytes: u64,
    pub concurrent_jobs: u16,
}

impl Quotas {
    pub fn for_tier(tier: PlanTier) -> Self {
        match tier {
            PlanTier::Free => Quotas {
                requests_per_minute: 60,
                upload_bytes_per_day: 1 << 30,
                storage_bytes: 1 << 30,
                concurrent_jobs: 1,
            },
            PlanTier::Team => Quotas {
                requests_per_minute: 600,
                upload_bytes_per_day: 50 << 30,
                storage_bytes: 100 << 30,
                concurrent_jobs: 8,
            },
            PlanTier::Enterprise => Quotas {
                requests_per_minute: 6_000,
                upload_bytes_per_day: 1 << 40,
                storage_bytes: 10 << 40,
                concurrent_jobs: 64,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    #[default]
    Active,
    Suspended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    pub display_name: String,
    pub tier: PlanTier,
    pub status: TenantStatus,
    pub quotas: Quotas,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl Tenant {
    pub fn new(
        id: TenantId,
        display_name: impl Into<String>,
        tier: PlanTier,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            tier,
            status: TenantStatus::Active,
            quotas: Quotas::for_tier(tier),
            created_at: now,
            labels: BTreeMap::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.status == TenantStatus::Active
    }
}

// ---------------------------------------------------------------------------
// Scoped tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    RemoteRead,
    RemoteWrite,
    UsageRead,
    Admin,
}

impl Scope {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().replace('-', "_").to_ascii_lowercase().as_str() {
            "remote_read" | "read" => Some(Scope::RemoteRead),
            "remote_write" | "write" => Some(Scope::RemoteWrite),
            "usage_read" | "usage" => Some(Scope::UsageRead),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::RemoteRead => "remote_read",
            Scope::RemoteWrite => "remote_write",
            Scope::UsageRead => "usage_read",
            Scope::Admin => "admin",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub tenant: TenantId,
    pub name: String,
    /// SHA-256 of the secret half. The plaintext is returned only once.
    pub digest: String,
    pub scopes: Vec<Scope>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ApiToken {
    pub fn has_scope(&self, required: Scope) -> bool {
        self.scopes.contains(&Scope::Admin) || self.scopes.contains(&required)
    }

    pub fn active(&self, now: DateTime<Utc>) -> bool {
        !self.revoked && self.expires_at.map(|e| e > now).unwrap_or(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedToken {
    pub token: ApiToken,
    /// Present exactly once at issuance.
    pub plaintext: String,
}

pub fn token_digest(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    hex::encode(h.finalize())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Cryptographically random hex from the OS entropy pool (`rand`'s
/// thread-local CSPRNG, seeded by the operating system). Token identifiers
/// and secrets must not be predictable; the previous hash-of-timestamp scheme
/// was not suitable for secrets and has been replaced.
pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Issue a token. `ttl` is expressed in seconds.
pub fn new_token(
    tenant: TenantId,
    name: &str,
    scopes: Vec<Scope>,
    ttl_seconds: Option<i64>,
    now: DateTime<Utc>,
) -> Result<IssuedToken, CloudError> {
    if name.trim().is_empty() {
        return Err(CloudError::BadRequest(
            "token name must not be empty".into(),
        ));
    }
    let mut unique: Vec<Scope> = scopes;
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return Err(CloudError::BadRequest(
            "token must have at least one scope".into(),
        ));
    }
    let id = random_hex(8);
    let secret = random_hex(32);
    let plaintext = format!("{TOKEN_PREFIX}.{id}.{secret}");
    let expires_at = match ttl_seconds {
        Some(seconds) if seconds <= 0 => {
            return Err(CloudError::BadRequest("ttl must be positive".into()))
        }
        Some(seconds) => Some(now + Duration::seconds(seconds)),
        None => None,
    };
    Ok(IssuedToken {
        token: ApiToken {
            id,
            tenant,
            name: name.to_string(),
            digest: token_digest(&secret),
            scopes: unique,
            created_at: now,
            expires_at,
            revoked: false,
            last_used_at: None,
        },
        plaintext,
    })
}

fn split_token(plaintext: &str) -> Option<(&str, &str, &str)> {
    let mut parts = plaintext.trim().split('.');
    let prefix = parts.next()?;
    let id = parts.next()?;
    let secret = parts.next()?;
    if parts.next().is_some() || prefix != TOKEN_PREFIX || id.is_empty() || secret.is_empty() {
        return None;
    }
    Some((prefix, id, secret))
}

// ---------------------------------------------------------------------------
// Usage accounting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    Request,
    UploadBytes,
    DownloadBytes,
    JobSecond,
}

impl UsageKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "request" | "requests" => Some(UsageKind::Request),
            "upload_bytes" | "upload" => Some(UsageKind::UploadBytes),
            "download_bytes" | "download" => Some(UsageKind::DownloadBytes),
            "job_second" | "job_seconds" => Some(UsageKind::JobSecond),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub idempotency_key: String,
    pub kind: UsageKind,
    pub quantity: u64,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub requests: u64,
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub job_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<DateTime<Utc>>,
}

impl UsageTotals {
    fn add(&mut self, kind: UsageKind, quantity: u64, at: DateTime<Utc>) {
        match kind {
            UsageKind::Request => self.requests = self.requests.saturating_add(quantity),
            UsageKind::UploadBytes => {
                self.upload_bytes = self.upload_bytes.saturating_add(quantity)
            }
            UsageKind::DownloadBytes => {
                self.download_bytes = self.download_bytes.saturating_add(quantity)
            }
            UsageKind::JobSecond => self.job_seconds = self.job_seconds.saturating_add(quantity),
        }
        self.last_event_at = Some(match self.last_event_at {
            Some(previous) if previous > at => previous,
            _ => at,
        });
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageLedger {
    events: Vec<(TenantId, UsageEvent)>,
    seen_keys: BTreeSet<String>,
    totals: BTreeMap<TenantId, UsageTotals>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReceipt {
    pub tenant: TenantId,
    pub duplicate: bool,
    pub totals: UsageTotals,
    pub window_requests: u64,
    pub window_upload_bytes: u64,
}

impl UsageLedger {
    fn window(
        &self,
        tenant: &TenantId,
        kind: UsageKind,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> u64 {
        self.events
            .iter()
            .filter(|(t, e)| t == tenant && e.kind == kind && e.at >= since && e.at <= until)
            .map(|(_, e)| e.quantity)
            .fold(0u64, u64::saturating_add)
    }

    pub fn totals(&self, tenant: &TenantId) -> UsageTotals {
        self.totals.get(tenant).cloned().unwrap_or_default()
    }

    pub fn events_since(&self, tenant: &TenantId, since: DateTime<Utc>) -> usize {
        self.events
            .iter()
            .filter(|(t, e)| t == tenant && e.at >= since)
            .count()
    }

    fn record(&mut self, tenant: &Tenant, event: UsageEvent) -> Result<UsageReceipt, CloudError> {
        let event_at = event.at;
        let minute_start = event_at - Duration::minutes(1);
        let day_start = event_at - Duration::days(1);
        let requests = self.window(&tenant.id, UsageKind::Request, minute_start, event.at);
        let uploads = self.window(&tenant.id, UsageKind::UploadBytes, day_start, event.at);

        if event.kind == UsageKind::Request
            && requests.saturating_add(event.quantity) > tenant.quotas.requests_per_minute
        {
            return Err(CloudError::QuotaExceeded(format!(
                "{} requests/min (limit {})",
                requests.saturating_add(event.quantity),
                tenant.quotas.requests_per_minute
            )));
        }
        if event.kind == UsageKind::UploadBytes
            && uploads.saturating_add(event.quantity) > tenant.quotas.upload_bytes_per_day
        {
            return Err(CloudError::QuotaExceeded(format!(
                "{} upload bytes/day (limit {})",
                uploads.saturating_add(event.quantity),
                tenant.quotas.upload_bytes_per_day
            )));
        }

        let duplicate = !self.seen_keys.insert(event.idempotency_key.clone());
        if !duplicate {
            let totals = self.totals.entry(tenant.id.clone()).or_default();
            totals.add(event.kind, event.quantity, event.at);
            self.events.push((tenant.id.clone(), event));
        }

        Ok(UsageReceipt {
            tenant: tenant.id.clone(),
            duplicate,
            totals: self.totals(&tenant.id),
            window_requests: self.window(
                &tenant.id,
                UsageKind::Request,
                minute_start,
                Utc::now().max(event_at),
            ),
            window_upload_bytes: self.window(
                &tenant.id,
                UsageKind::UploadBytes,
                day_start,
                Utc::now().max(event_at),
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Storage routing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    Local,
    S3,
    Gcs,
    AzureBlob,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRouter {
    pub backend: StorageBackend,
    pub region: String,
    pub bucket_prefix: String,
}

impl Default for StorageRouter {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Local,
            region: "local".into(),
            bucket_prefix: DEFAULT_BUCKET_PREFIX.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageRoute {
    pub tenant: TenantId,
    pub backend: StorageBackend,
    pub region: String,
    pub bucket: String,
    pub key: String,
}

impl StorageRouter {
    pub fn route(&self, tenant: &TenantId, digest: &str) -> Result<StorageRoute, CloudError> {
        let digest = digest.trim().to_ascii_lowercase();
        if digest.len() < 8 || digest.len() > 128 || !digest.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(CloudError::InvalidDigest(digest));
        }
        let key = format!(
            "tenants/{}/cas/{}/{}/{}",
            tenant,
            &digest[0..2],
            &digest[2..4],
            digest
        );
        Ok(StorageRoute {
            tenant: tenant.clone(),
            backend: self.backend,
            region: self.region.clone(),
            bucket: self.bucket_prefix.clone(),
            key,
        })
    }
}

// ---------------------------------------------------------------------------
// Audit trail
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<TenantId>,
    pub action: String,
    pub allowed: bool,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Control plane
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub tenant: TenantId,
    pub token_id: String,
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ControlPlane {
    tenants: BTreeMap<TenantId, Tenant>,
    tokens: BTreeMap<String, ApiToken>,
    usage: UsageLedger,
    router: StorageRouter,
    audit: Vec<AuditEvent>,
}

impl ControlPlane {
    pub fn new() -> Self {
        Self {
            tenants: BTreeMap::new(),
            tokens: BTreeMap::new(),
            usage: UsageLedger::default(),
            router: StorageRouter::default(),
            audit: Vec::new(),
        }
    }

    pub fn with_router(router: StorageRouter) -> Self {
        let mut plane = Self::new();
        plane.router = router;
        plane
    }

    pub fn router(&self) -> &StorageRouter {
        &self.router
    }

    pub fn set_router(&mut self, router: StorageRouter) {
        self.router = router;
    }

    pub fn tenants(&self) -> impl Iterator<Item = &Tenant> {
        self.tenants.values()
    }

    pub fn tenant(&self, id: &str) -> Option<&Tenant> {
        TenantId::parse(id)
            .ok()
            .and_then(|id| self.tenants.get(&id))
    }

    pub fn audit(&self) -> &[AuditEvent] {
        &self.audit
    }

    pub fn recent_audit(&self, limit: usize) -> Vec<&AuditEvent> {
        let skip = self.audit.len().saturating_sub(limit);
        self.audit.iter().skip(skip).collect()
    }

    fn log(
        &mut self,
        tenant: Option<TenantId>,
        action: impl Into<String>,
        allowed: bool,
        detail: impl Into<String>,
    ) {
        self.audit.push(AuditEvent {
            at: Utc::now(),
            tenant,
            action: action.into(),
            allowed,
            detail: detail.into(),
        });
        if self.audit.len() > 10_000 {
            let drain = self.audit.len() - 10_000;
            self.audit.drain(0..drain);
        }
    }

    pub fn create_tenant(
        &mut self,
        id: TenantId,
        display_name: impl Into<String>,
        tier: PlanTier,
    ) -> Result<Tenant, CloudError> {
        if self.tenants.contains_key(&id) {
            return Err(CloudError::Conflict(id.to_string()));
        }
        let tenant = Tenant::new(id.clone(), display_name, tier, Utc::now());
        self.tenants.insert(id.clone(), tenant.clone());
        self.log(Some(id), "tenant.create", true, format!("tier={tier:?}"));
        Ok(tenant)
    }

    pub fn set_tenant_status(
        &mut self,
        id: &TenantId,
        status: TenantStatus,
    ) -> Result<(), CloudError> {
        let tenant = self
            .tenants
            .get_mut(id)
            .ok_or_else(|| CloudError::TenantNotFound(id.to_string()))?;
        tenant.status = status;
        self.log(
            Some(id.clone()),
            "tenant.status",
            true,
            format!("status={status:?}"),
        );
        Ok(())
    }

    pub fn issue_token(
        &mut self,
        tenant_id: &TenantId,
        name: &str,
        scopes: Vec<Scope>,
        ttl_seconds: Option<i64>,
    ) -> Result<IssuedToken, CloudError> {
        if !self.tenants.contains_key(tenant_id) {
            return Err(CloudError::TenantNotFound(tenant_id.to_string()));
        }
        let issued = new_token(tenant_id.clone(), name, scopes, ttl_seconds, Utc::now())?;
        self.tokens
            .insert(issued.token.id.clone(), issued.token.clone());
        self.log(
            Some(tenant_id.clone()),
            "token.issue",
            true,
            format!("token={} scopes={:?}", issued.token.id, issued.token.scopes),
        );
        Ok(issued)
    }

    pub fn revoke_token(&mut self, token_id: &str) -> Result<(), CloudError> {
        let token = self
            .tokens
            .get_mut(token_id)
            .ok_or(CloudError::Unauthorized)?;
        token.revoked = true;
        let tenant = token.tenant.clone();
        self.log(
            Some(tenant),
            "token.revoke",
            true,
            format!("token={token_id}"),
        );
        Ok(())
    }

    pub fn tokens_for<'a>(
        &'a self,
        tenant: &'a TenantId,
    ) -> impl Iterator<Item = &'a ApiToken> + 'a {
        self.tokens.values().filter(move |t| &t.tenant == tenant)
    }

    /// Verify a token and required scope. Updates `last_used_at`.
    pub fn authorize(
        &mut self,
        plaintext: &str,
        required: Scope,
    ) -> Result<AuthContext, CloudError> {
        let now = Utc::now();
        let (_, id, secret) = split_token(plaintext).ok_or(CloudError::Unauthorized)?;
        let token = self.tokens.get(id).ok_or(CloudError::Unauthorized)?.clone();
        if !constant_time_eq(token.digest.as_bytes(), token_digest(secret).as_bytes()) {
            self.log(None, "token.auth", false, "secret mismatch");
            return Err(CloudError::Unauthorized);
        }
        if token.revoked {
            return Err(CloudError::TokenRevoked);
        }
        if token.expires_at.map(|e| e <= now).unwrap_or(false) {
            return Err(CloudError::TokenExpired);
        }
        let tenant = self
            .tenants
            .get(&token.tenant)
            .ok_or_else(|| CloudError::TenantNotFound(token.tenant.to_string()))?;
        if !tenant.active() {
            return Err(CloudError::TenantInactive(format!("{:?}", tenant.status)));
        }
        if !token.has_scope(required) {
            self.log(
                Some(token.tenant.clone()),
                "token.auth",
                false,
                format!("missing scope {required}"),
            );
            return Err(CloudError::ScopeDenied(required.to_string()));
        }
        if let Some(record) = self.tokens.get_mut(id) {
            record.last_used_at = Some(now);
        }
        Ok(AuthContext {
            tenant: token.tenant.clone(),
            token_id: token.id.clone(),
            scopes: token.scopes.clone(),
        })
    }

    pub fn record_usage(
        &mut self,
        plaintext: &str,
        event: UsageEvent,
    ) -> Result<UsageReceipt, CloudError> {
        let auth = self.authorize(plaintext, Scope::RemoteWrite)?;
        let tenant = self
            .tenants
            .get(&auth.tenant)
            .cloned()
            .ok_or_else(|| CloudError::TenantNotFound(auth.tenant.to_string()))?;
        let receipt = self.usage.record(&tenant, event)?;
        self.log(
            Some(auth.tenant),
            "usage.record",
            true,
            format!(
                "duplicate={} totals={:?}",
                receipt.duplicate, receipt.totals
            ),
        );
        Ok(receipt)
    }

    pub fn usage_totals(&self, tenant: &TenantId) -> UsageTotals {
        self.usage.totals(tenant)
    }

    pub fn route_artifact(
        &self,
        tenant: &TenantId,
        digest: &str,
    ) -> Result<StorageRoute, CloudError> {
        if !self.tenants.contains_key(tenant) {
            return Err(CloudError::TenantNotFound(tenant.to_string()));
        }
        self.router.route(tenant, digest)
    }

    /// In-memory control request handler. No network, no panics.
    pub fn handle(&mut self, request: Request) -> Response {
        if request.protocol != CONTROL_PROTOCOL_VERSION {
            return Response::error(CloudError::Protocol(request.protocol));
        }
        let method = request.method.to_ascii_uppercase();
        let path = request.path.trim_end_matches('/').to_string();
        match (method.as_str(), path.as_str()) {
            ("GET", "/v1/health") => Response::json(
                200,
                serde_json::json!({
                    "ok": true,
                    "protocol": CONTROL_PROTOCOL_VERSION,
                    "tenants": self.tenants.len(),
                }),
            ),
            ("POST", "/v1/tenants") => self.handle_create_tenant(request.body),
            ("POST", "/v1/tokens") => {
                self.handle_issue_token(request.token.as_deref(), request.body)
            }
            ("POST", "/v1/tokens/revoke") => {
                self.handle_revoke_token(request.token.as_deref(), request.body)
            }
            ("POST", "/v1/auth/check") => {
                self.handle_auth_check(request.token.as_deref(), request.body)
            }
            ("POST", "/v1/usage") => {
                self.handle_record_usage(request.token.as_deref(), request.body)
            }
            ("POST", "/v1/route") => self.handle_route(request.token.as_deref(), request.body),
            ("GET", p) if p.starts_with("/v1/usage/") => {
                let tenant = &p["/v1/usage/".len()..];
                self.handle_usage_summary(request.token.as_deref(), tenant)
            }
            _ => Response::error(CloudError::BadRequest(format!(
                "unknown route {method} {path}"
            ))),
        }
    }

    fn handle_create_tenant(&mut self, body: serde_json::Value) -> Response {
        #[derive(Deserialize)]
        struct Body {
            id: String,
            #[serde(default)]
            display_name: Option<String>,
            #[serde(default)]
            tier: PlanTier,
        }
        let body: Body = match serde_json::from_value(body) {
            Ok(body) => body,
            Err(e) => return Response::error(CloudError::BadRequest(e.to_string())),
        };
        let id = match TenantId::parse(body.id) {
            Ok(id) => id,
            Err(e) => return Response::error(e),
        };
        let display = body.display_name.unwrap_or_else(|| id.to_string());
        match self.create_tenant(id, display, body.tier) {
            Ok(tenant) => Response::json(
                201,
                serde_json::to_value(tenant).unwrap_or_else(|_| serde_json::json!({})),
            ),
            Err(e) => Response::error(e),
        }
    }

    fn handle_issue_token(&mut self, plaintext: Option<&str>, body: serde_json::Value) -> Response {
        #[derive(Deserialize)]
        struct Body {
            tenant: String,
            name: String,
            scopes: Vec<String>,
            #[serde(default)]
            ttl_seconds: Option<i64>,
        }
        let auth = match plaintext {
            Some(token) => match self.authorize(token, Scope::Admin) {
                Ok(auth) => auth,
                Err(e) => return Response::error(e),
            },
            None => return Response::error(CloudError::Unauthorized),
        };
        let body: Body = match serde_json::from_value(body) {
            Ok(body) => body,
            Err(e) => return Response::error(CloudError::BadRequest(e.to_string())),
        };
        let tenant = match TenantId::parse(body.tenant) {
            Ok(tenant) => tenant,
            Err(e) => return Response::error(e),
        };
        if tenant != auth.tenant {
            return Response::error(CloudError::ScopeDenied(
                "cannot issue tokens for another tenant".into(),
            ));
        }
        let mut scopes = Vec::new();
        for raw in body.scopes {
            match Scope::parse(&raw) {
                Some(scope) => scopes.push(scope),
                None => {
                    return Response::error(CloudError::BadRequest(format!(
                        "unknown scope `{raw}`"
                    )))
                }
            }
        }
        match self.issue_token(&tenant, &body.name, scopes, body.ttl_seconds) {
            Ok(issued) => Response::json(
                201,
                serde_json::to_value(issued).unwrap_or_else(|_| serde_json::json!({})),
            ),
            Err(e) => Response::error(e),
        }
    }

    fn handle_revoke_token(
        &mut self,
        plaintext: Option<&str>,
        body: serde_json::Value,
    ) -> Response {
        #[derive(Deserialize)]
        struct Body {
            token_id: String,
        }
        if let Some(token) = plaintext {
            if let Err(e) = self.authorize(token, Scope::Admin) {
                return Response::error(e);
            }
        } else {
            return Response::error(CloudError::Unauthorized);
        }
        let body: Body = match serde_json::from_value(body) {
            Ok(body) => body,
            Err(e) => return Response::error(CloudError::BadRequest(e.to_string())),
        };
        match self.revoke_token(&body.token_id) {
            Ok(()) => Response::json(200, serde_json::json!({"revoked": body.token_id})),
            Err(e) => Response::error(e),
        }
    }

    fn handle_auth_check(&mut self, plaintext: Option<&str>, body: serde_json::Value) -> Response {
        #[derive(Deserialize)]
        struct Body {
            scope: String,
        }
        let Some(token) = plaintext else {
            return Response::error(CloudError::Unauthorized);
        };
        let body: Body = match serde_json::from_value(body) {
            Ok(body) => body,
            Err(e) => return Response::error(CloudError::BadRequest(e.to_string())),
        };
        let Some(scope) = Scope::parse(&body.scope) else {
            return Response::error(CloudError::BadRequest(format!(
                "unknown scope `{}`",
                body.scope
            )));
        };
        match self.authorize(token, scope) {
            Ok(auth) => Response::json(
                200,
                serde_json::to_value(auth).unwrap_or_else(|_| serde_json::json!({})),
            ),
            Err(e) => Response::error(e),
        }
    }

    fn handle_record_usage(
        &mut self,
        plaintext: Option<&str>,
        body: serde_json::Value,
    ) -> Response {
        #[derive(Deserialize)]
        struct Body {
            kind: String,
            quantity: u64,
            idempotency_key: String,
            #[serde(default)]
            at: Option<DateTime<Utc>>,
        }
        let Some(token) = plaintext else {
            return Response::error(CloudError::Unauthorized);
        };
        let body: Body = match serde_json::from_value(body) {
            Ok(body) => body,
            Err(e) => return Response::error(CloudError::BadRequest(e.to_string())),
        };
        let Some(kind) = UsageKind::parse(&body.kind) else {
            return Response::error(CloudError::BadRequest(format!(
                "unknown usage kind `{}`",
                body.kind
            )));
        };
        if body.idempotency_key.trim().is_empty() {
            return Response::error(CloudError::BadRequest(
                "idempotency_key must not be empty".into(),
            ));
        }
        let event = UsageEvent {
            idempotency_key: body.idempotency_key,
            kind,
            quantity: body.quantity,
            at: body.at.unwrap_or_else(Utc::now),
        };
        match self.record_usage(token, event) {
            Ok(receipt) => Response::json(
                200,
                serde_json::to_value(receipt).unwrap_or_else(|_| serde_json::json!({})),
            ),
            Err(e) => Response::error(e),
        }
    }

    fn handle_usage_summary(&mut self, plaintext: Option<&str>, tenant: &str) -> Response {
        let Some(token) = plaintext else {
            return Response::error(CloudError::Unauthorized);
        };
        let auth = match self.authorize(token, Scope::UsageRead) {
            Ok(auth) => auth,
            Err(e) => return Response::error(e),
        };
        let tenant = match TenantId::parse(tenant) {
            Ok(tenant) => tenant,
            Err(e) => return Response::error(e),
        };
        if tenant != auth.tenant && !auth.scopes.contains(&Scope::Admin) {
            return Response::error(CloudError::ScopeDenied(
                "cannot read another tenant's usage".into(),
            ));
        }
        let totals = self.usage_totals(&tenant);
        Response::json(200, serde_json::json!({"tenant": tenant, "totals": totals}))
    }

    fn handle_route(&mut self, plaintext: Option<&str>, body: serde_json::Value) -> Response {
        #[derive(Deserialize)]
        struct Body {
            digest: String,
        }
        let Some(token) = plaintext else {
            return Response::error(CloudError::Unauthorized);
        };
        let auth = match self.authorize(token, Scope::RemoteRead) {
            Ok(auth) => auth,
            Err(e) => return Response::error(e),
        };
        let body: Body = match serde_json::from_value(body) {
            Ok(body) => body,
            Err(e) => return Response::error(CloudError::BadRequest(e.to_string())),
        };
        match self.route_artifact(&auth.tenant, &body.digest) {
            Ok(route) => Response::json(
                200,
                serde_json::to_value(route).unwrap_or_else(|_| serde_json::json!({})),
            ),
            Err(e) => Response::error(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Protocol envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    #[serde(default = "default_protocol")]
    pub protocol: u32,
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub body: serde_json::Value,
}

fn default_protocol() -> u32 {
    CONTROL_PROTOCOL_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    pub body: serde_json::Value,
}

impl Response {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Self { status, body }
    }

    pub fn error(error: CloudError) -> Self {
        Self {
            status: error.status(),
            body: serde_json::json!({
                "error": {
                    "code": error.code(),
                    "message": error.to_string(),
                }
            }),
        }
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant_id() -> TenantId {
        TenantId::parse("acme-labs").unwrap()
    }

    fn plane_with_tenant() -> ControlPlane {
        let mut plane = ControlPlane::new();
        plane
            .create_tenant(tenant_id(), "Acme Labs", PlanTier::Free)
            .unwrap();
        plane
    }

    #[test]
    fn tenant_ids_are_strict() {
        assert!(TenantId::parse("acme-labs").is_ok());
        assert!(TenantId::parse("ab").is_err());
        assert!(TenantId::parse("Acme").is_err());
        assert!(TenantId::parse("-acme").is_err());
        assert!(TenantId::parse("acme--labs").is_err());
    }

    #[test]
    fn token_digest_is_not_plaintext_and_verifies() {
        let issued =
            new_token(tenant_id(), "ci", vec![Scope::RemoteRead], None, Utc::now()).unwrap();
        assert!(issued.plaintext.starts_with(TOKEN_PREFIX));
        assert_ne!(issued.token.digest, issued.plaintext);
        let (_, id, secret) = split_token(&issued.plaintext).unwrap();
        assert_eq!(id, issued.token.id);
        assert_eq!(issued.token.digest, token_digest(secret));
    }

    #[test]
    fn authorization_enforces_scope_and_expiry() {
        let mut plane = plane_with_tenant();
        let issued = plane
            .issue_token(&tenant_id(), "reader", vec![Scope::RemoteRead], None)
            .unwrap();
        assert!(plane
            .authorize(&issued.plaintext, Scope::RemoteRead)
            .is_ok());
        assert!(matches!(
            plane.authorize(&issued.plaintext, Scope::RemoteWrite),
            Err(CloudError::ScopeDenied(_))
        ));
        assert!(matches!(
            plane.authorize(&issued.plaintext, Scope::Admin),
            Err(CloudError::ScopeDenied(_))
        ));
        assert!(matches!(
            plane.authorize("jhc1.deadbeef.not-a-secret", Scope::RemoteRead),
            Err(CloudError::Unauthorized)
        ));

        let expired = plane
            .issue_token(&tenant_id(), "old", vec![Scope::Admin], Some(1))
            .unwrap();
        let mut record = expired.token.clone();
        record.expires_at = Some(Utc::now() - Duration::seconds(1));
        plane.tokens.insert(record.id.clone(), record);
        assert!(matches!(
            plane.authorize(&expired.plaintext, Scope::Admin),
            Err(CloudError::TokenExpired)
        ));
    }

    #[test]
    fn admin_scope_implies_all() {
        let mut plane = plane_with_tenant();
        let issued = plane
            .issue_token(&tenant_id(), "admin", vec![Scope::Admin], None)
            .unwrap();
        assert!(plane
            .authorize(&issued.plaintext, Scope::RemoteWrite)
            .is_ok());
        assert!(plane.authorize(&issued.plaintext, Scope::UsageRead).is_ok());
    }

    #[test]
    fn revoked_tokens_fail() {
        let mut plane = plane_with_tenant();
        let issued = plane
            .issue_token(&tenant_id(), "temp", vec![Scope::RemoteRead], None)
            .unwrap();
        plane.revoke_token(&issued.token.id).unwrap();
        assert!(matches!(
            plane.authorize(&issued.plaintext, Scope::RemoteRead),
            Err(CloudError::TokenRevoked)
        ));
    }

    #[test]
    fn quota_rejects_excess_upload_and_is_idempotent() {
        let mut plane = plane_with_tenant();
        let issued = plane
            .issue_token(&tenant_id(), "writer", vec![Scope::RemoteWrite], None)
            .unwrap();
        let event = UsageEvent {
            idempotency_key: "upload-1".into(),
            kind: UsageKind::UploadBytes,
            quantity: 10,
            at: Utc::now(),
        };
        let first = plane
            .record_usage(&issued.plaintext, event.clone())
            .unwrap();
        assert!(!first.duplicate);
        assert_eq!(first.totals.upload_bytes, 10);

        let replay = plane.record_usage(&issued.plaintext, event).unwrap();
        assert!(replay.duplicate);
        assert_eq!(replay.totals.upload_bytes, 10);

        let too_much = UsageEvent {
            idempotency_key: "upload-2".into(),
            kind: UsageKind::UploadBytes,
            quantity: Quotas::for_tier(PlanTier::Free).upload_bytes_per_day,
            at: Utc::now(),
        };
        assert!(matches!(
            plane.record_usage(&issued.plaintext, too_much),
            Err(CloudError::QuotaExceeded(_))
        ));
    }

    #[test]
    fn request_rate_quota_uses_trailing_minute() {
        let mut plane = plane_with_tenant();
        let issued = plane
            .issue_token(&tenant_id(), "writer", vec![Scope::RemoteWrite], None)
            .unwrap();
        let quota = Quotas::for_tier(PlanTier::Free).requests_per_minute;
        let now = Utc::now();
        for i in 0..quota {
            plane
                .record_usage(
                    &issued.plaintext,
                    UsageEvent {
                        idempotency_key: format!("r-{i}"),
                        kind: UsageKind::Request,
                        quantity: 1,
                        at: now,
                    },
                )
                .unwrap();
        }
        assert!(matches!(
            plane.record_usage(
                &issued.plaintext,
                UsageEvent {
                    idempotency_key: "r-over".into(),
                    kind: UsageKind::Request,
                    quantity: 1,
                    at: now,
                }
            ),
            Err(CloudError::QuotaExceeded(_))
        ));
    }

    #[test]
    fn routing_is_deterministic_and_tenant_scoped() {
        let plane = plane_with_tenant();
        let digest = "a".repeat(64);
        let a = plane.route_artifact(&tenant_id(), &digest).unwrap();
        let b = plane.route_artifact(&tenant_id(), &digest).unwrap();
        assert_eq!(a, b);
        assert!(a.key.starts_with("tenants/acme-labs/cas/aa/aa/"));
        assert!(a.key.ends_with(&digest));

        let other = TenantId::parse("other-co").unwrap();
        assert!(matches!(
            plane.route_artifact(&other, &digest),
            Err(CloudError::TenantNotFound(_))
        ));
        assert!(matches!(
            plane.route_artifact(&tenant_id(), "not-hex"),
            Err(CloudError::InvalidDigest(_))
        ));
    }

    #[test]
    fn protocol_flow_create_issue_record_and_route() {
        let mut plane = ControlPlane::new();
        let created = plane.handle(Request {
            protocol: CONTROL_PROTOCOL_VERSION,
            method: "POST".into(),
            path: "/v1/tenants".into(),
            token: None,
            body: serde_json::json!({"id":"acme-labs","display_name":"Acme","tier":"team"}),
        });
        assert_eq!(created.status, 201);

        // Bootstrap admin token is issued out-of-band for an existing tenant.
        let admin = plane
            .issue_token(&tenant_id(), "root", vec![Scope::Admin], None)
            .unwrap();

        let issued = plane.handle(Request {
            protocol: CONTROL_PROTOCOL_VERSION,
            method: "POST".into(),
            path: "/v1/tokens".into(),
            token: Some(admin.plaintext.clone()),
            body: serde_json::json!({
                "tenant":"acme-labs",
                "name":"ci",
                "scopes":["remote_read","remote_write","usage_read"],
            }),
        });
        assert_eq!(issued.status, 201);
        let plaintext = issued.body["plaintext"].as_str().unwrap().to_string();
        assert!(plaintext.starts_with(TOKEN_PREFIX));

        let usage = plane.handle(Request {
            protocol: CONTROL_PROTOCOL_VERSION,
            method: "POST".into(),
            path: "/v1/usage".into(),
            token: Some(plaintext.clone()),
            body: serde_json::json!({
                "kind":"request",
                "quantity":1,
                "idempotency_key":"req-1",
            }),
        });
        assert_eq!(usage.status, 200);
        assert_eq!(usage.body["totals"]["requests"], 1);

        let route = plane.handle(Request {
            protocol: CONTROL_PROTOCOL_VERSION,
            method: "POST".into(),
            path: "/v1/route".into(),
            token: Some(plaintext.clone()),
            body: serde_json::json!({"digest":"b".repeat(64)}),
        });
        assert_eq!(route.status, 200);
        assert_eq!(route.body["tenant"], "acme-labs");

        let summary = plane.handle(Request {
            protocol: CONTROL_PROTOCOL_VERSION,
            method: "GET".into(),
            path: "/v1/usage/acme-labs".into(),
            token: Some(plaintext),
            body: serde_json::json!({}),
        });
        assert_eq!(summary.status, 200);
        assert_eq!(summary.body["totals"]["requests"], 1);
    }

    #[test]
    fn protocol_rejects_unknown_version_and_routes() {
        let mut plane = ControlPlane::new();
        let response = plane.handle(Request {
            protocol: 99,
            method: "GET".into(),
            path: "/v1/health".into(),
            token: None,
            body: serde_json::json!({}),
        });
        assert_eq!(response.status, 400);
        assert_eq!(response.body["error"]["code"], "unsupported_protocol");

        let response = plane.handle(Request {
            protocol: CONTROL_PROTOCOL_VERSION,
            method: "GET".into(),
            path: "/v1/nope".into(),
            token: None,
            body: serde_json::json!({}),
        });
        assert_eq!(response.status, 400);
    }

    #[test]
    fn cross_tenant_usage_read_is_denied() {
        let mut plane = plane_with_tenant();
        plane
            .create_tenant(
                TenantId::parse("other-co").unwrap(),
                "Other",
                PlanTier::Free,
            )
            .unwrap();
        let issued = plane
            .issue_token(&tenant_id(), "usage", vec![Scope::UsageRead], None)
            .unwrap();
        let response = plane.handle(Request {
            protocol: CONTROL_PROTOCOL_VERSION,
            method: "GET".into(),
            path: "/v1/usage/other-co".into(),
            token: Some(issued.plaintext),
            body: serde_json::json!({}),
        });
        assert_eq!(response.status, 403);
    }

    #[test]
    fn suspended_tenant_cannot_authorize() {
        let mut plane = plane_with_tenant();
        let issued = plane
            .issue_token(&tenant_id(), "reader", vec![Scope::RemoteRead], None)
            .unwrap();
        plane
            .set_tenant_status(&tenant_id(), TenantStatus::Suspended)
            .unwrap();
        assert!(matches!(
            plane.authorize(&issued.plaintext, Scope::RemoteRead),
            Err(CloudError::TenantInactive(_))
        ));
    }
}
