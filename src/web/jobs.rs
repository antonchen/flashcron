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
    let execution_id = job.current_execution_id.ok_or(StatusCode::CONFLICT)?;
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

pub(super) async fn create_job(
    State(state): State<ApiState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), StatusCode> {
    let mut payload = payload;
    let name = payload
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or(StatusCode::BAD_REQUEST)?;
    payload.as_object_mut().unwrap().remove("name");
    save_job(state, name, payload, false).await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "status": "created" })),
    ))
}

pub(super) async fn update_job(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Json(mut payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if let Some(body_name) = payload.as_object_mut().and_then(|body| body.remove("name")) {
        if body_name.as_str() != Some(name.as_str()) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    save_job(state, name, payload, true).await?;
    Ok(Json(serde_json::json!({ "status": "updated" })))
}

pub(super) async fn save_job(
    state: ApiState,
    name: String,
    payload: serde_json::Value,
    update: bool,
) -> Result<(), StatusCode> {
    if !valid_job_name(&name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let job: Job = serde_json::from_value(payload).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut job = job;
    job.compile_schedule()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    job.validate(&name).map_err(|_| StatusCode::BAD_REQUEST)?;

    state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    if !state
        .handle
        .add_or_update_job(name, job, update)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(if update {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::CONFLICT
        });
    }
    Ok(())
}

pub(super) async fn delete_job(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .handle
        .remove_job(name)
        .await
        .map_err(|error| match error {
            crate::error::Error::JobNotFound { .. } => StatusCode::NOT_FOUND,
            crate::error::Error::Scheduler(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
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
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let restore_phase = state.restore_status_rx.borrow().phase.clone();
    if restore_phase != PackageRestorePhase::Ready {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    // Note: Logging is handled inside handle.trigger_job (engine level)
    match state.handle.trigger_job(name).await {
        Ok(execution_id) => Ok(Json(
            serde_json::json!({ "status": "triggered", "execution_id": execution_id.to_string() }),
        )),
        Err(_) => Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
    }
}
