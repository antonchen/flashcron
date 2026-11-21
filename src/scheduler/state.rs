//! Scheduler state management

use crate::config::JobStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobRunState {
    Idle,
    Queued,
    Starting,
    Running,
}

/// Information about a scheduled job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// Job name
    pub name: String,
    /// Whether the job is enabled
    pub enabled: bool,
    /// Cron schedule expression
    pub schedule: String,
    /// Next scheduled run time
    pub next_run: Option<DateTime<Utc>>,
    /// Last run time
    pub last_run: Option<DateTime<Utc>>,
    /// Last run status
    pub last_status: Option<JobStatus>,
    /// Current run state
    pub run_state: JobRunState,
    /// Current or most recent execution ID
    #[serde(default)]
    pub current_execution_id: Option<Uuid>,
    /// Monotonic sequence of live output snapshots for the current execution
    #[serde(default)]
    pub output_sequence: u64,
    /// Bounded output preview for the current or most recent execution
    #[serde(skip)]
    pub live_output: String,
}

impl ScheduledJob {
    /// Create a new scheduled job info
    pub fn new(name: String, schedule: String, enabled: bool) -> Self {
        Self {
            name,
            enabled,
            schedule,
            next_run: None,
            last_run: None,
            last_status: None,
            run_state: JobRunState::Idle,
            current_execution_id: None,
            output_sequence: 0,
            live_output: String::new(),
        }
    }

    /// Update with next run time
    pub fn with_next_run(mut self, next: Option<DateTime<Utc>>) -> Self {
        self.next_run = next;
        self
    }

    pub fn is_in_flight(&self) -> bool {
        !matches!(self.run_state, JobRunState::Idle)
    }

    pub fn mark_queued(&mut self, execution_id: Uuid) {
        self.run_state = JobRunState::Queued;
        self.current_execution_id = Some(execution_id);
        self.output_sequence = 0;
        self.live_output.clear();
    }

    pub fn mark_starting(&mut self) {
        self.run_state = JobRunState::Starting;
        self.last_run = Some(Utc::now());
    }

    /// Mark as started
    pub fn mark_started(&mut self, execution_id: Uuid) {
        self.run_state = JobRunState::Running;
        self.last_run = Some(Utc::now());
        self.current_execution_id = Some(execution_id);
        self.output_sequence = 0;
        self.live_output.clear();
    }

    /// Mark as completed
    pub fn mark_completed(&mut self, status: JobStatus, next_run: Option<DateTime<Utc>>) {
        self.run_state = JobRunState::Idle;
        self.next_run = next_run;
        self.last_status = Some(status);
    }

    pub fn mark_skipped(&mut self) {
        self.last_status = Some(JobStatus::Pending);
    }
}

/// Overall scheduler state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerState {
    /// Scheduler start time
    pub started_at: DateTime<Utc>,
    /// Current time (for reference)
    pub current_time: DateTime<Utc>,
    /// Uptime in seconds
    pub uptime_seconds: i64,
    /// Whether the scheduler is running
    pub is_running: bool,
    /// Number of jobs configured
    pub total_jobs: usize,
    /// Number of enabled jobs
    pub enabled_jobs: usize,
    /// Number of jobs holding an execution slot (Starting or Running)
    pub running_jobs: usize,
    /// Number of jobs waiting in the global queue
    pub queue_len: usize,
    /// Total executions since start
    pub total_executions: u64,
    /// Total failures since start
    pub total_failures: u64,
    /// Job information
    pub jobs: HashMap<String, ScheduledJob>,
}

impl SchedulerState {
    /// Create a new scheduler state
    pub fn new() -> Self {
        Self {
            started_at: Utc::now(),
            current_time: Utc::now(),
            uptime_seconds: 0,
            is_running: true,
            total_jobs: 0,
            enabled_jobs: 0,
            running_jobs: 0,
            queue_len: 0,
            total_executions: 0,
            total_failures: 0,
            jobs: HashMap::new(),
        }
    }

    /// Update timing information
    pub fn update_time(&mut self) {
        self.current_time = Utc::now();
        self.uptime_seconds = (self.current_time - self.started_at).num_seconds();
    }

    /// Add a job to the state
    pub fn add_job(&mut self, job: ScheduledJob) {
        self.total_jobs += 1;
        if job.enabled {
            self.enabled_jobs += 1;
        }
        self.jobs.insert(job.name.clone(), job);
    }

    /// Get a job by name
    pub fn get_job(&self, name: &str) -> Option<&ScheduledJob> {
        self.jobs.get(name)
    }

    /// Get a mutable reference to a job
    pub fn get_job_mut(&mut self, name: &str) -> Option<&mut ScheduledJob> {
        self.jobs.get_mut(name)
    }

    pub fn can_accept_trigger(&self, job_name: &str) -> bool {
        self.jobs
            .get(job_name)
            .map(|job| !job.is_in_flight())
            .unwrap_or(false)
    }

    pub fn record_job_queued(&mut self, job_name: &str, execution_id: Uuid) {
        if let Some(job) = self.jobs.get_mut(job_name) {
            job.mark_queued(execution_id);
            self.queue_len += 1;
        }
    }

    pub fn record_job_starting(&mut self, job_name: &str) {
        if let Some(job) = self.jobs.get_mut(job_name) {
            if matches!(job.run_state, JobRunState::Queued) && self.queue_len > 0 {
                self.queue_len -= 1;
            }
            job.mark_starting();
            self.running_jobs += 1;
        }
    }

    /// Record job start
    pub fn record_job_start(&mut self, job_name: &str, execution_id: Uuid) {
        if let Some(job) = self.jobs.get_mut(job_name) {
            if job.current_execution_id == Some(execution_id) {
                job.mark_started(execution_id);
            }
        }
    }

    pub fn update_live_output(
        &mut self,
        job_name: &str,
        execution_id: Uuid,
        output: String,
    ) -> Option<u64> {
        if let Some(job) = self.jobs.get_mut(job_name) {
            if job.current_execution_id == Some(execution_id) {
                job.live_output = output;
                job.output_sequence += 1;
                return Some(job.output_sequence);
            }
        }
        None
    }

    /// Record job completion
    pub fn record_job_completion(
        &mut self,
        job_name: &str,
        execution_id: Uuid,
        status: JobStatus,
        next_run: Option<DateTime<Utc>>,
    ) {
        let was_failure = matches!(status, JobStatus::Failed { .. } | JobStatus::Timeout);
        if let Some(job) = self.jobs.get_mut(job_name) {
            if job.current_execution_id == Some(execution_id) {
                job.mark_completed(status, next_run);
            }
        }

        self.running_jobs -= 1;
        self.total_executions += 1;
        if was_failure {
            self.total_failures += 1;
        }
    }

    pub fn record_job_skipped(&mut self, job_name: &str) {
        if let Some(job) = self.jobs.get_mut(job_name) {
            job.mark_skipped();
        }
    }
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduled_job() {
        let mut job = ScheduledJob::new("test".to_string(), "* * * * *".to_string(), true);

        assert_eq!(job.run_state, JobRunState::Idle);

        let exec_id = Uuid::new_v4();
        job.mark_starting();
        job.mark_started(exec_id);
        assert_eq!(job.run_state, JobRunState::Running);

        job.mark_completed(JobStatus::Success, None);
        assert_eq!(job.run_state, JobRunState::Idle);
    }

    #[test]
    fn test_scheduler_state() {
        let mut state = SchedulerState::new();

        let job = ScheduledJob::new("test".to_string(), "* * * * *".to_string(), true);
        state.add_job(job);

        assert_eq!(state.total_jobs, 1);
        assert_eq!(state.enabled_jobs, 1);
        assert!(state.get_job("test").is_some());
    }
}
