use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// FlashCron - A lightweight, efficient cron daemon
#[derive(Parser)]
#[command(name = "flashcron")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Configuration file path
    #[arg(short, long, env = "FLASHCRON_CONFIG", global = true)]
    pub config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, global = true)]
    pub log_level: Option<String>,

    /// Output logs in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
