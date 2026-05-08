//! Battery state detection via sysfs.

use std::fs;

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryState {
    pub capacity: u8,
    pub discharging: bool,
}

/// Returns Some(BatteryState) if a battery is present; None on desktops/VMs/errors.
pub fn read_battery() -> Option<BatteryState> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = fs::read_to_string(path.join("type")).ok()?;
        if kind.trim() != "Battery" {
            continue;
        }
        let capacity: u8 = fs::read_to_string(path.join("capacity"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let status = fs::read_to_string(path.join("status")).unwrap_or_default();
        return Some(BatteryState {
            capacity,
            discharging: status.trim() == "Discharging",
        });
    }
    None
}
