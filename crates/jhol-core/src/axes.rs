//! Hardware and ABI axes — the part of an ML environment that package versions
//! cannot express.
//!
//! `torch==2.4.1` means nothing on its own: the wheel is `cp310-abi3-manylinux_2_17_x86_64`
//! or `cp312-cp312-macosx_11_0_arm64`, and only one of them can run here. pip installs
//! first and fails at import; this module answers the question *before* anything is
//! downloaded, from the wheel filename alone, because the filename carries the whole
//! compatibility statement.
//!
//! Axes are derived from two sources that already exist:
//!   * the **host** (os, arch, libc/glibc, python, and — when present — driver/GPU);
//!   * the **artifact** (wheel tags, or model card metadata).
//!
//! No tables to rot: `manylinux_2_17` *is* a glibc statement, `abi3` *is* a
//! forward-compatibility statement. Only the GPU/driver pairing needs external data,
//! and that is reported as detected facts rather than assumed.

use std::fmt;

/// libc flavour on Linux; irrelevant elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Libc {
    Glibc { major: u32, minor: u32 },
    Musl,
    Other,
}

impl fmt::Display for Libc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Libc::Glibc { major, minor } => write!(f, "glibc {major}.{minor}"),
            Libc::Musl => write!(f, "musl"),
            Libc::Other => write!(f, "libc unknown"),
        }
    }
}

/// What this machine is, in npm/PEP-425 vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAxes {
    /// Python-style platform os: `linux`, `macos`, `windows`.
    pub os: String,
    /// `x86_64`, `arm64`, `ppc64le`, …
    pub arch: String,
    pub libc: Libc,
    /// `3.11` etc. when known.
    pub python: Option<String>,
    /// CUDA driver version, when a driver is present.
    pub cuda_driver: Option<String>,
    /// Compute capabilities present (e.g. `[86]`), when detectable.
    pub sm: Vec<u32>,
    /// Total GPU memory in MiB of the largest device.
    pub gpu_mem_mb: Option<u64>,
}

impl HostAxes {
    /// Detect as much as the machine will tell us without side effects.
    ///
    /// `nvidia-smi` is consulted only when present; its absence is reported as
    /// "no CUDA", never as an error, because most machines are not GPU hosts.
    pub fn detect() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "macos".to_string(),
            other => other.to_string(),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x86_64".to_string(),
            "aarch64" => "arm64".to_string(),
            other => other.to_string(),
        };
        let libc = detect_libc();
        let python = std::env::var("JHOL_PYTHON_VERSION").ok();
        let (cuda_driver, sm, gpu_mem_mb) = detect_cuda();
        HostAxes {
            os,
            arch,
            libc,
            python,
            cuda_driver,
            sm,
            gpu_mem_mb,
        }
    }

    /// One-line summary for `doctor`.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} {}", self.os, self.arch), self.libc.to_string()];
        if let Some(py) = &self.python {
            parts.push(format!("python {py}"));
        }
        match &self.cuda_driver {
            Some(d) => parts.push(format!("cuda driver {d}")),
            None => parts.push("no cuda".to_string()),
        }
        if !self.sm.is_empty() {
            parts.push(format!("sm {}", self.sm.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")));
        }
        if let Some(mem) = self.gpu_mem_mb {
            parts.push(format!("{mem} MiB"));
        }
        parts.join(" · ")
    }
}

fn detect_libc() -> Libc {
    if cfg!(target_env = "musl") {
        return Libc::Musl;
    }
    if cfg!(target_os = "linux") {
        // `ldd --version` is the portable way to ask; glibc answers with its
        // version, musl with "musl libc".
        if let Ok(out) = std::process::Command::new("ldd").arg("--version").output() {
            let text = String::from_utf8_lossy(&out.stdout).to_string()
                + &String::from_utf8_lossy(&out.stderr);
            if text.to_ascii_lowercase().contains("musl") {
                return Libc::Musl;
            }
            if let Some(idx) = text.find(')') {
                let tail = text[idx + 1..].trim();
                let mut it = tail.split_whitespace();
                if let Some(ver) = it.next() {
                    let mut parts = ver.split('.');
                    if let (Some(ma), Some(mi)) = (parts.next(), parts.next()) {
                        if let (Ok(ma), Ok(mi)) = (ma.parse(), mi.parse()) {
                            return Libc::Glibc {
                                major: ma,
                                minor: mi,
                            };
                        }
                    }
                }
            }
        }
        return Libc::Other;
    }
    Libc::Other
}

fn detect_cuda() -> (Option<String>, Vec<u32>, Option<u64>) {
    let Ok(out) = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=driver_version,memory.total,compute_cap",
            "--format=csv,noheader",
        ])
        .output()
    else {
        return (None, Vec::new(), None);
    };
    if !out.status.success() {
        return (None, Vec::new(), None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut driver: Option<String> = None;
    let mut sm = Vec::new();
    let mut mem: Option<u64> = None;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').map(|f| f.trim()).collect();
        if fields.len() < 3 {
            continue;
        }
        if driver.is_none() && !fields[0].is_empty() {
            driver = Some(fields[0].to_string());
        }
        if let Some(mib) = fields[1].split_whitespace().next().and_then(|v| v.parse::<u64>().ok()) {
            mem = Some(mem.map_or(mib, |m: u64| m.max(mib)));
        }
        if !fields[2].is_empty() {
            if let Some((ma, mi)) = fields[2].split_once('.') {
                if let (Ok(ma), Ok(mi)) = (ma.parse::<u32>(), mi.parse::<u32>()) {
                    let cap = ma * 10 + mi;
                    if !sm.contains(&cap) {
                        sm.push(cap);
                    }
                }
            }
        }
    }
    sm.sort_unstable();
    (driver, sm, mem)
}

/// A wheel's compatibility statement, parsed from its filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelTag {
    /// `py3`, `cp311`, `cp312`…
    pub python: String,
    /// `none`, `abi3`, `cp311`…
    pub abi: String,
    /// `any`, `manylinux_2_17_x86_64`, `macosx_11_0_arm64`, …
    pub platform: String,
}

impl WheelTag {
    /// Parse `dist-version[-build]-py-abi-platform.whl`.
    pub fn parse(filename: &str) -> Option<Self> {
        let stem = filename.strip_suffix(".whl")?;
        let parts: Vec<&str> = stem.split('-').collect();
        if parts.len() < 5 {
            return None;
        }
        // The last three fields are py-abi-platform; anything before may include a
        // build tag, and the distribution name itself may contain hyphens? It may
        // not (PEP 427 replaces them with underscores), so taking from the end is
        // safe.
        let n = parts.len();
        Some(WheelTag {
            python: parts[n - 3].to_string(),
            abi: parts[n - 2].to_string(),
            platform: parts[n - 1].to_string(),
        })
    }
}

/// Why a wheel does not fit this machine. A reason, not a boolean, because "it does
/// not work here" is the sentence users have to guess their way out of today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incompatible {
    Platform { wheel: String, host: String },
    Python { wheel: String, host: String },
    Abi { wheel: String, host: String },
    Glibc { needs: (u32, u32), host: Libc },
    Arch { wheel: String, host: String },
}

impl fmt::Display for Incompatible {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Incompatible::Platform { wheel, host } => write!(f, "platform {wheel} != {host}"),
            Incompatible::Python { wheel, host } => {
                write!(f, "needs python {wheel}, host has {host}")
            }
            Incompatible::Abi { wheel, host } => write!(f, "abi {wheel} != {host}"),
            Incompatible::Glibc { needs, host } => write!(
                f,
                "needs glibc {}.{}, host has {host}",
                needs.0, needs.1
            ),
            Incompatible::Arch { wheel, host } => write!(f, "arch {wheel} != {host}"),
        }
    }
}

/// Does `tag` run here? Returns the specific reason when it does not.
pub fn wheel_fits(tag: &WheelTag, host: &HostAxes) -> Result<(), Incompatible> {
    // Pure-python wheels fit everywhere.
    if tag.platform.contains("any") && tag.abi == "none" {
        return Ok(());
    }

    // Python/ABI compatibility.
    if let Some(host_py) = &host.python {
        let host_py = normalise_python(host_py);
        match tag.abi.as_str() {
            // Stable ABI: works on the tagged version and every later one.
            "abi3" => {
                let want = parse_py(&tag.python);
                if parse_py(&host_py) < want {
                    return Err(Incompatible::Python {
                        wheel: format!(">={}", show_py(want)),
                        host: host_py,
                    });
                }
            }
            "none" => {
                if tag.python.starts_with("cp") {
                    // `abi: none` means "no ABI dependency", **not** "any
                    // interpreter": the python tag still names the version this
                    // wheel was built against (torch's macOS wheels are
                    // `cp312-none-macosx_11_0_arm64`). Treating those as universal
                    // is how a 3.8 wheel gets chosen for a 3.12 interpreter.
                    if parse_py(&host_py) != parse_py(&tag.python) {
                        return Err(Incompatible::Python {
                            wheel: tag.python.clone(),
                            host: host_py,
                        });
                    }
                } else if tag.python.starts_with("py") {
                    // Coarse tags: `py3` fits any 3.x.
                    let want = tag.python.trim_start_matches("py");
                    if !want.is_empty() && !host_py.starts_with(&want[..want.len().min(1)]) {
                        return Err(Incompatible::Python {
                            wheel: tag.python.clone(),
                            host: host_py,
                        });
                    }
                }
            }
            abi => {
                if parse_py(&host_py) != parse_py(abi) {
                    return Err(Incompatible::Abi {
                        wheel: abi.to_string(),
                        host: host_py,
                    });
                }
            }
        }
    }

    // Platform tags: `<family>_<version>_<arch>` or a bare arch.
    for alt in tag.platform.split('.') {
        if platform_alt_fits(alt, host).is_ok() {
            return Ok(());
        }
    }
    platform_reason(tag.platform.as_str(), host)
}

fn platform_reason(platform: &str, host: &HostAxes) -> Result<(), Incompatible> {
    let first = platform.split('.').next().unwrap_or(platform);
    if let Some((_, _, _, rest)) = split_platform_family(first) {
        // Wrong OS first: "linux wheel on macos" is a better sentence than
        // "needs glibc 2.17" when the machine has no glibc at all.
        if host.os != "linux" {
            return Err(Incompatible::Platform {
                wheel: format!("linux ({first})"),
                host: format!("{} {}", host.os, host.arch),
            });
        }
        if let Some((needs, arch)) = parse_manylinux(rest) {
            if !arch_matches(arch, &host.arch) {
                return Err(Incompatible::Arch {
                    wheel: arch.to_string(),
                    host: host.arch.clone(),
                });
            }
            return match host.libc {
                Libc::Glibc { major, minor } if (major, minor) >= needs => Ok(()),
                _ => Err(Incompatible::Glibc {
                    needs,
                    host: host.libc,
                }),
            };
        }
    }
    if let Some(rest) = first.strip_prefix("macosx_") {
        if host.os != "macos" {
            return Err(Incompatible::Platform {
                wheel: format!("macos ({first})"),
                host: format!("{} {}", host.os, host.arch),
            });
        }
        if let Some(arch) = rest.rsplit('_').next() {
            if !arch_matches(arch, &host.arch) && arch != "universal2" && arch != "intel" {
                return Err(Incompatible::Arch {
                    wheel: arch.to_string(),
                    host: host.arch.clone(),
                });
            }
            return Ok(());
        }
    }
    if arch_matches(first, &host.arch) {
        return Ok(());
    }
    Err(Incompatible::Platform {
        wheel: first.to_string(),
        host: format!("{} {}", host.os, host.arch),
    })
}

/// Do two architecture names mean the same machine?
///
/// PEP 425 uses `aarch64` on Linux and `arm64` on macOS for the same CPU; `x86_64`
/// and `amd64` likewise. String equality here rejects correct wheels, which is
/// exactly the class of mistake this module exists to remove.
fn arch_matches(wheel_arch: &str, host_arch: &str) -> bool {
    fn norm(a: &str) -> &str {
        match a {
            "aarch64" | "arm64" => "arm64",
            "x86_64" | "amd64" => "x86_64",
            "i686" | "i386" | "x86" => "x86",
            other => other,
        }
    }
    norm(wheel_arch) == norm(host_arch)
}

/// Split a platform tag into `(family, rest)` for the families we understand.
fn split_platform_family(tag: &str) -> Option<(&str, &str, &str, &str)> {
    for family in ["manylinux_", "manylinux2014_", "manylinux2010_", "manylinux1_"] {
        if let Some(rest) = tag.strip_prefix(family) {
            return Some((family, rest, "", rest));
        }
    }
    None
}

fn platform_alt_fits(alt: &str, host: &HostAxes) -> Result<(), ()> {
    if alt == "any" {
        return Ok(());
    }
    if let Some((_, _, _, rest)) = split_platform_family(alt) {
        if let Some((needs, arch)) = parse_manylinux(rest) {
            if !arch_matches(arch, &host.arch) {
                return Err(());
            }
            return match host.libc {
                Libc::Glibc { major, minor } if (major, minor) >= needs => Ok(()),
                _ => Err(()),
            };
        }
        return Err(());
    }
    if let Some(rest) = alt.strip_prefix("macosx_") {
        if let Some(arch) = rest.rsplit('_').next() {
            return if arch_matches(arch, &host.arch) || arch == "universal2" || arch == "intel" {
                Ok(())
            } else {
                Err(())
            };
        }
    }
    if arch_matches(alt, &host.arch) || alt == host.os {
        return Ok(());
    }
    Err(())
}

fn normalise_python(v: &str) -> String {
    let v = v.trim();
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        v.to_string()
    }
}

/// Parse both spellings into one representation.
///
/// `"3.12"` and `"cp312"` mean the same thing; `"cp39"` is 3.9, not 39.0. Reading
/// the tag as a plain integer is how a correct wheel gets rejected, so both forms go
/// through this one function.
fn parse_py(v: &str) -> (u32, u32) {
    let t = v.trim().trim_start_matches("cp").trim_start_matches("py");
    if let Some((a, b)) = t.split_once('.') {
        return (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
    }
    if t.len() >= 2 && t.chars().all(|c| c.is_ascii_digit()) {
        // cp312 -> 3.12, cp39 -> 3.9
        return (t[..1].parse().unwrap_or(0), t[1..].parse().unwrap_or(0));
    }
    (t.parse().unwrap_or(0), 0)
}

/// Parse a manylinux platform tag into `(glibc, arch)`.
///
/// Two spellings exist: the modern `manylinux_2_17_x86_64` (major_minor_arch) and
/// the legacy `manylinux2014_x86_64` (which *means* glibc 2.17). Splitting on the
/// first underscore — which is what I did first — turns `2_17_x86_64` into arch
/// `17_x86_64` and rejects a correct wheel.
fn parse_manylinux(rest: &str) -> Option<((u32, u32), &str)> {
    if let Some(legacy) = rest.strip_prefix("2014_") {
        return Some(((2, 17), legacy));
    }
    if let Some(legacy) = rest.strip_prefix("2010_") {
        return Some(((2, 12), legacy));
    }
    if let Some(legacy) = rest.strip_prefix("1_") {
        return Some(((2, 5), legacy));
    }
    let mut it = rest.splitn(3, '_');
    let ma: u32 = it.next()?.parse().ok()?;
    let mi: u32 = it.next()?.parse().ok()?;
    let arch = it.next()?;
    Some(((ma, mi), arch))
}

/// Render a parsed python version the way a human writes it.
fn show_py(v: (u32, u32)) -> String {
    format!("{}.{}", v.0, v.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(os: &str, arch: &str, libc: Libc, py: &str) -> HostAxes {
        HostAxes {
            os: os.into(),
            arch: arch.into(),
            libc,
            python: Some(py.into()),
            cuda_driver: None,
            sm: Vec::new(),
            gpu_mem_mb: None,
        }
    }

    #[test]
    fn parses_wheel_filenames() {
        let t = WheelTag::parse("numpy-2.1.0-cp312-cp312-manylinux_2_17_x86_64.whl").unwrap();
        assert_eq!(t.python, "cp312");
        assert_eq!(t.abi, "cp312");
        assert_eq!(t.platform, "manylinux_2_17_x86_64");

        // Build tag before the python tag must not shift the parse.
        let t = WheelTag::parse("pkg-1.0-2-py3-none-any.whl").unwrap();
        assert_eq!((t.python.as_str(), t.abi.as_str(), t.platform.as_str()), ("py3", "none", "any"));
        assert!(WheelTag::parse("not-a-wheel.txt").is_none());
    }

    #[test]
    fn manylinux_respects_glibc_and_arch() {
        let tag = WheelTag::parse("numpy-2.1.0-cp312-cp312-manylinux_2_17_x86_64.whl").unwrap();
        let ok = host("linux", "x86_64", Libc::Glibc { major: 2, minor: 35 }, "3.12");
        assert_eq!(wheel_fits(&tag, &ok), Ok(()));

        // Too old a glibc: named, with both versions.
        let old = host("linux", "x86_64", Libc::Glibc { major: 2, minor: 12 }, "3.12");
        assert_eq!(
            wheel_fits(&tag, &old),
            Err(Incompatible::Glibc { needs: (2, 17), host: old.libc })
        );

        // Wrong architecture.
        let arm = host("linux", "arm64", Libc::Glibc { major: 2, minor: 35 }, "3.12");
        assert!(matches!(wheel_fits(&tag, &arm), Err(Incompatible::Arch { .. })));

        // musl host, glibc wheel.
        let musl = host("linux", "x86_64", Libc::Musl, "3.12");
        assert!(matches!(wheel_fits(&tag, &musl), Err(Incompatible::Glibc { .. })));
    }

    #[test]
    fn abi3_is_forward_compatible_but_cp_tag_is_exact() {
        let abi3 = WheelTag::parse("cryptography-43.0-cp39-abi3-macosx_11_0_arm64.whl").unwrap();
        let mac = host("macos", "arm64", Libc::Other, "3.12");
        assert_eq!(wheel_fits(&abi3, &mac), Ok(()), "abi3 from 3.9 runs on 3.12");
        let older = host("macos", "arm64", Libc::Other, "3.8");
        assert!(matches!(wheel_fits(&abi3, &older), Err(Incompatible::Python { .. })));

        let exact = WheelTag::parse("torch-2.4.1-cp312-cp312-macosx_11_0_arm64.whl").unwrap();
        let py311 = host("macos", "arm64", Libc::Other, "3.11");
        assert!(matches!(wheel_fits(&exact, &py311), Err(Incompatible::Abi { .. })));
        assert_eq!(wheel_fits(&exact, &mac), Ok(()));
    }

    /// `cp312-none-macosx_11_0_arm64` is version-specific despite `abi: none`.
    #[test]
    fn cp_tagged_none_abi_is_version_specific() {
        let t = WheelTag::parse("torch-2.4.1-cp312-none-macosx_11_0_arm64.whl").unwrap();
        assert_eq!(t.abi, "none");
        assert_eq!(wheel_fits(&t, &host("macos", "arm64", Libc::Other, "3.12")), Ok(()));
        assert!(matches!(
            wheel_fits(&t, &host("macos", "arm64", Libc::Other, "3.11")),
            Err(Incompatible::Python { .. })
        ));
    }

    #[test]
    fn pure_python_is_universal() {
        let pure = WheelTag::parse("requests-2.32.0-py3-none-any.whl").unwrap();
        for h in [
            host("linux", "x86_64", Libc::Glibc { major: 2, minor: 12 }, "3.9"),
            host("macos", "arm64", Libc::Other, "3.13"),
        ] {
            assert_eq!(wheel_fits(&pure, &h), Ok(()));
        }
    }

    #[test]
    fn universal2_and_multi_platform_tags() {
        let univ = WheelTag::parse("pkg-1.0-cp312-cp312-macosx_11_0_universal2.whl").unwrap();
        assert_eq!(
            wheel_fits(&univ, &host("macos", "arm64", Libc::Other, "3.12")),
            Ok(())
        );
        // A wheel that lists two platforms is compatible if either fits.
        let multi = WheelTag::parse(
            "pkg-1.0-cp312-cp312-manylinux_2_17_aarch64.manylinux2014_aarch64.whl",
        )
        .unwrap();
        assert_eq!(
            wheel_fits(&multi, &host("linux", "arm64", Libc::Glibc { major: 2, minor: 35 }, "3.12")),
            Ok(())
        );
        assert!(wheel_fits(&multi, &host("linux", "x86_64", Libc::Glibc { major: 2, minor: 35 }, "3.12")).is_err());
    }
}
