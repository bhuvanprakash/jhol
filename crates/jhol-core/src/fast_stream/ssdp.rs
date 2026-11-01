//! Stream-to-Store Direct Projection (SSDP 2.0)
//!
//! Streams compressed input through gzip/tar extraction into the content-addressed store.
//! Neither compressed bodies nor fully expanded archives need whole-object memory buffers.

use sha2::{Digest, Sha256, Sha384, Sha512};
use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tar::Archive;

const STORE_COMPLETE_MARKER: &str = ".jhol-extract-complete";
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Stream Projection Result containing content hash and extracted metadata
#[derive(Debug, Clone)]
pub struct ProjectionResult {
    pub hash: String,
    pub unpacked_dir: PathBuf,
    pub files_extracted: usize,
    pub bytes_decompressed: usize,
}

/// Single-pass streaming unpacker and direct projector
pub struct StreamDirectProjector;

impl StreamDirectProjector {
    /// Streams a verified gzip tarball into `store_unpacked/<shard>/<hash>`.
    /// Compressed input remains resident; expanded archive bytes do not.
    pub fn project_stream(
        raw_gzip: &[u8],
        cache_dir: &Path,
        pkg_key: &str,
        expected_sri: Option<&str>,
        target_nm_link: Option<&Path>,
    ) -> Result<ProjectionResult, String> {
        if expected_sri.is_some_and(|sri| !crate::utils::verify_sri_bytes(raw_gzip, sri)) {
            return Err(format!("SSDP: Integrity check failed for {}", pkg_key));
        }
        let hash = format!("{:x}", Sha256::digest(raw_gzip));
        let shard = if hash.len() >= 2 { &hash[..2] } else { "xx" };
        let unpacked_dir = cache_dir.join("store_unpacked").join(shard).join(&hash);
        if unpacked_dir.join(STORE_COMPLETE_MARKER).is_file() {
            if let Some(nm_dest) = target_nm_link {
                Self::link_or_copy_package(&unpacked_dir, nm_dest)?;
            }
            return Ok(ProjectionResult {
                hash,
                unpacked_dir,
                files_extracted: 0,
                bytes_decompressed: 0,
            });
        }
        Self::project_reader(
            Cursor::new(raw_gzip),
            cache_dir,
            pkg_key,
            // `raw_gzip` is immutable and SRI-verified above; avoid rehashing
            // the same bytes during this in-memory projection.
            None,
            target_nm_link,
        )
    }

    /// Hash, verify, decompress, and extract a compressed tar stream without
    /// retaining the compressed body or expanded archive in memory.
    pub fn project_reader<R: Read>(
        reader: R,
        cache_dir: &Path,
        pkg_key: &str,
        expected_sri: Option<&str>,
        target_nm_link: Option<&Path>,
    ) -> Result<ProjectionResult, String> {
        Self::project_reader_checked(
            reader,
            cache_dir,
            pkg_key,
            expected_sri,
            None,
            target_nm_link,
        )
    }

    /// Like `project_reader`, but reject a stream whose SHA-256 does not match
    /// its expected content-addressed store key before publishing extracted files.
    pub(crate) fn project_reader_checked<R: Read>(
        reader: R,
        cache_dir: &Path,
        pkg_key: &str,
        expected_sri: Option<&str>,
        expected_sha256: Option<&str>,
        target_nm_link: Option<&Path>,
    ) -> Result<ProjectionResult, String> {
        let expected = match expected_sri {
            Some(sri) => Some(
                crate::utils::parse_sri(sri)
                    .ok_or_else(|| format!("SSDP: Integrity check failed for {}", pkg_key))?,
            ),
            None => None,
        };
        if expected
            .as_ref()
            .is_some_and(|(algo, _)| algo != "sha512" && algo != "sha384")
        {
            return Err(format!("SSDP: Integrity check failed for {}", pkg_key));
        }

        // Stage privately. Final hash path is unknown until the stream is read.
        let staging_parent = cache_dir.join("store_unpacked").join(".staging");
        std::fs::create_dir_all(&staging_parent)
            .map_err(|e| format!("SSDP: Failed to create staging parent: {}", e))?;
        let stage_path = loop {
            let id = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = staging_parent.join(format!("{}-{}.tmp", std::process::id(), id));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("SSDP: Failed to create staging dir: {}", e)),
            }
        };
        let mut stage_guard = crate::safe_file::TmpDirGuard::new(stage_path.clone());

        let mut digester = DigestingReader::new(reader, expected.as_ref().map(|(algo, _)| algo));
        let mut decoder = CountingReader {
            inner: flate2::read::MultiGzDecoder::new(&mut digester),
            bytes_read: 0,
        };
        let mut created_dirs: HashSet<PathBuf> = HashSet::with_capacity(32);
        created_dirs.insert(stage_path.clone());
        let mut archive_symlinks: HashSet<PathBuf> = HashSet::new();
        let mut files_extracted = 0;

        {
            let mut archive = Archive::new(&mut decoder);
            for entry in archive
                .entries()
                .map_err(|e| format!("SSDP tar error: {}", e))?
            {
                let mut entry = entry.map_err(|e| format!("SSDP entry error: {}", e))?;
                let (out_path, entry_type, is_symlink, symlink_relative) = {
                    let path_bytes = entry.path_bytes();
                    let Ok(path_str) = std::str::from_utf8(&path_bytes) else {
                        continue;
                    };
                    let Some(slash_idx) = path_str.find('/') else {
                        continue;
                    };
                    let rel_str = &path_str[slash_idx + 1..];
                    if rel_str.is_empty() {
                        continue;
                    }
                    let parts: smallvec::SmallVec<[&str; 8]> =
                        rel_str.split('/').filter(|s| !s.is_empty()).collect();
                    if parts.is_empty() {
                        continue;
                    }

                    // Enforce security archive path validation
                    crate::registry::validate_archive_path_in_private_stage(
                        path_str,
                        &parts,
                        entry.header().entry_type(),
                        entry.link_name().ok().flatten().as_deref(),
                        &archive_symlinks,
                    )?;

                    let mut out_path = stage_path.clone();
                    for component in &parts {
                        out_path.push(component);
                    }
                    let is_symlink = entry.header().entry_type().is_symlink();
                    let symlink_relative = is_symlink.then(|| {
                        let mut relative = PathBuf::new();
                        for component in &parts {
                            if *component != "." {
                                relative.push(component);
                            }
                        }
                        relative
                    });
                    (
                        out_path,
                        entry.header().entry_type(),
                        is_symlink,
                        symlink_relative,
                    )
                };

                if entry_type.is_dir() {
                    if !created_dirs.contains(&out_path) {
                        std::fs::create_dir_all(&out_path)
                            .map_err(|e| format!("SSDP mkdir error: {}", e))?;
                        created_dirs.insert(out_path.clone());
                    }
                } else {
                    if is_symlink || entry_type.is_hard_link() {
                        entry
                            .unpack(&out_path)
                            .map_err(|e| format!("SSDP unpack link error: {}", e))?;
                        if let Some(relative) = symlink_relative {
                            archive_symlinks.insert(relative);
                        }
                        files_extracted += 1;
                        continue;
                    }
                    if let Some(p) = out_path.parent() {
                        if !created_dirs.contains(p) {
                            std::fs::create_dir_all(p)
                                .map_err(|e| format!("SSDP mkdir error: {}", e))?;
                            created_dirs.insert(p.to_path_buf());
                        }
                    }
                    entry
                        .unpack(&out_path)
                        .map_err(|e| format!("SSDP unpack file error: {}", e))?;
                    files_extracted += 1;
                }
            }
        }
        // Tar readers may stop at the end marker before gzip reaches its trailer.
        // Drain it so GzDecoder validates CRC/ISIZE before marking store complete.
        std::io::copy(&mut decoder, &mut std::io::sink())
            .map_err(|e| format!("SSDP gzip trailer error: {}", e))?;
        let bytes_decompressed = decoder.bytes_read;
        drop(decoder);
        // Include any trailing bytes in the content hash and integrity check.
        std::io::copy(&mut digester, &mut std::io::sink())
            .map_err(|e| format!("SSDP compressed stream read error: {}", e))?;
        let (hash, sri_digest) = digester.finish();
        if expected_sha256.is_some_and(|wanted| wanted != hash) {
            return Err(format!("SSDP: Content hash mismatch for {}", pkg_key));
        }
        if expected
            .as_ref()
            .is_some_and(|(_, wanted)| Some(wanted.as_slice()) != sri_digest.as_deref())
        {
            return Err(format!("SSDP: Integrity check failed for {}", pkg_key));
        }
        let shard = if hash.len() >= 2 { &hash[..2] } else { "xx" };
        let unpacked_dir = cache_dir.join("store_unpacked").join(shard).join(&hash);
        let parent = unpacked_dir
            .parent()
            .ok_or_else(|| "SSDP: unpacked path has no parent".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("SSDP: Failed to create unpacked parent: {}", e))?;

        // Publish only after extraction and gzip trailer validation both pass.
        std::fs::write(stage_path.join(STORE_COMPLETE_MARKER), "ok")
            .map_err(|e| format!("SSDP completion marker error: {}", e))?;
        match std::fs::rename(&stage_path, &unpacked_dir) {
            Ok(()) => stage_guard.disarm(),
            Err(_) if unpacked_dir.join(STORE_COMPLETE_MARKER).is_file() => {
                // Another installer won the same content-addressed race.
            }
            Err(e) => return Err(format!("SSDP publish error: {}", e)),
        }
        // Publish a copy into the machine's durable origin so a later install
        // with a different cache dir can harvest the unpacked tree (clonefile,
        // no re-download, no re-extract).
        crate::registry::mirror_unpacked_to_durable(&unpacked_dir);

        // Link failure is install failure; never report success with a missing package.
        if let Some(nm_dest) = target_nm_link {
            Self::link_or_copy_package(&unpacked_dir, nm_dest)?;
        }

        Ok(ProjectionResult {
            hash,
            unpacked_dir,
            files_extracted,
            bytes_decompressed,
        })
    }

    /// Link or copy package from store_unpacked into node_modules
    fn link_or_copy_package(src: &Path, dst: &Path) -> Result<(), String> {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("SSDP link mkdir error: {}", e))?;
        }
        match std::fs::symlink_metadata(dst) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                std::fs::remove_dir_all(dst)
                    .map_err(|e| format!("SSDP link replace error: {}", e))?;
            }
            Ok(_) => {
                std::fs::remove_file(dst).map_err(|e| format!("SSDP link replace error: {}", e))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("SSDP link inspect error: {}", e)),
        }

        // On macOS/Linux, create directory symlink or reflink for instant O(1) linkage
        #[cfg(unix)]
        {
            if std::os::unix::fs::symlink(src, dst).is_ok() {
                return Ok(());
            }
        }

        // Fallback: copy tree if symlinking fails
        copy_dir_all(src, dst).map_err(|e| format!("SSDP link fallback error: {}", e))
    }
}

struct DigestingReader<R> {
    inner: R,
    sha256: Sha256,
    sri: Option<SriHasher>,
}

enum SriHasher {
    Sha512(Sha512),
    Sha384(Sha384),
}

impl<R> DigestingReader<R> {
    fn new(inner: R, algorithm: Option<&String>) -> Self {
        let sri = match algorithm.map(String::as_str) {
            Some("sha512") => Some(SriHasher::Sha512(Sha512::new())),
            Some("sha384") => Some(SriHasher::Sha384(Sha384::new())),
            _ => None,
        };
        Self {
            inner,
            sha256: Sha256::new(),
            sri,
        }
    }

    fn finish(self) -> (String, Option<Vec<u8>>) {
        let hash = format!("{:x}", self.sha256.finalize());
        let sri = self.sri.map(|hasher| match hasher {
            SriHasher::Sha512(h) => h.finalize().to_vec(),
            SriHasher::Sha384(h) => h.finalize().to_vec(),
        });
        (hash, sri)
    }
}

impl<R: Read> Read for DigestingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.sha256.update(&buf[..n]);
            if let Some(hasher) = &mut self.sri {
                match hasher {
                    SriHasher::Sha512(h) => h.update(&buf[..n]),
                    SriHasher::Sha384(h) => h.update(&buf[..n]),
                }
            }
        }
        Ok(n)
    }
}

struct CountingReader<R> {
    inner: R,
    bytes_read: usize,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read = self.bytes_read.saturating_add(n);
        Ok(n)
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    crate::materialize::copy_tree_excluding_root_file(
        src,
        dst,
        std::ffi::OsStr::new(STORE_COMPLETE_MARKER),
    )
    .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn create_test_tarball() -> Vec<u8> {
        let mut tar_data = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_data);
            let mut header = tar::Header::new_gnu();
            let content = b"console.log('jhol ssdp direct projection');";
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "package/index.js", &content[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut gz_data = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut gz_data, Compression::default());
            encoder.write_all(&tar_data).unwrap();
            encoder.finish().unwrap();
        }
        gz_data
    }

    fn sri_sha512(data: &[u8]) -> String {
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(data))
        )
    }

    #[test]
    fn digesting_reader_matches_independent_padding_and_chunk_vectors() {
        // Generated independently with Python hashlib; covers SHA padding,
        // block boundaries and fragmented reads, including the ARM backend.
        let vectors = [
            (0, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b", "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"),
            (1, "084fed08b978af4d7d196a7446a86b58009e636b611db16211b65a9aadff29c5", "8a6c69af6fb6247635f837958446fb8f10e39bd5fbc244f7e635176339a3be614f6394247f01dbe1126c178c7bd48cb5", "e45bf5817ddf94aa2f7a407071f0eedc6beb98f768b4cd33d1176d44d1563a45a5d7212290eb7670c6786b13591aedac86478993895e8b24e612014abaa6ba04"),
            (55, "c40dbcd18dfdb34a5c52a791f09286986b5e9d61f867278c03adda227684e897", "0336f580f9cce82d401e98c82a27d5a39245adc8f6b9f18155060cee4021cc8d696a450e946e27a11d5e35484745d63c", "88fb454046c4edf5630e814867b4cfe5cd2ee158d0a5f35d633f9a2c3bdd6b0fc24c8f33c55ffea83d5f99517e72ed0e7c62212738a9f053e88e214428dd1aa9"),
            (64, "30a1b375b49e0f1d5e00b6868bbfd471cbe5387806456cc11fd153c689bfe747", "c784adb09c278cd2c8eba901572dccf1e7490bf80db96b4bdb2bd6cdba20cb821b89d702d09c1e52b4f033d529f3463f", "a772aa58f464c85e98fee59859ae3085dfeae7a85c081ce8fbc39ba245a1d4a30b6bc9a34dd2a8588466d3da57f9be59a2deb92e2fe9d1c109c9ded8086eea2f"),
            (127, "fc7d260501b641289fe7df7d943df65d93c782f2f3a1e2a4e69c6f763d88f8c5", "281583a9e6cc94a58877823651f92f1dfdb6ae81962d6dcd2356dfa8fb5c820961281dc296405ad5f339ff9bc3992c00", "6c06aae9306473040c3f3566ec91934ff387a72aa55a2967a9c2977506ac1ac91dd480aef85ce5de41e5917c1f907df14ebf91d144b9aea711097e1a15d7bc92"),
            (128, "5a878aab98a988c5479b48c00d8e2bef988c946dc648583ffe7ac598d76bacd5", "a2c9bbf7536e70abacb7cc2a3029057d5cee8b1927934aa738605617e674e874495ddb9f08bf01ecf7b8a977412f5ac9", "4d5618155f746968d0485e9b9093977b17e0e81520b1a9cfb2539fe22e2fd76b49508302e8ab9083b29261562556ddc8017a5c89ce53d02fa13eb95bded120d7"),
            (129, "a1253e220a0a60177ba22d6f1ae079f6b8f0b8fa598b0adc1390b0d7c81756fa", "328d8c04d21981ee7fa570fd7c13a5d23eb8f71640c30a6baa50dbf79b5bf3139ec127bece3c8b680f04c21ccc6a293d", "129437ffcead15e9c0cfddfc2fed5d5d16069deaff871c17239ee321da9e7d000ae1b94411c306bacda4dfa618108ef3494db2eed6068f14556a27dbcb547c4a"),
            (8193, "40aad4a3dcbc0f12bfe6231e34fff39eb14338284396b090b01e7eadf3b653ef", "b0927142f91e3add752cfe627047026fbf50f5825214d0e717f97d5c01bb99c23b04c5822a58b9837eaa7cde587e554a", "bcd3a52e5e46a78ff3a70d9be59a151b8fa4ad3da9d52ca974e87a38381d3305d42563c168d3c52008ce46c8460cc7f6e455f69fa1e306ba18f76d36889c454c"),
        ];
        for (length, sha256, sha384, sha512) in vectors {
            let bytes: Vec<u8> = (0..length).map(|i| ((i * 17 + 3) % 256) as u8).collect();
            for (algorithm, expected) in [("sha384", sha384), ("sha512", sha512)] {
                for size in [1, 7, 63, 64, 127, 128, 4096] {
                    let algorithm = algorithm.to_string();
                    let mut reader = DigestingReader::new(bytes.as_slice(), Some(&algorithm));
                    let mut buffer = vec![0; size];
                    while reader.read(&mut buffer).unwrap() != 0 {}
                    let (hash, sri) = reader.finish();
                    assert_eq!(hash, sha256, "length={length}, read={size}");
                    assert_eq!(
                        hex::encode(sri.unwrap()),
                        expected,
                        "length={length}, read={size}"
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "Set JHOL_PROFILE_ARCHIVE to a verified local tarball; diagnostic, not registry cold"]
    fn profile_cold_archive_costs() {
        use std::time::Instant;
        let path = std::env::var("JHOL_PROFILE_ARCHIVE").expect("JHOL_PROFILE_ARCHIVE required");
        assert!(std::fs::metadata(&path).unwrap().len() <= 16 * 1024 * 1024);
        let bytes = std::fs::read(path).unwrap();
        let expected_sri = sri_sha512(&bytes);
        let mut expected = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes.as_slice())
            .read_to_end(&mut expected)
            .unwrap();
        assert!(expected.len() <= 64 * 1024 * 1024);
        let mut hash_ms = Vec::new();
        let mut inflate_ms = Vec::new();
        let mut native_ms = Vec::new();
        let mut project_ms = Vec::new();
        for _ in 0..15 {
            let start = Instant::now();
            std::hint::black_box(Sha256::digest(&bytes));
            std::hint::black_box(Sha512::digest(&bytes));
            hash_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            let mut out = Vec::with_capacity(expected.len());
            flate2::read::MultiGzDecoder::new(bytes.as_slice())
                .read_to_end(&mut out)
                .unwrap();
            inflate_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(out, expected);
            let mut native = libdeflater::Decompressor::new();
            let mut out = vec![0u8; expected.len()];
            let start = Instant::now();
            let length = native.gzip_decompress(&bytes, &mut out).unwrap();
            native_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(&out[..length], expected);
        }
        for _ in 0..5 {
            let root = tempfile::tempdir().unwrap();
            let start = Instant::now();
            let result = StreamDirectProjector::project_reader(
                bytes.as_slice(),
                root.path(),
                "profile@1.0.0",
                Some(&expected_sri),
                None,
            )
            .unwrap();
            project_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(result.bytes_decompressed, expected.len());
            assert!(result.files_extracted > 0);
        }
        println!(
            "[ARCHIVE-PROFILE] {}",
            serde_json::json!({
                "compressed_bytes": bytes.len(), "expanded_bytes": expected.len(),
                "hash_ms": hash_ms, "flate2_inflate_ms": inflate_ms,
                "libdeflate_reference_ms": native_ms, "verified_project_ms": project_ms,
                "note": "Local fixed-byte diagnostic; reference handles this verified single-member input only"
            })
        );
    }

    #[test]
    fn test_ssdp_direct_projection() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let nm_dir = temp_dir.path().join("node_modules").join("my-test-pkg");

        let tarball_bytes = create_test_tarball();
        let mut expected_decompressed = Vec::new();
        GzDecoder::new(Cursor::new(&tarball_bytes))
            .read_to_end(&mut expected_decompressed)
            .unwrap();
        let result = StreamDirectProjector::project_stream(
            &tarball_bytes,
            &cache_dir,
            "my-test-pkg@1.0.0",
            None,
            Some(&nm_dir),
        )
        .unwrap();

        assert!(!result.hash.is_empty());
        assert_eq!(result.files_extracted, 1);
        assert_eq!(result.bytes_decompressed, expected_decompressed.len());
        assert!(result.unpacked_dir.join("index.js").exists());
        assert!(nm_dir.join("index.js").exists());
    }

    #[test]
    fn test_ssdp_rejects_bad_gzip_trailer_before_marking_complete() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut tarball_bytes = create_test_tarball();
        *tarball_bytes.last_mut().unwrap() ^= 0xff;

        let result = StreamDirectProjector::project_stream(
            &tarball_bytes,
            temp_dir.path(),
            "bad-gzip@1.0.0",
            None,
            None,
        );
        let error = result.unwrap_err();
        assert!(error.contains("SSDP gzip trailer error"), "{error}");

        let hash = format!("{:x}", Sha256::digest(&tarball_bytes));
        let unpacked = temp_dir
            .path()
            .join("store_unpacked")
            .join(&hash[..2])
            .join(hash);
        assert!(!unpacked.join(STORE_COMPLETE_MARKER).exists());
        assert!(
            !unpacked.exists(),
            "failed extraction must not publish partial files"
        );
    }

    #[test]
    fn test_ssdp_copy_fallback_preserves_hidden_package_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let src = temp_dir.path().join("source");
        let dst = temp_dir.path().join("copy");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("target.js"), b"target").unwrap();
        std::fs::write(src.join("nested/child.js"), b"child").unwrap();
        std::fs::write(src.join(".npmignore"), b"private package metadata").unwrap();
        std::fs::write(src.join(STORE_COMPLETE_MARKER), b"ok").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("target.js", src.join("alias.js")).unwrap();
            std::os::unix::fs::symlink("nested", src.join("nested-alias")).unwrap();
        }

        copy_dir_all(&src, &dst).unwrap();

        assert_eq!(
            std::fs::read(dst.join(".npmignore")).unwrap(),
            b"private package metadata"
        );
        assert!(!dst.join(STORE_COMPLETE_MARKER).exists());
        assert_eq!(std::fs::read(dst.join("target.js")).unwrap(), b"target");
        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::read_link(dst.join("alias.js")).unwrap(),
                std::path::PathBuf::from("target.js")
            );
            assert_eq!(
                std::fs::read_link(dst.join("nested-alias")).unwrap(),
                std::path::PathBuf::from("nested")
            );
            assert_eq!(
                std::fs::read(dst.join("nested-alias/child.js")).unwrap(),
                b"child"
            );
        }
    }

    #[test]
    fn test_ssdp_reader_hashes_and_verifies_stream_without_body_buffer() {
        let temp_dir = tempfile::tempdir().unwrap();
        let tarball = create_test_tarball();
        let expected_hash = format!("{:x}", Sha256::digest(&tarball));
        let result = StreamDirectProjector::project_reader(
            Cursor::new(&tarball),
            temp_dir.path(),
            "reader-package@1.0.0",
            Some(&sri_sha512(&tarball)),
            None,
        )
        .unwrap();

        assert_eq!(result.hash, expected_hash);
        assert_eq!(result.files_extracted, 1);
        assert!(result.unpacked_dir.join("index.js").is_file());
    }

    #[test]
    fn test_ssdp_reader_rejects_bad_sri_without_publishing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let tarball = create_test_tarball();
        let mut bad_sri = sri_sha512(&tarball);
        let last = bad_sri.pop().unwrap();
        bad_sri.push(if last == 'A' { 'B' } else { 'A' });

        let error = StreamDirectProjector::project_reader(
            Cursor::new(&tarball),
            temp_dir.path(),
            "bad-sri@1.0.0",
            Some(&bad_sri),
            None,
        )
        .unwrap_err();
        let hash = format!("{:x}", Sha256::digest(&tarball));
        let unpacked = temp_dir
            .path()
            .join("store_unpacked")
            .join(&hash[..2])
            .join(hash);

        assert!(error.contains("Integrity check failed"), "{error}");
        assert!(!unpacked.exists());
    }

    #[test]
    fn test_ssdp_memory_projection_rejects_bad_sri_on_cache_miss_and_hit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let tarball = create_test_tarball();
        let good_sri = sri_sha512(&tarball);
        let mut bad_sri = good_sri.clone();
        let last = bad_sri.pop().unwrap();
        bad_sri.push(if last == 'A' { 'B' } else { 'A' });
        let cold_cache = temp_dir.path().join("cold-cache");
        let cold_error = StreamDirectProjector::project_stream(
            &tarball,
            &cold_cache,
            "memory-sri@1.0.0",
            Some(&bad_sri),
            None,
        )
        .unwrap_err();
        assert!(
            cold_error.contains("Integrity check failed"),
            "{cold_error}"
        );

        let first = StreamDirectProjector::project_stream(
            &tarball,
            temp_dir.path(),
            "memory-sri@1.0.0",
            Some(&good_sri),
            None,
        )
        .unwrap();
        assert!(first.unpacked_dir.join(STORE_COMPLETE_MARKER).is_file());

        let error = StreamDirectProjector::project_stream(
            &tarball,
            temp_dir.path(),
            "memory-sri@1.0.0",
            Some(&bad_sri),
            None,
        )
        .unwrap_err();
        assert!(error.contains("Integrity check failed"), "{error}");
    }

    #[test]
    fn test_ssdp_rejects_archive_symlink_ancestors() {
        let mut tar_data = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_data);
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            link.set_link_name("target").unwrap();
            link.set_cksum();
            builder
                .append_link(&mut link, "package/./alias", "target")
                .unwrap();

            let mut file = tar::Header::new_gnu();
            file.set_size(4);
            file.set_mode(0o644);
            file.set_cksum();
            builder
                .append_data(&mut file, "package/alias/child", &b"data"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            encoder.write_all(&tar_data).unwrap();
            encoder.finish().unwrap();
        }

        let temp_dir = tempfile::tempdir().unwrap();
        let hash = format!("{:x}", Sha256::digest(&compressed));
        let result = StreamDirectProjector::project_reader(
            Cursor::new(&compressed),
            temp_dir.path(),
            "symlink-ancestor@1.0.0",
            None,
            None,
        );

        assert!(result
            .unwrap_err()
            .contains("Unsafe archive destination symlink"));
        assert!(!temp_dir
            .path()
            .join("store_unpacked")
            .join(&hash[..2])
            .join(hash)
            .exists());
    }

    #[test]
    fn test_ssdp_reports_link_failure_after_publishing_complete_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let blocking_file = temp_dir.path().join("not-a-directory");
        std::fs::write(&blocking_file, b"block parent creation").unwrap();
        let link_path = blocking_file.join("package");

        let error = StreamDirectProjector::project_stream(
            &create_test_tarball(),
            &cache_dir,
            "link-failure@1.0.0",
            None,
            Some(&link_path),
        )
        .unwrap_err();

        assert!(error.contains("SSDP link mkdir error"), "{error}");
        let hash = format!("{:x}", Sha256::digest(create_test_tarball()));
        let unpacked = cache_dir.join("store_unpacked").join(&hash[..2]).join(hash);
        assert!(unpacked.join(STORE_COMPLETE_MARKER).is_file());
    }

    #[test]
    #[ignore = "manual SSDP extraction microbenchmark"]
    fn benchmark_ssdp_many_files() {
        let mut tar_data = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_data);
            let content = b"export const value = 1;\n";
            for i in 0..2_000 {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(
                        &mut header,
                        format!("package/lib/file-{i:04}.js"),
                        &content[..],
                    )
                    .unwrap();
            }
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            encoder.write_all(&tar_data).unwrap();
            encoder.finish().unwrap();
        }

        let temp_dir = tempfile::tempdir().unwrap();
        for run in 0..5 {
            let cache_dir = temp_dir.path().join(format!("cache-{run}"));
            let start = std::time::Instant::now();
            let result = StreamDirectProjector::project_reader(
                Cursor::new(&compressed),
                &cache_dir,
                "ssdp-benchmark@1.0.0",
                None,
                None,
            )
            .unwrap();
            assert_eq!(result.files_extracted, 2_000);
            assert_eq!(
                std::fs::read(result.unpacked_dir.join("lib/file-1999.js")).unwrap(),
                b"export const value = 1;\n"
            );
            eprintln!("SSDP 2k files run {run}: {:?}", start.elapsed());
        }
    }

    #[test]
    #[ignore = "manual SSDP in-memory projection/integrity benchmark"]
    fn benchmark_ssdp_memory_stream_integrity() {
        let mut payload = vec![0u8; 8 * 1024 * 1024];
        let mut state = 0x9e37_79b9_u32;
        for byte in &mut payload {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        let mut tar_data = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_data);
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "package/large.bin", &payload[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut compressed, Compression::fast());
            encoder.write_all(&tar_data).unwrap();
            encoder.finish().unwrap();
        }
        let sri = sri_sha512(&compressed);
        let temp_dir = tempfile::tempdir().unwrap();
        for run in 0..5 {
            let run_baseline = |cache_dir: &Path| {
                assert!(crate::utils::verify_sri_bytes(&compressed, &sri));
                let _lookup_hash = Sha256::digest(&compressed);
                StreamDirectProjector::project_reader(
                    Cursor::new(&compressed),
                    cache_dir,
                    "ssdp-memory-benchmark@1.0.0",
                    Some(&sri),
                    None,
                )
                .unwrap()
            };
            let run_candidate = |cache_dir: &Path| {
                StreamDirectProjector::project_stream(
                    &compressed,
                    cache_dir,
                    "ssdp-memory-benchmark@1.0.0",
                    Some(&sri),
                    None,
                )
                .unwrap()
            };
            let (baseline_result, baseline_time, candidate_result, candidate_time) = if run % 2 == 0
            {
                let start = std::time::Instant::now();
                let baseline = run_baseline(&temp_dir.path().join(format!("base-{run}")));
                let baseline_time = start.elapsed();
                let start = std::time::Instant::now();
                let candidate = run_candidate(&temp_dir.path().join(format!("candidate-{run}")));
                (baseline, baseline_time, candidate, start.elapsed())
            } else {
                let start = std::time::Instant::now();
                let candidate = run_candidate(&temp_dir.path().join(format!("candidate-{run}")));
                let candidate_time = start.elapsed();
                let start = std::time::Instant::now();
                let baseline = run_baseline(&temp_dir.path().join(format!("base-{run}")));
                (baseline, start.elapsed(), candidate, candidate_time)
            };
            assert_eq!(baseline_result.files_extracted, 1);
            assert_eq!(candidate_result.files_extracted, 1);
            for result in [&baseline_result, &candidate_result] {
                assert_eq!(
                    std::fs::read(result.unpacked_dir.join("large.bin")).unwrap(),
                    payload
                );
            }
            eprintln!(
                "SSDP in-memory integrity 8 MiB pair {run}: baseline={:?}, candidate={:?}",
                baseline_time, candidate_time
            );
        }
    }
}
