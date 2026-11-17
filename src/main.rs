//! FlashCron - A lightweight, efficient cron daemon
//!
//! Usage:
//!   flashcron run -c config.toml    # Start in foreground
//!   flashcron run -c config.toml --daemon # Start in background
//!   flashcron validate -c config.toml # Validate config
//!   flashcron list -c config.toml   # List jobs
//!   flashcron trigger <job>         # Trigger a job via IPC
//!   flashcron init                  # Generate default config

mod cmd;

use anyhow::Result;
use clap::Parser;
use log::error;
use std::time::Duration;

use crate::cmd::args::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let default_config_path = cmd::state::resolve_config_path(cli.config.clone());
    let explicit_config_path = cmd::state::resolve_explicit_config_path(cli.config.clone());

    // Initialize logging
    cmd::logging::init_logging(&cli, &default_config_path)?;

    let log_level = cli.log_level.clone();
    let json = cli.json;
    let command = cli.command;

    let result = match command {
        Commands::Run { daemon } => {
            cmd::commands::run_daemon(default_config_path, daemon, log_level, json).await
        }
        Commands::Validate => cmd::commands::validate_config(default_config_path),
        Commands::List { enabled, format } => {
            cmd::commands::list_jobs(default_config_path, enabled, &format).await
        }
        Commands::Trigger { job_name } => {
            cmd::commands::trigger_job(explicit_config_path, &job_name).await
        }
        Commands::Stop => cmd::commands::stop_daemon(explicit_config_path).await,
        Commands::Init { output, force } => cmd::commands::init_config(output, force),
        Commands::Status => cmd::commands::show_status(explicit_config_path).await,
        Commands::Schedule { count } => {
            cmd::commands::show_schedule(default_config_path, count).await
        }
        Commands::History {
            job_name,
            limit,
            id,
        } => cmd::commands::show_history(explicit_config_path, job_name, limit, id).await,
    };

    if let Err(e) = result {
        error!(status = "error", error = &*e.to_string(); "");
        // Allow time for logs to be flushed
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::process::exit(1);
    }

    // Explicitly exit to ensure all background threads are terminated
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::process::exit(0);
}
