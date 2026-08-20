use std::collections::HashMap;
use std::sync::Mutex;

/// Why a command produced no usable stdout.
///
/// "The binary is not installed" and "the binary ran and refused" are
/// different facts and callers must be able to tell them apart: an absent
/// NordVPN CLI means fall back to a heuristic, whereas a CLI that exists and
/// failed (dead daemon, missing group permission) means the authoritative
/// source is unavailable and we must fail closed rather than guess.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    #[error("{program} is not installed: {message}")]
    NotFound { program: String, message: String },
    #[error("{program} failed: {message}")]
    Failed { program: String, message: String },
}

impl CommandError {
    pub fn is_absent(&self) -> bool {
        matches!(self, CommandError::NotFound { .. })
    }
    pub fn message(&self) -> &str {
        match self {
            CommandError::NotFound { message, .. } | CommandError::Failed { message, .. } => {
                message
            }
        }
    }
}

#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    /// Ok(stdout) on success; Err distinguishes an absent binary from one
    /// that ran and failed.
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError>;
}

pub struct SystemCommands;

#[async_trait::async_trait]
impl CommandRunner for SystemCommands {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| {
                let message = e.to_string();
                let program = program.to_string();
                if e.kind() == std::io::ErrorKind::NotFound {
                    CommandError::NotFound { program, message }
                } else {
                    // Spawning failed for another reason (permission denied,
                    // for instance). The binary may well exist, so this is
                    // not "absent" and must not license a fallback guess.
                    CommandError::Failed { program, message }
                }
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(CommandError::Failed {
                program: program.to_string(),
                message: if stderr.is_empty() {
                    format!("exited with {}", output.status)
                } else {
                    stderr
                },
            })
        }
    }
}

#[derive(Default)]
struct FakeInner {
    responses: HashMap<String, Result<String, CommandError>>,
    calls: Vec<(String, Vec<String>)>,
}

/// Scripted stand-in for process execution. Nothing here ever spawns a
/// process — running a real VPN command in a test would change the machine's
/// network configuration.
#[derive(Default)]
pub struct FakeCommands {
    inner: Mutex<FakeInner>,
}

impl FakeCommands {
    pub fn new() -> Self {
        Self::default()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, FakeInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn respond(&self, program: &str, stdout: &str) {
        self.lock()
            .responses
            .insert(program.into(), Ok(stdout.into()));
    }
    /// Script the binary as not installed.
    pub fn absent(&self, program: &str) {
        self.lock().responses.insert(
            program.into(),
            Err(CommandError::NotFound {
                program: program.into(),
                message: "no such file or directory".into(),
            }),
        );
    }

    /// Script the binary as present but failing (dead daemon, no permission).
    pub fn fail(&self, program: &str, message: &str) {
        self.lock().responses.insert(
            program.into(),
            Err(CommandError::Failed {
                program: program.into(),
                message: message.into(),
            }),
        );
    }
    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.lock().calls.clone()
    }
}

#[async_trait::async_trait]
impl CommandRunner for FakeCommands {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
        let mut inner = self.lock();
        inner.calls.push((
            program.to_string(),
            args.iter().map(|a| a.to_string()).collect(),
        ));
        inner.responses.get(program).cloned().unwrap_or_else(|| {
            Err(CommandError::NotFound {
                program: program.to_string(),
                message: "no such command".into(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_returns_scripted_stdout_and_records_the_call() {
        let f = FakeCommands::new();
        f.respond("nordvpn", "Status: Connected\n");
        assert_eq!(
            f.run("nordvpn", &["status"]).await.unwrap(),
            "Status: Connected\n"
        );
        assert_eq!(
            f.calls(),
            vec![("nordvpn".to_string(), vec!["status".to_string()])]
        );
    }

    #[tokio::test]
    async fn fake_reports_a_scripted_failure_as_present_but_failing() {
        let f = FakeCommands::new();
        f.fail("nordvpn", "daemon is not running");
        let err = f.run("nordvpn", &["status"]).await.unwrap_err();
        assert!(!err.is_absent(), "a failing binary is not an absent one");
        assert_eq!(err.message(), "daemon is not running");
    }

    #[tokio::test]
    async fn fake_reports_a_scripted_absence() {
        let f = FakeCommands::new();
        f.absent("nordvpn");
        let err = f.run("nordvpn", &["status"]).await.unwrap_err();
        assert!(err.is_absent());
    }

    #[tokio::test]
    async fn unscripted_program_is_reported_missing_not_executed() {
        let f = FakeCommands::new();
        let err = f
            .run("definitely-not-a-real-program", &[])
            .await
            .unwrap_err();
        assert!(err.is_absent(), "unscripted programs are treated as absent");
        assert_eq!(f.calls().len(), 1);
    }
}
