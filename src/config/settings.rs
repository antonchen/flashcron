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

    /// History size (number of job executions to keep)
    #[serde(default = "default_history_size")]
    pub history_size: usize,

    /// Timezone for cron expressions (default: UTC)
    #[serde(default = "default_timezone")]
    pub timezone: String,

    /// Grace period in seconds for shutdown
    #[serde(default = "default_grace_period")]
    pub shutdown_grace_period: u64,

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

fn default_history_size() -> usize {
    1000
}

fn default_timezone() -> String {
    "System".to_string()
}

fn default_grace_period() -> u64 {
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
            history_size: default_history_size(),
            timezone: default_timezone(),
            shutdown_grace_period: default_grace_period(),
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

    /// Parse log level to tracing Level
    pub fn tracing_level(&self) -> tracing::Level {
        match self.log_level.to_lowercase().as_str() {
            "trace" => tracing::Level::TRACE,
            "debug" => tracing::Level::DEBUG,
            "info" => tracing::Level::INFO,
            "warn" | "warning" => tracing::Level::WARN,
            "error" => tracing::Level::ERROR,
            _ => tracing::Level::INFO,
        }
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

    #[test]
    fn test_tracing_level() {
        let mut settings = Settings::default();

        settings.log_level = "debug".to_string();
        assert_eq!(settings.tracing_level(), tracing::Level::DEBUG);

        settings.log_level = "error".to_string();
        assert_eq!(settings.tracing_level(), tracing::Level::ERROR);

        settings.log_level = "invalid".to_string();
        assert_eq!(settings.tracing_level(), tracing::Level::INFO);
    }
}
