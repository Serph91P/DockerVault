//! Real unprivileged ProcessOwner -> fixed sudo helper, in the isolated worker image.
use dockervault_service::process::{ProcessError, ProcessOwner, ProcessSpec};
use std::{fs, time::Duration};
const HELPER: &str = "/usr/local/libexec/dockervault-archive-worker";
fn spec(destination: &str, large: bool) -> ProcessSpec {
    let source = if large {
        "/fixture/large"
    } else {
        "/fixture/source"
    };
    let mut spec = ProcessSpec::new("/usr/bin/sudo", ["-n", HELPER]);
    spec.input = serde_json::to_vec(&serde_json::json!({"version":1,"dest":destination,"compress":large,"sources":[["root",source]],"allowed_sources":[source]})).unwrap();
    spec
}
fn assert_no_helper() {
    assert!(
        !fs::read_dir("/backups")
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".dockervault-archive-")),
        "owned private staging survives settlement"
    );
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        if let Ok(command) = fs::read(entry.path().join("cmdline")) {
            assert!(
                !command.starts_with(HELPER.as_bytes()),
                "owned helper survives settlement"
            );
        }
    }
}
#[tokio::main]
async fn main() {
    assert_eq!(nix::unistd::getuid().as_raw(), 1000);
    let owner = ProcessOwner::new(1).unwrap();
    // None is the published timeout=0 contract, not an arbitrary long deadline.
    let output = owner
        .start(spec("/backups/supervised.tar", false))
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(owner.active(), 0);
    assert_no_helper();
    println!("PASS: real fixed sudo worker via bounded stdin ProcessOwner with timeout=0");

    let mut timed = spec("/backups/timed.tar.gz", true);
    timed.timeout = Some(Duration::from_millis(30));
    assert!(matches!(
        owner.start(timed).unwrap().wait().await.unwrap(),
        Err(ProcessError::TimedOut)
    ));
    assert_eq!(owner.active(), 0);
    assert_no_helper();
    assert!(!std::path::Path::new("/backups/timed.tar.gz").exists());
    println!(
        "PASS: real nonzero timeout waits for sudo/root group settlement; no published partial archive"
    );

    // Observe an actual read of the protected input, not just process startup/CPU time.
    let access = nix::sys::inotify::Inotify::init(
        nix::sys::inotify::InitFlags::IN_NONBLOCK | nix::sys::inotify::InitFlags::IN_CLOEXEC,
    )
    .unwrap();
    access
        .add_watch(
            "/fixture/large",
            nix::sys::inotify::AddWatchFlags::IN_ACCESS,
        )
        .unwrap();
    let work = owner
        .start(spec("/backups/cancelled.tar.gz", true))
        .unwrap();
    let cancel = work.cancellation_token();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let copying = match access.read_events() {
            Ok(events) => events.iter().any(|event| {
                event
                    .mask
                    .contains(nix::sys::inotify::AddWatchFlags::IN_ACCESS)
                    && event.name.as_deref() == Some(std::ffi::OsStr::new("data"))
            }),
            Err(nix::errno::Errno::EAGAIN) => false,
            Err(error) => panic!("source access observation failed: {error}"),
        };
        if copying {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker did not enter actual archive compression"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancel.cancel();
    assert!(matches!(
        work.wait().await.unwrap(),
        Err(ProcessError::Cancelled)
    ));
    assert_eq!(owner.active(), 0);
    assert_no_helper();
    assert!(!std::path::Path::new("/backups/cancelled.tar.gz").exists());
    owner.shutdown(None).await;
    println!(
        "PASS: cancellation during real compression joins privileged root helper before owner release"
    );
}
