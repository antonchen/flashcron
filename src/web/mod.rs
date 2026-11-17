mod history;
mod jobs;
#[cfg(feature = "dep-manager")]
mod packages;
mod scripts;

use crate::config::Config;
use crate::config::Job;
use crate::db::DatabaseManager;
use crate::ipc::{PackageRestorePhase, PackageRestoreStatus};
use crate::scheduler::{ScheduledJob, SchedulerHandle, SchedulerState};
#[cfg(feature = "dep-manager")]
use axum::routing::delete;
use axum::{
    extract::{Path, Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
    pub config: Arc<RwLock<Config>>,
    pub scheduler_state: Arc<RwLock<SchedulerState>>,
    pub handle: SchedulerHandle,
    pub db: Option<DatabaseManager>,
    pub package_db: Arc<RwLock<Option<DatabaseManager>>>,
    pub sql_file: String,
    pub restore_status: tokio::sync::watch::Sender<PackageRestoreStatus>,
    pub restore_status_rx: tokio::sync::watch::Receiver<PackageRestoreStatus>,
}

pub async fn start_api_server(
    state: ApiState,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[allow(unused_mut)]
    let mut app = Router::new()
        .route("/", get(ui_handler))
        .route("/health", get(health_check))
        .route("/api/system/info", get(get_system_info))
        .route("/api/status", get(get_status))
        .route("/api/jobs", get(jobs::list_jobs).post(jobs::create_job))
        .route(
            "/api/jobs/{name}",
            get(jobs::get_job)
                .put(jobs::update_job)
                .delete(jobs::delete_job),
        )
        .route(
            "/api/jobs/{name}/output/stream",
            get(jobs::stream_job_output),
        )
        .route("/api/jobs/{name}/trigger", post(jobs::trigger_job))
        .route("/api/scripts", get(scripts::list_scripts))
        .route(
            "/api/scripts/{name}",
            get(scripts::read_script)
                .put(scripts::write_script)
                .delete(scripts::delete_script),
        )
        .route("/api/jobs/{name}/history", get(history::get_job_history))
        .route("/api/history", get(history::get_history))
        .route("/api/history/{execution_id}", get(history::get_execution));

    #[cfg(feature = "dep-manager")]
    {
        app = app
            .route(
                "/api/pkg",
                get(packages::list_packages)
                    .post(packages::add_package)
                    .delete(packages::delete_packages),
            )
            .route("/api/pkg/{id}", delete(packages::delete_package))
            .route(
                "/api/pkg/sources",
                get(packages::get_package_sources).put(packages::set_package_source),
            )
            .route(
                "/api/pkg/restore",
                get(packages::get_restore_status).post(packages::retry_package_restore),
            );
    }

    let app = app
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    log::info!(listening = &*format!("http://{}", addr); "");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn auth_middleware(
    State(state): State<ApiState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    #[cfg(feature = "dep-manager")]
    {
        let restore_phase = state.restore_status_rx.borrow().phase.clone();
        if restore_phase != PackageRestorePhase::Ready {
            let path = req.uri().path();
            let allowed = path == "/"
                || path == "/health"
                || path == "/api/pkg/restore"
                || (path == "/api/pkg" && req.method() == Method::GET);
            if !allowed {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        }
    }

    let config = state.config.read().await;

    // Job definitions and script contents are protected even though they use GET.
    if let Some(ref secret) = config.settings.api_token {
        let method = req.method();
        let path = req.uri().path();
        let package_list_during_restore = cfg!(feature = "dep-manager")
            && state.restore_status_rx.borrow().phase != PackageRestorePhase::Ready;
        let protected_read = path
            .strip_prefix("/api/jobs/")
            .is_some_and(|name| !name.contains('/'))
            || path
                .strip_prefix("/api/history/")
                .is_some_and(|execution_id| !execution_id.contains('/'))
            || path.ends_with("/output/stream")
            || path == "/api/scripts"
            || path.starts_with("/api/scripts/")
            || (path == "/api/pkg" && !package_list_during_restore);
        let protected_read = protected_read || path == "/api/pkg/sources";
        if (method != Method::GET && method != Method::HEAD) || protected_read {
            let provided = req
                .headers()
                .get("Runtime-Id")
                .and_then(|h| h.to_str().ok());

            if provided != Some(secret.as_str()) {
                let uri_str = req.uri().to_string();
                log::warn!(status = "unauthorized", method = method.as_str(), uri = uri_str.as_str(); "");
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }
    drop(config);

    Ok(next.run(req).await)
}

async fn ui_handler(State(state): State<ApiState>) -> axum::response::Html<String> {
    let restore = state.restore_status_rx.borrow().snapshot();
    if restore.phase == PackageRestorePhase::Ready {
        let html = include_str!(concat!(env!("OUT_DIR"), "/index.html"));
        let hidden = if cfg!(feature = "dep-manager") {
            ""
        } else {
            "hidden"
        };
        axum::response::Html(html.replace("__DEP_MANAGER_HIDDEN__", hidden))
    } else {
        axum::response::Html(format!(
            r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>FlashCron package restore</title>
<style>body{{font:16px system-ui;margin:0;background:#f4f6f8;color:#202a33}}main{{width:100%;max-width:1280px;box-sizing:border-box;margin:0 auto;padding:32px 16px}}@media(min-width:640px){{main{{padding-left:24px;padding-right:24px}}}}@media(min-width:1024px){{main{{padding-left:32px;padding-right:32px}}}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#111;color:#eee;padding:16px;max-height:65vh;overflow:auto}}button,input{{padding:10px 16px}}</style></head>
<body><main><h1>Package restore</h1><p id="phase">{}</p><p id="restore-error">{}</p>
<p id="status-error" role="alert"></p>
<form id="retry-form" hidden onsubmit="event.preventDefault();retryRestore()">
<label for="token">API token (leave blank to use the saved token)</label>
<input id="token" type="password" autocomplete="off">
<button id="retry" type="submit">Retry package restore</button>
<p id="retry-error" role="alert"></p></form><pre id="output"></pre></main>
<script>
const locale = navigator.languages?.[0] || navigator.language || 'en';
const isChinese = /^zh(?:-|$)/i.test(locale);
const translations = {{
    "FlashCron package restore": "FlashCron 依赖恢复",
    "Package restore": "依赖恢复",
    "API token (leave blank to use the saved token)": "API Token（留空则使用已保存的 Token）",
    "Retry package restore": "重试依赖恢复",
    "Tasks are paused until package restore succeeds.": "依赖恢复成功前，任务保持暂停。",
    "Status request failed (HTTP {{0}}).": "状态查询失败（HTTP {{0}}）。",
    "Invalid API token. Enter a valid token and retry.": "API Token 无效，请输入有效 Token 后重试。",
    "Retry failed (HTTP {{0}}).": "重试失败（HTTP {{0}}）。",
    "ready": "已就绪",
    "restoring": "正在恢复",
    "failed": "恢复失败"
}};
function t(message, ...values) {{
    const text = isChinese ? (translations[message] ?? message) : message;
    return text.replace(/\{{(\d+)\}}/g, (_, index) => String(values[index] ?? ''));
}}
document.documentElement.lang = isChinese ? 'zh-CN' : 'en';
document.title = t('FlashCron package restore');
document.querySelector('h1').textContent = t('Package restore');
document.querySelector('label[for="token"]').textContent = t('API token (leave blank to use the saved token)');
document.getElementById('retry').textContent = t('Retry package restore');
document.getElementById('phase').textContent = t(document.getElementById('phase').textContent);
const initialError = document.getElementById('restore-error');
if (initialError.textContent === 'Tasks are paused until package restore succeeds.') initialError.textContent = t(initialError.textContent);
let retrying = false;
async function refresh() {{
    try {{
        const response = await fetch('/api/pkg/restore');
        if (!response.ok) throw new Error(t('Status request failed (HTTP {{0}}).', response.status));
        const status = await response.json();
        document.getElementById('phase').textContent = t(status.phase);
        document.getElementById('restore-error').textContent = status.error || t('Tasks are paused until package restore succeeds.');
        document.getElementById('output').textContent = status.output || '';
        document.getElementById('retry-form').hidden = status.phase !== 'failed';
        document.getElementById('retry').disabled = retrying;
        document.getElementById('status-error').textContent = '';
        if (status.phase === 'ready') location.reload();
    }} catch (error) {{
        document.getElementById('status-error').textContent = error.message;
    }}
}}
async function retryRestore() {{
    if (retrying) return;
    retrying = true;
    document.getElementById('retry').disabled = true;
    document.getElementById('retry-error').textContent = '';
    try {{
        const token = document.getElementById('token').value.trim() || localStorage.getItem('fc_runtime_id');
        const response = await fetch('/api/pkg/restore', {{
            method: 'POST', headers: token ? {{'Runtime-Id': token}} : {{}}
        }});
        if (response.status === 401) throw new Error(t('Invalid API token. Enter a valid token and retry.'));
        if (!response.ok) throw new Error(t('Retry failed (HTTP {{0}}).', response.status));
        if (token) localStorage.setItem('fc_runtime_id', token);
        document.getElementById('token').value = '';
        await refresh();
    }} catch (error) {{
        document.getElementById('retry-error').textContent = error.message;
    }} finally {{
        retrying = false;
        document.getElementById('retry').disabled = false;
    }}
}}
refresh();
setInterval(refresh, 1000);
</script></body></html>"#,
            restore.phase.as_str(),
            escape_html(
                restore
                    .error
                    .as_deref()
                    .unwrap_or("Tasks are paused until package restore succeeds.")
            ),
        ))
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn health_check(State(state): State<ApiState>) -> Result<&'static str, StatusCode> {
    let phase = state.restore_status_rx.borrow().phase.clone();
    if phase == PackageRestorePhase::Ready {
        Ok("OK")
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

async fn get_status(State(state): State<ApiState>) -> Json<SchedulerState> {
    let mut s = state.scheduler_state.read().await.clone();
    s.update_time();
    Json(s)
}

async fn get_system_info() -> Json<serde_json::Value> {
    #[allow(unused_mut)]
    let mut features: Vec<&'static str> = vec![];
    #[cfg(feature = "dep-manager")]
    features.push("dep-manager");

    Json(serde_json::json!({
        "features": features
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_state(script_dir: PathBuf) -> ApiState {
        let mut config = Config::default();
        config.settings.script_dir = script_dir;
        let (_, handle) =
            crate::scheduler::Scheduler::new(config.clone(), PathBuf::from("test.toml"), None);
        let (_, restore_status_rx) = tokio::sync::watch::channel(PackageRestoreStatus::ready());
        let (restore_status, _) = tokio::sync::watch::channel(PackageRestoreStatus::ready());
        ApiState {
            config: Arc::new(RwLock::new(config)),
            scheduler_state: Arc::new(RwLock::new(SchedulerState::new())),
            handle,
            db: None,
            package_db: Arc::new(RwLock::new(None)),
            sql_file: String::new(),
            restore_status,
            restore_status_rx,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scripts_are_written_executable_and_paths_are_confined() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let state = test_state(directory.path().to_path_buf());
        assert_eq!(
            scripts::write_script(
                State(state.clone()),
                Path("sample.sh".into()),
                "#!/bin/sh\necho ok\n".into(),
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );
        let metadata = std::fs::metadata(directory.path().join("sample.sh")).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o755);
        assert_eq!(
            scripts::read_script(State(state.clone()), Path("sample.sh".into()))
                .await
                .unwrap()
                .0,
            "#!/bin/sh\necho ok\n"
        );
        assert_eq!(
            scripts::list_scripts(State(state)).await.unwrap().0[0].name,
            "sample.sh"
        );
        assert_eq!(
            scripts::script_path(directory.path(), "../outside"),
            Err(StatusCode::BAD_REQUEST)
        );
    }
}
