use crate::config::{JobExecution, Settings};
use crate::error::{Error, Result};
use crate::scheduler::SchedulerState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcRequest {
    Status,
    PackageRestoreStatus,
    Trigger {
        job_name: String,
    },
    Stop,
    HistoryList {
        job_name: Option<String>,
        limit: usize,
    },
    HistoryGet {
        execution_id: Uuid,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusPayload {
    pub config_path: String,
    pub state: SchedulerState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageRestorePhase {
    Ready,
    Restoring,
    Failed,
}

impl PackageRestorePhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Restoring => "restoring",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRestoreStatus {
    pub phase: PackageRestorePhase,
    pub output: Vec<u8>,
    pub error: Option<String>,
}

impl PackageRestoreStatus {
    pub fn ready() -> Self {
        Self {
            phase: PackageRestorePhase::Ready,
            output: Vec::new(),
            error: None,
        }
    }

    pub fn restoring() -> Self {
        Self {
            phase: PackageRestorePhase::Restoring,
            output: Vec::new(),
            error: None,
        }
    }

    pub fn snapshot(&self) -> PackageRestoreSnapshot {
        PackageRestoreSnapshot {
            phase: self.phase.clone(),
            output: String::from_utf8_lossy(&self.output).into_owned(),
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageRestoreSnapshot {
    pub phase: PackageRestorePhase,
    pub output: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcResponse {
    Status(StatusPayload),
    PackageRestoreStatus(PackageRestoreSnapshot),
    Triggered { execution_id: Uuid },
    Stopped,
    HistoryEntries { history: Vec<JobExecution> },
    HistoryEntry { execution: Option<JobExecution> },
    Ok,
    Error { message: String },
}

pub fn normalize_config_path(config_path: &Path) -> PathBuf {
    config_path.canonicalize().unwrap_or_else(|_| {
        if config_path.is_absolute() {
            config_path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(config_path)
        }
    })
}

pub fn default_socket_path() -> PathBuf {
    crate::config::default_ipc_socket()
}

pub async fn bind_listener(_settings: &Settings) -> Result<(PathBuf, UnixListener)> {
    let socket_path = default_socket_path();
    bind_listener_at(socket_path).await
}

pub async fn send_request(request: &IpcRequest) -> Result<IpcResponse> {
    send_request_to_path(default_socket_path(), request).await
}

pub async fn send_request_with_config(
    _config_path: &std::path::Path,
    request: &IpcRequest,
) -> Result<IpcResponse> {
    send_request(request).await
}

async fn bind_listener_at(socket_path: PathBuf) -> Result<(PathBuf, UnixListener)> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }

    if socket_path.exists() {
        match UnixStream::connect(&socket_path).await {
            Ok(_) => {
                return Err(Error::ipc_already_running(socket_path));
            }
            Err(_) => {
                let _ = std::fs::remove_file(&socket_path);
            }
        }
    }

    let listener = UnixListener::bind(&socket_path).map_err(|source| Error::IpcBind {
        path: socket_path.clone(),
        source,
    })?;

    Ok((socket_path, listener))
}

async fn send_request_to_path(socket_path: PathBuf, request: &IpcRequest) -> Result<IpcResponse> {
    let stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|source| Error::IpcConnect {
            path: socket_path.clone(),
            source,
        })?;

    let (read_half, mut write_half) = stream.into_split();
    let mut payload = serde_json::to_vec(request).map_err(|e| Error::Config(e.to_string()))?;
    payload.push(b'\n');
    write_half
        .write_all(&payload)
        .await
        .map_err(|source| Error::IpcWrite { source })?;
    write_half
        .shutdown()
        .await
        .map_err(|source| Error::IpcWrite { source })?;

    let mut reader = BufReader::new(read_half);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .await
        .map_err(|source| Error::IpcRead { source })?;

    serde_json::from_str(response.trim_end()).map_err(|e| Error::Config(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_path_matches_settings_default() {
        assert_eq!(default_socket_path(), crate::config::default_ipc_socket());
    }
}
