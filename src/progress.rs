//! Derives a real, per-backend completion fraction from the log lines that
//! backends already stream to the UI.
//!
//! Every supported package manager announces its work in one of two shapes:
//! an explicit `n/m` counter, or a plan line stating how many items will be
//! processed followed by one line per item.  [`ProgressTracker`] recognises
//! both, per [`BackendKind`], and yields a monotonically increasing fraction.
//!
//! Lines that are not recognised produce `None`, so a backend whose output
//! cannot be parsed simply shows no intra-backend progress rather than a
//! misleading one.

use crate::backends::BackendKind;
use std::collections::HashSet;

/// Minimum fraction increase before a new value is reported.  Nix build logs
/// can emit thousands of lines per second; without this every one of them
/// would queue a GTK redraw.
const MIN_DELTA: f64 = 0.005;

/// Fraction the bar is pinned to once a NixOS rebuild reaches activation:
/// the build is finished but the switch itself still has work to do.
const NIX_ACTIVATION_FRACTION: f64 = 0.95;

/// Parses one backend's output stream into a completion fraction.
///
/// Create one per backend run and feed it every output line in order.
pub struct ProgressTracker {
    kind: BackendKind,
    /// Total units of work, once the backend has announced it.
    total: Option<usize>,
    /// Units completed so far (tick counting).
    done: usize,
    /// Store paths / item IDs already counted, so an item that is announced
    /// and later confirmed is not counted twice.
    seen: HashSet<String>,
    /// Highest fraction reported so far; guarantees monotonicity.
    last: f64,
}

impl ProgressTracker {
    pub fn new(kind: BackendKind) -> Self {
        Self {
            kind,
            total: None,
            done: 0,
            seen: HashSet::new(),
            last: 0.0,
        }
    }

    /// Feed one line of backend output.
    ///
    /// Returns `Some(fraction)` only when progress advanced by at least
    /// [`MIN_DELTA`]; `None` when the line carried no usable signal.
    pub fn observe(&mut self, line: &str) -> Option<f64> {
        let raw = match self.kind {
            BackendKind::Nix => self.observe_nix(line),
            BackendKind::Flatpak => self.observe_flatpak(line),
            BackendKind::Apt => self.observe_apt(line),
            BackendKind::Dnf => Self::observe_dnf(line),
            BackendKind::Pacman | BackendKind::Zypper => Self::observe_bracketed(line),
            BackendKind::Homebrew => self.observe_homebrew(line),
            BackendKind::Fwupd => self.observe_fwupd(line),
            BackendKind::Plugin(_) => Self::observe_plugin(line),
        }?;
        self.report(raw)
    }

    /// Clamp, enforce monotonicity and apply the emission threshold.
    fn report(&mut self, fraction: f64) -> Option<f64> {
        let f = fraction.clamp(0.0, 1.0);
        if f < self.last + MIN_DELTA && f < 1.0 {
            return None;
        }
        if f <= self.last {
            return None;
        }
        self.last = f;
        Some(f)
    }

    /// Advance the tick counter for `id`, returning the resulting fraction.
    fn tick(&mut self, id: String) -> Option<f64> {
        let total = self.total?;
        if total == 0 || !self.seen.insert(id) {
            return None;
        }
        self.done = (self.done + 1).min(total);
        Some(self.done as f64 / total as f64)
    }

    // ── Nix ──────────────────────────────────────────────────────────────

    /// `nixos-rebuild` / `nix profile upgrade` announce their plan up front
    /// ("these 42 derivations will be built", "these 118 paths will be
    /// fetched") and then print one `building` / `copying path` /
    /// `downloading` line per store path.
    fn observe_nix(&mut self, line: &str) -> Option<f64> {
        let trimmed = line.trim();

        // `--print-build-logs` prefixes a builder's own output with
        // "derivation-name> ".  Those lines belong to the build, not to the
        // plan, and must never be parsed for progress markers.
        if is_nix_build_log_line(trimmed) {
            return None;
        }

        if let Some(n) = parse_nix_plan_count(trimmed) {
            self.total = Some(self.total.unwrap_or(0) + n);
            return None;
        }

        // Activation: the build is complete, the switch is still running.
        if is_nix_activation_line(trimmed) {
            return Some(NIX_ACTIVATION_FRACTION);
        }

        let path = nix_store_path_of(trimmed)?;
        self.tick(path)
    }

    // ── Flatpak ──────────────────────────────────────────────────────────

    /// `flatpak update -y` first prints a numbered transaction table, then
    /// reports each operation with an `n/m` counter.
    fn observe_flatpak(&mut self, line: &str) -> Option<f64> {
        let trimmed = line.trim();

        if is_numbered_table_row(trimmed) {
            self.total = Some(self.total.unwrap_or(0) + 1);
            return None;
        }

        if trimmed.contains("Updating")
            || trimmed.contains("Installing")
            || trimmed.contains("Downloading")
        {
            if let Some((n, m)) = find_ratio(trimmed) {
                return ratio_fraction(n, m);
            }
        }
        None
    }

    // ── APT ──────────────────────────────────────────────────────────────

    /// APT states its total up front — `2 upgraded, 1 newly installed, 0 to
    /// remove and 0 not upgraded.` — and then touches each package twice:
    /// once to unpack it and once to configure it.  Both halves are counted,
    /// so the bar advances across the whole install phase.
    fn observe_apt(&mut self, line: &str) -> Option<f64> {
        let trimmed = line.trim();

        if let Some(packages) = parse_apt_plan_count(trimmed) {
            self.total = Some(packages * 2);
            return None;
        }

        for prefix in ["Unpacking ", "Setting up "] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let name = rest.split_whitespace().next()?;
                return self.tick(format!("{prefix}{name}"));
            }
        }
        None
    }

    // ── DNF ──────────────────────────────────────────────────────────────

    /// DNF transaction lines end with a counter:
    /// `  Upgrading        : htop-3.2.2-1.fc40.x86_64      12/345`
    fn observe_dnf(line: &str) -> Option<f64> {
        let last = line.trim_end().rsplit(char::is_whitespace).next()?;
        let (n, m) = split_ratio(last)?;
        ratio_fraction(n, m)
    }

    // ── Pacman / Zypper ──────────────────────────────────────────────────

    /// Both prefix each transaction item with a bracketed counter:
    /// `( 3/42) upgrading htop` (pacman), `(3/42) Installing: htop` (zypper).
    fn observe_bracketed(line: &str) -> Option<f64> {
        let inner = line.trim_start().strip_prefix('(')?;
        let inner = inner.split(')').next()?;
        let (n, m) = split_ratio(inner.trim())?;
        ratio_fraction(n, m)
    }

    // ── Homebrew ─────────────────────────────────────────────────────────

    /// `==> Upgrading 7 outdated packages` announces the total, then each
    /// formula gets its own `==> Upgrading <name>` line.
    fn observe_homebrew(&mut self, line: &str) -> Option<f64> {
        let rest = line.trim().strip_prefix("==> Upgrading ")?;

        if let Some(count) = rest
            .strip_suffix(" outdated packages")
            .or_else(|| rest.strip_suffix(" outdated package"))
            .and_then(|n| n.trim().parse::<usize>().ok())
        {
            self.total = Some(count);
            return None;
        }

        self.tick(rest.to_string())
    }

    // ── Fwupd ────────────────────────────────────────────────────────────

    /// `fwupdmgr update` reports `Updating <device>...` per device and
    /// confirms each with `Successfully installed firmware`.
    fn observe_fwupd(&mut self, line: &str) -> Option<f64> {
        let trimmed = line.trim();

        if let Some((n, m)) = find_ratio(trimmed) {
            return ratio_fraction(n, m);
        }

        if trimmed.starts_with("Updating ") {
            self.total = Some(self.total.unwrap_or(0) + 1);
            return None;
        }

        if trimmed.contains("Successfully installed firmware") {
            let total = self.total?;
            self.done = (self.done + 1).min(total);
            return Some(self.done as f64 / total as f64);
        }
        None
    }

    // ── Plugins ──────────────────────────────────────────────────────────

    /// Plugin backends opt in by printing `up:progress:<0-100>`.
    fn observe_plugin(line: &str) -> Option<f64> {
        let value = line.trim().strip_prefix("up:progress:")?;
        let percent: f64 = value.trim().parse().ok()?;
        Some((percent / 100.0).clamp(0.0, 1.0))
    }
}

// ── Line helpers ─────────────────────────────────────────────────────────

/// `true` for builder output forwarded by `--print-build-logs`, which is
/// prefixed with `<derivation-name>> `.
fn is_nix_build_log_line(line: &str) -> bool {
    match line.find("> ") {
        // A store path or a quoted path before the marker means this is a real
        // nix status line that merely happens to contain "> ".
        Some(idx) => !line[..idx].contains('\'') && !line[..idx].contains(char::is_whitespace),
        None => false,
    }
}

/// Extracts the item count from a nix plan line, covering both the plural
/// ("these 42 derivations will be built:") and singular ("this derivation
/// will be built:") forms, for builds and fetches alike.
fn parse_nix_plan_count(line: &str) -> Option<usize> {
    if !(line.contains("will be built") || line.contains("will be fetched")) {
        return None;
    }
    if let Some(rest) = line.strip_prefix("these ") {
        return rest.split_whitespace().next()?.parse().ok();
    }
    if line.starts_with("this ") {
        return Some(1);
    }
    None
}

/// `true` once `nixos-rebuild` has left the build phase and is activating the
/// new system generation.
fn is_nix_activation_line(line: &str) -> bool {
    const MARKERS: &[&str] = &[
        "activating the configuration",
        "setting up /etc",
        "reloading user units",
        "restarting sysinit-reactivation",
        "switching to system configuration",
        "the following new units were started",
        "the following units were restarted",
    ];
    let lower = line.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

/// Returns the `/nix/store/...` path a build/fetch status line refers to.
fn nix_store_path_of(line: &str) -> Option<String> {
    const PREFIXES: &[&str] = &["building '", "copying path '", "downloading '"];
    let rest = PREFIXES.iter().find_map(|p| line.strip_prefix(p))?;
    let path = rest.split('\'').next()?;
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

/// Extracts the package count from APT's plan summary, e.g.
/// `2 upgraded, 1 newly installed, 0 to remove and 3 not upgraded.` -> 3.
fn parse_apt_plan_count(line: &str) -> Option<usize> {
    if !line.contains(" upgraded, ") || !line.contains(" newly installed") {
        return None;
    }
    let mut fields = line.split_whitespace();
    let upgraded: usize = fields.next()?.parse().ok()?;
    // "<n> upgraded, <m> newly installed, ..."
    let newly: usize = fields.nth(1)?.parse().ok()?;
    Some(upgraded + newly)
}

/// `true` for a flatpak transaction-table row such as `  1. org.gnome.Boxes ...`.
fn is_numbered_table_row(line: &str) -> bool {
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    !digits.is_empty() && line[digits.len()..].starts_with(". ")
}

/// Splits an `n/m` token into its two numbers, ignoring any trailing
/// punctuation the tool appended (flatpak writes `Updating 1/2...`).
fn split_ratio(token: &str) -> Option<(usize, usize)> {
    let (n, m) = token.split_once('/')?;
    let m: String = m
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    Some((n.trim().parse().ok()?, m.parse().ok()?))
}

/// Finds the first `n/m` token anywhere in a line.
fn find_ratio(line: &str) -> Option<(usize, usize)> {
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '[' || c == ']')
        .find_map(split_ratio)
}

/// Converts a completed-of-total counter into a fraction, rejecting the
/// nonsensical cases (`m == 0`, `n > m`).
fn ratio_fraction(n: usize, m: usize) -> Option<f64> {
    if m == 0 || n > m {
        return None;
    }
    Some(n as f64 / m as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed every line and return the last fraction reported.
    fn drive(kind: BackendKind, lines: &[&str]) -> Option<f64> {
        let mut t = ProgressTracker::new(kind);
        lines.iter().filter_map(|l| t.observe(l)).last()
    }

    /// Compare a reported fraction against an expected value.
    fn approx(actual: Option<f64>, expected: f64) {
        match actual {
            Some(f) if (f - expected).abs() < 1e-9 => {}
            other => panic!("expected ~{expected}, got {other:?}"),
        }
    }

    #[test]
    fn nix_build_plan_drives_fraction() {
        let mut t = ProgressTracker::new(BackendKind::Nix);
        assert_eq!(t.observe("these 4 derivations will be built:"), None);
        assert_eq!(t.observe("  /nix/store/aaa.drv"), None);
        assert_eq!(t.observe("building '/nix/store/aaa.drv'..."), Some(0.25));
        assert_eq!(t.observe("building '/nix/store/bbb.drv'..."), Some(0.5));
        // Repeating a path must not advance the bar.
        assert_eq!(t.observe("building '/nix/store/bbb.drv'..."), None);
        assert_eq!(t.observe("building '/nix/store/ccc.drv'..."), Some(0.75));
    }

    #[test]
    fn nix_sums_build_and_fetch_plans() {
        let mut t = ProgressTracker::new(BackendKind::Nix);
        t.observe("these 2 derivations will be built:");
        t.observe("these 2 paths will be fetched (12.34 MiB download, 56.78 MiB unpacked):");
        assert_eq!(
            t.observe("copying path '/nix/store/aaa-hello' from 'https://cache.nixos.org'..."),
            Some(0.25)
        );
    }

    #[test]
    fn nix_singular_plan_counts_as_one() {
        assert_eq!(
            parse_nix_plan_count("this derivation will be built:"),
            Some(1)
        );
        assert_eq!(
            parse_nix_plan_count("this path will be fetched (1.0 MiB download):"),
            Some(1)
        );
    }

    #[test]
    fn nix_ignores_builder_output() {
        let mut t = ProgressTracker::new(BackendKind::Nix);
        t.observe("these 2 derivations will be built:");
        // A builder printing something that looks like a plan line must not
        // be mistaken for one.
        assert_eq!(
            t.observe("hello-1.0> these 99 derivations will be built:"),
            None
        );
        assert_eq!(
            t.observe("hello-1.0> building '/nix/store/zzz.drv'..."),
            None
        );
        assert_eq!(t.observe("building '/nix/store/aaa.drv'..."), Some(0.5));
    }

    #[test]
    fn nix_pins_at_activation() {
        let mut t = ProgressTracker::new(BackendKind::Nix);
        t.observe("these 2 derivations will be built:");
        t.observe("building '/nix/store/aaa.drv'...");
        assert_eq!(t.observe("activating the configuration..."), Some(0.95));
    }

    #[test]
    fn nix_without_plan_reports_nothing() {
        assert_eq!(
            drive(BackendKind::Nix, &["building '/nix/store/aaa.drv'..."]),
            None
        );
    }

    #[test]
    fn flatpak_table_then_counter() {
        let mut t = ProgressTracker::new(BackendKind::Flatpak);
        assert_eq!(
            t.observe("        ID                         Branch  Op"),
            None
        );
        assert_eq!(t.observe(" 1. org.gnome.Platform          46      u"), None);
        assert_eq!(t.observe(" 2. org.gnome.Boxes             stable  u"), None);
        assert_eq!(
            t.observe("Updating 1/2\u{2026} org.gnome.Platform"),
            Some(0.5)
        );
        assert_eq!(t.observe("Updating 2/2\u{2026} org.gnome.Boxes"), Some(1.0));
    }

    #[test]
    fn apt_plan_then_unpack_and_configure() {
        let mut t = ProgressTracker::new(BackendKind::Apt);
        assert_eq!(t.observe("Reading package lists..."), None);
        assert_eq!(
            t.observe("2 upgraded, 0 newly installed, 0 to remove and 0 not upgraded."),
            None
        );
        // Two packages, unpacked then configured: four ticks in total.
        assert_eq!(
            t.observe("Unpacking htop (3.2.2-1) over (3.2.1-1) ..."),
            Some(0.25)
        );
        assert_eq!(
            t.observe("Unpacking jq (1.7-1) over (1.6-1) ..."),
            Some(0.5)
        );
        assert_eq!(t.observe("Setting up htop (3.2.2-1) ..."), Some(0.75));
        assert_eq!(t.observe("Setting up jq (1.7-1) ..."), Some(1.0));
    }

    #[test]
    fn apt_plan_counts_new_packages_too() {
        assert_eq!(
            parse_apt_plan_count("2 upgraded, 1 newly installed, 0 to remove and 3 not upgraded."),
            Some(3)
        );
        assert_eq!(
            parse_apt_plan_count("Get:1 http://deb.debian.org htop"),
            None
        );
    }

    #[test]
    fn dnf_trailing_counter() {
        assert_eq!(
            drive(
                BackendKind::Dnf,
                &["  Upgrading        : htop-3.2.2-1.fc40.x86_64      1/4"]
            ),
            Some(0.25)
        );
        assert_eq!(drive(BackendKind::Dnf, &["Running transaction"]), None);
    }

    #[test]
    fn pacman_and_zypper_bracketed_counter() {
        assert_eq!(
            drive(BackendKind::Pacman, &["( 1/4) upgrading htop"]),
            Some(0.25)
        );
        assert_eq!(
            drive(
                BackendKind::Zypper,
                &["(3/4) Installing: htop-3.2.2-1.x86_64"]
            ),
            Some(0.75)
        );
    }

    #[test]
    fn homebrew_plan_then_formulae() {
        let mut t = ProgressTracker::new(BackendKind::Homebrew);
        assert_eq!(t.observe("==> Upgrading 2 outdated packages"), None);
        assert_eq!(t.observe("==> Upgrading htop 3.2.1 -> 3.2.2"), Some(0.5));
        assert_eq!(t.observe("==> Upgrading jq 1.6 -> 1.7"), Some(1.0));
    }

    #[test]
    fn fwupd_counts_devices() {
        let mut t = ProgressTracker::new(BackendKind::Fwupd);
        assert_eq!(t.observe("Updating System Firmware\u{2026}"), None);
        assert_eq!(t.observe("Updating SSD Firmware\u{2026}"), None);
        assert_eq!(t.observe("Successfully installed firmware"), Some(0.5));
    }

    #[test]
    fn plugin_protocol_line() {
        assert_eq!(
            drive(BackendKind::Plugin("demo".into()), &["up:progress:42"]),
            Some(0.42)
        );
        assert_eq!(
            drive(
                BackendKind::Plugin("demo".into()),
                &["installing something"]
            ),
            None
        );
    }

    #[test]
    fn fraction_never_moves_backwards() {
        let mut t = ProgressTracker::new(BackendKind::Dnf);
        assert_eq!(t.observe("  Upgrading : a   3/4"), Some(0.75));
        // A later, smaller counter (e.g. the Verifying pass restarting at 1/4)
        // must not rewind the bar.
        assert_eq!(t.observe("  Verifying : b   1/4"), None);
    }

    #[test]
    fn small_advances_are_throttled() {
        let mut t = ProgressTracker::new(BackendKind::Nix);
        t.observe("these 1000 derivations will be built:");
        // A single item out of 1000 is below MIN_DELTA and is swallowed.
        assert_eq!(t.observe("building '/nix/store/a.drv'..."), None);
        let reports: Vec<f64> = (0..10)
            .filter_map(|i| t.observe(&format!("building '/nix/store/{i}-x.drv'...")))
            .collect();
        // Ten further items produce two reports, not ten: one each time the
        // 0.005 threshold is crossed.
        assert_eq!(reports.len(), 2);
        approx(reports.first().copied(), 0.005);
        approx(reports.last().copied(), 0.010);
    }

    #[test]
    fn nonsense_counters_are_ignored() {
        assert_eq!(ratio_fraction(5, 4), None);
        assert_eq!(ratio_fraction(1, 0), None);
    }
}
