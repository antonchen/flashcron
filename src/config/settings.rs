use crate::error::Result;
use chrono_tz::Tz;
use log::warn;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

//==============================================================================
// Constants for Default Values (Single Source of Truth)
//==============================================================================
pub const DEFAULT_LOG_LEVEL: &str = "info";
pub const DEFAULT_MAX_CONCURRENT_JOBS: usize = 10;
pub const DEFAULT_SHUTDOWN_TIMEOUT: u64 = 30;
pub const DEFAULT_PRINT_OUTPUT: bool = false;
pub const DEFAULT_TIMEZONE: &str = "System";
pub const DEFAULT_IPC_SOCKET_FALLBACK: &str = "/tmp/flashcron.sock";

#[cfg(feature = "web")]
pub const DEFAULT_JOB_HISTORY_SIZE: usize = 100;
#[cfg(feature = "web")]
pub const DEFAULT_MAX_HISTORY_SIZE: usize = 10000;
#[cfg(feature = "web")]
pub const DEFAULT_API_HOST: &str = "0.0.0.0";
#[cfg(feature = "web")]
pub const DEFAULT_API_PORT: u16 = 8080;
#[cfg(feature = "web")]
pub const DEFAULT_SCRIPT_DIR: &str = "scripts";
pub const DEFAULT_SQL_FILE: &str = "flashcron.db";

/// Global settings for FlashCron
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Working directory for all jobs
    pub working_dir: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Whether to log in JSON format
    #[serde(default)]
    pub json_logs: bool,

    /// Log file path
    pub log_file: Option<PathBuf>,

    /// Maximum number of jobs that can run simultaneously
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_jobs: usize,

    /// Default shell for executing commands
    #[serde(default = "default_shell")]
    pub shell: String,

    /// Arguments for the shell
    #[serde(default = "default_shell_args")]
    pub shell_args: Vec<String>,

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

    /// API authentication token
    #[cfg(feature = "web")]
    pub api_token: Option<String>,

    /// Directory containing scripts managed through the web interface
    #[cfg(feature = "web")]
    #[serde(default = "default_script_dir")]
    pub script_dir: PathBuf,

    /// SQLite database file path
    #[serde(default = "default_sql_file")]
    pub sql_file: String,

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

//==============================================================================
// Default Value Functions (invoked by serde)
//==============================================================================

pub fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_string()
}

pub fn default_max_concurrent() -> usize {
    DEFAULT_MAX_CONCURRENT_JOBS
}

pub fn default_shell() -> String {
    "/bin/sh".to_string()
}

pub fn default_shell_args() -> Vec<String> {
    vec!["-c".to_string()]
}

#[cfg(feature = "web")]
pub fn default_job_history_size() -> usize {
    DEFAULT_JOB_HISTORY_SIZE
}

#[cfg(feature = "web")]
pub fn default_max_history_size() -> usize {
    DEFAULT_MAX_HISTORY_SIZE
}

#[cfg(feature = "web")]
pub fn default_api_host() -> String {
    DEFAULT_API_HOST.to_string()
}

#[cfg(feature = "web")]
pub fn default_api_port() -> u16 {
    DEFAULT_API_PORT
}

#[cfg(feature = "web")]
pub fn default_script_dir() -> PathBuf {
    PathBuf::from(DEFAULT_SCRIPT_DIR)
}

pub fn default_timezone() -> String {
    DEFAULT_TIMEZONE.to_string()
}

pub fn default_shutdown_timeout() -> u64 {
    DEFAULT_SHUTDOWN_TIMEOUT
}

pub fn default_print_output() -> bool {
    DEFAULT_PRINT_OUTPUT
}

pub fn default_sql_file() -> String {
    DEFAULT_SQL_FILE.to_string()
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
            #[cfg(feature = "web")]
            job_history_size: default_job_history_size(),
            #[cfg(feature = "web")]
            max_history_size: default_max_history_size(),
            #[cfg(feature = "web")]
            api_host: default_api_host(),
            #[cfg(feature = "web")]
            api_port: default_api_port(),
            #[cfg(feature = "web")]
            api_token: None,
            #[cfg(feature = "web")]
            script_dir: default_script_dir(),
            sql_file: default_sql_file(),
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
        if self.timezone != DEFAULT_TIMEZONE {
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

    /// Validate settings that depend on the local runtime environment.
    pub fn validate(&self) -> Result<()> {
        if let Some(dir) = &self.working_dir {
            if !dir.exists() {
                warn!(
                    status = "global working directory does not exist",
                    working_dir = &*dir.display().to_string();
                    ""
                );
            }
        }

        Ok(())
    }
}

pub fn default_ipc_socket() -> PathBuf {
    PathBuf::from(DEFAULT_IPC_SOCKET_FALLBACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(settings.max_concurrent_jobs, DEFAULT_MAX_CONCURRENT_JOBS);
        assert_eq!(settings.print_output, DEFAULT_PRINT_OUTPUT);
        #[cfg(feature = "web")]
        assert_eq!(settings.api_host, "0.0.0.0");
        #[cfg(feature = "web")]
        assert_eq!(settings.script_dir, PathBuf::from("scripts"));
    }

    #[test]
    fn test_default_ipc_socket_is_fixed() {
        assert_eq!(
            default_ipc_socket(),
            PathBuf::from(DEFAULT_IPC_SOCKET_FALLBACK)
        );
    }
}
