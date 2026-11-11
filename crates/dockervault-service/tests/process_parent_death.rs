//! Actual service-process SIGKILL must not leave its directly executed child alive.
use dockervault_service::process::{ProcessOwner, ProcessSpec};
use nix::{
    sys::{
        prctl,
        signal::{Signal, kill, killpg},
        wait::{WaitPidFlag, WaitStatus, waitpid},
    },
    unistd::Pid,
};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct OwnerChild(Child);
impl Drop for OwnerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct ExecutedChild(Option<Pid>);
impl Drop for ExecutedChild {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            let _ = killpg(pid, Signal::SIGKILL);
            let _ = kill(pid, Signal::SIGKILL);
            let _ = waitpid(pid, None);
        }
    }
}
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--owner") {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let owner =
                    ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
                let spec = ProcessSpec::new(
                    "/bin/sh",
                    [
                        std::ffi::OsString::from("-c"),
                        std::ffi::OsString::from("printf '%s' \"$$\" > \"$1\"; exec /bin/sleep 30"),
                        std::ffi::OsString::from("public-owner-death"),
                        args[2].clone(),
                    ],
                );
                let _ = owner.start(spec).unwrap().wait().await.unwrap();
            });
        return;
    }
    prctl::set_child_subreaper(true).unwrap();
    let fixture =
        Fixture(std::env::temp_dir().join(format!("dv-parent-death-{}", std::process::id())));
    fs::create_dir(&fixture.0).unwrap();
    let marker = fixture.0.join("child.pid");
    let mut owner = OwnerChild(
        Command::new(std::env::current_exe().unwrap())
            .arg("--owner")
            .arg(&marker)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(owner.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(value) = fs::read_to_string(&marker)
            && let Ok(pid) = value.parse::<i32>()
        {
            break Pid::from_raw(pid);
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut executed = ExecutedChild(Some(pid));
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut settled = false;
    while Instant::now() < deadline {
        match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Signaled(_, Signal::SIGKILL, _)) => {
                executed.0 = None;
                settled = true;
                break;
            }
            Ok(WaitStatus::StillAlive) => {}
            other => panic!("Unexpected owned child settlement: {other:?}"),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        settled,
        "executed child survived actual SIGKILL of its service owner"
    );
    println!(
        "process-owner-death: actual service SIGKILL -> kernel-bound executed child SIGKILL/reap: PASS"
    );
}
