//! Python-compatible hook argv, explicit allowlisting, and physical process settlement.
use crate::{
    config::Config,
    process::{ProcessError, ProcessOwner, ProcessSpec},
};
use std::{fmt, os::unix::process::ExitStatusExt, time::Duration};
use tokio_util::sync::CancellationToken;

pub enum HookError {
    Syntax,
    Empty,
    Denied,
    Process(ProcessError),
    TimedOut(String),
    Exit(String, i32),
}
impl fmt::Debug for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HookError([REDACTED])")
    }
}
impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax => f.write_str("Invalid hook command syntax"),
            Self::Empty => f.write_str("Empty hook command"),
            Self::Denied => f.write_str("Hook command is not allowed"),
            Self::TimedOut(executable) => write!(f, "Hook '{executable}' timed out"),
            Self::Exit(executable, status) => {
                write!(f, "Hook '{executable}' exited with status {status}")
            }
            Self::Process(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for HookError {}

// POSIX libraries differ: Python shlex preserves #, CR delimiters, and escaped newlines.
pub fn parse_argv(command: &str) -> Result<Vec<String>, HookError> {
    let mut input = command.chars();
    let mut quote = None;
    let mut word = String::new();
    let mut started = false;
    let mut words = Vec::new();
    while let Some(character) = input.next() {
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None
                } else {
                    word.push(character)
                }
            }
            Some('"') => match character {
                '"' => quote = None,
                '\\' => {
                    let next = input.next().ok_or(HookError::Syntax)?;
                    if !['\\', '"'].contains(&next) {
                        word.push('\\');
                    }
                    word.push(next);
                }
                _ => word.push(character),
            },
            None => match character {
                ' ' | '\t' | '\r' | '\n' => {
                    if started {
                        words.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                '\'' | '"' => {
                    quote = Some(character);
                    started = true;
                }
                '\\' => {
                    word.push(input.next().ok_or(HookError::Syntax)?);
                    started = true;
                }
                _ => {
                    word.push(character);
                    started = true;
                }
            },
            Some(_) => unreachable!("only the two explicit quote states are constructed"),
        }
    }
    if quote.is_some() {
        return Err(HookError::Syntax);
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

pub struct Hooks {
    processes: ProcessOwner,
    allowed: String,
    timeout: Duration,
}
impl Hooks {
    pub fn new(
        config: &Config,
        parent_guard: impl Into<std::path::PathBuf>,
    ) -> Result<Self, HookError> {
        Ok(Self {
            processes: ProcessOwner::new(2, parent_guard).map_err(HookError::Process)?,
            allowed: config.allowed_hook_commands.clone(),
            timeout: Duration::from_secs(config.hook_command_timeout.max(0) as u64),
        })
    }
    pub fn active(&self) -> usize {
        self.processes.active()
    }
    pub async fn shutdown(&self, grace: Option<Duration>) {
        self.processes.shutdown(grace).await;
    }
    pub async fn run(&self, command: &str, cancel: CancellationToken) -> Result<(), HookError> {
        if cancel.is_cancelled() {
            return Err(HookError::Process(ProcessError::Cancelled));
        }
        let args = parse_argv(command)?;
        let executable = args.first().ok_or(HookError::Empty)?;
        if !self.allowed.split(',').any(|allowed| allowed == executable) {
            return Err(HookError::Denied);
        }
        let mut spec = ProcessSpec::new(executable, args.iter().skip(1));
        spec.allow_relative_program = true;
        spec.discard_output = true;
        spec.timeout = Some(self.timeout);
        spec.env = std::env::vars_os().collect();
        if !spec.env.iter().any(|(name, _)| name == "PATH") {
            spec.env.push(("PATH".into(), "/bin:/usr/bin".into()));
        }
        let work = self.processes.start(spec).map_err(HookError::Process)?;
        let stopping = work.cancellation_token();
        let waiting = work.wait();
        tokio::pin!(waiting);
        let output = tokio::select! {biased;
            _=cancel.cancelled()=> {stopping.cancel();waiting.await},
            output=&mut waiting=>output,
        }
        .map_err(|error| HookError::Process(ProcessError::Admission(error)))?;
        let output = match output {
            Err(ProcessError::TimedOut) => return Err(HookError::TimedOut(executable.clone())),
            other => other.map_err(HookError::Process)?,
        };
        if !output.status.success() {
            return Err(HookError::Exit(
                executable.clone(),
                output
                    .status
                    .code()
                    .unwrap_or_else(|| -output.status.signal().unwrap_or(1)),
            ));
        }
        Ok(())
    }
}
