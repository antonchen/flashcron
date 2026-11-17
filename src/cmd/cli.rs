use super::daemon::{ipc_socket_path, send_ipc_request};
use anyhow::{bail, Result};
use flashcron::ipc::{IpcRequest, IpcResponse, StatusPayload};
use flashcron::Config;
#[cfg(feature = "web")]
use std::path::Path;
use std::path::PathBuf;
use uuid::Uuid;

/// Validate configuration
pub fn validate_config(config_path: PathBuf) -> Result<()> {
    println!("Validating configuration: {:?}", config_path);

    let loaded = {
        #[cfg(feature = "web")]
        {
            Config::from_file_settings(&config_path)
        }
        #[cfg(not(feature = "web"))]
        {
            Config::from_file(&config_path)
        }
    };
    match loaded {
        Ok(config) => {
            println!("✓ Configuration is valid");

            #[cfg(feature = "web")]
            {
                if config.settings.job_history_size > config.settings.max_history_size {
                    println!(
                        "! Warning: job_history_size ({}) is greater than max_history_size ({})",
                        config.settings.job_history_size, config.settings.max_history_size
                    );
                    println!("  Individual job history will be limited by the global maximum.");
                }
            }

            println!(
                "  Jobs: {} total, {} enabled",
                config.jobs.len(),
                config.enabled_jobs().count()
            );

            for (name, job) in &config.jobs {
                let status = if job.enabled { "enabled" } else { "disabled" };
                println!("  - {} [{}]: {}", name, status, job.schedule);
            }

            Ok(())
        }
        Err(e) => {
            println!("✗ Configuration is invalid:");
            println!("  {}", e);
            std::process::exit(1);
        }
    }
}

/// List configured jobs
pub async fn list_jobs(config_path: PathBuf, enabled_only: bool, format: &str) -> Result<()> {
    #[cfg(feature = "web")]
    let config = load_web_cli_config(&config_path).await?;
    #[cfg(not(feature = "web"))]
    let config = Config::from_file(&config_path)?;
    let tz = config.settings.effective_timezone();

    let jobs: Vec<_> = if enabled_only {
        config.enabled_jobs().collect()
    } else {
        config.jobs.iter().collect()
    };

    match format {
        "json" => {
            let output: Vec<_> = jobs
                .iter()
                .map(|(name, job)| {
                    serde_json::json!({
                        "name": name,
                        "schedule": job.schedule,
                        "command": job.command,
                        "enabled": job.enabled,
                        "description": job.description,
                        "next_run": job.next_run(tz).map(|t| t.to_rfc3339()),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        _ => {
            println!(
                "{:<20} {:<20} {:<10} NEXT RUN ({})",
                "NAME", "SCHEDULE", "STATUS", tz
            );
            println!("{}", "-".repeat(75));

            for (name, job) in jobs {
                let status = if job.enabled { "enabled" } else { "disabled" };
                let next_run = job
                    .next_run(tz)
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "-".to_string());

                println!(
                    "{:<20} {:<20} {:<10} {}",
                    name, job.schedule, status, next_run
                );
            }
        }
    }

    Ok(())
}

/// Trigger a job immediately
pub async fn trigger_job(config_path: Option<PathBuf>, job_name: &str) -> Result<()> {
    match send_ipc_request(
        config_path.as_deref(),
        &IpcRequest::Trigger {
            job_name: job_name.to_string(),
        },
    )
    .await?
    {
        IpcResponse::Triggered { execution_id } => {
            println!("Triggered job '{}' ({})", job_name, execution_id);
            Ok(())
        }
        IpcResponse::Error { message } => bail!("{}", message),
        _ => bail!("Unexpected IPC response"),
    }
}

/// Generate default configuration
pub fn init_config(output: PathBuf, force: bool) -> Result<()> {
    if output.exists() && !force {
        anyhow::bail!(
            "File {:?} already exists. Use --force to overwrite.",
            output
        );
    }

    let default_config = Config::default_config();
    std::fs::write(&output, default_config)?;

    println!("✓ Created configuration file: {:?}", output);
    println!("\nEdit the file to configure your jobs, then run:");
    println!("  flashcron run -c {:?}", output);

    Ok(())
}

pub async fn stop_daemon(config_path: Option<PathBuf>) -> Result<()> {
    match send_ipc_request(config_path.as_deref(), &IpcRequest::Stop).await? {
        IpcResponse::Stopped => {
            println!("Shutdown signal sent.");
            Ok(())
        }
        IpcResponse::Error { message } => bail!("{}", message),
        _ => bail!("Unexpected IPC response"),
    }
}

/// Show daemon status
pub async fn show_status(config_path: Option<PathBuf>) -> Result<()> {
    match send_ipc_request(config_path.as_deref(), &IpcRequest::Status).await? {
        IpcResponse::Status(StatusPayload {
            config_path: daemon_config_path,
            state,
        }) => {
            println!("Config: {}", daemon_config_path);
            println!(
                "Socket: {}",
                ipc_socket_path(config_path.as_deref())?.display()
            );
            println!("Running: {}", state.is_running);
            println!("Queue: {}", state.queue_len);
            println!(
                "Jobs: {} total, {} enabled",
                state.total_jobs, state.enabled_jobs
            );
            println!(
                "Executions: {} total, {} failures",
                state.total_executions, state.total_failures
            );
            println!("Started: {}", state.started_at);
            println!("Current: {}", state.current_time);
            println!();
            println!(
                "{:<20} {:<10} {:<10} {:<20}",
                "NAME", "STATE", "STATUS", "NEXT RUN"
            );
            println!("{}", "-".repeat(75));
            for job in state.jobs.values() {
                let last_status = job
                    .last_status
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "-".to_string());
                let next_run = job
                    .next_run
                    .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "-".to_string());
                println!(
                    "{:<20} {:<10} {:<10} {}",
                    job.name,
                    format!("{:?}", job.run_state).to_lowercase(),
                    last_status,
                    next_run
                );
            }
            Ok(())
        }
        IpcResponse::Error { message } => bail!("{}", message),
        _ => bail!("Unexpected IPC response"),
    }
}

/// Show upcoming schedule
pub async fn show_schedule(config_path: PathBuf, count: usize) -> Result<()> {
    #[cfg(feature = "web")]
    let config = load_web_cli_config(&config_path).await?;
    #[cfg(not(feature = "web"))]
    let config = Config::from_file(&config_path)?;
    let tz = config.settings.effective_timezone();

    println!("Next {} scheduled runs (Timezone: {}):", count, tz);
    println!("{:<25} JOB", "TIME");
    println!("{}", "-".repeat(50));

    // Collect all upcoming runs
    let mut runs: Vec<_> = config
        .enabled_jobs()
        .flat_map(|(name, job)| {
            let name = name.clone();
            job.parse_schedule()
                .ok()
                .into_iter()
                .flat_map(move |schedule| {
                    let name = name.clone();
                    schedule
                        .upcoming(tz)
                        .take(count)
                        .map(move |time| (time, name.clone()))
                        .collect::<Vec<_>>()
                })
        })
        .collect();

    // Sort by time
    runs.sort_by_key(|(time, _)| *time);

    // Show top N
    for (time, name) in runs.into_iter().take(count) {
        println!("{:<25} {}", time.format("%Y-%m-%d %H:%M:%S"), name);
    }

    Ok(())
}

#[cfg(feature = "web")]
async fn load_web_cli_config(config_path: &Path) -> Result<Config> {
    let mut config = Config::from_file_settings(config_path)?;
    if Path::new(&config.settings.sql_file).exists() {
        let db = flashcron::db::DatabaseManager::init(&config.settings.sql_file).await?;
        let mut jobs = db.list_jobs().await?;
        for (name, job) in &mut jobs {
            job.compile_schedule().map_err(|reason| {
                anyhow::anyhow!("Invalid schedule for stored job '{name}': {reason}")
            })?;
            job.validate(name)?;
        }
        config.jobs = jobs;
    }
    Ok(config)
}

pub async fn show_history(
    config_path: Option<PathBuf>,
    job_name: Option<String>,
    limit: usize,
    id: Option<String>,
) -> Result<()> {
    if let Some(exec_id) = id {
        let execution_id = Uuid::parse_str(&exec_id)?;
        match send_ipc_request(
            config_path.as_deref(),
            &IpcRequest::HistoryGet { execution_id },
        )
        .await?
        {
            IpcResponse::HistoryEntry {
                execution: Some(exec),
            } => {
                println!("Job: {}", exec.job_name);
                println!("ID: {}", exec.id);
                let status_str = match exec.status {
                    flashcron::config::JobStatus::Success => "Success".to_string(),
                    flashcron::config::JobStatus::Failed { ref error } => {
                        format!("Failed: {}", error)
                    }
                    flashcron::config::JobStatus::Timeout => "Timeout".to_string(),
                    _ => "Unknown".to_string(),
                };
                println!("Status: {}", status_str);
                println!("Start Time: {}", exec.started_at);
                if let Some(end) = exec.ended_at {
                    println!("End Time: {}", end);
                    let ms = (end - exec.started_at).num_milliseconds();
                    if ms < 1000 {
                        println!("Duration: {}ms", ms);
                    } else {
                        println!("Duration: {:.2}s", ms as f64 / 1000.0);
                    }
                }

                println!("\n--- OUTPUT ---");
                match exec.output.as_deref() {
                    Some(output) if !output.trim().is_empty() => println!("{}", output.trim()),
                    _ => println!("(No output)"),
                }
            }
            IpcResponse::HistoryEntry { execution: None } => {
                println!("Execution ID {} not found.", exec_id);
            }
            IpcResponse::Error { message } => bail!("{}", message),
            _ => bail!("Unexpected IPC response"),
        }
    } else {
        match send_ipc_request(
            config_path.as_deref(),
            &IpcRequest::HistoryList { job_name, limit },
        )
        .await?
        {
            IpcResponse::HistoryEntries { history } => {
                if history.is_empty() {
                    println!("No history found.");
                    return Ok(());
                }

                println!(
                    "{:<36} | {:<20} | {:<10} | {:<25}",
                    "Execution ID", "Job Name", "Status", "Start Time"
                );
                println!("{:-<36}-+-{:-<20}-+-{:-<10}-+-{:-<25}", "", "", "", "");

                for exec in history {
                    let status_str = match exec.status {
                        flashcron::config::JobStatus::Success => "Success",
                        flashcron::config::JobStatus::Failed { .. } => "Failed",
                        flashcron::config::JobStatus::Timeout => "Timeout",
                        _ => "Unknown",
                    };
                    println!(
                        "{:<36} | {:<20} | {:<10} | {}",
                        exec.id,
                        exec.job_name.chars().take(20).collect::<String>(),
                        status_str,
                        exec.started_at.format("%Y-%m-%d %H:%M:%S")
                    );
                }
            }
            IpcResponse::Error { message } => bail!("{}", message),
            _ => bail!("Unexpected IPC response"),
        }
    }
    Ok(())
}
