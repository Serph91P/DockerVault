use dockervault_tools::archive_worker::MAX_REQUEST_BYTES;
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn invoke(input: &[u8], args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dockervault-archive-worker"))
        .args(args)
        .env_clear()
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    child.wait_with_output().unwrap()
}
#[test]
fn malformed_oversized_or_extra_argument_inputs_fail_without_echo_or_external_commands() {
    for (input, args) in [
        (b"not JSON".to_vec(), vec![]),
        (vec![b' '; MAX_REQUEST_BYTES + 1], vec![]),
        (b"{}".to_vec(), vec!["--shell"]),
    ] {
        let output = invoke(&input, &args);
        assert!(!output.status.success());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["status"], "error");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("not JSON"));
    }
}
