//! `PluginBackend` — implements the [`Backend`] trait for plugin-defined backends.

use super::descriptor::PluginDescriptor;
use super::parser;
use crate::backends::{Backend, BackendKind, UpdateResult};
use crate::executor::CommandExecutor;
use std::future::Future;
use std::pin::Pin;

/// A [`Backend`] implementation constructed from a YAML plugin descriptor.
///
/// Plugin backends delegate actual command execution to the provided
/// [`CommandExecutor`], which may route through the D-Bus daemon or
/// fall back to direct process spawning.
pub struct PluginBackend {
    descriptor: PluginDescriptor,
}

impl PluginBackend {
    /// Create a new plugin backend from a validated descriptor.
    pub fn new(descriptor: PluginDescriptor) -> Self {
        Self { descriptor }
    }

    /// Get the plugin ID.
    pub fn id(&self) -> &str {
        &self.descriptor.id
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

            let args: Vec<&str> = cmd.args.iter().map(|s| s.as_str()).collect();
            let result = runner.run(&cmd.program, &args).await;

            match result {
                Ok(output) => {
                    let count = parser::apply_parser_count(&cmd.parser, &output);
                    UpdateResult::Success {
                        updated_count: count,
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
                return UpdateResult::Success { updated_count: 0 };
            };

            let args: Vec<&str> = cmd.args.iter().map(|s| s.as_str()).collect();
            let result = runner.run(&cmd.program, &args).await;

            match result {
                Ok(output) => {
                    let count = parser::apply_parser_count(&cmd.parser, &output);
                    UpdateResult::Success {
                        updated_count: count,
                    }
                }
                Err(e) => UpdateResult::Error(e),
            }
        })
    }
}
