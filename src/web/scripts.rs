use super::*;

#[derive(Serialize)]
pub(super) struct ScriptEntry {
    pub(super) name: String,
    pub(super) path: String,
}

pub(super) async fn list_scripts(
    State(state): State<ApiState>,
) -> Result<Json<Vec<ScriptEntry>>, StatusCode> {
    let configured_dir = state.config.read().await.settings.script_dir.clone();
    let script_dir = if configured_dir.is_absolute() {
        configured_dir
    } else {
        std::env::current_dir()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .join(configured_dir)
    };
    let mut entries = match tokio::fs::read_dir(&script_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Json(Vec::new())),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let mut names = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        if entry
            .file_type()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .is_file()
        {
            if let Some(name) = entry.file_name().to_str() {
                names.push(ScriptEntry {
                    name: name.to_owned(),
                    path: script_dir.join(name).display().to_string(),
                });
            }
        }
    }
    names.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Json(names))
}

pub(super) async fn read_script(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<String>, StatusCode> {
    let script_dir = state.config.read().await.settings.script_dir.clone();
    let path = script_path(&script_dir, &name)?;
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !metadata.file_type().is_file() {
        return Err(StatusCode::NOT_FOUND);
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(content))
}

pub(super) async fn write_script(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    body: String,
) -> Result<StatusCode, StatusCode> {
    let script_dir = state.config.read().await.settings.script_dir.clone();
    let path = script_path(&script_dir, &name)?;
    tokio::fs::create_dir_all(&script_dir)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let temp_path = script_dir.join(format!(".{}.{}.tmp", name, Uuid::new_v4()));
    if tokio::fs::write(&temp_path, body).await.is_err() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o755)).is_err() {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    if tokio::fs::rename(&temp_path, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete_script(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let script_dir = state.config.read().await.settings.script_dir.clone();
    tokio::fs::remove_file(script_path(&script_dir, &name)?)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn script_path(directory: &FsPath, name: &str) -> Result<PathBuf, StatusCode> {
    if !valid_script_name(name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(directory.join(name))
}

pub(super) fn valid_script_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && name != "."
        && name != ".."
}
