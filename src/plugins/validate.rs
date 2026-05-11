//! Security validation for plugin descriptors.
//!
//! Ensures that plugin YAML files cannot be used for privilege escalation
//! or arbitrary command injection.

use super::descriptor::{PluginDescriptor, CURRENT_SCHEMA_VERSION};

/// Dangerous shell metacharacters that must not appear in command arguments.
const SHELL_METACHARACTERS: &[char] = &[';', '|', '&', '$', '`', '>', '<', '(', ')', '{', '}'];

/// Allowed environment variable names that plugins may set.
const ALLOWED_ENV_VARS: &[&str] = &[
    "LANG",
    "LC_ALL",
    "LC_MESSAGES",
    "DEBIAN_FRONTEND",
    "HOME",
    "PATH",
];

/// Allowed polkit action prefixes.
const ALLOWED_POLKIT_PREFIXES: &[&str] = &["io.github.up.update.", "io.github.up.cleanup."];

/// Maximum length for a plugin ID.
const MAX_ID_LENGTH: usize = 32;

/// Validate a plugin descriptor for security and correctness.
///
/// `is_user_path` indicates whether the plugin was loaded from a user-writable
/// directory. User-path plugins are not allowed to request root privileges.
pub fn validate_descriptor(desc: &PluginDescriptor, is_user_path: bool) -> Result<(), String> {
    // 1. Schema version check
    if desc.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported schema version {} (expected {})",
            desc.schema_version, CURRENT_SCHEMA_VERSION
        ));
    }

    // 2. ID format: lowercase alphanumeric + hyphens, max 32 chars
    if desc.id.is_empty() || desc.id.len() > MAX_ID_LENGTH {
        return Err(format!(
            "Plugin ID must be 1-{} characters, got '{}'",
            MAX_ID_LENGTH, desc.id
        ));
    }
    if !desc
        .id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "Plugin ID '{}' contains invalid characters (only lowercase a-z, 0-9, - allowed)",
            desc.id
        ));
    }

    // 3. User-path plugins cannot request root
    if is_user_path && desc.privilege.needs_root {
        return Err(format!(
            "Plugin '{}' from user directory cannot request root privileges",
            desc.id
        ));
    }

    // 4. Polkit action must use an allowed prefix
    if desc.privilege.needs_root {
        let action = &desc.privilege.polkit_action;
        if !ALLOWED_POLKIT_PREFIXES
            .iter()
            .any(|prefix| action.starts_with(prefix))
        {
            return Err(format!(
                "Plugin '{}' uses disallowed polkit action '{}'",
                desc.id, action
            ));
        }
    }

    // 5. Validate detection binary — no path traversal
    validate_program_name(&desc.detection.binary, &desc.id)?;

    // 6. Validate commands
    if let Some(cmd) = &desc.commands.update {
        validate_command(cmd, &desc.id, "update")?;
    }
    if let Some(cmd) = &desc.commands.list_available {
        validate_command(cmd, &desc.id, "list_available")?;
    }
    if let Some(cmd) = &desc.commands.cleanup {
        validate_command(cmd, &desc.id, "cleanup")?;
    }
    if let Some(cmd) = &desc.commands.estimate_size {
        validate_command(cmd, &desc.id, "estimate_size")?;
    }

    // 7. Version compatibility (basic semver check)
    if desc.metadata.min_up_version.is_empty() {
        return Err(format!("Plugin '{}' missing min_up_version", desc.id));
    }

    Ok(())
}

/// Validate a program name — must not contain path separators or traversal.
fn validate_program_name(program: &str, plugin_id: &str) -> Result<(), String> {
    if program.is_empty() {
        return Err(format!("Plugin '{}': empty program name", plugin_id));
    }
    if program.contains("..") {
        return Err(format!(
            "Plugin '{}': program '{}' contains path traversal",
            plugin_id, program
        ));
    }
    if program.starts_with('/') {
        return Err(format!(
            "Plugin '{}': program '{}' must not be an absolute path (resolved via PATH)",
            plugin_id, program
        ));
    }
    if program.contains('/') {
        return Err(format!(
            "Plugin '{}': program '{}' must not contain path separators",
            plugin_id, program
        ));
    }
    Ok(())
}

/// Validate a single command definition.
fn validate_command(
    cmd: &super::descriptor::CommandDef,
    plugin_id: &str,
    operation: &str,
) -> Result<(), String> {
    // Validate program name
    validate_program_name(&cmd.program, plugin_id)?;

    // Check args for shell metacharacters
    for (i, arg) in cmd.args.iter().enumerate() {
        for &meta in SHELL_METACHARACTERS {
            if arg.contains(meta) {
                return Err(format!(
                    "Plugin '{}' command '{}' arg[{}] contains shell metacharacter '{}'",
                    plugin_id, operation, i, meta
                ));
            }
        }
        // No path traversal in args either
        if arg.contains("..") && arg.contains('/') {
            return Err(format!(
                "Plugin '{}' command '{}' arg[{}] contains path traversal",
                plugin_id, operation, i
            ));
        }
    }

    // Validate environment variables
    for key in cmd.environment.keys() {
        if !ALLOWED_ENV_VARS.contains(&key.as_str()) {
            return Err(format!(
                "Plugin '{}' command '{}' sets disallowed env var '{}'",
                plugin_id, operation, key
            ));
        }
    }

    Ok(())
}
