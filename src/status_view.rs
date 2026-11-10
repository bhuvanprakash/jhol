//! The one place where model weight headers are merged into the status struct.
//!
//! `jhol-core` does not parse model files and must not: the AI crate owns that, and a
//! second reader would be a second answer. This module is the single composition point —
//! the CLI, the TUI and the web dashboard all call [`enrich`], so all three show the same
//! parameter counts and the same fit verdict.

use jhol_core::status::Status;
use std::path::{Path, PathBuf};

/// First `.safetensors` file in a directory, in sorted order so two runs agree.
fn first_safetensors(dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "safetensors").unwrap_or(false))
        .collect();
    files.sort();
    files.into_iter().next()
}

fn human_bytes(n: u64) -> String {
    let gb = 1024.0 * 1024.0 * 1024.0;
    let mb = 1024.0 * 1024.0;
    if n as f64 >= gb {
        format!("{:.1} GB", n as f64 / gb)
    } else {
        format!("{:.0} MB", n as f64 / mb)
    }
}

/// Fill model rows with what a header alone can tell us, and decide whether they fit.
///
/// A weight that cannot be measured is left `fits: None` with a stated reason — never a
/// pass. `None` means "we do not know", and the renderers print it that way.
pub fn enrich(mut status: Status, root: &Path) -> Status {
    let gpu_mb = status.axes.gpu_mem_mb;
    for artifact in status.artifacts.iter_mut() {
        if artifact.kind != "model" {
            continue;
        }
        let Some(dir) = artifact.path.as_deref() else {
            continue;
        };
        let dir = root.join(dir);
        let Some(file) = first_safetensors(&dir) else {
            artifact
                .reasons
                .push("no .safetensors file in the pinned directory".into());
            continue;
        };
        let summary = match jhol_ai::safetensors::summarise(&file) {
            Ok(s) => s,
            Err(e) => {
                artifact
                    .reasons
                    .push(format!("header unreadable: {e}"));
                continue;
            }
        };
        artifact.params = Some(summary.params);
        artifact.vram_bytes = Some(summary.vram_bytes());
        artifact.dtypes = summary.dtypes.clone();
        artifact
            .reasons
            .insert(0, format!("{} parameters", summary.params_human()));
        if !summary.unknown_dtypes.is_empty() {
            artifact.reasons.push(format!(
                "unknown dtype width: {}",
                summary.unknown_dtypes.join(", ")
            ));
        }

        let need = summary.vram_bytes();
        match gpu_mb {
            Some(mem) => {
                let need_mb = need.div_ceil(1024 * 1024);
                artifact.fits = Some(need_mb <= mem);
                artifact.reasons.push(format!(
                    "resident {} — host GPU {} MB, so it {}",
                    human_bytes(need),
                    mem,
                    if need_mb <= mem { "fits" } else { "does not fit" }
                ));
            }
            None => {
                artifact.fits = None;
                artifact.reasons.push(format!(
                    "resident {} — no GPU memory detected, so fit is unknown",
                    human_bytes(need)
                ));
            }
        }
    }
    status
}

/// `jhol inspect` and the dashboards resolve a model by name or by path with this.
pub fn resolve_weight_target(root: &Path, target: &str) -> Result<PathBuf, String> {
    let direct = Path::new(target);
    if direct.is_file() {
        return Ok(direct.to_path_buf());
    }
    if direct.is_dir() {
        return first_safetensors(direct)
            .ok_or_else(|| format!("{}: no .safetensors file", direct.display()));
    }
    let lock = jhol_core::model_lock::ModelLock::load(root)
        .ok_or_else(|| format!("{target}: no models.lock.json to resolve a name against"))?;
    let entry = lock
        .models
        .get(target)
        .ok_or_else(|| format!("{target}: not a path and not a pinned model"))?;
    first_safetensors(&root.join(&entry.dir))
        .ok_or_else(|| format!("{target}: pinned, but no .safetensors file on disk"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jhol_core::status::{Artifact, Axes, Status};

    fn status_with(dir: &str, gpu: Option<u64>) -> Status {
        let mut s = Status::default();
        s.axes = Axes { gpu_mem_mb: gpu, ..Default::default() };
        s.artifacts = vec![Artifact {
            kind: "model".into(),
            name: "w".into(),
            path: Some(dir.into()),
            present: true,
            ..Default::default()
        }];
        s
    }

    /// A safetensors file whose header declares one F32 tensor of `n` elements.
    fn write_weights(path: &Path, n: u64) {
        let header = format!(
            "{{\"t\":{{\"dtype\":\"F32\",\"shape\":[{n}],\"data_offsets\":[0,{}]}}}}",
            n * 4
        );
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend(std::iter::repeat(0u8).take((n * 4) as usize));
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_weight_is_measured_from_its_header_and_compared_to_the_host() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        write_weights(&dir.path().join("m/model.safetensors"), 1024);

        // 1024 F32 = 4096 bytes of tensors, 20% overhead -> ~4.9 KB, fits in 1 MB.
        let s = enrich(status_with("m", Some(1)), dir.path());
        let a = &s.artifacts[0];
        assert_eq!(a.params, Some(1024));
        assert_eq!(a.vram_bytes, Some(4096 + 819));
        assert_eq!(a.fits, Some(true));
        assert_eq!(a.dtypes.get("F32"), Some(&1));
    }

    #[test]
    fn without_a_gpu_the_verdict_is_unknown_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        write_weights(&dir.path().join("m/model.safetensors"), 1024);
        let s = enrich(status_with("m", None), dir.path());
        assert_eq!(s.artifacts[0].fits, None, "unknown must never read as a pass");
    }

    #[test]
    fn a_weight_too_big_for_the_host_is_a_failure_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        write_weights(&dir.path().join("m/model.safetensors"), 1024 * 1024); // 4 MiB
        let s = enrich(status_with("m", Some(1)), dir.path()); // 1 MB of GPU memory
        let a = &s.artifacts[0];
        assert_eq!(a.fits, Some(false));
        assert!(a.reasons.iter().any(|r| r.contains("does not fit")), "{:?}", a.reasons);
    }
}
