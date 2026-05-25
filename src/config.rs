#![allow(dead_code)]
use crate::backends::BackendKind;
use serde::{Deserialize, Serialize};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

/// User preference for snapshot creation behavior before updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SnapshotPreference {
    /// Prompt the user each time a snapshot tool is detected.
    #[default]
    Ask,
    /// Always create a snapshot without prompting.
    Always,
    /// Never create a snapshot; skip the prompt entirely.
    Never,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub skipped_backends: Vec<BackendKind>,
    #[serde(default)]
    pub snapshot_preference: SnapshotPreference,
}

/// Returns the path to the config JSON file, honoring XDG_CONFIG_HOME.
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("up").join("config.json")
}

/// Load the application config. Returns `AppConfig::default()` on any error
/// (missing file, parse error) to ensure a clean startup every time.
pub fn load_config() -> AppConfig {
    let path = config_path();
    if !path.exists() {
        return AppConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

/// Persist the application config to disk.
/// Creates parent directories if they don't exist.
/// Errors are non-fatal; callers should log but not panic.
pub fn save_config(config: &AppConfig) -> io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    let mut writer = BufWriter::new(file);
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write!(writer, "{json}")?;
    Ok(())
}
