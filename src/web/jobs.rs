use super::*;

pub(super) async fn list_jobs(State(state): State<ApiState>) -> Json<Vec<ScheduledJob>> {
    let scheduler = state.scheduler_state.read().await;
    Json(scheduler.jobs.values().cloned().collect())
}

pub(super) async fn get_job(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let config = state.config.read().await;
    let job = config.jobs.get(&name).ok_or(StatusCode::NOT_FOUND)?;
    let mut value = serde_json::to_value(job).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    value
        .as_object_mut()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .insert("name".into(), serde_json::json!(name));
    Ok(Json(value))
}

pub(super) async fn stream_job_output(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Query(query): Query<OutputStreamQuery>,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    let mut events = state.handle.subscribe();
    let job = state
        .scheduler_state
        .read()
        .await
        .jobs
        .get(&name)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    let execution_id = if let Some(expected_id) = query.execution_id {
        if job.current_execution_id != Some(expected_id)
            || matches!(
                job.run_state,
                crate::scheduler::JobRunState::Queued | crate::scheduler::JobRunState::Starting
            )
        {
            return Err(StatusCode::CONFLICT);
        }
        expected_id
    } else {
        job.current_execution_id.ok_or(StatusCode::CONFLICT)?
    };
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<std::result::Result<Event, Infallible>>(8);

    tokio::spawn(async move {
        let mut sequence = job.output_sequence;
        if sender
            .send(Ok(Event::default()
                .event("output")
                .id(format!("{execution_id}:{sequence}"))
                .data(job.live_output)))
            .await
            .is_err()
        {
            return;
        }
        if job.run_state != crate::scheduler::JobRunState::Running {
            let _ = sender
                .send(Ok(Event::default()
                    .event("complete")
                    .data(execution_id.to_string())))
                .await;
            return;
        }

        loop {
            let event = tokio::select! {
                _ = sender.closed() => break,
                event = events.recv() => event,
            };
            match event {
                Ok(crate::scheduler::SchedulerEvent::JobOutput {
                    job_name,
                    execution_id: event_id,
                    sequence: event_sequence,
                    output,
                }) if job_name == name && event_id == execution_id && event_sequence > sequence => {
                    sequence = event_sequence;
                    let event = Event::default()
                        .event("output")
                        .id(format!("{execution_id}:{sequence}"))
                        .data(output);
                    if sender.send(Ok(event)).await.is_err() {
                        break;
                    }
                }
                Ok(crate::scheduler::SchedulerEvent::JobCompleted {
                    job_name,
                    execution_id: event_id,
                    ..
                }) if job_name == name && event_id == execution_id => {
                    let _ = sender
                        .send(Ok(Event::default()
                            .event("complete")
                            .data(execution_id.to_string())))
                        .await;
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let current = state.scheduler_state.read().await.jobs.get(&name).cloned();
                    let Some(current) = current else { break };
                    if current.output_sequence > sequence {
                        sequence = current.output_sequence;
                        let event = Event::default()
                            .event("output")
                            .id(format!("{execution_id}:{sequence}"))
                            .data(current.live_output);
                        if sender.send(Ok(event)).await.is_err() {
                            break;
                        }
                    }
                    if current.current_execution_id != Some(execution_id)
                        || current.run_state != crate::scheduler::JobRunState::Running
                    {
                        let _ = sender
                            .send(Ok(Event::default()
                                .event("complete")
                                .data(execution_id.to_string())))
                            .await;
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                _ => {}
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(receiver)).keep_alive(KeepAlive::default()))
}

#[derive(serde::Deserialize)]
pub(super) struct OutputStreamQuery {
    execution_id: Option<uuid::Uuid>,
}

pub(super) async fn create_job(
    State(state): State<ApiState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let mut payload = payload;
    let name = payload
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or((StatusCode::BAD_REQUEST, "Job name is required.".into()))?;
    payload.as_object_mut().unwrap().remove("name");
    save_job(state, name, payload, false, None).await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "status": "created" })),
    ))
}

pub(super) async fn update_job(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Json(mut payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let new_name = match payload.as_object_mut().and_then(|body| body.remove("name")) {
        Some(value) => value
            .as_str()
            .map(str::to_owned)
            .ok_or((StatusCode::BAD_REQUEST, "Job name must be a string.".into()))?,
        None => name.clone(),
    };
    let rename_from = (new_name != name).then_some(name);
    save_job(state, new_name, payload, true, rename_from).await?;
    Ok(Json(serde_json::json!({ "status": "updated" })))
}

pub(super) async fn save_job(
    state: ApiState,
    name: String,
    payload: serde_json::Value,
    update: bool,
    rename_from: Option<String>,
) -> Result<(), (StatusCode, String)> {
    if !valid_job_name(&name) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Use 1-128 letters, digits, dots, hyphens or underscores for the job name.".into(),
        ));
    }
    let job: Job = serde_json::from_value(payload)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let mut job = job;
    job.compile_schedule().map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid schedule: {error}"),
        )
    })?;
    job.validate(&name)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;

    state.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database is unavailable.".into(),
    ))?;
    if !state
        .handle
        .add_or_update_job(name, job, update, rename_from)
        .await
        .map_err(|error| match error {
            crate::error::Error::JobNotFound { .. } => (StatusCode::NOT_FOUND, error.to_string()),
            crate::error::Error::Scheduler(message) => (StatusCode::CONFLICT, message),
            _ => {
                log::error!(status = "job save failed", error = &*error.to_string(); "");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not save job to the database.".into(),
                )
            }
        })?
    {
        return Err(if update {
            (StatusCode::NOT_FOUND, "Job no longer exists.".into())
        } else {
            (
                StatusCode::CONFLICT,
                "A job with this name already exists.".into(),
            )
        });
    }
    Ok(())
}

pub(super) async fn delete_job(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .handle
        .remove_job(name)
        .await
        .map_err(|error| match error {
            crate::error::Error::JobNotFound { .. } => {
                (StatusCode::NOT_FOUND, "Job no longer exists.".into())
            }
            crate::error::Error::Scheduler(message) => (StatusCode::CONFLICT, message),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not delete job. Check the service logs.".into(),
            ),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn valid_job_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && name != "."
        && name != ".."
}

pub(super) async fn trigger_job(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let restore_phase = state.restore_status_rx.borrow().phase.clone();
    if restore_phase != PackageRestorePhase::Ready {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Wait for dependency restoration to finish.".into(),
        ));
    }
    // Note: Logging is handled inside handle.trigger_job (engine level)
    match state.handle.trigger_job(name).await {
        Ok(execution_id) => Ok(Json(
            serde_json::json!({ "status": "triggered", "execution_id": execution_id.to_string() }),
        )),
        Err(crate::error::Error::JobNotFound { .. }) => {
            Err((StatusCode::NOT_FOUND, "Job no longer exists.".into()))
        }
        Err(crate::error::Error::Scheduler(message)) => Err((StatusCode::CONFLICT, message)),
        Err(error) => {
            log::error!(status = "job trigger failed", error = &*error.to_string(); "");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not trigger job.".into(),
            ))
        }
    }
}
