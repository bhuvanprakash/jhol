#![cfg(unix)]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ChildOnDrop(Child);

impl Drop for ChildOnDrop {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn daemon_stop_waits_for_accepted_connections_to_drain() {
    let temp = tempfile::tempdir().expect("temporary daemon directory");
    let socket = temp.path().join("daemon.sock");
    let socket_text = socket.to_str().expect("UTF-8 test socket path");
    let stderr_path = temp.path().join("daemon.stderr");
    let executable = env!("CARGO_BIN_EXE_jhol");
    let mut daemon = ChildOnDrop(
        Command::new(executable)
            .args(["daemon", "--serve"])
            .env("JHOL_DAEMON_SOCKET", socket_text)
            .env("JHOL_DAEMON_READ_TIMEOUT_SECS", "1")
            .env("JHOL_OFFLINE", "1")
            .env("JHOL_NO_SPLASH", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create(&stderr_path).expect("create daemon stderr log"),
            ))
            .spawn()
            .expect("start daemon process"),
    );

    let ready_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = daemon.0.try_wait().expect("check daemon startup") {
            let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
            panic!("daemon exited before ready: {status}; stderr:\n{stderr}");
        }
        match UnixStream::connect(&socket) {
            Ok(mut stream) => {
                let invalid = vec![0; jhol_core::daemon_cache::DAEMON_REQUEST_SIZE];
                stream.write_all(&invalid).expect("send readiness probe");
                let _ = jhol_core::daemon_cache::read_response(&mut stream);
                break;
            }
            Err(_) if Instant::now() < ready_deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                let status = daemon.0.try_wait().expect("check daemon after timeout");
                let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                #[cfg(target_os = "macos")]
                let sample = Command::new("sample")
                    .args([&daemon.0.id().to_string(), "1", "1"])
                    .output()
                    .ok()
                    .map(|output| {
                        String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .take(40)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                #[cfg(not(target_os = "macos"))]
                let sample = String::new();
                panic!(
                    "daemon did not become ready: {error}; status: {status:?}; stderr:\n{stderr}; sample:\n{sample}"
                );
            }
        }
    }

    let cwd = std::env::current_dir().expect("current directory");
    let request = jhol_core::daemon_cache::DaemonRequest::try_new(
        cwd.to_str().expect("UTF-8 current directory"),
        &[],
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
    .expect("valid partial request");
    let request_bytes = unsafe {
        std::slice::from_raw_parts(
            &request as *const jhol_core::daemon_cache::DaemonRequest as *const u8,
            jhol_core::daemon_cache::DAEMON_REQUEST_SIZE,
        )
    };
    let mut stalled = UnixStream::connect(&socket).expect("connect stalled client");
    stalled
        .write_all(&request_bytes[..request_bytes.len() / 2])
        .expect("send incomplete request");
    std::thread::sleep(Duration::from_millis(100));

    let started = Instant::now();
    let stop = Command::new(executable)
        .args(["daemon", "--stop"])
        .env("JHOL_DAEMON_SOCKET", socket_text)
        .env("JHOL_DAEMON_TIMEOUT_SECS", "10")
        .env("JHOL_OFFLINE", "1")
        .env("JHOL_NO_SPLASH", "1")
        .output()
        .expect("run daemon stop command");
    assert!(
        stop.status.success(),
        "daemon stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "daemon returned before the accepted connection's read timeout"
    );

    let status = daemon.0.wait().expect("wait for daemon exit");
    assert!(status.success(), "daemon exited unsuccessfully: {status}");
    assert!(
        std::fs::symlink_metadata(&socket).is_err(),
        "daemon socket remains after graceful stop"
    );
}
