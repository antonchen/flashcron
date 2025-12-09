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
use std::path::PathBuf;
use tracing::{error, info, Level};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    init_logging(&cli)?;

    match cli.command {
        Commands::Run { foreground: _ } => run_daemon(cli.config).await,
        Commands::Validate => validate_config(cli.config),
        Commands::List { enabled, format } => list_jobs(cli.config, enabled, &format),
        Commands::Trigger { job_name } => trigger_job(cli.config, &job_name).await,
        Commands::Init { output, force } => init_config(output, force),
        Commands::Status => show_status(),
        Commands::Schedule { count } => show_schedule(cli.config, count),
    }
}

/// Initialize logging
fn init_logging(cli: &Cli) -> Result<()> {
    let level = cli
        .log_level
        .as_deref()
        .unwrap_or("info")
        .parse::<Level>()
        .unwrap_or(Level::INFO);

    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .with_target(false);

    if cli.json {
        subscriber.json().init();
    } else {
        subscriber.init();
    }

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
    let (scheduler, handle) = Scheduler::new(config, config_path.clone());

    // Setup signal handlers
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        if let Err(e) = wait_for_shutdown_signal().await {
            error!("Signal handler error: {}", e);
        }
        info!("Shutdown signal received");
        let _ = shutdown_handle.shutdown().await;
    });

    // Setup config file watcher
    let reload_handle = handle.clone();
    let watch_path = config_path.clone();
    tokio::spawn(async move {
        if let Err(e) = watch_config_file(watch_path, reload_handle).await {
            error!("Config watcher error: {}", e);
        }
    });

    // Run scheduler
    scheduler.run().await?;

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
            _ = sigterm.recv() => info!("Received SIGTERM"),
            _ = sigint.recv() => info!("Received SIGINT"),
            _ = sighup.recv() => info!("Received SIGHUP"),
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
        match rx.recv() {
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
            Err(e) => {
                error!("Channel error: {:?}", e);
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
                        "next_run": job.next_run().map(|t| t.to_rfc3339()),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        _ => {
            println!(
                "{:<20} {:<20} {:<10} NEXT RUN",
                "NAME", "SCHEDULE", "STATUS"
            );
            println!("{}", "-".repeat(75));

            for (name, job) in jobs {
                let status = if job.enabled { "enabled" } else { "disabled" };
                let next_run = job
                    .next_run()
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

    println!("Next {} scheduled runs:", count);
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
                        .upcoming(chrono::Utc)
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
        println!("{:<25} {}", time.format("%Y-%m-%d %H:%M:%S UTC"), name);
    }

    Ok(())
}
