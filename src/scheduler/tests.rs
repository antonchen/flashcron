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
    let (mut scheduler, handle) = Scheduler::new(config, PathBuf::from("test.toml"), None);

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
async fn final_failure_hook_receives_one_json_message_after_retries() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let temp_dir = tempfile::tempdir_in("/tmp").unwrap();
    let hook_path = temp_dir.path().join("failure-hook");
    fs::write(
        &hook_path,
        "#!/bin/sh\nbase=${0%/*}\ncat > \"$base/message\"\nprintf 'called\\n' >> \"$base/count\"\n",
    )
    .unwrap();
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

    let mut config = Config::from_str(
        r#"
            [jobs.failure]
            schedule = "* * * * *"
            command = "printf 'failure output'; exit 7"
            run_on_startup = true
            retry_count = 1
            retry_delay = 0
        "#,
        "test.toml",
    )
    .unwrap();
    config.settings.failure_hook = Some(hook_path);

    let (scheduler, handle) = Scheduler::new(config, PathBuf::from("test.toml"), None);
    let worker = tokio::spawn(scheduler.run());

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = handle.get_status().await.unwrap();
            if matches!(
                state.jobs["failure"].last_status,
                Some(JobStatus::Failed { .. })
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("job did not reach its final failed state");

    let count_path = temp_dir.path().join("count");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if tokio::fs::try_exists(&count_path).await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failure hook was not called");

    let message: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(temp_dir.path().join("message"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(message.as_object().unwrap().len(), 7);
    assert_eq!(message["event"], "job.failed");
    assert_eq!(message["job_name"], "failure");
    assert_eq!(message["trigger"], "startup");
    assert!(chrono::DateTime::parse_from_rfc3339(message["ended_at"].as_str().unwrap()).is_ok());
    assert_eq!(message["attempt"], 2);
    assert_eq!(message["error"], "exit_code");
    assert_eq!(message["output"], "failure output");
    assert_eq!(
        tokio::fs::read_to_string(count_path).await.unwrap(),
        "called\n"
    );

    handle.shutdown().await.unwrap();
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn failure_hook_timeout_kills_and_reaps_child() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let temp_dir = tempfile::tempdir_in("/tmp").unwrap();
    let hook_path = temp_dir.path().join("failure-hook");
    let pid_path = temp_dir.path().join("pid");
    std::fs::write(
        &hook_path,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
            pid_path.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut child = tokio::process::Command::new(&hook_path)
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !pid_path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("hook did not start");
    let pid = std::fs::read_to_string(pid_path)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();

    let result = run_failure_hook_child(
        &mut child,
        b"{}\n",
        Duration::from_millis(50),
        "timeout-test",
    )
    .await;

    assert!(result.unwrap().is_none(), "hook should time out");
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
}

#[cfg(feature = "web")]
#[tokio::test]
async fn test_add_or_update_job_applies_runtime_definition() {
    let config = Config::default();
    let db = DatabaseManager::init(":memory:").await.unwrap();
    let (mut scheduler, _) = Scheduler::new(config, PathBuf::from("test.toml"), Some(db.clone()));
    scheduler
        .add_or_update_job(
            "web-job".into(),
            Job {
                schedule: "* * * * *".into(),
                command: "echo first".into(),
                ..Job::default()
            },
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(scheduler.state.read().await.total_jobs, 1);
    assert_eq!(scheduler.trigger_queue.len(), 1);

    scheduler
        .add_or_update_job(
            "web-job".into(),
            Job {
                schedule: "0 0 1 1 *".into(),
                command: "echo second".into(),
                enabled: false,
                ..Job::default()
            },
            true,
            None,
        )
        .await
        .unwrap();
    assert_eq!(scheduler.state.read().await.total_jobs, 1);
    assert_eq!(scheduler.state.read().await.enabled_jobs, 0);
    assert!(scheduler.trigger_queue.is_empty());
    assert_eq!(
        scheduler.config.read().await.jobs["web-job"].command,
        "echo second"
    );
    assert_eq!(
        db.list_jobs().await.unwrap()["web-job"].command,
        "echo second"
    );
}

#[cfg(feature = "web")]
#[tokio::test]
async fn test_deleted_job_is_not_recreated_by_pending_update() {
    let db = DatabaseManager::init(":memory:").await.unwrap();
    let (mut scheduler, handle) = Scheduler::new(
        Config::default(),
        PathBuf::from("test.toml"),
        Some(db.clone()),
    );
    let config = scheduler.config.clone();
    let state = scheduler.state.clone();
    let worker = tokio::spawn(async move {
        while let Some(message) = scheduler.message_rx.recv().await {
            if scheduler.handle_message(message).await.unwrap() {
                break;
            }
        }
        assert!(scheduler.trigger_queue.is_empty());
    });
    let job = Job {
        schedule: "0 0 1 1 *".into(),
        command: "echo test".into(),
        ..Job::default()
    };
    assert!(handle
        .add_or_update_job("web-job".into(), job.clone(), false, None)
        .await
        .unwrap());
    assert!(!handle
        .add_or_update_job("web-job".into(), job.clone(), false, None)
        .await
        .unwrap());
    // Queue both operations before waiting, as concurrent HTTP handlers can do.
    let (delete_tx, delete_rx) = tokio::sync::oneshot::channel();
    handle
        .send(SchedulerMessage::RemoveJob {
            job_name: "web-job".into(),
            response_tx: delete_tx,
        })
        .await
        .unwrap();
    let (update_tx, update_rx) = tokio::sync::oneshot::channel();
    handle
        .send(SchedulerMessage::AddOrUpdateJob {
            job_name: "web-job".into(),
            job: Box::new(job),
            update: true,
            rename_from: None,
            response_tx: update_tx,
        })
        .await
        .unwrap();
    delete_rx.await.unwrap().unwrap();
    assert!(!update_rx.await.unwrap().unwrap());
    assert!(db.list_jobs().await.unwrap().is_empty());
    assert!(config.read().await.jobs.is_empty());
    assert!(state.read().await.jobs.is_empty());
    handle.shutdown().await.unwrap();
    worker.await.unwrap();
}

#[cfg(feature = "web")]
#[tokio::test]
async fn test_remove_job_clears_scheduler_definition_and_queue() {
    let mut config = Config::default();
    config.jobs.insert(
        "web-job".into(),
        Job {
            schedule: "* * * * *".into(),
            ..Job::default()
        },
    );
    let (mut scheduler, _) = Scheduler::new(config, PathBuf::from("test.toml"), None);
    scheduler.initialize().await.unwrap();
    scheduler.remove_job("web-job").await.unwrap();
    assert!(scheduler.config.read().await.jobs.is_empty());
    assert!(scheduler.state.read().await.jobs.is_empty());
    assert!(scheduler.trigger_queue.is_empty());
}

#[tokio::test]
async fn test_scheduler_status() {
    let config = test_config();
    let (mut scheduler, _handle) = Scheduler::new(config, PathBuf::from("test.toml"), None);

    // Initialize manually for testing
    scheduler.initialize().await.unwrap();

    // Get status through the scheduler's state directly
    let state = scheduler.state.read().await;
    assert_eq!(state.total_jobs, 1);
    assert_eq!(state.enabled_jobs, 1);
}

#[tokio::test]
async fn test_scheduler_counts_reset_after_restart() {
    use tempfile::NamedTempFile;

    let db_file = NamedTempFile::new_in("/tmp").unwrap();
    let db_path = db_file.path().to_str().unwrap().to_string();

    // 1. Create DB and save a fake execution
    let db = DatabaseManager::init(&db_path).await.unwrap();
    let mut exec = JobExecution::new("test-job", "manual");
    exec.complete_success(0, Some("ok".into()));
    db.save(exec, 100, 10000).await.unwrap();

    // 2. Initialize scheduler with this DB
    // Match config job name to our saved DB name
    let config_str = r#"
            [jobs.test-job]
            schedule = "* * * * *"
            command = "echo test"
        "#;
    let config = Config::from_str(config_str, "test.toml").unwrap();

    let (mut scheduler, _handle) = Scheduler::new(config, PathBuf::from("test.toml"), Some(db));

    // 3. Trigger manual initialization
    scheduler.initialize().await.unwrap();

    // 4. Scheduler counts are in-memory only.
    let state = scheduler.state.read().await;
    assert_eq!(state.total_executions, 0);
}
