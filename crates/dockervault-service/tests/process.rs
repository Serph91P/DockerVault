#![cfg(target_os = "linux")]
use dockervault_service::process::{ProcessError, ProcessOwner, ProcessSpec};
use nix::{errno::Errno, sys::signal::kill, unistd::Pid};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn fixed_argv_is_not_expanded_by_a_shell_and_inherited_secrets_are_absent() {
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let output = owner
        .start(ProcessSpec::new(
            "/usr/bin/printf",
            ["%s", "$(touch /forbidden);* literal"],
        ))
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"$(touch /forbidden);* literal");
    let output = owner
        .start(ProcessSpec::new("/usr/bin/env", std::iter::empty::<&str>()))
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    let env = String::from_utf8(output.stdout).unwrap();
    assert!(env.contains("PATH=/usr/local/bin:/usr/bin:/bin\n"));
    for forbidden in [
        "CARGO_HOME=",
        "DATABASE_URL=",
        "CREDENTIAL_ENCRYPTION_KEY=",
        "KOMODO_API_SECRET=",
    ] {
        assert!(!env.contains(forbidden));
    }
    owner.shutdown(None).await;
}

#[tokio::test]
async fn full_stdout_and_stderr_pipes_fail_boundedly_instead_of_deadlocking_or_truncating_success()
{
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let spec = ProcessSpec::new(
        "/bin/sh",
        [
            "-c",
            "head -c 262144 /dev/zero & head -c 262144 /dev/zero >&2 & wait",
        ],
    );
    let result = tokio::time::timeout(Duration::from_secs(3), owner.start(spec).unwrap().wait())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(ProcessError::OutputLimit)));
    owner.shutdown(None).await;
    assert_eq!(owner.active(), 0);
}

#[tokio::test]
async fn cancellation_escalates_and_reaps_the_term_resistant_grandchild_before_release() {
    let fixture = Fixture::new();
    let child_file = fixture.path("grandchild");
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let mut spec = ProcessSpec::new(
        "/bin/sh",
        [
            "-c",
            "trap '' TERM; /bin/sh -c 'trap \"\" TERM; echo $$ > \"$DV_CHILD_FILE\"; exec sleep 30' & wait",
        ],
    );
    spec.env
        .push(("DV_CHILD_FILE".into(), child_file.as_os_str().to_owned()));
    let work = owner.start(spec).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !child_file.exists() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = std::fs::read_to_string(&child_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(kill(Pid::from_raw(pid), None).is_ok());
    work.cancel();
    work.cancel();
    let result = tokio::time::timeout(Duration::from_secs(3), work.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(ProcessError::Cancelled)));
    assert_eq!(kill(Pid::from_raw(pid), None), Err(Errno::ESRCH));
    owner.shutdown(None).await;
    assert_eq!(owner.active(), 0);
}

#[tokio::test]
async fn timeout_joins_a_blocked_stdin_writer_and_all_output_readers() {
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let mut spec = ProcessSpec::new("/bin/sleep", ["30"]);
    spec.input = vec![42; 1024 * 1024];
    spec.timeout = Some(Duration::from_millis(30));
    let result = tokio::time::timeout(Duration::from_secs(3), owner.start(spec).unwrap().wait())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(ProcessError::TimedOut)));
    owner.shutdown(None).await;
    assert_eq!(owner.active(), 0);
}

#[tokio::test]
async fn cancellation_before_the_owned_spawn_poll_prevents_delayed_side_effects() {
    let fixture = Fixture::new();
    let marker = fixture.path("must-not-appear");
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    for _ in 0..16 {
        let mut spec = ProcessSpec::new(
            "/bin/sh",
            ["-c", "sleep .1; printf forbidden > \"$DV_MARKER\""],
        );
        spec.env
            .push(("DV_MARKER".into(), marker.as_os_str().to_owned()));
        let work = owner.start(spec).unwrap();
        work.cancel();
        assert!(matches!(
            work.wait().await.unwrap(),
            Err(ProcessError::Cancelled)
        ));
    }
    owner.shutdown(None).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn command_and_output_debug_do_not_disclose_arguments_environment_or_bytes() {
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let mut spec = ProcessSpec::new("/usr/bin/printf", ["%s", "private-test-output"]);
    spec.env
        .push(("PRIVATE".into(), "private-test-environment".into()));
    let diagnostic = format!("{spec:?}");
    assert!(!diagnostic.contains("private-test"));
    let output = owner.start(spec).unwrap().wait().await.unwrap().unwrap();
    assert!(!format!("{output:?}").contains("private-test-output"));
    owner.shutdown(None).await;
}

#[tokio::test]
async fn rejects_relative_executables_and_oversized_stdin_before_spawn() {
    assert!(matches!(
        ProcessOwner::new(1, "relative-guard"),
        Err(ProcessError::InvalidSpec)
    ));
    let owner = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    assert!(matches!(
        owner.start(ProcessSpec::new("sh", ["-c", "exit 0"])),
        Err(ProcessError::InvalidSpec)
    ));
    let mut spec = ProcessSpec::new("/bin/true", std::iter::empty::<&str>());
    spec.input = vec![0; 1024 * 1024 + 1];
    assert!(matches!(owner.start(spec), Err(ProcessError::InvalidSpec)));
    assert_eq!(owner.active(), 0);
    owner.shutdown(None).await;
}
