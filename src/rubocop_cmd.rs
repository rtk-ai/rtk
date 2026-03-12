use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct RubocopOutput {
    files: Vec<RubocopFile>,
    summary: RubocopSummary,
}

#[derive(Debug, Deserialize)]
struct RubocopFile {
    path: String,
    offenses: Vec<RubocopOffense>,
}

#[derive(Debug, Deserialize)]
struct RubocopOffense {
    severity: String,
    message: String,
    cop_name: String,
    correctable: bool,
    location: RubocopLocation,
}

#[derive(Debug, Deserialize)]
struct RubocopLocation {
    start_line: u32,
    start_column: u32,
}

#[derive(Debug, Deserialize)]
struct RubocopSummary {
    offense_count: u32,
    #[allow(dead_code)]
    target_file_count: u32,
    inspected_file_count: u32,
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (program, base_args) = detect_rubocop_command();

    let mut cmd = Command::new(&program);
    for a in &base_args {
        cmd.arg(a);
    }

    // Inject --format json unless user specified a format
    let user_specified_format = args
        .iter()
        .any(|a| a.starts_with("--format") || a == "-f" || a.starts_with("-f"));

    if !user_specified_format {
        cmd.args(["--format", "json"]);
    }

    for a in args {
        cmd.arg(a);
    }

    if verbose > 0 {
        eprintln!(
            "Running: {} {} --format json {}",
            program,
            base_args.join(" "),
            args.join(" ")
        );
    }

    let output = cmd
        .output()
        .context("Failed to run rubocop. Is it installed? Try: gem install rubocop")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = if !user_specified_format && !stdout.trim().is_empty() {
        filter_rubocop_json(&stdout)
    } else {
        // Fallback: text output (user specified format, or empty stdout)
        filter_rubocop_text(&raw)
    };

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "rubocop", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("rubocop {}", args.join(" ")),
        &format!("rtk rubocop {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn detect_rubocop_command() -> (String, Vec<String>) {
    // Check for bin/rubocop
    if std::path::Path::new("bin/rubocop").exists() {
        return ("bin/rubocop".to_string(), vec![]);
    }

    // Check if bundle is available
    if Command::new("which")
        .arg("bundle")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return (
            "bundle".to_string(),
            vec!["exec".to_string(), "rubocop".to_string()],
        );
    }

    ("rubocop".to_string(), vec![])
}

/// Filter RuboCop JSON output — group by cop and file
pub fn filter_rubocop_json(output: &str) -> String {
    let parsed: Result<RubocopOutput, _> = serde_json::from_str(output);

    let data = match parsed {
        Ok(d) => d,
        Err(e) => {
            // Fallback to text parser
            return format!(
                "RuboCop (JSON parse failed: {})\n{}",
                e,
                filter_rubocop_text(output)
            );
        }
    };

    let total_offenses = data.summary.offense_count;
    let inspected = data.summary.inspected_file_count;

    if total_offenses == 0 {
        return format!(
            "✓ RuboCop: {} file{} inspected, no offenses detected",
            inspected,
            if inspected == 1 { "" } else { "s" }
        );
    }

    // Count correctable offenses
    let correctable_count: u32 = data
        .files
        .iter()
        .flat_map(|f| &f.offenses)
        .filter(|o| o.correctable)
        .count() as u32;

    // Group by cop name
    let mut by_cop: HashMap<&str, u32> = HashMap::new();
    for file in &data.files {
        for offense in &file.offenses {
            *by_cop.entry(&offense.cop_name).or_insert(0) += 1;
        }
    }

    // Sort files by offense count
    let mut files_with_offenses: Vec<&RubocopFile> =
        data.files.iter().filter(|f| !f.offenses.is_empty()).collect();
    files_with_offenses.sort_by(|a, b| b.offenses.len().cmp(&a.offenses.len()));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "RuboCop: {} file{} inspected, {} offense{} detected",
        inspected,
        if inspected == 1 { "" } else { "s" },
        total_offenses,
        if total_offenses == 1 { "" } else { "s" }
    ));

    if correctable_count > 0 {
        result.push_str(&format!(" ({} correctable)", correctable_count));
    }
    result.push('\n');
    result.push_str("════════════════════════════════════════\n");

    // Show top cops
    let mut cop_counts: Vec<_> = by_cop.iter().collect();
    cop_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !cop_counts.is_empty() {
        result.push_str("\nTop cops:\n");
        for (cop, count) in cop_counts.iter().take(10) {
            result.push_str(&format!(
                "  {:<45} {} offense{}\n",
                cop,
                count,
                if **count == 1 { "" } else { "s" }
            ));
        }
    }

    // Show top files with offense details
    result.push_str("\nTop files:\n");
    for file in files_with_offenses.iter().take(10) {
        let count = file.offenses.len();
        result.push_str(&format!(
            "  {} ({} offense{})\n",
            file.path,
            count,
            if count == 1 { "" } else { "s" }
        ));

        // Show up to 3 offenses per file
        for offense in file.offenses.iter().take(3) {
            let severity_char = severity_char(&offense.severity);
            result.push_str(&format!(
                "    L{:<4}:{:<3} {}: {}: {}\n",
                offense.location.start_line,
                offense.location.start_column,
                severity_char,
                offense.cop_name,
                truncate(&offense.message, 80)
            ));
        }

        if count > 3 {
            result.push_str(&format!("    ... +{} more\n", count - 3));
        }
    }

    if files_with_offenses.len() > 10 {
        result.push_str(&format!(
            "\n... +{} more files\n",
            files_with_offenses.len() - 10
        ));
    }

    if correctable_count > 0 {
        result.push_str(&format!(
            "\nHint: rubocop -A to auto-correct {} correctable offense{}\n",
            correctable_count,
            if correctable_count == 1 { "" } else { "s" }
        ));
    }

    result.trim().to_string()
}

fn severity_char(severity: &str) -> &str {
    match severity {
        "convention" => "C",
        "warning" => "W",
        "error" => "E",
        "fatal" => "F",
        "refactor" => "R",
        _ => "?",
    }
}

/// Fallback text parser for RuboCop output (when JSON is unavailable)
pub fn filter_rubocop_text(output: &str) -> String {
    // Pattern: "path/to/file.rb:line:col: S: CopName: message"
    let offense_re = regex::Regex::new(
        r"^(\S+\.\w+):(\d+):(\d+):\s+([CWEFR]):\s+([^:]+):\s+(.+)$"
    );

    let offense_re = match offense_re {
        Ok(r) => r,
        Err(_) => return output.trim().to_string(),
    };

    let mut offenses: Vec<(String, u32, u32, String, String, String)> = Vec::new();

    for line in output.lines() {
        if let Some(caps) = offense_re.captures(line) {
            let path = caps.get(1).map_or("", |m| m.as_str()).to_string();
            let line_num: u32 = caps.get(2).map_or("0", |m| m.as_str()).parse().unwrap_or(0);
            let col_num: u32 = caps.get(3).map_or("0", |m| m.as_str()).parse().unwrap_or(0);
            let severity = caps.get(4).map_or("?", |m| m.as_str()).to_string();
            let cop = caps.get(5).map_or("", |m| m.as_str()).to_string();
            let message = caps.get(6).map_or("", |m| m.as_str()).to_string();
            offenses.push((path, line_num, col_num, severity, cop, message));
        }
    }

    // Extract "N files inspected, M offenses" from summary
    let mut inspected = 0u32;
    let mut total = 0u32;
    for line in output.lines() {
        let l = line.trim();
        // "8 files inspected, 5 offenses detected"
        if l.contains("inspected") && l.contains("offense") {
            let parts: Vec<&str> = l.split(',').collect();
            for part in &parts {
                let words: Vec<&str> = part.split_whitespace().collect();
                for (i, word) in words.iter().enumerate() {
                    if *word == "inspected" && i > 0 {
                        inspected = words[i - 1].parse().unwrap_or(0);
                    }
                    if word.starts_with("offense") && i > 0 {
                        total = words[i - 1].parse().unwrap_or(0);
                    }
                }
            }
        }
    }

    if total == 0 && offenses.is_empty() {
        return format!(
            "✓ RuboCop: {} file{} inspected, no offenses detected",
            inspected,
            if inspected == 1 { "" } else { "s" }
        );
    }

    // Group by file
    let mut by_file: HashMap<&str, Vec<_>> = HashMap::new();
    for offense in &offenses {
        by_file.entry(&offense.0).or_default().push(offense);
    }

    let mut files_sorted: Vec<_> = by_file.iter().collect();
    files_sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    let mut result = String::new();
    result.push_str(&format!(
        "RuboCop: {} file{} inspected, {} offense{} detected\n",
        inspected,
        if inspected == 1 { "" } else { "s" },
        total,
        if total == 1 { "" } else { "s" }
    ));
    result.push_str("════════════════════════════════════════\n");

    for (path, file_offenses) in files_sorted.iter().take(10) {
        result.push_str(&format!(
            "  {} ({} offense{})\n",
            path,
            file_offenses.len(),
            if file_offenses.len() == 1 { "" } else { "s" }
        ));
        for offense in file_offenses.iter().take(3) {
            result.push_str(&format!(
                "    L{:<4}:{:<3} {}: {}: {}\n",
                offense.1,
                offense.2,
                offense.3,
                offense.4,
                truncate(&offense.5, 80)
            ));
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_json_no_offenses() {
        let json = r#"{"files":[],"summary":{"offense_count":0,"target_file_count":8,"inspected_file_count":8}}"#;
        let result = filter_rubocop_json(json);
        assert!(result.contains("✓ RuboCop"), "got: {}", result);
        assert!(result.contains("no offenses"), "got: {}", result);
        assert!(result.contains("8 files"), "got: {}", result);
    }

    #[test]
    fn test_filter_json_with_offenses() {
        let json = r#"{
  "files": [
    {
      "path": "app/models/user.rb",
      "offenses": [
        {
          "severity": "convention",
          "message": "Missing frozen string literal comment.",
          "cop_name": "Style/FrozenStringLiteralComment",
          "correctable": true,
          "location": {"start_line": 1, "start_column": 1, "last_line": 1, "last_column": 1, "length": 0}
        },
        {
          "severity": "warning",
          "message": "Trailing whitespace detected.",
          "cop_name": "Layout/TrailingWhitespace",
          "correctable": true,
          "location": {"start_line": 24, "start_column": 5, "last_line": 24, "last_column": 5, "length": 0}
        }
      ]
    },
    {
      "path": "app/services/user_service.rb",
      "offenses": [
        {
          "severity": "convention",
          "message": "Missing top-level documentation comment",
          "cop_name": "Style/Documentation",
          "correctable": false,
          "location": {"start_line": 8, "start_column": 1, "last_line": 8, "last_column": 1, "length": 0}
        }
      ]
    }
  ],
  "summary": {
    "offense_count": 3,
    "target_file_count": 5,
    "inspected_file_count": 5
  }
}"#;
        let result = filter_rubocop_json(json);
        assert!(result.contains("3 offenses detected"), "got: {}", result);
        assert!(result.contains("Top cops:"), "got: {}", result);
        assert!(result.contains("Top files:"), "got: {}", result);
        assert!(result.contains("user.rb"), "got: {}", result);
        assert!(result.contains("2 correctable"), "got: {}", result);
    }

    #[test]
    fn test_filter_json_correctable_hint() {
        let json = r#"{
  "files": [
    {
      "path": "app/models/user.rb",
      "offenses": [
        {
          "severity": "convention",
          "message": "Missing frozen string literal comment.",
          "cop_name": "Style/FrozenStringLiteralComment",
          "correctable": true,
          "location": {"start_line": 1, "start_column": 1, "last_line": 1, "last_column": 1, "length": 0}
        }
      ]
    }
  ],
  "summary": {"offense_count": 1, "target_file_count": 1, "inspected_file_count": 1}
}"#;
        let result = filter_rubocop_json(json);
        assert!(result.contains("rubocop -A"), "got: {}", result);
    }

    #[test]
    fn test_filter_text_no_offenses() {
        let text = "8 files inspected, 0 offenses detected\n";
        let result = filter_rubocop_text(text);
        assert!(result.contains("✓ RuboCop"), "got: {}", result);
        assert!(result.contains("no offenses"), "got: {}", result);
    }

    #[test]
    fn test_filter_text_with_offenses() {
        let text = r#"app/models/user.rb:12:3: C: Style/FrozenStringLiteralComment: Missing frozen string literal comment.
app/models/user.rb:24:5: W: Layout/TrailingWhitespace: Trailing whitespace detected.
app/services/user_service.rb:8:1: C: Style/Documentation: Missing top-level class documentation comment.

3 files inspected, 3 offenses detected
"#;
        let result = filter_rubocop_text(text);
        assert!(result.contains("offenses detected"), "got: {}", result);
        assert!(result.contains("user.rb"), "got: {}", result);
    }

    #[test]
    fn test_severity_char() {
        assert_eq!(severity_char("convention"), "C");
        assert_eq!(severity_char("warning"), "W");
        assert_eq!(severity_char("error"), "E");
        assert_eq!(severity_char("fatal"), "F");
        assert_eq!(severity_char("refactor"), "R");
        assert_eq!(severity_char("unknown"), "?");
    }
}
