//! Job executor module - handles running commands

use crate::config::Job;
use crate::error::{Error, Result};
use log::{debug, warn};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::RwLock;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio::time::{timeout, Duration};

/// Result of executing a job command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult {
    /// Exit code returned by the child process.
    pub exit_code: i32,
    /// Merged stdout/stderr captured in arrival order.
    pub output: String,
}

/// Job executor responsible for running commands
pub struct JobExecutor {
    /// Default shell to use
    shell: RwLock<String>,
    /// Shell arguments
    shell_args: RwLock<Vec<String>>,
    /// Optional working directory applied when a job does not override it
    working_dir: RwLock<Option<PathBuf>>,
}

impl JobExecutor {
    /// Create a new job executor
    pub fn new(shell: String, shell_args: Vec<String>, working_dir: Option<PathBuf>) -> Self {
        Self {
            shell: RwLock::new(shell),
            shell_args: RwLock::new(shell_args),
            working_dir: RwLock::new(working_dir),
        }
    }

    /// Update default execution settings.
    pub fn update_defaults(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_dir: Option<PathBuf>,
    ) {
        *self.shell.write().unwrap() = shell;
        *self.shell_args.write().unwrap() = shell_args;
        *self.working_dir.write().unwrap() = working_dir;
    }

    /// Execute a job
    pub async fn execute(&self, job_name: &str, job: &Job) -> Result<ExecutionResult> {
        self.execute_with_live_output(job_name, job, None).await
    }

    pub async fn execute_with_live_output(
        &self,
        job_name: &str,
        job: &Job,
        live_output: Option<watch::Sender<String>>,
    ) -> Result<ExecutionResult> {
        let (shell, shell_args) = self.get_shell(job);

        debug!(
            job_name = job_name,
            status = "executing job",
            command = &*job.command,
            shell = &*shell;
            ""
        );

        // Build command
        let mut cmd = Command::new(&shell);

        cmd.process_group(0);

        for arg in &shell_args {
            cmd.arg(arg);
        }
        cmd.arg(&job.command);

        // Set working directory
        if let Some(dir) = self.effective_working_dir(job) {
            cmd.current_dir(dir);
        }

        use nix::unistd::Uid;
        if Uid::current().is_root() && job.user.as_deref() != Some("root") {
            self.configure_unix_user(&mut cmd, job_name, job.user.as_deref().unwrap_or("nobody"))?;
        }

        // Set environment variables
        for (key, value) in &job.environment {
            cmd.env(key, value);
        }

        // Configure I/O
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Kill on drop for proper cleanup
        cmd.kill_on_drop(true);

        // Spawn the process
        let mut child = cmd.spawn().map_err(|e| Error::JobSpawn {
            job_name: job_name.to_string(),
            source: e,
        })?;
        // wait() clears the child ID before descendants necessarily close the pipes.
        let process_group = child.id();

        // Get handles to stdout/stderr
        let mut stdout_handle = child.stdout.take().expect("stdout was piped");
        let mut stderr_handle = child.stderr.take().expect("stderr was piped");

        // Read output with size limit - merge stdout/stderr in arrival order
        let max_output = job.max_output_size;
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(32);
        let stdout_task = tokio::spawn({
            let tx = tx.clone();
            async move {
                let mut buf = vec![0u8; 8192];
                loop {
                    match stdout_handle.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                match stderr_handle.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let read_output = async move {
            let mut output = Vec::with_capacity(4096.min(max_output));
            let mut live_tail = Vec::with_capacity(8192);
            while let Some(chunk) = rx.recv().await {
                if output.len() < max_output {
                    let remaining = max_output - output.len();
                    output.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                }
                if let Some(sender) = &live_output {
                    live_tail.extend_from_slice(&chunk);
                    if live_tail.len() > 8192 {
                        live_tail.drain(..live_tail.len() - 8192);
                    }
                    let _ = sender.send(String::from_utf8_lossy(&live_tail).into_owned());
                }
            }

            let _ = tokio::join!(stdout_task, stderr_task);
            String::from_utf8_lossy(&output).into_owned()
        };

        // Wait for completion with optional timeout
        if job.has_timeout() {
            let duration = Duration::from_secs(job.timeout);
            match timeout(duration, async {
                let (status, output) = tokio::join!(child.wait(), read_output);
                (status, output)
            })
            .await
            {
                Ok((status, output)) => {
                    let exit_code = status
                        .map_err(|e| Error::JobSpawn {
                            job_name: job_name.to_string(),
                            source: e,
                        })?
                        .code()
                        .unwrap_or(-1);
                    Ok(ExecutionResult { exit_code, output })
                }
                Err(_) => {
                    if let Some(pid) = process_group {
                        use nix::sys::signal::{killpg, Signal};
                        use nix::unistd::Pid;

                        if let Err(error) = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL) {
                            warn!(job_name = job_name, status = "failed to kill job process group", error = &*error.to_string(); "");
                        }
                    }
                    if let Err(error) = child.wait().await {
                        warn!(job_name = job_name, status = "failed to reap timed out job", error = &*error.to_string(); "");
                    }
                    warn!(job_name = job_name, status = "job timed out", timeout = job.timeout; "");
                    Err(Error::job_timeout(job_name, job.timeout))
                }
            }
        } else {
            let (status, output) = tokio::join!(child.wait(), read_output);
            let status = status.map_err(|e| Error::JobSpawn {
                job_name: job_name.to_string(),
                source: e,
            })?;
            let exit_code = status.code().unwrap_or(-1);
            Ok(ExecutionResult { exit_code, output })
        }
    }

    /// Get shell and args for a job
    fn get_shell(&self, job: &Job) -> (String, Vec<String>) {
        if let Some(shell) = &job.shell {
            // Job-specific shell
            (shell.clone(), vec!["-c".to_string()])
        } else {
            // Default shell
            (
                self.shell.read().unwrap().clone(),
                self.shell_args.read().unwrap().clone(),
            )
        }
    }

    fn effective_working_dir(&self, job: &Job) -> Option<PathBuf> {
        job.working_dir
            .clone()
            .or_else(|| self.working_dir.read().unwrap().clone())
    }

    fn configure_unix_user(
        &self,
        cmd: &mut Command,
        job_name: &str,
        user_name: &str,
    ) -> Result<()> {
        use nix::unistd::User;
        use std::io::{Error as IoError, ErrorKind};
        use std::os::unix::process::CommandExt;

        let user = User::from_name(user_name)
            .map_err(|source| Error::JobSpawn {
                job_name: job_name.to_string(),
                source: IoError::other(source.to_string()),
            })?
            .ok_or_else(|| Error::JobSpawn {
                job_name: job_name.to_string(),
                source: IoError::new(
                    ErrorKind::NotFound,
                    format!("Unix user '{}' not found", user_name),
                ),
            })?;

        let std_cmd = cmd.as_std_mut();
        std_cmd.gid(user.gid.as_raw());
        std_cmd.uid(user.uid.as_raw());

        Ok(())
    }
}

impl Default for JobExecutor {
    fn default() -> Self {
        Self::new("/bin/sh".to_string(), vec!["-c".to_string()], None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_job(command: &str) -> Job {
        Job {
            command: command.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_execute_simple_command() {
        let executor = JobExecutor::default();

        let cmd = "echo hello";

        let job = test_job(cmd);
        let result = executor.execute("test", &job).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_failing_command() {
        let executor = JobExecutor::default();

        let cmd = "exit 1";

        let job = test_job(cmd);
        let result = executor.execute("test", &job).await;

        assert!(result.is_ok());
        assert_ne!(result.unwrap().exit_code, 0);
    }

    #[tokio::test]
    async fn test_execute_with_timeout() {
        let executor = JobExecutor::default();

        let cmd = "sleep 10";

        let job = Job {
            command: cmd.to_string(),
            timeout: 1, // 1 second timeout
            ..Default::default()
        };

        let result = executor.execute("test", &job).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::JobTimeout { .. }));
    }

    #[tokio::test]
    async fn test_timeout_kills_process_group_descendants() {
        for shell_action in ["wait", "exit 0"] {
            let temp_dir = tempfile::tempdir_in("/tmp").unwrap();
            let marker = temp_dir.path().join("descendant-survived");
            let job = Job {
                command: format!("(sleep 2; touch '{}') & {shell_action}", marker.display()),
                timeout: 1,
                ..Default::default()
            };

            let result = JobExecutor::default().execute("timeout-child", &job).await;
            assert!(matches!(result, Err(Error::JobTimeout { .. })));
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert!(
                !marker.exists(),
                "timed out descendant continued running after shell action: {shell_action}"
            );
        }
    }

    #[tokio::test]
    async fn test_execute_with_env() {
        let executor = JobExecutor::default();

        let cmd = "echo $TEST_VAR";

        let mut job = test_job(cmd);
        job.environment
            .insert("TEST_VAR".to_string(), "test_value".to_string());

        let result = executor.execute("test", &job).await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("test_value"));
    }

    #[tokio::test]
    async fn test_output_capture() {
        let executor = JobExecutor::default();

        let cmd = "echo stdout_test && echo stderr_test >&2";

        let job = test_job(cmd);
        let result = executor.execute("test", &job).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.output.contains("stdout_test"));
        assert!(result.output.contains("stderr_test"));
    }

    #[tokio::test]
    async fn live_output_is_available_before_execution_finishes() {
        let executor = JobExecutor::default();
        let job = test_job("printf live_marker; sleep 1");
        let (sender, mut receiver) = watch::channel(String::new());
        let execution = tokio::spawn(async move {
            executor
                .execute_with_live_output("live-output", &job, Some(sender))
                .await
        });

        tokio::time::timeout(Duration::from_millis(500), receiver.changed())
            .await
            .expect("live output should arrive before the process exits")
            .unwrap();
        assert!(receiver.borrow().contains("live_marker"));
        assert!(!execution.is_finished());
        execution.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_output_preserves_interleaving_order() {
        let executor = JobExecutor::default();
        let job = test_job(
            "printf 'stdout_first\\n'; sleep 0.05; printf 'stderr_first\\n' >&2; sleep 0.05; printf 'stdout_second\\n'; sleep 0.05; printf 'stderr_second\\n' >&2",
        );

        let result = executor.execute("test", &job).await.unwrap();

        let stdout_first = result.output.find("stdout_first").unwrap();
        let stderr_first = result.output.find("stderr_first").unwrap();
        let stdout_second = result.output.find("stdout_second").unwrap();
        let stderr_second = result.output.find("stderr_second").unwrap();

        assert!(stdout_first < stderr_first);
        assert!(stderr_first < stdout_second);
        assert!(stdout_second < stderr_second);
    }

    #[tokio::test]
    async fn test_global_working_directory_is_used_when_job_does_not_override_it() {
        let temp_dir = tempfile::tempdir_in("/tmp").unwrap();
        let (shell, shell_args, command) = ("/bin/sh".to_string(), vec!["-c".to_string()], "pwd");
        let executor = JobExecutor::new(shell, shell_args, Some(temp_dir.path().to_path_buf()));

        let job = test_job(command);
        let result = executor.execute("test", &job).await.unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result
            .output
            .trim()
            .contains(&temp_dir.path().display().to_string()));
    }

    #[tokio::test]
    async fn test_root_defaults_to_nobody_and_non_root_ignores_user() {
        use nix::unistd::{Uid, User};

        let current_user = User::from_uid(Uid::current()).unwrap().unwrap();
        let executor = JobExecutor::default();
        let job = test_job("id -un");

        let result = executor.execute("test", &job).await.unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.output.trim(),
            if Uid::current().is_root() {
                "nobody"
            } else {
                current_user.name.as_str()
            }
        );

        if !Uid::current().is_root() {
            let mut job = test_job("id -un");
            job.user = Some("nobody".to_string());
            let result = executor.execute("test", &job).await.unwrap();
            assert_eq!(result.exit_code, 0);
            assert_eq!(result.output.trim(), current_user.name);
        }
    }
}
