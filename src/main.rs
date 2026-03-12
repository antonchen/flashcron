//! FlashCron - A lightweight, efficient cron daemon
//!
//! Usage:
//!   flashcron run -c config.toml    # Start the daemon
//!   flashcron validate -c config.toml # Validate config
//!   flashcron list -c config.toml   # List jobs
//!   flashcron trigger <job> -c config.toml # Trigger a job
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

    // Initialize logging
    cmd::logging::init_logging(&cli)?;

    let result = match cli.command {
        Commands::Run { foreground: _ } => cmd::commands::run_daemon(cli.config).await,
        Commands::Validate => cmd::commands::validate_config(cli.config),
        Commands::List { enabled, format } => {
            cmd::commands::list_jobs(cli.config, enabled, &format)
        }
        Commands::Trigger { job_name } => cmd::commands::trigger_job(cli.config, &job_name).await,
        Commands::Init { output, force } => cmd::commands::init_config(output, force),
        Commands::Status => cmd::commands::show_status(),
        Commands::Schedule { count } => cmd::commands::show_schedule(cli.config, count),
        #[cfg(feature = "web")]
        Commands::History {
            job_name,
            limit,
            id,
        } => cmd::commands::show_history(job_name, limit, id).await,
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
