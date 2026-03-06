//! Rails command filter.
//!
//! Rust-powered filters for `rails test` (minitest state machine) and `routes`
//! (HashMap namespace grouping). Other subcommands (db:migrate, generate, etc.)
//! are handled by TOML DSL filters via `run_other()`.

use crate::tracking;
use crate::utils::{exit_code_from_output, ruby_exec, truncate};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;

// ── Common rails subcommand execution ────────────────────────────────────────

/// Execute a rails subcommand, apply a filter to stdout, handle tee/exit code/tracking.
fn run_rails_filtered(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    filter: impl Fn(&str) -> String,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = ruby_exec("rails");
    cmd.arg(subcommand).args(args);

    if verbose > 0 {
        eprintln!("Running: rails {} {}", subcommand, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run rails {}. Is Rails installed?", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, &format!("rails {}", subcommand));

    let filtered = if stdout.trim().is_empty() && !output.status.success() {
        format!("Rails {}: FAILED (no stdout, see stderr below)", subcommand)
    } else {
        filter(&stdout)
    };

    let tee_label = subcommand.replace(':', "_");
    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("rails_{}", tee_label), exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && (!output.status.success() || verbose > 0) {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("rails {} {}", subcommand, args.join(" ")),
        &format!("rtk rails {} {}", subcommand, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

// ── rails test (Minitest) ────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum ParseState {
    Header,
    Failures,
    Summary,
}

pub fn run_test(args: &[String], verbose: u8) -> Result<()> {
    run_rails_filtered("test", args, verbose, filter_minitest_output)
}

// ── rails routes ─────────────────────────────────────────────────────────────

pub fn run_routes(args: &[String], verbose: u8) -> Result<()> {
    // Detect grep/controller flags — user is already filtering
    let has_grep = args
        .iter()
        .any(|a| a == "-g" || a == "--grep" || a == "-c" || a == "--controller");

    run_rails_filtered("routes", args, verbose, move |output| {
        filter_rails_routes(output, has_grep)
    })
}

// ── Other rails subcommands (TOML filter fallback → passthrough) ─────────────

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("rails: no subcommand specified");
    }

    let subcommand = args[0].to_string_lossy().to_string();

    // Build the full command string for TOML filter matching
    let full_args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    let raw_command = format!("rails {}", full_args.join(" "));

    let timer = tracking::TimedExecution::start();

    // Try TOML filter first (handles db:migrate, db:rollback, generate, etc.)
    let toml_match = if std::env::var("RTK_NO_TOML").is_ok() {
        None
    } else {
        crate::toml_filter::find_matching_filter(&raw_command)
    };

    if let Some(filter) = toml_match {
        let mut cmd = ruby_exec("rails");
        cmd.args(args);

        if verbose > 0 {
            eprintln!("Running (TOML filter): {}", raw_command);
        }

        let output = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .output()
            .with_context(|| format!("Failed to run rails {}", subcommand))?;

        let stdout_raw = String::from_utf8_lossy(&output.stdout);
        let exit_code = exit_code_from_output(&output, &format!("rails {}", subcommand));

        if !output.status.success() {
            let _ = crate::tee::tee_and_hint(&stdout_raw, &raw_command, exit_code);
        }

        let filtered = crate::toml_filter::apply_filter(filter, &stdout_raw);
        print!("{}", filtered);

        timer.track(
            &raw_command,
            &format!("rtk:toml {}", raw_command),
            &stdout_raw,
            &filtered,
        );

        if !output.status.success() {
            std::process::exit(exit_code);
        }

        return Ok(());
    }

    // No TOML match: raw passthrough
    let mut cmd = ruby_exec("rails");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: rails {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run rails {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, &format!("rails {}", subcommand));

    print!("{}", stdout);
    eprint!("{}", stderr);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("rails_{}", subcommand), exit_code)
    {
        println!("{}", hint);
    }

    timer.track(
        &format!("rails {}", subcommand),
        &format!("rtk rails {}", subcommand),
        &raw,
        &raw,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

// ── Filter: Minitest output ──────────────────────────────────────────────────

/// Check if a line is a non-actionable minitest noise line.
fn is_minitest_noise(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("Run options:")
        || t.starts_with("# Running:")
        || t.starts_with("Running ")
        || t.starts_with("Finished in ")
        // Dot-progress lines like "..F.E.": all chars are .|F|E|S and length > 1
        || (t.len() > 1
            && t.chars().all(|c| c == '.' || c == 'F' || c == 'E' || c == 'S'))
}

/// Check if a line starts a numbered failure block: "  1) Failure:" or "  2) Error:"
fn is_numbered_minitest_failure(line: &str) -> bool {
    let t = line.trim();
    if let Some(pos) = t.find(')') {
        let prefix = &t[..pos];
        let suffix = t[pos + 1..].trim();
        prefix.chars().all(|c| c.is_ascii_digit())
            && !prefix.is_empty()
            && (suffix.starts_with("Failure") || suffix.starts_with("Error"))
    } else {
        false
    }
}

fn filter_minitest_output(output: &str) -> String {
    if output.trim().is_empty() {
        return "Rails test: No output".to_string();
    }

    let mut state = ParseState::Header;
    let mut summary_line = String::new();
    let mut failure_blocks: Vec<String> = Vec::new();
    let mut current_failure = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip noise lines in all states
        if is_minitest_noise(line) {
            continue;
        }

        match state {
            ParseState::Header => {
                if is_numbered_minitest_failure(trimmed)
                    || trimmed.starts_with("Failure:")
                    || trimmed.starts_with("Error:")
                {
                    state = ParseState::Failures;
                    current_failure.push_str(trimmed);
                    current_failure.push('\n');
                } else if is_summary_line(trimmed) {
                    summary_line = trimmed.to_string();
                    state = ParseState::Summary;
                }
            }
            ParseState::Failures => {
                if is_summary_line(trimmed) {
                    if !current_failure.trim().is_empty() {
                        failure_blocks.push(compact_minitest_failure(&current_failure));
                    }
                    summary_line = trimmed.to_string();
                    state = ParseState::Summary;
                } else if is_numbered_minitest_failure(trimmed) {
                    // New numbered failure block
                    if !current_failure.trim().is_empty() {
                        failure_blocks.push(compact_minitest_failure(&current_failure));
                    }
                    current_failure = String::new();
                    current_failure.push_str(trimmed);
                    current_failure.push('\n');
                } else if (trimmed.starts_with("Failure:") || trimmed.starts_with("Error:"))
                    && !current_failure.trim().is_empty()
                {
                    failure_blocks.push(compact_minitest_failure(&current_failure));
                    current_failure = String::new();
                    current_failure.push_str(trimmed);
                    current_failure.push('\n');
                } else if !trimmed.is_empty() {
                    current_failure.push_str(trimmed);
                    current_failure.push('\n');
                }
            }
            ParseState::Summary => {
                break;
            }
        }
    }

    // Capture any remaining failure block
    if !current_failure.trim().is_empty() && state == ParseState::Failures {
        failure_blocks.push(compact_minitest_failure(&current_failure));
    }

    // If we found a summary line, use it
    if !summary_line.is_empty() {
        return build_minitest_summary(&summary_line, &failure_blocks);
    }

    // Fallback: look for summary anywhere in output
    for line in output.lines().rev() {
        let t = line.trim();
        if is_summary_line(t) {
            return build_minitest_summary(t, &failure_blocks);
        }
    }

    // Last resort
    crate::utils::fallback_tail(output, "rails test", 5)
}

/// Extract test name and file:line from a minitest failure block,
/// appending up to 3 truncated message lines for context.
fn compact_minitest_failure(block: &str) -> String {
    let mut lines: Vec<&str> = block.lines().collect();
    lines.retain(|l| !l.trim().is_empty());

    let mut test_name = String::new();
    let mut file_line = String::new();
    let mut message_lines: Vec<String> = Vec::new();

    for line in &lines {
        let t = line.trim();

        // Extract test name: "TestClass#test_name [file:line]:" or after "Failure:" / "Error:"
        if t.contains('#') && t.contains('[') && t.contains(']') {
            // Format: "BookingTest#test_should_validate_dates [test/models/booking_test.rb:23]:"
            if let Some(bracket_start) = t.find('[') {
                test_name = t[..bracket_start].trim().to_string();
                // Remove leading number and ") " prefix
                if let Some(paren_pos) = test_name.find(") ") {
                    test_name = test_name[paren_pos + 2..].to_string();
                }
                if let Some(bracket_end) = t.find(']') {
                    file_line = t[bracket_start + 1..bracket_end].to_string();
                }
            }
        } else if t.contains('#') && t.ends_with(':') {
            // Format: "TestClass#test_name:"
            test_name = t.trim_end_matches(':').to_string();
            if let Some(paren_pos) = test_name.find(") ") {
                test_name = test_name[paren_pos + 2..].to_string();
            }
        } else if t.starts_with("Failure:") || t.starts_with("Error:") {
            // Unnumbered format
            continue;
        } else if t.starts_with("test/") || t.starts_with("./test/") {
            file_line = t.to_string();
        } else {
            message_lines.push(t.to_string());
        }
    }

    let mut result = String::new();
    if !test_name.is_empty() {
        result.push_str(&test_name);
    } else if let Some(first) = message_lines.first() {
        result.push_str(first);
        message_lines.remove(0);
    }

    if !file_line.is_empty() {
        result.push_str(&format!("\n   {}", file_line));
    }

    for msg in message_lines.iter().take(3) {
        result.push_str(&format!("\n   {}", truncate(msg, 120)));
    }

    result
}

fn is_summary_line(line: &str) -> bool {
    line.contains("runs,") && line.contains("assertions,") && line.contains("failures,")
}

fn build_minitest_summary(summary: &str, failures: &[String]) -> String {
    let parts: Vec<&str> = summary.split(',').collect();

    let runs = match extract_count(parts.first().unwrap_or(&"")) {
        Some(r) => r,
        None => return format!("Rails test: {}", summary),
    };
    let assertions = extract_count(parts.get(1).unwrap_or(&"")).unwrap_or(0);
    let failure_count = extract_count(parts.get(2).unwrap_or(&"")).unwrap_or(0);
    let errors = extract_count(parts.get(3).unwrap_or(&"")).unwrap_or(0);
    let skips = extract_count(parts.get(4).unwrap_or(&"")).unwrap_or(0);

    // Sanity check: if runs is 0, parsing likely failed — show raw summary
    if runs == 0 {
        return format!("Rails test: {}", summary);
    }

    // Warn if summary mentions failures but we parsed 0 — may indicate format change
    if failure_count == 0 && runs > 0 && summary.contains("failure") {
        let raw_failure_part = parts.get(2).unwrap_or(&"").trim();
        if raw_failure_part.contains(|c: char| c.is_ascii_digit())
            && !raw_failure_part.starts_with('0')
        {
            eprintln!(
                "[rtk] rails test: warning: could not parse failure count from '{}'",
                raw_failure_part
            );
        }
    }

    // Parallel sanity check for errors field
    if errors == 0 && runs > 0 && summary.contains("error") {
        let raw_error_part = parts.get(3).unwrap_or(&"").trim();
        if raw_error_part.contains(|c: char| c.is_ascii_digit()) && !raw_error_part.starts_with('0')
        {
            eprintln!(
                "[rtk] rails test: warning: could not parse error count from '{}'",
                raw_error_part
            );
        }
    }

    if failure_count == 0 && errors == 0 {
        let mut result = format!(
            "\u{2713} Rails test: {} passed ({} assertions)",
            runs, assertions
        );
        if skips > 0 {
            result.push_str(&format!(", {} skipped", skips));
        }
        return result;
    }

    let mut result = format!(
        "Rails test: {} runs, {} failures, {} errors\n",
        runs, failure_count, errors
    );
    result.push_str("\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\n");

    for (i, failure) in failures.iter().take(5).enumerate() {
        result.push_str(&format!("\n{}. \u{274c} {}\n", i + 1, failure));
    }

    if failures.len() > 5 {
        result.push_str(&format!("\n... +{} more failures\n", failures.len() - 5));
    }

    result.trim().to_string()
}

fn extract_count(part: &str) -> Option<usize> {
    part.split_whitespace().next().and_then(|s| s.parse().ok())
}

// ── Filter: Rails routes ─────────────────────────────────────────────────────

/// Known mounted engine paths
const MOUNTED_ENGINES: &[&str] = &[
    "sidekiq",
    "active_storage",
    "activestorage",
    "action_mailbox",
    "rails/conductor",
    "letter_opener",
    "blazer",
    "flipper",
    "good_job",
];

fn filter_rails_routes(output: &str, has_grep: bool) -> String {
    if output.trim().is_empty() {
        return "Rails routes: No routes found".to_string();
    }

    // Parse routes from output
    let mut unparsed_count = 0;
    let mut parsed_routes: Vec<(String, String, String)> = Vec::new(); // (verb, uri, controller#action)

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Prefix") || trimmed.starts_with("--") {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 3 {
            let (verb_idx, verb) = parts
                .iter()
                .enumerate()
                .find(|(_, p)| matches!(**p, "GET" | "POST" | "PUT" | "PATCH" | "DELETE"))
                .map(|(i, v)| (i, *v))
                .unwrap_or((0, ""));

            if !verb.is_empty() && verb_idx + 2 <= parts.len() {
                let uri = parts.get(verb_idx + 1).unwrap_or(&"");
                let controller = parts.get(verb_idx + 2).unwrap_or(&"");

                // Strip (.:format) from URI
                let clean_uri = uri.replace("(.:format)", "");

                parsed_routes.push((verb.to_string(), clean_uri, controller.to_string()));
            } else {
                unparsed_count += 1;
            }
        } else {
            unparsed_count += 1;
        }
    }

    let route_count = parsed_routes.len();

    if route_count == 0 {
        return "Rails routes: No routes found".to_string();
    }

    // Grep mode: compact whitespace, return matching routes directly
    if has_grep {
        let mut result = format!("Routes: {} matched\n", route_count);
        for (verb, uri, ctrl) in &parsed_routes {
            result.push_str(&format!("  {} {} {}\n", verb, uri, ctrl));
        }
        return result.trim().to_string();
    }

    // Full mode: namespace-based grouping
    // Extract namespace from controller: "admin/accesses#index" → "admin/"
    let mut namespaces: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut mounted: HashMap<String, usize> = HashMap::new();

    for (_verb, uri, ctrl) in &parsed_routes {
        // Check for mounted engines (single scan)
        if let Some(engine_name) = MOUNTED_ENGINES
            .iter()
            .find(|eng| uri.contains(&format!("/{}/", eng)) || ctrl.contains(*eng))
        {
            *mounted.entry(engine_name.to_string()).or_insert(0) += 1;
            continue;
        }

        // Extract namespace and resource from controller
        let ctrl_path = ctrl.split('#').next().unwrap_or(ctrl);
        let parts: Vec<&str> = ctrl_path.split('/').collect();

        let (namespace, resource) = if parts.len() > 1 {
            let ns = parts[..parts.len() - 1].join("/");
            let res = parts[parts.len() - 1].to_string();
            (format!("{}/", ns), res)
        } else {
            ("[root]".to_string(), ctrl_path.to_string())
        };

        *namespaces
            .entry(namespace)
            .or_default()
            .entry(resource)
            .or_insert(0) += 1;
    }

    let mut result = format!("Routes: {} total\n", route_count);

    // Sort namespaces by total route count (descending)
    let mut ns_totals: Vec<(String, usize, &HashMap<String, usize>)> = namespaces
        .iter()
        .map(|(ns, resources)| {
            let total: usize = resources.values().sum();
            (ns.clone(), total, resources)
        })
        .collect();
    ns_totals.sort_by(|a, b| b.1.cmp(&a.1));

    for (ns, total, resources) in &ns_totals {
        result.push_str(&format!("\n{} ({} routes)\n", ns, total));

        // Sort resources by count descending
        let mut res_list: Vec<(&String, &usize)> = resources.iter().collect();
        res_list.sort_by(|a, b| b.1.cmp(a.1));

        let compact: Vec<String> = res_list
            .iter()
            .take(10)
            .map(|(name, count)| format!("{} ({})", name, count))
            .collect();
        result.push_str(&format!("  {}\n", compact.join(" ")));

        if res_list.len() > 10 {
            result.push_str(&format!("  ... +{} more\n", res_list.len() - 10));
        }
    }

    // Mounted engines
    if !mounted.is_empty() {
        let mut mounted_list: Vec<(String, usize)> = mounted.into_iter().collect();
        mounted_list.sort_by(|a, b| b.1.cmp(&a.1));
        for (engine, count) in &mounted_list {
            result.push_str(&format!("\n[mounted] {} ({} routes)\n", engine, count));
        }
    }

    if unparsed_count > 0 {
        result.push_str(&format!(
            "\n({} routes could not be parsed)\n",
            unparsed_count
        ));
    }

    result.trim().to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::count_tokens;

    // ── Minitest tests ───────────────────────────────────────────────────────

    #[test]
    fn test_filter_minitest_all_pass() {
        let output = r#"Running 5 tests in a single process (parallelized)
Run options: --seed 12345

# Running:

.....

Finished in 0.1234s, 40.5195 runs/s, 80.0 assertions/s.
5 runs, 10 assertions, 0 failures, 0 errors, 0 skips
"#;
        let result = filter_minitest_output(output);
        assert!(result.starts_with("\u{2713} Rails test:"));
        assert!(result.contains("5 passed"));
        assert!(result.contains("10 assertions"));
    }

    #[test]
    fn test_filter_minitest_with_failures() {
        let output = r#"Run options: --seed 54321

# Running:

..F.

Failure:
UsersControllerTest#test_should_create_user [test/controllers/users_controller_test.rb:25]:
Expected: true
  Actual: false

4 runs, 8 assertions, 1 failures, 0 errors, 0 skips
"#;
        let result = filter_minitest_output(output);
        assert!(result.contains("1 failures"));
        assert!(result.contains("\u{274c}"));
        assert!(result.contains("UsersControllerTest"));
    }

    #[test]
    fn test_filter_minitest_no_output() {
        let result = filter_minitest_output("");
        assert_eq!(result, "Rails test: No output");
    }

    #[test]
    fn test_filter_minitest_with_errors() {
        let output = r#"Run options: --seed 11111

# Running:

E.

Error:
UsersControllerTest#test_should_show_user:
NoMethodError: undefined method `name' for nil
    test/controllers/users_controller_test.rb:15

2 runs, 3 assertions, 0 failures, 1 errors, 0 skips
"#;
        let result = filter_minitest_output(output);
        assert!(result.contains("1 errors"));
        assert!(result.contains("\u{274c}"));
    }

    #[test]
    fn test_filter_minitest_with_skips() {
        let output = "3 runs, 5 assertions, 0 failures, 0 errors, 2 skips\n";
        let result = filter_minitest_output(output);
        assert!(result.contains("\u{2713} Rails test:"));
        assert!(result.contains("2 skipped"));
    }

    #[test]
    fn test_is_summary_line() {
        assert!(is_summary_line(
            "5 runs, 10 assertions, 0 failures, 0 errors, 0 skips"
        ));
        assert!(!is_summary_line("Running tests..."));
        assert!(!is_summary_line("Finished in 0.1234s"));
    }

    #[test]
    fn test_extract_count() {
        assert_eq!(extract_count("5 runs"), Some(5));
        assert_eq!(extract_count(" 10 assertions"), Some(10));
        assert_eq!(extract_count(" 0 failures"), Some(0));
        assert_eq!(extract_count(""), None);
    }

    #[test]
    fn test_filter_minitest_numbered_failures() {
        let output = r#"Run options: --seed 99999

# Running:

...F..E.

  1) Failure:
BookingTest#test_should_validate_dates [test/models/booking_test.rb:23]:
Expected: true
  Actual: false

  2) Error:
UserTest#test_should_send_email [test/models/user_test.rb:45]:
NoMethodError: undefined method `deliver_now' for nil
    test/models/user_test.rb:46
    test/models/user_test.rb:12

8 runs, 15 assertions, 1 failures, 1 errors, 0 skips
"#;
        let result = filter_minitest_output(output);
        // Should detect numbered failure format
        assert!(result.contains("\u{274c}"));
        assert!(result.contains("1 failures"));
        assert!(result.contains("1 errors"));
        assert!(result.contains("BookingTest#test_should_validate_dates"));
        assert!(result.contains("test/models/booking_test.rb:23"));
        assert!(result.contains("UserTest#test_should_send_email"));
        // Noise lines stripped
        assert!(!result.contains("Run options:"));
        assert!(!result.contains("# Running:"));
        assert!(!result.contains("...F..E."));
    }

    #[test]
    fn test_filter_minitest_clean_failure_format() {
        let output = r#"Running 3 tests in a single process (parallelized)
Run options: --seed 42

# Running:

..F

  1) Failure:
OrdersControllerTest#test_should_create_order [test/controllers/orders_controller_test.rb:18]:
Expected response to be a <2XX: success>, but was a <422: Unprocessable Entity>

3 runs, 6 assertions, 1 failures, 0 errors, 0 skips
"#;
        let result = filter_minitest_output(output);
        // Clean format: test name on one line, file:line indented below, message indented below
        assert!(result.contains("OrdersControllerTest#test_should_create_order"));
        assert!(result.contains("test/controllers/orders_controller_test.rb:18"));
        assert!(result.contains("422"));
        // Noise stripped
        assert!(!result.contains("Run options:"));
        assert!(!result.contains("# Running:"));
        assert!(!result.contains("..F"));
        assert!(!result.contains("Finished in"));
    }

    #[test]
    fn test_filter_minitest_strips_noise_on_pass() {
        let output = r#"Running 10 tests in a single process (parallelized)
Run options: --seed 55555

# Running:

..........

Finished in 2.3456s, 4.2667 runs/s, 17.0666 assertions/s.
10 runs, 40 assertions, 0 failures, 0 errors, 0 skips
"#;
        let result = filter_minitest_output(output);
        assert!(result.starts_with("\u{2713} Rails test:"));
        assert!(result.contains("10 passed"));
        // All noise lines stripped
        assert!(!result.contains("Run options:"));
        assert!(!result.contains("# Running:"));
        assert!(!result.contains(".........."));
        assert!(!result.contains("Finished in"));
    }

    #[test]
    fn test_minitest_token_savings() {
        let input = r#"Running 20 tests in a single process (parallelized)
Run options: --seed 12345

# Running:

..........F.......E.

  1) Failure:
BookingTest#test_should_validate_dates [test/models/booking_test.rb:23]:
Expected: true
  Actual: false

  2) Error:
UserTest#test_should_send_email [test/models/user_test.rb:45]:
NoMethodError: undefined method `deliver_now' for nil
    test/models/user_test.rb:46
    app/models/user.rb:31
    test/models/user_test.rb:12

Finished in 5.6789s, 3.5224 runs/s, 14.0896 assertions/s.
20 runs, 80 assertions, 1 failures, 1 errors, 0 skips
"#;
        let output = filter_minitest_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 35.0,
            "Minitest filter: expected >=35% savings, got {:.1}%",
            savings
        );
    }

    // ── Routes tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_filter_rails_routes() {
        let output = r#"                   Prefix Verb   URI Pattern                    Controller#Action
                     root GET    /                              pages#home
                    users GET    /users(.:format)               users#index
                          POST   /users(.:format)               users#create
                 new_user GET    /users/new(.:format)           users#new
                edit_user GET    /users/:id/edit(.:format)      users#edit
                     user GET    /users/:id(.:format)           users#show
                          PATCH  /users/:id(.:format)           users#update
                          PUT    /users/:id(.:format)           users#update
                          DELETE /users/:id(.:format)           users#destroy
                    posts GET    /posts(.:format)               posts#index
                     post GET    /posts/:id(.:format)           posts#show
"#;
        let result = filter_rails_routes(output, false);
        assert!(result.contains("11 total"));
        assert!(result.contains("users"));
        assert!(result.contains("posts"));
        assert!(result.contains("pages"));
    }

    #[test]
    fn test_filter_rails_routes_empty() {
        let result = filter_rails_routes("", false);
        assert!(result.contains("No routes found"));
    }

    #[test]
    fn test_filter_rails_routes_namespace_grouping() {
        let output = r#"    Prefix Verb   URI Pattern                           Controller#Action
     admin_users GET    /admin/users(.:format)              admin/users#index
     admin_user GET    /admin/users/:id(.:format)           admin/users#show
     admin_posts GET    /admin/posts(.:format)              admin/posts#index
     admin_post GET    /admin/posts/:id(.:format)           admin/posts#show
           users GET    /users(.:format)                    users#index
            user GET    /users/:id(.:format)                users#show
"#;
        let result = filter_rails_routes(output, false);
        assert!(result.contains("6 total"));
        // Namespace grouping
        assert!(result.contains("admin/"), "should group admin namespace");
        assert!(result.contains("[root]"), "should group root-level routes");
    }

    #[test]
    fn test_filter_rails_routes_grep_mode() {
        let output = r#"    Prefix Verb   URI Pattern                           Controller#Action
     admin_users GET    /admin/users(.:format)              admin/users#index
     admin_user GET    /admin/users/:id(.:format)           admin/users#show
"#;
        let result = filter_rails_routes(output, true);
        assert!(result.contains("2 matched"));
        // Grep mode: compact, per-route listing
        assert!(result.contains("GET /admin/users"));
        assert!(!result.contains("(.:format)"), "should strip format suffix");
    }

    #[test]
    fn test_filter_rails_routes_strips_format() {
        let output = r#"    Prefix Verb   URI Pattern             Controller#Action
     users GET    /users(.:format)        users#index
"#;
        let result = filter_rails_routes(output, true);
        assert!(!result.contains("(.:format)"));
        assert!(result.contains("/users"));
    }

    #[test]
    fn test_filter_rails_routes_mounted_engines() {
        let output = r#"    Prefix Verb   URI Pattern                           Controller#Action
     users GET    /users(.:format)                        users#index
     sidekiq      GET    /sidekiq/busy(.:format)          sidekiq/web#busy
                  GET    /sidekiq/queues(.:format)         sidekiq/web#queues
"#;
        let result = filter_rails_routes(output, false);
        assert!(result.contains("[mounted] sidekiq"));
    }

    // ── Token savings ────────────────────────────────────────────────────────

    #[test]
    fn test_token_savings_minitest() {
        let input = r#"Running 50 tests in a single process (parallelized)
Run options: --seed 12345

# Running:

..................................................

Finished in 2.5s, 20.0 runs/s, 40.0 assertions/s.
50 runs, 100 assertions, 0 failures, 0 errors, 0 skips
"#;
        let output = filter_minitest_output(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 50.0,
            "Minitest: expected >=50% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_token_savings_routes() {
        let input = r#"                   Prefix Verb   URI Pattern                    Controller#Action
                     root GET    /                              pages#home
                    users GET    /users(.:format)               users#index
                          POST   /users(.:format)               users#create
                 new_user GET    /users/new(.:format)           users#new
                edit_user GET    /users/:id/edit(.:format)      users#edit
                     user GET    /users/:id(.:format)           users#show
                          PATCH  /users/:id(.:format)           users#update
                          PUT    /users/:id(.:format)           users#update
                          DELETE /users/:id(.:format)           users#destroy
                    posts GET    /posts(.:format)               posts#index
                          POST   /posts(.:format)               posts#create
                 new_post GET    /posts/new(.:format)           posts#new
                edit_post GET    /posts/:id/edit(.:format)      posts#edit
                     post GET    /posts/:id(.:format)           posts#show
                          PATCH  /posts/:id(.:format)           posts#update
                          PUT    /posts/:id(.:format)           posts#update
                          DELETE /posts/:id(.:format)           posts#destroy
"#;
        let output = filter_rails_routes(input, false);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 50.0,
            "Routes: expected >=50% savings, got {:.1}%",
            savings
        );
    }

    // ── ANSI handling test ──────────────────────────────────────────────────

    #[test]
    fn test_filter_minitest_ansi_dot_progress() {
        // ANSI-colored dot progress line should not break filtering
        let output = "Run options: --seed 12345\n\n# Running:\n\n\x1b[32m.\x1b[0m\x1b[32m.\x1b[0m\x1b[32m.\x1b[0m\n\n3 runs, 6 assertions, 0 failures, 0 errors, 0 skips\n";
        let result = filter_minitest_output(output);
        assert!(result.contains("\u{2713} Rails test:"));
        assert!(result.contains("3 passed"));
    }
}
