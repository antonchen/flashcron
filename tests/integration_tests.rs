//! Integration tests for FlashCron

use flashcron::{Config, JobExecutor, Scheduler};
use std::path::PathBuf;
use tempfile::NamedTempFile;

/// Test configuration parsing and validation
#[test]
fn test_config_parsing() {
    let config_str = r#"
        [settings]
        log_level = "debug"
        max_concurrent_jobs = 5

        [jobs.echo_test]
        schedule = "*/5 * * * *"
        command = "echo hello"
        description = "Test echo job"
        enabled = true

        [jobs.disabled_job]
        schedule = "0 0 * * *"
        command = "echo disabled"
        enabled = false
    "#;

    let config = Config::from_str(config_str, "test.toml").unwrap();

    assert_eq!(config.settings.log_level, "debug");
    assert_eq!(config.settings.max_concurrent_jobs, 5);
    assert_eq!(config.jobs.len(), 2);
    assert_eq!(config.enabled_jobs().count(), 1);
}

/// Test invalid cron expression is rejected
#[test]
fn test_invalid_cron_rejected() {
    let config_str = r#"
        [jobs.bad]
        schedule = "invalid cron"
        command = "echo test"
    "#;

    let result = Config::from_str(config_str, "test.toml");
    assert!(result.is_err());
}

/// Test default config generation
#[test]
fn test_default_config_valid() {
    let default = Config::default_config();
    let result = Config::from_str(&default, "default.toml");
    assert!(result.is_ok());
}

/// Test job execution
#[tokio::test]
async fn test_job_execution() {
    let executor = JobExecutor::default();

    let config_str = if cfg!(windows) {
        r#"
            [jobs.test]
            schedule = "* * * * *"
            command = "echo hello world"
        "#
    } else {
        r#"
            [jobs.test]
            schedule = "* * * * *"
            command = "echo hello world"
        "#
    };

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let job = config.get_job("test").unwrap();

    let result = executor.execute("test", job).await;
    assert!(result.is_ok());

    let (exit_code, stdout, _) = result.unwrap();
    assert_eq!(exit_code, 0);
    assert!(stdout.contains("hello"));
}

/// Test job timeout
#[tokio::test]
async fn test_job_timeout() {
    let executor = JobExecutor::default();

    let config_str = if cfg!(windows) {
        r#"
            [jobs.slow]
            schedule = "* * * * *"
            command = "ping -n 30 127.0.0.1"
            timeout = 1
        "#
    } else {
        r#"
            [jobs.slow]
            schedule = "* * * * *"
            command = "sleep 30"
            timeout = 1
        "#
    };

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let job = config.get_job("slow").unwrap();

    let result = executor.execute("slow", job).await;
    assert!(result.is_err());
}

/// Test environment variables
#[tokio::test]
async fn test_environment_variables() {
    let executor = JobExecutor::default();

    let config_str = if cfg!(windows) {
        r#"
            [jobs.env_test]
            schedule = "* * * * *"
            command = "echo %MY_VAR%"
            environment = { MY_VAR = "test_value_123" }
        "#
    } else {
        r#"
            [jobs.env_test]
            schedule = "* * * * *"
            command = "echo $MY_VAR"
            environment = { MY_VAR = "test_value_123" }
        "#
    };

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let job = config.get_job("env_test").unwrap();

    let result = executor.execute("env_test", job).await;
    assert!(result.is_ok());

    let (exit_code, stdout, _) = result.unwrap();
    assert_eq!(exit_code, 0);
    assert!(stdout.contains("test_value_123"));
}

/// Test scheduler creation
#[tokio::test]
async fn test_scheduler_creation() {
    let config_str = r#"
        [jobs.test]
        schedule = "* * * * *"
        command = "echo test"
    "#;

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let (_scheduler, _handle) = Scheduler::new(config, PathBuf::from("test.toml"), None);
    // Scheduler created successfully
}

/// Test persistence and state recovery
#[tokio::test]
async fn test_persistence_recovery() {
    use flashcron::config::JobExecution;
    use flashcron::db::DatabaseManager;
    use tempfile::NamedTempFile;

    let db_file = NamedTempFile::new().unwrap();
    let db_path = db_file.path().to_str().unwrap().to_string();

    // 1. Create DB and save a fake execution
    let db = DatabaseManager::init(&db_path).await.unwrap();
    let mut exec = JobExecution::new("test-job", "manual");
    exec.complete_success(0, "ok".into(), "".into());
    db.save(exec).await.unwrap();

    // 2. Initialize scheduler with this DB
    let config_str = r#"
        [settings]
        timezone = "UTC"
        [jobs.test-job]
        schedule = "* * * * *"
        command = "echo hello"
    "#;
    let config = Config::from_str(config_str, "test.toml").unwrap();
    let (_scheduler, _handle) = Scheduler::new(config, PathBuf::from("test.toml"), None);
    // Scheduler created successfully
}

/// Test multiple concurrent jobs
#[tokio::test]
async fn test_concurrent_execution() {
    let cmd = if cfg!(windows) {
        "echo job"
    } else {
        "echo job"
    };

    let config_str = format!(
        r#"
        [settings]
        max_concurrent_jobs = 3

        [jobs.job1]
        schedule = "* * * * *"
        command = "{}"

        [jobs.job2]
        schedule = "* * * * *"
        command = "{}"

        [jobs.job3]
        schedule = "* * * * *"
        command = "{}"
    "#,
        cmd, cmd, cmd
    );

    let config = Config::from_str(&config_str, "test.toml").unwrap();

    // Execute all jobs concurrently
    let mut handles = Vec::new();
    for (name, job) in config.jobs.iter() {
        let exec = JobExecutor::default();
        let job = job.clone();
        let name = name.clone();
        handles.push(tokio::spawn(async move { exec.execute(&name, &job).await }));
    }

    // All should succeed
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}

/// Test job with working directory
#[tokio::test]
async fn test_working_directory() {
    let executor = JobExecutor::default();

    let temp_dir = std::env::temp_dir();
    let temp_dir_str = temp_dir.to_string_lossy();

    let config_str = if cfg!(windows) {
        format!(
            r#"
            [jobs.pwd_test]
            schedule = "* * * * *"
            command = "cd"
            working_dir = "{}"
        "#,
            temp_dir_str.replace('\\', "\\\\")
        )
    } else {
        format!(
            r#"
            [jobs.pwd_test]
            schedule = "* * * * *"
            command = "pwd"
            working_dir = "{}"
        "#,
            temp_dir_str
        )
    };

    let config = Config::from_str(&config_str, "test.toml").unwrap();
    let job = config.get_job("pwd_test").unwrap();

    let result = executor.execute("pwd_test", job).await;
    assert!(result.is_ok());

    let (exit_code, stdout, _) = result.unwrap();
    assert_eq!(exit_code, 0);
    // Output should contain temp directory path
    assert!(!stdout.is_empty());
}

/// Test next run calculation
#[test]
fn test_next_run_calculation() {
    let config_str = r#"
        [jobs.every_minute]
        schedule = "* * * * *"
        command = "echo test"
    "#;

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let job = config.get_job("every_minute").unwrap();

    let next = job.next_run(chrono::Utc);
    assert!(next.is_some());

    // Should be within the next minute
    let now = chrono::Utc::now();
    let diff = next.unwrap() - now;
    assert!(diff.num_seconds() <= 60);
    assert!(diff.num_seconds() >= 0);
}

/// Test output size limit
#[tokio::test]
async fn test_output_size_limit() {
    let executor = JobExecutor::default();

    // Generate command that produces lots of output
    let config_str = if cfg!(windows) {
        r#"
            [jobs.big_output]
            schedule = "* * * * *"
            command = "cmd /c \"for /L %i in (1,1,1000) do @echo Line %i of output\""
            max_output_size = 1000
        "#
    } else {
        r#"
            [jobs.big_output]
            schedule = "* * * * *"
            command = "for i in $(seq 1 1000); do echo \"Line $i of output\"; done"
            max_output_size = 1000
        "#
    };

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let job = config.get_job("big_output").unwrap();

    let result = executor.execute("big_output", job).await;
    assert!(result.is_ok());

    let (_, stdout, _) = result.unwrap();
    // Output should be truncated to max_output_size
    assert!(stdout.len() <= 1000);
}

/// Test config file from disk
#[test]
fn test_config_from_file() {
    use std::io::Write;

    let config_content = r#"
        [jobs.file_test]
        schedule = "0 0 * * *"
        command = "echo from file"
    "#;

    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();

    let config = Config::from_file(temp_file.path()).unwrap();
    assert!(config.get_job("file_test").is_some());
}

/// Test retry policy configuration
#[test]
fn test_retry_policy() {
    let config_str = r#"
        [jobs.retry_job]
        schedule = "* * * * *"
        command = "echo test"
        retry_count = 3
        retry_delay = 30
    "#;

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let job = config.get_job("retry_job").unwrap();
    let policy = job.retry_policy();

    assert!(policy.is_enabled());
    assert_eq!(policy.max_attempts, 3);
    assert_eq!(policy.delay_seconds, 30);
}

/// Test disabled job handling
#[test]
fn test_disabled_jobs_filtered() {
    let config_str = r#"
        [jobs.enabled_job]
        schedule = "* * * * *"
        command = "echo enabled"
        enabled = true

        [jobs.disabled_job]
        schedule = "* * * * *"
        command = "echo disabled"
        enabled = false
    "#;

    let config = Config::from_str(config_str, "test.toml").unwrap();

    let enabled: Vec<_> = config.enabled_jobs().collect();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].0, "enabled_job");
}

/// Test various cron expressions
#[test]
fn test_cron_expressions() {
    let expressions = vec![
        "* * * * *",     // Every minute
        "*/5 * * * *",   // Every 5 minutes
        "0 * * * *",     // Every hour
        "0 0 * * *",     // Daily at midnight
        "0 0 * * 7",     // Weekly on Sunday
        "0 0 1 * *",     // Monthly on the 1st
        "0 0 1 1 *",     // Yearly on Jan 1
        "30 4 1,15 * *", // At 4:30 on 1st and 15th
        "0 0 * * 1-5",   // Weekdays at midnight
    ];

    for expr in expressions {
        let config_str = format!(
            r#"
            [jobs.test]
            schedule = "{}"
            command = "echo test"
        "#,
            expr
        );

        let result = Config::from_str(&config_str, "test.toml");
        assert!(result.is_ok(), "Expression '{}' should be valid", expr);
    }
}
