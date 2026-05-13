//! Filters winget (Windows Package Manager) output.
//!
//! Handles: list, upgrade, install, uninstall, search.
//! Strips: spinners, separators, license boilerplate, progress bars.
//! Compacts: package tables to `id: version` or `id: current → available`.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Spinner artifact lines: "   - ", "   / ", "   | ", "   \ "
    static ref SPINNER_RE: Regex = Regex::new(r"^\s+[-/|\\]\s*$").unwrap();
    /// Table separator: "-----..."
    static ref SEPARATOR_RE: Regex = Regex::new(r"^-{10,}$").unwrap();
    /// Progress bar: block characters
    static ref PROGRESS_RE: Regex = Regex::new(r"[█▓▒░]{3,}").unwrap();
    /// Column splitter: 2+ spaces
    static ref COL_SPLIT_RE: Regex = Regex::new(r"  +").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let subcommand = args.first().map(|s| s.as_str()).unwrap_or("");

    let mut cmd = resolved_command("winget");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: winget {}", args.join(" "));
    }

    // Passthrough for help/version flags — don't filter help text
    let has_help = args
        .iter()
        .any(|a| matches!(a.as_str(), "--help" | "-?" | "-h" | "--version"));

    if has_help {
        return runner::run_filtered(
            cmd,
            "winget",
            &args.join(" "),
            filter_strip_noise,
            RunOptions::default(),
        );
    }

    match subcommand {
        "list" => runner::run_filtered(
            cmd,
            "winget",
            &args.join(" "),
            filter_winget_list,
            RunOptions::default(),
        ),
        "upgrade" => runner::run_filtered(
            cmd,
            "winget",
            &args.join(" "),
            filter_winget_upgrade,
            RunOptions::default(),
        ),
        "install" | "uninstall" | "reinstall" => runner::run_filtered(
            cmd,
            "winget",
            &args.join(" "),
            filter_winget_install,
            RunOptions::default(),
        ),
        _ => {
            // search, show, source, etc. — passthrough with spinner strip only
            runner::run_filtered(
                cmd,
                "winget",
                &args.join(" "),
                filter_strip_noise,
                RunOptions::default(),
            )
        }
    }
}

struct PackageRow {
    id: String,
    version: String,
    available: String,
}

fn is_known_source(s: &str) -> bool {
    matches!(s, "winget" | "msstore" | "msstorexml" | "winget-pkgs")
}

/// Parse a single data row from a winget table.
///
/// Winget uses fixed-width columns separated by 2+ spaces between column boundaries,
/// but adjacent columns (e.g. Available + Source) may be separated by only 1 space.
/// We split by 2+ spaces and handle the Available/Source edge case explicitly.
fn parse_data_row(line: &str) -> Option<PackageRow> {
    let raw: Vec<&str> = COL_SPLIT_RE
        .split(line.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if raw.len() < 3 {
        return None;
    }

    let id = raw.get(1)?.to_string();

    // Skip unmanaged ARP entries
    if id.starts_with("ARP\\") || id.starts_with("ARP/") {
        return None;
    }

    let version = raw.get(2)?.to_string();
    if version == "Версия" || version == "Version" {
        return None; // header row
    }

    // Determine available:
    // - 5+ parts → [name, id, version, available, source, ...]
    // - 4 parts  → [name, id, version, "available source"] or [name, id, version, source]
    let available = if raw.len() >= 5 {
        let candidate = raw[3].to_string();
        if is_known_source(&candidate) {
            String::new()
        } else {
            candidate
        }
    } else if raw.len() == 4 {
        let last = raw[3];
        // Could be "5.22.166.1003 winget" (available stuck to source with 1 space)
        if let Some(pos) = last.rfind(' ') {
            let potential_source = &last[pos + 1..];
            if is_known_source(potential_source) {
                last[..pos].to_string() // extract available part
            } else if is_known_source(last) {
                String::new() // pure source column, no available
            } else {
                String::new()
            }
        } else if is_known_source(last) {
            String::new()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Some(PackageRow {
        id,
        version,
        available,
    })
}

/// Extract data rows from winget table output.
fn collect_rows(input: &str) -> Vec<PackageRow> {
    let mut past_sep = false;
    let mut rows = Vec::new();

    for line in input.lines() {
        let t = line.trim_end();

        if SEPARATOR_RE.is_match(t) {
            past_sep = true;
            continue;
        }
        if !past_sep {
            continue;
        }
        if t.trim().is_empty() || SPINNER_RE.is_match(t.trim()) {
            continue;
        }

        if let Some(row) = parse_data_row(t) {
            rows.push(row);
        }
    }

    rows
}

/// `winget list` — show upgradeable packages and up-to-date count.
pub fn filter_winget_list(input: &str) -> String {
    let rows = collect_rows(input);
    if rows.is_empty() && !input.contains('-') {
        return String::new();
    }

    let mut upgradeable: Vec<String> = Vec::new();
    let mut up_to_date = 0usize;

    for row in &rows {
        if !row.available.is_empty() {
            upgradeable.push(format!("  {}: {}→{}", row.id, row.version, row.available));
        } else {
            up_to_date += 1;
        }
    }

    let mut out = String::new();

    if !upgradeable.is_empty() {
        out.push_str(&format!(
            "upgradeable ({}/{}):\n",
            upgradeable.len(),
            rows.len()
        ));
        out.push_str(&upgradeable.join("\n"));
        out.push('\n');
    }

    out.push_str(&format!("up-to-date: {}", up_to_date));
    out
}

/// `winget upgrade` — compact list of available updates.
pub fn filter_winget_upgrade(input: &str) -> String {
    let rows = collect_rows(input);
    if rows.is_empty() && !input.contains('-') {
        return String::new();
    }

    let lines: Vec<String> = rows
        .iter()
        .filter(|r| !r.available.is_empty())
        .map(|r| format!("  {}: {}→{}", r.id, r.version, r.available))
        .collect();

    let count = lines.len();
    let mut out = format!("upgrades ({}):\n", count);
    out.push_str(&lines.join("\n"));
    out
}

/// `winget install/uninstall` — strip noise, keep key status lines.
pub fn filter_winget_install(input: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();

    for line in input.lines() {
        let t = line.trim();

        if t.is_empty() || SPINNER_RE.is_match(t) || PROGRESS_RE.is_match(t) {
            continue;
        }
        if SEPARATOR_RE.is_match(t) {
            continue;
        }
        // Skip license boilerplate (Russian and English)
        if t.starts_with("Лицензия")
            || t.starts_with("License")
            || t.starts_with("Корпорация")
            || t.starts_with("Microsoft is not")
        {
            continue;
        }

        lines.push(t);
    }

    lines.join("\n")
}

/// Strip spinners and progress bars from any winget output.
pub fn filter_strip_noise(input: &str) -> String {
    input
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !SPINNER_RE.is_match(t) && !PROGRESS_RE.is_match(t)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_list_shows_upgradeable() {
        let input = include_str!("../../../tests/fixtures/winget_list_raw.txt");
        let output = filter_winget_list(input);
        assert!(output.contains("upgradeable"), "should label upgradeable section:\n{output}");
        assert!(output.contains("→"), "should show version arrow:\n{output}");
        assert!(output.contains("Docker.DockerDesktop"), "should include Docker:\n{output}");
        assert!(output.contains("up-to-date"), "should show up-to-date count:\n{output}");
    }

    #[test]
    fn test_list_skips_unmanaged() {
        let input = include_str!("../../../tests/fixtures/winget_list_raw.txt");
        let output = filter_winget_list(input);
        assert!(!output.contains("ARP\\"), "should skip unmanaged ARP entries:\n{output}");
    }

    #[test]
    fn test_list_token_savings() {
        let input = include_str!("../../../tests/fixtures/winget_list_raw.txt");
        let output = filter_winget_list(input);
        let savings = 100.0
            * (1.0 - count_tokens(&output) as f64 / count_tokens(input) as f64);
        assert!(
            savings >= 60.0,
            "expected ≥60% savings for winget list, got {:.1}%\noutput:\n{output}",
            savings
        );
    }

    #[test]
    fn test_upgrade_compact() {
        let input = include_str!("../../../tests/fixtures/winget_upgrade_raw.txt");
        let output = filter_winget_upgrade(input);
        assert!(output.starts_with("upgrades ("), "should start with count:\n{output}");
        assert!(output.contains("→"), "should show version arrow:\n{output}");
        assert!(output.contains("GitHub.cli"), "should include GitHub CLI:\n{output}");
    }

    #[test]
    fn test_upgrade_token_savings() {
        let input = include_str!("../../../tests/fixtures/winget_upgrade_raw.txt");
        let output = filter_winget_upgrade(input);
        let savings = 100.0
            * (1.0 - count_tokens(&output) as f64 / count_tokens(input) as f64);
        assert!(
            savings >= 60.0,
            "expected ≥60% savings for winget upgrade, got {:.1}%\noutput:\n{output}",
            savings
        );
    }

    #[test]
    fn test_install_strips_spinners() {
        let input = include_str!("../../../tests/fixtures/winget_install_raw.txt");
        let output = filter_winget_install(input);
        assert!(!output.contains("   -"), "should strip spinner lines:\n{output}");
        assert!(!output.contains('█'), "should strip progress bars:\n{output}");
        assert!(output.contains("Git"), "should keep found/status lines:\n{output}");
    }

    #[test]
    fn test_install_strips_boilerplate() {
        let input = include_str!("../../../tests/fixtures/winget_install_raw.txt");
        let output = filter_winget_install(input);
        assert!(!output.contains("Лицензия"), "should strip license line:\n{output}");
        assert!(!output.contains("Корпорация"), "should strip MS disclaimer:\n{output}");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(filter_winget_install(""), "");
        // list/upgrade with no table just return empty
        let list_out = filter_winget_list("");
        assert!(list_out.is_empty() || list_out.contains("up-to-date"));
    }

    #[test]
    fn test_available_bundled_with_source() {
        // winget list sometimes has only 1 space between available and source
        let row = "BlueStacks                             BlueStack.BlueStacks                   5.22.51.1038       5.22.166.1003 winget";
        let parsed = parse_data_row(row).expect("should parse row");
        assert_eq!(parsed.id, "BlueStack.BlueStacks");
        assert_eq!(parsed.version, "5.22.51.1038");
        assert_eq!(parsed.available, "5.22.166.1003");
    }

    #[test]
    fn test_no_available() {
        let row = "NVM for Windows 1.2.2                  CoreyButler.NVMforWindows              1.2.2                            winget";
        let parsed = parse_data_row(row).expect("should parse row");
        assert_eq!(parsed.id, "CoreyButler.NVMforWindows");
        assert_eq!(parsed.version, "1.2.2");
        assert!(parsed.available.is_empty(), "available should be empty");
    }
}
