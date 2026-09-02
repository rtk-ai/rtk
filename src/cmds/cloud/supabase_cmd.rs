//! Supabase CLI database output compression.
//!
//! Only explicitly safe, non-interactive database operations are filtered.
//! Everything else is passed through byte-for-byte with inherited stdin.

use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::tee::force_tee_hint;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::resolved_command;
use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterKind {
    DbLint,
    DbPush,
    DbDiffFile,
    DbResetLocal,
    MigrationList,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let display = sanitize_args(args);
    let mut cmd = resolved_command("supabase");
    cmd.args(args);

    if let Some(kind) = filter_kind(args) {
        if verbose > 0 {
            eprintln!("Running: supabase {display}");
        }
        return runner::run_filtered(
            cmd,
            "supabase",
            &display,
            move |raw| filter_output(kind, raw),
            RunOptions::with_tee("supabase").early_exit_on_failure(),
        );
    }

    if verbose > 0 {
        eprintln!("supabase passthrough: {display}");
    }
    runner::run(
        cmd,
        "supabase",
        &display,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

fn filter_kind(args: &[String]) -> Option<FilterKind> {
    match args {
        [db, lint, ..] if db == "db" && lint == "lint" => Some(FilterKind::DbLint),
        [db, push, rest @ ..] if db == "db" && push == "push" => {
            has_any_flag(rest, &["--yes", "--dry-run"]).then_some(FilterKind::DbPush)
        }
        [db, diff, rest @ ..] if db == "db" && diff == "diff" => {
            has_value_flag(rest, &["-f", "--file", "-o", "--output"])
                .then_some(FilterKind::DbDiffFile)
        }
        [db, reset, rest @ ..] if db == "db" && reset == "reset" => {
            (has_any_flag(rest, &["--local"]) && !has_any_flag(rest, &["--linked"]))
                .then_some(FilterKind::DbResetLocal)
        }
        [migration, list, ..] if migration == "migration" && list == "list" => {
            Some(FilterKind::MigrationList)
        }
        _ => None,
    }
}

fn has_any_flag(args: &[String], names: &[&str]) -> bool {
    args.iter()
        .any(|arg| names.iter().any(|name| arg == name))
}

fn has_value_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().enumerate().any(|(index, arg)| {
        names.iter().any(|name| {
            arg == name && args.get(index + 1).is_some_and(|value| !value.starts_with('-'))
                || arg
                    .strip_prefix(name)
                    .is_some_and(|suffix| suffix.starts_with('=') && suffix.len() > 1)
        })
    })
}

fn sanitize_args(args: &[String]) -> String {
    const SENSITIVE: &[&str] = &[
        "--access-token",
        "--auth-token",
        "--db-url",
        "--database-url",
        "--password",
        "--token",
        "--url",
    ];

    let mut result = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            result.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }

        if SENSITIVE.iter().any(|flag| arg == flag) {
            result.push(arg.clone());
            redact_next = true;
            continue;
        }

        if let Some(flag) = SENSITIVE
            .iter()
            .find(|flag| arg.starts_with(&format!("{flag}=")))
        {
            result.push(format!("{flag}=[REDACTED]"));
        } else {
            result.push(arg.clone());
        }
    }
    result.join(" ")
}

fn filter_output(kind: FilterKind, raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }

    match kind {
        FilterKind::MigrationList => filter_migration_list(raw),
        _ => filter_status_output(kind, raw),
    }
}

fn filter_status_output(kind: FilterKind, raw: &str) -> String {
    let recognized = raw.lines().any(|line| is_recognized(kind, line));
    if !recognized {
        return raw.to_string();
    }

    let mut kept = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_progress_noise(trimmed) {
            continue;
        }
        if is_actionable(trimmed) || is_kind_result(kind, trimmed) {
            kept.push(compact_status_line(kind, trimmed));
        }
    }

    if kept.is_empty() {
        return raw.to_string();
    }
    cap_lines(raw, kept, "supabase")
}

fn compact_status_line(kind: FilterKind, line: &str) -> String {
    if matches!(kind, FilterKind::DbPush | FilterKind::DbResetLocal) {
        if let Some(migration) = line.strip_prefix("Applying migration ") {
            return format!("apply {migration}");
        }
    }
    line.to_string()
}

fn filter_migration_list(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let header_index = lines.iter().position(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("local") && lower.contains("remote")
    });
    let Some(header_index) = header_index else {
        return raw.to_string();
    };

    let mut kept = Vec::new();
    for line in &lines[header_index..] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.chars().all(|c| "-| ".contains(c)) {
            continue;
        }
        if trimmed.contains('|') || is_actionable(trimmed) {
            kept.push(trimmed.to_string());
        }
    }
    if kept.is_empty() {
        raw.to_string()
    } else {
        cap_lines(raw, kept, "supabase-migration-list")
    }
}

fn is_recognized(kind: FilterKind, line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    match kind {
        FilterKind::DbLint => {
            lower.contains("linting schema")
                || lower.contains("schema errors")
                || lower.contains("no schema errors")
        }
        FilterKind::DbPush => {
            lower.contains("push these migrations")
                || lower.contains("applying migration")
                || lower.contains("finished supabase db push")
                || lower.contains("would push")
        }
        FilterKind::DbDiffFile => {
            lower.contains("diff written")
                || lower.contains("schema diff")
                || lower.contains("creating shadow database")
        }
        FilterKind::DbResetLocal => {
            lower.contains("resetting local database")
                || lower.contains("finished supabase db reset")
                || lower.contains("applying migration")
        }
        FilterKind::MigrationList => false,
    }
}

fn is_progress_noise(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "connecting to ",
        "initialising ",
        "initializing ",
        "creating shadow database",
        "setting up ",
        "recreating database",
        "restarting containers",
        "seeding data",
        "loading seed",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn is_actionable(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "error",
        "warning",
        "warn:",
        "failed",
        "failure",
        "fatal",
        "hint:",
        "notice:",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn is_kind_result(kind: FilterKind, line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    match kind {
        FilterKind::DbLint => {
            lower.contains("schema errors")
                || lower.contains("no schema errors")
                || lower.contains("function ")
                || lower.contains("relation ")
                || lower.contains("security")
        }
        FilterKind::DbPush => {
            lower.contains(".sql")
                || lower.contains("would push")
                || lower.contains("finished supabase db push")
                || lower.contains("up to date")
        }
        FilterKind::DbDiffFile => {
            lower.contains("diff written")
                || lower.contains(".sql")
                || lower.contains("no schema changes")
        }
        FilterKind::DbResetLocal => {
            lower.contains(".sql")
                || lower.contains("finished supabase db reset")
                || lower.contains("local database")
        }
        FilterKind::MigrationList => false,
    }
}

fn cap_lines(raw: &str, lines: Vec<String>, slug: &str) -> String {
    if lines.len() <= CAP_LIST {
        return lines.join("\n");
    }

    let omitted = lines.len() - CAP_LIST;
    let mut result = lines[..CAP_LIST].join("\n");
    result.push_str(&format!("\n... +{omitted} more lines"));
    if let Some(hint) = force_tee_hint(raw, slug) {
        result.push('\n');
        result.push_str(&hint);
    } else {
        result.push_str("\n[run the original command without rtk for full output]");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn selects_only_safe_supabase_commands() {
        assert_eq!(
            filter_kind(&args(&["db", "lint"])),
            Some(FilterKind::DbLint)
        );
        assert_eq!(
            filter_kind(&args(&["db", "push", "--yes"])),
            Some(FilterKind::DbPush)
        );
        assert_eq!(
            filter_kind(&args(&["db", "push", "--dry-run"])),
            Some(FilterKind::DbPush)
        );
        assert_eq!(
            filter_kind(&args(&["db", "diff", "--file", "schema.sql"])),
            Some(FilterKind::DbDiffFile)
        );
        assert_eq!(
            filter_kind(&args(&["db", "diff", "--output=schema.sql"])),
            Some(FilterKind::DbDiffFile)
        );
        assert_eq!(
            filter_kind(&args(&["db", "reset", "--local"])),
            Some(FilterKind::DbResetLocal)
        );
        assert_eq!(
            filter_kind(&args(&["migration", "list"])),
            Some(FilterKind::MigrationList)
        );
    }

    #[test]
    fn interactive_and_remote_commands_are_passthrough() {
        assert_eq!(filter_kind(&args(&["db", "push"])), None);
        assert_eq!(filter_kind(&args(&["db", "diff"])), None);
        assert_eq!(filter_kind(&args(&["db", "reset"])), None);
        assert_eq!(
            filter_kind(&args(&["db", "reset", "--local", "--linked"])),
            None
        );
        assert_eq!(filter_kind(&args(&["functions", "deploy", "api"])), None);
    }

    #[test]
    fn redacts_sensitive_arguments_from_tracking_label() {
        let sanitized = sanitize_args(&args(&[
            "db",
            "push",
            "--db-url",
            "postgres://secret",
            "--access-token=token-value",
            "--password",
            "password-value",
        ]));
        assert_eq!(
            sanitized,
            "db push --db-url [REDACTED] --access-token=[REDACTED] --password [REDACTED]"
        );
        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("token-value"));
        assert!(!sanitized.contains("password-value"));
    }

    #[test]
    fn unknown_format_is_preserved() {
        let raw = "Supabase changed this output format completely\nopaque data\n";
        assert_eq!(filter_output(FilterKind::DbLint, raw), raw);
    }

    #[test]
    fn migration_list_keeps_versions_and_drops_connection_noise() {
        let raw = "\
Connecting to remote database...

   Local          | Remote         | Time (UTC)
  ----------------|----------------|---------------------
   20250101000000 | 20250101000000 | 2025-01-01 00:00:00
   20250102000000 |                | 2025-01-02 00:00:00
";
        let filtered = filter_output(FilterKind::MigrationList, raw);
        assert!(!filtered.contains("Connecting"));
        assert!(filtered.contains("Local"));
        assert!(filtered.contains("20250101000000"));
        assert!(filtered.contains("20250102000000"));
    }

    #[test]
    fn push_preserves_results_and_actionable_messages() {
        let raw = "\
Connecting to remote database...
Setting up migration table...
Applying migration 20250101000000_first.sql...
Applying migration 20250102000000_second.sql...
WARNING: policy already exists
Finished supabase db push.
";
        let filtered = filter_output(FilterKind::DbPush, raw);
        assert!(!filtered.contains("Connecting"));
        assert!(!filtered.contains("Setting up"));
        assert!(filtered.contains("20250101000000_first.sql"));
        assert!(filtered.contains("WARNING"));
        assert!(filtered.contains("Finished supabase db push"));
    }

    #[test]
    fn representative_output_saves_at_least_sixty_percent() {
        let raw = "\
Connecting to remote database...
Setting up migration table...
Initialising login role...
Creating shadow database...
Recreating database...
Restarting containers...
Applying migration 20250101000000_first.sql...
Applying migration 20250102000000_second.sql...
Finished supabase db push.
";
        let filtered = filter_output(FilterKind::DbPush, raw);
        let savings = 100.0 - filtered.len() as f64 / raw.len() as f64 * 100.0;
        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }
}
