//! Error types for FlashCron

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias using FlashCron's Error type
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for FlashCron
#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Failed to parse config file '{path}': {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("Failed to read config file '{path}': {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Invalid cron expression '{expr}': {reason}")]
    CronParse { expr: String, reason: String },

    #[error("Job '{job_name}' not found")]
    JobNotFound { job_name: String },

    #[error("Job '{job_name}' failed with exit code {exit_code}")]
    JobFailed { job_name: String, exit_code: i32 },

    #[error("Job '{job_name}' timed out after {timeout_secs}s")]
    JobTimeout { job_name: String, timeout_secs: u64 },

    #[error("Failed to spawn job '{job_name}': {source}")]
    JobSpawn {
        job_name: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Scheduler error: {0}")]
    Scheduler(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Channel send error")]
    ChannelSend,

    #[error("Shutdown signal received")]
    Shutdown,
}

impl Error {
    /// Create a configuration error
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Create a cron parse error
    pub fn cron_parse(expr: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::CronParse {
            expr: expr.into(),
            reason: reason.into(),
        }
    }

    /// Create a job not found error
    pub fn job_not_found(name: impl Into<String>) -> Self {
        Self::JobNotFound {
            job_name: name.into(),
        }
    }

    /// Create a job failed error
    pub fn job_failed(name: impl Into<String>, exit_code: i32) -> Self {
        Self::JobFailed {
            job_name: name.into(),
            exit_code,
        }
    }

    /// Create a job timeout error
    pub fn job_timeout(name: impl Into<String>, timeout_secs: u64) -> Self {
        Self::JobTimeout {
            job_name: name.into(),
            timeout_secs,
        }
    }

    /// Check if this is a shutdown error
    pub fn is_shutdown(&self) -> bool {
        matches!(self, Self::Shutdown)
    }
}
