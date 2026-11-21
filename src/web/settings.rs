use super::*;

#[derive(serde::Serialize)]
pub(super) struct SettingsResponse {
    failure_hook: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingsUpdate {
    failure_hook: serde_json::Value,
}

pub(super) async fn get_settings(State(state): State<ApiState>) -> Json<SettingsResponse> {
    let config = state.config.read().await;
    Json(SettingsResponse {
        failure_hook: config
            .settings
            .failure_hook
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    })
}

pub(super) async fn update_settings(
    State(state): State<ApiState>,
    Json(payload): Json<SettingsUpdate>,
) -> Result<Json<SettingsResponse>, (StatusCode, String)> {
    let path = match payload.failure_hook {
        serde_json::Value::Null => None,
        serde_json::Value::String(path) => {
            let path = path.trim();
            (!path.is_empty()).then(|| std::path::PathBuf::from(path))
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Failure hook must be a path or null.".into(),
            ))
        }
    };

    state
        .handle
        .set_failure_hook(path.clone())
        .await
        .map_err(|error| {
            log::error!(status = "settings update failed", error = &*error.to_string(); "");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not save settings: {error}"),
            )
        })?;

    Ok(Json(SettingsResponse {
        failure_hook: path.map(|path| path.to_string_lossy().into_owned()),
    }))
}
