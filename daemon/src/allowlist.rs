use crate::executor::ResolvedCommand;
use std::collections::HashMap;

/// Maintains the set of allowed commands per backend operation.
///
/// The daemon will ONLY execute commands that appear in this allowlist.
/// This prevents arbitrary command injection through the D-Bus interface.
pub struct CommandAllowlist {
    /// backend_id → update commands
    update_commands: HashMap<String, Vec<ResolvedCommand>>,
    /// backend_id → cleanup commands
    cleanup_commands: HashMap<String, Vec<ResolvedCommand>>,
    /// distro_id/variant → upgrade commands
    upgrade_commands: HashMap<String, Vec<ResolvedCommand>>,
    /// tool → snapshot commands
    snapshot_commands: HashMap<String, Vec<ResolvedCommand>>,
}

impl Default for CommandAllowlist {
    fn default() -> Self {
        let mut allowlist = Self {
            update_commands: HashMap::new(),
            cleanup_commands: HashMap::new(),
            upgrade_commands: HashMap::new(),
            snapshot_commands: HashMap::new(),
        };
        allowlist.register_builtin_backends();
        allowlist
    }
}

impl CommandAllowlist {
    /// Register the built-in backends that ship with Up.
    fn register_builtin_backends(&mut self) {
        // APT
        self.update_commands.insert(
            "apt".into(),
            vec![
                ResolvedCommand {
                    program: "apt".into(),
                    args: vec!["update".into()],
                    environment: vec![
                        ("DEBIAN_FRONTEND".into(), "noninteractive".into()),
                        ("LANG".into(), "C".into()),
                    ],
                },
                ResolvedCommand {
                    program: "apt".into(),
                    args: vec!["upgrade".into(), "-y".into()],
                    environment: vec![
                        ("DEBIAN_FRONTEND".into(), "noninteractive".into()),
                        ("LANG".into(), "C".into()),
                    ],
                },
            ],
        );
        self.cleanup_commands.insert(
            "apt".into(),
            vec![ResolvedCommand {
                program: "apt".into(),
                args: vec!["autoremove".into(), "-y".into()],
                environment: vec![("DEBIAN_FRONTEND".into(), "noninteractive".into())],
            }],
        );

        // DNF
        self.update_commands.insert(
            "dnf".into(),
            vec![ResolvedCommand {
                program: "dnf".into(),
                args: vec!["upgrade".into(), "-y".into(), "--refresh".into()],
                environment: vec![("LANG".into(), "C".into())],
            }],
        );
        self.cleanup_commands.insert(
            "dnf".into(),
            vec![ResolvedCommand {
                program: "dnf".into(),
                args: vec!["autoremove".into(), "-y".into()],
                environment: vec![],
            }],
        );

        // Pacman
        self.update_commands.insert(
            "pacman".into(),
            vec![ResolvedCommand {
                program: "pacman".into(),
                args: vec!["-Syu".into(), "--noconfirm".into()],
                environment: vec![("LANG".into(), "C".into())],
            }],
        );
        self.cleanup_commands.insert(
            "pacman".into(),
            vec![ResolvedCommand {
                program: "pacman".into(),
                args: vec!["-Sc".into(), "--noconfirm".into()],
                environment: vec![],
            }],
        );

        // Zypper
        self.update_commands.insert(
            "zypper".into(),
            vec![
                ResolvedCommand {
                    program: "zypper".into(),
                    args: vec!["refresh".into()],
                    environment: vec![("LANG".into(), "C".into())],
                },
                ResolvedCommand {
                    program: "zypper".into(),
                    args: vec!["update".into(), "-y".into()],
                    environment: vec![("LANG".into(), "C".into())],
                },
            ],
        );
        self.cleanup_commands.insert(
            "zypper".into(),
            vec![ResolvedCommand {
                program: "zypper".into(),
                args: vec!["clean".into(), "--all".into()],
                environment: vec![],
            }],
        );

        // Nix
        self.update_commands.insert(
            "nix".into(),
            vec![ResolvedCommand {
                program: "nixos-rebuild".into(),
                args: vec!["switch".into(), "--upgrade".into()],
                environment: vec![],
            }],
        );

        // Snapshot tools
        self.snapshot_commands.insert(
            "timeshift".into(),
            vec![ResolvedCommand {
                program: "timeshift".into(),
                args: vec![
                    "--create".into(),
                    "--comments".into(),
                    "Pre-update snapshot (Up)".into(),
                ],
                environment: vec![],
            }],
        );
        self.snapshot_commands.insert(
            "snapper".into(),
            vec![ResolvedCommand {
                program: "snapper".into(),
                args: vec![
                    "create".into(),
                    "--description".into(),
                    "Pre-update snapshot (Up)".into(),
                ],
                environment: vec![],
            }],
        );
    }

    /// Register commands from a plugin descriptor (called by the frontend
    /// when communicating plugin definitions to the daemon).
    #[allow(dead_code)]
    pub fn register_plugin(
        &mut self,
        backend_id: &str,
        update_cmds: Vec<ResolvedCommand>,
        cleanup_cmds: Vec<ResolvedCommand>,
    ) {
        if !update_cmds.is_empty() {
            self.update_commands
                .insert(backend_id.to_string(), update_cmds);
        }
        if !cleanup_cmds.is_empty() {
            self.cleanup_commands
                .insert(backend_id.to_string(), cleanup_cmds);
        }
    }

    pub fn get_update_commands(&self, backend_id: &str) -> Option<Vec<ResolvedCommand>> {
        self.update_commands.get(backend_id).cloned()
    }

    pub fn get_cleanup_commands(&self, backend_id: &str) -> Option<Vec<ResolvedCommand>> {
        self.cleanup_commands.get(backend_id).cloned()
    }

    pub fn get_upgrade_commands(
        &self,
        distro_id: &str,
        variant: &str,
    ) -> Option<Vec<ResolvedCommand>> {
        let key = format!("{}/{}", distro_id, variant);
        self.upgrade_commands.get(&key).cloned()
    }

    pub fn get_snapshot_commands(&self, tool: &str) -> Option<Vec<ResolvedCommand>> {
        self.snapshot_commands.get(tool).cloned()
    }
}
