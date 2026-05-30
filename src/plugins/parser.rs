//! Output parser engine for plugin backends.
//!
//! Interprets command output according to the parser definitions in YAML descriptors.

use super::descriptor::ParserDef;
use regex::Regex;

/// Apply a parser definition to command output and return a count result.
/// Used by update and cleanup operations to determine how many packages were affected.
pub fn apply_parser_count(parser: &ParserDef, output: &str) -> usize {
    match parser {
        ParserDef::RegexCount { pattern } => match Regex::new(pattern) {
            Ok(re) => output.lines().filter(|line| re.is_match(line)).count(),
            Err(_) => 0,
        },
        ParserDef::LineCount { pattern } => {
            match Regex::new(pattern) {
                Ok(re) => output.lines().filter(|line| re.is_match(line)).count(),
                Err(_) => {
                    // Fallback to simple prefix matching
                    output.lines().filter(|line| line.contains(pattern)).count()
                }
            }
        }
        ParserDef::LineField { skip_lines, .. } => {
            // Count non-empty lines after skipping headers
            output
                .lines()
                .skip(*skip_lines)
                .filter(|l| !l.trim().is_empty())
                .count()
        }
        ParserDef::ExitCode { .. } => {
            // Not applicable for count — the exit code is handled at a higher level
            0
        }
        _ => 0,
    }
}

/// Apply a parser definition to extract a list of package names from output.
/// Used by list_available to enumerate pending updates.
pub fn apply_parser_list(parser: &ParserDef, output: &str) -> Vec<String> {
    match parser {
        ParserDef::LineField {
            field_index,
            separator,
            skip_lines,
        } => output
            .lines()
            .skip(*skip_lines)
            .filter(|l| !l.trim().is_empty())
            .filter_map(|line| {
                let fields: Vec<&str> = if separator.is_empty() {
                    line.split_whitespace().collect()
                } else {
                    line.split(separator.as_str()).collect()
                };
                fields.get(*field_index).map(|f| f.trim().to_string())
            })
            .filter(|s| !s.is_empty())
            .collect(),
        ParserDef::RegexCount { pattern } => {
            // Use regex to find matching lines — each match is one item
            match Regex::new(pattern) {
                Ok(re) => output
                    .lines()
                    .filter(|line| re.is_match(line))
                    .map(|line| line.trim().to_string())
                    .collect(),
                Err(_) => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// Apply a size parser to extract byte count from output.
/// Returns None if parsing fails.
#[allow(dead_code)]
pub fn apply_parser_size(parser: &ParserDef, output: &str) -> Option<u64> {
    match parser {
        ParserDef::SizeRegex {
            pattern,
            unit_group,
        } => {
            let re = Regex::new(pattern).ok()?;
            for line in output.lines() {
                if let Some(caps) = re.captures(line) {
                    if let Some(m) = caps.get(*unit_group) {
                        let value_str = m.as_str();
                        if let Ok(value) = value_str.parse::<f64>() {
                            // Try to detect unit from the surrounding text
                            let full_match = caps.get(0)?.as_str();
                            let multiplier = detect_size_multiplier(full_match);
                            return Some((value * multiplier) as u64);
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Detect the byte multiplier from a size string containing unit indicators.
#[allow(dead_code)]
fn detect_size_multiplier(text: &str) -> f64 {
    let lower = text.to_ascii_lowercase();
    if lower.contains("gib") || lower.contains("gb") {
        1024.0 * 1024.0 * 1024.0
    } else if lower.contains("mib") || lower.contains("mb") {
        1024.0 * 1024.0
    } else if lower.contains("kib") || lower.contains("kb") {
        1024.0
    } else {
        1.0 // assume bytes
    }
}
