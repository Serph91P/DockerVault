use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dv-offline-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn command() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_dockervault-recover"));
    c.env_clear().env("PATH", "");
    c
}
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dockervault-core/tests/fixtures")
}
#[test]
fn documented_four_flag_invocation_recovers_v1_and_legacy_without_external_executables() {
    let root = Fixture::new();
    let output = root.0.join("recovered");
    let fixtures = fixtures();
    for suffix in ["", ".legacy"] {
        let result = command()
            .arg("--archive")
            .arg(fixtures.join(format!("pax-links.tar.gz{suffix}.enc")))
            .arg("--output")
            .arg(&output)
            .arg("--identity")
            .arg(fixtures.join("pax-links.tar.gz.identity.TEST_ONLY"))
            .arg("--key")
            .arg(fixtures.join(format!("pax-links.tar.gz{suffix}.key")))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read(&output).unwrap(),
            std::fs::read(fixtures.join("pax-links.tar.gz")).unwrap()
        );
    }
}
#[test]
fn offline_errors_preserve_destination_and_never_include_private_identity() {
    let root = Fixture::new();
    let output = root.0.join("recovered");
    std::fs::write(&output, b"PUBLIC sentinel").unwrap();
    let fixtures = fixtures();
    let result = command()
        .arg("--archive")
        .arg(fixtures.join("pax-links.tar.tampered.enc"))
        .arg("--output")
        .arg(&output)
        .arg("--identity")
        .arg(fixtures.join("pax-links.tar.identity.TEST_ONLY"))
        .arg("--key")
        .arg(fixtures.join("pax-links.tar.key"))
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
    assert!(!String::from_utf8_lossy(&result.stderr).contains("AGE-SECRET-KEY-"));
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 1);
}
#[test]
fn sigterm_during_actual_plaintext_staging_joins_before_exit_and_preserves_destination() {
    struct Process(std::process::Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = Fixture::new();
    let source = root.0.join("source");
    let cipher = root.0.join("input.enc");
    let output = root.0.join("recovered");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(67108864)
        .unwrap();
    std::fs::write(&output, b"PUBLIC sentinel").unwrap();
    let fields: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixtures().join("credential-vectors.TEST_ONLY.json")).unwrap(),
    )
    .unwrap();
    let mut material = b"DVKEY\0\x01\n".to_vec();
    material.extend(
        fields["dek_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()),
    );
    dockervault_core::crypto::encrypt_file(&source, &cipher, &material, &|| false).unwrap();
    let mut child = Process(
        command()
            .arg("--archive")
            .arg(&cipher)
            .arg("--output")
            .arg(&output)
            .arg("--identity")
            .arg(fixtures().join("pax-links.tar.identity.TEST_ONLY"))
            .arg("--key")
            .arg(fixtures().join("pax-links.tar.key"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let entered = std::fs::read_dir(&root.0)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".dockervault-crypto-")
                    && entry
                        .path()
                        .join("output")
                        .metadata()
                        .is_ok_and(|m| m.len() >= 1048576)
            });
        if entered {
            break;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "process finished before observing actual private plaintext"
        );
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.0.id().try_into().unwrap()),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "signal did not physically settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(status.code(), Some(143));
    assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 3);
}
#[test]
fn help_and_invalid_invocations_have_truthful_exit_status() {
    assert!(command().arg("--help").output().unwrap().status.success());
    assert_eq!(
        command().arg("--unknown").output().unwrap().status.code(),
        Some(2)
    );
    assert_eq!(command().output().unwrap().status.code(), Some(2));
}
