//! AI / model artifact pins (blueprint §12 / Phase C).
//! Content-addressed by exact revision + per-file digests. No download here.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("digest mismatch for {path}: expected {expected}, got {got}")]
    DigestMismatch {
        path: String,
        expected: String,
        got: String,
    },
    #[error("missing file: {0}")]
    Missing(String),
    #[error("network access requires explicit approval")]
    NetworkPermissionRequired,
    #[error("invalid model manifest: {0}")]
    InvalidManifest(String),
    #[error("download failed: {0}")]
    Download(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilePin {
    pub path: String,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelArtifact {
    #[serde(default = "model_schema_v1")]
    pub schema: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Aggregate digest over sorted file pins.
    pub digest: String,
    pub files: Vec<FilePin>,
}

fn model_schema_v1() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadedFile {
    pub path: String,
    pub digest: String,
    pub size_bytes: u64,
    pub bytes_received: u64,
    pub resumed: bool,
    pub already_present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadReceipt {
    pub source: String,
    pub revision: String,
    pub files: Vec<DownloadedFile>,
    pub bytes_received: u64,
    pub cleaned_partials: Vec<String>,
}

const LINEAR_CONFIG_LIMIT: u64 = 16 * 1024;
const LINEAR_ARTIFACT_LIMIT: u64 = 256 * 1024;
const MODEL_MANIFEST_LIMIT: u64 = 256 * 1024;
const LINEAR_MAX_FEATURES: usize = 64;
const LINEAR_MAX_CLASSES: usize = 16;
const LINEAR_MAX_INPUT_ABS: f64 = 1000.0;
const LINEAR_WEIGHTS_MAGIC: &[u8; 8] = b"JHLIN001";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinearModelConfig {
    schema: u32,
    format: String,
    name: String,
    runtime: String,
    weights: String,
    input_features: usize,
    labels: Vec<String>,
}

/// Output from Jhol's small, deterministic CPU linear-classifier runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InferenceResult {
    pub model: String,
    pub runtime: String,
    pub artifact_digest: String,
    pub predicted_label: String,
    pub logits: Vec<f64>,
    pub probabilities: Vec<f64>,
}

fn sha256_file(path: &Path) -> Result<String, AiError> {
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

fn validate_relative_path(relative: &str) -> Result<PathBuf, AiError> {
    if relative.is_empty() || relative.contains(['\\', '\0']) {
        return Err(AiError::InvalidManifest(format!(
            "file path must be a non-empty portable relative path: {relative}"
        )));
    }
    let parts: Vec<_> = relative.split('/').collect();
    if parts.iter().any(|part| {
        part.is_empty()
            || *part == "."
            || *part == ".."
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    }) {
        return Err(AiError::InvalidManifest(format!(
            "unsafe model file path: {relative}"
        )));
    }
    let path = parts.iter().collect::<PathBuf>();
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(AiError::InvalidManifest(format!(
            "unsafe model file path: {relative}"
        )));
    }
    Ok(path)
}

fn resolve_existing_file(root: &Path, relative: &str) -> Result<PathBuf, AiError> {
    let root = fs::canonicalize(root)?;
    let path = root.join(validate_relative_path(relative)?);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AiError::Missing(relative.to_string())
        } else {
            AiError::Io(error)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AiError::InvalidManifest(format!(
            "model path must be a regular file: {relative}"
        )));
    }
    let canonical = fs::canonicalize(path)?;
    if !canonical.starts_with(root) {
        return Err(AiError::InvalidManifest(format!(
            "model path escapes project root: {relative}"
        )));
    }
    Ok(canonical)
}

fn resolve_model_paths(
    root: &Path,
    relative: &str,
    digest: &str,
) -> Result<(PathBuf, PathBuf), AiError> {
    let root = fs::canonicalize(root)?;
    let relative_path = validate_relative_path(relative)?;
    let destination = root.join(&relative_path);
    let parent = destination
        .parent()
        .ok_or_else(|| AiError::InvalidManifest(format!("invalid model path: {relative}")))?;
    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(&root) {
        return Err(AiError::InvalidManifest(format!(
            "model path escapes project root: {relative}"
        )));
    }
    let filename = relative_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AiError::InvalidManifest(format!("invalid model path: {relative}")))?;
    let short_digest = digest.get(..12).unwrap_or(digest);
    let partial = parent.join(format!(".{filename}.jhol-{short_digest}.part"));
    if let Ok(metadata) = fs::symlink_metadata(&partial) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AiError::InvalidManifest(format!(
                "partial download must be a regular file: {}",
                partial.display()
            )));
        }
    }
    Ok((destination, partial))
}

fn resolve_download_paths(
    root: &Path,
    relative: &str,
    digest: &str,
) -> Result<(PathBuf, PathBuf), AiError> {
    let (destination, partial) = resolve_model_paths(root, relative, digest)?;
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AiError::InvalidManifest(format!(
                "model destination must be a regular file: {relative}"
            )));
        }
        return Err(AiError::InvalidManifest(format!(
            "refusing to replace existing unverified model file: {relative}"
        )));
    }
    Ok((destination, partial))
}

fn aggregate_digest(files: &[FilePin]) -> String {
    let mut sorted = files.to_vec();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hasher = Sha256::new();
    for file in sorted {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.digest.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

fn hf_file_url(source: &str, revision: &str, path: &str) -> Result<String, AiError> {
    let repo = source
        .strip_prefix("hf://")
        .ok_or_else(|| AiError::InvalidManifest("source must use hf://owner/repository".into()))?;
    let mut repo_parts = repo.split('/');
    let owner = repo_parts.next().unwrap_or_default();
    let name = repo_parts.next().unwrap_or_default();
    if owner.is_empty()
        || name.is_empty()
        || owner == "."
        || owner == ".."
        || name == "."
        || name == ".."
        || repo_parts.next().is_some()
        || ![owner, name].iter().all(|part| {
            part.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
        || revision.len() != 40
        || !revision
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(AiError::InvalidManifest(
            "model fetch needs hf://owner/repository and a 40-character commit revision".into(),
        ));
    }
    let path = validate_relative_path(path)?;
    let path = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!(
        "https://huggingface.co/{owner}/{name}/resolve/{revision}/{path}"
    ))
}

impl ModelArtifact {
    pub fn pin_directory(name: &str, root: &Path, rel_files: &[&str]) -> Result<Self, AiError> {
        if rel_files.is_empty() {
            return Err(AiError::InvalidManifest(
                "model artifact needs at least one file".into(),
            ));
        }
        let mut files = Vec::new();
        for rel in rel_files {
            let p = resolve_existing_file(root, rel)?;
            files.push(FilePin {
                path: rel.to_string(),
                digest: sha256_file(&p)?,
                size_bytes: Some(fs::metadata(&p)?.len()),
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self {
            schema: 1,
            name: name.to_string(),
            source: None,
            revision: None,
            digest: aggregate_digest(&files),
            files,
        })
    }

    pub fn pin_directory_at_revision(
        name: &str,
        source: &str,
        revision: &str,
        root: &Path,
        rel_files: &[&str],
    ) -> Result<Self, AiError> {
        for path in rel_files {
            hf_file_url(source, revision, path)?;
        }
        let mut artifact = Self::pin_directory(name, root, rel_files)?;
        artifact.source = Some(source.to_string());
        artifact.revision = Some(revision.to_string());
        Ok(artifact)
    }

    pub fn pin_remote_files(
        name: &str,
        source: &str,
        revision: &str,
        mut files: Vec<FilePin>,
    ) -> Result<Self, AiError> {
        if name.trim().is_empty() || files.is_empty() {
            return Err(AiError::InvalidManifest(
                "remote model pin needs a name and at least one file".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for file in &files {
            validate_relative_path(&file.path)?;
            hf_file_url(source, revision, &file.path)?;
            if !seen.insert(file.path.as_str()) {
                return Err(AiError::InvalidManifest(format!(
                    "duplicate model file path: {}",
                    file.path
                )));
            }
            if file.digest.len() != 64
                || !file
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                || file.size_bytes.is_none()
            {
                return Err(AiError::InvalidManifest(format!(
                    "remote file {} needs a lowercase SHA-256 digest and size_bytes",
                    file.path
                )));
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let digest = aggregate_digest(&files);
        Ok(Self {
            schema: 1,
            name: name.to_string(),
            source: Some(source.to_string()),
            revision: Some(revision.to_string()),
            digest,
            files,
        })
    }

    /// Verify every pinned file still matches; fail closed on mismatch.
    pub fn verify(&self, root: &Path) -> Result<(), AiError> {
        if self.schema != 1 {
            return Err(AiError::InvalidManifest(format!(
                "unsupported model schema {}, expected 1",
                self.schema
            )));
        }
        if aggregate_digest(&self.files) != self.digest {
            return Err(AiError::InvalidManifest(
                "aggregate model digest does not match file pins".into(),
            ));
        }
        for f in &self.files {
            let p = resolve_existing_file(root, &f.path)?;
            let got = sha256_file(&p)?;
            if got != f.digest {
                return Err(AiError::DigestMismatch {
                    path: f.path.clone(),
                    expected: f.digest.clone(),
                    got,
                });
            }
        }
        Ok(())
    }

    /// Fetch missing Hugging Face files only after explicit permission. Callback
    /// receives URL, partial path, resume offset, and hard byte limit.
    pub fn prepare_with_download<F>(
        &self,
        root: &Path,
        allow_network: bool,
        max_bytes: u64,
        mut download: F,
    ) -> Result<DownloadReceipt, AiError>
    where
        F: FnMut(&str, &Path, u64, u64) -> Result<u64, String>,
    {
        if !allow_network {
            return Err(AiError::NetworkPermissionRequired);
        }
        if self.schema != 1 {
            return Err(AiError::InvalidManifest(format!(
                "unsupported model schema {}, expected 1",
                self.schema
            )));
        }
        if aggregate_digest(&self.files) != self.digest {
            return Err(AiError::InvalidManifest(
                "aggregate model digest does not match file pins".into(),
            ));
        }
        let source = self
            .source
            .as_deref()
            .ok_or_else(|| AiError::InvalidManifest("model source is required for fetch".into()))?;
        let revision = self.revision.as_deref().ok_or_else(|| {
            AiError::InvalidManifest("model revision is required for fetch".into())
        })?;
        let mut total_expected = 0u64;
        for file in &self.files {
            match resolve_existing_file(root, &file.path) {
                Ok(path) => {
                    let actual = sha256_file(&path)?;
                    if actual != file.digest {
                        return Err(AiError::DigestMismatch {
                            path: file.path.clone(),
                            expected: file.digest.clone(),
                            got: actual,
                        });
                    }
                    continue;
                }
                Err(AiError::Missing(_)) => {}
                Err(error) => return Err(error),
            }
            let size = file.size_bytes.ok_or_else(|| {
                AiError::InvalidManifest(format!(
                    "missing size_bytes for downloadable file {}",
                    file.path
                ))
            })?;
            total_expected = total_expected
                .checked_add(size)
                .ok_or_else(|| AiError::InvalidManifest("total model size overflows u64".into()))?;
            if total_expected > max_bytes {
                return Err(AiError::InvalidManifest(format!(
                    "model requires {total_expected} bytes, exceeding --max-bytes {max_bytes}"
                )));
            }
            if file.digest.len() != 64 || !file.digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(AiError::InvalidManifest(format!(
                    "file digest must be a 64-character SHA-256 hex value: {}",
                    file.path
                )));
            }
        }

        let root = fs::canonicalize(root)?;
        let mut files = Vec::with_capacity(self.files.len());
        let mut bytes_received = 0u64;
        let mut cleaned_partials = Vec::new();
        for file in &self.files {
            match resolve_existing_file(&root, &file.path) {
                Ok(path) => {
                    let size = fs::metadata(&path)?.len();
                    let (_, partial) = resolve_model_paths(&root, &file.path, &file.digest)?;
                    if partial.exists() {
                        fs::remove_file(&partial)?;
                        cleaned_partials.push(file.path.clone());
                    }
                    files.push(DownloadedFile {
                        path: file.path.clone(),
                        digest: file.digest.clone(),
                        size_bytes: size,
                        bytes_received: 0,
                        resumed: false,
                        already_present: true,
                    });
                    continue;
                }
                Err(AiError::Missing(_)) => {}
                Err(error) => return Err(error),
            };
            let expected_size = file.size_bytes.ok_or_else(|| {
                AiError::InvalidManifest(format!(
                    "missing size_bytes for downloadable file {}",
                    file.path
                ))
            })?;
            let url = hf_file_url(source, revision, &file.path)?;
            let (destination, partial) = resolve_download_paths(&root, &file.path, &file.digest)?;
            let offset = fs::metadata(&partial).map(|meta| meta.len()).unwrap_or(0);
            if offset > expected_size {
                fs::remove_file(&partial)?;
                cleaned_partials.push(file.path.clone());
            }
            if expected_size == 0 && !partial.exists() {
                fs::File::create(&partial)?;
            }
            let offset = offset.min(expected_size);
            let received = if offset == expected_size {
                0
            } else {
                download(&url, &partial, offset, expected_size).map_err(AiError::Download)?
            };
            let partial_size = fs::metadata(&partial)?.len();
            if partial_size != expected_size {
                return Err(AiError::InvalidManifest(format!(
                    "downloaded size for {} is {partial_size}, expected {expected_size}; partial kept for resume",
                    file.path
                )));
            }
            let got = sha256_file(&partial)?;
            if got != file.digest {
                fs::remove_file(&partial)?;
                return Err(AiError::DigestMismatch {
                    path: file.path.clone(),
                    expected: file.digest.clone(),
                    got,
                });
            }
            fs::hard_link(&partial, &destination)?;
            fs::remove_file(&partial)?;
            cleaned_partials.push(file.path.clone());
            bytes_received = bytes_received.saturating_add(received);
            files.push(DownloadedFile {
                path: file.path.clone(),
                digest: file.digest.clone(),
                size_bytes: expected_size,
                bytes_received: received,
                resumed: offset > 0,
                already_present: false,
            });
        }
        Ok(DownloadReceipt {
            source: source.to_string(),
            revision: revision.to_string(),
            files,
            bytes_received,
            cleaned_partials,
        })
    }

    pub fn write_manifest(&self, path: &Path) -> Result<(), AiError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn read_manifest(path: &Path) -> Result<Self, AiError> {
        let mut content = String::new();
        fs::File::open(path)?
            .take(MODEL_MANIFEST_LIMIT + 1)
            .read_to_string(&mut content)?;
        if content.len() as u64 > MODEL_MANIFEST_LIMIT {
            return Err(AiError::InvalidManifest(format!(
                "manifest exceeds the {MODEL_MANIFEST_LIMIT}-byte limit"
            )));
        }
        Ok(serde_json::from_str(&content)?)
    }
}

/// Run the manifest-pinned `jhol.linear-classifier.v1` model on the bounded
/// `jhol-linear-cpu-v1` runtime. The model is data only; no model code executes.
pub fn infer_linear_cpu(
    artifact: &ModelArtifact,
    root: &Path,
    config_path: &str,
    input: &[f64],
) -> Result<InferenceResult, AiError> {
    if artifact.schema != 1 || artifact.files.is_empty() || artifact.name.trim().is_empty() {
        return Err(AiError::InvalidManifest(
            "inference requires a named schema-1 model artifact".into(),
        ));
    }
    let config_path_buf = validate_relative_path(config_path)?;
    let config_path = config_path_buf.to_string_lossy().into_owned();
    if !artifact.files.iter().any(|file| file.path == config_path) {
        return Err(AiError::InvalidManifest(format!(
            "inference config is not pinned by the model manifest: {config_path}"
        )));
    }
    if input.is_empty() || input.len() > LINEAR_MAX_FEATURES {
        return Err(AiError::InvalidManifest(format!(
            "input must contain 1 to {LINEAR_MAX_FEATURES} features"
        )));
    }
    if input
        .iter()
        .any(|value| !value.is_finite() || value.abs() > LINEAR_MAX_INPUT_ABS)
    {
        return Err(AiError::InvalidManifest(format!(
            "input features must be finite and within +/-{LINEAR_MAX_INPUT_ABS}"
        )));
    }

    // Cap the whole artifact before hashing or reading it. Then verify and retain
    // the exact bytes used below, avoiding a verify-then-reopen race.
    let mut total_bytes = 0u64;
    let mut pins = std::collections::HashMap::new();
    let mut files = std::collections::HashMap::new();
    for pin in &artifact.files {
        validate_relative_path(&pin.path)?;
        if pins.insert(pin.path.as_str(), pin).is_some() {
            return Err(AiError::InvalidManifest(format!(
                "duplicate model file path: {}",
                pin.path
            )));
        }
        let path = resolve_existing_file(root, &pin.path)?;
        let metadata = fs::metadata(&path)?;
        if pin.path == config_path && metadata.len() > LINEAR_CONFIG_LIMIT {
            return Err(AiError::InvalidManifest(format!(
                "inference config exceeds the {LINEAR_CONFIG_LIMIT}-byte limit"
            )));
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| AiError::InvalidManifest("model size overflows u64".into()))?;
        if total_bytes > LINEAR_ARTIFACT_LIMIT {
            return Err(AiError::InvalidManifest(format!(
                "linear model artifact exceeds the {LINEAR_ARTIFACT_LIMIT}-byte runtime limit"
            )));
        }
        if pin
            .size_bytes
            .is_some_and(|expected| expected != metadata.len())
        {
            return Err(AiError::InvalidManifest(format!(
                "model file size does not match pin: {}",
                pin.path
            )));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        fs::File::open(path)?
            .take(LINEAR_ARTIFACT_LIMIT + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != metadata.len() {
            return Err(AiError::InvalidManifest(format!(
                "model file changed while reading: {}",
                pin.path
            )));
        }
        let got = hex::encode(Sha256::digest(&bytes));
        if got != pin.digest {
            return Err(AiError::DigestMismatch {
                path: pin.path.clone(),
                expected: pin.digest.clone(),
                got,
            });
        }
        files.insert(pin.path.as_str(), bytes);
    }
    if aggregate_digest(&artifact.files) != artifact.digest {
        return Err(AiError::InvalidManifest(
            "aggregate model digest does not match file pins".into(),
        ));
    }

    let config_bytes = files
        .get(config_path.as_str())
        .ok_or_else(|| AiError::Missing(config_path.clone()))?;
    if config_bytes.len() as u64 > LINEAR_CONFIG_LIMIT {
        return Err(AiError::InvalidManifest(format!(
            "inference config exceeds the {LINEAR_CONFIG_LIMIT}-byte limit"
        )));
    }
    let config: LinearModelConfig = serde_json::from_slice(config_bytes)?;
    if config.schema != 1
        || config.format != "jhol.linear-classifier.v1"
        || config.runtime != "jhol-linear-cpu-v1"
        || config.name != artifact.name
        || config.name.trim().is_empty()
        || config.name.len() > 128
        || config.name.chars().any(char::is_control)
    {
        return Err(AiError::InvalidManifest(
            "unsupported model, format, or runtime in inference config".into(),
        ));
    }
    if config.input_features == 0
        || config.input_features > LINEAR_MAX_FEATURES
        || config.input_features != input.len()
        || config.labels.len() < 2
        || config.labels.len() > LINEAR_MAX_CLASSES
        || config.labels.iter().any(|label| {
            label.trim().is_empty() || label.len() > 128 || label.chars().any(char::is_control)
        })
    {
        return Err(AiError::InvalidManifest(
            "model dimensions or labels are outside the runtime limits".into(),
        ));
    }
    let mut labels = std::collections::HashSet::new();
    if config.labels.iter().any(|label| !labels.insert(label)) {
        return Err(AiError::InvalidManifest(
            "model labels must be unique".into(),
        ));
    }
    let weights_path = validate_relative_path(&config.weights)?
        .to_string_lossy()
        .into_owned();
    if !pins.contains_key(weights_path.as_str()) {
        return Err(AiError::InvalidManifest(format!(
            "inference weights are not pinned by the model manifest: {weights_path}"
        )));
    }
    let weights = files
        .get(weights_path.as_str())
        .ok_or_else(|| AiError::Missing(weights_path.clone()))?;
    let weight_count = config
        .input_features
        .checked_mul(config.labels.len())
        .and_then(|count| count.checked_add(config.labels.len()))
        .ok_or_else(|| AiError::InvalidManifest("model dimensions overflow".into()))?;
    let expected_bytes = LINEAR_WEIGHTS_MAGIC.len() + weight_count * 4;
    if weights.len() != expected_bytes || !weights.starts_with(LINEAR_WEIGHTS_MAGIC) {
        return Err(AiError::InvalidManifest(
            "weights must use the exact JHLIN001 little-endian linear format".into(),
        ));
    }
    let mut values = Vec::with_capacity(weight_count);
    for bytes in weights[LINEAR_WEIGHTS_MAGIC.len()..].chunks_exact(4) {
        let value = f32::from_le_bytes(bytes.try_into().expect("four-byte chunk"));
        if !value.is_finite() || value.abs() > 1_000_000.0 {
            return Err(AiError::InvalidManifest(
                "model weight is non-finite or outside the runtime limit".into(),
            ));
        }
        values.push(f64::from(value));
    }

    let features = config.input_features;
    let classes = config.labels.len();
    let mut logits = Vec::with_capacity(classes);
    for class in 0..classes {
        let row_start = class * features;
        let bias = values[classes * features + class];
        let score = input
            .iter()
            .enumerate()
            .fold(bias, |sum, (feature, value)| {
                sum + value * values[row_start + feature]
            });
        if !score.is_finite() {
            return Err(AiError::InvalidManifest(
                "inference produced a non-finite score".into(),
            ));
        }
        logits.push(score);
    }
    let max_logit = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut probabilities: Vec<f64> = logits
        .iter()
        .map(|score| (score - max_logit).exp())
        .collect();
    let sum = probabilities.iter().sum::<f64>();
    if !sum.is_finite() || sum <= 0.0 {
        return Err(AiError::InvalidManifest(
            "inference normalization failed".into(),
        ));
    }
    probabilities.iter_mut().for_each(|value| *value /= sum);
    let mut predicted = 0;
    for index in 1..classes {
        if logits[index] > logits[predicted] {
            predicted = index;
        }
    }

    Ok(InferenceResult {
        model: config.name,
        runtime: config.runtime,
        artifact_digest: artifact.digest.clone(),
        predicted_label: config.labels[predicted].clone(),
        logits,
        probabilities,
    })
}

/// Offline prepare: confirm pins verify; no network.
pub fn prepare_offline(root: &Path, manifest: &Path) -> Result<ModelArtifact, AiError> {
    let art = ModelArtifact::read_manifest(manifest)?;
    art.verify(root)?;
    Ok(art)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_linear_artifact(root: &Path) -> ModelArtifact {
        let model_dir = root.join("models/demo");
        fs::create_dir_all(&model_dir).unwrap();
        fs::write(
            model_dir.join("config.json"),
            br#"{"schema":1,"format":"jhol.linear-classifier.v1","name":"jhol-demo-linear-v1","runtime":"jhol-linear-cpu-v1","weights":"models/demo/weights.bin","input_features":2,"labels":["small","large"]}"#,
        )
        .unwrap();
        let mut weights = LINEAR_WEIGHTS_MAGIC.to_vec();
        for value in [0.5f32, -0.25, -0.5, 0.25, 0.0, 0.0] {
            weights.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(model_dir.join("weights.bin"), weights).unwrap();
        ModelArtifact::pin_directory(
            "jhol-demo-linear-v1",
            root,
            &["models/demo/config.json", "models/demo/weights.bin"],
        )
        .unwrap()
    }

    #[test]
    fn pin_and_verify() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models/demo");
        fs::create_dir_all(&models).unwrap();
        fs::write(models.join("weights.bin"), b"abc").unwrap();
        fs::write(models.join("config.json"), b"{}").unwrap();
        let art = ModelArtifact::pin_directory(
            "demo",
            dir.path(),
            &["models/demo/weights.bin", "models/demo/config.json"],
        )
        .unwrap();
        art.verify(dir.path()).unwrap();
        let man = dir.path().join("models/demo.jhol-model.json");
        art.write_manifest(&man).unwrap();
        prepare_offline(dir.path(), &man).unwrap();
        fs::write(models.join("weights.bin"), b"mutated").unwrap();
        assert!(art.verify(dir.path()).is_err());
    }

    #[test]
    fn prepare_requires_explicit_network_permission() {
        let dir = tempfile::tempdir().unwrap();
        let art = ModelArtifact {
            schema: 1,
            name: "demo".into(),
            source: Some("hf://example-org/example-model".into()),
            revision: Some("0123456789012345678901234567890123456789".into()),
            digest: "0".repeat(64),
            files: vec![],
        };
        let mut called = false;
        let error = art
            .prepare_with_download(dir.path(), false, 1024, |_, _, _, _| {
                called = true;
                Ok(0)
            })
            .unwrap_err();
        assert!(matches!(error, AiError::NetworkPermissionRequired));
        assert!(!called);
    }

    #[test]
    fn prepare_resumes_and_verifies_hugging_face_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_digest = hex::encode(Sha256::digest(b"abcdef"));
        let pin = FilePin {
            path: "models/demo/weights.bin".into(),
            digest: file_digest,
            size_bytes: Some(6),
        };
        let art = ModelArtifact::pin_remote_files(
            "demo",
            "hf://example-org/example-model",
            "0123456789012345678901234567890123456789",
            vec![pin],
        )
        .unwrap();
        let partial = dir
            .path()
            .join("models/demo/.weights.bin.jhol-")
            .with_file_name(format!(
                ".weights.bin.jhol-{}.part",
                &art.files[0].digest[..12]
            ));
        fs::create_dir_all(partial.parent().unwrap()).unwrap();
        fs::write(&partial, b"abc").unwrap();

        let receipt = art
            .prepare_with_download(dir.path(), true, 6, |url, dest, start, limit| {
                assert_eq!(
                    url,
                    "https://huggingface.co/example-org/example-model/resolve/0123456789012345678901234567890123456789/models/demo/weights.bin"
                );
                assert_eq!(start, 3);
                assert_eq!(limit, 6);
                use std::io::Write;
                fs::OpenOptions::new()
                    .append(true)
                    .open(dest)
                    .unwrap()
                    .write_all(b"def")
                    .unwrap();
                Ok(3)
            })
            .unwrap();

        assert_eq!(
            fs::read(dir.path().join("models/demo/weights.bin")).unwrap(),
            b"abcdef"
        );
        assert_eq!(receipt.bytes_received, 3);
        assert!(receipt.files[0].resumed);
        assert!(!receipt.files[0].already_present);
        assert_eq!(
            receipt.cleaned_partials,
            vec!["models/demo/weights.bin".to_string()]
        );
        assert!(!partial.exists());
    }

    #[test]
    fn verify_rejects_path_escape_and_aggregate_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let mut art = ModelArtifact {
            schema: 1,
            name: "demo".into(),
            source: None,
            revision: None,
            digest: "0".repeat(64),
            files: vec![FilePin {
                path: "../outside.bin".into(),
                digest: "0".repeat(64),
                size_bytes: None,
            }],
        };
        art.digest = aggregate_digest(&art.files);
        assert!(matches!(
            art.verify(dir.path()),
            Err(AiError::InvalidManifest(_))
        ));
        art.files[0].path = "model.bin".into();
        assert!(matches!(
            art.verify(dir.path()),
            Err(AiError::InvalidManifest(_))
        ));
    }

    #[test]
    fn linear_cpu_inference_is_repeatable_and_uses_pinned_model_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = demo_linear_artifact(dir.path());
        let first = infer_linear_cpu(
            &artifact,
            dir.path(),
            "models/demo/config.json",
            &[-2.0, 1.0],
        )
        .unwrap();
        let second = infer_linear_cpu(
            &artifact,
            dir.path(),
            "models/demo/config.json",
            &[-2.0, 1.0],
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.model, "jhol-demo-linear-v1");
        assert_eq!(first.runtime, "jhol-linear-cpu-v1");
        assert_eq!(first.predicted_label, "large");
        assert!((first.probabilities.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn linear_cpu_inference_rejects_unpinned_or_unbounded_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = demo_linear_artifact(dir.path());
        assert!(infer_linear_cpu(
            &artifact,
            dir.path(),
            "models/demo/config.json",
            &[f64::INFINITY, 0.0],
        )
        .is_err());

        let weights_path = dir.path().join("models/demo/weights.bin");
        let mut changed_weights = fs::read(&weights_path).unwrap();
        changed_weights[8] ^= 1;
        fs::write(weights_path, changed_weights).unwrap();
        assert!(matches!(
            infer_linear_cpu(
                &artifact,
                dir.path(),
                "models/demo/config.json",
                &[0.0, 0.0],
            ),
            Err(AiError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn linear_cpu_inference_rejects_artifacts_over_memory_bound() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = demo_linear_artifact(dir.path());
        let oversized = dir.path().join("models/demo/extra.bin");
        fs::write(&oversized, vec![0; LINEAR_ARTIFACT_LIMIT as usize + 1]).unwrap();
        let mut files: Vec<_> = artifact.files.clone();
        files.push(FilePin {
            path: "models/demo/extra.bin".into(),
            digest: sha256_file(&oversized).unwrap(),
            size_bytes: Some(LINEAR_ARTIFACT_LIMIT + 1),
        });
        let oversized_artifact = ModelArtifact {
            schema: 1,
            name: artifact.name,
            source: None,
            revision: None,
            digest: aggregate_digest(&files),
            files,
        };
        assert!(matches!(
            infer_linear_cpu(
                &oversized_artifact,
                dir.path(),
                "models/demo/config.json",
                &[0.0, 0.0],
            ),
            Err(AiError::InvalidManifest(_))
        ));
    }

    #[test]
    fn model_manifest_read_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("oversized.jhol-model.json");
        fs::write(&manifest, vec![b' '; MODEL_MANIFEST_LIMIT as usize + 1]).unwrap();
        assert!(matches!(
            ModelArtifact::read_manifest(&manifest),
            Err(AiError::InvalidManifest(_))
        ));
    }
}
