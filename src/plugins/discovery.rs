//! Plugin discovery — scans XDG data directories for YAML backend descriptors.

use super::descriptor::PluginDescriptor;
use super::validate;
use log::{info, warn};
use std::collections::HashMap;
use std::path::PathBuf;

/// Maximum allowed file size for a plugin descriptor (64 KB).
const MAX_DESCRIPTOR_SIZE: u64 = 64 * 1024;

/// Scan all plugin directories and return validated, active descriptors.
///
/// Higher-priority directories (user, /etc) override same-named plugins from
/// lower-priority directories (/usr/share). A `.disabled` file in `/etc/up/backends.d/`
/// disables a plugin entirely.
pub fn discover_plugins() -> Vec<PluginDescriptor> {
    let dirs = plugin_search_dirs();
    let mut seen: HashMap<String, PluginDescriptor> = HashMap::new();

    // Iterate in listed order: system dirs first, user/etc last.
    // Later inserts override earlier ones, so user/admin plugins take priority.
    for dir in dirs.iter() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_yaml = path
                    .extension()
                    .map(|e| e == "yaml" || e == "yml")
                    .unwrap_or(false);
                if !is_yaml {
                    continue;
                }

                // Check file size before reading
                if let Ok(meta) = std::fs::metadata(&path) {
                    if meta.len() > MAX_DESCRIPTOR_SIZE {
                        warn!(
                            "Skipping plugin {:?}: exceeds max size ({}B)",
                            path,
                            meta.len()
                        );
                        continue;
                    }
                }

                match load_descriptor(&path) {
                    Ok(desc) => {
                        // Determine if the plugin came from a user-writable directory
                        let is_user_path = is_user_plugin_path(&path);

                        match validate::validate_descriptor(&desc, is_user_path) {
                            Ok(()) => {
                                info!("Loaded plugin: {} from {:?}", desc.id, path);
                                seen.insert(desc.id.clone(), desc);
                            }
                            Err(e) => {
                                warn!("Plugin {:?} failed validation: {}", path, e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Skipping plugin {:?}: {}", path, e);
                    }
                }
            }
        }
    }

    // Check /etc/up/backends.d/ for disabled plugins
    let etc_dir = PathBuf::from("/etc/up/backends.d");
    seen.retain(|id, _| {
        let disabled_path = etc_dir.join(format!("{}.disabled", id));
        if disabled_path.exists() {
            info!("Plugin {} disabled via {:?}", id, disabled_path);
            false
        } else {
            true
        }
    });

    seen.into_values().collect()
}

/// Load and parse a single YAML descriptor file.
fn load_descriptor(path: &std::path::Path) -> Result<PluginDescriptor, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("Cannot read file: {}", e))?;
    let descriptor: PluginDescriptor =
        serde_yml::from_str(&content).map_err(|e| format!("YAML parse error: {}", e))?;
    Ok(descriptor)
}

/// Check if a path is in a user-writable location (XDG_DATA_HOME).
fn is_user_plugin_path(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    // User home directories
    if path_str.contains("/.local/share/") {
        return true;
    }
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        if path_str.starts_with(&data_home) {
            return true;
        }
    }
    false
}

/// Return plugin search directories in priority order (lowest first).
fn plugin_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // System directories from XDG_DATA_DIRS (lowest priority)
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        dirs.push(PathBuf::from(dir).join("up/backends.d"));
    }

    // User directory (XDG_DATA_HOME) — higher priority
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("up/backends.d"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/up/backends.d"));
    }

    // /etc override directory (highest priority)
    dirs.push(PathBuf::from("/etc/up/backends.d"));

    dirs
}
