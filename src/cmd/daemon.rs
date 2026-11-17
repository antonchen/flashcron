use anyhow::{bail, Context, Result};
use flashcron::ipc::{self, IpcRequest, IpcResponse, PackageRestoreStatus, StatusPayload};
use flashcron::{Config, Scheduler};
#[cfg(feature = "web")]
use log::warn;
use log::{debug, error, info};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
#[cfg(feature = "web")]
use uuid::Uuid;

const INTERNAL_DAEMON_ENV: &str = "FLASHCRON_INTERNAL_DAEMON";

pub async fn run_daemon(
    config_path: PathBuf,
    daemon: bool,
    log_level: Option<String>,
    json: bool,
) -> Result<()> {
    if daemon && std::env::var_os(INTERNAL_DAEMON_ENV).is_none() {
        let mut child = spawn_background_process(log_level.as_deref(), json, &config_path)?;
        wait_for_socket_ready(&config_path, &mut child).await?;
        let socket_path = ipc_socket_path(Some(config_path.as_path()))?;
        info!(
            status = "background ready",
            config = config_path.to_string_lossy().as_ref(),
            socket = socket_path.to_string_lossy().as_ref();
            ""
        );
        return Ok(());
    }

    run_supervisor(config_path).await
}

async fn run_supervisor(config_path: PathBuf) -> Result<()> {
    info!(
        status = "starting",
        config = config_path.to_string_lossy().as_ref();
        ""
    );

    #[allow(unused_mut)]
    let mut config = {
        #[cfg(feature = "web")]
        {
            Config::from_file_settings(&config_path)
        }
        #[cfg(not(feature = "web"))]
        {
            Config::from_file(&config_path)
        }
    }
    .with_context(|| format!("Failed to load config from {:?}", config_path))?;

    #[cfg(feature = "web")]
    {
        if config.settings.api_token.is_none() {
            let token = Uuid::new_v4().to_string();
            persist_api_token(&config_path, &token)?;
            config.settings.api_token = Some(token);
        }
    }

    #[cfg(feature = "web")]
    let api_host = config.settings.api_host.clone();
    #[cfg(feature = "web")]
    let api_port = config.settings.api_port;
    #[cfg(feature = "web")]
    let sql_file = config.settings.sql_file.clone();

    #[cfg(feature = "web")]
    let db = Some(
        flashcron::db::DatabaseManager::init(&config.settings.sql_file)
            .await
            .context("Failed to initialize web database")?,
    );
    #[cfg(not(feature = "web"))]
    let db = None;

    #[cfg(feature = "web")]
    if let Some(ref db) = db {
        let mut jobs = db.list_jobs().await.context("Failed to load web jobs")?;
        for (name, job) in &mut jobs {
            job.compile_schedule().map_err(|reason| {
                anyhow::anyhow!("Invalid schedule for stored job '{name}': {reason}")
            })?;
            job.validate(name)?;
        }
        config.jobs = jobs;
        let active_jobs = config.jobs.keys().cloned().collect();
        if let Err(e) = db
            .cleanup(
                active_jobs,
                config.settings.job_history_size,
                config.settings.max_history_size,
            )
            .await
        {
            warn!(status = "db cleanup failed", error = &*e.to_string(); "");
        }
    }

    info!(
        status = "config loaded",
        total = config.jobs.len(),
        enabled = config.enabled_jobs().count();
        ""
    );

    let initial_restore_status = {
        #[cfg(feature = "dep-manager")]
        {
            if db.is_some() {
                PackageRestoreStatus::restoring()
            } else {
                PackageRestoreStatus {
                    phase: ipc::PackageRestorePhase::Failed,
                    output: Vec::new(),
                    error: Some("Package database is unavailable".to_string()),
                }
            }
        }
        #[cfg(not(feature = "dep-manager"))]
        {
            PackageRestoreStatus::ready()
        }
    };
    #[cfg(any(feature = "web", feature = "dep-manager"))]
    let (restore_status, restore_status_rx) = tokio::sync::watch::channel(initial_restore_status);
    #[cfg(not(any(feature = "web", feature = "dep-manager")))]
    let (_, restore_status_rx) = tokio::sync::watch::channel(initial_restore_status);
    #[cfg(feature = "web")]
    let package_db = std::sync::Arc::new(tokio::sync::RwLock::new(db.clone()));

    let shutdown_timeout = config.settings.shutdown_timeout;
    let (socket_path, listener) = ipc::bind_listener(&config.settings).await?;
    info!(
        status = "ipc listening",
        socket = socket_path.to_string_lossy().as_ref();
        ""
    );
    let (scheduler, handle) = Scheduler::new_with_restore_status(
        config,
        config_path.clone(),
        db.clone(),
        restore_status_rx.clone(),
    );

    let ipc_task = {
        let handle = handle.clone();
        let config_path = config_path.clone();
        let db = db.clone();
        let restore_status_rx = restore_status_rx.clone();
        tokio::spawn(async move {
            if let Err(e) =
                start_ipc_server(listener, handle, config_path, db, restore_status_rx).await
            {
                error!(status = "ipc server error", error = &*e.to_string(); "");
            }
        })
    };

    #[cfg(feature = "web")]
    let api_task = {
        let api_state = flashcron::web::ApiState {
            config: scheduler.get_config(),
            scheduler_state: scheduler.get_state(),
            handle: handle.clone(),
            db: db.clone(),
            package_db: package_db.clone(),
            sql_file: sql_file.clone(),
            restore_status: restore_status.clone(),
            restore_status_rx: restore_status_rx.clone(),
        };
        Some(tokio::spawn(async move {
            if let Err(e) = flashcron::web::start_api_server(api_state, &api_host, api_port).await {
                error!(status = "api server error", error = &*e.to_string(); "");
            }
        }))
    };
    #[cfg(not(feature = "web"))]
    let api_task: Option<tokio::task::JoinHandle<()>> = None;

    let reload_handle = handle.clone();
    let watch_path = config_path.clone();
    let watcher_task = tokio::spawn(async move {
        if let Err(e) = watch_config_file(watch_path, reload_handle).await {
            error!(status = "config watcher error", error = &*e.to_string(); "");
        }
    });

    let scheduler_handle_for_sig = handle.clone();
    let signal_task = tokio::spawn(async move {
        if let Err(e) = wait_for_shutdown_signal(shutdown_timeout).await {
            error!(status = "signal handler error", error = &*e.to_string(); "");
        }
        let _ = scheduler_handle_for_sig.shutdown().await;
    });

    #[allow(unused_mut)]
    let mut scheduler_task = tokio::spawn(scheduler.run());

    #[cfg(feature = "dep-manager")]
    {
        if let Some(db) = package_db.read().await.clone() {
            let restore_tx = restore_status.clone();
            let mut restore_task = tokio::spawn(async move {
                flashcron::dep_manager::restore_packages(&db, restore_tx).await
            });
            tokio::select! {
                result = &mut restore_task => {
                    if let Ok(Err(e)) = result {
                        error!(status = "package restore failed", error = &*e.to_string(); "");
                    }
                }
                result = &mut scheduler_task => {
                    if let Ok(Err(e)) = result {
                        error!(status = "scheduler error", error = &*e.to_string(); "");
                    }
                    restore_task.abort();
                }
            }
        }
    }

    if !scheduler_task.is_finished() {
        if let Err(e) = scheduler_task.await {
            error!(status = "scheduler task failed", error = &*e.to_string(); "");
        }
    }

    watcher_task.abort();
    ipc_task.abort();
    signal_task.abort();
    if let Some(task) = api_task {
        task.abort();
    }
    let _ = std::fs::remove_file(&socket_path);

    info!(status = "stopped"; "");
    Ok(())
}

#[cfg(feature = "web")]
fn persist_api_token(config_path: &Path, token: &str) -> Result<()> {
    let content = std::fs::read_to_string(config_path)?;
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .context("Failed to parse config for API token persistence")?;
    if !document["settings"].is_table() {
        document["settings"] = toml_edit::table();
    }
    document["settings"]["api_token"] = toml_edit::value(token);
    std::fs::write(config_path, document.to_string())
        .context("Failed to persist generated API token")
}

fn spawn_background_process(
    log_level: Option<&str>,
    json: bool,
    config_path: &Path,
) -> Result<std::process::Child> {
    use std::os::unix::process::CommandExt;

    let executable = std::env::current_exe()?;
    let mut command = std::process::Command::new(executable);
    command.env(INTERNAL_DAEMON_ENV, "1");
    command.arg("--config").arg(config_path);
    if let Some(level) = log_level {
        command.arg("--log-level").arg(level);
    }
    if json {
        command.arg("--json");
    }
    command.arg("run");
    command.stdin(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::other)
        });
    }

    command
        .spawn()
        .with_context(|| "Failed to spawn background daemon")
}

async fn wait_for_socket_ready(config_path: &Path, child: &mut std::process::Child) -> Result<()> {
    for _ in 0..50 {
        match ipc::send_request_with_config(config_path, &IpcRequest::Status).await {
            Ok(IpcResponse::Status(_)) => return Ok(()),
            Ok(_) => break,
            Err(_) => {}
        }

        if let Some(status) = child.try_wait()? {
            bail!("Background daemon exited early with status {}", status);
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    bail!("Timed out waiting for background daemon to become ready")
}

async fn start_ipc_server(
    listener: UnixListener,
    handle: flashcron::scheduler::SchedulerHandle,
    config_path: PathBuf,
    db: Option<flashcron::db::DatabaseManager>,
    restore_status: tokio::sync::watch::Receiver<PackageRestoreStatus>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let handle = handle.clone();
        let config_path = config_path.clone();
        let db = db.clone();
        let restore_status = restore_status.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_ipc_client(stream, handle, config_path, db, restore_status).await
            {
                error!(status = "ipc client failed", error = &*e.to_string(); "");
            }
        });
    }
}

async fn handle_ipc_client(
    stream: UnixStream,
    handle: flashcron::scheduler::SchedulerHandle,
    config_path: PathBuf,
    db: Option<flashcron::db::DatabaseManager>,
    restore_status: tokio::sync::watch::Receiver<PackageRestoreStatus>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let request: IpcRequest = serde_json::from_str(line.trim_end())?;

    let response = match request {
        IpcRequest::Status => {
            let state = handle.get_status().await?;
            IpcResponse::Status(StatusPayload {
                config_path: config_path.to_string_lossy().into_owned(),
                state,
            })
        }
        IpcRequest::PackageRestoreStatus => {
            let status = restore_status.borrow().snapshot();
            IpcResponse::PackageRestoreStatus(status)
        }
        IpcRequest::Trigger { job_name } => match handle.trigger_job(job_name).await {
            Ok(execution_id) => IpcResponse::Triggered { execution_id },
            Err(e) => IpcResponse::Error {
                message: e.to_string(),
            },
        },
        IpcRequest::Stop => {
            handle.shutdown().await?;
            IpcResponse::Stopped
        }
        IpcRequest::HistoryList { job_name, limit } => {
            let history = match db {
                Some(ref db) => db.list(job_name.as_deref().unwrap_or(""), limit).await?,
                None => Vec::new(),
            };
            IpcResponse::HistoryEntries { history }
        }
        IpcRequest::HistoryGet { execution_id } => {
            let execution = match db {
                Some(ref db) => db.get(execution_id).await?,
                None => None,
            };
            IpcResponse::HistoryEntry { execution }
        }
    };

    let mut payload = serde_json::to_vec(&response)?;
    payload.push(b'\n');
    write_half.write_all(&payload).await?;
    write_half.shutdown().await?;

    Ok(())
}

pub(super) fn ipc_socket_path(config_path: Option<&Path>) -> Result<PathBuf> {
    let _ = config_path;
    Ok(ipc::default_socket_path())
}

pub(super) async fn send_ipc_request(
    config_path: Option<&Path>,
    request: &IpcRequest,
) -> Result<IpcResponse> {
    match config_path {
        Some(path) => Ok(ipc::send_request_with_config(path, request).await?),
        None => Ok(ipc::send_request(request).await?),
    }
}

async fn wait_for_shutdown_signal(timeout_secs: u64) -> Result<()> {
    let timeout_str = format!("{}s", timeout_secs);
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sighup = signal(SignalKind::hangup())?;

    tokio::select! {
        res = sigterm.recv() => { if res.is_none() { return Ok(()); } info!(status = "shutting down", signal = "SIGTERM", timeout = &*timeout_str; ""); },
        res = sigint.recv() => { if res.is_none() { return Ok(()); } info!(status = "shutting down", signal = "SIGINT", timeout = &*timeout_str; ""); },
        res = sighup.recv() => { if res.is_none() { return Ok(()); } info!(status = "shutting down", signal = "SIGHUP", timeout = &*timeout_str; ""); },
    }

    Ok(())
}

async fn watch_config_file(
    path: PathBuf,
    handle: flashcron::scheduler::SchedulerHandle,
) -> Result<()> {
    use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    let watch_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let target_name = path
        .file_name()
        .map(|name| name.to_owned())
        .ok_or_else(|| anyhow::anyhow!("Config file name is missing"))?;

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        NotifyConfig::default().with_poll_interval(Duration::from_secs(2)),
    )?;

    watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
    info!(
        status = "watching config",
        file = path.to_string_lossy().as_ref();
        ""
    );

    loop {
        match rx.try_recv() {
            Ok(Ok(event)) => {
                let is_target = event
                    .paths
                    .iter()
                    .any(|event_path| event_path.file_name() == Some(target_name.as_os_str()));
                let is_reload_event = matches!(
                    event.kind,
                    notify::EventKind::Any
                        | notify::EventKind::Create(_)
                        | notify::EventKind::Modify(_)
                        | notify::EventKind::Remove(_)
                );
                if is_target && is_reload_event {
                    debug!(
                        status = "config changed",
                        file = path.to_string_lossy().as_ref();
                        ""
                    );
                    while rx.try_recv().is_ok() {}
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if let Err(e) = handle.reload_config().await {
                        error!(status = "config reload failed", error = &*e.to_string(); "");
                    }
                }
            }
            Ok(Err(e)) => {
                error!(status = "watch error", error = &*format!("{:?}", e); "");
            }
            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }

    Ok(())
}
