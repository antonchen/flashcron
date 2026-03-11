//! Configuration module for FlashCron

mod job;
mod settings;

pub use job::{Job, JobExecution, JobStatus, RetryPolicy};
pub use settings::Settings;

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Main configuration structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Global settings
    #[serde(default)]
    pub settings: Settings,

    /// Job definitions
    #[serde(default)]
    pub jobs: HashMap<String, Job>,
}

impl Config {
    /// Load configuration from a TOML file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| Error::ConfigRead {
            path: path.to_path_buf(),
            source: e,
        })?;

        Self::from_str(&content, path)
    }

    /// Parse configuration from a TOML string
    pub fn from_str(content: &str, path: impl AsRef<Path>) -> Result<Self> {
        let config: Config = toml::from_str(content).map_err(|e| Error::ConfigParse {
            path: path.as_ref().to_path_buf(),
            source: Box::new(e),
        })?;

        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        for (name, job) in &self.jobs {
            job.validate(name)?;
        }
        Ok(())
    }

    /// Get a job by name
    pub fn get_job(&self, name: &str) -> Option<&Job> {
        self.jobs.get(name)
    }

    /// Get all enabled jobs
    pub fn enabled_jobs(&self) -> impl Iterator<Item = (&String, &Job)> {
        self.jobs.iter().filter(|(_, job)| job.enabled)
    }

    /// Create default configuration
    pub fn default_config() -> String {
        r#"# FlashCron Configuration File
# Documentation: https://github.com/yourusername/flashcron

[settings]
# Working directory for all jobs (default: current directory)
# working_dir = "/var/flashcron"

# Log level: trace, debug, info, warn, error
log_level = "info"

# Enable JSON logging format
json_logs = false

# Maximum concurrent jobs (0 = unlimited)
max_concurrent_jobs = 10

# Default shell for commands
shell = "/bin/sh"

# Whether to print command execution output to stdout/stderr
print_output = false

# Enable config file watching for hot reload
watch_config = true

# Timezone for job scheduling and display
# Priority: TZ env variable > config > system timezone > UTC
# Examples: "System", "UTC", "Asia/Shanghai", "America/New_York"
timezone = "System"

# Prometheus metrics endpoint (requires 'metrics' feature)
# metrics_addr = "127.0.0.1:9090"

# Example job definitions
[jobs.hello]
schedule = "*/5 * * * *"  # Every 5 minutes
command = "echo 'Hello from FlashCron!'"
description = "A simple hello world job"
enabled = true

[jobs.cleanup]
schedule = "0 3 * * *"  # Every day at 3 AM
command = "echo 'Cleanup starting...'"
description = "Daily cleanup task"
enabled = true
timeout = 3600  # 1 hour timeout
retry_count = 3
retry_delay = 60
print_output = false  # Override global setting for this job

[jobs.backup]
schedule = "0 2 * * 7"  # Every Sunday at 2 AM
command = "echo 'Backup starting...'"
description = "Weekly backup"
enabled = true
environment = { BACKUP_TYPE = "full", COMPRESS = "true" }
"#
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let config = r#"
            [jobs.test]
            schedule = "* * * * *"
            command = "echo test"
        "#;

        let cfg = Config::from_str(config, "test.toml").unwrap();
        assert!(cfg.jobs.contains_key("test"));
    }

    #[test]
    fn test_parse_full_config() {
        let config = r#"
            [settings]
            log_level = "debug"
            max_concurrent_jobs = 5

            [jobs.hello]
            schedule = "*/5 * * * *"
            command = "echo hello"
            description = "Test job"
            enabled = true
            timeout = 60
            retry_count = 3

            [jobs.backup]
            schedule = "0 2 * * *"
            command = "/bin/backup.sh"
            working_dir = "/tmp"
            environment = { FOO = "bar" }
        "#;

        let cfg = Config::from_str(config, "test.toml").unwrap();
        assert_eq!(cfg.settings.log_level, "debug");
        assert_eq!(cfg.settings.max_concurrent_jobs, 5);
        assert_eq!(cfg.jobs.len(), 2);
    }

    #[test]
    fn test_invalid_cron_expression() {
        let config = r#"
            [jobs.bad]
            schedule = "invalid"
            command = "echo test"
        "#;

        let result = Config::from_str(config, "test.toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_default_config_is_valid() {
        let default = Config::default_config();
        let result = Config::from_str(&default, "default.toml");
        if let Err(ref e) = result {
            eprintln!("Config error: {:?}", e);
        }
        assert!(result.is_ok(), "Config failed: {:?}", result.err());
    }
}
