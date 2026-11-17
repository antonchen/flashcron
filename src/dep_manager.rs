use crate::db::DatabaseManager;
use crate::error::Result;
use crate::ipc::{PackageRestorePhase, PackageRestoreStatus};
use log::{error, info, warn};
use std::process::{Output, Stdio};
use std::str::FromStr;
use std::{fs, io::Write, path::PathBuf};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

static PACKAGE_MANAGER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PkgType {
    APT,
    PIP,
}

impl std::fmt::Display for PkgType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkgType::APT => write!(f, "APT"),
            PkgType::PIP => write!(f, "PIP"),
        }
    }
}

impl FromStr for PkgType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "APT" => Ok(PkgType::APT),
            "PIP" => Ok(PkgType::PIP),
            _ => Err(format!("Unknown package type: {}", s)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PkgStatus {
    Failed = 0,
    Installed = 1,
    Pending = 2,
}

impl From<i64> for PkgStatus {
    fn from(v: i64) -> Self {
        match v {
            0 => PkgStatus::Failed,
            1 => PkgStatus::Installed,
            _ => PkgStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManagedPackage {
    pub id: i64,
    pub pkg_type: PkgType,
    pub name: String,
    pub status: PkgStatus,
    pub target: Option<String>,
    pub current: Option<String>,
    pub error_msg: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageSources {
    pub apt_mirror: Option<String>,
    pub pip_index_url: String,
}

pub async fn package_sources() -> std::io::Result<PackageSources> {
    let apt_mirror = tokio::task::spawn_blocking(read_apt_mirror)
        .await
        .map_err(std::io::Error::other)??;
    let configured_index = read_pip_index_url().await?;
    let pip_index_url = if configured_index.is_empty() {
        std::env::var("PIP_INDEX_URL")
            .ok()
            .filter(|index| !index.trim().is_empty())
            .unwrap_or_else(|| "https://pypi.org/simple".to_owned())
    } else {
        configured_index
    };
    Ok(PackageSources {
        apt_mirror,
        pip_index_url,
    })
}

pub async fn set_package_source(kind: &str, value: &str) -> std::io::Result<()> {
    let _guard = PACKAGE_MANAGER_LOCK.lock().await;
    match kind {
        "apt" => {
            let value = validate_apt_mirror(value)?;
            tokio::task::spawn_blocking(move || replace_apt_mirror(&value))
                .await
                .map_err(std::io::Error::other)??;
            Ok(())
        }
        "pip" => set_pip_index_url(value).await,
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unknown package source",
        )),
    }
}

fn validate_apt_mirror(value: &str) -> std::io::Result<String> {
    let normalized = value.trim_end_matches('/');
    let pattern = regex::Regex::new(r"^https?://[A-Za-z0-9.-]+(?::[0-9]{1,5})?$").unwrap();
    if pattern.is_match(normalized) {
        Ok(normalized.to_owned())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Enter an HTTP or HTTPS mirror host without a path",
        ))
    }
}

fn apt_source_files() -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let main = PathBuf::from("/etc/apt/sources.list");
    if fs::symlink_metadata(&main).is_ok_and(|meta| meta.file_type().is_file()) {
        files.push(main);
    }

    let dir = PathBuf::from("/etc/apt/sources.list.d");
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                let supported = matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("list" | "sources")
                );
                if supported
                    && fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_file())
                {
                    files.push(path);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    files.sort();
    Ok(files)
}

fn write_source_file(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    let metadata = fs::metadata(path)?;
    let temp = path.with_extension(format!("flashcron-{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(content)?;
        file.sync_all()?;
        fs::set_permissions(&temp, metadata.permissions())?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn read_apt_mirror() -> std::io::Result<Option<String>> {
    if !is_debian_system()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "APT mirror management supports Debian only",
        ));
    }
    read_apt_mirror_from_files(apt_source_files()?)
}

fn read_apt_mirror_from_files(files: Vec<PathBuf>) -> std::io::Result<Option<String>> {
    let pattern =
        regex::Regex::new(r"(https?://[A-Za-z0-9.-]+(?::[0-9]+)?)/debian/?(?:\s|$)").unwrap();
    let mut mirrors = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path)?;
        for line in text.lines() {
            let active = line.split('#').next().unwrap_or("");
            mirrors.extend(
                pattern
                    .captures_iter(active)
                    .map(|found| found[1].to_owned()),
            );
        }
    }
    mirrors.sort();
    mirrors.dedup();
    if mirrors.len() > 1 {
        return Ok(None);
    }
    Ok(mirrors.pop())
}

fn replace_apt_mirror(mirror: &str) -> std::io::Result<()> {
    if !is_debian_system()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "APT mirror management supports Debian only",
        ));
    }
    let pattern = regex::Regex::new(
        r"https?://[A-Za-z0-9.-]+(?::[0-9]+)?(/debian(?:-security)?/?)([ \t\r\n]|$)",
    )
    .unwrap();
    let mut updates = Vec::new();
    for path in apt_source_files()? {
        let text = fs::read_to_string(&path)?;
        let mut matched = false;
        let mut updated = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let (active, comment) = line.split_at(line.find('#').unwrap_or(line.len()));
            updated.push_str(&pattern.replace_all(active, |found: &regex::Captures<'_>| {
                matched = true;
                format!("{mirror}{}{}", &found[1], &found[2])
            }));
            updated.push_str(comment);
        }
        if matched {
            updates.push((path, updated));
        }
    }
    if updates.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No supported Debian repository URIs found",
        ));
    }
    for (path, content) in updates {
        write_source_file(&path, content.as_bytes())?;
    }
    Ok(())
}

fn is_debian_system() -> std::io::Result<bool> {
    let os_release = fs::read_to_string("/etc/os-release")?;
    Ok(os_release
        .lines()
        .any(|line| line.trim() == "ID=debian" || line.trim() == "ID=\"debian\""))
}

async fn read_pip_index_url() -> std::io::Result<String> {
    let output = Command::new("pip")
        .args(["config", "get", "global.index-url"])
        .output()
        .await?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else if output.status.code() == Some(1)
        && String::from_utf8_lossy(&output.stderr).trim() == "ERROR: No such key - global.index-url"
    {
        Ok(String::new())
    } else {
        Err(std::io::Error::other(format!(
            "pip config get global.index-url failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

async fn set_pip_index_url(index: &str) -> std::io::Result<()> {
    let pattern = regex::Regex::new(r"^https?://[^\s?#]+$").unwrap();
    if !pattern.is_match(index) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Enter a valid HTTP or HTTPS PIP index URL",
        ));
    }
    let output = Command::new("pip")
        .args(["config", "--site", "set", "global.index-url", index])
        .output()
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

/// Validate package name without using regex to keep binary size small
pub fn is_valid_pkg_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == '+')
}

/// Validate package version without using regex
pub fn is_valid_pkg_version(version: &str) -> bool {
    if version.is_empty() {
        return false;
    }
    version.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '.' | '-' | '_' | '+' | '=' | '<' | '>' | '~' | '^' | '*' | ','
            )
    })
}

/// Perform package sync (run at startup)
pub async fn sync_packages(db: &DatabaseManager) -> Result<()> {
    let pending = db.get_pending_packages().await?;
    sync_package_list(pending, db, None).await
}

/// Restore the full persisted manifest in a newly created container.
pub async fn restore_packages(
    db: &DatabaseManager,
    restore_tx: watch::Sender<PackageRestoreStatus>,
) -> Result<()> {
    restore_tx.send_replace(PackageRestoreStatus::restoring());
    let result = match db.list_packages().await {
        Ok(packages) => sync_package_list(packages, db, Some(restore_tx.clone())).await,
        Err(error) => Err(error),
    };
    match &result {
        Ok(()) => restore_tx.send_replace(PackageRestoreStatus::ready()),
        Err(error) => {
            let output = restore_tx.borrow().output.clone();
            restore_tx.send_replace(PackageRestoreStatus {
                phase: PackageRestorePhase::Failed,
                output,
                error: Some(error.to_string()),
            })
        }
    };
    result
}

async fn sync_package_list(
    pending: Vec<ManagedPackage>,
    db: &DatabaseManager,
    restore_tx: Option<watch::Sender<PackageRestoreStatus>>,
) -> Result<()> {
    let _guard = PACKAGE_MANAGER_LOCK.lock().await;
    if pending.is_empty() {
        return Ok(());
    }

    info!(status = "syncing packages", count = pending.len(); "");

    let mut apt_pkgs = Vec::new();
    let mut pip_pkgs = Vec::new();

    for pkg in pending {
        match pkg.pkg_type {
            PkgType::APT => apt_pkgs.push(pkg),
            PkgType::PIP => pip_pkgs.push(pkg),
        }
    }

    // Process concurrently
    let apt_restore = restore_tx.clone();
    let apt = async {
        if !apt_pkgs.is_empty() {
            process_apt(apt_pkgs, db, apt_restore).await?;
        }
        Ok::<_, crate::error::Error>(())
    };

    let pip = async {
        if !pip_pkgs.is_empty() {
            process_pip(pip_pkgs, db, restore_tx).await?;
        }
        Ok::<_, crate::error::Error>(())
    };

    let (apt_result, pip_result) = tokio::join!(apt, pip);
    apt_result?;
    pip_result?;

    info!(status = "package sync completed"; "");
    Ok(())
}

async fn run_package_command(
    mut command: Command,
    restore_tx: Option<watch::Sender<PackageRestoreStatus>>,
) -> std::io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    command.process_group(0);

    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (tx, mut rx) = mpsc::unbounded_channel::<(bool, Vec<u8>)>();
    let stdout_task = tokio::spawn(read_package_output(stdout, false, tx.clone()));
    let stderr_task = tokio::spawn(read_package_output(stderr, true, tx.clone()));
    drop(tx);

    let output_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        while let Some((is_stderr, chunk)) = rx.recv().await {
            if is_stderr {
                stderr.extend_from_slice(&chunk);
            } else {
                stdout.extend_from_slice(&chunk);
            }
            if let Some(ref restore_tx) = restore_tx {
                restore_tx.send_modify(|state| state.output.extend_from_slice(&chunk));
            }
        }
        (stdout, stderr)
    });

    let status = child.wait().await?;
    let _ = tokio::join!(stdout_task, stderr_task);
    let (stdout, stderr) = output_task
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn read_package_output<R>(
    mut stream: R,
    is_stderr: bool,
    tx: mpsc::UnboundedSender<(bool, Vec<u8>)>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = vec![0; 8192];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(size) => {
                if tx.send((is_stderr, buffer[..size].to_vec())).is_err() {
                    break;
                }
            }
        }
    }
}

async fn process_apt(
    packages: Vec<ManagedPackage>,
    db: &DatabaseManager,
    restore_tx: Option<watch::Sender<PackageRestoreStatus>>,
) -> Result<()> {
    // Environment check
    let is_root = nix::unistd::Uid::current().is_root();

    if !is_root {
        let err_msg = "APT installation requires root privileges.";
        warn!(pkg = "APT", error = err_msg; "");
        for pkg in packages {
            let _ = db
                .update_package_status(pkg.id, PkgStatus::Failed, Some(err_msg.to_string()), None)
                .await;
        }
        return Err(crate::error::Error::config(err_msg));
    }

    // Filter valid names and collect
    let mut names_to_install = Vec::new();

    for pkg in packages {
        if is_valid_pkg_name(&pkg.name) {
            names_to_install.push(pkg);
        } else {
            let _ = db
                .update_package_status(
                    pkg.id,
                    PkgStatus::Failed,
                    Some("Invalid package name".to_string()),
                    None,
                )
                .await;
        }
    }

    if names_to_install.is_empty() {
        return Ok(());
    }

    // Update package index
    info!(pkg = "APT", status = "updating index"; "");
    let mut update_command = Command::new("apt-get");
    update_command
        .env("DEBIAN_FRONTEND", "noninteractive")
        .arg("update");
    let update = run_package_command(update_command, restore_tx.clone()).await;
    match update {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let error = String::from_utf8_lossy(&output.stderr).into_owned();
            for pkg in &names_to_install {
                let _ = db
                    .update_package_status(pkg.id, PkgStatus::Failed, Some(error.clone()), None)
                    .await;
            }
            return Err(crate::error::Error::config(error));
        }
        Err(error) => {
            for pkg in &names_to_install {
                let _ = db
                    .update_package_status(pkg.id, PkgStatus::Failed, Some(error.to_string()), None)
                    .await;
            }
            return Err(crate::error::Error::config(error.to_string()));
        }
    }

    let pkg_names: Vec<String> = names_to_install
        .iter()
        .map(|p| {
            if let Some(ref ver) = p.target {
                format!("{}={}", p.name, ver)
            } else {
                p.name.clone()
            }
        })
        .collect();
    info!(pkg = "APT", status = "installing/upgrading", count = pkg_names.len(); "");

    let mut cmd = Command::new("apt-get");
    cmd.env("DEBIAN_FRONTEND", "noninteractive")
        .arg("install")
        .arg("-y")
        .arg("--no-install-recommends")
        .args(&pkg_names);

    match run_package_command(cmd, restore_tx).await {
        Ok(output) => {
            if output.status.success() {
                for pkg in names_to_install {
                    let mut current_ver = None;
                    let dpkg_cmd = Command::new("dpkg-query")
                        .arg("-W")
                        .arg("-f=${Version}")
                        .arg(&pkg.name)
                        .output()
                        .await;
                    if let Ok(dpkg_out) = dpkg_cmd {
                        if dpkg_out.status.success() {
                            let ver_str =
                                String::from_utf8_lossy(&dpkg_out.stdout).trim().to_string();
                            if !ver_str.is_empty() {
                                current_ver = Some(ver_str);
                            }
                        }
                    }
                    let _ = db
                        .update_package_status(pkg.id, PkgStatus::Installed, None, current_ver)
                        .await;
                }
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                error!(pkg = "APT", status = "failed", error = &*stderr; "");
                for pkg in names_to_install {
                    let _ = db
                        .update_package_status(
                            pkg.id,
                            PkgStatus::Failed,
                            Some(stderr.clone()),
                            None,
                        )
                        .await;
                }
                Err(crate::error::Error::config(stderr))
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            error!(pkg = "APT", status = "failed", error = &*err_msg; "");
            for pkg in names_to_install {
                let _ = db
                    .update_package_status(pkg.id, PkgStatus::Failed, Some(err_msg.clone()), None)
                    .await;
            }
            Err(crate::error::Error::config(err_msg))
        }
    }
}

async fn process_pip(
    packages: Vec<ManagedPackage>,
    db: &DatabaseManager,
    restore_tx: Option<watch::Sender<PackageRestoreStatus>>,
) -> Result<()> {
    // Environment check
    let has_venv = std::env::var("VIRTUAL_ENV").is_ok() || std::env::var("CONDA_PREFIX").is_ok();

    if !has_venv {
        let err_msg = "PIP installation requires a virtual environment (venv/conda) to prevent system contamination.";
        warn!(pkg = "PIP", error = err_msg; "");
        for pkg in packages {
            let _ = db
                .update_package_status(pkg.id, PkgStatus::Failed, Some(err_msg.to_string()), None)
                .await;
        }
        return Err(crate::error::Error::config(err_msg));
    }

    // Filter valid names
    let mut names_to_install = Vec::new();
    for pkg in packages {
        if is_valid_pkg_name(&pkg.name) {
            names_to_install.push(pkg);
        } else {
            let _ = db
                .update_package_status(
                    pkg.id,
                    PkgStatus::Failed,
                    Some("Invalid package name".to_string()),
                    None,
                )
                .await;
        }
    }

    if names_to_install.is_empty() {
        return Ok(());
    }

    let pkg_names: Vec<String> = names_to_install
        .iter()
        .map(|p| {
            if let Some(ref ver) = p.target {
                if ver.starts_with('=')
                    || ver.starts_with('>')
                    || ver.starts_with('<')
                    || ver.starts_with('~')
                    || ver.starts_with('^')
                {
                    format!("{}{}", p.name, ver)
                } else {
                    format!("{}=={}", p.name, ver)
                }
            } else {
                p.name.clone()
            }
        })
        .collect();
    info!(pkg = "PIP", status = "installing", count = pkg_names.len(); "");

    let mut cmd = Command::new("pip");
    cmd.arg("install")
        .arg("--no-cache-dir")
        .arg("--upgrade")
        .args(&pkg_names);

    match read_pip_index_url().await {
        Ok(index) if !index.is_empty() => {
            cmd.arg("--index-url").arg(index);
        }
        Ok(_) => {}
        Err(error) => {
            let message = error.to_string();
            for pkg in names_to_install {
                db.update_package_status(pkg.id, PkgStatus::Failed, Some(message.clone()), None)
                    .await?;
            }
            return Err(crate::error::Error::config(message));
        }
    }

    match run_package_command(cmd, restore_tx).await {
        Ok(output) => {
            if output.status.success() {
                for pkg in names_to_install {
                    let mut current_ver = None;
                    let pip_cmd = Command::new("pip")
                        .arg("show")
                        .arg(&pkg.name)
                        .output()
                        .await;
                    if let Ok(pip_out) = pip_cmd {
                        if pip_out.status.success() {
                            let stdout = String::from_utf8_lossy(&pip_out.stdout);
                            for line in stdout.lines() {
                                if line.starts_with("Version: ") {
                                    let ver_str =
                                        line.trim_start_matches("Version: ").trim().to_string();
                                    if !ver_str.is_empty() {
                                        current_ver = Some(ver_str);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    let _ = db
                        .update_package_status(pkg.id, PkgStatus::Installed, None, current_ver)
                        .await;
                }
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                error!(pkg = "PIP", status = "failed", error = &*stderr; "");
                for pkg in names_to_install {
                    let _ = db
                        .update_package_status(
                            pkg.id,
                            PkgStatus::Failed,
                            Some(stderr.clone()),
                            None,
                        )
                        .await;
                }
                Err(crate::error::Error::config(stderr))
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            error!(pkg = "PIP", status = "failed", error = &*err_msg; "");
            for pkg in names_to_install {
                let _ = db
                    .update_package_status(pkg.id, PkgStatus::Failed, Some(err_msg.clone()), None)
                    .await;
            }
            Err(crate::error::Error::config(err_msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_apt_mirrors_leave_the_current_address_empty() {
        let source = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            source.path(),
            "deb https://mirror-a.example/debian stable main\ndeb https://mirror-a.example/debian stable-updates main\n",
        )
        .unwrap();
        assert_eq!(
            read_apt_mirror_from_files(vec![source.path().to_owned()]).unwrap(),
            Some("https://mirror-a.example".to_owned())
        );

        fs::write(
            source.path(),
            "deb https://mirror-a.example/debian stable main\ndeb https://mirror-b.example/debian stable-updates main\n",
        )
        .unwrap();
        assert_eq!(
            read_apt_mirror_from_files(vec![source.path().to_owned()]).unwrap(),
            None
        );
    }

    #[test]
    fn test_valid_pkg_name() {
        assert!(is_valid_pkg_name("python3-pip"));
        assert!(is_valid_pkg_name("libssl-dev"));
        assert!(is_valid_pkg_name("g++"));
        assert!(is_valid_pkg_name("numpy-1.21.0"));
        assert!(is_valid_pkg_name("a_b.c+d"));
        assert!(!is_valid_pkg_name("rm -rf /"));
        assert!(!is_valid_pkg_name("foo&bar"));
        assert!(!is_valid_pkg_name("foo bar"));
        assert!(!is_valid_pkg_name(""));
    }

    #[test]
    fn test_valid_pkg_version() {
        assert!(is_valid_pkg_version("1.2.3"));
        assert!(is_valid_pkg_version(">=1.0.0"));
        assert!(is_valid_pkg_version("~=2.0"));
        assert!(is_valid_pkg_version("==1.2.3"));
        assert!(is_valid_pkg_version("1.2.3-alpha.1"));
        assert!(!is_valid_pkg_version("rm -rf /"));
        assert!(!is_valid_pkg_version("foo&bar"));
        assert!(!is_valid_pkg_version("foo bar"));
        assert!(!is_valid_pkg_version(""));
    }
}
