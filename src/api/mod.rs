use crate::config::Config;
use crate::db::DatabaseManager;
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
    pub db: Option<DatabaseManager>,
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
        .route("/api/jobs/{name}/history", get(get_job_history))
        .route("/api/history", get(get_history))
        .route("/api/history/{execution_id}", get(get_execution))
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
    let mut s = state.scheduler_state.read().await.clone();
    s.update_time();
    Json(s)
}

async fn list_jobs(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let s = state.scheduler_state.read().await;
    let jobs: Vec<_> = s.jobs.values().collect();
    Json(serde_json::json!(jobs))
}

async fn get_job_history(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<HistoryResponse>, axum::http::StatusCode> {
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(50);

    if let Some(ref db) = state.db {
        match db.list(&name, limit).await {
            Ok(history) => return Ok(Json(HistoryResponse { history })),
            Err(e) => {
                log::error!(status = "db list failed", job = &*name, error = &*e.to_string(); "");
            }
        }
    }

    Ok(Json(HistoryResponse {
        history: Vec::new(),
    }))
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
) -> Result<Json<HistoryResponse>, axum::http::StatusCode> {
    let job_name = params.get("job_name");
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(50);

    // Prefer Database for global history
    if let Some(ref db) = state.db {
        // If job_name is None, we need a global list method in db.rs
        // I'll update db.rs to handle empty job_name in list()
        match db
            .list(job_name.map(|s| s.as_str()).unwrap_or(""), limit)
            .await
        {
            Ok(history) => return Ok(Json(HistoryResponse { history })),
            Err(e) => {
                log::error!(status = "db history failed", error = &*e.to_string(); "");
            }
        }
    }

    // Memory fallback is now always empty since we removed recent_history
    Ok(Json(HistoryResponse {
        history: Vec::new(),
    }))
}

async fn get_execution(
    State(state): State<ApiState>,
    Path(execution_id): Path<Uuid>,
) -> Result<Json<crate::config::JobExecution>, axum::http::StatusCode> {
    // 1. Try memory first (Active executions could be added here later)

    // 2. Fallback to database
    if let Some(ref db) = state.db {
        match db.get(execution_id).await {
            Ok(Some(exec)) => return Ok(Json(exec)),
            Ok(None) => return Err(axum::http::StatusCode::NOT_FOUND),
            Err(e) => {
                log::error!(status = "db lookup failed", error = &*e.to_string(); "");
                return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }

    Err(axum::http::StatusCode::NOT_FOUND)
}
