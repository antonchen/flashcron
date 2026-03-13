//! Main scheduler engine

use crate::config::{Config, Job, JobExecution, JobStatus};
use crate::error::{Error, Result};
use crate::executor::JobExecutor;
use crate::scheduler::state::{ScheduledJob, SchedulerState};
use crate::scheduler::{JobTrigger, SchedulerEvent, SchedulerMessage};
use chrono::Utc;
use log::{debug, error, info, warn};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::time::{self, Duration, Instant};

/// Wrapper for job triggers in the priority queue (min-heap by time)
#[derive(Debug, Clone)]
struct TimedTrigger {
    trigger: JobTrigger,
}

impl PartialEq for TimedTrigger {
    fn eq(&self, other: &Self) -> bool {
        self.trigger.scheduled_at == other.trigger.scheduled_at
    }
}

impl Eq for TimedTrigger {}

impl PartialOrd for TimedTrigger {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimedTrigger {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior
        other.trigger.scheduled_at.cmp(&self.trigger.scheduled_at)
    }
}

/// The main scheduler
pub struct Scheduler {
    /// Configuration
    config: Arc<RwLock<Config>>,
    /// Config file path for reloading
    config_path: PathBuf,
    /// Scheduler state
    pub(crate) state: Arc<RwLock<SchedulerState>>,
    /// Job executor
    executor: Arc<JobExecutor>,
    /// Message receiver
    message_rx: mpsc::Receiver<SchedulerMessage>,
    /// Message sender (kept for potential future use in reload scenarios)
    #[allow(dead_code)]
    message_tx: mpsc::Sender<SchedulerMessage>,
    /// Event broadcaster
    event_tx: broadcast::Sender<SchedulerEvent>,
    /// Priority queue of upcoming triggers
    trigger_queue: BinaryHeap<TimedTrigger>,
    /// Semaphore for limiting concurrent jobs
    job_semaphore: Arc<tokio::sync::Semaphore>,
    /// Shutdown flag
    shutdown: bool,
}

impl Scheduler {
    /// Create a new scheduler
    pub fn new(config: Config, config_path: PathBuf) -> (Self, SchedulerHandle) {
        let (message_tx, message_rx) = mpsc::channel(100);
        let (event_tx, _) = broadcast::channel(100);

        let max_concurrent = config.settings.max_concurrent_jobs;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(if max_concurrent == 0 {
            usize::MAX
        } else {
            max_concurrent
        }));

        let executor = Arc::new(JobExecutor::new(
            config.settings.shell.clone(),
            config.settings.shell_args.clone(),
        ));

        let scheduler = Self {
            config: Arc::new(RwLock::new(config)),
            config_path,
            state: Arc::new(RwLock::new(SchedulerState::new())),
            executor,
            message_rx,
            message_tx: message_tx.clone(),
            event_tx: event_tx.clone(),
            trigger_queue: BinaryHeap::new(),
            job_semaphore: semaphore,
            shutdown: false,
        };

        let handle = SchedulerHandle {
            message_tx,
            event_tx,
        };

        (scheduler, handle)
    }

    /// Initialize the scheduler state from config
    async fn initialize(&mut self) -> Result<()> {
        let config_lock = self.config.read().await;
        let tz = config_lock.settings.effective_timezone();
        let mut state = self.state.write().await;

        for (name, job) in &config_lock.jobs {
            let next_run = if job.enabled {
                job.next_run(tz)
                    .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc))
            } else {
                None
            };
            let scheduled_job = ScheduledJob::new(name.clone(), job.schedule.clone(), job.enabled)
                .with_next_run(next_run);

            state.add_job(scheduled_job);

            // Add to trigger queue only if enabled
            if job.enabled {
                if let Some(next) = next_run {
                    self.trigger_queue.push(TimedTrigger {
                        trigger: JobTrigger::new(name.clone(), next, job.clone()),
                    });
                }
            }
        }

        debug!(status = "scheduler initialized"; "");

        Ok(())
    }

    /// Run jobs that are configured to run on startup
    async fn run_startup_jobs(&mut self) -> Result<()> {
        let config = self.config.read().await;
        let startup_jobs: Vec<_> = config
            .jobs
            .iter()
            .filter(|(_, job)| job.enabled && job.run_on_startup)
            .map(|(name, _)| name.clone())
            .collect();
        drop(config);

        for job_name in startup_jobs {
            self.trigger_job_with_source(&job_name, "startup").await?;
        }

        Ok(())
    }

    /// Get the scheduler state reference
    pub fn get_state(&self) -> Arc<RwLock<SchedulerState>> {
        self.state.clone()
    }

    /// Get the configuration reference
    pub fn get_config(&self) -> Arc<RwLock<Config>> {
        self.config.clone()
    }

    /// Main scheduler loop
    pub async fn run(mut self) -> Result<()> {
        self.initialize().await?;

        // Emit started event
        let _ = self.event_tx.send(SchedulerEvent::Started);

        // Run startup jobs
        self.run_startup_jobs().await?;

        debug!(status = "scheduler started"; "");

        loop {
            // Calculate sleep duration until next trigger
            let sleep_duration = self.calculate_sleep_duration();

            tokio::select! {
                // Wait for next trigger or message
                _ = time::sleep(sleep_duration) => {
                    self.process_due_triggers().await?;
                }

                // Handle incoming messages
                Some(msg) = self.message_rx.recv() => {
                    if self.handle_message(msg).await? {
                        break;
                    }
                }
            }

            if self.shutdown {
                break;
            }
        }

        let shutdown_timeout = self.config.read().await.settings.shutdown_timeout;

        debug!(status = "scheduler stopping"; "");

        // Wait for running jobs to finish, up to the timeout
        let state = self.state.clone();
        let timeout_duration = Duration::from_secs(shutdown_timeout);

        let wait_result = tokio::time::timeout(timeout_duration, async move {
            loop {
                let count = state.read().await.running_jobs;
                if count == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        if wait_result.is_err() {
            log::warn!(
                status = "shutdown timedout",
                timeout = &*format!("{}s", shutdown_timeout);
                ""
            );
        } else {
            debug!(status = "all jobs finished"; "");
        }

        debug!(status = "scheduler stopped"; "");
        let _ = self.event_tx.send(SchedulerEvent::Stopped);

        Ok(())
    }

    /// Calculate how long to sleep until the next trigger
    fn calculate_sleep_duration(&self) -> Duration {
        if let Some(next) = self.trigger_queue.peek() {
            let ms = next.trigger.ms_until_due();
            if ms <= 0 {
                Duration::from_millis(0)
            } else {
                Duration::from_millis(ms as u64)
            }
        } else {
            // No jobs scheduled, sleep for a minute and check again
            Duration::from_secs(60)
        }
    }

    /// Process all due triggers
    async fn process_due_triggers(&mut self) -> Result<()> {
        let now = Utc::now();

        while let Some(timed) = self.trigger_queue.peek() {
            if timed.trigger.scheduled_at > now {
                break;
            }

            let timed = self.trigger_queue.pop().unwrap();
            self.execute_trigger(timed.trigger).await?;
        }

        Ok(())
    }

    /// Execute a job trigger
    async fn execute_trigger(&mut self, trigger: JobTrigger) -> Result<()> {
        let job_name = trigger.job_name.clone();
        let job = trigger.job.clone();

        // Schedule next occurrence
        let tz = self.config.read().await.settings.effective_timezone();
        if let Some(next) = job
            .next_run(tz)
            .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc))
        {
            // Make sure we don't schedule the same time again
            if next > trigger.scheduled_at {
                self.trigger_queue.push(TimedTrigger {
                    trigger: JobTrigger::new(job_name.clone(), next, job.clone()),
                });

                // Update state with next run time
                let mut state = self.state.write().await;
                if let Some(sj) = state.get_job_mut(&job_name) {
                    sj.next_run = Some(next);
                }
            }
        }

        // Check if already running
        {
            let state = self.state.read().await;
            if let Some(sj) = state.get_job(&job_name) {
                if sj.is_running {
                    debug!(job_name = &*job_name, status = "skipped running"; "");
                    return Ok(());
                }
            }
        }

        // Spawn job execution (Trigger: cron)
        self.spawn_job_execution(job_name, job, "cron")
            .await
            .map(|_| ())
    }

    /// Spawn a job execution task
    async fn spawn_job_execution(
        &self,
        job_name: String,
        job: Job,
        trigger_source: &'static str,
    ) -> crate::error::Result<uuid::Uuid> {
        let executor = Arc::clone(&self.executor);
        let state = Arc::clone(&self.state);
        let event_tx = self.event_tx.clone();
        let semaphore = Arc::clone(&self.job_semaphore);

        let config_lock = self.config.read().await;
        let config = config_lock.clone();
        #[cfg(feature = "web")]
        let job_history_size = config.settings.job_history_size;
        #[cfg(feature = "web")]
        let max_history_size = config.settings.max_history_size;
        let print_output = job.print_output.unwrap_or(config.settings.print_output);
        drop(config_lock);

        // Create execution record before spawning so we can return the ID
        let mut execution = JobExecution::new(&job_name, trigger_source);
        let execution_id = execution.id;

        tokio::spawn(async move {
            // Acquire semaphore permit
            let _permit = match semaphore.acquire().await {
                Ok(p) => p,
                Err(_) => {
                    error!(job_name = &*job_name, trigger = trigger_source, status = "semaphore failed"; "");
                    return;
                }
            };

            // Update state - mark as running
            {
                let mut s = state.write().await;
                s.record_job_start(&job_name, execution_id);
            }

            // Emit starting event
            let _ = event_tx.send(SchedulerEvent::JobStarting {
                job_name: job_name.clone(),
                execution_id,
            });

            let exec_id_str = execution_id.to_string();
            info!(
                job_name = &*job_name,
                trigger = trigger_source,
                status = "starting",
                execution_id = &*exec_id_str;
                ""
            );

            // Execute the job
            let start = Instant::now();
            let result = executor.execute(&job_name, &job).await;
            let duration = start.elapsed();

            // Process result
            let (status, next_run) = match result {
                Ok((exit_code, stdout, stderr)) => {
                    if print_output {
                        for line in stdout.lines() {
                            info!(job_name = &*job_name, status = "output", output = line; "");
                        }
                        for line in stderr.lines() {
                            error!(job_name = &*job_name, status = "output", output = line; "");
                        }
                    }

                    let duration_str = format!("{}ms", duration.as_millis());
                    if exit_code == 0 {
                        execution.complete_success(exit_code, stdout, stderr);
                        info!(
                            job_name = &*job_name,
                            trigger = trigger_source,
                            status = "success",
                            duration = &*duration_str;
                            ""
                        );
                        let tz = config.settings.effective_timezone();
                        (
                            JobStatus::Success,
                            job.next_run(tz)
                                .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc)),
                        )
                    } else {
                        let error_msg = format!("Exit code: {}", exit_code);
                        execution.complete_failed(
                            error_msg.clone(),
                            Some(exit_code),
                            stdout,
                            stderr,
                        );
                        warn!(
                            job_name = &*job_name,
                            trigger = trigger_source,
                            status = "failed",
                            exit_code = exit_code;
                            ""
                        );
                        let tz = config.settings.effective_timezone();
                        (
                            JobStatus::Failed { error: error_msg },
                            job.next_run(tz)
                                .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc)),
                        )
                    }
                }
                Err(Error::JobTimeout { .. }) => {
                    execution.complete_timeout();
                    warn!(
                        job_name = &*job_name,
                        trigger = trigger_source,
                        status = "timeout";
                        ""
                    );
                    let tz = config.settings.effective_timezone();
                    (
                        JobStatus::Timeout,
                        job.next_run(tz)
                            .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc)),
                    )
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    execution.complete_failed(
                        error_msg.clone(),
                        None,
                        String::new(),
                        String::new(),
                    );
                    error!(
                        job_name = &*job_name,
                        trigger = trigger_source,
                        status = "error",
                        error = &*error_msg;
                        ""
                    );
                    let tz = config.settings.effective_timezone();
                    (
                        JobStatus::Failed { error: error_msg },
                        job.next_run(tz)
                            .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc)),
                    )
                }
            };

            let success = matches!(status, JobStatus::Success);

            // Update state
            {
                let mut s = state.write().await;
                s.record_job_completion(
                    &job_name,
                    status,
                    execution,
                    next_run,
                    #[cfg(feature = "web")]
                    job_history_size,
                    #[cfg(feature = "web")]
                    max_history_size,
                );
            }

            // Emit completion event
            let _ = event_tx.send(SchedulerEvent::JobCompleted {
                job_name,
                execution_id,
                success,
                duration: duration.as_millis() as u64,
            });
        });

        Ok(execution_id)
    }

    /// Trigger a job immediately
    async fn trigger_job(&mut self, job_name: &str) -> crate::error::Result<uuid::Uuid> {
        self.trigger_job_with_source(job_name, "manual").await
    }

    /// Trigger a job immediately with explicit source
    async fn trigger_job_with_source(
        &mut self,
        job_name: &str,
        source: &'static str,
    ) -> crate::error::Result<uuid::Uuid> {
        let config = self.config.read().await;
        let job = config
            .get_job(job_name)
            .ok_or_else(|| Error::job_not_found(job_name))?
            .clone();
        drop(config);

        self.spawn_job_execution(job_name.to_string(), job, source)
            .await
    }

    /// Reload configuration from file
    async fn reload_config(&mut self) -> Result<()> {
        debug!(status = "reloading configuration"; "");

        let mut new_config = Config::from_file(&self.config_path)?;

        // Preserve existing api_token if the new config doesn't set one
        #[cfg(feature = "web")]
        {
            let old_config = self.config.read().await;
            if new_config.settings.api_token.is_none() && old_config.settings.api_token.is_some() {
                new_config.settings.api_token = old_config.settings.api_token.clone();
            }
        }

        // Update executor shell settings
        self.executor.update_shell(
            new_config.settings.shell.clone(),
            new_config.settings.shell_args.clone(),
        );

        // Clear and rebuild trigger queue
        self.trigger_queue.clear();

        let mut state = self.state.write().await;

        // Keep track of previous job states
        let old_jobs = state.jobs.clone();

        // Reset job info
        state.jobs.clear();
        state.total_jobs = 0;
        state.enabled_jobs = 0;

        // Add jobs from new config
        let tz = new_config.settings.effective_timezone();
        for (name, job) in &new_config.jobs {
            let next_run = if job.enabled {
                job.next_run(tz)
                    .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc))
            } else {
                None
            };
            let mut scheduled_job =
                ScheduledJob::new(name.clone(), job.schedule.clone(), job.enabled)
                    .with_next_run(next_run);

            // Preserve previous state
            if let Some(old_job) = old_jobs.get(name) {
                scheduled_job.is_running = old_job.is_running;
                scheduled_job.last_run = old_job.last_run;
                scheduled_job.last_status = old_job.last_status.clone();
                scheduled_job.run_count = old_job.run_count;
                scheduled_job.failure_count = old_job.failure_count;
            }

            state.add_job(scheduled_job);

            if job.enabled {
                if let Some(next) = next_run {
                    self.trigger_queue.push(TimedTrigger {
                        trigger: JobTrigger::new(name.clone(), next, job.clone()),
                    });
                }
            }
        }

        let job_count = state.enabled_jobs;
        drop(state);

        *self.config.write().await = new_config;

        info!(
            status = "config reloaded",
            file = self.config_path.to_string_lossy().as_ref(),
            enabled = job_count as u64;
            ""
        );
        let _ = self
            .event_tx
            .send(SchedulerEvent::ConfigReloaded { job_count });

        Ok(())
    }

    /// Handle incoming messages
    async fn handle_message(&mut self, msg: SchedulerMessage) -> Result<bool> {
        match msg {
            SchedulerMessage::TriggerJob {
                job_name,
                response_tx,
            } => {
                let result = self.trigger_job(&job_name).await;
                if let Err(e) = &result {
                    let err_str = e.to_string();
                    warn!(job_name = &*job_name, status = "trigger failed", error = &*err_str; "");
                }
                let _ = response_tx.send(result);
            }
            SchedulerMessage::ReloadConfig => {
                if let Err(e) = self.reload_config().await {
                    let err_str = e.to_string();
                    error!(status = "reload failed", error = &*err_str; "");
                }
            }
            SchedulerMessage::GetStatus { response_tx } => {
                let mut state = self.state.read().await.clone();
                state.update_time();
                let _ = response_tx.send(state);
            }
            SchedulerMessage::StopJob { job_name } => {
                // TODO: Implement job cancellation
                warn!(job_name = &*job_name, status = "stop unimplemented"; "");
            }
            SchedulerMessage::Shutdown => {
                debug!(status = "shutdown requested"; "");
                self.shutdown = true;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Handle for communicating with the scheduler
#[derive(Clone)]
pub struct SchedulerHandle {
    message_tx: mpsc::Sender<SchedulerMessage>,
    event_tx: broadcast::Sender<SchedulerEvent>,
}

impl SchedulerHandle {
    /// Send a message to the scheduler
    pub async fn send(&self, msg: SchedulerMessage) -> Result<()> {
        self.message_tx
            .send(msg)
            .await
            .map_err(|_| Error::ChannelSend)
    }

    /// Trigger a job immediately
    pub async fn trigger_job(&self, job_name: impl Into<String>) -> Result<uuid::Uuid> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(SchedulerMessage::TriggerJob {
            job_name: job_name.into(),
            response_tx: tx,
        })
        .await?;
        rx.await.map_err(|_| Error::ChannelSend)?
    }

    /// Reload configuration
    pub async fn reload_config(&self) -> Result<()> {
        self.send(SchedulerMessage::ReloadConfig).await
    }

    /// Get scheduler status
    pub async fn get_status(&self) -> Result<SchedulerState> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(SchedulerMessage::GetStatus { response_tx: tx })
            .await?;
        rx.await.map_err(|_| Error::ChannelSend)
    }

    /// Stop a running job
    pub async fn stop_job(&self, job_name: impl Into<String>) -> Result<()> {
        self.send(SchedulerMessage::StopJob {
            job_name: job_name.into(),
        })
        .await
    }

    /// Request shutdown
    pub async fn shutdown(&self) -> Result<()> {
        self.send(SchedulerMessage::Shutdown).await
    }

    /// Subscribe to scheduler events
    pub fn subscribe(&self) -> broadcast::Receiver<SchedulerEvent> {
        self.event_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        let toml = r#"
            [settings]
            max_concurrent_jobs = 2

            [jobs.test]
            schedule = "* * * * *"
            command = "echo test"
        "#;
        Config::from_str(toml, "test.toml").unwrap()
    }

    #[tokio::test]
    async fn test_scheduler_creation() {
        let config = test_config();
        let (mut scheduler, handle) = Scheduler::new(config, PathBuf::from("test.toml"));

        // Initialize manually for testing
        scheduler.initialize().await.unwrap();

        // Spawn a task to process messages
        tokio::spawn(async move {
            if let Some(msg) = scheduler.message_rx.recv().await {
                scheduler.handle_message(msg).await.unwrap();
            }
        });

        // Verify handle works
        assert!(handle.trigger_job("test").await.is_ok());
    }

    #[tokio::test]
    async fn test_scheduler_status() {
        let config = test_config();
        let (mut scheduler, _handle) = Scheduler::new(config, PathBuf::from("test.toml"));

        // Initialize manually for testing
        scheduler.initialize().await.unwrap();

        // Get status through the scheduler's state directly
        let state = scheduler.state.read().await;
        assert_eq!(state.total_jobs, 1);
        assert_eq!(state.enabled_jobs, 1);
    }
}
