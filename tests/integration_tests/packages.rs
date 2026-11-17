#[cfg(feature = "dep-manager")]
#[tokio::test]
async fn test_package_restore_stays_queryable_and_retries_before_startup_jobs() {
    let _ipc_guard = super::lock_ipc_tests();
    use flashcron::db::DatabaseManager;
    use flashcron::dep_manager::{PkgStatus, PkgType};
    use flashcron::ipc::{IpcRequest, IpcResponse};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    use std::time::Duration;

    struct DaemonGuard(std::process::Child);
    impl Drop for DaemonGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let temp_dir = tempfile::tempdir_in("/tmp").unwrap();
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let count_file = temp_dir.path().join("pip-install-count");
    let pip = bin_dir.join("pip");
    let index_file = temp_dir.path().join("pip-index");
    let config_error = temp_dir.path().join("pip-config-error");
    let install_args = temp_dir.path().join("pip-install-args");
    fs::write(
        &pip,
        format!(
            r#"#!/bin/sh
if [ "$1" = config ]; then
    if [ "$2" = get ] && [ "$3" = global.index-url ]; then
        if [ -f '{config_error}' ]; then
            echo 'fixture config read failed' >&2
            exit 2
        fi
        if [ -f '{index_file}' ]; then cat '{index_file}'; exit 0; fi
        echo 'ERROR: No such key - global.index-url' >&2
        exit 1
    fi
    if [ "$2" = --site ] && [ "$3" = set ] && [ "$4" = global.index-url ]; then
        printf '%s\n' "$5" > '{index_file}'
        exit 0
    fi
    exit 2
fi
if [ "$1" = show ]; then echo 'Version: 1.0'; exit 0; fi
[ "$1" = install ] || exit 2
printf '%s\n' "$@" > '{install_args}'
n=0
[ ! -f '{count_file}' ] || n=$(cat '{count_file}')
n=$((n+1))
echo $n > '{count_file}'
printf 'attempt-%s-out\n' "$n"
printf 'attempt-%s-err\n' "$n" >&2
if [ "$n" -eq 1 ]; then sleep 0.3; exit 1; fi
sleep 1
exit 0
"#,
            config_error = config_error.display(),
            index_file = index_file.display(),
            install_args = install_args.display(),
            count_file = count_file.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&pip, fs::Permissions::from_mode(0o755)).unwrap();

    let db_path = temp_dir.path().join("flashcron.db");
    let db = DatabaseManager::init(db_path.to_str().unwrap())
        .await
        .unwrap();
    let package_id = db
        .add_packages(PkgType::PIP, vec![("restore-fixture".into(), None)])
        .await
        .unwrap();
    assert_eq!(
        db.add_packages(
            PkgType::PIP,
            vec![("restore-fixture".into(), Some("2.0".into()))]
        )
        .await
        .unwrap(),
        package_id
    );
    assert_eq!(
        db.list_packages().await.unwrap()[0].target.as_deref(),
        Some("2.0")
    );
    let packages_before_failed_delete = db.list_packages().await.unwrap();
    assert!(db
        .delete_packages(vec![package_id[0], i64::MAX])
        .await
        .is_err());
    assert_eq!(
        db.list_packages().await.unwrap().len(),
        packages_before_failed_delete.len()
    );
    db.add_packages(PkgType::PIP, vec![("restore-fixture".into(), None)])
        .await
        .unwrap();
    db.update_package_status(
        package_id[0],
        PkgStatus::Installed,
        None,
        Some("1.0".into()),
    )
    .await
    .unwrap();
    drop(db);

    let config_path = temp_dir.path().join("flashcron.toml");
    let started_marker = temp_dir.path().join("startup-job-ran");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    fs::write(
        &config_path,
        format!(
            "[settings]\nsql_file = {:?}\napi_host = \"127.0.0.1\"\napi_port = {}\napi_token = \"test-token\"\n",
            db_path,
            port,
        ),
    )
    .unwrap();

    seed_web_job(
        &db_path,
        "startup",
        serde_json::json!({
            "schedule": "0 0 1 1 *",
            "command": format!("touch '{}'", started_marker.display()),
            "run_on_startup": true
        }),
    )
    .await;

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());
    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("flashcron"))
        .arg("--config")
        .arg(&config_path)
        .arg("run")
        .env("PATH", test_path)
        .env("VIRTUAL_ENV", temp_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = DaemonGuard(child);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap();
    let restore_url = format!("http://127.0.0.1:{port}/api/pkg/restore");
    let mut failed_status = None;
    let mut saw_restoring = false;

    for _ in 0..10 {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("daemon exited before restore status was available: {status}");
        }
        if let Ok(response) = client.get(&restore_url).send().await {
            if let Ok(status) = response
                .json::<flashcron::ipc::PackageRestoreSnapshot>()
                .await
            {
                saw_restoring |= status.phase == flashcron::ipc::PackageRestorePhase::Restoring;
                if status.phase == flashcron::ipc::PackageRestorePhase::Failed {
                    failed_status = Some(status);
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let failed_status = failed_status.expect("first restore attempt did not fail");
    assert!(saw_restoring, "restore progress was not observable");
    let ipc_status = tokio::time::timeout(
        Duration::from_secs(1),
        flashcron::ipc::send_request_with_config(&config_path, &IpcRequest::PackageRestoreStatus),
    )
    .await
    .expect("IPC restore status request timed out")
    .unwrap();
    assert!(matches!(
        ipc_status,
        IpcResponse::PackageRestoreStatus(status)
            if status.phase == flashcron::ipc::PackageRestorePhase::Failed
    ));
    assert!(failed_status.output.contains("attempt-1-out"));
    assert!(failed_status.output.contains("attempt-1-err"));
    assert!(
        !started_marker.exists(),
        "startup task ran while restore failed"
    );
    let health = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let dashboard = client
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(dashboard.contains("Package restore"));
    let jobs = client
        .get(format!("http://127.0.0.1:{port}/api/jobs"))
        .send()
        .await
        .unwrap();
    assert_eq!(jobs.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);

    assert_eq!(fs::read_to_string(&count_file).unwrap().trim(), "1");
    assert!(!fs::read_to_string(&install_args)
        .unwrap()
        .lines()
        .any(|arg| arg == "--index-url"));

    fs::write(&config_error, "").unwrap();
    let retry = client
        .post(&restore_url)
        .header("Runtime-Id", "test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), reqwest::StatusCode::ACCEPTED);
    let mut config_read_failed = false;
    for _ in 0..30 {
        let status = client
            .get(&restore_url)
            .send()
            .await
            .unwrap()
            .json::<flashcron::ipc::PackageRestoreSnapshot>()
            .await
            .unwrap();
        if status.phase == flashcron::ipc::PackageRestorePhase::Failed {
            assert!(status.error.unwrap().contains("fixture config read failed"));
            config_read_failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(config_read_failed, "config read error did not fail restore");
    assert_eq!(fs::read_to_string(&count_file).unwrap().trim(), "1");
    assert!(!started_marker.exists());

    fs::remove_file(&config_error).unwrap();
    let mirror = "https://mirror.example/simple";
    fs::write(&index_file, mirror).unwrap();
    let retry = client
        .post(&restore_url)
        .header("Runtime-Id", "test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), reqwest::StatusCode::ACCEPTED);

    let mut ready_status = None;
    for _ in 0..30 {
        if let Ok(response) = client.get(&restore_url).send().await {
            if let Ok(status) = response
                .json::<flashcron::ipc::PackageRestoreSnapshot>()
                .await
            {
                if status.phase == flashcron::ipc::PackageRestorePhase::Ready {
                    ready_status = Some(status);
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ready_status = ready_status.expect("retry did not finish successfully");
    assert!(
        ready_status.output.is_empty(),
        "successful restore output was retained"
    );
    let dashboard = client
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(dashboard.contains("id=\"packages-view\""));
    assert!(dashboard.contains("id=\"nav-packages\""));
    let packages_nav_start = dashboard
        .find("id=\"nav-packages\"")
        .and_then(|index| dashboard[..index].rfind("<button"))
        .expect("Packages navigation is missing");
    let packages_nav = dashboard[packages_nav_start..].split('>').next().unwrap();
    assert!(packages_nav.contains("rounded"));
    assert!(!packages_nav.contains("hidden"));

    for _ in 0..50 {
        if started_marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        started_marker.exists(),
        "startup job did not run after restore"
    );

    let ipc_status =
        flashcron::ipc::send_request_with_config(&config_path, &IpcRequest::PackageRestoreStatus)
            .await
            .unwrap();
    assert!(matches!(
        ipc_status,
        IpcResponse::PackageRestoreStatus(status)
            if status.phase == flashcron::ipc::PackageRestorePhase::Ready
    ));

    let packages = client
        .get(format!("http://127.0.0.1:{port}/api/pkg"))
        .header("Runtime-Id", "test-token")
        .send()
        .await
        .unwrap()
        .json::<Vec<flashcron::dep_manager::ManagedPackage>>()
        .await
        .unwrap();
    assert_eq!(packages[0].status, PkgStatus::Installed);

    assert_eq!(fs::read_to_string(&count_file).unwrap().trim(), "2");
    let args = fs::read_to_string(&install_args).unwrap();
    let args: Vec<_> = args.lines().collect();
    assert!(args.windows(2).any(|pair| pair == ["--index-url", mirror]));

    let saved_mirror = "https://saved.example/simple";
    let saved = client
        .put(format!("http://127.0.0.1:{port}/api/pkg/sources"))
        .header("Runtime-Id", "test-token")
        .json(&serde_json::json!({"kind": "pip", "value": saved_mirror}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), reqwest::StatusCode::NO_CONTENT);
    assert_eq!(
        fs::read_to_string(&index_file).unwrap().trim(),
        saved_mirror
    );

    let stop = flashcron::ipc::send_request_with_config(&config_path, &IpcRequest::Stop)
        .await
        .unwrap();
    assert!(matches!(stop, IpcResponse::Stopped));
    assert!(child.0.wait().unwrap().success());
}
use super::seed_web_job;
