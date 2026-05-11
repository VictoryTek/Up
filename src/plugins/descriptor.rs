//! YAML descriptor types for the backend plugin system.
//!
//! These types correspond to the plugin YAML schema (version 1).
//! Plugins are loaded from XDG data directories and parsed with `serde_yml`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current schema version supported by this version of Up.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// A complete plugin descriptor loaded from a YAML file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginDescriptor {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub icon_name: String,
    pub detection: DetectionConfig,
    pub privilege: PrivilegeConfig,
    pub commands: CommandSet,
    pub capabilities: CapabilitySet,
    pub metadata: PluginMetadata,
}

/// How to detect if this backend is available on the current system.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DetectionConfig {
    /// Required binary that must exist in PATH.
    pub binary: String,
    /// Optional: the current OS ID (from /etc/os-release) must be one of these.
    #[serde(default)]
    pub os_id: Vec<String>,
    /// Optional: a file path that must exist.
    pub file_exists: Option<String>,
}

/// Privilege requirements for this backend.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrivilegeConfig {
    pub needs_root: bool,
    pub polkit_action: String,
}

/// Set of commands this plugin can execute.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandSet {
    pub update: Option<CommandDef>,
    pub list_available: Option<CommandDef>,
    pub cleanup: Option<CommandDef>,
    pub estimate_size: Option<CommandDef>,
}

/// A single command definition with its arguments, environment, and output parser.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandDef {
    pub program: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    pub parser: ParserDef,
}

/// Output parser configuration — determines how command output is interpreted.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ParserDef {
    /// Count lines matching a regex pattern.
    #[serde(rename = "regex_count")]
    RegexCount { pattern: String },

    /// Count lines matching a simple pattern.
    #[serde(rename = "line_count")]
    LineCount { pattern: String },

    /// Extract a specific field from each output line.
    #[serde(rename = "line_field")]
    LineField {
        field_index: usize,
        separator: String,
        #[serde(default)]
        skip_lines: usize,
    },

    /// Extract a size value from regex capture groups.
    #[serde(rename = "size_regex")]
    SizeRegex { pattern: String, unit_group: usize },

    /// Extract a value from JSON output using a dot-separated path.
    #[serde(rename = "json_path")]
    JsonPath { path: String },

    /// Use the exit code to determine the result.
    #[serde(rename = "exit_code")]
    ExitCode {
        #[serde(default = "default_success_codes")]
        success_codes: Vec<i32>,
        update_code: Option<i32>,
    },
}

fn default_success_codes() -> Vec<i32> {
    vec![0]
}

/// Declares what operations this backend supports.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilitySet {
    pub update: bool,
    pub list_available: bool,
    #[serde(default)]
    pub cleanup: bool,
    #[serde(default)]
    pub estimate_size: bool,
    #[serde(default)]
    pub count_available: bool,
}

/// Plugin metadata for authorship and versioning.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginMetadata {
    pub author: String,
    pub version: String,
    pub min_up_version: String,
    pub license: String,
}
