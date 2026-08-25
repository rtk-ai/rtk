use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::{Context, Result};

const MAX_SEARCH_RESULTS: usize = 20;
const MAX_FALLBACK_LINES: usize = 40;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("nix");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: nix {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run nix")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let filtered = filter_nix_output(&raw);

    println!("{}", filtered);

    timer.track(
        &format!("nix {}", args.join(" ")),
        &format!("rtk nix {}", args.join(" ")),
        &raw,
        &filtered,
    );

    let code = output.status.code().unwrap_or(1);
    if !output.status.success() {
        return Ok(code);
    }

    Ok(0)
}

fn filter_nix_output(raw: &str) -> String {
    let clean = strip_ansi(raw);
    let mut entries: Vec<String> = Vec::new();
    let mut summaries: Vec<String> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut fallback: Vec<String> = Vec::new();

    let mut pending_entry: Option<String> = None;

    for line in clean.lines() {
        if line.trim().is_empty() {
            if let Some(entry) = pending_entry.take() {
                entries.push(entry);
            }
            continue;
        }

        if let Some(entry) = pending_entry.clone() {
            if line.starts_with("  ") && !line.trim().starts_with('*') {
                entries.push(format!("{entry} - {}", line.trim()));
                pending_entry = None;
                continue;
            }

            entries.push(entry);
            pending_entry = None;
        }

        let trimmed = line.trim();
        if trimmed.starts_with("* ") {
            let after_star = trimmed.trim_start_matches("* ");
            let normalized = normalize_entry_name(after_star);
            if is_low_signal_entry(&normalized) {
                pending_entry = None;
                continue;
            }
            pending_entry = Some(normalized);
            continue;
        }

        let lower = trimmed.to_lowercase();
        if lower.starts_with("error:") || lower.starts_with("warning:") {
            diagnostics.push(trimmed.to_string());
            continue;
        }

        if is_noise_line(trimmed) {
            continue;
        }

        if trimmed.starts_with("these ")
            && (trimmed.contains("will be built")
                || trimmed.contains("will be fetched")
                || trimmed.contains("will be downloaded"))
        {
            summaries.push(trimmed.to_string());
            continue;
        }

        fallback.push(trimmed.to_string());
    }

    if let Some(entry) = pending_entry.take() {
        entries.push(entry);
    }

    if !entries.is_empty() {
        let shown = entries.len().min(MAX_SEARCH_RESULTS);
        let mut out = vec![format!(
            "Nix: {} results (showing {})",
            entries.len(),
            shown
        )];
        out.extend(entries.into_iter().take(shown));
        if !diagnostics.is_empty() {
            out.push(String::new());
            out.extend(diagnostics);
        }
        return out.join("\n");
    }

    let mut out: Vec<String> = Vec::new();
    out.extend(summaries);
    out.extend(diagnostics);

    if out.is_empty() {
        out.extend(fallback.into_iter().take(MAX_FALLBACK_LINES));
    }

    if out.is_empty() {
        "ok nix".to_string()
    } else {
        out.join("\n")
    }
}

fn is_noise_line(line: &str) -> bool {
    line.starts_with("evaluating '")
        || line.starts_with("copying path '")
        || line.starts_with("copying ")
        || line.starts_with("downloading ")
        || line.starts_with("building '")
        || line.starts_with("unpacking ")
        || line.starts_with("querying info about ")
        || line.starts_with("warning: ignoring the client-specified setting")
}

fn normalize_entry_name(entry: &str) -> String {
    // nix search entries are often prefixed with: legacyPackages.<arch>.
    if let Some(rest) = entry.strip_prefix("legacyPackages.") {
        if let Some(dot_idx) = rest.find('.') {
            return rest[dot_idx + 1..].to_string();
        }
    }
    entry.to_string()
}

fn is_low_signal_entry(entry: &str) -> bool {
    entry.starts_with("tests.") || entry.contains(".tests.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_nix_search_drops_evaluating_noise() {
        let raw = r#"
evaluating 'legacyPackages.x86_64-linux.hello'...
* legacyPackages.x86_64-linux.hello (2.12.2)
  Program that produces a familiar, friendly greeting
evaluating 'legacyPackages.x86_64-linux.hello-wayland'...
* legacyPackages.x86_64-linux.hello-wayland (0-unstable)
  Hello world Wayland client
"#;
        let filtered = filter_nix_output(raw);
        assert!(filtered.contains("Nix: 2 results"));
        assert!(filtered.contains("hello (2.12.2) - Program"));
        assert!(!filtered.contains("evaluating"));
    }

    #[test]
    fn test_filter_nix_keeps_errors() {
        let raw = r#"
evaluating 'legacyPackages.x86_64-linux.foo'...
error: attribute 'foo' missing
"#;
        let filtered = filter_nix_output(raw);
        assert!(filtered.contains("error: attribute 'foo' missing"));
    }

    #[test]
    fn test_filter_nix_drops_test_entries_and_normalizes_prefix() {
        let raw = r#"
* legacyPackages.x86_64-linux.tests.foo
  test fixture
* legacyPackages.x86_64-linux.hello (2.12.2)
  Program that produces a familiar, friendly greeting
"#;
        let filtered = filter_nix_output(raw);
        assert!(!filtered.contains("tests.foo"));
        assert!(filtered.contains("hello (2.12.2)"));
        assert!(!filtered.contains("legacyPackages.x86_64-linux"));
    }
}
