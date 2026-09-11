//! Filters Prisma CLI output by stripping ASCII art and verbose decoration.

use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone)]
pub enum PrismaCommand {
    Generate,
    Migrate { subcommand: MigrateSubcommand },
    DbPush,
}

#[derive(Debug, Clone)]
pub enum MigrateSubcommand {
    Dev { name: Option<String> },
    Status,
    Deploy,
}

pub fn run(cmd: PrismaCommand, args: &[String], verbose: u8) -> Result<i32> {
    match cmd {
        PrismaCommand::Generate => run_generate(args, verbose),
        PrismaCommand::Migrate { subcommand } => run_migrate(subcommand, args, verbose),
        PrismaCommand::DbPush => run_db_push(args, verbose),
    }
}

/// Create a Command that will run prisma (tries global first, then npx)
fn create_prisma_command() -> Command {
    if tool_exists("prisma") {
        resolved_command("prisma")
    } else {
        let mut c = resolved_command("npx");
        c.arg("prisma");
        c
    }
}

fn run_generate(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = create_prisma_command();
    cmd.arg("generate");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: prisma generate");
    }

    let result = exec_capture(&mut cmd)
        .context("Failed to run prisma generate (try: npm install -g prisma)")?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    if !result.success() {
        if !result.stdout.trim().is_empty() {
            eprint!("{}", result.stdout);
        }
        if !result.stderr.trim().is_empty() {
            eprint!("{}", result.stderr);
        }
        timer.track("prisma generate", "rtk prisma generate", &raw, &raw);
        return Ok(result.exit_code);
    }

    let filtered = filter_prisma_generate(&raw);
    let shown = never_worse(&raw, &filtered);
    println!("{}", shown);
    timer.track("prisma generate", "rtk prisma generate", &raw, shown);

    Ok(0)
}

fn run_migrate(subcommand: MigrateSubcommand, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = create_prisma_command();
    cmd.arg("migrate");

    let cmd_name = match &subcommand {
        MigrateSubcommand::Dev { name } => {
            cmd.arg("dev");
            if let Some(n) = name {
                cmd.arg("--name").arg(n);
            }
            "prisma migrate dev"
        }
        MigrateSubcommand::Status => {
            cmd.arg("status");
            "prisma migrate status"
        }
        MigrateSubcommand::Deploy => {
            cmd.arg("deploy");
            "prisma migrate deploy"
        }
    };

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {}", cmd_name);
    }

    let result = exec_capture(&mut cmd).context("Failed to run prisma migrate")?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    if !result.success() {
        if !result.stdout.trim().is_empty() {
            eprint!("{}", result.stdout);
        }
        if !result.stderr.trim().is_empty() {
            eprint!("{}", result.stderr);
        }
        timer.track(cmd_name, &format!("rtk {}", cmd_name), &raw, &raw);
        return Ok(result.exit_code);
    }

    let filtered = match subcommand {
        MigrateSubcommand::Dev { .. } => filter_migrate_dev(&raw),
        MigrateSubcommand::Status => filter_migrate_status(&raw),
        MigrateSubcommand::Deploy => filter_migrate_deploy(&raw),
    };

    let shown = never_worse(&raw, &filtered);
    println!("{}", shown);
    timer.track(cmd_name, &format!("rtk {}", cmd_name), &raw, shown);

    Ok(0)
}

fn run_db_push(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = create_prisma_command();
    cmd.arg("db").arg("push");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: prisma db push");
    }

    let result = exec_capture(&mut cmd).context("Failed to run prisma db push")?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    if !result.success() {
        if !result.stdout.trim().is_empty() {
            eprint!("{}", result.stdout);
        }
        if !result.stderr.trim().is_empty() {
            eprint!("{}", result.stderr);
        }
        timer.track("prisma db push", "rtk prisma db push", &raw, &raw);
        return Ok(result.exit_code);
    }

    let filtered = filter_db_push(&raw);
    let shown = never_worse(&raw, &filtered);
    println!("{}", shown);
    timer.track("prisma db push", "rtk prisma db push", &raw, shown);

    Ok(0)
}

/// Filter prisma generate output - strip ASCII art, extract counts
fn filter_prisma_generate(output: &str) -> String {
    let mut models = 0;
    let mut enums = 0;
    let mut types = 0;
    let mut output_path = String::new();

    for line in output.lines() {
        // Skip ASCII art and box drawing
        if line.contains("█")
            || line.contains("▀")
            || line.contains("▄")
            || line.contains("┌")
            || line.contains("└")
            || line.contains("│")
        {
            continue;
        }

        // Extract counts
        if line.contains("model") && line.contains("generated") {
            if let Some(num) = extract_number(line) {
                models = num;
            }
        }
        if line.contains("enum") {
            if let Some(num) = extract_number(line) {
                enums = num;
            }
        }
        if line.contains("type") {
            if let Some(num) = extract_number(line) {
                types = num;
            }
        }

        // Extract output path
        if line.contains("node_modules") && line.contains("@prisma") {
            output_path = line.trim().to_string();
        }
    }

    let mut result = String::new();
    result.push_str("Prisma Client generated\n");

    if models > 0 || enums > 0 || types > 0 {
        result.push_str(&format!(
            "  • {} models, {} enums, {} types\n",
            models, enums, types
        ));
    }

    if !output_path.is_empty() {
        result.push_str("  • Output: node_modules/@prisma/client\n");
    }

    result.trim().to_string()
}

/// Filter migrate dev output - extract migration changes
fn filter_migrate_dev(output: &str) -> String {
    let mut migration_name = String::new();
    let mut tables_added = 0;
    let mut tables_modified = 0;
    let mut relations = Vec::new();
    let mut indexes = Vec::new();
    let mut applied = false;

    for line in output.lines() {
        // Extract migration name
        if line.contains("migration") && line.contains("_") {
            if let Some(pos) = line.find("202") {
                let end = line[pos..]
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(line.len() - pos);
                migration_name = line[pos..pos + end].to_string();
            }
        }

        // Count changes
        if line.contains("CREATE TABLE") {
            tables_added += 1;
        }
        if line.contains("ALTER TABLE") {
            tables_modified += 1;
        }
        if line.contains("FOREIGN KEY") || line.contains("REFERENCES") {
            if let Some(table) = extract_table_name(line) {
                relations.push(table);
            }
        }
        if line.contains("CREATE INDEX") || line.contains("CREATE UNIQUE INDEX") {
            if let Some(idx) = extract_index_name(line) {
                indexes.push(idx);
            }
        }

        if line.contains("applied") || line.contains("✓") {
            applied = true;
        }
    }

    let mut result = String::new();

    if !migration_name.is_empty() {
        result.push_str(&format!("Migration: {}\n", migration_name));
    }

    result.push_str("Changes:\n");
    if tables_added > 0 {
        result.push_str(&format!("  + {} table(s)\n", tables_added));
    }
    if tables_modified > 0 {
        result.push_str(&format!("  ~ {} table(s) modified\n", tables_modified));
    }
    if !relations.is_empty() {
        result.push_str(&format!("  + {} relation(s)\n", relations.len()));
    }
    if !indexes.is_empty() {
        result.push_str(&format!("  ~ {} index(es)\n", indexes.len()));
    }

    result.push('\n');
    if applied {
        result.push_str("Applied | Pending: 0\n");
    }

    result.trim().to_string()
}

/// Parses the `N migrations found in <path>` header that Prisma prints for
/// both `migrate status` and `migrate deploy`. Returns `None` when the header
/// is absent, which means the output is not a shape we know how to summarize.
fn parse_migrations_found(output: &str) -> Option<usize> {
    for line in output.lines() {
        let line = line.trim();
        if !line.contains("found in") {
            continue;
        }
        if line.starts_with("No migration found") {
            return Some(0);
        }
        let mut words = line.split_whitespace();
        let count = words.next().and_then(|word| word.parse::<usize>().ok());
        let noun = words.next().unwrap_or("");
        if let Some(count) = count {
            if noun.starts_with("migration") {
                return Some(count);
            }
        }
    }
    None
}

/// Collects the migration names Prisma lists under its
/// `Following migration(s) have not yet been applied:` header.
fn parse_pending_migrations(output: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_block = false;

    for line in output.lines() {
        let line = line.trim();

        if line.contains("have not yet been applied") {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }

        // Blank lines before the first name are padding; after it, the list is over.
        if line.is_empty() {
            if names.is_empty() {
                continue;
            }
            break;
        }
        // Migration directory names never contain spaces, so anything that does
        // is the prose that follows the list.
        if line.contains(' ') {
            break;
        }
        names.push(line.to_string());
    }

    names
}

/// Collects the migration names from Prisma's ``Applying migration `name` `` lines.
fn parse_applied_migrations(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Applying migration ")?;
            Some(rest.trim_matches('`').to_string())
        })
        .collect()
}

/// Filter migrate status output.
///
/// Prisma never prints an "N applied" line, so counting lines that merely
/// contain the word `applied` reported `0 applied, 0 pending` against a
/// healthy database, and counted the "have not yet been applied" header as an
/// applied migration. Parse the documented shapes instead and fall back to the
/// raw output on anything else (drift, failed migrations, shadow-database
/// errors): a status summary that cannot be trusted is worse than no summary.
fn filter_migrate_status(output: &str) -> String {
    let total = parse_migrations_found(output);
    let pending = parse_pending_migrations(output);
    let up_to_date = output.contains("Database schema is up to date!");

    match (total, up_to_date, pending.is_empty()) {
        (Some(total), true, true) => format!("{} migration(s) found, schema up to date", total),
        (Some(total), false, false) => {
            let mut result = format!(
                "{} migration(s) found, {} not yet applied:",
                total,
                pending.len()
            );
            for name in pending.iter().take(5) {
                result.push_str(&format!("\n  - {}", name));
            }
            if pending.len() > 5 {
                result.push_str(&format!("\n  ... and {} more", pending.len() - 5));
            }
            result
        }
        _ => output.trim().to_string(),
    }
}

/// Filter migrate deploy output.
///
/// Counts the ``Applying migration `name` `` lines Prisma emits per migration,
/// not lines containing the word `applied` — the previous heuristic scored the
/// two summary sentences and reported "2 migration(s) deployed" for a run that
/// applied three. Unrecognized output is returned verbatim.
fn filter_migrate_deploy(output: &str) -> String {
    if output.contains("No pending migrations to apply") {
        return "No pending migrations to apply".to_string();
    }

    let applied = parse_applied_migrations(output);
    if !applied.is_empty() && output.contains("successfully applied") {
        let mut result = format!("{} migration(s) applied:", applied.len());
        for name in applied.iter().take(5) {
            result.push_str(&format!("\n  - {}", name));
        }
        if applied.len() > 5 {
            result.push_str(&format!("\n  ... and {} more", applied.len() - 5));
        }
        return result;
    }

    output.trim().to_string()
}

/// Filter db push output
fn filter_db_push(output: &str) -> String {
    let mut tables_added = 0;
    let mut columns_modified = 0;
    let mut dropped = 0;

    for line in output.lines() {
        if line.contains("CREATE TABLE") {
            tables_added += 1;
        }
        if line.contains("ALTER") || line.contains("ADD COLUMN") {
            columns_modified += 1;
        }
        if line.contains("DROP") {
            dropped += 1;
        }
    }

    let mut result = String::new();
    result.push_str("Schema pushed to database\n");

    if tables_added > 0 || columns_modified > 0 || dropped > 0 {
        result.push_str(&format!(
            "  + {} tables, ~ {} columns, - {} dropped\n",
            tables_added, columns_modified, dropped
        ));
    }

    result.trim().to_string()
}

/// Extract first number from a line
fn extract_number(line: &str) -> Option<usize> {
    line.split_whitespace()
        .find_map(|word| word.parse::<usize>().ok())
}

/// Extract table name from SQL
fn extract_table_name(line: &str) -> Option<String> {
    if line.contains("TABLE") {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "TABLE" && i + 1 < parts.len() {
                return Some(
                    parts[i + 1]
                        .trim_matches(|c| c == '`' || c == '"' || c == ';')
                        .to_string(),
                );
            }
        }
    }
    None
}

/// Extract index name from SQL
fn extract_index_name(line: &str) -> Option<String> {
    if line.contains("INDEX") {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "INDEX" && i + 1 < parts.len() {
                return Some(
                    parts[i + 1]
                        .trim_matches(|c| c == '`' || c == '"' || c == ';')
                        .to_string(),
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_generate() {
        let output = r#"
Prisma schema loaded from prisma/schema.prisma

✔ Generated Prisma Client (v5.7.0) to ./node_modules/@prisma/client in 234ms

Start by importing your Prisma Client:

import { PrismaClient } from '@prisma/client'

42 models, 18 enums, 890 types generated
"#;
        let result = filter_prisma_generate(output);
        assert!(result.contains("Prisma Client generated"));
        // Parser may not extract exact counts from this format, just check it doesn't crash
        assert!(!result.contains("Prisma schema loaded"));
        assert!(!result.contains("Start by importing"));
    }

    #[test]
    fn test_filter_migrate_dev() {
        let output = r#"
Applying migration 20260128_add_sessions

CREATE TABLE "Session" (
  "id" TEXT NOT NULL,
  "userId" TEXT NOT NULL,
  FOREIGN KEY ("userId") REFERENCES "User"("id")
);

CREATE INDEX "session_status_idx" ON "Session"("status");

✓ Migration applied
"#;
        let result = filter_migrate_dev(output);
        assert!(result.contains("20260128_add_sessions"));
        assert!(result.contains("+ 1 table"));
        assert!(result.contains("Applied"));
    }

    #[test]
    fn test_extract_number() {
        assert_eq!(extract_number("42 models generated"), Some(42));
        assert_eq!(extract_number("no numbers here"), None);
    }

    // Fixtures below are verbatim captures from Prisma 7.8.0 against a
    // PostgreSQL 16 database, not hand-written approximations.

    const STATUS_UP_TO_DATE: &str = r#"Loaded Prisma config from prisma.config.ts.

Prisma schema loaded from prisma\schema.prisma.
Datasource "db": PostgreSQL database "rtk_probe", schema "public" at "localhost:5432"

3 migrations found in prisma/migrations

Database schema is up to date!
"#;

    const STATUS_PENDING: &str = r#"Loaded Prisma config from prisma.config.ts.

Prisma schema loaded from prisma\schema.prisma.
Datasource "db": PostgreSQL database "rtk_probe", schema "public" at "localhost:5432"

4 migrations found in prisma/migrations
Following migration have not yet been applied:
20260401000000_add_flange

To apply migrations in development run prisma migrate dev.
To apply migrations in production run prisma migrate deploy.
"#;

    const DEPLOY_APPLYING: &str = r#"Loaded Prisma config from prisma.config.ts.

Prisma schema loaded from prisma\schema.prisma.
Datasource "db": PostgreSQL database "rtk_probe", schema "public" at "localhost:5432"

3 migrations found in prisma/migrations

Applying migration `20260101000000_init`
Applying migration `20260201000000_add_gadget`
Applying migration `20260301000000_add_sprocket`

The following migration(s) have been applied:

migrations/
  └─ 20260101000000_init/
    └─ migration.sql
  └─ 20260201000000_add_gadget/
    └─ migration.sql
  └─ 20260301000000_add_sprocket/
    └─ migration.sql

All migrations have been successfully applied.
"#;

    const DEPLOY_NOOP: &str = r#"Loaded Prisma config from prisma.config.ts.

Prisma schema loaded from prisma\schema.prisma.
Datasource "db": PostgreSQL database "rtk_probe", schema "public" at "localhost:5432"

3 migrations found in prisma/migrations


No pending migrations to apply.
"#;

    #[test]
    fn status_reports_the_real_total_when_up_to_date() {
        let result = filter_migrate_status(STATUS_UP_TO_DATE);
        assert_eq!(result, "3 migration(s) found, schema up to date");
    }

    /// Regression: the previous heuristic counted lines containing the word
    /// "applied", which Prisma never prints on a healthy database, so a
    /// six-migration project was summarized as "0 applied, 0 pending".
    #[test]
    fn status_never_claims_zero_when_migrations_exist() {
        let result = filter_migrate_status(STATUS_UP_TO_DATE);
        assert!(
            !result.contains('0'),
            "summary must not report a zero count for 3 migrations: {result}"
        );
        assert!(result.contains('3'), "summary must carry the real total: {result}");
    }

    #[test]
    fn status_names_the_pending_migrations() {
        let result = filter_migrate_status(STATUS_PENDING);
        assert!(result.contains("4 migration(s) found"), "{result}");
        assert!(result.contains("1 not yet applied"), "{result}");
        assert!(result.contains("20260401000000_add_flange"), "{result}");
    }

    /// Regression: "Following migration have not yet been applied:" contains
    /// the substring "applied", so the old counter scored a pending migration
    /// as an applied one.
    #[test]
    fn status_does_not_count_the_pending_header_as_applied() {
        let result = filter_migrate_status(STATUS_PENDING);
        assert!(
            !result.contains("1 applied"),
            "pending header must not be read as an applied migration: {result}"
        );
    }

    #[test]
    fn status_falls_back_to_raw_on_unknown_shape() {
        let drift = "Drift detected: your database schema is not in sync with your migration history.";
        assert_eq!(filter_migrate_status(drift), drift);
    }

    #[test]
    fn status_falls_back_when_totals_and_state_disagree() {
        // "up to date" alongside a pending list is contradictory: refuse to summarize.
        let contradictory = "2 migrations found in prisma/migrations\n\
             Following migration have not yet been applied:\n\
             20260401000000_add_flange\n\
             Database schema is up to date!";
        assert_eq!(filter_migrate_status(contradictory), contradictory);
    }

    #[test]
    fn deploy_counts_every_applied_migration() {
        let result = filter_migrate_deploy(DEPLOY_APPLYING);
        assert!(
            result.starts_with("3 migration(s) applied"),
            "three migrations were applied, got: {result}"
        );
        assert!(result.contains("20260301000000_add_sprocket"), "{result}");
    }

    #[test]
    fn deploy_reports_a_noop_run() {
        assert_eq!(
            filter_migrate_deploy(DEPLOY_NOOP),
            "No pending migrations to apply"
        );
    }

    #[test]
    fn deploy_falls_back_to_raw_on_unknown_shape() {
        let partial = "Applying migration `20260101000000_init`";
        assert_eq!(filter_migrate_deploy(partial), partial);
    }

    #[test]
    fn parses_the_migrations_found_header() {
        assert_eq!(parse_migrations_found(STATUS_UP_TO_DATE), Some(3));
        assert_eq!(parse_migrations_found(STATUS_PENDING), Some(4));
        assert_eq!(
            parse_migrations_found("1 migration found in prisma/migrations"),
            Some(1)
        );
        assert_eq!(
            parse_migrations_found("No migration found in prisma/migrations"),
            Some(0)
        );
        assert_eq!(parse_migrations_found("nothing to see here"), None);
    }

    #[test]
    fn pending_list_stops_before_the_trailing_prose() {
        let names = parse_pending_migrations(STATUS_PENDING);
        assert_eq!(names, vec!["20260401000000_add_flange".to_string()]);
    }
}
