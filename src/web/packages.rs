use super::*;

#[cfg(feature = "dep-manager")]
#[derive(serde::Deserialize)]
pub(super) struct AddPackageRequest {
    pkg_type: String,
    packages: Vec<PackageSpec>,
}

#[cfg(feature = "dep-manager")]
#[derive(serde::Deserialize)]
pub(super) struct PackageSpec {
    name: String,
    target: Option<String>,
}

#[cfg(feature = "dep-manager")]
#[derive(serde::Deserialize)]
pub(super) struct SetPackageSourceRequest {
    kind: String,
    value: String,
}

#[cfg(feature = "dep-manager")]
pub(super) async fn get_package_sources(
) -> Result<Json<crate::dep_manager::PackageSources>, StatusCode> {
    crate::dep_manager::package_sources()
        .await
        .map(Json)
        .map_err(|error| {
            log::error!(status = "package source read failed", error = &*error.to_string(); "");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

#[cfg(feature = "dep-manager")]
pub(super) async fn set_package_source(
    axum::extract::Json(payload): axum::extract::Json<SetPackageSourceRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    crate::dep_manager::set_package_source(&payload.kind, &payload.value)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            let status = match error.kind() {
                std::io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
                std::io::ErrorKind::NotFound
                | std::io::ErrorKind::PermissionDenied
                | std::io::ErrorKind::Unsupported => {
                    StatusCode::CONFLICT
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            log::error!(status = "package source update failed", source_type = payload.kind.as_str(); "");
            (status, error.to_string())
        })
}

#[cfg(feature = "dep-manager")]
pub(super) async fn list_packages(
    State(state): State<ApiState>,
) -> Result<Json<Vec<crate::dep_manager::ManagedPackage>>, axum::http::StatusCode> {
    let db = state.package_db.read().await.clone();
    if let Some(ref db) = db {
        match db.list_packages().await {
            Ok(pkgs) => Ok(Json(pkgs)),
            Err(e) => {
                log::error!(status = "db list_packages failed", error = &*e.to_string(); "");
                Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    } else {
        Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[cfg(feature = "dep-manager")]
pub(super) async fn add_package(
    State(state): State<ApiState>,
    axum::extract::Json(payload): axum::extract::Json<AddPackageRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    use std::str::FromStr;

    let restore_phase = state.restore_status_rx.borrow().phase.clone();
    if restore_phase != PackageRestorePhase::Ready {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Package restore is not ready".into(),
        ));
    }

    let pkg_type = crate::dep_manager::PkgType::from_str(&payload.pkg_type)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;

    if payload.packages.is_empty()
        || payload
            .packages
            .iter()
            .any(|package| !crate::dep_manager::is_valid_pkg_name(&package.name))
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid package name".to_string(),
        ));
    }

    for package in &payload.packages {
        if package
            .target
            .as_ref()
            .is_some_and(|target| !crate::dep_manager::is_valid_pkg_version(target))
        {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                "Invalid package version".to_string(),
            ));
        }
    }

    let db = state.package_db.read().await.clone();
    if let Some(ref db) = db {
        match db
            .add_packages(
                pkg_type,
                payload
                    .packages
                    .into_iter()
                    .map(|package| (package.name, package.target))
                    .collect(),
            )
            .await
        {
            Ok(ids) => {
                // Trigger background sync
                let db_clone = db.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::dep_manager::sync_packages(&db_clone).await {
                        log::error!(status = "background sync failed", error = &*e.to_string(); "");
                    }
                });

                Ok(Json(serde_json::json!({ "ids": ids, "status": "added" })))
            }
            Err(e) => {
                log::error!(status = "db add_package failed", error = &*e.to_string(); "");
                Err((
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                ))
            }
        }
    } else {
        Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Database not initialized".to_string(),
        ))
    }
}

#[cfg(feature = "dep-manager")]
pub(super) async fn delete_package(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    delete_packages(State(state), Json(vec![id])).await
}

#[cfg(feature = "dep-manager")]
pub(super) async fn delete_packages(
    State(state): State<ApiState>,
    Json(ids): Json<Vec<i64>>,
) -> Result<StatusCode, StatusCode> {
    if ids.is_empty() || ids.iter().any(|id| *id <= 0) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let restore_phase = state.restore_status_rx.borrow().phase.clone();
    if restore_phase != PackageRestorePhase::Ready {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let db = state.package_db.read().await.clone();
    if let Some(ref db) = db {
        match db.delete_packages(ids).await {
            Ok(_) => Ok(StatusCode::NO_CONTENT),
            Err(e) => {
                log::error!(status = "db delete_package failed", error = &*e.to_string(); "");
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[cfg(feature = "dep-manager")]
pub(super) async fn get_restore_status(
    State(state): State<ApiState>,
) -> Json<crate::ipc::PackageRestoreSnapshot> {
    let status = state.restore_status_rx.borrow().snapshot();
    Json(status)
}

#[cfg(feature = "dep-manager")]
pub(super) async fn retry_package_restore(
    State(state): State<ApiState>,
) -> Result<StatusCode, StatusCode> {
    let db = {
        let mut package_db = state.package_db.write().await;
        if package_db.is_none() {
            *package_db = Some(
                DatabaseManager::init(&state.sql_file)
                    .await
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
            );
        }
        package_db.clone().ok_or(StatusCode::SERVICE_UNAVAILABLE)?
    };

    let mut accepted = false;
    state.restore_status.send_modify(|status| {
        if status.phase == PackageRestorePhase::Failed {
            *status = PackageRestoreStatus::restoring();
            accepted = true;
        }
    });
    if !accepted {
        return Err(StatusCode::CONFLICT);
    }

    let restore_status = state.restore_status.clone();
    tokio::spawn(async move {
        if let Err(error) = crate::dep_manager::restore_packages(&db, restore_status).await {
            log::error!(status = "package restore retry failed", error = &*error.to_string(); "");
        }
    });

    Ok(StatusCode::ACCEPTED)
}
