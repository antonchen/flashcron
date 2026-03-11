//! Job configuration structures

use crate::error::{Error, Result};
use cron::Schedule;
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;

/// Job definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Cron schedule expression (standard 5-field format)
    pub schedule: String,

    /// Command to execute
    pub command: String,

    /// Optional description
    #[serde(default)]
    pub description: String,

    /// Whether the job is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Working directory for the command
    #[serde(default)]
    pub working_dir: Option<PathBuf>,

    /// Environment variables
    #[serde(default)]
    pub environment: HashMap<String, String>,

    /// Timeout in seconds (0 = no timeout)
    #[serde(default)]
    pub timeout: u64,

    /// Shell to use (overrides global setting)
    #[serde(default)]
    pub shell: Option<String>,

    /// Number of retry attempts on failure
    #[serde(default)]
    pub retry_count: u32,

    /// Delay between retries in seconds
    #[serde(default = "default_retry_delay")]
    pub retry_delay: u64,

    /// User to run the job as (Unix only)
    #[serde(default)]
    pub user: Option<String>,

    /// Whether to capture and log stdout/stderr
    #[serde(default = "default_capture_output")]
    pub capture_output: bool,

    /// Maximum output size to capture in bytes
    #[serde(default = "default_max_output")]
    pub max_output_size: usize,

    /// Run on startup (in addition to schedule)
    #[serde(default)]
    pub run_on_startup: bool,

    /// Tags for filtering/grouping
    #[serde(default)]
    pub tags: Vec<String>,

    /// Whether to print output (overrides global setting)
    #[serde(default)]
    pub print_output: Option<bool>,
}

fn default_enabled() -> bool {
    true
}

fn default_retry_delay() -> u64 {
    60
}

fn default_capture_output() -> bool {
    true
}

fn default_max_output() -> usize {
    1024 * 1024 // 1MB
}

impl Job {
    /// Validate the job configuration
    pub fn validate(&self, name: &str) -> Result<()> {
        // Validate cron expression
        self.parse_schedule().map_err(|e| Error::CronParse {
            expr: self.schedule.clone(),
            reason: e,
        })?;

        // Validate command is not empty
        if self.command.trim().is_empty() {
            return Err(Error::config(format!("Job '{}' has empty command", name)));
        }

        // Validate working directory exists if specified
        if let Some(ref dir) = self.working_dir {
            if !dir.exists() {
                warn!(
                    job_name = &*name,
                    working_dir = &*dir.display().to_string();
                    "Working directory does not exist"
                );
            }
        }

        Ok(())
    }

    /// Parse the cron schedule
    pub fn parse_schedule(&self) -> std::result::Result<Schedule, String> {
        let parts: Vec<&str> = self.schedule.split_whitespace().collect();

        let mut expr = if parts.len() == 5 {
            format!("0 {}", self.schedule)
        } else {
            self.schedule.clone()
        };

        // The cron crate requires Day of Week (field 6) to use names (Sun, Mon, etc.) or 1-7 (Sun-Sat)
        // Some standard crons use 0-6 or 0-7. We normalize 0 and 7 to Sun, and 1-6 to Mon-Sat.
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() >= 6 {
            let dow_field = fields[5];
            let fixed_dow = Self::normalize_dow_field(dow_field);

            // Reconstruct the expression with the fixed DOW field
            let new_fields_str: Vec<String> = fields
                .iter()
                .enumerate()
                .map(|(i, &f)| {
                    if i == 5 {
                        fixed_dow.clone()
                    } else {
                        f.to_string()
                    }
                })
                .collect();
            expr = new_fields_str.join(" ");
        }

        Schedule::from_str(&expr).map_err(|e| e.to_string())
    }

    fn normalize_dow_field(field: &str) -> String {
        let parts: Vec<&str> = field.splitn(2, '/').collect();
        let expr = parts[0];

        let fixed_expr = expr
            .split(',')
            .map(|item| {
                item.split('-')
                    .map(|val| match val {
                        "0" | "7" => "Sun",
                        "1" => "Mon",
                        "2" => "Tue",
                        "3" => "Wed",
                        "4" => "Thu",
                        "5" => "Fri",
                        "6" => "Sat",
                        other => other,
                    })
                    .collect::<Vec<_>>()
                    .join("-")
            })
            .collect::<Vec<_>>()
            .join(",");

        if parts.len() > 1 {
            format!("{}/{}", fixed_expr, parts[1])
        } else {
            fixed_expr
        }
    }

    /// Get the next scheduled run time
    pub fn next_run<Tz: chrono::TimeZone>(&self, tz: Tz) -> Option<chrono::DateTime<Tz>> {
        self.parse_schedule()
            .ok()
            .and_then(|schedule| schedule.upcoming(tz).next())
    }

    /// Get the retry policy
    pub fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_attempts: self.retry_count,
            delay_seconds: self.retry_delay,
        }
    }

    /// Check if job has timeout configured
    pub fn has_timeout(&self) -> bool {
        self.timeout > 0
    }
}

impl Default for Job {
    fn default() -> Self {
        Self {
            schedule: "* * * * *".to_string(),
            command: String::new(),
            description: String::new(),
            enabled: true,
            working_dir: None,
            environment: HashMap::new(),
            timeout: 0,
            shell: None,
            retry_count: 0,
            retry_delay: 60,
            user: None,
            capture_output: true,
            max_output_size: 1024 * 1024,
            run_on_startup: false,
            tags: Vec::new(),
            print_output: None,
        }
    }
}

/// Retry policy for failed jobs
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Delay between retries in seconds
    pub delay_seconds: u64,
}

impl RetryPolicy {
    /// Check if retry is enabled
    pub fn is_enabled(&self) -> bool {
        self.max_attempts > 0
    }
}

/// Status of a job execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is scheduled and waiting
    Pending,
    /// Job is currently running
    Running,
    /// Job completed successfully
    Success,
    /// Job failed with error
    Failed { error: String },
    /// Job was killed due to timeout
    Timeout,
    /// Job was manually cancelled
    Cancelled,
    /// Job is being retried
    Retrying { attempt: u32 },
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Success => write!(f, "success"),
            Self::Failed { error } => write!(f, "failed: {}", error),
            Self::Timeout => write!(f, "timeout"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Retrying { attempt } => write!(f, "retrying (attempt {})", attempt),
        }
    }
}

/// Record of a job execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExecution {
    /// Unique execution ID
    pub id: Uuid,
    /// Job name
    pub job_name: String,
    /// Start time
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// End time (if completed)
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Exit status
    pub status: JobStatus,
    /// Exit code (if available)
    pub exit_code: Option<i32>,
    /// Captured stdout
    pub stdout: Option<String>,
    /// Captured stderr
    pub stderr: Option<String>,
    /// Retry attempt number
    pub attempt: u32,
}

impl JobExecution {
    /// Create a new job execution record
    pub fn new(job_name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            job_name: job_name.into(),
            started_at: chrono::Utc::now(),
            ended_at: None,
            status: JobStatus::Running,
            exit_code: None,
            stdout: None,
            stderr: None,
            attempt: 1,
        }
    }

    /// Mark execution as completed with success
    pub fn complete_success(&mut self, exit_code: i32, stdout: String, stderr: String) {
        self.ended_at = Some(chrono::Utc::now());
        self.status = JobStatus::Success;
        self.exit_code = Some(exit_code);
        self.stdout = Some(stdout);
        self.stderr = Some(stderr);
    }

    /// Mark execution as failed
    pub fn complete_failed(
        &mut self,
        error: String,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    ) {
        self.ended_at = Some(chrono::Utc::now());
        self.status = JobStatus::Failed { error };
        self.exit_code = exit_code;
        self.stdout = Some(stdout);
        self.stderr = Some(stderr);
    }

    /// Mark execution as timed out
    pub fn complete_timeout(&mut self) {
        self.ended_at = Some(chrono::Utc::now());
        self.status = JobStatus::Timeout;
    }

    /// Get execution duration
    pub fn duration(&self) -> Option<chrono::Duration> {
        self.ended_at.map(|end| end - self.started_at)
    }

    /// Check if execution is still running
    pub fn is_running(&self) -> bool {
        matches!(self.status, JobStatus::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_validation() {
        let job = Job {
            schedule: "*/5 * * * *".to_string(),
            command: "echo hello".to_string(),
            ..Default::default()
        };

        assert!(job.validate("test").is_ok());
    }

    #[test]
    fn test_invalid_schedule() {
        let job = Job {
            schedule: "invalid".to_string(),
            command: "echo hello".to_string(),
            ..Default::default()
        };

        assert!(job.validate("test").is_err());
    }

    #[test]
    fn test_empty_command() {
        let job = Job {
            schedule: "* * * * *".to_string(),
            command: "".to_string(),
            ..Default::default()
        };

        assert!(job.validate("test").is_err());
    }

    #[test]
    fn test_next_run() {
        let job = Job {
            schedule: "* * * * *".to_string(),
            command: "echo test".to_string(),
            ..Default::default()
        };

        let next = job.next_run();
        assert!(next.is_some());
    }

    #[test]
    fn test_job_execution() {
        let mut exec = JobExecution::new("test-job");
        assert!(exec.is_running());

        exec.complete_success(0, "output".to_string(), "".to_string());
        assert!(!exec.is_running());
        assert!(matches!(exec.status, JobStatus::Success));
        assert!(exec.duration().is_some());
    }
}
