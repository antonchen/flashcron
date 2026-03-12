//! FlashCron - A lightweight, efficient cron daemon
//!
//! Usage:
//!   flashcron run -c config.toml    # Start the daemon
//!   flashcron validate -c config.toml # Validate config
//!   flashcron list -c config.toml   # List jobs
//!   flashcron trigger <job> -c config.toml # Trigger a job
//!   flashcron init                  # Generate default config

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use flashcron::{Config, Scheduler};
use log::{error, info, LevelFilter};
use std::path::PathBuf;
use std::time::Duration;

/// FlashCron - A lightweight, efficient cron daemon
#[derive(Parser)]
#[command(name = "flashcron")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Configuration file path
    #[arg(short, long, default_value = "flashcron.toml", global = true)]
    config: PathBuf,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, global = true)]
    log_level: Option<String>,

    /// Output logs in JSON format
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the cron daemon
    Run {
        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,
    },

    /// Validate configuration file
    Validate,

    /// List all configured jobs
    List {
        /// Show only enabled jobs
        #[arg(short, long)]
        enabled: bool,

        /// Output format (table, json)
        #[arg(short, long, default_value = "table")]
        format: String,
    },

    /// Trigger a job immediately
    Trigger {
        /// Job name to trigger
        job_name: String,
    },

    /// Generate default configuration file
    Init {
        /// Output path for config file
        #[arg(short, long, default_value = "flashcron.toml")]
        output: PathBuf,

        /// Overwrite existing file
        #[arg(short = 'f', long)]
        force: bool,
    },

    /// Show daemon status (if running)
    Status,

    /// Show next scheduled run times
    Schedule {
        /// Number of upcoming runs to show
        #[arg(short = 'n', long, default_value = "10")]
        count: usize,
    },

    /// Show job execution history
    #[cfg(feature = "web")]
    History {
        /// Job name to filter by
        job_name: Option<String>,

        /// Limit number of records
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// Query specific execution by ID
        #[arg(long)]
        id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    init_logging(&cli)?;

    let result = match cli.command {
        Commands::Run { foreground: _ } => run_daemon(cli.config).await,
        Commands::Validate => validate_config(cli.config),
        Commands::List { enabled, format } => list_jobs(cli.config, enabled, &format),
        Commands::Trigger { job_name } => trigger_job(cli.config, &job_name).await,
        Commands::Init { output, force } => init_config(output, force),
        Commands::Status => show_status(),
        Commands::Schedule { count } => show_schedule(cli.config, count),
        #[cfg(feature = "web")]
        Commands::History {
            job_name,
            limit,
            id,
        } => show_history(job_name, limit, id).await,
    };

    if let Err(e) = result {
        error!("Error: {}", e);
        // Allow time for logs to be flushed
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::process::exit(1);
    }

    // Explicitly exit to ensure all background threads are terminated
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::process::exit(0);
}

/// Initialize logging using fern and log
fn init_logging(cli: &Cli) -> Result<()> {
    // Load config to get settings, fallback to default if not found
    let settings = if let Ok(config) = Config::from_file(&cli.config) {
        config.settings
    } else {
        flashcron::config::Settings::default()
    };

    let tz = settings.effective_timezone();

    // Priority: CLI > Config
    let log_level = cli.log_level.as_ref().unwrap_or(&settings.log_level);
    let use_json = cli.json || settings.json_logs;

    let level = match log_level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    };

    let mut base_config = fern::Dispatch::new()
        .level(level)
        .level_for("tokio_util", LevelFilter::Warn)
        .level_for("hyper", LevelFilter::Warn);

    if use_json {
        base_config = base_config.chain(
            fern::Dispatch::new()
                .format(move |out, message, record| {
                    let timestamp = chrono::Utc::now()
                        .with_timezone(&tz)
                        .format("%Y-%m-%d %H:%M:%S%.3f")
                        .to_string();

                    // Extract KV pairs from the record
                    let mut kv_map = serde_json::Map::new();

                    // A simple visitor to collect KVs
                    struct JsonVisitor<'a>(&'a mut serde_json::Map<String, serde_json::Value>);
                    impl<'kvs> log::kv::Visitor<'kvs> for JsonVisitor<'_> {
                        fn visit_pair(
                            &mut self,
                            key: log::kv::Key<'kvs>,
                            value: log::kv::Value<'kvs>,
                        ) -> std::result::Result<(), log::kv::Error> {
                            let key_str = key.to_string();
                            let final_key = if key_str == "job_name" {
                                "job"
                            } else {
                                &key_str
                            };
                            self.0.insert(
                                final_key.to_string(),
                                serde_json::Value::String(value.to_string()),
                            );
                            Ok(())
                        }
                    }
                    let _ = record.key_values().visit(&mut JsonVisitor(&mut kv_map));

                    // Construct the final JSON object with timestamp FIRST
                    let mut json_obj = serde_json::Map::new();
                    json_obj.insert(
                        "timestamp".to_string(),
                        serde_json::Value::String(timestamp),
                    );
                    json_obj.insert(
                        "level".to_string(),
                        serde_json::Value::String(record.level().to_string()),
                    );

                    let msg_str = message.to_string();

                    if kv_map.is_empty() {
                        // Standard message, no fields nesting
                        json_obj.insert("message".to_string(), serde_json::Value::String(msg_str));
                    } else {
                        // For KV-rich logs, message is an object
                        let mut message_content = serde_json::Map::new();
                        for (k, v) in kv_map {
                            message_content.insert(k, v);
                        }
                        if !msg_str.is_empty() {
                            message_content
                                .insert("message".to_string(), serde_json::Value::String(msg_str));
                        }
                        json_obj.insert(
                            "message".to_string(),
                            serde_json::Value::Object(message_content),
                        );
                    }

                    out.finish(format_args!("{}", serde_json::Value::Object(json_obj)));
                })
                .chain(std::io::stdout()),
        );
    } else {
        base_config = base_config.chain(
            fern::Dispatch::new()
                .format(move |out, message, record| {
                    let timestamp = chrono::Utc::now()
                        .with_timezone(&tz)
                        .format("%Y-%m-%d %H:%M:%S%.3f")
                        .to_string();

                    // Extract KV pairs
                    let mut kvs = Vec::new();
                    struct TextVisitor<'a>(&'a mut Vec<(String, String)>);
                    impl<'kvs> log::kv::Visitor<'kvs> for TextVisitor<'_> {
                        fn visit_pair(
                            &mut self,
                            key: log::kv::Key<'kvs>,
                            value: log::kv::Value<'kvs>,
                        ) -> std::result::Result<(), log::kv::Error> {
                            self.0.push((key.to_string(), value.to_string()));
                            Ok(())
                        }
                    }
                    let _ = record.key_values().visit(&mut TextVisitor(&mut kvs));

                    let mut kv_str = String::new();
                    let mut job_name = None;
                    let mut output = None;
                    let mut status = None;

                    for (k, v) in kvs {
                        if k == "job_name" || k == "job" {
                            job_name = Some(v);
                        } else if k == "output" {
                            output = Some(v);
                        } else if k == "status" {
                            status = Some(v);
                        } else {
                            if !kv_str.is_empty() {
                                kv_str.push(' ');
                            }
                            kv_str.push_str(&format!("{}={}", k, v));
                        }
                    }

                    let msg_str = message.to_string();

                    // Special formatting for job output/status as requested
                    let final_msg = if let Some(job) = job_name {
                        if let Some(out_val) = output {
                            format!("job={} output: {}", job, out_val)
                        } else if let Some(stat_val) = status {
                            format!("job={} status: {}", job, stat_val)
                        } else {
                            if !msg_str.is_empty() {
                                format!("{} job={}", msg_str, job)
                            } else {
                                format!("job={}", job)
                            }
                        }
                    } else {
                        msg_str
                    };

                    let mut final_with_kv = final_msg;
                    if !kv_str.is_empty() {
                        if !final_with_kv.is_empty() {
                            final_with_kv.push(' ');
                        }
                        final_with_kv.push_str(&kv_str);
                    }

                    out.finish(format_args!(
                        "{}  {:<5} {}",
                        timestamp,
                        record.level(),
                        final_with_kv
                    ))
                })
                .chain(std::io::stdout()),
        );
    }

    base_config
        .apply()
        .map_err(|e| anyhow::anyhow!("Failed to initialize logging: {}", e))?;

    Ok(())
}

/// Run the daemon
async fn run_daemon(config_path: PathBuf) -> Result<()> {
    info!("Starting FlashCron v{}", flashcron::VERSION);

    // Load configuration
    let config = Config::from_file(&config_path)
        .with_context(|| format!("Failed to load config from {:?}", config_path))?;

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

    // Run scheduler and wait for shutdown signal concurrently
    let scheduler_handle = handle.clone();
    tokio::select! {
        res = scheduler.run() => {
            if let Err(e) = res {
                error!("Scheduler error: {}", e);
            }
        }
        sig_res = wait_for_shutdown_signal() => {
            if let Err(e) = sig_res {
                error!("Signal handler error: {}", e);
            }
            info!("Shutting down gracefully...");
            let _ = scheduler_handle.shutdown().await;

            // Give the scheduler a moment to exit the loop gracefully
            // If it doesn't exit quickly, the select! will end and we'll abort tasks anyway
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // Abort background tasks
    watcher_task.abort();

    #[cfg(feature = "web")]
    api_task.abort();

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
fn validate_config(config_path: PathBuf) -> Result<()> {
    println!("Validating configuration: {:?}", config_path);

    match Config::from_file(&config_path) {
        Ok(config) => {
            println!("✓ Configuration is valid");
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
fn list_jobs(config_path: PathBuf, enabled_only: bool, format: &str) -> Result<()> {
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
async fn trigger_job(config_path: PathBuf, job_name: &str) -> Result<()> {
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
fn init_config(output: PathBuf, force: bool) -> Result<()> {
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
fn show_status() -> Result<()> {
    // TODO: Implement IPC to query running daemon
    println!("Status check not implemented yet.");
    println!("Use 'ps' or task manager to check if flashcron is running.");
    Ok(())
}

/// Show upcoming schedule
fn show_schedule(config_path: PathBuf, count: usize) -> Result<()> {
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
async fn show_history(job_name: Option<String>, limit: usize, id: Option<String>) -> Result<()> {
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
