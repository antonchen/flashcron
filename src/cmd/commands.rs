use anyhow::{Context, Result};
use flashcron::{Config, Scheduler};
use log::{error, info};
use std::path::PathBuf;

/// Run the daemon
pub async fn run_daemon(config_path: PathBuf) -> Result<()> {
    info!("Starting FlashCron v{}", flashcron::VERSION);

    // Load configuration
    let config = Config::from_file(&config_path)
        .with_context(|| format!("Failed to load config from {:?}", config_path))?;

    crate::cmd::state::save_state(&config_path);

    info!(
        "Loaded {} jobs ({} enabled)",
        config.jobs.len(),
        config.enabled_jobs().count()
    );

    // Create scheduler
    #[cfg(feature = "web")]
    let api_host = config.settings.api_host.clone();
    #[cfg(feature = "web")]
    let api_port = config.settings.api_port;

    let (scheduler, handle) = Scheduler::new(config, config_path.clone());

    #[cfg(feature = "web")]
    let api_task = {
        let api_state = flashcron::api::ApiState {
            scheduler_state: scheduler.get_state(),
            handle: handle.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = flashcron::api::start_api_server(api_state, &api_host, api_port).await {
                error!("API server error: {}", e);
            }
        })
    };

    // Setup config file watcher
    let reload_handle = handle.clone();
    let watch_path = config_path.clone();
    let watcher_task = tokio::spawn(async move {
        if let Err(e) = watch_config_file(watch_path, reload_handle).await {
            error!("Config watcher error: {}", e);
        }
    });

    // Wait for shutdown signal in a separate task
    let scheduler_handle_for_sig = handle.clone();
    tokio::spawn(async move {
        if let Err(e) = wait_for_shutdown_signal().await {
            error!("Signal handler error: {}", e);
        }
        info!("Shutting down gracefully...");
        let _ = scheduler_handle_for_sig.shutdown().await;
    });

    // Run scheduler to completion
    if let Err(e) = scheduler.run().await {
        error!("Scheduler error: {}", e);
    }

    // Abort background tasks
    watcher_task.abort();

    #[cfg(feature = "web")]
    api_task.abort();

    crate::cmd::state::clear_state();

    info!("FlashCron stopped");
    Ok(())
}

/// Wait for shutdown signal (Ctrl+C or SIGTERM)
async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sighup = signal(SignalKind::hangup())?;

        tokio::select! {
            res = sigterm.recv() => { if res.is_none() { return Ok(()); } info!("Received SIGTERM"); },
            res = sigint.recv() => { if res.is_none() { return Ok(()); } info!("Received SIGINT"); },
            res = sighup.recv() => { if res.is_none() { return Ok(()); } info!("Received SIGHUP"); },
        }
    }

    #[cfg(windows)]
    {
        tokio::signal::ctrl_c().await?;
        info!("Received Ctrl+C");
    }

    Ok(())
}

/// Watch config file for changes
async fn watch_config_file(
    path: PathBuf,
    handle: flashcron::scheduler::SchedulerHandle,
) -> Result<()> {
    use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        NotifyConfig::default().with_poll_interval(Duration::from_secs(2)),
    )?;

    watcher.watch(&path, RecursiveMode::NonRecursive)?;

    info!("Watching config file {:?} for changes", path);

    loop {
        // Use try_recv to avoid blocking the tokio worker thread
        match rx.try_recv() {
            Ok(Ok(event)) => {
                if event.kind.is_modify() || event.kind.is_create() {
                    info!("Config file changed, reloading...");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if let Err(e) = handle.reload_config().await {
                        error!("Failed to reload config: {}", e);
                    }
                }
            }
            Ok(Err(e)) => {
                error!("Watch error: {:?}", e);
            }
            Err(mpsc::TryRecvError::Empty) => {
                // Yield to tokio and wait a bit before checking again
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                break;
            }
        }
    }

    Ok(())
}

/// Validate configuration
pub fn validate_config(config_path: PathBuf) -> Result<()> {
    println!("Validating configuration: {:?}", config_path);

    match Config::from_file(&config_path) {
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
pub fn list_jobs(config_path: PathBuf, enabled_only: bool, format: &str) -> Result<()> {
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
pub async fn trigger_job(config_path: PathBuf, job_name: &str) -> Result<()> {
    let config = Config::from_file(&config_path)?;

    let job = config
        .get_job(job_name)
        .ok_or_else(|| anyhow::anyhow!("Job '{}' not found", job_name))?;

    println!("Triggering job: {}", job_name);
    println!("Command: {}", job.command);

    let executor = flashcron::JobExecutor::default();
    let start = std::time::Instant::now();

    match executor.execute(job_name, job).await {
        Ok((exit_code, stdout, stderr)) => {
            let duration = start.elapsed();

            println!("\n--- Output ---");
            if !stdout.is_empty() {
                println!("{}", stdout);
            }
            if !stderr.is_empty() {
                eprintln!("--- Stderr ---\n{}", stderr);
            }

            println!("--- Result ---");
            println!("Exit code: {}", exit_code);
            println!("Duration: {:?}", duration);

            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
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

/// Show daemon status
pub fn show_status() -> Result<()> {
    // TODO: Implement IPC to query running daemon
    println!("Status check not implemented yet.");
    println!("Use 'ps' or task manager to check if flashcron is running.");
    Ok(())
}

/// Show upcoming schedule
pub fn show_schedule(config_path: PathBuf, count: usize) -> Result<()> {
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
pub async fn show_history(
    job_name: Option<String>,
    limit: usize,
    id: Option<String>,
) -> Result<()> {
    let client = reqwest::Client::new();
    let base_url = "http://127.0.0.1:8080";

    if let Some(exec_id) = id {
        let url = format!("{}/api/history/{}", base_url, exec_id);
        let resp = client.get(&url).send().await?;

        if resp.status().is_success() {
            let exec: flashcron::config::JobExecution = resp.json().await?;
            println!("Job: {}", exec.job_name);
            println!("ID: {}", exec.id);
            let status_str = match exec.status {
                flashcron::config::JobStatus::Success => "Success".to_string(),
                flashcron::config::JobStatus::Failed { ref error } => format!("Failed: {}", error),
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

            println!("\n--- STDOUT ---");
            match exec.stdout {
                Some(out) if !out.trim().is_empty() => println!("{}", out.trim()),
                _ => println!("(No output)"),
            }

            match exec.stderr {
                Some(err) if !err.trim().is_empty() => {
                    println!("\n--- STDERR / ERROR ---");
                    println!("{}", err.trim());
                }
                _ => {}
            }
        } else {
            println!("Execution ID {} not found.", exec_id);
        }
    } else {
        let mut url = format!("{}/api/history?limit={}", base_url, limit);
        if let Some(name) = job_name {
            url.push_str(&format!("&job_name={}", name));
        }

        let resp = client.get(&url).send().await?;
        if resp.status().is_success() {
            #[derive(serde::Deserialize)]
            struct HistoryResp {
                history: Vec<flashcron::config::JobExecution>,
            }
            let data: HistoryResp = resp.json().await?;

            if data.history.is_empty() {
                println!("No history found.");
                return Ok(());
            }

            println!(
                "{:<36} | {:<20} | {:<10} | {:<25}",
                "Execution ID", "Job Name", "Status", "Start Time"
            );
            println!("{:-<36}-+-{:-<20}-+-{:-<10}-+-{:-<25}", "", "", "", "");

            for exec in data.history {
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
        } else {
            println!("Failed to retrieve history: {}", resp.status());
        }
    }
    Ok(())
}
