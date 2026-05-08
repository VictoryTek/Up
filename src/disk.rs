// src/disk.rs
//
// Disk-space utilities: detect available space, format byte counts,
// and parse per-backend dry-run output to estimate update sizes.

/// Detect available disk space on the root filesystem in bytes.
///
/// Runs `df -k /` synchronously with `LANG=C` and parses the
/// "Available" column.  Returns 0 on spawn failure or parse error.
pub fn detect_available_space() -> u64 {
    let result = std::process::Command::new("df")
        .args(["-k", "/"])
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output();
    match result {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_df_available(&text).unwrap_or(0)
        }
        Err(_) => 0,
    }
}

/// Parse the stdout of `df -k /` and return the available bytes.
///
/// The "Available" column is in 1-KiB blocks; multiply by 1 024.
/// Handles long filesystem names that cause `df` to wrap the data
/// row onto the next line.
pub(crate) fn parse_df_available(text: &str) -> Option<u64> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    // Skip the header line ("Filesystem  1K-blocks  Used  Available  Use%  Mounted on").
    lines.next()?;
    // First data line.
    let first = lines.next()?;
    let parts: Vec<&str> = first.split_whitespace().collect();
    let avail_str = if parts.len() >= 4 {
        // Normal case: all columns fit on one line.
        // Columns (0-based): 0=Filesystem 1=1K-blocks 2=Used 3=Available …
        parts[3]
    } else {
        // Long filesystem name caused a line-wrap.
        // The next line then contains: [1K-blocks, Used, Available, Use%, Mounted on]
        let second = lines.next()?;
        let second_parts: Vec<&str> = second.split_whitespace().collect();
        second_parts.get(2).copied()?
    };
    let kb: u64 = avail_str.parse().ok()?;
    Some(kb * 1_024)
}

/// Format a byte count as a human-readable string.
///
/// - < 1 MiB (1 048 576 bytes)    → `"N KB"`
/// - < 1 GiB (1 073 741 824 bytes) → `"N MB"`
/// - ≥ 1 GiB                       → `"N.N GB"`
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1_048_576 {
        format!("{} KB", bytes / 1_024)
    } else if bytes < 1_073_741_824 {
        format!("{} MB", bytes / 1_048_576)
    } else {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    }
}

/// Convert a numeric value and unit string into bytes.
///
/// Recognised units (case-insensitive):
/// `k`/`kb`/`kib` → ×1 024, `m`/`mb`/`mib` → ×1 048 576,
/// `g`/`gb`/`gib` → ×1 073 741 824.  Unknown units are treated as bytes.
pub(crate) fn parse_size_value(n: f64, unit: &str) -> u64 {
    match unit.to_ascii_lowercase().as_str() {
        "k" | "kb" | "kib" => (n * 1_024.0) as u64,
        "m" | "mb" | "mib" => (n * 1_048_576.0) as u64,
        "g" | "gb" | "gib" => (n * 1_073_741_824.0) as u64,
        _ => n as u64,
    }
}

/// Parse `apt-get -s upgrade` stdout to find the estimated required disk space.
///
/// Looks for the line starting with `"After this operation,"` and containing
/// `"disk space"`.  Returns `None` when space will be freed (not consumed) or
/// the line cannot be parsed.
pub fn parse_apt_size(output: &str) -> Option<u64> {
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("After this operation,") && t.contains("disk space") {
            // "After this operation, 234 MB of additional disk space will be used."
            // "After this operation, 234 kB disk space will be freed."
            if t.contains("freed") {
                return None;
            }
            let parts: Vec<&str> = t.split_whitespace().collect();
            // Tokens: ["After","this","operation,","234","MB","of",...]
            //                                        ^3    ^4
            if parts.len() >= 5 {
                if let Ok(n) = parts[3].parse::<f64>() {
                    return Some(parse_size_value(n, parts[4]));
                }
            }
        }
    }
    None
}

/// Parse `dnf upgrade --assumeno` combined stdout+stderr to estimate disk usage.
///
/// Checks lines in priority order:
/// 1. `"Disk usage after transaction:"` (DNF5)
/// 2. `"Total installed size:"` (DNF4)
/// 3. `"Total download size:"` (fallback)
pub fn parse_dnf_size(output: &str) -> Option<u64> {
    let mut download_fallback: Option<u64> = None;
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Disk usage after transaction:") || t.starts_with("Total installed size:")
        {
            if let Some(v) = parse_dnf_size_line(t) {
                return Some(v);
            }
        }
        if t.starts_with("Total download size:") && download_fallback.is_none() {
            download_fallback = parse_dnf_size_line(t);
        }
    }
    download_fallback
}

fn parse_dnf_size_line(line: &str) -> Option<u64> {
    // "Total download size: 52 M"          → after colon: "52 M"
    // "Disk usage after transaction: +141 M" → after colon: "+141 M"
    let after_colon = line.splitn(2, ':').nth(1)?.trim();
    let tokens: Vec<&str> = after_colon.split_whitespace().collect();
    if tokens.len() >= 2 {
        let num_str = tokens[0].trim_start_matches('+').trim_start_matches('-');
        if let Ok(n) = num_str.parse::<f64>() {
            return Some(parse_size_value(n, tokens[1]));
        }
    }
    None
}

/// Parse `zypper update --dry-run` stdout to estimate required disk space.
///
/// Looks for `"After the operation,"` lines.  Returns `None` if space is freed
/// or the line cannot be parsed.
pub fn parse_zypper_size(output: &str) -> Option<u64> {
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("After the operation,") {
            if t.contains("freed") {
                return None;
            }
            let parts: Vec<&str> = t.split_whitespace().collect();
            // "After the operation, additional 141 MiB will be used."
            //  0     1   2          3          4   5   …
            // "After the operation, 141 MiB will be used."
            //  0     1   2          3   4   …
            let (num_idx, unit_idx) = if parts.get(3) == Some(&"additional") {
                (4, 5)
            } else {
                (3, 4)
            };
            if let (Some(&num_str), Some(&unit)) = (parts.get(num_idx), parts.get(unit_idx)) {
                if let Ok(n) = num_str.parse::<f64>() {
                    return Some(parse_size_value(n, unit));
                }
            }
        }
    }
    None
}

/// Parse `flatpak remote-ls --updates --user --columns=download-size` output.
///
/// Each non-empty line is a size like `"234.5 MB"`, `"1.2 GB"`, `"512 kB"`,
/// or a bare number in bytes.  Sums all parsed values; returns 0 if none
/// are parseable.
pub fn parse_flatpak_sizes(output: &str) -> u64 {
    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim().split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(n) = parts[0].parse::<f64>() {
                    return Some(parse_size_value(n, parts[1]));
                }
            }
            // Bare number (bytes with no unit label).
            if parts.len() == 1 {
                return parts[0].parse::<u64>().ok();
            }
            None
        })
        .sum()
}

/// Parse total download size in bytes from `fwupdmgr get-updates --json` output.
///
/// Sums `Devices[].Releases[0].Size` (bytes) for all devices with pending firmware
/// updates.  Returns 0 on JSON parse error or when no sizes are present.
pub(crate) fn parse_fwupd_size(json_text: &str) -> u64 {
    let value: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let mut total: u64 = 0;
    if let Some(devices) = value.get("Devices").and_then(|d| d.as_array()) {
        for device in devices {
            if let Some(releases) = device.get("Releases").and_then(|r| r.as_array()) {
                if let Some(first) = releases.first() {
                    if let Some(size) = first.get("Size").and_then(|s| s.as_u64()) {
                        total += size;
                    }
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_df_available_normal() {
        let text = "Filesystem     1K-blocks      Used Available Use% Mounted on\n\
                    /dev/sda1       61255040  37123024  21001296  64% /\n";
        assert_eq!(parse_df_available(text), Some(21001296 * 1024));
    }

    #[test]
    fn test_parse_df_available_wrapped() {
        let text =
            "Filesystem                        1K-blocks    Used Available Use% Mounted on\n\
                    /dev/mapper/ubuntu--vg-ubuntu--lv\n\
                                                      102622432 5001236  97421196  5% /\n";
        // Available is the 3rd column (0-based: index 2) of the wrapped line.
        assert_eq!(parse_df_available(text), Some(97421196 * 1024));
    }

    #[test]
    fn test_format_bytes_kb() {
        assert_eq!(format_bytes(512 * 1024), "512 KB");
    }

    #[test]
    fn test_format_bytes_mb() {
        assert_eq!(format_bytes(200 * 1_048_576), "200 MB");
    }

    #[test]
    fn test_format_bytes_gb() {
        assert_eq!(format_bytes(2 * 1_073_741_824), "2.0 GB");
    }

    #[test]
    fn test_parse_size_value_units() {
        assert_eq!(parse_size_value(1.0, "kB"), 1_024);
        assert_eq!(parse_size_value(1.0, "MB"), 1_048_576);
        assert_eq!(parse_size_value(1.0, "MiB"), 1_048_576);
        assert_eq!(parse_size_value(1.0, "GB"), 1_073_741_824);
        assert_eq!(parse_size_value(1.0, "M"), 1_048_576);
    }

    #[test]
    fn test_parse_apt_size_used() {
        let output = "Reading package lists...\n\
            The following packages will be upgraded:\n  htop\n\
            After this operation, 52 MB of additional disk space will be used.\n";
        assert_eq!(parse_apt_size(output), Some(52 * 1_048_576));
    }

    #[test]
    fn test_parse_apt_size_freed_returns_none() {
        let output = "After this operation, 12 kB disk space will be freed.\n";
        assert_eq!(parse_apt_size(output), None);
    }

    #[test]
    fn test_parse_apt_size_missing() {
        let output = "0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\n";
        assert_eq!(parse_apt_size(output), None);
    }

    #[test]
    fn test_parse_dnf_size_installed() {
        let output = "Last metadata expiration check: ...\n\
            Total installed size: 123 M\n\
            Total download size: 52 M\n";
        assert_eq!(parse_dnf_size(output), Some(123 * 1_048_576));
    }

    #[test]
    fn test_parse_dnf_size_dnf5_disk_usage() {
        let output = "Disk usage after transaction: +141 M\n";
        assert_eq!(parse_dnf_size(output), Some(141 * 1_048_576));
    }

    #[test]
    fn test_parse_dnf_size_download_fallback() {
        let output = "Total download size: 52 M\n";
        assert_eq!(parse_dnf_size(output), Some(52 * 1_048_576));
    }

    #[test]
    fn test_parse_zypper_size_used() {
        let output = "Some zypper output...\n\
            After the operation, additional 141 MiB will be used.\n";
        assert_eq!(parse_zypper_size(output), Some(141 * 1_048_576));
    }

    #[test]
    fn test_parse_zypper_size_no_additional() {
        let output = "After the operation, 78 MiB will be used.\n";
        assert_eq!(parse_zypper_size(output), Some(78 * 1_048_576));
    }

    #[test]
    fn test_parse_zypper_size_freed_returns_none() {
        let output = "After the operation, 12 MiB will be freed.\n";
        assert_eq!(parse_zypper_size(output), None);
    }

    #[test]
    fn test_parse_flatpak_sizes_mixed() {
        let output = "234.5 MB\n1.2 kB\n";
        let expected = parse_size_value(234.5, "MB") + parse_size_value(1.2, "kB");
        assert_eq!(parse_flatpak_sizes(output), expected);
    }

    #[test]
    fn test_parse_flatpak_sizes_bare_bytes() {
        let output = "1048576\n";
        assert_eq!(parse_flatpak_sizes(output), 1_048_576);
    }

    #[test]
    fn test_parse_fwupd_size() {
        let json = r#"{
            "Devices": [
                {
                    "Name": "Unifying Receiver",
                    "Releases": [{ "Version": "1.0", "Size": 2097152 }]
                },
                {
                    "Name": "Firmware",
                    "Releases": [{ "Version": "2.0", "Size": 1048576 }]
                }
            ]
        }"#;
        assert_eq!(parse_fwupd_size(json), 3_145_728);
    }
}
