//! Run only in docker/archive-worker-test.Dockerfile as UID1000.
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const HELPER: &str = "/usr/local/libexec/dockervault-archive-worker";
fn main() {
    assert_eq!(nix::unistd::getuid().as_raw(), 1000);
    let metadata = fs::metadata(HELPER).unwrap();
    assert_eq!(metadata.uid(), 0);
    assert_eq!(metadata.mode() & 0o022, 0);
    assert!(fs::read("/fixture/source/data").is_err());
    let mut child = Command::new("/usr/bin/sudo")
        .args(["-n", HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(br#"{"version":1,"dest":"/backups/sudo.tar","compress":false,"sources":[["root","/fixture/source"]],"allowed_sources":["/fixture/source"]}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "ok");
    assert_eq!(value["skipped_files"], serde_json::json!([]));
    let metadata = fs::metadata("/backups/sudo.tar").unwrap();
    assert_eq!(metadata.uid(), 1000);
    assert_eq!(metadata.gid(), 1000);
    assert_eq!(metadata.mode() & 0o777, 0o600);
    let mut archive = tar::Archive::new(fs::File::open("/backups/sudo.tar").unwrap());
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == std::path::Path::new("root/data") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"PUBLIC root-readable fixture\n");
            found = true;
        }
    }
    assert!(found);
    assert!(
        !Command::new("/usr/bin/sudo")
            .args(["-n", HELPER, "--shell"])
            .stdin(Stdio::null())
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/true"])
            .stdin(Stdio::null())
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/sh", "-c", "true"])
            .stdin(Stdio::null())
            .output()
            .unwrap()
            .status
            .success()
    );
    println!(
        "PASS: fixed root-owned executable/policy, privileged bytes, returned0600 UID/GID, no extra args/interpreter"
    );
    // Hold real stdin read, then check the unprivileged owner can kill the root helper.
    let mut held = Command::new("/usr/bin/sudo")
        .args(["-n", HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let pid = loop {
        let found = fs::read_dir("/proc")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let pid = entry.file_name().to_string_lossy().parse::<i32>().ok()?;
                let command = fs::read(entry.path().join("cmdline")).ok()?;
                let status = fs::read_to_string(entry.path().join("status")).ok()?;
                (command.starts_with(HELPER.as_bytes())
                    && status.lines().any(|line| line.starts_with("State:\tS")))
                .then_some(pid)
            })
            .next();
        if let Some(pid) = found {
            break pid;
        }
        assert!(Instant::now() < deadline, "root helper did not start");
        std::thread::sleep(Duration::from_millis(5));
    };
    let signal = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
    drop(held.stdin.take());
    let _ = held.wait_with_output().unwrap();
    assert!(
        signal.is_ok(),
        "unprivileged owner cannot physically kill owned root helper: {signal:?}"
    );
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    println!(
        "PASS: real held-stdin root helper SIGKILL and physical reap under unprivileged ownership"
    );
}
