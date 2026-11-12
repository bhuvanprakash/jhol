//! Unit tests for registry module

#[cfg(test)]
mod tests {

    use crate::registry;

    #[test]
    fn test_base_name_unscoped() {
        assert_eq!(registry::base_name("lodash"), "lodash");
        assert_eq!(registry::base_name("lodash@4.17.21"), "lodash");
        assert_eq!(registry::base_name("react@18"), "react");
    }

    #[test]
    fn test_base_name_scoped() {
        assert_eq!(registry::base_name("@babel/core"), "@babel/core");
        assert_eq!(registry::base_name("@babel/core@7.0.0"), "@babel/core");
        assert_eq!(registry::base_name("@types/node@18"), "@types/node");
    }

    #[test]
    fn test_base_name_edge_cases() {
        // Package names with @ in the middle
        assert_eq!(registry::base_name("pkg@1.0.0"), "pkg");
        // Scoped packages
        assert_eq!(registry::base_name("@scope/pkg"), "@scope/pkg");
        assert_eq!(registry::base_name("@scope/pkg@1.0"), "@scope/pkg");
    }

    #[test]
    fn test_version_satisfaction() {
        use semver::{Version, VersionReq};

        let v1_0_0 = Version::parse("1.0.0").unwrap();
        let v1_5_0 = Version::parse("1.5.0").unwrap();
        let v2_0_0 = Version::parse("2.0.0").unwrap();

        let req_exact = VersionReq::parse(">=1.0.0, <1.1.0").unwrap();
        let req_caret = VersionReq::parse("^1.0.0").unwrap();
        let req_tilde = VersionReq::parse("~1.0.0").unwrap();
        let req_star = VersionReq::parse("*").unwrap();

        // Exact range
        assert!(req_exact.matches(&v1_0_0));
        assert!(!req_exact.matches(&v1_5_0));

        // Caret (^) - compatible with version
        assert!(req_caret.matches(&v1_0_0));
        assert!(req_caret.matches(&v1_5_0));
        assert!(!req_caret.matches(&v2_0_0));

        // Tilde (~) - approximately equivalent
        assert!(req_tilde.matches(&v1_0_0));
        assert!(!req_tilde.matches(&v1_5_0));

        // Star (*) - any version
        assert!(req_star.matches(&v1_0_0));
        assert!(req_star.matches(&v2_0_0));
    }

    #[test]
    fn test_integrity_hash_format() {
        // SRI format: algorithm-base64hash
        let valid_sri = "sha512-abc123def456";
        assert!(valid_sri.starts_with("sha"));
        assert!(valid_sri.contains('-'));
    }

    #[test]
    fn test_version_satisfaction_with_spaced_comparators() {
        assert!(registry::version_satisfies(">=2.1.2", "2.1.2"));
        assert!(registry::version_satisfies(">=2.1.2", "2.9.9"));
        assert!(!registry::version_satisfies(">=2.1.2", "2.0.0"));
    }

    #[test]
    fn parsed_version_satisfaction_preserves_npm_range_semantics() {
        use semver::Version;

        for (range, raw_version, expected) in [
            ("*", "1.2.3", true),
            ("*", "1.2.3-nightly.1", false),
            ("4.1.6", "4.1.6", true),
            ("4.1.6", "4.2.11", false),
            ("^3.0.0 || ^4.0.0", "4.2.0", true),
            (">=0.6.2 <2.0.0", "1.2.1", true),
            ("^4.0.0", "5.0.0", false),
        ] {
            let parsed = Version::parse(raw_version).unwrap();
            assert_eq!(
                registry::version_satisfies_parsed(range, &parsed),
                expected,
                "parsed check: {range} against {raw_version}"
            );
            assert_eq!(
                registry::version_satisfies(range, raw_version),
                expected,
                "string check: {range} against {raw_version}"
            );
        }
    }
}

#[cfg(test)]
mod synth_invariant_tests {
    use crate::registry;

    /// Regression guard: a bundled-index synth must never advertise a version
    /// whose `dependencies` are unknown. It previously listed every bundled
    /// version but only populated deps for the latest, so the resolver could
    /// select an older version, receive an empty dependency map, and silently
    /// drop that version's whole subtree (seen as intermittent missing
    /// transitive packages on a cold install).
    #[test]
    fn bundled_synth_only_exposes_versions_with_known_deps() {
        // A few packages that appear in the bundled index; if none resolve
        // (e.g. index unavailable in this environment) the test is a no-op.
        let mut checked = 0;
        for (pkg, constraint) in [
            ("react", "^18.0.0"),
            ("lodash", "^4.17.0"),
            ("axios", "^1.0.0"),
        ] {
            let Some(synth) = registry::synthesize_packument_from_bundled_index(pkg, constraint)
            else {
                continue;
            };
            let versions = synth
                .get("versions")
                .and_then(|v| v.as_object())
                .expect("synth has versions");
            assert!(!versions.is_empty(), "{pkg}: empty synth");
            for (ver, entry) in versions {
                assert!(
                    entry.get("dependencies").is_some(),
                    "{pkg}@{ver}: synth version without dependencies — a synth may only \
                     advertise versions whose deps are known"
                );
            }
            checked += 1;
        }
        if checked == 0 {
            eprintln!("note: bundled index unavailable; synth invariant not exercised");
        }
    }
}

#[cfg(test)]
mod spec_satisfies_tests {
    use crate::registry;

    #[test]
    fn multi_comparator_range_matches() {
        assert!(registry::version_satisfies(">=0.6.2 <2.0.0", "1.2.1"));
        assert!(registry::version_satisfies(">=0.6.2 <2.0.0", "0.6.2"));
        assert!(!registry::version_satisfies(">=0.6.2 <2.0.0", "2.0.0"));
        assert!(registry::version_satisfies(">= 0.6.2 < 2.0.0", "1.2.1"));
    }
}

#[cfg(test)]
mod exact_pin_tests {
    use crate::registry::is_exact_version_pin;

    #[test]
    fn plain_versions_are_pins() {
        assert!(is_exact_version_pin("1.2.3"));
        assert!(is_exact_version_pin("v1.2.3"));
        assert!(is_exact_version_pin("=1.2.3"));
    }

    #[test]
    fn prerelease_versions_are_pins() {
        // Regression: the hyphen inside a prerelease was mistaken for a hyphen
        // range, so `pkg@1.64.0-alpha-2026-09-16` was resolved as a *range* and a
        // newer prerelease could be chosen instead of the pinned one.
        assert!(is_exact_version_pin("1.64.0-alpha-2026-09-16"));
        assert!(is_exact_version_pin("1.2.3-beta.1"));
        assert!(is_exact_version_pin("2.0.0-rc.1+build.5"));
    }

    #[test]
    fn ranges_and_partials_are_not_pins() {
        assert!(!is_exact_version_pin("^1.2.3"));
        assert!(!is_exact_version_pin("~1.2.3"));
        assert!(!is_exact_version_pin(">=1.2.3"));
        assert!(!is_exact_version_pin("1.2"));
        assert!(!is_exact_version_pin("1"));
        assert!(!is_exact_version_pin("1.x"));
        assert!(!is_exact_version_pin("*"));
        assert!(!is_exact_version_pin("1.2.3 - 2.0.0"));
        assert!(!is_exact_version_pin(""));
    }
}

/// A malicious package tarball must not be able to write outside the directory it
/// is extracted into. `extract_tar_from_bytes` strips the leading `package/`
/// component and joins the rest onto the destination — if `..` components survive
/// that join, a published tarball could escape `node_modules` and overwrite
/// arbitrary files (a classic supply-chain attack on package managers).
///
/// The header is built by hand because `tar::Builder` refuses to write such a path —
/// which is itself a hint that the reader may guard it too. This test settles that.
#[test]
fn tar_entry_path_traversal_is_blocked() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn octal(field: &mut [u8], value: u64) {
        let s = format!("{:0width$o}", value, width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
        field[s.len()] = 0;
    }

    fn entry(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], 0o644); // mode
        octal(&mut h[108..116], 0); // uid
        octal(&mut h[116..124], 0); // gid
        octal(&mut h[124..136], payload.len() as u64); // size
        octal(&mut h[136..148], 0); // mtime
        h[148..156].copy_from_slice(b"        "); // chksum placeholder
        h[156] = b'0'; // regular file
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u64 = h.iter().map(|b| *b as u64).sum();
        octal(&mut h[148..156], sum);
        let mut out = h.to_vec();
        out.extend_from_slice(payload);
        out.resize(512 + payload.len().div_ceil(512) * 512, 0);
        out
    }

    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("node_modules").join("evil");
    let outside = td.path().join("node_modules/escaped.txt");

    // Positive control: prove this archive builder reaches the extractor.
    let mut benign = entry("package/probe.txt", b"ok");
    benign.extend_from_slice(&[0u8; 1024]);
    let mut control = GzEncoder::new(Vec::new(), Compression::fast());
    control.write_all(&benign).unwrap();
    let control_path = td.path().join("control.tgz");
    std::fs::write(&control_path, control.finish().unwrap()).unwrap();
    crate::registry::extract_tarball_to_dir(&control_path, &dest).unwrap();
    assert_eq!(std::fs::read(dest.join("probe.txt")).unwrap(), b"ok");

    let mut tar_bytes = Vec::new();
    tar_bytes.extend_from_slice(&entry("package/../escaped.txt", b"pwned"));
    tar_bytes.extend_from_slice(&entry("package/absolute-probe.txt", b"ok"));
    tar_bytes.extend_from_slice(&[0u8; 1024]); // end-of-archive

    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&tar_bytes).unwrap();
    let tgz_path = td.path().join("evil-1.0.0.tgz");
    std::fs::write(&tgz_path, gz.finish().unwrap()).unwrap();

    let error = crate::registry::extract_tarball_to_dir(&tgz_path, &dest).unwrap_err();
    assert!(error.contains("Unsafe archive entry path"), "{error}");

    assert!(
        !outside.exists(),
        "path traversal: a tarball entry escaped the extraction dir and wrote {}",
        outside.display()
    );
}

/// The lexical `..` check does not stop a **symlink escape**: a tarball can ship a
/// symlink pointing outside the destination and then write *through* it. tar-rs
/// validates each entry path lexically, so this is the variant that needs its own
/// answer.
#[test]
fn tar_entry_symlink_escape_is_blocked() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn octal(field: &mut [u8], value: u64) {
        let s = format!("{:0width$o}", value, width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
        field[s.len()] = 0;
    }

    fn build(name: &str, linkname: &str, typeflag: u8, payload: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], 0o777);
        octal(&mut h[108..116], 0);
        octal(&mut h[116..124], 0);
        octal(&mut h[124..136], payload.len() as u64);
        octal(&mut h[136..148], 0);
        h[148..156].copy_from_slice(b"        ");
        h[156] = typeflag;
        if !linkname.is_empty() {
            h[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
        }
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u64 = h.iter().map(|b| *b as u64).sum();
        octal(&mut h[148..156], sum);
        let mut out = h.to_vec();
        if !payload.is_empty() {
            out.extend_from_slice(payload);
            out.resize(512 + payload.len().div_ceil(512) * 512, 0);
        }
        out
    }

    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("node_modules").join("evil");
    let outside_dir = td.path().join("node_modules/outside");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let victim = outside_dir.join("pwned.txt");

    let mut tar_bytes = Vec::new();
    // symlink inside the package pointing at a directory outside the destination
    tar_bytes.extend_from_slice(&build("package/link", "../outside", b'2', b""));
    // then write through the symlink
    tar_bytes.extend_from_slice(&build("package/link/pwned.txt", "", b'0', b"pwned"));
    tar_bytes.extend_from_slice(&[0u8; 1024]);

    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&tar_bytes).unwrap();
    let tgz_path = td.path().join("evil-2.0.0.tgz");
    std::fs::write(&tgz_path, gz.finish().unwrap()).unwrap();

    let error = crate::registry::extract_tarball_to_dir(&tgz_path, &dest).unwrap_err();
    assert!(
        error.contains("Unsafe archive symlink target")
            || error.contains("Unsafe archive destination symlink"),
        "expected the escaping symlink to be refused, got: {error}"
    );

    assert!(
        !victim.exists(),
        "symlink escape: tarball wrote through a symlink to {}",
        victim.display()
    );
}

/// The symlink guard checks `symlink_metadata` on each destination component,
/// but a **hard link** is not a symlink: it resolves to a regular file, so that
/// guard cannot see it. If tar-rs honours a hardlink whose target lies outside
/// the destination, a later entry that writes to the same path would modify the
/// file outside — the classic hardlink archive attack.
#[test]
fn tar_entry_hardlink_escape_is_blocked() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn octal(field: &mut [u8], value: u64) {
        let s = format!("{:0width$o}", value, width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
        field[s.len()] = 0;
    }

    fn build(name: &str, linkname: &str, typeflag: u8, payload: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], 0o777);
        octal(&mut h[124..136], payload.len() as u64);
        h[148..156].copy_from_slice(b"        ");
        h[156] = typeflag;
        if !linkname.is_empty() {
            h[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
        }
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u64 = h.iter().map(|b| *b as u64).sum();
        octal(&mut h[148..156], sum);
        let mut out = h.to_vec();
        if !payload.is_empty() {
            out.extend_from_slice(payload);
            out.resize(512 + payload.len().div_ceil(512) * 512, 0);
        }
        out
    }

    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("node_modules").join("evil");
    let outside_dir = td.path().join("node_modules").join("outside");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let victim = outside_dir.join("victim.txt");
    std::fs::write(&victim, b"original").unwrap();

    let mut tar_bytes = Vec::new();
    // hard link pointing at a file outside the extraction directory
    tar_bytes.extend_from_slice(&build("package/x", "../outside/victim.txt", b'1', b""));
    // a later entry writing the same path follows the link and rewrites the victim
    tar_bytes.extend_from_slice(&build("package/x", "", b'0', b"pwned"));
    tar_bytes.extend_from_slice(&[0u8; 1024]);

    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&tar_bytes).unwrap();
    let tgz_path = td.path().join("evil-3.0.0.tgz");
    std::fs::write(&tgz_path, gz.finish().unwrap()).unwrap();

    let result = crate::registry::extract_tarball_to_dir(&tgz_path, &dest);
    let error = result.unwrap_err();
    assert!(
        error.contains("Unsafe archive hard link"),
        "expected the hard link to be refused, got: {error}"
    );
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"original",
        "hardlink escape: {} was modified by extraction",
        victim.display()
    );
    assert!(
        !dest.join("x").exists(),
        "hardlink escape: the refused archive still materialized {}",
        dest.join("x").display()
    );
    assert_eq!(
        std::fs::read_dir(&dest).unwrap().count(),
        0,
        "hardlink escape: refused extraction left artifacts in {}",
        dest.display()
    );
    // Benign control: an ordinary archive still extracts fully.
    let mut benign = build("package/ok.txt", "", b'0', b"fine");
    benign.extend_from_slice(&[0u8; 1024]);
    let mut gz2 = GzEncoder::new(Vec::new(), Compression::fast());
    gz2.write_all(&benign).unwrap();
    let ok_path = td.path().join("ok.tgz");
    std::fs::write(&ok_path, gz2.finish().unwrap()).unwrap();
    crate::registry::extract_tarball_to_dir(&ok_path, &dest).unwrap();
    assert_eq!(std::fs::read(dest.join("ok.txt")).unwrap(), b"fine");
    assert_eq!(std::fs::read(&victim).unwrap(), b"original");
}

/// The symlink guard must allow targets that stay inside the extraction root,
/// otherwise a legitimate archive would be rejected. Pairs with the escaping
/// case above to show the guard discriminates rather than blanket-refusing.
#[test]
fn tar_entry_symlinks_are_materialized_or_reported() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn octal(field: &mut [u8], value: u64) {
        let s = format!("{:0width$o}", value, width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
        field[s.len()] = 0;
    }

    fn build(name: &str, linkname: &str, typeflag: u8, payload: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], 0o777);
        octal(&mut h[124..136], payload.len() as u64);
        h[148..156].copy_from_slice(b"        ");
        h[156] = typeflag;
        if !linkname.is_empty() {
            h[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
        }
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u64 = h.iter().map(|b| *b as u64).sum();
        octal(&mut h[148..156], sum);
        let mut out = h.to_vec();
        if !payload.is_empty() {
            out.extend_from_slice(payload);
            out.resize(512 + payload.len().div_ceil(512) * 512, 0);
        }
        out
    }

    let td = tempfile::tempdir().unwrap();
    let dest = td.path().join("node_modules").join("good");
    let mut tar_bytes = Vec::new();
    tar_bytes.extend_from_slice(&build("package/real.js", "", b'0', b"real"));
    tar_bytes.extend_from_slice(&build("package/assets/", "", b'5', b""));
    // links inside the package: one sibling, one from a nested directory
    tar_bytes.extend_from_slice(&build("package/alias.js", "real.js", b'2', b""));
    tar_bytes.extend_from_slice(&build("package/sub/up.js", "../real.js", b'2', b""));
    tar_bytes.extend_from_slice(&build("package/assets-link", "assets", b'2', b""));
    tar_bytes.extend_from_slice(&[0u8; 1024]);

    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&tar_bytes).unwrap();
    let tgz_path = td.path().join("good-1.0.0.tgz");
    std::fs::write(&tgz_path, gz.finish().unwrap()).unwrap();

    crate::registry::extract_tarball_to_dir(&tgz_path, &dest)
        .expect("internal symlinks must be allowed");
    assert_eq!(std::fs::read(dest.join("real.js")).unwrap(), b"real");
    assert_eq!(std::fs::read(dest.join("alias.js")).unwrap(), b"real");
    assert_eq!(std::fs::read(dest.join("sub/up.js")).unwrap(), b"real");
    let assets_link = dest.join("assets-link");
    assert!(assets_link.is_dir());
    assert!(std::fs::symlink_metadata(assets_link)
        .unwrap()
        .file_type()
        .is_symlink());

    let mut conflicting_tar = Vec::new();
    conflicting_tar.extend_from_slice(&build("package/conflict/", "", b'5', b""));
    conflicting_tar.extend_from_slice(&build("package/conflict", "target", b'2', b""));
    conflicting_tar.extend_from_slice(&[0u8; 1024]);
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&conflicting_tar).unwrap();
    let conflict_tgz = td.path().join("conflict-1.0.0.tgz");
    std::fs::write(&conflict_tgz, gz.finish().unwrap()).unwrap();
    let conflict_dest = td.path().join("node_modules/conflict");
    let error = crate::registry::extract_tarball_to_dir(&conflict_tgz, &conflict_dest)
        .expect_err("failed symlink creation must fail the extraction");
    assert!(error.contains("Failed to create symlink"), "got: {error}");
}

/// npm/semver exclude prereleases unless the range itself names a prerelease on
/// the same major.minor.patch. A nightly leaking into a plain `^2` tree is a real
/// supply-chain hazard, so pin the rule.
#[test]
fn prerelease_version_does_not_satisfy_stable_range() {
    let nightly = "4.0.0-nightly.202508271359";
    assert!(!crate::registry::version_satisfies("^2.1.3", nightly));
    assert!(!crate::registry::version_satisfies("^2.1.1", nightly));
    assert!(!crate::registry::version_satisfies(">=2.0.0", nightly));
    assert!(!crate::registry::version_satisfies("2.0.0", nightly));
    // an explicit prerelease range must still match (E77 explicit pins)
    assert!(crate::registry::version_satisfies(
        "^4.0.0-nightly.202508271359",
        nightly
    ));
    // and stable versions still satisfy stable ranges
    assert!(crate::registry::version_satisfies("^2.1.3", "2.1.3"));
    // `*` and an empty spec follow the same npm rule: stable only, so a
    // max-version selection cannot hoist a nightly through `"dep": "*"`.
    assert!(!crate::registry::version_satisfies("*", nightly));
    assert!(!crate::registry::version_satisfies("", nightly));
    assert!(crate::registry::version_satisfies("*", "2.1.3"));
    assert!(crate::registry::version_satisfies("", "2.1.3"));
}

/// npm's `*` (and an empty range) matches **stable** versions only: semver excludes
/// prereleases unless the range itself carries one. If a plain `*` can return a
/// nightly, any package declaring `"dep": "*"` would get an unreleased build —
/// the same canary-in-the-tree class as E99, just through a different function.
#[test]
fn star_range_does_not_pick_a_prerelease() {
    let versions: Vec<String> = ["2.0.0", "2.1.3", "4.0.0-nightly.202508271359"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for range in ["*", ""] {
        assert_eq!(
            crate::registry::resolve_range(&versions, range).as_deref(),
            Some("2.1.3"),
            "range {range:?} must resolve to the newest stable version"
        );
    }
    // An explicit prerelease range must still be able to select it.
    assert_eq!(
        crate::registry::resolve_range(&versions, "^4.0.0-nightly.202508271359").as_deref(),
        Some("4.0.0-nightly.202508271359")
    );
    // A prerelease-only package must remain installable.
    let only_pre: Vec<String> = ["1.0.0-beta.1", "1.0.0-beta.2"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        crate::registry::resolve_range(&only_pre, "*").as_deref(),
        Some("1.0.0-beta.2")
    );
}

#[cfg(test)]
mod streamed_store_tests {
    use base64::Engine;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sha2::{Digest, Sha256, Sha512};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn tarball() -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut archive = tar::Builder::new(&mut tar_bytes);
            let content = b"streamed tarball body";
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, "package/index.js", &content[..])
                .unwrap();
            archive.finish().unwrap();
        }
        let mut compressed = Vec::new();
        let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();
        compressed
    }

    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            stream.flush().unwrap();
        });
        format!("http://{address}/package.tgz")
    }

    #[test]
    fn network_unpack_wrapper_streams_to_store_and_projects_with_sri() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = tarball();
        let url = serve_once(bytes.clone());
        let integrity = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&bytes))
        );
        let expected_hash = format!("{:x}", Sha256::digest(&bytes));

        let (hash, unpacked) = crate::registry::download_unpack_to_store_unpacked(
            &url,
            temp.path(),
            "jhol-stream-fixture@1.0.0",
            Some(&integrity),
        )
        .unwrap();

        assert_eq!(hash, expected_hash);
        assert_eq!(
            std::fs::read(temp.path().join("store").join(format!("{hash}.tgz"))).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("store").join(format!("{hash}.tgz.sri")))
                .unwrap(),
            integrity,
            "the verified registry SRI should be memoized without rereading the tarball"
        );
        assert_eq!(
            unpacked,
            temp.path()
                .join("store_unpacked")
                .join(&hash[..2])
                .join(&hash)
        );
        assert!(crate::registry::store_extract_complete(&unpacked));
        assert_eq!(
            std::fs::read(unpacked.join("index.js")).unwrap(),
            b"streamed tarball body"
        );
    }

    #[test]
    fn failed_stream_integrity_does_not_publish_sri_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = tarball();
        let url = serve_once(bytes.clone());
        let wrong_integrity = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(b"different bytes"))
        );
        let hash = format!("{:x}", Sha256::digest(&bytes));

        let result = crate::registry::download_tarball_to_store_hash_only(
            &url,
            temp.path(),
            "jhol-stream-fixture@1.0.0",
            Some(&wrong_integrity),
        );

        assert!(result.is_err(), "integrity mismatch must fail closed");
        assert!(
            !temp
                .path()
                .join("store")
                .join(format!("{hash}.tgz.sri"))
                .exists(),
            "an unverified digest must never become a trusted sidecar"
        );
    }

    #[test]
    fn missing_store_unpack_streams_tarball_into_verified_content_address() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = tarball();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let store = temp.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let tarball_path = store.join(format!("{hash}.tgz"));
        std::fs::write(&tarball_path, bytes).unwrap();
        let cache = temp.path().join("cache");

        assert!(
            crate::registry::ensure_store_unpacked_for_tarball_in(&tarball_path, &cache).unwrap()
        );
        let unpacked = cache.join("store_unpacked").join(&hash[..2]).join(&hash);
        assert!(crate::registry::store_extract_complete(&unpacked));
        assert_eq!(
            std::fs::read(unpacked.join("index.js")).unwrap(),
            b"streamed tarball body"
        );
    }

    #[test]
    fn durable_mirror_writes_a_packument_into_every_root() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("durable-a");
        let b = td.path().join("durable-b");
        let bytes = b"etag-1\tgzip\npayload-bytes";
        crate::origin::mirror_packument_to_roots(
            bytes,
            std::ffi::OsStr::new("deadbeef.json"),
            &[a.clone(), b.clone()],
        );
        assert_eq!(
            std::fs::read(a.join("packuments").join("deadbeef.json")).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(b.join("packuments").join("deadbeef.json")).unwrap(),
            bytes
        );
    }

    #[test]
    fn durable_mirror_is_atomic_over_an_existing_file() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        let dst = root.join("packuments").join("k.json");
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&dst, b"old").unwrap();
        crate::origin::mirror_packument_to_roots(
            b"new",
            std::ffi::OsStr::new("k.json"),
            &[root.to_path_buf()],
        );
        assert_eq!(std::fs::read(&dst).unwrap(), b"new");
        // The temp sibling used for the atomic swap must not be left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dst.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }
}

/// Freshness contract for the build-time / machine-local bundled index: online it
/// may answer an exact pin, never a range or `latest` (a snapshot would install an
/// outdated version, e.g. dayjs 1.11.19 while the registry has 1.11.23).
#[cfg(test)]
mod bundled_freshness_tests {
    use crate::registry::{bundled_index_allowed, offline_mode};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    /// When online and not asked otherwise, the bundle answers **nothing** — not even an exact
    /// pin. It used to answer pins, and that made a build-time snapshot the resolver of record:
    /// the greedy estimate pinned `mime@1.6.0` from the bundle, the pin was served from the same
    /// bundle, and the chart solver — handed a one-version packument — reported a conflict, so the
    /// estimate stood and jhol installed trees no other tool produces.
    ///
    /// A snapshot can be a fallback (registry unreachable) or an explicit preference. It cannot be
    /// an authority while a registry is reachable.
    fn online_bundle_answers_nothing_unless_asked() {
        if offline_mode() || std::env::var("JHOL_PREFER_BUNDLED").is_ok() {
            return; // environment sets the policy that is under test
        }
        assert!(!bundled_index_allowed(&s(&["1.11.19"])));
        assert!(!bundled_index_allowed(&s(&["1.11.19", "=1.11.19"])));
        assert!(!bundled_index_allowed(&s(&["^1.11.13"])));
        assert!(!bundled_index_allowed(&s(&["latest"])));
        assert!(!bundled_index_allowed(&[]));
    }
}
