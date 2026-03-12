use std::fs;
use std::path::{Path, PathBuf};

const STATE_FILE: &str = "/tmp/flashcron.state";

pub fn resolve_config_path(cli_config: Option<PathBuf>) -> PathBuf {
    // 1 & 2: Command line argument or environment variable (handled by clap)
    if let Some(path) = cli_config {
        return path;
    }

    // 3: Try to read from running daemon's state file
    if let Ok(state_content) = fs::read_to_string(STATE_FILE) {
        let state_path = PathBuf::from(state_content.trim());
        if state_path.exists() {
            return state_path;
        }
    }

    // 4: Default fallback
    PathBuf::from("flashcron.toml")
}

pub fn save_state(config_path: &Path) {
    if let Ok(abs_path) = fs::canonicalize(config_path) {
        let _ = fs::write(STATE_FILE, abs_path.to_string_lossy().as_ref());
    } else {
        let _ = fs::write(STATE_FILE, config_path.to_string_lossy().as_ref());
    }
}

pub fn clear_state() {
    let _ = fs::remove_file(STATE_FILE);
}
