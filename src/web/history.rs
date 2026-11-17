use super::*;

pub(super) async fn get_job_history(
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
                log::error!(job_name = &*name, status = "db list failed", error = &*e.to_string(); "");
            }
        }
    }

    Ok(Json(HistoryResponse {
        history: Vec::new(),
    }))
}

#[derive(Debug, Serialize)]
pub(super) struct HistoryResponse {
    history: Vec<crate::config::JobExecution>,
}

pub(super) async fn get_history(
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

pub(super) async fn get_execution(
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
