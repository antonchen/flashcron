use crate::scheduler::{SchedulerHandle, SchedulerState};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
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
        .route("/api/history", get(get_history))
        .route("/api/history/{id}", get(get_execution))
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    log::info!("API server listening on http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
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
