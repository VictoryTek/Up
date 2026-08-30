use crate::backends::BackendKind;
use serde::{Deserialize, Serialize};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub skipped_backends: Vec<BackendKind>,
    /// Plugin descriptor ids the user has disabled in the Plugin manager.
    /// Discovered-but-disabled plugins are not registered as backends.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip_preserves_skipped_backends() {
        let tmp = std::env::temp_dir().join(format!(
            "up-config-test-{}-{}",
            std::process::id(),
            crate::history::now_secs()
        ));
        // SAFETY: single-threaded within this test; no other test reads/writes XDG_CONFIG_HOME.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &tmp);
        }

        let config = AppConfig {
            skipped_backends: vec![BackendKind::Apt, BackendKind::Plugin("xbps".into())],
            disabled_plugins: vec!["eopkg".into(), "swupd".into()],
        };
        save_config(&config).expect("save_config should succeed");

        let loaded = load_config();
        assert_eq!(loaded.skipped_backends, config.skipped_backends);
        assert_eq!(loaded.disabled_plugins, config.disabled_plugins);

        let _ = std::fs::remove_dir_all(&tmp);
        // SAFETY: same single-threaded context as the set_var above.
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }
}
