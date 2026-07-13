use std::collections::HashMap;

use super::ls::{LsRecord, LsRecordType, FormatOptions};

/// Format bytes into human-readable size
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

/// Synthesizes the compact, token-optimized string from a list of records.
#[allow(unused_mut)]
pub fn synthesize_output(mut records: Vec<LsRecord>, options: &FormatOptions) -> (String, String) {
    if records.is_empty() {
        return ("(empty)\n".to_string(), String::new());
    }

    let mut by_ext = HashMap::new();

    // Sort to ensure stable output order on Unix if no custom sorting is requested
    #[cfg(not(target_os = "windows"))]{
        if !options.sort_by_time && !options.reverse {
            records.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    let mut dirs_out = String::new();
    let mut files_out = String::new();
    let mut symlinks_out = String::new();

    let mut dir_count = 0;
    let mut file_count = 0;
    let mut sym_count = 0;

    for r in &records {
        let perms_prefix = if options.show_long {
            if let Some(oct) = &r.octal_permissions {
                format!("{}  ", oct)
            } else {
                "".to_string()
            }
        } else {
            "".to_string()
        };

        match r.file_type {
            LsRecordType::DIRECTORY => {
                dirs_out.push_str(&format!("{}{}/\n", perms_prefix, r.name));
                dir_count += 1;
            }
            LsRecordType::SYMBOLINK => {
                symlinks_out.push_str(&format!("{}{}{}  {}\n", perms_prefix, r.name, if r.name.contains(" -> ") { "" } else { "" }, human_size(r.size)));
                sym_count += 1;
            }
            _ => {
                if r.file_type == LsRecordType::FILE {
                    *by_ext.entry(r.extension.clone()).or_insert(0) += 1;
                }
                files_out.push_str(&format!("{}{}{}  {}\n", perms_prefix, r.name, if r.name.contains(" -> ") { "" } else { "" }, human_size(r.size)));
                file_count += 1;
            }
        }
    }

    let mut entries = String::new();
    entries.push_str(&dirs_out);
    entries.push_str(&symlinks_out);
    entries.push_str(&files_out);

    // Summary line (separate so caller can suppress when piped)
    let mut summary = if sym_count > 0 {
        format!("\nSummary: {} files, {} dirs, {} symlinks", file_count, dir_count, sym_count)
    } else {
        format!("\nSummary: {} files, {} dirs", file_count, dir_count)
    };

    if !by_ext.is_empty() {
        let mut ext_counts: Vec<_> = by_ext.iter().collect();
        ext_counts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let ext_parts: Vec<String> = ext_counts
            .iter()
            .take(5)
            .map(|(ext, count)| format!("{} {}", count, ext))
            .collect();
        summary.push_str(" (");
        summary.push_str(&ext_parts.join(", "));
        if ext_counts.len() > 5 {
            summary.push_str(&format!(", +{} more", ext_counts.len() - 5));
        }
        summary.push(')');
    }
    summary.push('\n');

    (entries, summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(500), "500B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1234), "1.2K");
        assert_eq!(human_size(1_048_576), "1.0M");
        assert_eq!(human_size(2_500_000), "2.4M");
    }
}

