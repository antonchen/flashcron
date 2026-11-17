//! Main scheduler engine

use crate::config::{Config, Job, JobExecution, JobStatus};
use crate::db::DatabaseManager;
use crate::error::{Error, Result};
use crate::executor::JobExecutor;
use crate::ipc::{PackageRestorePhase, PackageRestoreStatus};
use crate::scheduler::state::{JobRunState, ScheduledJob, SchedulerState};
use crate::scheduler::{JobTrigger, SchedulerEvent, SchedulerMessage};
use chrono::Utc;
use log::{debug, error, info, warn};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, OwnedSemaphorePermit, RwLock};
use tokio::time::{self, Duration, Instant};
use uuid::Uuid;

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

#[derive(Debug, Clone)]
struct QueuedJob {
    execution_id: Uuid,
    job_name: String,
    job: Job,
    trigger_source: &'static str,
}

enum DispatchResult {
    Started(Uuid),
    Queued(Uuid),
    Skipped,
}

fn should_persist_history(config: &Config) -> bool {
    #[cfg(feature = "web")]
    {
        config.settings.max_history_size > 0
    }

    #[cfg(not(feature = "web"))]
    {
        let _ = config;
        false
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
    /// Message sender
    message_tx: mpsc::Sender<SchedulerMessage>,
    /// Event broadcaster
    event_tx: broadcast::Sender<SchedulerEvent>,
    /// Priority queue of upcoming triggers
    trigger_queue: BinaryHeap<TimedTrigger>,
    /// Jobs waiting for a free execution slot
    ready_queue: VecDeque<QueuedJob>,
    /// Semaphore for limiting concurrent jobs
    job_semaphore: Arc<tokio::sync::Semaphore>,
    /// Database manager
    db: Option<DatabaseManager>,
    /// Shutdown flag
    shutdown: bool,
    /// Package restoration state; scheduled work remains gated until ready.
    restore_status: tokio::sync::watch::Receiver<PackageRestoreStatus>,
}

impl Scheduler {
    /// Create a new scheduler
    pub fn new(
        config: Config,
        config_path: PathBuf,
        db: Option<DatabaseManager>,
    ) -> (Self, SchedulerHandle) {
        let (_, restore_status) = tokio::sync::watch::channel(PackageRestoreStatus::ready());
        Self::new_with_restore_status(config, config_path, db, restore_status)
    }

    pub fn new_with_restore_status(
        config: Config,
        config_path: PathBuf,
        db: Option<DatabaseManager>,
        restore_status: tokio::sync::watch::Receiver<PackageRestoreStatus>,
    ) -> (Self, SchedulerHandle) {
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
            config.settings.working_dir.clone(),
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
            ready_queue: VecDeque::new(),
            job_semaphore: semaphore,
            db,
            shutdown: false,
            restore_status,
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
                        trigger: JobTrigger::new(name.clone(), next),
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

        loop {
            let restore_phase = self.restore_status.borrow().phase.clone();
            if restore_phase == PackageRestorePhase::Ready {
                break;
            }

            tokio::select! {
                changed = self.restore_status.changed() => {
                    if changed.is_err() {
                        return Err(Error::Scheduler("package restore state closed".to_string()));
                    }
                }
                Some(msg) = self.message_rx.recv() => {
                    if self.handle_message(msg).await? {
                        break;
                    }
                }
            }
        }

        // Run startup jobs
        if !self.shutdown {
            self.run_startup_jobs().await?;
        }

        debug!(status = "scheduler started"; "");

        let mut maintenance_interval = time::interval(Duration::from_secs(24 * 3600));

        loop {
            if self.shutdown {
                break;
            }

            // Calculate sleep duration until next trigger
            let sleep_duration = self.calculate_sleep_duration();

            tokio::select! {
                // Wait for next trigger or message
                _ = time::sleep(sleep_duration) => {
                    self.process_due_triggers().await?;
                }

                // Periodic maintenance
                _ = maintenance_interval.tick() => {
                    #[cfg(feature = "web")]
                    if let Some(ref db) = self.db {
                        let config = self.config.read().await;
                        let active_jobs = config.jobs.keys().cloned().collect();
                        let job_size = config.settings.job_history_size;
                        let max_size = config.settings.max_history_size;
                        if let Err(e) = db.cleanup(active_jobs, job_size, max_size).await {
                            warn!(status = "db cleanup failed", error = &*e.to_string(); "");
                        }
                    }
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

        self.start_queued_jobs().await?;

        Ok(())
    }

    /// Execute a job trigger
    async fn execute_trigger(&mut self, trigger: JobTrigger) -> Result<()> {
        let job_name = trigger.job_name.clone();
        let (job, next_run) = {
            let config = self.config.read().await;
            let tz = config.settings.effective_timezone();
            let job = config
                .get_job(&job_name)
                .ok_or_else(|| Error::job_not_found(&job_name))?
                .clone();
            let next_run = job
                .next_run(tz)
                .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc));
            (job, next_run)
        };

        // Schedule next occurrence
        if let Some(next) = next_run {
            // Make sure we don't schedule the same time again
            if next > trigger.scheduled_at {
                self.trigger_queue.push(TimedTrigger {
                    trigger: JobTrigger::new(job_name.clone(), next),
                });

                // Update state with next run time
                let mut state = self.state.write().await;
                if let Some(sj) = state.get_job_mut(&job_name) {
                    sj.next_run = Some(next);
                }
            }
        }

        match self.dispatch_job(job_name.clone(), job, "cron").await? {
            DispatchResult::Started(_) | DispatchResult::Queued(_) => {}
            DispatchResult::Skipped => {
                debug!(job_name = &*job_name, status = "skipped overlap"; "");
            }
        }

        Ok(())
    }

    async fn dispatch_job(
        &mut self,
        job_name: String,
        job: Job,
        trigger_source: &'static str,
    ) -> crate::error::Result<DispatchResult> {
        let execution_id = Uuid::new_v4();
        let permit = {
            let mut state = self.state.write().await;
            if !state.can_accept_trigger(&job_name) {
                return Ok(DispatchResult::Skipped);
            }

            match self.job_semaphore.clone().try_acquire_owned() {
                Ok(permit) => {
                    state.record_job_starting(&job_name);
                    Some(permit)
                }
                Err(tokio::sync::TryAcquireError::NoPermits) => {
                    state.record_job_queued(&job_name);
                    None
                }
                Err(tokio::sync::TryAcquireError::Closed) => {
                    return Err(Error::Scheduler("job semaphore closed".to_string()));
                }
            }
        };

        if let Some(permit) = permit {
            self.spawn_job_execution(job_name, job, trigger_source, execution_id, permit)
                .await?;
            Ok(DispatchResult::Started(execution_id))
        } else {
            self.ready_queue.push_back(QueuedJob {
                execution_id,
                job_name,
                job,
                trigger_source,
            });
            Ok(DispatchResult::Queued(execution_id))
        }
    }

    async fn start_queued_jobs(&mut self) -> Result<()> {
        loop {
            if self.ready_queue.is_empty() {
                return Ok(());
            }

            let permit = match self.job_semaphore.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(tokio::sync::TryAcquireError::NoPermits) => return Ok(()),
                Err(tokio::sync::TryAcquireError::Closed) => {
                    return Err(Error::Scheduler("job semaphore closed".to_string()));
                }
            };

            let queued = match self.ready_queue.pop_front() {
                Some(queued) => queued,
                None => return Ok(()),
            };
            {
                let mut state = self.state.write().await;
                state.record_job_starting(&queued.job_name);
            }

            self.spawn_job_execution(
                queued.job_name,
                queued.job,
                queued.trigger_source,
                queued.execution_id,
                permit,
            )
            .await?;
        }
    }

    /// Spawn a job execution task
    async fn spawn_job_execution(
        &self,
        job_name: String,
        job: Job,
        trigger_source: &'static str,
        execution_id: Uuid,
        permit: OwnedSemaphorePermit,
    ) -> crate::error::Result<()> {
        let executor = Arc::clone(&self.executor);
        let state = Arc::clone(&self.state);
        let event_tx = self.event_tx.clone();
        let db = self.db.clone();
        let message_tx = self.message_tx.clone();

        let config_lock = self.config.read().await;
        let print_output = job
            .print_output
            .unwrap_or(config_lock.settings.print_output);
        let persist_history = should_persist_history(&config_lock);
        let timezone = config_lock.settings.effective_timezone();
        drop(config_lock);

        tokio::spawn(async move {
            let _permit = permit;
            let mut execution = JobExecution::new(job_name.clone(), trigger_source);
            execution.id = execution_id;

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

            let (live_output_tx, mut live_output_rx) = tokio::sync::watch::channel(String::new());
            let output_state = Arc::clone(&state);
            let output_events = event_tx.clone();
            let output_job_name = job_name.clone();
            let output_task = tokio::spawn(async move {
                while live_output_rx.changed().await.is_ok() {
                    let output = live_output_rx.borrow_and_update().clone();
                    let update = output_state
                        .write()
                        .await
                        .update_live_output(&output_job_name, output.clone());
                    if let Some((execution_id, sequence)) = update {
                        let _ = output_events.send(SchedulerEvent::JobOutput {
                            job_name: output_job_name.clone(),
                            execution_id,
                            sequence,
                            output,
                        });
                    }
                }
            });

            let started = Instant::now();
            let retry_policy = job.retry_policy();
            let mut attempt = 1;
            let final_result = loop {
                let result = executor
                    .execute_with_live_output(&job_name, &job, Some(live_output_tx.clone()))
                    .await;
                let should_retry = !matches!(&result, Ok(result) if result.exit_code == 0)
                    && retry_policy.is_enabled()
                    && attempt <= retry_policy.max_attempts;

                if should_retry {
                    warn!(
                        job_name = &*job_name,
                        trigger = trigger_source,
                        status = "retrying",
                        attempt = attempt,
                        delay = retry_policy.delay_seconds;
                        ""
                    );
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(retry_policy.delay_seconds)).await;
                    continue;
                }

                break result;
            };
            drop(live_output_tx);
            let _ = output_task.await;

            let duration = started.elapsed();
            execution.attempt = attempt;

            let (status, next_run) = match final_result {
                Ok(result) => {
                    let exit_code = result.exit_code;
                    let output = result.output;
                    if print_output {
                        for line in output.lines() {
                            info!(job_name = &*job_name, status = "output", output = line; "");
                        }
                    }

                    let duration_str = format!("{}ms", duration.as_millis());
                    let stored_output = if persist_history {
                        Some(output.clone())
                    } else {
                        None
                    };
                    if exit_code == 0 {
                        execution.complete_success(exit_code, stored_output);
                        info!(
                            job_name = &*job_name,
                            trigger = trigger_source,
                            status = "success",
                            duration = &*duration_str;
                            ""
                        );
                        (
                            JobStatus::Success,
                            job.next_run(timezone)
                                .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc)),
                        )
                    } else {
                        let error_msg = format!("Exit code: {}", exit_code);
                        execution.complete_failed(
                            error_msg.clone(),
                            Some(exit_code),
                            stored_output,
                        );
                        warn!(
                            job_name = &*job_name,
                            trigger = trigger_source,
                            status = "failed",
                            exit_code = exit_code;
                            ""
                        );
                        (
                            JobStatus::Failed { error: error_msg },
                            job.next_run(timezone)
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
                    (
                        JobStatus::Timeout,
                        job.next_run(timezone)
                            .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc)),
                    )
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    execution.complete_failed(error_msg.clone(), None, None);
                    error!(
                        job_name = &*job_name,
                        trigger = trigger_source,
                        status = "error",
                        error = &*error_msg;
                        ""
                    );
                    (
                        JobStatus::Failed { error: error_msg },
                        job.next_run(timezone)
                            .map(|t: chrono::DateTime<chrono_tz::Tz>| t.with_timezone(&Utc)),
                    )
                }
            };

            let success = matches!(status, JobStatus::Success);

            // Save to database if enabled
            if let Some(db) = db {
                if let Err(e) = db.save(execution.clone(), persist_history).await {
                    error!(job_name = &*job_name, status = "db save failed", error = &*e.to_string(); "");
                }
            }

            // Update state
            {
                let mut s = state.write().await;
                s.record_job_completion(&job_name, status, next_run);
            }

            // Emit completion event
            let _ = event_tx.send(SchedulerEvent::JobCompleted {
                job_name,
                execution_id,
                success,
                duration: duration.as_millis() as u64,
            });

            let _ = message_tx.send(SchedulerMessage::JobCompleted).await;
        });

        Ok(())
    }

    /// Trigger a job immediately
    async fn trigger_job(&mut self, job_name: &str) -> crate::error::Result<uuid::Uuid> {
        self.trigger_job_with_source(job_name, "manual").await
    }

    async fn add_or_update_job(
        &mut self,
        job_name: String,
        mut job: Job,
        update: bool,
    ) -> Result<bool> {
        job.compile_schedule().map_err(|reason| Error::CronParse {
            expr: job.schedule.clone(),
            reason,
        })?;
        job.validate(&job_name)?;
        #[cfg(feature = "web")]
        if let Some(db) = &self.db {
            if !db.store_job(job_name.clone(), job.clone(), update).await? {
                return Ok(false);
            }
        }
        #[cfg(not(feature = "web"))]
        let _ = update;

        let timezone = self.config.read().await.settings.effective_timezone();
        let next_run = job
            .enabled
            .then(|| job.next_run(timezone).map(|time| time.with_timezone(&Utc)))
            .flatten();

        self.trigger_queue
            .retain(|timed| timed.trigger.job_name != job_name);
        if let Some(next) = next_run {
            self.trigger_queue.push(TimedTrigger {
                trigger: JobTrigger::new(job_name.clone(), next),
            });
        }
        let mut state = self.state.write().await;
        if let Some(previous) = state.jobs.remove(&job_name) {
            state.total_jobs -= 1;
            if previous.enabled {
                state.enabled_jobs -= 1;
            }
            if matches!(previous.run_state, JobRunState::Queued) {
                state.queue_len = state.queue_len.saturating_sub(1);
            }
            let mut scheduled =
                ScheduledJob::new(job_name.clone(), job.schedule.clone(), job.enabled)
                    .with_next_run(next_run);
            scheduled.run_state = previous.run_state;
            scheduled.current_execution_id = previous.current_execution_id;
            scheduled.output_sequence = previous.output_sequence;
            scheduled.live_output = previous.live_output;
            scheduled.last_run = previous.last_run;
            scheduled.last_status = previous.last_status;
            if matches!(scheduled.run_state, JobRunState::Queued) {
                state.queue_len += 1;
            }
            state.add_job(scheduled);
        } else {
            state.add_job(
                ScheduledJob::new(job_name.clone(), job.schedule.clone(), job.enabled)
                    .with_next_run(next_run),
            );
        }
        drop(state);

        self.config.write().await.jobs.insert(job_name, job);
        Ok(true)
    }

    async fn remove_job(&mut self, job_name: &str) -> Result<()> {
        let current = self.state.read().await.jobs.get(job_name).cloned();
        let Some(current) = current else {
            return Err(Error::JobNotFound {
                job_name: job_name.to_owned(),
            });
        };
        if matches!(
            current.run_state,
            JobRunState::Starting | JobRunState::Running
        ) {
            return Err(Error::Scheduler(format!("Job '{job_name}' is running")));
        }
        #[cfg(feature = "web")]
        if let Some(db) = &self.db {
            if !db.delete_job(job_name).await? {
                return Err(Error::JobNotFound {
                    job_name: job_name.to_owned(),
                });
            }
        }
        self.trigger_queue
            .retain(|timed| timed.trigger.job_name != job_name);
        self.ready_queue
            .retain(|queued| queued.job_name != job_name);
        self.config.write().await.jobs.remove(job_name);
        let mut state = self.state.write().await;
        state.jobs.remove(job_name);
        state.total_jobs = state.total_jobs.saturating_sub(1);
        if current.enabled {
            state.enabled_jobs = state.enabled_jobs.saturating_sub(1);
        }
        if matches!(current.run_state, JobRunState::Queued) {
            state.queue_len = state.queue_len.saturating_sub(1);
        }
        Ok(())
    }

    /// Trigger a job immediately with explicit source
    async fn trigger_job_with_source(
        &mut self,
        job_name: &str,
        source: &'static str,
    ) -> crate::error::Result<uuid::Uuid> {
        if self.restore_status.borrow().phase != PackageRestorePhase::Ready {
            return Err(Error::Scheduler(
                "package restoration is not ready".to_string(),
            ));
        }

        let config = self.config.read().await;
        let job = config
            .get_job(job_name)
            .ok_or_else(|| Error::job_not_found(job_name))?
            .clone();
        drop(config);

        match self.dispatch_job(job_name.to_string(), job, source).await? {
            DispatchResult::Started(execution_id) | DispatchResult::Queued(execution_id) => {
                Ok(execution_id)
            }
            DispatchResult::Skipped => Err(Error::Scheduler(format!(
                "Job '{}' is already queued or running",
                job_name
            ))),
        }
    }

    /// Reload configuration from file
    async fn reload_config(&mut self) -> Result<()> {
        debug!(status = "reloading configuration"; "");

        #[allow(unused_mut)]
        let mut new_config = {
            #[cfg(feature = "web")]
            {
                Config::from_file_settings(&self.config_path)?
            }
            #[cfg(not(feature = "web"))]
            {
                Config::from_file(&self.config_path)?
            }
        };

        // Preserve existing api_token if the new config doesn't set one
        #[cfg(feature = "web")]
        {
            let old_config = self.config.read().await;
            new_config.jobs = old_config.jobs.clone();
            if new_config.settings.api_token.is_none() && old_config.settings.api_token.is_some() {
                new_config.settings.api_token = old_config.settings.api_token.clone();
            }
        }

        // Update executor defaults
        self.executor.update_defaults(
            new_config.settings.shell.clone(),
            new_config.settings.shell_args.clone(),
            new_config.settings.working_dir.clone(),
        );

        // Clear and rebuild trigger queue
        self.trigger_queue.clear();

        let mut state = self.state.write().await;

        // Keep track of previous job states
        let mut old_jobs = std::mem::take(&mut state.jobs);

        // Reset job info
        state.total_jobs = 0;
        state.enabled_jobs = 0;
        state.queue_len = 0;

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
            if let Some(old_job) = old_jobs.remove(name) {
                scheduled_job.run_state = old_job.run_state;
                scheduled_job.current_execution_id = old_job.current_execution_id;
                scheduled_job.output_sequence = old_job.output_sequence;
                scheduled_job.live_output = old_job.live_output;
                scheduled_job.last_run = old_job.last_run;
                scheduled_job.last_status = old_job.last_status;
            }

            if matches!(scheduled_job.run_state, JobRunState::Queued) {
                state.queue_len += 1;
            }

            state.add_job(scheduled_job);

            if job.enabled {
                if let Some(next) = next_run {
                    self.trigger_queue.push(TimedTrigger {
                        trigger: JobTrigger::new(name.clone(), next),
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
            SchedulerMessage::AddOrUpdateJob {
                job_name,
                job,
                update,
                response_tx,
            } => {
                let result = self.add_or_update_job(job_name, *job, update).await;
                let _ = response_tx.send(result);
            }
            SchedulerMessage::RemoveJob {
                job_name,
                response_tx,
            } => {
                let _ = response_tx.send(self.remove_job(&job_name).await);
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
            SchedulerMessage::JobCompleted => {
                self.start_queued_jobs().await?;
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

    pub async fn add_or_update_job(
        &self,
        job_name: String,
        job: Job,
        update: bool,
    ) -> Result<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(SchedulerMessage::AddOrUpdateJob {
            job_name,
            job: Box::new(job),
            update,
            response_tx: tx,
        })
        .await?;
        rx.await.map_err(|_| Error::ChannelSend)?
    }

    pub async fn remove_job(&self, job_name: String) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(SchedulerMessage::RemoveJob {
            job_name,
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
#[path = "tests.rs"]
mod tests;
