//! Configuration module for FlashCron

mod job;
mod settings;

pub use job::{Job, JobExecution, JobStatus, RetryPolicy};
pub use settings::*;

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Main configuration structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

        let value: toml::Value = toml::from_str(&content).map_err(|e| Error::ConfigParse {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;
        let mut config = Self::from_value(value.clone(), path)?;
        config.apply_file_defaults_from_value(&value, path);
        Ok(config)
    }

    #[cfg(feature = "web")]
    pub fn from_file_settings(path: impl AsRef<Path>) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WebConfig {
            #[serde(default)]
            settings: Settings,
        }

        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| Error::ConfigRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut value: toml::Value = toml::from_str(&content).map_err(|e| Error::ConfigParse {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;
        let ignored_jobs = value
            .as_table_mut()
            .and_then(|table| table.remove("jobs"))
            .and_then(|jobs| jobs.as_table().map(toml::map::Map::len))
            .unwrap_or_default();
        if ignored_jobs > 0 {
            log::warn!(status = "toml jobs ignored in web mode", count = ignored_jobs; "");
        }
        let parsed: WebConfig = value.clone().try_into().map_err(|e| Error::ConfigParse {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;
        let mut config = Self {
            settings: parsed.settings,
            jobs: HashMap::new(),
        };
        config.apply_file_defaults_from_value(&value, path);
        config.validate()?;
        Ok(config)
    }

    /// Parse configuration from a TOML string
    pub fn from_str(content: &str, path: impl AsRef<Path>) -> Result<Self> {
        let value: toml::Value = toml::from_str(content).map_err(|e| Error::ConfigParse {
            path: path.as_ref().to_path_buf(),
            source: Box::new(e),
        })?;

        Self::from_value(value, path)
    }

    fn from_value(value: toml::Value, path: impl AsRef<Path>) -> Result<Self> {
        let mut config: Config = value.try_into().map_err(|e| Error::ConfigParse {
            path: path.as_ref().to_path_buf(),
            source: Box::new(e),
        })?;

        config.compile_schedules()?;
        config.validate()?;
        Ok(config)
    }

    fn apply_file_defaults_from_value(&mut self, value: &toml::Value, path: &Path) {
        let settings = value.get("settings").and_then(toml::Value::as_table);
        let base = path.parent().unwrap_or_else(|| Path::new("."));

        if settings.is_none_or(|settings| !settings.contains_key("sql_file")) {
            self.settings.sql_file = base.join(DEFAULT_SQL_FILE).display().to_string();
        }

        #[cfg(feature = "web")]
        if settings.is_none_or(|settings| !settings.contains_key("script_dir")) {
            self.settings.script_dir = base.join(DEFAULT_SCRIPT_DIR);
        }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        self.settings.validate()?;

        #[cfg(feature = "web")]
        {
            if self.settings.job_history_size > self.settings.max_history_size {
                log::warn!(
                    status = "history size mismatch",
                    job_history_size = self.settings.job_history_size,
                    max_history_size = self.settings.max_history_size;
                    ""
                );
            }
        }

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

    fn compile_schedules(&mut self) -> Result<()> {
        for job in self.jobs.values_mut() {
            job.compile_schedule().map_err(|reason| Error::CronParse {
                expr: job.schedule.clone(),
                reason,
            })?;
        }
        Ok(())
    }

    /// Create default configuration
    pub fn default_config() -> String {
        format!(
            r#"# FlashCron Configuration File
# Documentation: https://github.com/antonchen/flashcron

[settings]
# Working directory for all jobs (default: current directory)
# working_dir = "/var/flashcron"

# Log level: trace, debug, info, warn, error
log_level = "{log_level}"

# Enable JSON logging format
json_logs = false

# Maximum concurrent jobs (0 = unlimited)
max_concurrent_jobs = {max_concurrent}

# Default shell for commands
shell = "{shell}"

# Whether to print command execution output to stdout/stderr
print_output = {print_output}

{web_settings}

# API authentication token for write operations (trigger, etc.)
# If not set, a random UUID will be generated and printed to logs on startup.
# api_token = "secret-token"

# Timezone for job scheduling. Priority: TZ env > config > system > UTC
# Examples: "System", "UTC", "Asia/Shanghai", "America/New_York"
timezone = "{timezone}"

{job_examples}
"#,
            job_examples = default_job_examples(),
            web_settings = default_web_settings(),
            log_level = settings::DEFAULT_LOG_LEVEL,
            max_concurrent = settings::DEFAULT_MAX_CONCURRENT_JOBS,
            shell = "/bin/sh",
            print_output = settings::DEFAULT_PRINT_OUTPUT,
            timezone = settings::DEFAULT_TIMEZONE,
        )
    }
}

#[cfg(feature = "web")]
fn default_web_settings() -> String {
    format!(
        "# Max history entries to keep per job.\njob_history_size = {}\n\
         # Max total history entries to keep globally.\nmax_history_size = {}",
        settings::DEFAULT_JOB_HISTORY_SIZE,
        settings::DEFAULT_MAX_HISTORY_SIZE
    )
}

#[cfg(not(feature = "web"))]
fn default_web_settings() -> String {
    String::new()
}

#[cfg(feature = "web")]
fn default_job_examples() -> &'static str {
    ""
}

#[cfg(not(feature = "web"))]
fn default_job_examples() -> &'static str {
    r#"# Example job definitions
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
# user = "backup"
environment = { BACKUP_TYPE = "full", COMPRESS = "true" }"#
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

    #[cfg(feature = "web")]
    #[test]
    fn web_settings_loader_ignores_toml_jobs() {
        let path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            path.path(),
            "[settings]\napi_port = 8123\n[jobs.invalid]\nschedule = 'not cron'\ncommand = ''\nunknown = true\n",
        )
        .unwrap();

        let config = Config::from_file_settings(path.path()).unwrap();
        assert_eq!(config.settings.api_port, 8123);
        assert!(config.jobs.is_empty());
    }

    #[test]
    fn test_default_config_is_valid() {
        let default = Config::default_config();
        let result = Config::from_str(&default, "default.toml");
        if let Err(e) = &result {
            eprintln!("Config error: {:?}", e);
        }
        assert!(result.is_ok(), "Config failed: {:?}", result.err());
    }

    #[cfg(feature = "web")]
    #[test]
    fn file_defaults_are_relative_to_config_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[settings]\n").unwrap();

        let config = Config::from_file(&path).unwrap();

        assert_eq!(
            config.settings.sql_file,
            dir.path().join("flashcron.db").display().to_string()
        );
        assert_eq!(config.settings.script_dir, dir.path().join("scripts"));
    }

    #[cfg(feature = "web")]
    #[test]
    fn explicit_relative_paths_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[settings]\nsql_file = 'custom.db'\nscript_dir = 'custom-scripts'\n",
        )
        .unwrap();

        let config = Config::from_file(&path).unwrap();

        assert_eq!(config.settings.sql_file, "custom.db");
        assert_eq!(config.settings.script_dir, Path::new("custom-scripts"));
    }
}
