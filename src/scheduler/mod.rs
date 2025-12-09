//! Scheduler module - the core scheduling engine

mod engine;
mod state;

pub use engine::{Scheduler, SchedulerHandle};
pub use state::{ScheduledJob, SchedulerState};

use crate::config::Job;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Message types for scheduler communication
#[derive(Debug)]
pub enum SchedulerMessage {
    /// Trigger a job immediately
    TriggerJob { job_name: String },
    /// Reload configuration
    ReloadConfig,
    /// Get scheduler status
    GetStatus {
        response_tx: tokio::sync::oneshot::Sender<SchedulerState>,
    },
    /// Stop a running job
    StopJob { job_name: String },
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
        duration_ms: u64,
    },
    /// Job failed
    JobFailed {
        job_name: String,
        execution_id: Uuid,
        error: String,
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
    /// The job configuration
    pub job: Job,
}

impl JobTrigger {
    /// Create a new job trigger
    pub fn new(job_name: String, scheduled_at: DateTime<Utc>, job: Job) -> Self {
        Self {
            job_name,
            scheduled_at,
            job,
        }
    }

    /// Check if this trigger is due
    pub fn is_due(&self) -> bool {
        Utc::now() >= self.scheduled_at
    }

    /// Get milliseconds until this trigger is due
    pub fn ms_until_due(&self) -> i64 {
        let now = Utc::now();
        (self.scheduled_at - now).num_milliseconds().max(0)
    }
}
