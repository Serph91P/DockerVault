use dockervault_service::process::{ProcessError, ProcessOwner, ProcessSpec};
#[tokio::test]
async fn explicit_working_directory_reaches_the_real_exec_and_relative_directory_is_rejected() {
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let root = std::env::temp_dir().join(format!("dv-process-cwd-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let mut spec = ProcessSpec::new("/bin/pwd", std::iter::empty::<&str>());
    spec.working_directory = Some(root.clone());
    let output = owner.start(spec).unwrap().wait().await.unwrap().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{}\n", root.display()).as_bytes());
    let mut invalid = ProcessSpec::new("/bin/pwd", std::iter::empty::<&str>());
    invalid.working_directory = Some("relative".into());
    assert!(matches!(
        owner.start(invalid),
        Err(ProcessError::InvalidSpec)
    ));
    owner.shutdown(None).await;
    std::fs::remove_dir(root).unwrap();
}

#[tokio::test]
async fn explicitly_ignored_output_is_drained_with_zero_capture_and_successful_exit() {
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let mut spec = ProcessSpec::new("/usr/bin/head", ["-c", "262144", "/dev/zero"]);
    spec.discard_output = true;
    let output = owner.start(spec).unwrap().wait().await.unwrap().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(owner.active(), 0);
    owner.shutdown(None).await;
}
#[tokio::test]
async fn explicit_owned_stdout_spool_keeps_full_bytes_without_growing_capture_memory() {
    use std::io::{Read, Seek};
    let path = std::env::temp_dir().join(format!("dv-process-spool-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let mut spec = ProcessSpec::new("/usr/bin/head", ["-c", "262144", "/dev/zero"]);
    spec.stdout_file = Some(file.try_clone().unwrap());
    let output = owner.start(spec).unwrap().wait().await.unwrap().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(file.metadata().unwrap().len(), 262144);
    file.rewind().unwrap();
    let mut data = Vec::new();
    file.read_to_end(&mut data).unwrap();
    assert!(data.iter().all(|&v| v == 0));
    assert_eq!(owner.active(), 0);
    owner.shutdown(None).await;
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn owned_stdout_sink_failure_cannot_be_reported_as_child_success() {
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let mut spec = ProcessSpec::new("/usr/bin/head", ["-c", "262144", "/dev/zero"]);
    spec.stdout_file = Some(
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap(),
    );
    let output = owner.start(spec).unwrap().wait().await.unwrap().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert_eq!(owner.active(), 0);
    owner.shutdown(None).await;
}

#[tokio::test]
async fn authorized_path_search_is_explicit_and_external_cancel_handle_still_joins() {
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let mut spec = ProcessSpec::new("sleep", ["30"]);
    spec.allow_relative_program = true;
    let work = owner.start(spec).unwrap();
    work.cancellation_token().cancel();
    assert!(matches!(
        work.wait().await.unwrap(),
        Err(ProcessError::Cancelled)
    ));
    assert_eq!(owner.active(), 0);
    owner.shutdown(None).await;
}
