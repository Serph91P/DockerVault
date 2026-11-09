//! Fixed-argv subprocesses with bounded pipes and process-group settlement.
use crate::ownership::{Owner, Work, WorkError};
use nix::{
    errno::Errno,
    sys::{
        signal::{Signal, killpg},
        wait::{WaitPidFlag, WaitStatus, waitpid},
    },
    unistd::Pid,
};
use std::{
    ffi::OsString,
    fmt, io,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 64 * 1024;
const INPUT_LIMIT: usize = 1024 * 1024;
const TERM_GRACE: Duration = Duration::from_millis(200);

pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub input: Vec<u8>,
    pub timeout: Option<Duration>,
    pub discard_output: bool,
    pub allow_relative_program: bool,
}

impl ProcessSpec {
    pub fn new<I, S>(program: impl Into<PathBuf>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
            input: Vec::new(),
            timeout: None,
            discard_output: false,
            allow_relative_program: false,
        }
    }
}

impl fmt::Debug for ProcessSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProcessSpec { command: [REDACTED] }")
    }
}

pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl fmt::Debug for ProcessOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessOutput")
            .field("status", &self.status)
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr_bytes", &self.stderr.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    Admission(WorkError),
    InvalidSpec,
    Cancelled,
    TimedOut,
    OutputLimit,
    UnsettledDescendants,
    Io(io::ErrorKind),
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Owned process error: {self:?}")
    }
}
impl std::error::Error for ProcessError {}

#[derive(Clone)]
pub struct ProcessOwner {
    owner: Owner,
}

type ProcessWork = Work<Result<ProcessOutput, ProcessError>>;

impl ProcessOwner {
    pub fn new(capacity: usize) -> Result<Self, ProcessError> {
        // Adopt and reap grandchildren rather than relying on a container PID 1.
        nix::sys::prctl::set_child_subreaper(true)
            .map_err(|error| ProcessError::Io(io::Error::from_raw_os_error(error as i32).kind()))?;
        Ok(Self {
            owner: Owner::new(capacity).map_err(ProcessError::Admission)?,
        })
    }

    pub fn start(&self, spec: ProcessSpec) -> Result<ProcessWork, ProcessError> {
        if (!spec.program.is_absolute() && !spec.allow_relative_program)
            || spec.input.len() > INPUT_LIMIT
        {
            return Err(ProcessError::InvalidSpec);
        }
        self.owner
            .submit_async(move |cancel| run(spec, cancel))
            .map_err(ProcessError::Admission)
    }

    pub fn active(&self) -> usize {
        self.owner.active()
    }

    pub async fn shutdown(&self, grace: Option<Duration>) {
        self.owner.shutdown(grace).await;
    }
}

async fn read_bounded(
    mut pipe: impl AsyncRead + Unpin,
    overflow: CancellationToken,
    discard: bool,
) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            return Ok(result);
        }
        if discard {
            continue;
        }
        let remaining = OUTPUT_LIMIT - result.len();
        result.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            overflow.cancel();
        }
        // Drain after the limit too, until the owner kills and reaps the group.
    }
}

fn signal(group: Pid, signal: Signal) {
    // ESRCH is a settled group. Other failures must not cause an early release;
    // the wait/reap path below still requires physical exit.
    let _ = killpg(group, signal);
}

fn group_alive(group: Pid) -> bool {
    loop {
        match waitpid(Pid::from_raw(-group.as_raw()), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) => break,
            Ok(_) | Err(Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
    !matches!(killpg(group, None), Err(Errno::ESRCH))
}

async fn settle_descendants(group: Pid) -> bool {
    if !group_alive(group) {
        return false;
    }
    signal(group, Signal::SIGTERM);
    let deadline = Instant::now() + TERM_GRACE;
    while group_alive(group) {
        if Instant::now() >= deadline {
            signal(group, Signal::SIGKILL);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    true
}

async fn terminate(child: &mut Child, group: Pid) -> io::Result<ExitStatus> {
    signal(group, Signal::SIGTERM);
    match tokio::time::timeout(TERM_GRACE, child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            signal(group, Signal::SIGKILL);
            child.wait().await
        }
    }
}

async fn joined<T>(task: JoinHandle<io::Result<T>>) -> Result<T, ProcessError> {
    task.await
        .map_err(|_| ProcessError::Io(io::ErrorKind::Other))?
        .map_err(|error| ProcessError::Io(error.kind()))
}

enum Event {
    Exited(io::Result<ExitStatus>),
    Failed(ProcessError),
}

async fn run(spec: ProcessSpec, cancel: CancellationToken) -> Result<ProcessOutput, ProcessError> {
    if cancel.is_cancelled() {
        return Err(ProcessError::Cancelled);
    }
    let deadline = spec
        .timeout
        .map(|duration| {
            Instant::now()
                .checked_add(duration)
                .ok_or(ProcessError::InvalidSpec)
        })
        .transpose()?;
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .envs(spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| ProcessError::Io(error.kind()))?;
    let Some(pid) = child.id() else {
        let _ = child.wait().await;
        return Err(ProcessError::Io(io::ErrorKind::Other));
    };
    // Linux pid_t values are positive signed integers.
    let group = Pid::from_raw(pid as i32);
    let overflow = CancellationToken::new();
    let stdout = child.stdout.take().expect("configured stdout pipe");
    let stderr = child.stderr.take().expect("configured stderr pipe");
    let mut stdin = child.stdin.take().expect("configured stdin pipe");
    let stdout = tokio::spawn(read_bounded(stdout, overflow.clone(), spec.discard_output));
    let stderr = tokio::spawn(read_bounded(stderr, overflow.clone(), spec.discard_output));
    let input = tokio::spawn(async move {
        stdin.write_all(&spec.input).await?;
        stdin.shutdown().await
    });
    let deadline = async {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending::<()>().await,
        }
    };
    let event = tokio::select! {
        biased;
        _ = overflow.cancelled() => Event::Failed(ProcessError::OutputLimit),
        _ = cancel.cancelled() => Event::Failed(ProcessError::Cancelled),
        _ = deadline => Event::Failed(ProcessError::TimedOut),
        status = child.wait() => Event::Exited(status),
    };
    let (status, mut failure) = match event {
        Event::Exited(Ok(status)) => (Ok(status), None),
        Event::Exited(Err(error)) => (
            terminate(&mut child, group).await,
            Some(ProcessError::Io(error.kind())),
        ),
        Event::Failed(error) => (terminate(&mut child, group).await, Some(error)),
    };
    if settle_descendants(group).await && failure.is_none() {
        failure = Some(ProcessError::UnsettledDescendants);
    }
    // No task, pipe, environment/input buffer or permit is abandoned on cancel.
    let input = joined(input).await;
    let stdout = joined(stdout).await;
    let stderr = joined(stderr).await;
    if overflow.is_cancelled() && failure.is_none() {
        failure = Some(ProcessError::OutputLimit);
    }
    if let Some(error) = failure {
        return Err(error);
    }
    input?;
    Ok(ProcessOutput {
        status: status.map_err(|error| ProcessError::Io(error.kind()))?,
        stdout: stdout?,
        stderr: stderr?,
    })
}
