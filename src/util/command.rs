use std::collections::HashMap;
use std::sync::Mutex;

#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    /// Ok(stdout) on success; Err(message) when the command fails or is absent.
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
}

pub struct SystemCommands;

#[async_trait::async_trait]
impl CommandRunner for SystemCommands {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }
}

#[derive(Default)]
struct FakeInner {
    responses: HashMap<String, Result<String, String>>,
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
    pub fn fail(&self, program: &str, message: &str) {
        self.lock()
            .responses
            .insert(program.into(), Err(message.into()));
    }
    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.lock().calls.clone()
    }
}

#[async_trait::async_trait]
impl CommandRunner for FakeCommands {
    async fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let mut inner = self.lock();
        inner.calls.push((
            program.to_string(),
            args.iter().map(|a| a.to_string()).collect(),
        ));
        inner
            .responses
            .get(program)
            .cloned()
            .unwrap_or_else(|| Err(format!("no such command: {program}")))
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
    async fn fake_reports_a_scripted_failure() {
        let f = FakeCommands::new();
        f.fail("nordvpn", "command not found");
        assert_eq!(
            f.run("nordvpn", &["status"]).await.unwrap_err(),
            "command not found"
        );
    }

    #[tokio::test]
    async fn unscripted_program_is_reported_missing_not_executed() {
        let f = FakeCommands::new();
        assert!(f.run("definitely-not-a-real-program", &[]).await.is_err());
        assert_eq!(f.calls().len(), 1);
    }
}
