//! Scheduler state management

use crate::config::{JobExecution, JobStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(feature = "web")]
use std::collections::VecDeque;
use uuid::Uuid;

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
    /// Number of times this job has run
    pub run_count: u64,
    /// Number of failures
    pub failure_count: u64,
    /// Whether the job is currently running
    pub is_running: bool,
    /// Current execution ID (if running)
    pub current_execution_id: Option<Uuid>,
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
            run_count: 0,
            failure_count: 0,
            is_running: false,
            current_execution_id: None,
        }
    }

    /// Update with next run time
    pub fn with_next_run(mut self, next: Option<DateTime<Utc>>) -> Self {
        self.next_run = next;
        self
    }

    /// Mark as started
    pub fn mark_started(&mut self, execution_id: Uuid) {
        self.is_running = true;
        self.current_execution_id = Some(execution_id);
        self.last_run = Some(Utc::now());
    }

    /// Mark as completed
    pub fn mark_completed(&mut self, status: JobStatus, next_run: Option<DateTime<Utc>>) {
        self.is_running = false;
        self.current_execution_id = None;
        self.run_count += 1;
        self.next_run = next_run;

        if matches!(status, JobStatus::Failed { .. } | JobStatus::Timeout) {
            self.failure_count += 1;
        }

        self.last_status = Some(status);
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
    /// Number of currently running jobs
    pub running_jobs: usize,
    /// Total executions since start
    pub total_executions: u64,
    /// Total failures since start
    pub total_failures: u64,
    /// Job information
    pub jobs: HashMap<String, ScheduledJob>,
    /// Recent execution history
    #[cfg(feature = "web")]
    pub recent_history: VecDeque<JobExecution>,
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
            total_executions: 0,
            total_failures: 0,
            jobs: HashMap::new(),
            #[cfg(feature = "web")]
            recent_history: VecDeque::new(),
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

    /// Record job start
    pub fn record_job_start(&mut self, job_name: &str, execution_id: Uuid) {
        if let Some(job) = self.jobs.get_mut(job_name) {
            job.mark_started(execution_id);
            self.running_jobs += 1;
        }
    }

    /// Record job completion
    pub fn record_job_completion(
        &mut self,
        job_name: &str,
        status: JobStatus,
        #[allow(unused_variables)] execution: JobExecution,
        next_run: Option<DateTime<Utc>>,
        #[cfg(feature = "web")] job_history_size: usize,
        #[cfg(feature = "web")] max_history_size: usize,
    ) {
        if let Some(job) = self.jobs.get_mut(job_name) {
            let was_failure = matches!(status, JobStatus::Failed { .. } | JobStatus::Timeout);
            job.mark_completed(status, next_run);

            if self.running_jobs > 0 {
                self.running_jobs -= 1;
            }

            self.total_executions += 1;
            if was_failure {
                self.total_failures += 1;
            }
        }

        // Add to history
        #[cfg(feature = "web")]
        {
            self.recent_history.push_front(execution);

            // Apply job_history_size limit
            let mut job_count = 0;
            let mut i = 0;
            while i < self.recent_history.len() {
                if self.recent_history[i].job_name == job_name {
                    job_count += 1;
                    if job_count > job_history_size {
                        self.recent_history.remove(i);
                        continue;
                    }
                }
                i += 1;
            }

            // Apply max_history_size limit
            while self.recent_history.len() > max_history_size {
                self.recent_history.pop_back();
            }
        }
    }

    /// Get success rate as percentage
    pub fn success_rate(&self) -> f64 {
        if self.total_executions == 0 {
            100.0
        } else {
            let successes = self.total_executions - self.total_failures;
            (successes as f64 / self.total_executions as f64) * 100.0
        }
    }

    /// Get jobs that are due to run
    pub fn due_jobs(&self) -> Vec<&ScheduledJob> {
        let now = Utc::now();
        self.jobs
            .values()
            .filter(|j| j.enabled && !j.is_running && j.next_run.map(|t| t <= now).unwrap_or(false))
            .collect()
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

        assert!(!job.is_running);
        assert_eq!(job.run_count, 0);

        let exec_id = Uuid::new_v4();
        job.mark_started(exec_id);
        assert!(job.is_running);
        assert_eq!(job.current_execution_id, Some(exec_id));

        job.mark_completed(JobStatus::Success, None);
        assert!(!job.is_running);
        assert_eq!(job.run_count, 1);
        assert_eq!(job.failure_count, 0);
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

    #[test]
    fn test_success_rate() {
        let mut state = SchedulerState::new();
        assert_eq!(state.success_rate(), 100.0);

        state.total_executions = 10;
        state.total_failures = 2;
        assert!((state.success_rate() - 80.0).abs() < 0.01);
    }
}
