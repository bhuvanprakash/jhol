//! Minimal path-style S3-compatible CAS transport using AWS Signature V4.
//!
//! Objects are addressed by tenant and SHA-256 digest. Downloads are bounded
//! and verified before the bytes are returned to a caller that may publish
//! them into a local store.

use std::{io::Read, thread, time::Duration};

use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{digest_bytes, verify_object, RemoteCasObject, RemoteError};

type HmacSha256 = Hmac<Sha256>;

const MAX_OBJECT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ATTEMPTS: usize = 3;

/// Optional S3-compatible backend. Credentials are private and omitted from
/// formatting/serialization; construct from environment, never from a URL.
pub struct S3Client {
    endpoint: Url,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    tenant: String,
    agent: ureq::Agent,
}

impl S3Client {
    pub fn from_env(endpoint: Option<&str>, tenant: Option<&str>) -> Result<Self, RemoteError> {
        let endpoint = match endpoint {
            Some(endpoint) => endpoint.to_owned(),
            None => std::env::var("JHOL_REMOTE_CAS_URL")
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or(RemoteError::NotConfigured)?,
        };
        let tenant = tenant
            .map(str::to_owned)
            .or_else(|| std::env::var("JHOL_REMOTE_TENANT").ok())
            .ok_or(RemoteError::NotConfigured)?;
        let bucket = required_env("JHOL_REMOTE_S3_BUCKET")?;
        let region = required_env("JHOL_REMOTE_S3_REGION")?;
        let access_key = required_env("JHOL_REMOTE_S3_ACCESS_KEY_ID")?;
        let secret_key = required_env("JHOL_REMOTE_S3_SECRET_ACCESS_KEY")?;
        let session_token = std::env::var("JHOL_REMOTE_S3_SESSION_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        Self::new(
            &endpoint,
            &bucket,
            &region,
            &access_key,
            &secret_key,
            session_token.as_deref(),
            &tenant,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: &str,
        bucket: &str,
        region: &str,
        access_key: &str,
        secret_key: &str,
        session_token: Option<&str>,
        tenant: &str,
    ) -> Result<Self, RemoteError> {
        let endpoint = Url::parse(endpoint)
            .map_err(|_| RemoteError::InvalidConfiguration("endpoint URL is invalid"))?;
        let host = endpoint
            .host_str()
            .ok_or(RemoteError::InvalidConfiguration(
                "endpoint host is missing",
            ))?;
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && loopback) {
            return Err(RemoteError::InvalidConfiguration(
                "endpoint must use HTTPS (HTTP is allowed only on loopback)",
            ));
        }
        if endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            return Err(RemoteError::InvalidConfiguration(
                "endpoint cannot contain query, fragment, or user information",
            ));
        }
        if !valid_bucket(bucket) {
            return Err(RemoteError::InvalidConfiguration("bucket name is invalid"));
        }
        if region.is_empty()
            || !region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(RemoteError::InvalidConfiguration("region is invalid"));
        }
        if access_key.is_empty()
            || secret_key.is_empty()
            || has_line_break(access_key)
            || has_line_break(secret_key)
            || session_token.is_some_and(has_line_break)
        {
            return Err(RemoteError::NotConfigured);
        }
        if !valid_segment(tenant) {
            return Err(RemoteError::InvalidConfiguration(
                "tenant identifier is invalid",
            ));
        }

        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .redirects(0)
            .build();
        Ok(Self {
            endpoint,
            bucket: bucket.to_owned(),
            region: region.to_owned(),
            access_key: access_key.to_owned(),
            secret_key: secret_key.to_owned(),
            session_token: session_token.map(str::to_owned),
            tenant: tenant.to_owned(),
            agent,
        })
    }

    pub fn put(&self, obj: &RemoteCasObject, payload: &[u8]) -> Result<(), RemoteError> {
        self.validate_object(obj)?;
        verify_object(obj, payload)?;
        if let Some(expected) = obj.size_bytes {
            if expected != payload.len() as u64 {
                return Err(RemoteError::SizeMismatch {
                    expected,
                    got: payload.len() as u64,
                });
            }
        }
        self.check_size(obj, payload.len() as u64)?;
        let url = self.object_url(obj)?;
        let response = self.request("PUT", &url, payload)?;
        if !(200..300).contains(&response.status()) {
            return Err(RemoteError::HttpStatus(response.status()));
        }
        Ok(())
    }

    pub fn get(&self, obj: &RemoteCasObject) -> Result<Vec<u8>, RemoteError> {
        self.validate_object(obj)?;
        if let Some(size) = obj.size_bytes {
            self.check_size(obj, size)?;
        }
        for attempt in 0..MAX_ATTEMPTS {
            match self.get_once(obj) {
                Err(RemoteError::Transport(_)) if attempt + 1 < MAX_ATTEMPTS => {
                    sleep_backoff(attempt, None);
                }
                result => return result,
            }
        }
        Err(RemoteError::Transport("retry limit exhausted".into()))
    }

    fn get_once(&self, obj: &RemoteCasObject) -> Result<Vec<u8>, RemoteError> {
        let url = self.object_url(obj)?;
        let response = self.request("GET", &url, &[])?;
        if response.status() == 404 {
            return Err(RemoteError::NotFound);
        }
        if !(200..300).contains(&response.status()) {
            return Err(RemoteError::HttpStatus(response.status()));
        }
        if let Some(length) = response.header("Content-Length") {
            if let Ok(length) = length.parse::<u64>() {
                self.check_size(obj, length)?;
                if let Some(expected) = obj.size_bytes {
                    if length != expected {
                        return Err(RemoteError::SizeMismatch {
                            expected,
                            got: length,
                        });
                    }
                }
            }
        }

        let mut payload = Vec::new();
        response
            .into_reader()
            .take(MAX_OBJECT_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|error| RemoteError::Transport(error.to_string()))?;
        if payload.len() as u64 > MAX_OBJECT_BYTES {
            return Err(RemoteError::ObjectTooLarge {
                size: payload.len() as u64,
                limit: MAX_OBJECT_BYTES,
            });
        }
        if let Some(expected) = obj.size_bytes {
            if payload.len() as u64 != expected {
                return Err(RemoteError::SizeMismatch {
                    expected,
                    got: payload.len() as u64,
                });
            }
        }
        verify_object(obj, &payload)?;
        Ok(payload)
    }

    fn validate_object(&self, obj: &RemoteCasObject) -> Result<(), RemoteError> {
        if obj.format_version != 1 {
            return Err(RemoteError::BadVersion(obj.format_version));
        }
        if obj.tenant.as_deref() != Some(self.tenant.as_str()) {
            return Err(RemoteError::TenantMismatch {
                object: obj.tenant.clone(),
                client: Some(self.tenant.clone()),
            });
        }
        if obj.content_digest.len() != 64
            || !obj
                .content_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(RemoteError::InvalidObject(
                "content digest must be lowercase SHA-256 hex",
            ));
        }
        if !matches!(obj.kind.as_str(), "blob" | "action_result" | "receipt") {
            return Err(RemoteError::InvalidObject("kind is not in the v1 schema"));
        }
        Ok(())
    }

    fn check_size(&self, _obj: &RemoteCasObject, size: u64) -> Result<(), RemoteError> {
        if size > MAX_OBJECT_BYTES {
            return Err(RemoteError::ObjectTooLarge {
                size,
                limit: MAX_OBJECT_BYTES,
            });
        }
        Ok(())
    }

    fn object_url(&self, obj: &RemoteCasObject) -> Result<Url, RemoteError> {
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .map_err(|_| RemoteError::InvalidConfiguration("endpoint cannot be a base URL"))?
            .pop_if_empty()
            .push(&self.bucket)
            .push("tenants")
            .push(&self.tenant)
            .push("sha256")
            .push(&obj.content_digest);
        Ok(url)
    }

    fn request(
        &self,
        method: &str,
        url: &Url,
        payload: &[u8],
    ) -> Result<ureq::Response, RemoteError> {
        let payload_hash = digest_bytes(payload);
        for attempt in 0..MAX_ATTEMPTS {
            let now = Utc::now();
            let date = now.format("%Y%m%dT%H%M%SZ").to_string();
            let short_date = now.format("%Y%m%d").to_string();
            let authorization =
                self.authorization(method, url, &payload_hash, &date, &short_date)?;
            let request = if method == "PUT" {
                self.agent.put(url.as_str())
            } else {
                self.agent.get(url.as_str())
            }
            .set("x-amz-content-sha256", &payload_hash)
            .set("x-amz-date", &date)
            .set("Authorization", &authorization);
            let request = match &self.session_token {
                Some(token) => request.set("x-amz-security-token", token),
                None => request,
            };
            let result = if method == "PUT" {
                request.send_bytes(payload)
            } else {
                request.call()
            };
            match result {
                Ok(response) => return Ok(response),
                Err(ureq::Error::Status(status, response)) => {
                    if status == 404 && method == "GET" {
                        return Err(RemoteError::NotFound);
                    }
                    if retryable(status) && attempt + 1 < MAX_ATTEMPTS {
                        sleep_backoff(attempt, response.header("Retry-After"));
                        continue;
                    }
                    return Err(RemoteError::HttpStatus(status));
                }
                Err(ureq::Error::Transport(error)) => {
                    if attempt + 1 < MAX_ATTEMPTS {
                        sleep_backoff(attempt, None);
                        continue;
                    }
                    return Err(RemoteError::Transport(error.to_string()));
                }
            }
        }
        Err(RemoteError::Transport("retry limit exhausted".into()))
    }

    fn authorization(
        &self,
        method: &str,
        url: &Url,
        payload_hash: &str,
        date: &str,
        short_date: &str,
    ) -> Result<String, RemoteError> {
        let host = match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_owned(),
            _ => {
                return Err(RemoteError::InvalidConfiguration(
                    "endpoint host is missing",
                ))
            }
        };
        let mut canonical_headers =
            format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{date}\n");
        let mut signed_headers = "host;x-amz-content-sha256;x-amz-date".to_owned();
        if let Some(token) = &self.session_token {
            canonical_headers.push_str(&format!("x-amz-security-token:{token}\n"));
            signed_headers.push_str(";x-amz-security-token");
        }
        let canonical_request = format!(
            "{method}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
            url.path(),
            url.query().unwrap_or_default()
        );
        let scope = format!("{short_date}/{}/s3/aws4_request", self.region);
        let request_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let string_to_sign = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{request_hash}");
        let date_key = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            short_date.as_bytes(),
        )?;
        let region_key = hmac_sha256(&date_key, self.region.as_bytes())?;
        let service_key = hmac_sha256(&region_key, b"s3")?;
        let signing_key = hmac_sha256(&service_key, b"aws4_request")?;
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes())?);
        Ok(format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        ))
    }
}

fn required_env(name: &'static str) -> Result<String, RemoteError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(RemoteError::InvalidConfiguration(name))
}

fn valid_bucket(value: &str) -> bool {
    (3..=63).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && !value.contains("..")
        && !value.contains(".-")
        && !value.contains("-.")
        && value.parse::<std::net::Ipv4Addr>().is_err()
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && value != "."
        && value != ".."
}

fn has_line_break(value: &str) -> bool {
    value.bytes().any(|b| b == b'\r' || b == b'\n')
}

fn hmac_sha256(key: &[u8], value: &[u8]) -> Result<Vec<u8>, RemoteError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| RemoteError::InvalidConfiguration("HMAC key is invalid"))?;
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn retryable(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

fn sleep_backoff(attempt: usize, retry_after: Option<&str>) {
    let delay = retry_after
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(2)))
        .unwrap_or_else(|| Duration::from_millis(200 * (1 << attempt.min(3))));
    thread::sleep(delay);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "client closed before complete request");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let body_start = end + 4;
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if bytes.len() >= body_start + content_length {
                    return bytes;
                }
            }
        }
    }

    fn reply(stream: &mut TcpStream, status: &str, body: &[u8], extra: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn tenant_and_digest_form_confined_path() {
        let client = S3Client::new(
            "https://storage.example/base",
            "jhol-cache",
            "us-east-1",
            "access",
            "secret",
            None,
            "team-a",
        )
        .unwrap();
        let obj = RemoteCasObject {
            format_version: 1,
            kind: "blob".into(),
            content_digest: "a".repeat(64),
            action_key: None,
            tenant: Some("team-a".into()),
            size_bytes: None,
        };
        assert_eq!(
            client.object_url(&obj).unwrap().path(),
            format!("/base/jhol-cache/tenants/team-a/sha256/{}", "a".repeat(64))
        );
    }

    #[test]
    fn rejects_non_tls_remote_and_path_injection() {
        assert!(matches!(
            S3Client::new(
                "http://storage.example",
                "jhol-cache",
                "r1",
                "a",
                "s",
                None,
                "t1"
            ),
            Err(RemoteError::InvalidConfiguration(_))
        ));
        assert!(matches!(
            S3Client::new(
                "https://storage.example",
                "jhol-cache",
                "r1",
                "a",
                "s",
                None,
                "../t1"
            ),
            Err(RemoteError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn signer_matches_aws_s3_sigv4_example() {
        let client = S3Client::new(
            "https://examplebucket.s3.amazonaws.com",
            "examplebucket",
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            None,
            "tenant-1",
        )
        .unwrap();
        let url = Url::parse("https://examplebucket.s3.amazonaws.com/?lifecycle=").unwrap();
        let auth = client
            .authorization(
                "GET",
                &url,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "20130524T000000Z",
                "20130524",
            )
            .unwrap();
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
    }

    #[test]
    fn loopback_put_fetch_and_throttle_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for (index, response) in [
                ("200 OK", b"".as_slice()),
                ("429 Too Many Requests", b"".as_slice()),
                ("200 OK", b"verified bytes".as_slice()),
            ]
            .into_iter()
            .enumerate()
            {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let request_text = String::from_utf8_lossy(&request).to_ascii_lowercase();
                assert!(request_text.contains("authorization: aws4-hmac-sha256"));
                assert!(request_text.contains("/jhol-cache/tenants/team-a/sha256/"));
                if index == 0 {
                    assert!(request_text.starts_with("put "));
                } else {
                    assert!(request_text.starts_with("get "));
                }
                if index == 1 {
                    reply(&mut stream, response.0, response.1, "Retry-After: 0\r\n");
                } else {
                    reply(&mut stream, response.0, response.1, "");
                }
            }
        });
        let client = S3Client::new(
            &format!("http://{address}/s3"),
            "jhol-cache",
            "us-east-1",
            "access",
            "secret",
            None,
            "team-a",
        )
        .unwrap();
        let payload = b"verified bytes";
        let obj = RemoteCasObject {
            format_version: 1,
            kind: "blob".into(),
            content_digest: digest_bytes(payload),
            action_key: None,
            tenant: Some("team-a".into()),
            size_bytes: Some(payload.len() as u64),
        };
        client.put(&obj, payload).unwrap();
        assert_eq!(client.get(&obj).unwrap(), payload);
        server.join().unwrap();
    }

    #[test]
    fn fetch_retries_a_mid_body_disconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let _ = read_request(&mut first);
            write!(
                first,
                "HTTP/1.1 200 OK\r\nContent-Length: 14\r\nConnection: close\r\n\r\nshort"
            )
            .unwrap();
            first.flush().unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            assert!(String::from_utf8_lossy(&read_request(&mut second)).starts_with("GET "));
            reply(&mut second, "200 OK", b"verified bytes", "");
        });
        let client = S3Client::new(
            &format!("http://{address}"),
            "jhol-cache",
            "us-east-1",
            "access",
            "secret",
            None,
            "team-a",
        )
        .unwrap();
        let payload = b"verified bytes";
        let obj = RemoteCasObject {
            format_version: 1,
            kind: "blob".into(),
            content_digest: digest_bytes(payload),
            action_key: None,
            tenant: Some("team-a".into()),
            size_bytes: Some(payload.len() as u64),
        };
        assert_eq!(client.get(&obj).unwrap(), payload);
        server.join().unwrap();
    }

    #[test]
    fn fetch_rejects_poisoned_remote_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            assert!(String::from_utf8_lossy(&request).starts_with("GET "));
            reply(&mut stream, "200 OK", b"poisoned", "");
        });
        let client = S3Client::new(
            &format!("http://{address}"),
            "jhol-cache",
            "us-east-1",
            "access",
            "secret",
            None,
            "team-a",
        )
        .unwrap();
        let expected = b"verified";
        let obj = RemoteCasObject {
            format_version: 1,
            kind: "blob".into(),
            content_digest: digest_bytes(expected),
            action_key: None,
            tenant: Some("team-a".into()),
            size_bytes: Some(expected.len() as u64),
        };
        assert!(matches!(
            client.get(&obj),
            Err(RemoteError::DigestMismatch { .. })
        ));
        server.join().unwrap();
    }
}
