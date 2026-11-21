#![cfg(target_os = "linux")]
use std::process::Command;
const GUARD: &str = env!("CARGO_BIN_EXE_dockervault-process-guard");
#[test]
fn real_guard_exec_preserves_literal_arguments_and_unprivileged_identity() {
    let literal = "PUBLIC literal $HOME; $(printf should-not-run)";
    let output = Command::new(GUARD)
        .arg(std::process::id().to_string())
        .args(["/usr/bin/printf", "%s", literal])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, literal.as_bytes());
    assert!(output.stderr.is_empty());
    let uid = Command::new(GUARD)
        .arg(std::process::id().to_string())
        .args(["/usr/bin/id", "-u"])
        .output()
        .unwrap();
    assert!(uid.status.success());
    assert_eq!(
        String::from_utf8(uid.stdout).unwrap().trim(),
        nix::unistd::getuid().as_raw().to_string()
    );
}
#[test]
fn invalid_or_stale_parent_and_failed_exec_have_only_generic_errors_without_arguments() {
    for args in [
        vec![],
        vec!["0", "/usr/bin/printf", "PUBLIC-never-printed"],
        vec!["2147483647", "/usr/bin/printf", "PUBLIC-never-printed"],
        vec!["not-a-pid", "/usr/bin/printf", "PUBLIC-never-printed"],
    ] {
        let result = Command::new(GUARD).args(args).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert_eq!(result.stderr, b"Unable to start owned process\n");
    }
    let result = Command::new(GUARD)
        .arg(std::process::id().to_string())
        .args(["/missing/PUBLIC-not-an-executable", "PUBLIC-never-printed"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(result.stderr, b"Unable to start owned process\n");
}
