//! Scheduler module - the core scheduling engine

mod engine;
mod state;

pub use engine::{Scheduler, SchedulerHandle};
pub use state::{JobRunState, ScheduledJob, SchedulerState};

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Message types for scheduler communication
#[derive(Debug)]
pub enum SchedulerMessage {
    /// Trigger a job immediately
    TriggerJob {
        job_name: String,
        response_tx: tokio::sync::oneshot::Sender<crate::error::Result<Uuid>>,
    },
    /// Add or replace one job definition immediately
    AddOrUpdateJob {
        job_name: String,
        job: Box<crate::config::Job>,
        update: bool,
        rename_from: Option<String>,
        response_tx: tokio::sync::oneshot::Sender<crate::error::Result<bool>>,
    },
    /// Remove a job definition immediately
    RemoveJob {
        job_name: String,
        response_tx: tokio::sync::oneshot::Sender<crate::error::Result<()>>,
    },
    #[cfg(feature = "web")]
    SetFailureHook {
        path: Option<std::path::PathBuf>,
        response_tx: tokio::sync::oneshot::Sender<crate::error::Result<()>>,
    },
    /// Reload configuration
    ReloadConfig,
    /// Get scheduler status
    GetStatus {
        response_tx: tokio::sync::oneshot::Sender<SchedulerState>,
    },
    /// Internal signal that a job completed and the queue can advance
    JobCompleted,
    /// Shutdown the scheduler
    Shutdown,
}

/// Event emitted by the scheduler
#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    /// Job is about to start
    JobStarting {
        job_name: String,
        execution_id: Uuid,
    },
    /// Job completed
    JobCompleted {
        job_name: String,
        execution_id: Uuid,
        success: bool,
        duration: u64,
    },
    /// Job failed
    JobFailed {
        job_name: String,
        execution_id: Uuid,
        error: String,
    },
    /// Updated bounded output preview for a running job
    JobOutput {
        job_name: String,
        execution_id: Uuid,
        sequence: u64,
        output: String,
    },
    /// Configuration reloaded
    ConfigReloaded { job_count: usize },
    /// Scheduler started
    Started,
    /// Scheduler stopped
    Stopped,
}

/// Represents a scheduled job trigger
#[derive(Debug, Clone)]
pub struct JobTrigger {
    /// Job name
    pub job_name: String,
    /// Scheduled time
    pub scheduled_at: DateTime<Utc>,
}

impl JobTrigger {
    /// Create a new job trigger
    pub fn new(job_name: String, scheduled_at: DateTime<Utc>) -> Self {
        Self {
            job_name,
            scheduled_at,
        }
    }

    /// Get milliseconds until this trigger is due
    pub fn ms_until_due(&self) -> i64 {
        let now = Utc::now();
        (self.scheduled_at - now).num_milliseconds().max(0)
    }
}
