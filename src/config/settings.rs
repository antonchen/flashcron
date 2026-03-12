//! Global settings configuration

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

/// Global settings for the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Working directory for all jobs
    #[serde(default)]
    pub working_dir: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Enable JSON logging format
    #[serde(default)]
    pub json_logs: bool,

    /// Log file path (if not set, logs to stdout)
    #[serde(default)]
    pub log_file: Option<PathBuf>,

    /// Maximum concurrent jobs
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_jobs: usize,

    /// Default shell for commands
    #[serde(default = "default_shell")]
    pub shell: String,

    /// Shell arguments (e.g., ["-c"] for sh)
    #[serde(default = "default_shell_args")]
    pub shell_args: Vec<String>,

    /// Watch config file for changes
    #[serde(default = "default_watch_config")]
    pub watch_config: bool,

    /// PID file path
    #[serde(default)]
    pub pid_file: Option<PathBuf>,

    /// Prometheus metrics endpoint address
    #[cfg(feature = "metrics")]
    #[serde(default)]
    pub metrics_addr: Option<String>,

    /// Max history entries to keep per job
    #[cfg(feature = "web")]
    #[serde(default = "default_job_history_size")]
    pub job_history_size: usize,

    /// Max total history entries to keep globally
    #[cfg(feature = "web")]
    #[serde(default = "default_max_history_size")]
    pub max_history_size: usize,

    /// API Server host
    #[cfg(feature = "web")]
    #[serde(default = "default_api_host")]
    pub api_host: String,

    /// API Server port
    #[cfg(feature = "web")]
    #[serde(default = "default_api_port")]
    pub api_port: u16,

    /// Timezone for cron expressions (default: UTC)
    #[serde(default = "default_timezone")]
    pub timezone: String,

    /// Grace period in seconds for shutdown
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout: u64,

    /// Whether to print command execution output
    #[serde(default = "default_print_output")]
    pub print_output: bool,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_max_concurrent() -> usize {
    10
}

fn default_shell() -> String {
    if cfg!(windows) {
        "cmd".to_string()
    } else {
        "/bin/sh".to_string()
    }
}

fn default_shell_args() -> Vec<String> {
    if cfg!(windows) {
        vec!["/C".to_string()]
    } else {
        vec!["-c".to_string()]
    }
}

fn default_watch_config() -> bool {
    true
}

#[cfg(feature = "web")]
fn default_job_history_size() -> usize {
    100
}

#[cfg(feature = "web")]
fn default_max_history_size() -> usize {
    10000
}

#[cfg(feature = "web")]
fn default_api_host() -> String {
    "127.0.0.1".to_string()
}

#[cfg(feature = "web")]
fn default_api_port() -> u16 {
    8080
}

fn default_timezone() -> String {
    "System".to_string()
}

fn default_shutdown_timeout() -> u64 {
    30
}

fn default_print_output() -> bool {
    false
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            working_dir: None,
            log_level: default_log_level(),
            json_logs: false,
            log_file: None,
            max_concurrent_jobs: default_max_concurrent(),
            shell: default_shell(),
            shell_args: default_shell_args(),
            watch_config: default_watch_config(),
            pid_file: None,
            #[cfg(feature = "metrics")]
            metrics_addr: None,
            #[cfg(feature = "web")]
            job_history_size: default_job_history_size(),
            #[cfg(feature = "web")]
            max_history_size: default_max_history_size(),
            #[cfg(feature = "web")]
            api_host: default_api_host(),
            #[cfg(feature = "web")]
            api_port: default_api_port(),
            timezone: default_timezone(),
            shutdown_timeout: default_shutdown_timeout(),
            print_output: default_print_output(),
        }
    }
}

impl Settings {
    /// Get the effective timezone based on priority:
    /// 1. TZ environment variable
    /// 2. Configured timezone (if not "System")
    /// 3. System timezone
    /// 4. UTC (fallback)
    pub fn effective_timezone(&self) -> Tz {
        // 1. TZ environment variable
        if let Ok(tz_str) = std::env::var("TZ") {
            if let Ok(tz) = Tz::from_str(&tz_str) {
                return tz;
            }
        }

        // 2. Configured timezone (if not the default "System")
        if self.timezone != "System" {
            if let Ok(tz) = Tz::from_str(&self.timezone) {
                return tz;
            }
        }

        // 3. System timezone
        if let Ok(tz_str) = iana_time_zone::get_timezone() {
            if let Ok(tz) = Tz::from_str(&tz_str) {
                return tz;
            }
        }

        // 4. Fallback to UTC
        Tz::UTC
    }

    /// Get the shell command parts for executing a command
    pub fn shell_command(&self) -> (&str, &[String]) {
        (&self.shell, &self.shell_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.log_level, "info");
        assert_eq!(settings.max_concurrent_jobs, 10);
        assert!(settings.watch_config);
        assert!(!settings.print_output);
    }
}
