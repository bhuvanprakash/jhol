//! Optional remote CAS / action-cache client (blueprint Phase D).
//!
//! Local-only installs must work with this crate unused. No credentials in
//! serialized objects. Network I/O is behind `RemoteClient::fetch` and is a
//! optional S3-compatible transport; local-only installs do not use this crate.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub mod s3;

/// Wire object matching `schemas/remote-cas-v1.schema.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteCasObject {
    pub format_version: u32,
    pub kind: String,
    pub content_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemoteError {
    #[error("digest mismatch: expected {expected}, got {got}")]
    DigestMismatch { expected: String, got: String },
    #[error("remote not configured (set JHOL_REMOTE_CAS_URL to enable)")]
    NotConfigured,
    #[error("unsupported format_version {0}")]
    BadVersion(u32),
    #[error("tenant isolation: object tenant {object:?} != client {client:?}")]
    TenantMismatch {
        object: Option<String>,
        client: Option<String>,
    },
    #[error("remote object not found")]
    NotFound,
    #[error("remote object exceeds size limit ({size} bytes; limit {limit})")]
    ObjectTooLarge { size: u64, limit: u64 },
    #[error("remote object size mismatch: expected {expected}, got {got}")]
    SizeMismatch { expected: u64, got: u64 },
    #[error("invalid remote configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("invalid remote object: {0}")]
    InvalidObject(&'static str),
    #[error("remote service returned HTTP {0}")]
    HttpStatus(u16),
    #[error("remote transport failed: {0}")]
    Transport(String),
}

/// Content-addressed digest (sha256 hex) of raw bytes — verification before trust.
pub fn digest_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// Fail closed if object digest does not match payload bytes.
pub fn verify_object(obj: &RemoteCasObject, payload: &[u8]) -> Result<(), RemoteError> {
    if obj.format_version != 1 {
        return Err(RemoteError::BadVersion(obj.format_version));
    }
    let got = digest_bytes(payload);
    if got != obj.content_digest {
        return Err(RemoteError::DigestMismatch {
            expected: obj.content_digest.clone(),
            got,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct RemoteClient {
    pub endpoint: Option<String>,
    pub tenant: Option<String>,
}

impl RemoteClient {
    pub fn from_env() -> Self {
        Self {
            endpoint: std::env::var("JHOL_REMOTE_CAS_URL").ok(),
            tenant: std::env::var("JHOL_REMOTE_TENANT").ok(),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.endpoint
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    /// Upload verified content to the optional S3-compatible CAS.
    pub fn put(&self, obj: &RemoteCasObject, payload: &[u8]) -> Result<(), RemoteError> {
        self.check_tenant(obj)?;
        s3::S3Client::from_env(self.endpoint.as_deref(), self.tenant.as_deref())?.put(obj, payload)
    }

    /// Fetch content and verify its digest and declared size before returning it.
    pub fn fetch(&self, obj: &RemoteCasObject) -> Result<Vec<u8>, RemoteError> {
        self.check_tenant(obj)?;
        s3::S3Client::from_env(self.endpoint.as_deref(), self.tenant.as_deref())?.get(obj)
    }

    fn check_tenant(&self, obj: &RemoteCasObject) -> Result<(), RemoteError> {
        if !self.is_configured() {
            return Err(RemoteError::NotConfigured);
        }
        if obj.tenant != self.tenant || obj.tenant.as_deref().unwrap_or_default().is_empty() {
            return Err(RemoteError::TenantMismatch {
                object: obj.tenant.clone(),
                client: self.tenant.clone(),
            });
        }
        Ok(())
    }

    /// Stub fetch: verifies tenant + digest only. Real HTTP arrives later.
    pub fn accept_local_blob(
        &self,
        obj: &RemoteCasObject,
        payload: &[u8],
    ) -> Result<(), RemoteError> {
        self.check_tenant(obj)?;
        verify_object(obj, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_rejects_tampered_payload() {
        let payload = b"hello-cas";
        let obj = RemoteCasObject {
            format_version: 1,
            kind: "blob".into(),
            content_digest: digest_bytes(payload),
            action_key: None,
            tenant: Some("t1".into()),
            size_bytes: Some(payload.len() as u64),
        };
        assert!(verify_object(&obj, payload).is_ok());
        assert!(matches!(
            verify_object(&obj, b"tampered"),
            Err(RemoteError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn unconfigured_client_refuses() {
        let c = RemoteClient::default();
        let payload = b"x";
        let obj = RemoteCasObject {
            format_version: 1,
            kind: "blob".into(),
            content_digest: digest_bytes(payload),
            action_key: None,
            tenant: None,
            size_bytes: None,
        };
        assert_eq!(
            c.accept_local_blob(&obj, payload),
            Err(RemoteError::NotConfigured)
        );
    }

    #[test]
    fn tenant_mismatch_fail_closed() {
        let c = RemoteClient {
            endpoint: Some("https://cas.example".into()),
            tenant: Some("a".into()),
        };
        let payload = b"x";
        let obj = RemoteCasObject {
            format_version: 1,
            kind: "blob".into(),
            content_digest: digest_bytes(payload),
            action_key: None,
            tenant: Some("b".into()),
            size_bytes: None,
        };
        assert!(matches!(
            c.accept_local_blob(&obj, payload),
            Err(RemoteError::TenantMismatch { .. })
        ));
    }
}
