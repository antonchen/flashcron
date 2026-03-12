//! # FlashCron
//!
//! A lightweight, efficient, and eco-friendly cron daemon written in Rust.
//!
//! ## Features
//!
//! - **Efficient**: Minimal memory footprint (~2-5MB), fast startup
//! - **Simple**: TOML configuration, intuitive CLI
//! - **Observable**: Built-in logging and Web API/Dashboard
//! - **Reliable**: Graceful shutdown, job timeout handling, automatic retry
//! - **Cross-platform**: Linux, macOS, Windows support

#[cfg(feature = "web")]
pub mod api;
pub mod config;
pub mod error;
pub mod executor;
pub mod scheduler;

pub use config::Config;
pub use error::{Error, Result};
pub use executor::JobExecutor;
pub use scheduler::Scheduler;

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
