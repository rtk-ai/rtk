//! Bundler package manager filter.
//!
//! Handles `bundle list`, `outdated`, `install`, and `update` with text-based
//! parsing. Unrecognized subcommands pass through to bundler directly.

use crate::tracking;
use crate::utils::exit_code_from_output;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::process::Command;

lazy_static! {
    static ref RE_INSTALLING: Regex =
        Regex::new(r"^Installing\s+(\S+)\s+(\S+)(?:\s+\(was\s+(\S+)\))?").unwrap();
}

// ── Public entry point ───────────────────────────────────────────────────────

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let subcommand = args.first().map(|s| s.as_str()).unwrap_or("");

    let (raw, filtered, exit_code) = match subcommand {
        "list" => run_filtered("list", &args[1..], verbose, filter_bundle_list)?,
        "outdated" => run_filtered("outdated", &args[1..], verbose, filter_bundle_outdated)?,
        "install" => run_filtered("install", &args[1..], verbose, filter_bundle_install)?,
        "update" => run_filtered("update", &args[1..], verbose, filter_bundle_install)?,
        _ => run_passthrough(args, verbose)?,
    };

    timer.track(
        &format!("bundle {}", args.join(" ")),
        &format!("rtk bundle {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

// ── Subcommand execution ─────────────────────────────────────────────────────

/// Execute a bundle subcommand, apply a filter to stdout, handle tee/exit code.
/// Returns (raw, filtered, exit_code) so the caller can track before exiting.
fn run_filtered(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    filter: fn(&str) -> String,
) -> Result<(String, String, i32)> {
    // bundle itself doesn't need ruby_exec (bundle exec bundle is redundant)
    let mut cmd = Command::new("bundle");
    cmd.arg(subcommand).args(args);

    if verbose > 0 {
        eprintln!("Running: bundle {} {}", subcommand, args.join(" "));
    }

    let output = cmd.output().with_context(|| {
        format!(
            "Failed to run bundle {}. Is bundler installed? Check: which bundle, ruby --version",
            subcommand
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, &format!("bundle {}", subcommand));

    let filtered = if stdout.trim().is_empty() && !output.status.success() {
        format!(
            "Bundle {}: FAILED (no stdout, see stderr below)",
            subcommand
        )
    } else {
        filter(&stdout)
    };

    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("bundle-{}", subcommand), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && (!output.status.success() || verbose > 0) {
        eprintln!("{}", stderr.trim());
    }

    Ok((raw, filtered, exit_code))
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<(String, String, i32)> {
    let mut cmd = Command::new("bundle");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: bundle {}", args.join(" "));
    }

    let output = cmd.output().with_context(|| {
        format!(
            "Failed to run bundle {}. Is bundler installed? Check: which bundle, ruby --version",
            args.join(" ")
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = exit_code_from_output(&output, &format!("bundle {}", args.join(" ")));

    print!("{}", stdout);
    eprint!("{}", stderr);

    Ok((raw.clone(), raw, exit_code))
}

// ── Filters ──────────────────────────────────────────────────────────────────

/// Filter `bundle list` output.
/// Input format: "Gems included by the bundle:\n  * gem_name (version)\n  ..."
fn filter_bundle_list(output: &str) -> String {
    let output = &crate::utils::strip_ansi(output);
    let mut gems: Vec<(&str, &str)> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        // Parse lines like "  * gem_name (1.2.3)" or "  * gem_name (1.2.3 abc123)"
        if let Some(rest) = trimmed.strip_prefix("* ") {
            if let Some(paren_pos) = rest.find('(') {
                let name = rest[..paren_pos].trim();
                let version = rest[paren_pos..].trim_matches(|c| c == '(' || c == ')');
                // Take only the version number (before any space for git hash)
                let version = version.split_whitespace().next().unwrap_or(version);
                gems.push((name, version));
            }
        }
    }

    if gems.is_empty() {
        // Check if there's an error message (avoid matching gem names like "better_errors")
        if output.contains("Could not ")
            || output.contains("Bundler could not")
            || output.contains("An error occurred")
        {
            return output.trim().to_string();
        }
        return "Bundle: No gems found".to_string();
    }

    let mut result = format!("Bundle: {} gems\n", gems.len());
    result.push_str("═══════════════════════════════════════\n");

    for (name, version) in gems.iter().take(30) {
        result.push_str(&format!("  {} ({})\n", name, version));
    }

    if gems.len() > 30 {
        result.push_str(&format!("  ... +{} more gems\n", gems.len() - 30));
    }

    result.trim().to_string()
}

/// Filter `bundle outdated` output.
/// Input format: "Outdated gems included in the bundle:\n  * gem (newest N, installed M, requested ~> X) in group Y\n  ..."
fn filter_bundle_outdated(output: &str) -> String {
    let mut outdated: Vec<(String, String, String)> = Vec::new(); // (name, installed, newest)

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("* ") {
            // Parse: "gem_name (newest 2.0, installed 1.5, requested ~> 1.0) in group default"
            if let Some(paren_pos) = rest.find('(') {
                let name = rest[..paren_pos].trim().to_string();
                let details = rest[paren_pos..].trim();

                let newest = extract_version_field(details, "newest");
                let installed = extract_version_field(details, "installed");

                outdated.push((name, installed, newest));
            }
        }
    }

    if outdated.is_empty() {
        if output.contains("Bundle up to date")
            || output.contains("no outdated")
            || output.trim().is_empty()
        {
            return "✓ Bundle: All gems up to date".to_string();
        }
        // Might be an error or different format
        return crate::utils::fallback_tail(output, "bundle outdated", 3);
    }

    let mut result = format!("Bundle outdated: {} gems\n", outdated.len());
    result.push_str("═══════════════════════════════════════\n");

    for (i, (name, installed, newest)) in outdated.iter().take(20).enumerate() {
        result.push_str(&format!(
            "{}. {} ({} → {})\n",
            i + 1,
            name,
            installed,
            newest
        ));
    }

    if outdated.len() > 20 {
        result.push_str(&format!("\n... +{} more gems\n", outdated.len() - 20));
    }

    result.push_str("\nRun `bundle update <gem>` to update");

    result.trim().to_string()
}

/// Filter `bundle install` / `bundle update` output.
/// Detect "Installing X Y (was Z)" for updates, strip noise lines, keep post-install messages.
fn filter_bundle_install(output: &str) -> String {
    let mut installed: Vec<String> = Vec::new(); // "name version"
    let mut updated: Vec<String> = Vec::new(); // "name old → new"
    let mut using_count = 0;
    let mut summary_line = String::new();
    let mut post_install_msgs: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip noise lines
        if trimmed.starts_with("Fetching gem metadata")
            || trimmed.starts_with("Resolving dependencies")
            || trimmed.starts_with("Fetching source index")
        {
            continue;
        }

        if trimmed.starts_with("Using ") {
            using_count += 1;
        } else if let Some(caps) = RE_INSTALLING.captures(trimmed) {
            let name = caps.get(1).map_or("", |m| m.as_str());
            let version = caps.get(2).map_or("", |m| m.as_str());
            if let Some(was) = caps.get(3) {
                updated.push(format!("{} {} → {}", name, was.as_str(), version));
            } else {
                installed.push(format!("{} {}", name, version));
            }
        } else if trimmed.starts_with("Bundle complete!") || trimmed.starts_with("Bundle updated!")
        {
            summary_line = trimmed.to_string();
        } else if trimmed.starts_with("Bundler could not")
            || trimmed.starts_with("An error occurred")
            || trimmed.starts_with("Could not find gem")
            || trimmed.starts_with("There was an error")
            || trimmed.starts_with("Your Ruby version is")
            || trimmed.starts_with("Bundler::GemNotFound")
        {
            // On error, return the output from this error line onward
            return output
                .find(trimmed)
                .map(|pos| output[pos..].trim().to_string())
                .unwrap_or_else(|| crate::utils::fallback_tail(output, "bundle install", 5));
        } else if !trimmed.is_empty()
            && !trimmed.starts_with("Downloading")
            && !trimmed.starts_with("Fetching ")
        {
            // Post-install messages from gems
            post_install_msgs.push(trimmed.to_string());
        }
    }

    let total_gems = using_count + installed.len() + updated.len();

    // No changes: compact summary
    if installed.is_empty() && updated.is_empty() {
        if !summary_line.is_empty() || total_gems > 0 {
            return format!("ok ✓ bundle install ({} gems)", total_gems);
        }
        // Fallback: return last few lines
        return crate::utils::fallback_tail(output, "bundle install", 5);
    }

    // With changes
    let mut result = format!("ok ✓ bundle install ({} gems)\n", total_gems);

    if !installed.is_empty() {
        for gem in installed.iter().take(20) {
            result.push_str(&format!("  installed: {}\n", gem));
        }
        if installed.len() > 20 {
            result.push_str(&format!("  ... +{} more installed\n", installed.len() - 20));
        }
    }

    if !updated.is_empty() {
        for gem in updated.iter().take(20) {
            result.push_str(&format!("  updated: {}\n", gem));
        }
        if updated.len() > 20 {
            result.push_str(&format!("  ... +{} more updated\n", updated.len() - 20));
        }
    }

    // Keep post-install messages (can contain breaking change notices)
    if !post_install_msgs.is_empty() {
        let meaningful: Vec<&String> = post_install_msgs
            .iter()
            .filter(|m| !m.starts_with("*") || m.contains("NOTICE") || m.contains("WARNING"))
            .take(5)
            .collect();
        if !meaningful.is_empty() {
            result.push('\n');
            for msg in meaningful {
                result.push_str(&format!("  {}\n", msg));
            }
        }
    }

    result.trim().to_string()
}

/// Extract a version field from bundle outdated details string.
/// e.g., extract_version_field("(newest 2.0, installed 1.5)", "newest") -> "2.0"
fn extract_version_field(details: &str, field: &str) -> String {
    if let Some(pos) = details.find(field) {
        let after = &details[pos + field.len()..];
        let after = after.trim_start();
        let version: String = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ',' && *c != ')')
            .collect();
        if !version.is_empty() {
            return version;
        }
    }
    "unknown".to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::count_tokens;

    #[test]
    fn test_filter_bundle_list() {
        let output = r#"Gems included by the bundle:
  * actioncable (7.1.3)
  * actionmailbox (7.1.3)
  * actionmailer (7.1.3)
  * actionpack (7.1.3)
  * activerecord (7.1.3)
  * devise (4.9.3)
  * puma (6.4.2)
  * rails (7.1.3)
  * sidekiq (7.2.0)
  * turbo-rails (2.0.4)
"#;
        let result = filter_bundle_list(output);
        assert!(result.contains("10 gems"));
        assert!(result.contains("rails (7.1.3)"));
        assert!(result.contains("devise (4.9.3)"));
    }

    #[test]
    fn test_filter_bundle_list_empty() {
        let result = filter_bundle_list("");
        assert!(result.contains("No gems found"));
    }

    #[test]
    fn test_filter_bundle_list_with_git_hash() {
        let output = r#"Gems included by the bundle:
  * my_gem (1.2.3 abc1234)
  * other_gem (2.0.0)
"#;
        let result = filter_bundle_list(output);
        assert!(result.contains("2 gems"));
        assert!(result.contains("my_gem (1.2.3)"));
        assert!(result.contains("other_gem (2.0.0)"));
    }

    #[test]
    fn test_filter_bundle_outdated_none() {
        let result = filter_bundle_outdated("Bundle up to date!");
        assert!(result.contains("✓ Bundle"));
        assert!(result.contains("up to date"));
    }

    #[test]
    fn test_filter_bundle_outdated_some() {
        let output = r#"Outdated gems included in the bundle:
  * faker (newest 3.3.1, installed 3.2.0, requested ~> 3.0) in group development, test
  * devise (newest 4.9.4, installed 4.9.3, requested ~> 4.9) in group default
  * puma (newest 6.5.0, installed 6.4.2) in group default
"#;
        let result = filter_bundle_outdated(output);
        assert!(result.contains("3 gems"));
        assert!(result.contains("faker (3.2.0 → 3.3.1)"));
        assert!(result.contains("devise (4.9.3 → 4.9.4)"));
        assert!(result.contains("puma (6.4.2 → 6.5.0)"));
    }

    #[test]
    fn test_filter_bundle_install_all_cached() {
        let output = r#"Using rake 13.1.0
Using concurrent-ruby 1.2.3
Using activesupport 7.1.3
Using rails 7.1.3
Bundle complete! 50 gems, 3 git sources
"#;
        let result = filter_bundle_install(output);
        assert!(result.contains("ok ✓ bundle install"));
        assert!(result.contains("4 gems"));
        assert!(!result.contains("Using rake"));
    }

    #[test]
    fn test_filter_bundle_install_new_gems() {
        let output = r#"Using rake 13.1.0
Using concurrent-ruby 1.2.3
Installing faker 3.3.1
Installing devise 4.9.4
Bundle complete! 52 gems, 3 git sources
"#;
        let result = filter_bundle_install(output);
        assert!(result.contains("ok ✓ bundle install"));
        assert!(result.contains("installed: faker 3.3.1"));
        assert!(result.contains("installed: devise 4.9.4"));
        assert!(!result.contains("Using rake"));
    }

    #[test]
    fn test_filter_bundle_install_with_updates() {
        let output = r#"Fetching gem metadata from https://rubygems.org/.........
Resolving dependencies...
Using rake 13.2.1
Using concurrent-ruby 1.3.5
Installing sidekiq 8.1.1 (was 8.0.0)
Installing stripe 18.4.0
Bundle complete! 142 gems, 387 total gems installed.
"#;
        let result = filter_bundle_install(output);
        assert!(result.contains("ok ✓ bundle install"));
        assert!(
            result.contains("updated: sidekiq 8.0.0 → 8.1.1"),
            "should detect update pattern: {}",
            result
        );
        assert!(result.contains("installed: stripe 18.4.0"));
        assert!(!result.contains("Fetching gem metadata"));
        assert!(!result.contains("Resolving dependencies"));
    }

    #[test]
    fn test_filter_bundle_install_failure() {
        let output = r#"Fetching gem metadata from https://rubygems.org/.........
Resolving dependencies...
Bundler could not find compatible versions for gem "activerecord":
  In Gemfile:
    rails (= 8.1.2) was resolved to 8.1.2, which depends on
      activerecord (= 8.1.2)
"#;
        let result = filter_bundle_install(output);
        assert!(result.contains("Bundler could not find"));
        assert!(result.contains("activerecord"));
    }

    #[test]
    fn test_extract_version_field() {
        assert_eq!(
            extract_version_field("(newest 3.3.1, installed 3.2.0)", "newest"),
            "3.3.1"
        );
        assert_eq!(
            extract_version_field("(newest 3.3.1, installed 3.2.0)", "installed"),
            "3.2.0"
        );
        assert_eq!(
            extract_version_field("(newest 6.5.0, installed 6.4.2)", "newest"),
            "6.5.0"
        );
    }

    #[test]
    fn test_token_savings_list() {
        let input = r#"Gems included by the bundle:
  * actioncable (7.1.3)
  * actionmailbox (7.1.3)
  * actionmailer (7.1.3)
  * actionpack (7.1.3)
  * actiontext (7.1.3)
  * actionview (7.1.3)
  * activejob (7.1.3)
  * activemodel (7.1.3)
  * activerecord (7.1.3)
  * activestorage (7.1.3)
  * activesupport (7.1.3)
  * bootsnap (1.18.3)
  * builder (3.2.4)
  * concurrent-ruby (1.2.3)
  * devise (4.9.3)
  * erubi (1.12.0)
  * globalid (1.2.1)
  * i18n (1.14.4)
  * jbuilder (2.12.0)
  * loofah (2.22.0)
  * mail (2.8.1)
  * marcel (1.0.4)
  * method_source (1.0.0)
  * minitest (5.22.3)
  * msgpack (1.7.2)
  * net-imap (0.4.10)
  * net-pop (0.1.2)
  * net-smtp (0.5.0)
  * nio4r (2.7.0)
  * nokogiri (1.16.3)
  * puma (6.4.2)
  * racc (1.7.3)
  * rack (3.0.9)
  * rack-session (2.0.0)
  * rack-test (2.1.0)
  * rackup (2.1.0)
  * rails (7.1.3)
  * rails-dom-testing (2.2.0)
  * rails-html-sanitizer (1.6.0)
  * railties (7.1.3)
  * rake (13.1.0)
  * rdoc (6.6.3)
  * redis (5.1.0)
  * reline (0.5.0)
  * sidekiq (7.2.0)
  * sprockets (4.2.1)
  * sprockets-rails (3.4.2)
  * stimulus-rails (1.3.3)
  * thor (1.3.1)
  * turbo-rails (2.0.4)
  * tzinfo (2.0.6)
  * web-console (4.2.1)
  * websocket-driver (0.7.6)
  * websocket-extensions (0.1.5)
"#;
        let output = filter_bundle_list(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 10.0,
            "Bundle list: expected ≥10% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_token_savings_outdated() {
        let input = r#"Outdated gems included in the bundle:
  * faker (newest 3.3.1, installed 3.2.0, requested ~> 3.0) in group development, test
  * devise (newest 4.9.4, installed 4.9.3, requested ~> 4.9) in group default
  * puma (newest 6.5.0, installed 6.4.2) in group default
  * sidekiq (newest 7.3.0, installed 7.2.0, requested ~> 7.0) in group default
  * turbo-rails (newest 2.1.0, installed 2.0.4, requested ~> 2.0) in group default
"#;
        let output = filter_bundle_outdated(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 30.0,
            "Bundle outdated: expected ≥30% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_token_savings_install() {
        let input = r#"Fetching gem metadata from https://rubygems.org/.........
Resolving dependencies...
Using rake 13.2.1
Using concurrent-ruby 1.3.5
Using activesupport 7.1.3
Using rails-dom-testing 2.2.0
Using rails-html-sanitizer 1.6.0
Using actionview 7.1.3
Using actionpack 7.1.3
Using activemodel 7.1.3
Using activerecord 7.1.3
Using devise 4.9.3
Using puma 6.4.2
Using sidekiq 7.2.0
Using turbo-rails 2.0.4
Installing faker 3.3.1
Installing stripe 18.4.0
Bundle complete! 52 gems, 387 total gems installed.
"#;
        let output = filter_bundle_install(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 30.0,
            "Bundle install: expected ≥30% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── ANSI handling test ──────────────────────────────────────────────────

    #[test]
    fn test_filter_bundle_list_with_ansi() {
        // ANSI-colored gem names should be stripped and parsed correctly
        let output = "Gems included by the bundle:\n  * \x1b[32mrails\x1b[0m (7.1.3)\n  * \x1b[32mpuma\x1b[0m (6.4.2)\n";
        let result = filter_bundle_list(output);
        assert!(result.contains("2 gems"), "should find 2 gems: {}", result);
        assert!(
            result.contains("rails"),
            "should parse gem name 'rails': {}",
            result
        );
        assert!(
            result.contains("puma"),
            "should parse gem name 'puma': {}",
            result
        );
    }
}
