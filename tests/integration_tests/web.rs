use super::seed_web_job;

#[cfg(feature = "web")]
struct TestDaemon(std::process::Child);

#[cfg(feature = "web")]
impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(feature = "web")]
async fn wait_for_daemon_ready(config_path: &std::path::Path) {
    use flashcron::ipc::{IpcRequest, IpcResponse};
    use std::time::Duration;

    let mut ready = false;
    for _ in 0..50 {
        match flashcron::ipc::send_request_with_config(config_path, &IpcRequest::Status).await {
            Ok(IpcResponse::Status(_)) => {
                ready = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }

    assert!(ready, "daemon did not become ready");
}

#[cfg(feature = "web")]
fn write_test_web_config(
    temp: &tempfile::TempDir,
    failure_hook: Option<&std::path::Path>,
    max_concurrent_jobs: Option<usize>,
) -> (std::path::PathBuf, std::path::PathBuf, u16) {
    let config_path = temp.path().join("flashcron.toml");
    let db_path = temp.path().join("flashcron.db");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut settings = format!(
        "sql_file = {:?}\napi_host = \"127.0.0.1\"\napi_port = {}\napi_token = \"secret\"\n",
        db_path, port
    );
    if let Some(path) = failure_hook {
        settings.push_str(&format!("failure_hook = {:?}\n", path));
    }
    if let Some(limit) = max_concurrent_jobs {
        settings.push_str(&format!("max_concurrent_jobs = {limit}\n"));
    }
    std::fs::write(&config_path, format!("[settings]\n{settings}")).unwrap();
    (config_path, db_path, port)
}

#[cfg(feature = "web")]
async fn start_test_daemon(config_path: &std::path::Path) -> TestDaemon {
    use std::process::Stdio;

    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("flashcron"))
        .arg("--config")
        .arg(config_path)
        .arg("run")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_daemon_ready(config_path).await;
    TestDaemon(child)
}

#[cfg(feature = "web")]
fn test_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

#[cfg(feature = "web")]
#[tokio::test]
async fn test_web_job_and_script_management() {
    let _ipc_guard = super::lock_ipc_tests();
    use flashcron::ipc::{IpcRequest, IpcResponse};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    use std::time::Duration;

    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let config_path = temp.path().join("flashcron.toml");
    let db_path = temp.path().join("flashcron.db");
    let script_dir = temp.path().join("scripts");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    fs::write(
        &config_path,
        format!(
            "[settings]\nsql_file = {:?}\napi_host = \"127.0.0.1\"\napi_port = {}\napi_token = \"secret\"\nscript_dir = {:?}\n",
            db_path, port, script_dir
        ),
    )
    .unwrap();

    let mut child = TestDaemon(
        std::process::Command::new(assert_cmd::cargo::cargo_bin("flashcron"))
            .arg("--config")
            .arg(&config_path)
            .arg("run")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for_daemon_ready(&config_path).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");

    let public_list = client.get(format!("{base}/api/jobs")).send().await.unwrap();
    assert_eq!(public_list.status(), reqwest::StatusCode::OK);

    let created = client
        .post(format!("{base}/api/jobs"))
        .header("Runtime-Id", "secret")
        .json(&serde_json::json!({
            "name": "api-created",
            "schedule": "0 0 1 1 *",
            "command": "echo first"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let status = client
        .get(format!("{base}/api/status"))
        .send()
        .await
        .unwrap()
        .json::<flashcron::scheduler::SchedulerState>()
        .await
        .unwrap();
    assert_eq!(status.total_jobs, 1);

    let mut job = client
        .get(format!("{base}/api/jobs/api-created"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    job["command"] = serde_json::json!("echo second");
    job["enabled"] = serde_json::json!(false);
    let updated = client
        .put(format!("{base}/api/jobs/api-created"))
        .header("Runtime-Id", "secret")
        .json(&job)
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), reqwest::StatusCode::OK);
    let protected_job = client
        .get(format!("{base}/api/jobs/api-created"))
        .send()
        .await
        .unwrap();
    assert_eq!(protected_job.status(), reqwest::StatusCode::UNAUTHORIZED);
    let job_detail = client
        .get(format!("{base}/api/jobs/api-created"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(job_detail["command"], "echo second");
    let jobs = client
        .get(format!("{base}/api/jobs"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert_eq!(jobs[0]["enabled"], false);

    let cli_list = std::process::Command::new(assert_cmd::cargo::cargo_bin("flashcron"))
        .arg("--config")
        .arg(&config_path)
        .arg("list")
        .output()
        .unwrap();
    assert!(cli_list.status.success());
    let cli_jobs = String::from_utf8_lossy(&cli_list.stdout);
    assert!(cli_jobs.contains("api-created"));
    assert!(!cli_jobs.contains("toml-only-job"));

    let saved_script = client
        .put(format!("{base}/api/scripts/hello.sh"))
        .header("Runtime-Id", "secret")
        .header("Content-Type", "text/plain")
        .body("#!/bin/sh\necho hello\n")
        .send()
        .await
        .unwrap();
    assert_eq!(saved_script.status(), reqwest::StatusCode::NO_CONTENT);
    let protected_scripts = client
        .get(format!("{base}/api/scripts"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        protected_scripts.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let protected_live_output = client
        .get(format!("{base}/api/jobs/api-created/output/stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        protected_live_output.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let no_active_output = client
        .get(format!("{base}/api/jobs/api-created/output/stream"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(no_active_output.status(), reqwest::StatusCode::CONFLICT);
    let enabled_for_stream = client
        .put(format!("{base}/api/jobs/api-created"))
        .header("Runtime-Id", "secret")
        .json(&serde_json::json!({
            "schedule": "0 0 1 1 *",
            "command": "printf stream_marker; sleep 0.5",
            "enabled": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(enabled_for_stream.status(), reqwest::StatusCode::OK);
    let triggered = client
        .post(format!("{base}/api/jobs/api-created/trigger"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(triggered.status(), reqwest::StatusCode::OK);
    let mut running = false;
    for _ in 0..50 {
        let jobs = client
            .get(format!("{base}/api/jobs"))
            .send()
            .await
            .unwrap()
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap();
        if jobs.iter().any(|job| job["run_state"] == "Running") {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(running, "triggered job did not enter Running state");
    let active_delete = client
        .delete(format!("{base}/api/jobs/api-created"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(active_delete.status(), reqwest::StatusCode::CONFLICT);
    let live_output = client
        .get(format!("{base}/api/jobs/api-created/output/stream"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(live_output.status(), reqwest::StatusCode::OK);
    let live_output = live_output.text().await.unwrap();
    assert!(live_output.contains("stream_marker"));
    assert!(live_output.contains("event: complete"));
    assert_eq!(
        fs::metadata(script_dir.join("hello.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    let deleted_script = client
        .delete(format!("{base}/api/scripts/hello.sh"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted_script.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(!script_dir.join("hello.sh").exists());

    for endpoint in ["jobs/api-created", "scripts/hello.sh"] {
        let unauthorized = client
            .delete(format!("{base}/api/{endpoint}"))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    let deleted_job = client
        .delete(format!("{base}/api/jobs/api-created"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted_job.status(), reqwest::StatusCode::NO_CONTENT);
    let jobs_after_delete = client
        .get(format!("{base}/api/jobs"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert!(jobs_after_delete.is_empty());
    let db = flashcron::db::DatabaseManager::init(db_path.to_str().unwrap())
        .await
        .unwrap();
    assert!(db.list_jobs().await.unwrap().is_empty());
    assert!(db.list("api-created", 50).await.unwrap().is_empty());

    let stop = flashcron::ipc::send_request_with_config(&config_path, &IpcRequest::Stop)
        .await
        .unwrap();
    assert!(matches!(stop, IpcResponse::Stopped));
    for _ in 0..50 {
        if child.0.try_wait().unwrap().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("daemon did not stop after test");
}

#[cfg(feature = "web")]
#[tokio::test]
async fn manual_output_stream_reconnects_after_queued_execution_starts() {
    let _ipc_guard = super::lock_ipc_tests();
    use std::time::Duration;

    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let (config_path, _, port) = write_test_web_config(&temp, None, Some(1));
    let _daemon = start_test_daemon(&config_path).await;
    let client = test_http_client();
    let base = format!("http://127.0.0.1:{port}");

    for (name, command) in [
        ("blocker", "sleep 0.3"),
        ("streamed", "printf stream_marker; sleep 0.2"),
    ] {
        let response = client
            .post(format!("{base}/api/jobs"))
            .header("Runtime-Id", "secret")
            .json(&serde_json::json!({
                "name": name,
                "schedule": "0 0 1 1 *",
                "command": command
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    }

    for name in ["blocker", "streamed"] {
        let response = client
            .post(format!("{base}/api/jobs/{name}/trigger"))
            .header("Runtime-Id", "secret")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        if name == "streamed" {
            let execution_id = response.json::<serde_json::Value>().await.unwrap()["execution_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let endpoint =
                format!("{base}/api/jobs/streamed/output/stream?execution_id={execution_id}");
            let queued = client
                .get(&endpoint)
                .header("Runtime-Id", "secret")
                .send()
                .await
                .unwrap();
            assert_eq!(queued.status(), reqwest::StatusCode::CONFLICT);

            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let jobs = client
                        .get(format!("{base}/api/jobs"))
                        .send()
                        .await
                        .unwrap()
                        .json::<Vec<serde_json::Value>>()
                        .await
                        .unwrap();
                    if jobs
                        .iter()
                        .any(|job| job["name"] == "streamed" && job["run_state"] == "Running")
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("queued execution did not start");

            let stream = client
                .get(endpoint)
                .header("Runtime-Id", "secret")
                .send()
                .await
                .unwrap();
            assert_eq!(stream.status(), reqwest::StatusCode::OK);
            let body = stream.text().await.unwrap();
            assert!(body.contains("stream_marker"));
            assert!(body.contains("event: complete"));
        }
    }
}

#[cfg(feature = "web")]
#[tokio::test]
async fn settings_api_persists_and_activates_failure_hook() {
    let _ipc_guard = super::lock_ipc_tests();
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let (config_path, _, port) = write_test_web_config(&temp, None, None);
    let hook_path = temp.path().join("hook.sh");
    let message_path = temp.path().join("hook-message");
    std::fs::write(
        &hook_path,
        format!("#!/bin/sh\ncat > '{}'\n", message_path.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _daemon = start_test_daemon(&config_path).await;
    let client = test_http_client();
    let base = format!("http://127.0.0.1:{port}");

    let saved = client
        .put(format!("{base}/api/settings"))
        .header("Runtime-Id", "secret")
        .json(&serde_json::json!({ "failure_hook": hook_path }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), reqwest::StatusCode::OK);
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(
        config["settings"]["failure_hook"].as_str(),
        hook_path.to_str()
    );

    let created = client
        .post(format!("{base}/api/jobs"))
        .header("Runtime-Id", "secret")
        .json(&serde_json::json!({
            "name": "failure",
            "schedule": "0 0 1 1 *",
            "command": "exit 1"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let triggered = client
        .post(format!("{base}/api/jobs/failure/trigger"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(triggered.status(), reqwest::StatusCode::OK);

    let message = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = tokio::fs::read(&message_path).await {
                if let Ok(message) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    break message;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("saved failure hook was not activated");
    assert_eq!(message["job_name"], "failure");
}

#[cfg(feature = "web")]
#[tokio::test]
async fn clearing_settings_api_hook_removes_persistence_and_disables_it() {
    let _ipc_guard = super::lock_ipc_tests();
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let hook_path = temp.path().join("hook.sh");
    let marker_path = temp.path().join("hook-called");
    std::fs::write(
        &hook_path,
        format!("#!/bin/sh\ntouch '{}'\n", marker_path.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (config_path, _, port) = write_test_web_config(&temp, Some(&hook_path), None);
    let _daemon = start_test_daemon(&config_path).await;
    let client = test_http_client();
    let base = format!("http://127.0.0.1:{port}");

    let cleared = client
        .put(format!("{base}/api/settings"))
        .header("Runtime-Id", "secret")
        .json(&serde_json::json!({ "failure_hook": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(cleared.status(), reqwest::StatusCode::OK);
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert!(config["settings"].get("failure_hook").is_none());

    let created = client
        .post(format!("{base}/api/jobs"))
        .header("Runtime-Id", "secret")
        .json(&serde_json::json!({
            "name": "failure",
            "schedule": "0 0 1 1 *",
            "command": "exit 1"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let triggered = client
        .post(format!("{base}/api/jobs/failure/trigger"))
        .header("Runtime-Id", "secret")
        .send()
        .await
        .unwrap();
    assert_eq!(triggered.status(), reqwest::StatusCode::OK);

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let jobs = client
                .get(format!("{base}/api/jobs"))
                .send()
                .await
                .unwrap()
                .json::<Vec<serde_json::Value>>()
                .await
                .unwrap();
            if jobs.iter().any(|job| {
                job["name"] == "failure"
                    && job["run_state"] == "Idle"
                    && job["last_status"].get("Failed").is_some()
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failed job did not finish");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!marker_path.exists());
}

#[cfg(feature = "web")]
async fn wait_for_history_entry(
    config_path: &std::path::Path,
    execution_id: uuid::Uuid,
) -> flashcron::config::JobExecution {
    use flashcron::ipc::{IpcRequest, IpcResponse};
    use std::time::Duration;

    for _ in 0..50 {
        match flashcron::ipc::send_request_with_config(
            config_path,
            &IpcRequest::HistoryGet { execution_id },
        )
        .await
        {
            Ok(IpcResponse::HistoryEntry {
                execution: Some(exec),
            }) => return exec,
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }

    panic!("history entry was not persisted in time");
}

#[cfg(feature = "web")]
async fn wait_for_recent_history_entry(
    config_path: &std::path::Path,
    job_name: &str,
) -> flashcron::config::JobExecution {
    use flashcron::ipc::{IpcRequest, IpcResponse};
    use std::time::Duration;

    for _ in 0..50 {
        match flashcron::ipc::send_request_with_config(
            config_path,
            &IpcRequest::HistoryList {
                job_name: Some(job_name.to_string()),
                limit: 1,
            },
        )
        .await
        {
            Ok(IpcResponse::HistoryEntries { history }) if !history.is_empty() => {
                return history.into_iter().next().unwrap();
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }

    panic!("history list did not return an entry in time");
}

#[cfg(feature = "web")]
fn assert_token_order(text: &str, tokens: &[&str]) {
    let mut last_index = None;

    for token in tokens {
        let index = text
            .find(token)
            .unwrap_or_else(|| panic!("missing token {}", token));
        if let Some(last_index) = last_index {
            assert!(index > last_index, "token {} is out of order", token);
        }
        last_index = Some(index);
    }
}

#[cfg(feature = "web")]
#[tokio::test]
async fn test_history_output_matches_logged_output_order() {
    let _ipc_guard = super::lock_ipc_tests();
    use flashcron::ipc::{IpcRequest, IpcResponse};
    use std::fs;
    use std::process::Stdio;

    let temp_dir = tempfile::tempdir_in("/tmp").unwrap();
    let config_path = temp_dir.path().join("flashcron.toml");
    let db_path = temp_dir.path().join("flashcron.db");
    fs::write(
        &config_path,
        format!(
            r#"
        [settings]
        sql_file = "{}"
        print_output = true
        job_history_size = 100
        max_history_size = 100

    "#,
            db_path.display(),
        ),
    )
    .unwrap();

    seed_web_job(
        &db_path,
        "sequence",
        serde_json::json!({
            "schedule": "0 0 1 1 *",
            "command": "printf 'stdout_first\\n'; sleep 0.05; printf 'stderr_first\\n' >&2; sleep 0.05; printf 'stdout_second\\n'; sleep 0.05; printf 'stderr_second\\n' >&2",
            "print_output": true
        }),
    )
    .await;

    let binary = assert_cmd::cargo::cargo_bin("flashcron");
    let child = std::process::Command::new(binary)
        .arg("--config")
        .arg(&config_path)
        .arg("run")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    wait_for_daemon_ready(&config_path).await;

    let trigger = flashcron::ipc::send_request_with_config(
        &config_path,
        &IpcRequest::Trigger {
            job_name: "sequence".to_string(),
        },
    )
    .await
    .unwrap();

    let execution_id = match trigger {
        IpcResponse::Triggered { execution_id } => execution_id,
        IpcResponse::Error { message } if message.contains("already queued or running") => {
            wait_for_recent_history_entry(&config_path, "sequence")
                .await
                .id
        }
        other => panic!("unexpected trigger response: {:?}", other),
    };

    let execution = wait_for_history_entry(&config_path, execution_id).await;
    let history_output = execution.output.unwrap_or_default();
    let tokens = [
        "stdout_first",
        "stderr_first",
        "stdout_second",
        "stderr_second",
    ];
    assert_token_order(&history_output, &tokens);

    let stop = flashcron::ipc::send_request_with_config(&config_path, &IpcRequest::Stop)
        .await
        .unwrap();
    assert!(matches!(stop, IpcResponse::Stopped));

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());

    let logs = String::from_utf8_lossy(&output.stdout);
    assert_token_order(&logs, &tokens);
}

#[cfg(feature = "web")]
#[tokio::test]
async fn test_history_is_not_persisted_when_max_history_size_is_zero() {
    let _ipc_guard = super::lock_ipc_tests();
    use flashcron::ipc::{IpcRequest, IpcResponse};
    use std::fs;
    use std::process::Stdio;

    let temp_dir = tempfile::tempdir_in("/tmp").unwrap();
    let config_path = temp_dir.path().join("flashcron.toml");
    let db_path = temp_dir.path().join("flashcron.db");
    fs::write(
        &config_path,
        format!(
            r#"
        [settings]
        sql_file = "{}"
        print_output = true
        job_history_size = 100
        max_history_size = 0

    "#,
            db_path.display(),
        ),
    )
    .unwrap();

    seed_web_job(
        &db_path,
        "sequence",
        serde_json::json!({
            "schedule": "0 0 1 1 *",
            "command": "printf 'no_history\\n'",
            "print_output": true
        }),
    )
    .await;

    let binary = assert_cmd::cargo::cargo_bin("flashcron");
    let mut child = std::process::Command::new(binary)
        .arg("--config")
        .arg(&config_path)
        .arg("run")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_for_daemon_ready(&config_path).await;

    let trigger = flashcron::ipc::send_request_with_config(
        &config_path,
        &IpcRequest::Trigger {
            job_name: "sequence".to_string(),
        },
    )
    .await
    .unwrap();

    let execution_id = match trigger {
        IpcResponse::Triggered { execution_id } => execution_id,
        other => panic!("unexpected trigger response: {:?}", other),
    };

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let history = flashcron::ipc::send_request_with_config(
        &config_path,
        &IpcRequest::HistoryGet { execution_id },
    )
    .await
    .unwrap();
    assert!(matches!(
        history,
        IpcResponse::HistoryEntry { execution: None }
    ));

    let stop = flashcron::ipc::send_request_with_config(&config_path, &IpcRequest::Stop)
        .await
        .unwrap();
    assert!(matches!(stop, IpcResponse::Stopped));

    let status = child.wait().unwrap();
    assert!(status.success());
}
