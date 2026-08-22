//! `PluginBackend` — implements the [`Backend`] trait for plugin-defined backends.

use super::descriptor::{CommandDef, PluginDescriptor};
use super::parser;
use crate::backends::{Backend, BackendError, BackendKind, UpdateResult};
use crate::executor::CommandExecutor;
use std::future::Future;
use std::pin::Pin;

/// A [`Backend`] implementation constructed from a YAML plugin descriptor.
///
/// Plugin backends delegate actual command execution to the provided
/// [`CommandExecutor`].
pub struct PluginBackend {
    descriptor: PluginDescriptor,
}

impl PluginBackend {
    /// Create a new plugin backend from a validated descriptor.
    pub fn new(descriptor: PluginDescriptor) -> Self {
        Self { descriptor }
    }

    /// Get the plugin ID.
    #[allow(dead_code)]
    pub fn id(&self) -> &str {
        &self.descriptor.id
    }

    /// Run a plugin command, routing through `pkexec` (with any declared
    /// `environment` applied via `env`) when the plugin needs root.
    async fn run_command(
        &self,
        cmd: &CommandDef,
        runner: &dyn CommandExecutor,
    ) -> Result<String, BackendError> {
        if self.descriptor.privilege.needs_root {
            let mut owned_args: Vec<String> = Vec::new();
            if !cmd.environment.is_empty() {
                owned_args.push("env".to_string());
                for (key, value) in &cmd.environment {
                    owned_args.push(format!("{key}={value}"));
                }
            }
            owned_args.push(cmd.program.clone());
            owned_args.extend(cmd.args.iter().cloned());
            let args: Vec<&str> = owned_args.iter().map(String::as_str).collect();
            runner.run("pkexec", &args).await
        } else {
            let args: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
            runner.run(&cmd.program, &args).await
        }
    }
}

impl Backend for PluginBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Plugin(self.descriptor.id.clone())
    }

    fn display_name(&self) -> &str {
        &self.descriptor.display_name
    }

    fn description(&self) -> &str {
        &self.descriptor.description
    }

    fn icon_name(&self) -> &str {
        &self.descriptor.icon_name
    }

    fn needs_root(&self) -> bool {
        self.descriptor.privilege.needs_root
    }

    fn run_update<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            let Some(cmd) = &self.descriptor.commands.update else {
                return UpdateResult::Skipped("No update command defined".into());
            };

            let result = self.run_command(cmd, runner).await;

            match result {
                Ok(output) => {
                    let count = parser::apply_parser_count(&cmd.parser, &output);
                    UpdateResult::Success {
                        updated_count: count,
                        updated_items: Vec::new(),
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }

    fn list_available(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        Box::pin(async move {
            let Some(cmd) = &self.descriptor.commands.list_available else {
                return Ok(Vec::new());
            };

            let args: Vec<&str> = cmd.args.iter().map(|s| s.as_str()).collect();

            // list_available is always unprivileged — spawn directly
            let output = tokio::process::Command::new(&cmd.program)
                .args(&args)
                .envs(&cmd.environment)
                .output()
                .await
                .map_err(|e| format!("Failed to run {}: {}", cmd.program, e))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let packages = parser::apply_parser_list(&cmd.parser, &stdout);
            Ok(packages)
        })
    }

    fn estimate_size(&self) -> Pin<Box<dyn Future<Output = Option<u64>> + Send + '_>> {
        Box::pin(async move {
            let cmd = self.descriptor.commands.estimate_size.as_ref()?;

            let args: Vec<&str> = cmd.args.iter().map(|s| s.as_str()).collect();
            let output = tokio::process::Command::new(&cmd.program)
                .args(&args)
                .envs(&cmd.environment)
                .output()
                .await
                .ok()?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            parser::apply_parser_size(&cmd.parser, &stdout)
        })
    }

    fn supports_cleanup(&self) -> bool {
        self.descriptor.capabilities.cleanup && self.descriptor.commands.cleanup.is_some()
    }

    fn run_cleanup<'a>(
        &'a self,
        runner: &'a dyn CommandExecutor,
    ) -> Pin<Box<dyn Future<Output = UpdateResult> + Send + 'a>> {
        Box::pin(async move {
            let Some(cmd) = &self.descriptor.commands.cleanup else {
                return UpdateResult::Success {
                    updated_count: 0,
                    updated_items: Vec::new(),
                };
            };

            let result = self.run_command(cmd, runner).await;

            match result {
                Ok(output) => {
                    let count = parser::apply_parser_count(&cmd.parser, &output);
                    UpdateResult::Success {
                        updated_count: count,
                        updated_items: Vec::new(),
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::descriptor::{
        CapabilitySet, CommandSet, DetectionConfig, ParserDef, PluginMetadata, PrivilegeConfig,
    };
    use super::*;
    use crate::executor::test_utils::MockExecutor;
    use std::collections::HashMap;

    fn test_descriptor(needs_root: bool, environment: HashMap<String, String>) -> PluginDescriptor {
        PluginDescriptor {
            schema_version: 1,
            id: "testplug".into(),
            display_name: "Test Plugin".into(),
            description: String::new(),
            icon_name: String::new(),
            detection: DetectionConfig {
                binary: "testprog".into(),
                os_id: Vec::new(),
                file_exists: None,
            },
            privilege: PrivilegeConfig {
                needs_root,
                polkit_action: "io.github.up.update.system".into(),
            },
            commands: CommandSet {
                update: Some(CommandDef {
                    program: "testprog".into(),
                    args: vec!["upgrade".into()],
                    environment,
                    parser: ParserDef::LineCount {
                        pattern: "^x".into(),
                    },
                }),
                list_available: None,
                cleanup: None,
                estimate_size: None,
            },
            capabilities: CapabilitySet {
                update: true,
                list_available: false,
                cleanup: false,
                estimate_size: false,
                count_available: false,
            },
            metadata: PluginMetadata {
                author: String::new(),
                version: "1.0.0".into(),
                min_up_version: "2.0.0".into(),
                license: String::new(),
            },
        }
    }

    #[tokio::test]
    async fn needs_root_update_routes_through_pkexec_with_env() {
        let mut environment = HashMap::new();
        environment.insert("LANG".to_string(), "C".to_string());
        let backend = PluginBackend::new(test_descriptor(true, environment));
        let mock = MockExecutor::with_output("");

        let _ = backend.run_update(&mock).await;

        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        let (program, args) = &calls[0];
        assert_eq!(program, "pkexec");
        assert_eq!(
            args,
            &vec![
                "env".to_string(),
                "LANG=C".to_string(),
                "testprog".to_string(),
                "upgrade".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn non_root_update_runs_program_directly() {
        let backend = PluginBackend::new(test_descriptor(false, HashMap::new()));
        let mock = MockExecutor::with_output("");

        let _ = backend.run_update(&mock).await;

        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        let (program, args) = &calls[0];
        assert_eq!(program, "testprog");
        assert_eq!(args, &vec!["upgrade".to_string()]);
    }
}
