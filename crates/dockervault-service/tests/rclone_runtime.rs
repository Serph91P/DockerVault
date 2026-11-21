use dockervault_core::rclone_package::RcloneError;
use dockervault_service::{ownership::Owner, process::ProcessOwner, rclone::provider_options};
use std::{io::Write, os::unix::fs::OpenOptionsExt};
use tokio_util::sync::CancellationToken;
#[tokio::test]
async fn cli_faults_are_generic_and_cancel_waits_for_the_actual_child_reap() {
    let processes = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let blocking = Owner::new(1).unwrap();
    for binary in ["/bin/false", "/bin/echo", "/PUBLIC-unavailable-rclone"] {
        assert_eq!(
            provider_options(
                &processes,
                &blocking,
                std::path::Path::new(binary),
                CancellationToken::new()
            )
            .await,
            Err(RcloneError::Invalid)
        );
        assert_eq!(processes.active(), 0);
    }
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        provider_options(
            &processes,
            &blocking,
            std::path::Path::new("/PUBLIC-unavailable-rclone"),
            cancelled
        )
        .await,
        Err(RcloneError::Cancelled)
    );
    let root = std::env::temp_dir().join(format!("dv-rclone-runtime-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let binary = root.join("cli");
    let mut script = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&binary)
        .unwrap();
    script
        .write_all(b"#!/bin/sh\nprintf '[]'\nexit 3\n")
        .unwrap();
    drop(script);
    assert_eq!(
        provider_options(&processes, &blocking, &binary, CancellationToken::new()).await,
        Err(RcloneError::Invalid)
    );
    std::fs::write(&binary,b"#!/bin/sh\ntrap '' TERM\nprintf '%s' \"$$\" > \"$(dirname \"$0\")/pid\"\nexec /bin/sleep 60\n").unwrap();
    let cancel = CancellationToken::new();
    let actor = {
        let processes = processes.clone();
        let blocking = blocking.clone();
        let binary = binary.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { provider_options(&processes, &blocking, &binary, cancel).await })
    };
    let pid = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(raw) = std::fs::read_to_string(root.join("pid"))
                && let Ok(pid) = raw.parse::<i32>()
            {
                break pid;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(processes.active(), 1);
    cancel.cancel();
    assert_eq!(actor.await.unwrap(), Err(RcloneError::Cancelled));
    assert_eq!(processes.active(), 0);
    assert_eq!(blocking.active(), 0);
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
    processes.shutdown(None).await;
    blocking.shutdown(None).await;
    std::fs::remove_dir_all(root).unwrap();
}
