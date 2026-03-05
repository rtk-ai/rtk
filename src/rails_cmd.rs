//! Rails command filter.
//!
//! Sub-enum dispatch for `rails test`, `routes`, `db:migrate`, `db:migrate:status`,
//! `db:rollback`, and `generate`. Each subcommand has a specialized text parser.
//! Unrecognized subcommands pass through to rails directly.

use crate::tracking;
use crate::utils::{exit_code_from_output, ruby_exec, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;
use std::ffi::OsString;

lazy_static! {
    static ref RE_MIGRATED_TIME: Regex =
        Regex::new(r"(?:migrated|reverted) \((\d+\.\d+)s\)").unwrap();
    static ref RE_MIGRATE_STATUS_LINE: Regex =
        Regex::new(r"^\s*(up|down)\s+(\d+)\s+(.+)$").unwrap();
}

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

// ── rails db:migrate ─────────────────────────────────────────────────────────

pub fn run_db_migrate(args: &[String], verbose: u8) -> Result<()> {
    run_rails_filtered("db:migrate", args, verbose, filter_rails_migrate)
}

// ── rails db:migrate:status ──────────────────────────────────────────────────

pub fn run_db_migrate_status(args: &[String], verbose: u8) -> Result<()> {
    run_rails_filtered(
        "db:migrate:status",
        args,
        verbose,
        filter_rails_migrate_status,
    )
}

// ── rails db:rollback ───────────────────────────────────────────────────────

pub fn run_db_rollback(args: &[String], verbose: u8) -> Result<()> {
    // Reuse migrate filter -- rollback output has same format with "reverting" direction
    run_rails_filtered("db:rollback", args, verbose, filter_rails_migrate)
}

// ── rails generate ──────────────────────────────────────────────────────────

pub fn run_generate(args: &[String], verbose: u8) -> Result<()> {
    let generator_type = args
        .first()
        .cloned()
        .unwrap_or_else(|| "generator".to_string());
    let generator_name = args.get(1).cloned().unwrap_or_default();

    run_rails_filtered("generate", args, verbose, move |output| {
        filter_rails_generate(output, &generator_type, &generator_name)
    })
}

// ── Multi-DB variant detection ────────────────────────────────────────────────

/// Detect multi-DB migration variants (e.g. db:migrate:primary, db:rollback:animals).
/// Returns the filter type if the subcommand should be filtered, None for passthrough.
/// Note: db:migrate:status is handled by its own Clap variant, not here.
fn detect_multi_db_filter(subcommand: &str) -> Option<fn(&str) -> String> {
    if subcommand.starts_with("db:migrate:") && subcommand != "db:migrate:status" {
        return Some(filter_rails_migrate);
    }
    if subcommand.starts_with("db:rollback:") {
        return Some(filter_rails_migrate);
    }
    None
}

// ── Passthrough for other rails subcommands ──────────────────────────────────

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("rails: no subcommand specified");
    }

    let subcommand = args[0].to_string_lossy().to_string();
    let remaining: Vec<String> = args[1..]
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    // Multi-DB variants: db:migrate:primary, db:rollback:animals, etc.
    // Route to the same filters as their base commands since output format is identical.
    if let Some(filter) = detect_multi_db_filter(&subcommand) {
        return run_rails_filtered(&subcommand, &remaining, verbose, filter);
    }

    let timer = tracking::TimedExecution::start();

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
        let mut result = format!("✓ Rails test: {} passed ({} assertions)", runs, assertions);
        if skips > 0 {
            result.push_str(&format!(", {} skipped", skips));
        }
        return result;
    }

    let mut result = format!(
        "Rails test: {} runs, {} failures, {} errors\n",
        runs, failure_count, errors
    );
    result.push_str("═══════════════════════════════════════\n");

    for (i, failure) in failures.iter().take(5).enumerate() {
        result.push_str(&format!("\n{}. ❌ {}\n", i + 1, failure));
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

// ── Filter: Rails db:migrate ─────────────────────────────────────────────────

/// Filters both `db:migrate` and `db:rollback` output.
/// Detects direction from 'migrating'/'reverting' keywords.
fn filter_rails_migrate(output: &str) -> String {
    if output.trim().is_empty() {
        return "Rails migrate: No output".to_string();
    }

    let mut migrations: Vec<(String, Option<f64>)> = Vec::new(); // (name, timing)
    let mut direction = "up";
    let mut first_error: Option<String> = None;
    let mut total_time: f64 = 0.0;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("==") {
            if trimmed.contains("migrating") {
                if let Some(name) = extract_migration_name(trimmed) {
                    migrations.push((name, None));
                }
            } else if trimmed.contains("reverting") {
                direction = "down";
                if let Some(name) = extract_migration_name(trimmed) {
                    migrations.push((name, None));
                }
            } else if trimmed.contains("migrated") || trimmed.contains("reverted") {
                // Capture timing from completion lines like "migrated (0.0043s)" or "reverted (0.0035s)"
                if let Some(caps) = RE_MIGRATED_TIME.captures(trimmed) {
                    if let Some(time_str) = caps.get(1) {
                        if let Ok(t) = time_str.as_str().parse::<f64>() {
                            total_time += t;
                            if let Some(last) = migrations.last_mut() {
                                last.1 = Some(t);
                            }
                        }
                    }
                }
            }
        } else if trimmed.contains("Error:")
            || trimmed.contains("Exception:")
            || trimmed.contains("StandardError")
            || trimmed.contains("ActiveRecord::")
            || trimmed.contains("Mysql2::")
            || trimmed.contains("PG::")
            || trimmed.contains("SQLite3::")
        {
            // Capture first error line (most specific), not last (most generic)
            if first_error.is_none() {
                first_error = Some(truncate(trimmed, 200));
            }
        }
    }

    if let Some(error_msg) = &first_error {
        let mut result = format!("Rails migrate: FAILED ({})\n", direction);
        result.push_str("═══════════════════════════════════════\n");
        result.push_str(&format!("  {}\n", error_msg));
        if let Some(last) = migrations.last() {
            result.push_str(&format!("  Failed at: {}\n", last.0));
        }
        return result.trim().to_string();
    }

    if migrations.is_empty() {
        if output.contains("already up") || output.contains("Schema is up to date") {
            return "✓ Rails migrate: Schema is up to date".to_string();
        }
        return "✓ Rails migrate: No pending migrations".to_string();
    }

    let direction_label = if direction == "down" {
        "reverted"
    } else {
        "applied"
    };

    let mut result = if total_time > 0.0 {
        format!(
            "ok ✓ db:migrate ({} migrations {}, {:.2}s)\n",
            migrations.len(),
            direction_label,
            total_time
        )
    } else {
        format!(
            "ok ✓ db:migrate ({} migrations {})\n",
            migrations.len(),
            direction_label
        )
    };

    for (name, timing) in migrations.iter().take(10) {
        if let Some(t) = timing {
            result.push_str(&format!("  {} ({:.2}s)\n", name, t));
        } else {
            result.push_str(&format!("  {}\n", name));
        }
    }
    if migrations.len() > 10 {
        result.push_str(&format!(
            "  ... +{} more migrations\n",
            migrations.len() - 10
        ));
    }

    result.trim().to_string()
}

fn extract_migration_name(line: &str) -> Option<String> {
    // "== 20240201120000 CreateUsersTable: migrating ==" -> "CreateUsersTable"
    let stripped = line.trim_matches(|c: char| c == '=' || c.is_whitespace());
    let parts: Vec<&str> = stripped.split(':').next()?.split_whitespace().collect();
    if parts.len() >= 2 {
        Some(parts[1..].join(" "))
    } else {
        Some(stripped.to_string())
    }
}

// ── Filter: Rails db:migrate:status ─────────────────────────────────────────

fn filter_rails_migrate_status(output: &str) -> String {
    if output.trim().is_empty() {
        return "db:migrate:status: No output".to_string();
    }

    let mut total = 0usize;
    let mut down_migrations: Vec<(String, String)> = Vec::new(); // (id, name)

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(caps) = RE_MIGRATE_STATUS_LINE.captures(trimmed) {
            total += 1;
            let status = caps.get(1).map_or("", |m| m.as_str());
            if status == "down" {
                let id = caps.get(2).map_or("", |m| m.as_str()).to_string();
                let name = caps.get(3).map_or("", |m| m.as_str()).trim().to_string();
                down_migrations.push((id, name));
            }
        }
    }

    if total == 0 {
        return "db:migrate:status: No migrations found".to_string();
    }

    if down_migrations.is_empty() {
        return format!("db:migrate:status — {} migrations (all up)", total);
    }

    let mut result = format!(
        "db:migrate:status — {} migrations ({} pending)\n",
        total,
        down_migrations.len()
    );
    for (id, name) in &down_migrations {
        result.push_str(&format!("  down  {}  {}\n", id, name));
    }

    result.trim().to_string()
}

// ── Filter: Rails generate ──────────────────────────────────────────────────

fn filter_rails_generate(output: &str, generator_type: &str, generator_name: &str) -> String {
    if output.trim().is_empty() {
        return format!(
            "ok ✓ rails g {} {} (no output)",
            generator_type, generator_name
        );
    }

    let mut created: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("create") {
            created.push(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("remove") {
            removed.push(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("skip") {
            skipped.push(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("conflict") {
            conflicts.push(rest.trim().to_string());
        }
        // Skip "invoke" lines — they add no actionable info
    }

    let total_files = created.len() + removed.len();
    let action = if !removed.is_empty() && created.is_empty() {
        "destroy"
    } else {
        "g"
    };

    let mut result = format!(
        "ok ✓ rails {} {} {} ({} files)\n",
        action, generator_type, generator_name, total_files
    );

    for file in &created {
        result.push_str(&format!("  create {}\n", file));
    }
    for file in &removed {
        result.push_str(&format!("  remove {}\n", file));
    }
    if !skipped.is_empty() {
        result.push_str(&format!("  ({} skipped)\n", skipped.len()));
    }
    if !conflicts.is_empty() {
        for file in &conflicts {
            result.push_str(&format!("  conflict {}\n", file));
        }
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
        assert!(result.starts_with("✓ Rails test:"));
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
        assert!(result.contains("❌"));
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
        assert!(result.contains("❌"));
    }

    #[test]
    fn test_filter_minitest_with_skips() {
        let output = "3 runs, 5 assertions, 0 failures, 0 errors, 2 skips\n";
        let result = filter_minitest_output(output);
        assert!(result.contains("✓ Rails test:"));
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
        assert!(result.contains("❌"));
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
        assert!(result.starts_with("✓ Rails test:"));
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
            "Minitest filter: expected ≥35% savings, got {:.1}%",
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

    // ── Migration tests ──────────────────────────────────────────────────────

    #[test]
    fn test_filter_rails_migrate_success_with_timing() {
        let output = r#"== 20240201120000 CreateUsersTable: migrating =================================
-- create_table(:users)
   -> 0.0042s
== 20240201120000 CreateUsersTable: migrated (0.0043s) ========================

== 20240202110000 AddEmailIndexToUsers: migrating ============================
-- add_index(:users, :email, {:unique=>true})
   -> 0.0021s
== 20240202110000 AddEmailIndexToUsers: migrated (0.0022s) ====================
"#;
        let result = filter_rails_migrate(output);
        assert!(
            result.contains("ok ✓ db:migrate"),
            "should have ok marker: {}",
            result
        );
        assert!(result.contains("2 migrations applied"));
        assert!(result.contains("CreateUsersTable"));
        assert!(result.contains("AddEmailIndexToUsers"));
        // Should show per-migration timing
        assert!(result.contains("0.00s") || result.contains("(0.00"));
        // Should show total time
        assert!(result.contains("0.01s") || result.contains("0.00s"));
    }

    #[test]
    fn test_filter_rails_migrate_no_pending() {
        let result = filter_rails_migrate("");
        assert!(result.contains("No output") || result.contains("No pending migrations"));
    }

    #[test]
    fn test_filter_rails_migrate_already_up() {
        let output = "Schema is up to date.\n";
        let result = filter_rails_migrate(output);
        assert!(result.contains("up to date"));
    }

    #[test]
    fn test_extract_migration_name() {
        assert_eq!(
            extract_migration_name("== 20240201120000 CreateUsersTable: migrating =="),
            Some("CreateUsersTable".to_string())
        );
        assert_eq!(
            extract_migration_name("== 20240202110000 AddEmailIndexToUsers: migrating =="),
            Some("AddEmailIndexToUsers".to_string())
        );
    }

    // ── db:migrate:status tests ─────────────────────────────────────────────

    #[test]
    fn test_filter_rails_migrate_status_all_up() {
        let output = r#"database: myapp_development

 Status   Migration ID    Migration Name
--------------------------------------------------
   up     20200101120000  Create users
   up     20200102120000  Create parkings
   up     20200103120000  Add status to parkings
"#;
        let result = filter_rails_migrate_status(output);
        assert!(result.contains("3 migrations"));
        assert!(result.contains("all up"));
    }

    #[test]
    fn test_filter_rails_migrate_status_pending() {
        let output = r#"database: myapp_development

 Status   Migration ID    Migration Name
--------------------------------------------------
   up     20200101120000  Create users
   up     20200102120000  Create parkings
  down    20260228120000  Add status to bookings
  down    20260228130000  Create short term price lists
"#;
        let result = filter_rails_migrate_status(output);
        assert!(result.contains("4 migrations"));
        assert!(result.contains("2 pending"));
        assert!(result.contains("down  20260228120000  Add status to bookings"));
        assert!(result.contains("down  20260228130000  Create short term price lists"));
        // Should NOT include "up" migrations
        assert!(!result.contains("Create users"));
    }

    // ── db:rollback tests ───────────────────────────────────────────────────

    #[test]
    fn test_filter_rails_rollback() {
        let output = r#"== 20260228130000 CreateShortTermPriceLists: reverting =========================
-- drop_table(:short_term_price_lists)
   -> 0.0034s
== 20260228130000 CreateShortTermPriceLists: reverted (0.0035s) ================
"#;
        let result = filter_rails_migrate(output);
        assert!(result.contains("ok ✓ db:migrate"));
        assert!(result.contains("reverted"));
        assert!(result.contains("CreateShortTermPriceLists"));
    }

    // ── Multi-DB variant routing tests ──────────────────────────────────────

    #[test]
    fn test_detect_multi_db_migrate_primary() {
        assert!(detect_multi_db_filter("db:migrate:primary").is_some());
    }

    #[test]
    fn test_detect_multi_db_migrate_secondary() {
        assert!(detect_multi_db_filter("db:migrate:secondary").is_some());
    }

    #[test]
    fn test_detect_multi_db_migrate_animals() {
        assert!(detect_multi_db_filter("db:migrate:animals").is_some());
    }

    #[test]
    fn test_detect_multi_db_rollback_primary() {
        assert!(detect_multi_db_filter("db:rollback:primary").is_some());
    }

    #[test]
    fn test_detect_multi_db_status_not_routed() {
        // db:migrate:status is handled by its own Clap variant, not multi-DB routing
        assert!(detect_multi_db_filter("db:migrate:status").is_none());
    }

    #[test]
    fn test_detect_multi_db_unrelated_not_routed() {
        assert!(detect_multi_db_filter("db:seed").is_none());
        assert!(detect_multi_db_filter("console").is_none());
        assert!(detect_multi_db_filter("server").is_none());
    }

    #[test]
    fn test_filter_multi_db_migrate_output() {
        // Multi-DB migration output is identical to single-DB output
        let output = r#"== 20240201120000 CreateUsersTable: migrating =================================
-- create_table(:users)
   -> 0.0042s
== 20240201120000 CreateUsersTable: migrated (0.0043s) ========================
"#;
        let result = filter_rails_migrate(output);
        assert!(result.contains("ok ✓ db:migrate"));
        assert!(result.contains("1 migrations applied"));
        assert!(result.contains("CreateUsersTable"));
    }

    #[test]
    fn test_filter_multi_db_rollback_output() {
        // Multi-DB rollback output is identical to single-DB output
        let output = r#"== 20260228130000 CreateShortTermPriceLists: reverting =========================
-- drop_table(:short_term_price_lists)
   -> 0.0034s
== 20260228130000 CreateShortTermPriceLists: reverted (0.0035s) ================
"#;
        let result = filter_rails_migrate(output);
        assert!(result.contains("ok ✓ db:migrate"));
        assert!(result.contains("reverted"));
        assert!(result.contains("CreateShortTermPriceLists"));
    }

    // ── rails generate tests ────────────────────────────────────────────────

    #[test]
    fn test_filter_rails_generate_model() {
        let output = r#"      invoke  active_record
      create    db/migrate/20260228120000_create_short_term_price_lists.rb
      create    app/models/short_term_price_list.rb
      invoke    rspec
      create      spec/models/short_term_price_list_spec.rb
      invoke      factory_bot
      create        spec/factories/short_term_price_lists.rb
"#;
        let result = filter_rails_generate(output, "model", "ShortTermPriceList");
        assert!(result.contains("ok ✓ rails g model ShortTermPriceList"));
        assert!(result.contains("4 files"));
        assert!(result.contains("create db/migrate"));
        assert!(result.contains("create app/models"));
        // Should not contain "invoke" lines
        assert!(!result.contains("invoke"));
    }

    #[test]
    fn test_filter_rails_generate_destroy() {
        let output = r#"      invoke  active_record
      remove    db/migrate/20260228120000_create_short_term_price_lists.rb
      remove    app/models/short_term_price_list.rb
      invoke    rspec
      remove      spec/models/short_term_price_list_spec.rb
"#;
        let result = filter_rails_generate(output, "model", "ShortTermPriceList");
        assert!(result.contains("rails destroy"));
        assert!(result.contains("3 files"));
        assert!(result.contains("remove db/migrate"));
    }

    #[test]
    fn test_filter_rails_generate_with_skip() {
        let output = r#"      invoke  active_record
      create    db/migrate/20260228120000_create_things.rb
      create    app/models/thing.rb
      invoke    rspec
        skip      spec/models/thing_spec.rb
"#;
        let result = filter_rails_generate(output, "model", "Thing");
        assert!(result.contains("2 files"));
        assert!(result.contains("1 skipped"));
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
            "Minitest: expected ≥50% savings, got {:.1}%",
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
            "Routes: expected ≥50% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_token_savings_migrate() {
        let input = r#"== 20240201120000 CreateUsersTable: migrating =================================
-- create_table(:users)
   -> 0.0042s
== 20240201120000 CreateUsersTable: migrated (0.0043s) ========================

== 20240202110000 AddEmailIndexToUsers: migrating ============================
-- add_index(:users, :email, {:unique=>true})
   -> 0.0021s
== 20240202110000 AddEmailIndexToUsers: migrated (0.0022s) ====================

== 20240203100000 CreatePostsTable: migrating =================================
-- create_table(:posts)
   -> 0.0035s
== 20240203100000 CreatePostsTable: migrated (0.0036s) ========================
"#;
        let output = filter_rails_migrate(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 40.0,
            "Migrate: expected ≥40% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_rails_migrate_error() {
        let input = r#"== 20240201120000 CreateUsersTable: migrating =================================
-- create_table(:users)
ActiveRecord::StatementInvalid: PG::DuplicateTable: ERROR: relation "users" already exists
/app/db/migrate/20240201120000_create_users_table.rb:3:in `change'
"#;
        let output = filter_rails_migrate(input);
        assert!(
            output.contains("FAILED"),
            "should detect migration failure: {}",
            output
        );
        assert!(
            output.contains("CreateUsersTable"),
            "should name the failing migration: {}",
            output
        );
        assert!(
            output.contains("DuplicateTable") || output.contains("already exists"),
            "should include first error detail: {}",
            output
        );
    }

    #[test]
    fn test_token_savings_generate() {
        let input = r#"      invoke  active_record
      create    db/migrate/20260301120000_create_posts.rb
      create    app/models/post.rb
      invoke    test_unit
      create      test/models/post_test.rb
      create      test/fixtures/posts.yml
      invoke  resource_route
       route    resources :posts
      invoke  scaffold_controller
      create    app/controllers/posts_controller.rb
      invoke    erb
      create      app/views/posts
      create      app/views/posts/index.html.erb
      create      app/views/posts/edit.html.erb
      create      app/views/posts/show.html.erb
      create      app/views/posts/new.html.erb
      create      app/views/posts/_form.html.erb
      create      app/views/posts/_post.html.erb
      invoke    resource_route
      invoke    test_unit
      create      test/controllers/posts_controller_test.rb
      create      test/system/posts_test.rb
      invoke    helper
      create      app/helpers/posts_helper.rb
      invoke      test_unit
      invoke    jbuilder
      create      app/views/posts/index.json.jbuilder
      create      app/views/posts/show.json.jbuilder
      create      app/views/posts/_post.json.jbuilder
"#;
        let output = filter_rails_generate(input, "scaffold", "post");

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 20.0,
            "Rails generate: expected ≥20% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── ANSI handling test ──────────────────────────────────────────────────

    #[test]
    fn test_filter_minitest_ansi_dot_progress() {
        // ANSI-colored dot progress line should not break filtering
        let output = "Run options: --seed 12345\n\n# Running:\n\n\x1b[32m.\x1b[0m\x1b[32m.\x1b[0m\x1b[32m.\x1b[0m\n\n3 runs, 6 assertions, 0 failures, 0 errors, 0 skips\n";
        let result = filter_minitest_output(output);
        assert!(result.contains("✓ Rails test:"));
        assert!(result.contains("3 passed"));
    }

    // ── Empty migration table test (Issue 11) ───────────────────────────────

    #[test]
    fn test_filter_rails_migrate_status_empty() {
        let output = "database: myapp_development\n\n Status   Migration ID    Migration Name\n--------------------------------------------------\n";
        let result = filter_rails_migrate_status(output);
        assert!(
            result.contains("No migrations found"),
            "should report no migrations for empty table: {}",
            result
        );
    }
}
