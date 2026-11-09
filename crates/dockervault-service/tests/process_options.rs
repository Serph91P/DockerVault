use dockervault_service::process::{ProcessError, ProcessOwner, ProcessSpec};
#[tokio::test]
async fn explicitly_ignored_output_is_drained_with_zero_capture_and_successful_exit() {
    let owner = ProcessOwner::new(1).unwrap();
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
async fn authorized_path_search_is_explicit_and_external_cancel_handle_still_joins() {
    let owner = ProcessOwner::new(1).unwrap();
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
