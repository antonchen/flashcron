use crate::config::Config;
use crate::scheduler::{SchedulerHandle, SchedulerState};
use axum::{
    extract::{Path, Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
    pub config: Arc<RwLock<Config>>,
    pub scheduler_state: Arc<RwLock<SchedulerState>>,
    pub handle: SchedulerHandle,
}

pub async fn start_api_server(
    state: ApiState,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(ui_handler))
        .route("/health", get(health_check))
        .route("/api/status", get(get_status))
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/{name}/trigger", post(trigger_job))
        .route("/api/history", get(get_history))
        .route("/api/history/{id}", get(get_execution))
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
    let config = state.config.read().await;

    // Only authenticate if api_token is configured
    if let Some(ref secret) = config.settings.api_token {
        let method = req.method();
        // Skip auth for GET and HEAD requests (read-only)
        if method != Method::GET && method != Method::HEAD {
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

    Ok(next.run(req).await)
}

async fn ui_handler() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("index.html"))
}

async fn health_check() -> &'static str {
    "OK"
}

async fn get_status(State(state): State<ApiState>) -> Json<SchedulerState> {
    let s = state.scheduler_state.read().await;
    Json(s.clone())
}

async fn list_jobs(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let s = state.scheduler_state.read().await;
    let jobs: Vec<_> = s.jobs.values().collect();
    Json(serde_json::json!(jobs))
}

async fn trigger_job(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    // Note: Logging is handled inside handle.trigger_job (engine level)
    match state.handle.trigger_job(name).await {
        Ok(execution_id) => Ok(Json(
            serde_json::json!({ "status": "triggered", "execution_id": execution_id.to_string() }),
        )),
        Err(_) => Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Debug, Serialize)]
struct HistoryResponse {
    history: Vec<crate::config::JobExecution>,
}

async fn get_history(
    State(state): State<ApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<HistoryResponse> {
    let s = state.scheduler_state.read().await;
    let job_name = params.get("job_name");
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(20);

    let history: Vec<_> = s
        .recent_history
        .iter()
        .filter(|exec| job_name.is_none_or(|name| exec.job_name == *name))
        .take(limit)
        .cloned()
        .collect();

    Json(HistoryResponse { history })
}

async fn get_execution(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::config::JobExecution>, axum::http::StatusCode> {
    let s = state.scheduler_state.read().await;
    let exec = s.recent_history.iter().find(|e| e.id == id).cloned();

    match exec {
        Some(e) => Ok(Json(e)),
        None => Err(axum::http::StatusCode::NOT_FOUND),
    }
}
