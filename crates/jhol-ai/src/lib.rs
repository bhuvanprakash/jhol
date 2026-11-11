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

/// Digest of a file on disk, for callers that need the same identity the store
/// uses (the CLI's `ai pull` records it in a manifest).
pub fn digest_file(path: &Path) -> Result<String, AiError> {
    sha256_file(path)
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
    // `as_chunks` yields `&[[u8; 4]]`, so no fallible slice-to-array conversion
    // (and no `expect` that would abort the process) is needed; any trailing
    // partial word was already rejected by the length check above.
    for bytes in weights[LINEAR_WEIGHTS_MAGIC.len()..].as_chunks::<4>().0 {
        let value = f32::from_le_bytes(*bytes);
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

// ── Safetensors introspection ────────────────────────────────────────────────
//
// A safetensors file is `[u64 LE header length][JSON header][tensor data]`, and the
// header maps every tensor to a byte range. That means a model's *structure* — how
// many parameters, which dtypes, how the shards are laid out — is readable from a
// few kilobytes, before any weights are fetched. Models therefore enter resolution
// with real metadata, exactly like npm packages do.
pub mod safetensors {
    use super::{AiError, Path};
    use std::collections::BTreeMap;

    /// Upper bound on a header we will accept. Real headers are tens of KB; a
    /// claimed length beyond this is either corrupt or hostile, and reading it
    /// would let a remote file choose how much memory we allocate.
    pub const MAX_HEADER_BYTES: u64 = 32 * 1024 * 1024;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Tensor {
        pub name: String,
        pub dtype: String,
        pub shape: Vec<u64>,
        pub begin: u64,
        pub end: u64,
    }

    impl Tensor {
        pub fn elements(&self) -> u64 {
            self.shape.iter().product::<u64>().max(if self.shape.is_empty() { 0 } else { 1 })
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Header {
        pub tensors: Vec<Tensor>,
        /// Bytes occupied by the JSON header (not counting its 8-byte length).
        pub header_bytes: usize,
        /// Byte offset where tensor data begins: `8 + header_bytes`.
        pub data_start: u64,
        pub total_params: u64,
        /// dtype -> tensor count
        pub dtypes: BTreeMap<String, usize>,
    }

    impl Header {
        pub fn tensor(&self, name: &str) -> Option<&Tensor> {
            self.tensors.iter().find(|t| t.name == name)
        }
    }

    /// The declared header length from the first 8 bytes, sanity-checked.
    pub fn header_len(first_eight: &[u8]) -> Result<u64, AiError> {
        if first_eight.len() < 8 {
            return Err(AiError::InvalidManifest(
                "safetensors: file shorter than its 8-byte header length".into(),
            ));
        }
        let n = u64::from_le_bytes(first_eight[..8].try_into().expect("8 bytes"));
        if n == 0 || n > MAX_HEADER_BYTES {
            return Err(AiError::InvalidManifest(format!(
                "safetensors: implausible header length {n}"
            )));
        }
        Ok(n)
    }

    /// Parse `[8-byte length][JSON header]`. Extra trailing bytes are ignored, so a
    /// caller that fetched `8 + N` bytes (or more) can pass them straight in.
    pub fn parse_header(bytes: &[u8]) -> Result<Header, AiError> {
        let n = header_len(bytes)?;
        let end = 8usize
            .checked_add(n as usize)
            .ok_or_else(|| AiError::InvalidManifest("safetensors: length overflow".into()))?;
        let json = bytes
            .get(8..end)
            .ok_or_else(|| AiError::InvalidManifest("safetensors: truncated header".into()))?;
        let json: &[u8] = {
            // The spec allows trailing whitespace padding inside the header.
            let s = String::from_utf8_lossy(json);
            let trimmed = s.trim_end();
            // Re-borrow through the original slice by trimming at the JSON end.
            let _ = trimmed;
            json
        };
        let doc: serde_json::Value = serde_json::from_slice(json)
            .map_err(|e| AiError::InvalidManifest(format!("safetensors: header is not JSON: {e}")))?;
        let obj = doc
            .as_object()
            .ok_or_else(|| AiError::InvalidManifest("safetensors: header is not an object".into()))?;

        let mut tensors = Vec::new();
        let mut dtypes: BTreeMap<String, usize> = BTreeMap::new();
        let mut total_params = 0u64;
        for (name, value) in obj {
            if name == "__metadata__" {
                continue;
            }
            let dtype = value
                .get("dtype")
                .and_then(|d| d.as_str())
                .ok_or_else(|| AiError::InvalidManifest(format!("safetensors: {name} has no dtype")))?
                .to_string();
            let shape: Vec<u64> = value
                .get("shape")
                .and_then(|s| s.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                .unwrap_or_default();
            let offsets = value
                .get("data_offsets")
                .and_then(|o| o.as_array())
                .filter(|a| a.len() == 2)
                .ok_or_else(|| {
                    AiError::InvalidManifest(format!("safetensors: {name} has no data_offsets"))
                })?;
            let begin = offsets[0].as_u64().unwrap_or(0);
            let end_off = offsets[1].as_u64().unwrap_or(0);
            if end_off < begin {
                return Err(AiError::InvalidManifest(format!(
                    "safetensors: {name} has inverted data_offsets"
                )));
            }
            let t = Tensor {
                name: name.clone(),
                dtype: dtype.clone(),
                shape,
                begin,
                end: end_off,
            };
            total_params = total_params.saturating_add(t.elements());
            *dtypes.entry(dtype).or_default() += 1;
            tensors.push(t);
        }
        tensors.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Header {
            tensors,
            header_bytes: n as usize,
            data_start: 8 + n,
            total_params,
            dtypes,
        })
    }

    /// Parse a local `.safetensors` file by reading only its header.
    /// Bytes per element, for the dtypes safetensors actually carries.
    pub fn bytes_per_dtype(dtype: &str) -> Option<u64> {
        Some(match dtype {
            "F64" | "I64" | "U64" => 8,
            "F32" | "I32" | "U32" => 4,
            "F16" | "BF16" | "I16" | "U16" => 2,
            "I8" | "U8" | "BOOL" | "F8_E4M3" | "F8_E5M2" => 1,
            _ => return None,
        })
    }

    /// A model, described from its header alone: parameter count, dtype mix, and the
    /// bytes it will occupy once loaded.
    ///
    /// This is the whole reason weights can enter resolution with real metadata: a 7 GB
    /// checkpoint answers "how big will you be in memory?" after reading a few kilobytes.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct WeightSummary {
        pub tensors: usize,
        pub params: u64,
        /// Bytes of tensor data (sum of each tensor's byte range).
        pub data_bytes: u64,
        /// Bytes of JSON header — the price of asking.
        pub header_bytes: usize,
        /// dtype -> tensor count, ordered so two runs print the same table.
        pub dtypes: BTreeMap<String, usize>,
        /// dtypes with no known width, so a caller can refuse to quote a size.
        pub unknown_dtypes: Vec<String>,
        /// The heaviest tensors, largest first — where the bytes actually are.
        pub largest: Vec<(String, u64)>,
    }

    impl WeightSummary {
        /// Resident estimate: tensor bytes plus 20% for the allocator, the Python object
        /// graph and the activations a loader keeps alive. Deliberately generous — an
        /// under-quote here is an out-of-memory at the worst possible moment.
        pub fn vram_bytes(&self) -> u64 {
            self.data_bytes + self.data_bytes / 5
        }

        pub fn params_human(&self) -> String {
            let p = self.params as f64;
            if p >= 1e9 {
                format!("{:.2} B", p / 1e9)
            } else if p >= 1e6 {
                format!("{:.1} M", p / 1e6)
            } else {
                format!("{}", self.params)
            }
        }
    }

    pub fn summarise(path: &Path) -> Result<WeightSummary, AiError> {
        summarise_from_header(&parse_file(path)?)
    }

    /// The same summary from bytes already in hand — a ranged fetch reads the header of a
    /// remote checkpoint, so it never needs the file on disk.
    pub fn summarise_bytes(bytes: &[u8]) -> Result<WeightSummary, AiError> {
        summarise_from_header(&parse_header(bytes)?)
    }

    pub fn summarise_from_header(header: &Header) -> Result<WeightSummary, AiError> {
        let mut out = WeightSummary {
            tensors: header.tensors.len(),
            params: header.total_params,
            header_bytes: header.header_bytes,
            dtypes: header.dtypes.clone(),
            ..Default::default()
        };
        for tensor in &header.tensors {
            out.data_bytes += tensor.end.saturating_sub(tensor.begin);
        }
        let mut weighed: Vec<(String, u64)> = header
            .tensors
            .iter()
            .map(|t| (t.name.clone(), t.end.saturating_sub(t.begin)))
            .collect();
        weighed.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        weighed.truncate(10);
        out.largest = weighed;
        out.unknown_dtypes = out
            .dtypes
            .keys()
            .filter(|d| bytes_per_dtype(d).is_none())
            .cloned()
            .collect();
        Ok(out)
    }

    /// A whole model on disk: every safetensors shard, summed, plus the sidecars a loader
    /// actually needs.
    ///
    /// Real checkpoints are sharded (`model-00001-of-00008.safetensors`…). Reading one shard
    /// and calling that the model under-reports parameters by the number of shards, which is
    /// the kind of wrong that looks right — so a directory is always read as a whole.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct ModelSummary {
        pub shards: Vec<(String, WeightSummary)>,
        pub params: u64,
        pub data_bytes: u64,
        pub header_bytes: u64,
        pub dtypes: BTreeMap<String, usize>,
        pub largest: Vec<(String, u64)>,
        pub unknown_dtypes: Vec<String>,
        /// `config.json`, `tokenizer.json`, … present or absent, in a fixed order.
        pub sidecars: Vec<(String, bool)>,
    }

    pub const SIDECARS: [&str; 4] = [
        "config.json",
        "tokenizer.json",
        "tokenizer.model",
        "generation_config.json",
    ];

    impl ModelSummary {
        pub fn vram_bytes(&self) -> u64 {
            self.data_bytes + self.data_bytes / 5
        }
    }

    pub fn summarise_dir(dir: &Path) -> Result<ModelSummary, AiError> {
        let mut shards: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "safetensors").unwrap_or(false))
            .collect();
        shards.sort();
        if shards.is_empty() {
            return Err(AiError::InvalidManifest(format!(
                "{}: no .safetensors file",
                dir.display()
            )));
        }
        let mut out = ModelSummary::default();
        for path in shards {
            let summary = summarise(&path)?;
            out.params = out.params.saturating_add(summary.params);
            out.data_bytes = out.data_bytes.saturating_add(summary.data_bytes);
            out.header_bytes = out.header_bytes.saturating_add(summary.header_bytes as u64);
            for (dtype, n) in &summary.dtypes {
                *out.dtypes.entry(dtype.clone()).or_default() += n;
            }
            for dtype in &summary.unknown_dtypes {
                if !out.unknown_dtypes.contains(dtype) {
                    out.unknown_dtypes.push(dtype.clone());
                }
            }
            out.largest.extend(summary.largest.iter().cloned());
            out.shards.push((
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                summary,
            ));
        }
        out.largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.largest.truncate(10);
        out.sidecars = SIDECARS
            .iter()
            .map(|n| (n.to_string(), dir.join(n).is_file()))
            .collect();
        Ok(out)
    }

    pub fn parse_file(path: &Path) -> Result<Header, AiError> {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        let mut first = [0u8; 8];
        f.read_exact(&mut first)?;
        let n = header_len(&first)?;
        let mut buf = vec![0u8; 8 + n as usize];
        buf[..8].copy_from_slice(&first);
        f.read_exact(&mut buf[8..])?;
        parse_header(&buf)
    }
}


// ── GGUF introspection ──────────────────────────────────────────────────────
//
// GGUF is the format single-file checkpoints ship in: a magic number, a metadata
// key/value table, then one descriptor per tensor. Like safetensors, the *structure* —
// architecture, parameter count, quantisation mix — is readable from the head of the file,
// so a 5 GB quantised model answers "how big, in what precision?" in one range request.
pub mod gguf {
    use super::{AiError, Path};
    use std::collections::BTreeMap;

    pub const MAGIC: u32 = 0x4655_4747; // "GGUF" little-endian
    /// A header plus tensor descriptors. Real files are well under this; a claimed size
    /// beyond it is corrupt or hostile.
    pub const MAX_HEADER_BYTES: u64 = 64 * 1024 * 1024;

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct GgufSummary {
        pub version: u32,
        pub tensors: usize,
        pub params: u64,
        /// `general.architecture`, when present.
        pub architecture: Option<String>,
        /// `general.name`, when present.
        pub name: Option<String>,
        /// quant type name -> tensor count.
        pub quants: BTreeMap<String, usize>,
        /// Tensor bytes for types whose packet size is known; `None` when any type is not.
        pub data_bytes: Option<u64>,
        /// The heaviest tensors, largest first.
        pub largest: Vec<(String, u64)>,
    }

    impl GgufSummary {
        pub fn vram_bytes(&self) -> Option<u64> {
            self.data_bytes.map(|b| b + b / 5)
        }
    }

    /// GGML tensor type -> (bytes per packet, elements per packet).
    fn packet(kind: u32) -> Option<(u64, u64)> {
        Some(match kind {
            0 => (4, 1),      // F32
            1 => (2, 1),      // F16
            2 => (18, 32),    // Q4_0
            3 => (20, 32),    // Q4_1
            6 => (22, 32),    // Q5_0
            7 => (24, 32),    // Q5_1
            8 => (34, 32),    // Q8_0
            9 => (36, 32),    // Q8_1
            10 => (84, 256),  // Q2_K
            11 => (110, 256), // Q3_K
            12 => (144, 256), // Q4_K
            13 => (176, 256), // Q5_K
            14 => (210, 256), // Q6_K
            15 => (292, 256), // Q8_K
            _ => return None,
        })
    }

    fn type_name(kind: u32) -> String {
        match kind {
            0 => "F32".into(),
            1 => "F16".into(),
            2 => "Q4_0".into(),
            3 => "Q4_1".into(),
            6 => "Q5_0".into(),
            7 => "Q5_1".into(),
            8 => "Q8_0".into(),
            9 => "Q8_1".into(),
            10 => "Q2_K".into(),
            11 => "Q3_K".into(),
            12 => "Q4_K".into(),
            13 => "Q5_K".into(),
            14 => "Q6_K".into(),
            15 => "Q8_K".into(),
            other => format!("type{other}"),
        }
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        at: usize,
    }

    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], AiError> {
            if self.at + n > self.bytes.len() {
                return Err(AiError::InvalidManifest(
                    "gguf: header truncated".to_string(),
                ));
            }
            let out = &self.bytes[self.at..self.at + n];
            self.at += n;
            Ok(out)
        }
        fn u32(&mut self) -> Result<u32, AiError> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
        }
        fn u64(&mut self) -> Result<u64, AiError> {
            Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8")))
        }
        fn string(&mut self) -> Result<String, AiError> {
            let n = self.u64()? as usize;
            let raw = self.take(n)?;
            Ok(String::from_utf8_lossy(raw).to_string())
        }
        /// Skip a metadata value; strings inside are returned in case a caller wants them.
        fn value(&mut self, kind: u32, out: &mut Vec<String>) -> Result<(), AiError> {
            match kind {
                0 | 1 | 7 => {
                    self.take(1)?;
                }
                2 | 3 => {
                    self.take(2)?;
                }
                4..=6 => {
                    self.take(4)?;
                }
                10..=12 => {
                    self.take(8)?;
                }
                8 => {
                    let s = self.string()?;
                    out.push(s);
                }
                9 => {
                    let inner = self.u32()?;
                    let n = self.u64()?;
                    // A hostile count would let a 40-byte file drive a billion iterations.
                    if n > 1_000_000 {
                        return Err(AiError::InvalidManifest(
                            "gguf: implausible array length".to_string(),
                        ));
                    }
                    for _ in 0..n {
                        self.value(inner, out)?;
                    }
                }
                other => {
                    return Err(AiError::InvalidManifest(format!(
                        "gguf: unknown metadata value type {other}"
                    )))
                }
            }
            Ok(())
        }
    }

    /// Parse a GGUF header from bytes that begin at offset 0.
    pub fn summarise_bytes(bytes: &[u8]) -> Result<GgufSummary, AiError> {
        let mut r = Reader { bytes, at: 0 };
        let magic = r.u32()?;
        if magic != MAGIC {
            return Err(AiError::InvalidManifest(format!(
                "gguf: bad magic 0x{magic:08x}"
            )));
        }
        let version = r.u32()?;
        let tensor_count = r.u64()? as usize;
        let kv_count = r.u64()? as usize;
        let mut out = GgufSummary {
            version,
            tensors: tensor_count,
            ..Default::default()
        };
        for _ in 0..kv_count {
            let key = r.string()?;
            let kind = r.u32()?;
            let mut strings = Vec::new();
            r.value(kind, &mut strings)?;
            match key.as_str() {
                "general.architecture" => out.architecture = strings.first().cloned(),
                "general.name" => out.name = strings.first().cloned(),
                _ => {}
            }
        }
        let mut known_bytes = true;
        for _ in 0..tensor_count {
            let name = r.string()?;
            let n_dims = r.u32()?;
            let mut elems: u64 = 1;
            for _ in 0..n_dims {
                elems = elems.saturating_mul(r.u64()?);
            }
            let ttype = r.u32()?;
            let _offset = r.u64()?;
            out.params = out.params.saturating_add(elems);
            *out.quants.entry(type_name(ttype)).or_default() += 1;
            match packet(ttype) {
                Some((bytes_per_packet, elements_per_packet)) => {
                    let packets = elems.div_ceil(elements_per_packet);
                    let b = packets.saturating_mul(bytes_per_packet);
                    out.largest.push((name, b));
                    match out.data_bytes.as_mut() {
                        Some(total) => *total = total.saturating_add(b),
                        None => out.data_bytes = Some(b),
                    }
                }
                None => known_bytes = false,
            }
        }
        if !known_bytes {
            out.data_bytes = None;
        }
        out.largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.largest.truncate(10);
        Ok(out)
    }

    pub fn summarise(path: &Path) -> Result<GgufSummary, AiError> {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        let meta = f.metadata()?;
        let want = meta.len().min(MAX_HEADER_BYTES) as usize;
        let mut buf = vec![0u8; want];
        let mut got = 0usize;
        while got < want {
            match f.read(&mut buf[got..])? {
                0 => break,
                n => got += n,
            }
        }
        buf.truncate(got);
        summarise_bytes(&buf)
    }
}

// ── Content-addressed model store ────────────────────────────────────────────
//
// Weights are the reason a "package cache" is not enough: 7 GB shared by six
// projects is 7 GB of disk or 42 GB, depending on whether the store dedupes. Files
// are stored once by SHA-256 and materialised by hard link (same filesystem, zero
// copy, zero extra bytes) with a copy fallback when linking is impossible.
pub mod cas {
    use super::{sha256_file, AiError, Path, PathBuf};

    /// Toggle the write bits on a stored entry. Read-only while stored; writable
    /// when an entry is being replaced.
    fn set_writable(path: &Path, writable: bool) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if writable { 0o644 } else { 0o444 };
            let mut p = std::fs::metadata(path)?.permissions();
            p.set_mode(mode);
            std::fs::set_permissions(path, p)?;
        }
        #[cfg(not(unix))]
        {
            let mut p = std::fs::metadata(path)?.permissions();
            p.set_readonly(!writable);
            std::fs::set_permissions(path, p)?;
        }
        Ok(())
    }

    #[derive(Debug, Clone)]
    pub struct ModelStore {
        root: PathBuf,
    }

    impl ModelStore {
        pub fn new(root: impl Into<PathBuf>) -> Self {
            Self { root: root.into() }
        }

        pub fn root(&self) -> &Path {
            &self.root
        }

        pub fn path_for(&self, digest: &str) -> PathBuf {
            let shard = digest.get(0..2).unwrap_or("00");
            self.root.join(shard).join(digest)
        }

        pub fn contains(&self, digest: &str) -> bool {
            self.path_for(digest).is_file()
        }

        /// Copy `file` into the store if absent; return its digest.
        ///
        /// An existing entry is **verified before it is trusted**. A store entry and
        /// the project copies materialised from it share one inode, so an in-place
        /// edit to a project file rewrites the stored bytes for everyone. Entries are
        /// therefore written read-only (an edit fails loudly instead of corrupting
        /// ten projects), and a poisoned entry is detected and replaced rather than
        /// propagated.
        pub fn put(&self, file: &Path) -> Result<String, AiError> {
            let digest = sha256_file(file)?;
            let dest = self.path_for(&digest);
            if dest.is_file() {
                if sha256_file(&dest).map(|d| d == digest).unwrap_or(false) {
                    return Ok(digest);
                }
                // Corrupt entry: make it writable again, then replace it.
                let _ = set_writable(&dest, true);
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = dest.with_extension("tmp");
            std::fs::copy(file, &tmp)?;
            std::fs::rename(&tmp, &dest)?;
            // Read-only: dedupe only stays safe while the shared inode cannot be
            // rewritten through a project.
            let _ = set_writable(&dest, false);
            Ok(digest)
        }

        /// Materialise `digest` at `dest`, preferring a hard link so two projects
        /// share one copy of the bytes.
        pub fn materialise(&self, digest: &str, dest: &Path) -> Result<(), AiError> {
            let src = self.path_for(digest);
            if !src.is_file() {
                return Err(AiError::Missing(format!("model store entry {digest}")));
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if dest.exists() {
                std::fs::remove_file(dest)?;
            }
            if sha256_file(&src)? != digest {
                return Err(AiError::DigestMismatch {
                    path: src.display().to_string(),
                    expected: digest.to_string(),
                    got: "stored bytes do not match their own name".to_string(),
                });
            }
            match std::fs::hard_link(&src, dest) {
                Ok(()) => Ok(()),
                // Different filesystem, or a platform/filesystem that refuses
                // links: a copy is slower but never wrong.
                Err(_) => {
                    std::fs::copy(&src, dest)?;
                    let _ = set_writable(dest, false);
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod ai_artifact_tests {
    use super::cas::ModelStore;
    use super::safetensors::{parse_file, parse_header, MAX_HEADER_BYTES};
    use std::io::Write;

    /// Build a real safetensors file: 8-byte LE length, JSON header, raw data.
    fn write_safetensors(path: &std::path::Path, tensors: &[(&str, &str, Vec<u64>)]) {
        let mut offset = 0u64;
        let mut header = serde_json::Map::new();
        header.insert(
            "__metadata__".to_string(),
            serde_json::json!({"format": "pt"}),
        );
        let mut data = Vec::new();
        for (name, dtype, shape) in tensors {
            let elems: u64 = shape.iter().product::<u64>().max(1);
            let bytes = elems * if *dtype == "F32" { 4 } else { 2 };
            header.insert(
                (*name).to_string(),
                serde_json::json!({
                    "dtype": dtype,
                    "shape": shape,
                    "data_offsets": [offset, offset + bytes],
                }),
            );
            data.extend(std::iter::repeat(0u8).take(bytes as usize));
            offset += bytes;
        }
        let json = serde_json::to_vec(&header).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(&(json.len() as u64).to_le_bytes()).unwrap();
        f.write_all(&json).unwrap();
        f.write_all(&data).unwrap();
    }

    /// Structure without weights: this is what makes a model a resolvable artifact
    /// rather than an opaque blob.
    #[test]
    fn header_only_parsing_reports_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("model.safetensors");
        write_safetensors(
            &p,
            &[
                ("model.embed.weight", "F32", vec![2, 3]),
                ("model.norm.weight", "F16", vec![3]),
            ],
        );
        let h = parse_file(&p).unwrap();
        assert_eq!(h.tensors.len(), 2);
        assert_eq!(h.total_params, 6 + 3);
        assert_eq!(h.dtypes.get("F32"), Some(&1));
        assert_eq!(h.dtypes.get("F16"), Some(&1));
        let emb = h.tensor("model.embed.weight").unwrap();
        assert_eq!(emb.elements(), 6);
        assert_eq!((emb.begin, emb.end), (0, 24), "F32 x 6 = 24 bytes");
        assert_eq!(h.data_start, 8 + h.header_bytes as u64);
    }

    /// Only the first `8 + N` bytes are needed, so a caller can range-fetch a
    /// header instead of a 7 GB file.
    #[test]
    fn parsing_needs_only_the_header_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("m.safetensors");
        write_safetensors(&p, &[("w", "F32", vec![4])]);
        let full = std::fs::read(&p).unwrap();
        let n = u64::from_le_bytes(full[..8].try_into().unwrap()) as usize;
        let prefix = &full[..8 + n];
        assert_eq!(parse_header(prefix).unwrap().total_params, 4);
        // Truncated header is an error, not a panic.
        assert!(parse_header(&full[..8 + n - 1]).is_err());
        // Absurd declared length is rejected rather than allocated.
        let mut hostile = vec![0u8; 8];
        hostile.copy_from_slice(&(MAX_HEADER_BYTES + 1).to_le_bytes());
        assert!(parse_header(&hostile).is_err());
    }

    /// Two projects pulling the same weights must not double the disk usage.
    #[test]
    fn store_dedupes_and_materialises_by_link() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().join("store"));
        let src = tmp.path().join("model.bin");
        std::fs::write(&src, b"weights-weights-weights").unwrap();

        let digest = store.put(&src).unwrap();
        assert!(store.contains(&digest));

        let a = tmp.path().join("projA/model.bin");
        let b = tmp.path().join("projB/model.bin");
        store.materialise(&digest, &a).unwrap();
        store.materialise(&digest, &b).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let ia = std::fs::metadata(&a).unwrap().ino();
            let ib = std::fs::metadata(&b).unwrap().ino();
            assert_eq!(ia, ib, "materialised files must share one copy of the bytes");
        }
    }

    /// The dedupe hazard: a project copy and the store entry share an inode, so an
    /// in-place edit rewrites the stored bytes. Read-only entries stop it, and a
    /// poisoned entry is detected and replaced instead of propagated.
    #[test]
    fn store_entries_are_read_only_and_repairable() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().join("store"));
        let src = tmp.path().join("w.bin");
        std::fs::write(&src, b"clean-weights").unwrap();
        let digest = store.put(&src).unwrap();

        // Simulate corruption that bypassed the permission bits (root, chmod, a
        // copy in from elsewhere).
        let entry = store.path_for(&digest);
        let _ = std::fs::set_permissions(&entry, {
            let mut p = std::fs::metadata(&entry).unwrap().permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                p.set_mode(0o644);
            }
            #[cfg(not(unix))]
            p.set_readonly(false);
            p
        });
        std::fs::write(&entry, b"corrupt!!!!").unwrap();

        // `put` of the good bytes repairs it.
        store.put(&src).unwrap();
        assert_eq!(std::fs::read(&entry).unwrap(), b"clean-weights");

        // And materialising never hands out bytes that disagree with the name.
        let bad = store.path_for(&digest);
        let _ = std::fs::set_permissions(&bad, {
            let mut p = std::fs::metadata(&bad).unwrap().permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                p.set_mode(0o644);
            }
            #[cfg(not(unix))]
            p.set_readonly(false);
            p
        });
        std::fs::write(&bad, b"tampered").unwrap();
        assert!(store.materialise(&digest, &tmp.path().join("out.bin")).is_err());
    }

    #[test]
    fn missing_store_entry_is_a_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path());
        let err = store
            .materialise("deadbeef", &tmp.path().join("x.bin"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("deadbeef"), "{err}");
    }
}
