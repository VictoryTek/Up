pub mod check;
pub mod detect;
pub mod execute;
pub mod version;

pub use check::{run_prerequisite_checks, CheckResult};
pub use detect::{
    detect_distro, detect_hostname, detect_nixos_config_type, DistroInfo, NixOsConfigType,
    UpgradePageInit,
};
pub use execute::execute_upgrade;
pub use version::{check_upgrade_available, next_nixos_channel};
