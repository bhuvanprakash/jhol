//! Adversarial capsule reads: no panic (release builds abort), no UB, fail closed.
use std::path::Path;

fn build_capsule(path: &Path) {
    let mut b = jhol_core::capsule::CapsuleBuilder::new([0xab; 8]);
    let deps: &[(u32, u16)] = &[(1, 0)];
    b.add_package("left-pad", "1.0.0", &[0x01; 32], 1024, &[], 0, 0, "");
    b.add_package(
        "ms",
        "2.1.3",
        &[0x02; 32],
        512,
        deps,
        1,
        0,
        "https://example.invalid/ms.tgz",
    );
    b.add_package("@scope/foo", "2.1.3", &[0x03; 32], 2048, &[], 2, 0, "");
    jhol_core::capsule::write_capsule(path, b).expect("write");
}

/// If the capsule opens despite the tampering, every accessor must still be safe.
fn exercise(path: &Path) {
    if let Ok(view) = jhol_core::capsule::read_capsule(path) {
        for i in 0..view.package_count() {
            let e = view.entry(i);
            let (n, v, u) = (view.entry_name(e), view.entry_version(e), view.entry_url(e));
            // must be valid &str (the type guarantees it) and bounded
            assert!(n.len() < 4096 && v.len() < 4096 && u.len() < 4096);
            let _ = view.entry_deps(e); // bounded by the fix
        }
    }
}

#[test]
fn tampered_capsules_do_not_crash() {
    let td = tempfile::tempdir().unwrap();
    let good = td.path().join("good.capsule");
    build_capsule(&good);
    let bytes = std::fs::read(&good).unwrap();
    exercise(&good); // control: valid file reads fine

    // A) truncated to 20 bytes (header slice OOB)
    let a = td.path().join("trunc.capsule");
    std::fs::write(&a, &bytes[..20]).unwrap();
    assert!(
        jhol_core::capsule::read_capsule(&a).is_err(),
        "truncated capsule must be rejected"
    );
    exercise(&a);

    // B) string table (tail) overwritten with invalid UTF-8
    let b = td.path().join("badutf8.capsule");
    let mut bb = bytes.clone();
    let n = bb.len();
    for x in bb[n.saturating_sub(96)..].iter_mut() {
        *x = 0xFF;
    }
    std::fs::write(&b, &bb).unwrap();
    exercise(&b);

    // C) entries region corrupted (offsets/counts become nonsense)
    let c = td.path().join("badentries.capsule");
    let mut cc = bytes.clone();
    let end = 160.min(cc.len());
    for x in cc[64..end].iter_mut() {
        *x = 0x7F;
    }
    std::fs::write(&c, &cc).unwrap();
    exercise(&c);

    // D) every byte flipped in the first 256 bytes
    let d = td.path().join("badhead.capsule");
    let mut dd = bytes.clone();
    let head = 256.min(dd.len());
    for x in dd[..head].iter_mut() {
        *x = !*x;
    }
    std::fs::write(&d, &dd).unwrap();
    exercise(&d);

    // E) TOCTOU: open a valid capsule, then corrupt the mapped file underneath it.
    // The accessors are handed offsets that no longer fit the table, which is exactly the
    // input that used to panic (release builds abort) and could produce an invalid &str.
    let e = td.path().join("toctou.capsule");
    std::fs::copy(&good, &e).unwrap();
    let view = jhol_core::capsule::read_capsule(&e).expect("valid capsule opens");
    assert!(view.package_count() > 0);
    let count = view.package_count();
    let corrupt = vec![0xFFu8; std::fs::metadata(&e).unwrap().len() as usize];
    std::fs::write(&e, &corrupt).unwrap();
    for i in 0..count {
        let entry = view.entry(i);
        let (n, v, u) = (
            view.entry_name(entry),
            view.entry_version(entry),
            view.entry_url(entry),
        );
        let d = view.entry_deps(entry);
        // Bounded results only: the fix returns "" / &[] instead of slicing out of bounds.
        assert!(
            n.len() < 4096 && v.len() < 4096 && u.len() < 4096 && d.len() < 8192,
            "accessor returned an unbounded value after the file changed underneath"
        );
    }
    println!("  TOCTOU: accessors stayed bounded after the mapped file was corrupted (no panic)");
    println!("  adversarial: 5 tampered capsules + control handled with no panic/abort");
}
