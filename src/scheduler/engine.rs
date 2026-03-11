//! Main scheduler engine

use crate::config::{Config, Job, JobExecution, JobStatus};
use crate::error::{Error, Result};
use crate::executor::JobExecutor;
use crate::scheduler::state::{ScheduledJob, SchedulerState};
use crate::scheduler::{JobTrigger, SchedulerEvent, SchedulerMessage};
use chrono::Utc;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::time::{self, Duration, Instant};
use tracing::{debug, error, info, info_span, warn};

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
        let config = self.config.read().await;
        let mut state = self.state.write().await;

        for (name, job) in config.enabled_jobs() {
            let next_run = job.next_run();
            let scheduled_job = ScheduledJob::new(name.clone(), job.schedule.clone(), job.enabled)
                .with_next_run(next_run);

            state.add_job(scheduled_job);

            // Add to trigger queue
            if let Some(next) = next_run {
                self.trigger_queue.push(TimedTrigger {
                    trigger: JobTrigger::new(name.clone(), next, job.clone()),
                });
            }
        }

        info!(
            "Scheduler initialized with {} jobs ({} enabled)",
            state.total_jobs, state.enabled_jobs
        );

        Ok(())
    }

    /// Run jobs that are configured to run on startup
    async fn run_startup_jobs(&mut self) -> Result<()> {
        let config = self.config.read().await;
        let startup_jobs: Vec<_> = config
            .enabled_jobs()
            .filter(|(_, job)| job.run_on_startup)
            .map(|(name, _)| name.clone())
            .collect();
        drop(config);

        for job_name in startup_jobs {
            info!(job = %job_name, "Running startup job");
            self.trigger_job(&job_name).await?;
        }

        Ok(())
    }

    /// Main scheduler loop
    pub async fn run(mut self) -> Result<()> {
        self.initialize().await?;

        // Emit started event
        let _ = self.event_tx.send(SchedulerEvent::Started);

        // Run startup jobs
        self.run_startup_jobs().await?;

        info!("Scheduler started");

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

        info!("Scheduler stopped");
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
        if let Some(next) = job.next_run() {
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
                    debug!(job = %job_name, "Job already running, skipping");
                    return Ok(());
                }
            }
        }

        // Spawn job execution
        self.spawn_job_execution(job_name, job).await
    }

    /// Spawn a job execution task
    async fn spawn_job_execution(&self, job_name: String, job: Job) -> Result<()> {
        let executor = Arc::clone(&self.executor);
        let state = Arc::clone(&self.state);
        let event_tx = self.event_tx.clone();
        let semaphore = Arc::clone(&self.job_semaphore);
        
        let config = self.config.read().await;
        let history_size = config.settings.history_size;
        let print_output = job.print_output.unwrap_or(config.settings.print_output);
        drop(config);

        tokio::spawn(async move {
            // Acquire semaphore permit
            let _permit = match semaphore.acquire().await {
                Ok(p) => p,
                Err(_) => {
                    error!(job = %job_name, "Failed to acquire job semaphore");
                    return;
                }
            };

            // Create execution record
            let mut execution = JobExecution::new(&job_name);
            let execution_id = execution.id;

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

            info!(job = %job_name, execution_id = %execution_id, "Starting job");

            // Execute the job
            let start = Instant::now();
            let result = executor.execute(&job_name, &job).await;
            let duration = start.elapsed();

            // Process result
            let (status, next_run) = match result {
                Ok((exit_code, stdout, stderr)) => {
                    if print_output {
                        let span = info_span!("output", job_name = %job_name);
                        let _guard = span.enter();
                        for line in stdout.lines() {
                            info!("{}", line);
                        }
                        for line in stderr.lines() {
                            error!("{}", line);
                        }
                    }

                    if exit_code == 0 {
                        execution.complete_success(exit_code, stdout, stderr);
                        info!(
                            job = %job_name,
                            duration_ms = %duration.as_millis(),
                            "Job completed successfully"
                        );
                        (JobStatus::Success, job.next_run())
                    } else {
                        let error = format!("Exit code: {}", exit_code);
                        execution.complete_failed(error.clone(), Some(exit_code), stdout, stderr);
                        warn!(
                            job = %job_name,
                            exit_code = %exit_code,
                            "Job failed"
                        );
                        (JobStatus::Failed { error }, job.next_run())
                    }
                }
                Err(Error::JobTimeout { .. }) => {
                    execution.complete_timeout();
                    warn!(job = %job_name, "Job timed out");
                    (JobStatus::Timeout, job.next_run())
                }
                Err(e) => {
                    let error = e.to_string();
                    execution.complete_failed(error.clone(), None, String::new(), String::new());
                    error!(job = %job_name, error = %e, "Job execution error");
                    (JobStatus::Failed { error }, job.next_run())
                }
            };

            let success = matches!(status, JobStatus::Success);

            // Update state
            {
                let mut s = state.write().await;
                s.record_job_completion(&job_name, status, execution, next_run, history_size);
            }

            // Emit completion event
            let _ = event_tx.send(SchedulerEvent::JobCompleted {
                job_name,
                execution_id,
                success,
                duration_ms: duration.as_millis() as u64,
            });
        });

        Ok(())
    }

    /// Trigger a job immediately
    async fn trigger_job(&mut self, job_name: &str) -> Result<()> {
        let config = self.config.read().await;
        let job = config
            .get_job(job_name)
            .ok_or_else(|| Error::job_not_found(job_name))?
            .clone();
        drop(config);

        info!(job = %job_name, "Manually triggering job");
        self.spawn_job_execution(job_name.to_string(), job).await
    }

    /// Reload configuration from file
    async fn reload_config(&mut self) -> Result<()> {
        info!("Reloading configuration from {:?}", self.config_path);

        let new_config = Config::from_file(&self.config_path)?;

        // Update executor shell settings
        self.executor.update_shell(
            new_config.settings.shell.clone(),
            new_config.settings.shell_args.clone(),
        );

        // Clear and rebuild trigger queue
        self.trigger_queue.clear();

        let mut state = self.state.write().await;

        // Keep track of running jobs
        let running_jobs: Vec<_> = state
            .jobs
            .iter()
            .filter(|(_, j)| j.is_running)
            .map(|(n, j)| (n.clone(), j.current_execution_id))
            .collect();

        // Reset job info
        state.jobs.clear();
        state.total_jobs = 0;
        state.enabled_jobs = 0;

        // Add jobs from new config
        for (name, job) in new_config.enabled_jobs() {
            let next_run = job.next_run();
            let mut scheduled_job =
                ScheduledJob::new(name.clone(), job.schedule.clone(), job.enabled)
                    .with_next_run(next_run);

            // Preserve running state
            if let Some((_, exec_id)) = running_jobs.iter().find(|(n, _)| n == name) {
                scheduled_job.is_running = true;
                scheduled_job.current_execution_id = *exec_id;
            }

            state.add_job(scheduled_job);

            if let Some(next) = next_run {
                self.trigger_queue.push(TimedTrigger {
                    trigger: JobTrigger::new(name.clone(), next, job.clone()),
                });
            }
        }

        let job_count = state.enabled_jobs;
        drop(state);

        *self.config.write().await = new_config;

        info!("Configuration reloaded: {} enabled jobs", job_count);
        let _ = self
            .event_tx
            .send(SchedulerEvent::ConfigReloaded { job_count });

        Ok(())
    }

    /// Handle incoming messages
    async fn handle_message(&mut self, msg: SchedulerMessage) -> Result<bool> {
        match msg {
            SchedulerMessage::TriggerJob { job_name } => {
                if let Err(e) = self.trigger_job(&job_name).await {
                    warn!(job = %job_name, error = %e, "Failed to trigger job");
                }
            }
            SchedulerMessage::ReloadConfig => {
                if let Err(e) = self.reload_config().await {
                    error!(error = %e, "Failed to reload configuration");
                }
            }
            SchedulerMessage::GetStatus { response_tx } => {
                let mut state = self.state.read().await.clone();
                state.update_time();
                let _ = response_tx.send(state);
            }
            SchedulerMessage::StopJob { job_name } => {
                // TODO: Implement job cancellation
                warn!(job = %job_name, "Job cancellation not yet implemented");
            }
            SchedulerMessage::Shutdown => {
                info!("Shutdown requested");
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
    pub async fn trigger_job(&self, job_name: impl Into<String>) -> Result<()> {
        self.send(SchedulerMessage::TriggerJob {
            job_name: job_name.into(),
        })
        .await
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
        let (_scheduler, handle) = Scheduler::new(config, PathBuf::from("test.toml"));

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
