//! Laravel Pint (PHP-CS-Fixer wrapper) output filter.
//!
//! Pint emits verbose per-rule progress and config chatter on its default
//! text output. It also supports `--format=json`, which gives a structured
//! list of files with their applied rules. We inject `--format=json` when
//! the user hasn't picked a format, parse it, and emit a compact summary
//! grouped by file and sorted by rule count.

use super::utils::php_tool_command;
use crate::core::runner;
use crate::core::utils::fallback_tail;
use anyhow::Result;
use serde::Deserialize;

const MAX_FILES_SHOWN: usize = 15;
const MAX_RULES_PER_FILE: usize = 5;

#[derive(Deserialize)]
struct PintOutput {
    #[serde(default)]
    files: Vec<PintFile>,
}

#[derive(Deserialize)]
struct PintFile {
    // Pint ≥ ~1.14 renamed the JSON keys: name→path, appliedFixers→fixers.
    // Aliases keep both schemas parsing so output stays compressed across versions.
    #[serde(alias = "path")]
    name: String,
    // PHP-CS-Fixer omits the fixers key entirely in dry-run/diff modes, so it
    // must default rather than fail the whole parse.
    #[serde(rename = "appliedFixers", alias = "fixers", default)]
    applied_fixers: Vec<String>,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command("pint");

    let has_format = args
        .iter()
        .any(|a| a == "--format" || a.starts_with("--format="));
    let is_utility_cmd = args
        .first()
        .map(|a| matches!(a.as_str(), "--version" | "-V" | "--help" | "-h"))
        .unwrap_or(false);

    if !has_format && !is_utility_cmd {
        cmd.arg("--format=json");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: pint {}", args.join(" "));
    }

    let filter = move |stdout: &str| -> String {
        if has_format || is_utility_cmd {
            fallback_tail(stdout, "pint", 60)
        } else {
            filter_pint_json(stdout)
        }
    };

    runner::run_filtered(
        cmd,
        "pint",
        &args.join(" "),
        filter,
        runner::RunOptions::stdout_only().tee("pint"),
    )
}

pub(crate) fn filter_pint_json(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "pint: ok".to_string();
    }

    let parsed: Result<PintOutput, _> = serde_json::from_str(trimmed);
    let pint = match parsed {
        Ok(p) => p,
        Err(_) => return fallback_tail(output, "pint (JSON parse error)", 20),
    };

    if pint.files.is_empty() {
        return "pint: ok".to_string();
    }

    let total_files = pint.files.len();
    let total_rules: usize = pint.files.iter().map(|f| f.applied_fixers.len()).sum();

    let mut files = pint.files;
    files.sort_by(|a, b| {
        b.applied_fixers
            .len()
            .cmp(&a.applied_fixers.len())
            .then(a.name.cmp(&b.name))
    });

    let mut result = format!("pint: {} changes in {} files\n", total_rules, total_files);

    // Resolve cwd once; short_path() used to re-syscall current_dir() per file.
    let cwd_prefix = std::env::current_dir()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .map(|s| format!("{}/", s));

    for file in files.iter().take(MAX_FILES_SHOWN) {
        let name = cwd_prefix
            .as_deref()
            .and_then(|c| file.name.strip_prefix(c))
            .unwrap_or(&file.name);
        result.push_str(&format!("\n{} ({})\n", name, file.applied_fixers.len()));
        for rule in file.applied_fixers.iter().take(MAX_RULES_PER_FILE) {
            result.push_str(&format!("  - {}\n", rule));
        }
        if file.applied_fixers.len() > MAX_RULES_PER_FILE {
            result.push_str(&format!(
                "  ... +{} more rules\n",
                file.applied_fixers.len() - MAX_RULES_PER_FILE
            ));
        }
    }

    if files.len() > MAX_FILES_SHOWN {
        result.push_str(&format!(
            "\n... +{} more files\n",
            files.len() - MAX_FILES_SHOWN
        ));
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pint_empty_is_ok() {
        assert_eq!(filter_pint_json(""), "pint: ok");
    }

    #[test]
    fn test_pint_no_files_is_ok() {
        assert_eq!(filter_pint_json(r#"{"files":[]}"#), "pint: ok");
    }

    #[test]
    fn test_pint_single_file() {
        let json = r#"{"files":[{"name":"app/Foo.php","appliedFixers":["no_unused_imports","ordered_imports"]}]}"#;
        let result = filter_pint_json(json);
        assert!(result.contains("2 changes in 1 files"), "got: {}", result);
        assert!(result.contains("app/Foo.php (2)"), "got: {}", result);
        assert!(result.contains("no_unused_imports"), "got: {}", result);
        assert!(result.contains("ordered_imports"), "got: {}", result);
    }

    #[test]
    fn test_pint_current_schema_path_fixers() {
        // Pint ≥ ~1.14 emits path/fixers instead of name/appliedFixers.
        // Without aliases this fell back to raw output (no compression).
        let json = r#"{"result":"fail","files":[{"path":"app/Foo.php","fixers":["concat_space","ordered_imports"]}]}"#;
        let result = filter_pint_json(json);
        assert!(result.contains("2 changes in 1 files"), "got: {}", result);
        assert!(result.contains("app/Foo.php (2)"), "got: {}", result);
        assert!(result.contains("concat_space"), "got: {}", result);
    }

    #[test]
    fn test_pint_sorted_by_count_desc() {
        let json = r#"{"files":[
            {"name":"a.php","appliedFixers":["x"]},
            {"name":"b.php","appliedFixers":["x","y","z"]},
            {"name":"c.php","appliedFixers":["x","y"]}
        ]}"#;
        let result = filter_pint_json(json);
        let pos_b = result.find("b.php").unwrap();
        let pos_c = result.find("c.php").unwrap();
        let pos_a = result.find("a.php").unwrap();
        assert!(pos_b < pos_c && pos_c < pos_a, "got: {}", result);
    }

    #[test]
    fn test_pint_file_truncation() {
        let mut files = Vec::new();
        for i in 1..=20 {
            files.push(format!(
                r#"{{"name":"f{}.php","appliedFixers":["x"]}}"#,
                i
            ));
        }
        let json = format!(r#"{{"files":[{}]}}"#, files.join(","));
        let result = filter_pint_json(&json);
        assert!(result.contains("20 changes in 20 files"), "got: {}", result);
        assert!(result.contains("+5 more files"), "got: {}", result);
    }

    #[test]
    fn test_pint_rule_truncation() {
        let json = r#"{"files":[{"name":"f.php","appliedFixers":["a","b","c","d","e","f","g"]}]}"#;
        let result = filter_pint_json(json);
        assert!(result.contains("  - a\n"), "got: {}", result);
        assert!(result.contains("  - e\n"), "got: {}", result);
        assert!(!result.contains("  - f\n"), "got: {}", result);
        assert!(result.contains("+2 more rules"), "got: {}", result);
    }

    #[test]
    fn test_pint_file_without_fixers_key() {
        // PHP-CS-Fixer omits applied_fixers when there's nothing to report;
        // the entry must still parse rather than fall back to raw output.
        let json = r#"{"files":[{"name":"x.php"}]}"#;
        let result = filter_pint_json(json);
        assert!(result.contains("0 changes in 1 files"), "got: {}", result);
        assert!(result.contains("x.php (0)"), "got: {}", result);
    }

    #[test]
    fn test_pint_invalid_json_falls_back() {
        let result = filter_pint_json("Laravel Pint v1.13.6\n\n... some text ...");
        assert!(!result.contains("pint: ok"), "got: {}", result);
    }
}
