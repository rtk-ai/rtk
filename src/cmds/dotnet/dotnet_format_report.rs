//! Parses dotnet format JSON reports into compact summaries.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FormatReportEntry {
    file_path: String,
    #[serde(default)]
    file_changes: Vec<FileChange>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FileChange {
    line_number: u32,
    char_number: u32,
    diagnostic_id: String,
    format_description: String,
}

#[derive(Debug)]
pub struct ChangeDetail {
    pub line_number: u32,
    pub char_number: u32,
    pub diagnostic_id: String,
    pub format_description: String,
}

#[derive(Debug)]
pub struct FileWithChanges {
    pub path: String,
    pub changes: Vec<ChangeDetail>,
}

#[derive(Debug)]
pub struct FormatSummary {
    pub files_with_changes: Vec<FileWithChanges>,
    pub files_unchanged: usize,
    pub total_files: usize,
}

pub fn parse_format_report(path: &Path) -> Result<FormatSummary> {
    let file = File::open(path)
        .with_context(|| format!("Failed to read dotnet format report at {}", path.display()))?;
    let reader = BufReader::new(file);

    let entries: Vec<FormatReportEntry> = serde_json::from_reader(reader).with_context(|| {
        format!(
            "Failed to parse dotnet format report JSON at {}",
            path.display()
        )
    })?;

    let total_files = entries.len();
    let files_with_changes: Vec<FileWithChanges> = entries
        .into_iter()
        .filter_map(|entry| {
            if entry.file_changes.is_empty() {
                return None;
            }

            let changes = entry
                .file_changes
                .into_iter()
                .map(|change| ChangeDetail {
                    line_number: change.line_number,
                    char_number: change.char_number,
                    diagnostic_id: change.diagnostic_id,
                    format_description: change.format_description,
                })
                .collect();

            Some(FileWithChanges {
                path: entry.file_path,
                changes,
            })
        })
        .collect();

    let files_unchanged = total_files.saturating_sub(files_with_changes.len());

    Ok(FormatSummary {
        files_with_changes,
        files_unchanged,
        total_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("dotnet")
            .join(name)
    }

    #[test]
    fn test_parse_format_report_all_formatted() {
        let summary = parse_format_report(&fixture("format_success.json")).expect("parse report");

        assert_eq!(summary.total_files, 2);
        assert_eq!(summary.files_unchanged, 2);
        assert!(summary.files_with_changes.is_empty());
    }

    #[test]
    fn test_parse_format_report_with_changes() {
        let summary = parse_format_report(&fixture("format_changes.json")).expect("parse report");

        assert_eq!(summary.total_files, 3);
        assert_eq!(summary.files_unchanged, 1);
        assert_eq!(summary.files_with_changes.len(), 2);
        assert!(summary.files_with_changes[0].path.contains("Program.cs"));
        assert_eq!(summary.files_with_changes[0].changes[0].line_number, 42);
    }

    #[test]
    fn test_parse_format_report_empty() {
        let summary = parse_format_report(&fixture("format_empty.json")).expect("parse report");

        assert_eq!(summary.total_files, 0);
        assert_eq!(summary.files_unchanged, 0);
        assert!(summary.files_with_changes.is_empty());
    }
}
