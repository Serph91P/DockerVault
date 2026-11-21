//! Unprivileged native replacement for the accepted recovery_child.py exec guard.
//! Bind after exec, not unsafe pre_exec inside the threaded service runtime.
#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    use nix::{
        sys::{prctl, signal::Signal},
        unistd::{Pid, getppid},
    };
    use std::os::unix::process::CommandExt;
    let args: Vec<_> = std::env::args_os().collect();
    let parent = args
        .get(1)
        .and_then(|v| v.to_str())
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|v| *v > 0);
    if let (Some(parent), Some(program)) = (parent, args.get(2))
        && prctl::set_pdeathsig(Signal::SIGKILL).is_ok()
        && getppid() == Pid::from_raw(parent)
    {
        let _ = std::process::Command::new(program).args(&args[3..]).exec();
    }
    eprintln!("Unable to start owned process");
    std::process::ExitCode::FAILURE
}
#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("Owned process execution requires Linux");
    std::process::ExitCode::FAILURE
}
