//! Job executor module - handles running commands

use crate::config::Job;
use crate::error::{Error, Result};
use std::process::Stdio;
use std::sync::RwLock;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{debug, instrument, warn};

/// Job executor responsible for running commands
pub struct JobExecutor {
    /// Default shell to use
    shell: RwLock<String>,
    /// Shell arguments
    shell_args: RwLock<Vec<String>>,
}

impl JobExecutor {
    /// Create a new job executor
    pub fn new(shell: String, shell_args: Vec<String>) -> Self {
        Self {
            shell: RwLock::new(shell),
            shell_args: RwLock::new(shell_args),
        }
    }

    /// Update shell settings
    pub fn update_shell(&self, shell: String, shell_args: Vec<String>) {
        *self.shell.write().unwrap() = shell;
        *self.shell_args.write().unwrap() = shell_args;
    }

    /// Execute a job
    #[instrument(skip(self, job), fields(job_name = %job_name))]
    pub async fn execute(&self, job_name: &str, job: &Job) -> Result<(i32, String, String)> {
        let (shell, shell_args) = self.get_shell(job);

        debug!(
            command = %job.command,
            shell = %shell,
            "Executing job"
        );

        // Build command
        let mut cmd = Command::new(&shell);

        for arg in &shell_args {
            cmd.arg(arg);
        }
        cmd.arg(&job.command);

        // Set working directory
        if let Some(ref dir) = job.working_dir {
            cmd.current_dir(dir);
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

        // Get handles to stdout/stderr
        let mut stdout_handle = child.stdout.take().expect("stdout was piped");
        let mut stderr_handle = child.stderr.take().expect("stderr was piped");

        // Read output with size limit - read both streams in parallel
        let max_output = job.max_output_size;

        let read_stdout = async move {
            let mut stdout = Vec::with_capacity(4096.min(max_output));
            let mut buf = vec![0u8; 8192];
            loop {
                match stdout_handle.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdout.len() < max_output {
                            let remaining = max_output - stdout.len();
                            stdout.extend_from_slice(&buf[..n.min(remaining)]);
                        }
                    }
                    Err(_) => break,
                }
            }
            String::from_utf8_lossy(&stdout).into_owned()
        };

        let read_stderr = async move {
            let mut stderr = Vec::with_capacity(1024.min(max_output));
            let mut buf = vec![0u8; 8192];
            loop {
                match stderr_handle.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if stderr.len() < max_output {
                            let remaining = max_output - stderr.len();
                            stderr.extend_from_slice(&buf[..n.min(remaining)]);
                        }
                    }
                    Err(_) => break,
                }
            }
            String::from_utf8_lossy(&stderr).into_owned()
        };

        // Read both streams concurrently
        let read_output = async { tokio::join!(read_stdout, read_stderr) };

        // Wait for completion with optional timeout
        let result = if job.has_timeout() {
            let duration = Duration::from_secs(job.timeout);
            match timeout(duration, async {
                let output = read_output.await;
                let status = child.wait().await;
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
                    Ok((exit_code, output.0, output.1))
                }
                Err(_) => {
                    // Timeout - process will be killed due to kill_on_drop
                    warn!(job = %job_name, timeout = %job.timeout, "Job timed out");
                    Err(Error::job_timeout(job_name, job.timeout))
                }
            }
        } else {
            let output = read_output.await;
            let status = child.wait().await.map_err(|e| Error::JobSpawn {
                job_name: job_name.to_string(),
                source: e,
            })?;
            let exit_code = status.code().unwrap_or(-1);
            Ok((exit_code, output.0, output.1))
        };

        result
    }

    /// Get shell and args for a job
    fn get_shell(&self, job: &Job) -> (String, Vec<String>) {
        if let Some(ref shell) = job.shell {
            // Job-specific shell
            if cfg!(windows) {
                (shell.clone(), vec!["/C".to_string()])
            } else {
                (shell.clone(), vec!["-c".to_string()])
            }
        } else {
            // Default shell
            (
                self.shell.read().unwrap().clone(),
                self.shell_args.read().unwrap().clone(),
            )
        }
    }
}

impl Default for JobExecutor {
    fn default() -> Self {
        if cfg!(windows) {
            Self::new("cmd".to_string(), vec!["/C".to_string()])
        } else {
            Self::new("/bin/sh".to_string(), vec!["-c".to_string()])
        }
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

        let cmd = if cfg!(windows) {
            "echo hello"
        } else {
            "echo hello"
        };

        let job = test_job(cmd);
        let result = executor.execute("test", &job).await;

        assert!(result.is_ok());
        let (exit_code, stdout, _) = result.unwrap();
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_failing_command() {
        let executor = JobExecutor::default();

        let cmd = if cfg!(windows) { "exit 1" } else { "exit 1" };

        let job = test_job(cmd);
        let result = executor.execute("test", &job).await;

        assert!(result.is_ok());
        let (exit_code, _, _) = result.unwrap();
        assert_ne!(exit_code, 0);
    }

    #[tokio::test]
    async fn test_execute_with_timeout() {
        let executor = JobExecutor::default();

        let cmd = if cfg!(windows) {
            "ping -n 10 127.0.0.1"
        } else {
            "sleep 10"
        };

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
    async fn test_execute_with_env() {
        let executor = JobExecutor::default();

        let cmd = if cfg!(windows) {
            "echo %TEST_VAR%"
        } else {
            "echo $TEST_VAR"
        };

        let mut job = test_job(cmd);
        job.environment
            .insert("TEST_VAR".to_string(), "test_value".to_string());

        let result = executor.execute("test", &job).await;
        assert!(result.is_ok());

        let (exit_code, stdout, _) = result.unwrap();
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("test_value"));
    }

    #[tokio::test]
    async fn test_output_capture() {
        let executor = JobExecutor::default();

        let cmd = if cfg!(windows) {
            "echo stdout_test && echo stderr_test 1>&2"
        } else {
            "echo stdout_test && echo stderr_test >&2"
        };

        let job = test_job(cmd);
        let result = executor.execute("test", &job).await;

        assert!(result.is_ok());
        let (_, stdout, stderr) = result.unwrap();
        assert!(stdout.contains("stdout_test"));
        assert!(stderr.contains("stderr_test"));
    }
}
