//! PyPI release inspection — which wheel would actually run here.
//!
//! This is the question ML engineers lose hours to: `pip install torch` succeeds,
//! then the import fails because the wheel was built for another CUDA, another
//! glibc, another Python ABI. The PyPI JSON API lists the files and their tags, and
//! the tags carry the whole compatibility statement — so the answer is available
//! before any download, and it should come with a reason, not a boolean.

use crate::axes::{wheel_fits, HostAxes, Incompatible, WheelTag};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelFile {
    pub filename: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub package: String,
    pub version: String,
    pub files: Vec<WheelFile>,
}

/// One file's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub filename: String,
    pub size: u64,
    /// `None` = compatible; `Some(reason)` = why not.
    pub reason: Option<String>,
}

/// Fetch a release's file list (version omitted = latest).
pub fn fetch_release(package: &str, version: Option<&str>) -> Result<Release, String> {
    if package.is_empty() || package.contains('/') || package.contains("..") {
        return Err(format!("invalid package name: {package}"));
    }
    let url = match version {
        Some(v) => format!("https://pypi.org/pypi/{package}/{v}/json"),
        None => format!("https://pypi.org/pypi/{package}/json"),
    };
    let body = crate::http_client::get(&url)?;
    let doc: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("{package}: {e}"))?;
    let version = doc
        .get("info")
        .and_then(|i| i.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let files = doc
        .get("urls")
        .and_then(|u| u.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some(WheelFile {
                        filename: f.get("filename")?.as_str()?.to_string(),
                        size: f.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                        url: f.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        package: package.to_string(),
        version,
        files,
    })
}

/// Classify every file of a release against the host, with reasons.
pub fn classify(release: &Release, host: &HostAxes) -> Vec<Verdict> {
    let mut out: Vec<Verdict> = release
        .files
        .iter()
        .map(|f| {
            // sdists carry no tags; they are "may work" rather than compatible.
            let reason = match WheelTag::parse(&f.filename) {
                Some(tag) => wheel_fits(&tag, host).err().map(reason_text),
                None if f.filename.ends_with(".tar.gz") || f.filename.ends_with(".zip") => {
                    Some("source distribution (build required)".to_string())
                }
                None => Some("unrecognised artifact".to_string()),
            };
            Verdict {
                filename: f.filename.clone(),
                size: f.size,
                reason,
            }
        })
        .collect();
    out.sort_by(|a, b| (a.reason.is_some(), &a.filename).cmp(&(b.reason.is_some(), &b.filename)));
    out
}

fn reason_text(i: Incompatible) -> String {
    i.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axes::Libc;

    fn mac_arm(py: &str) -> HostAxes {
        HostAxes {
            os: "macos".into(),
            arch: "arm64".into(),
            libc: Libc::Other,
            python: Some(py.into()),
            cuda_driver: None,
            sm: Vec::new(),
            gpu_mem_mb: None,
        }
    }

    #[test]
    fn classifies_with_reasons_not_booleans() {
        let release = Release {
            package: "torch".into(),
            version: "2.4.1".into(),
            files: vec![
                WheelFile { filename: "torch-2.4.1-cp312-cp312-macosx_11_0_arm64.whl".into(), size: 1, url: String::new() },
                WheelFile { filename: "torch-2.4.1-cp312-cp312-manylinux_2_17_aarch64.whl".into(), size: 1, url: String::new() },
                WheelFile { filename: "torch-2.4.1-cp311-cp311-macosx_11_0_arm64.whl".into(), size: 1, url: String::new() },
                WheelFile { filename: "torch-2.4.1.tar.gz".into(), size: 1, url: String::new() },
            ],
        };
        let verdicts = classify(&release, &mac_arm("3.12"));
        // Compatible first.
        assert!(verdicts[0].reason.is_none(), "{:?}", verdicts[0]);
        assert!(verdicts[0].filename.contains("cp312-cp312-macosx"));
        let reasons: Vec<String> = verdicts.iter().filter_map(|v| v.reason.clone()).collect();
        // A linux wheel on macOS names the platform, not a glibc version.
        assert!(reasons.iter().any(|r| r.contains("platform")), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("abi")), "cp311 vs cp312: {reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("source distribution")), "{reasons:?}");
    }
}
