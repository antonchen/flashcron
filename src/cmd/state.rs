use std::path::PathBuf;

pub fn resolve_config_path(cli_config: Option<PathBuf>) -> PathBuf {
    let path = cli_config.unwrap_or_else(|| PathBuf::from("flashcron.toml"));
    flashcron::ipc::normalize_config_path(&path)
}

pub fn resolve_explicit_config_path(cli_config: Option<PathBuf>) -> Option<PathBuf> {
    cli_config.map(|path| flashcron::ipc::normalize_config_path(&path))
}
