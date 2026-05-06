use super::constants::NOISE_DIRS;
use anyhow::Result;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;

/// Represents a single directory entry for token-optimized listing.
pub struct LsRecord {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub extension: String,
}

/// Fetches file information from the filesystem using native Rust std::fs.
pub fn fetch_entries(paths: &[&str], show_all: bool) -> Result<Vec<LsRecord>> {
    let mut records = Vec::new();
    let targets = if paths.is_empty() {
        vec!["."]
    } else {
        paths.to_vec()
    };

    for path_str in &targets {
        let path = Path::new(path_str);
        if !path.exists() {
            eprintln!("rtk: {}: No such file or directory", path_str);
            continue;
        }

        if path.is_dir() {
            for entry in std::fs::read_dir(path)?.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();

                if name == "." || name == ".." {
                    continue;
                }

                if !show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
                    continue;
                }

                if let Ok(metadata) = entry.metadata() {
                    records.push(LsRecord {
                        extension: get_extension(&name),
                        is_dir: metadata.is_dir(),
                        size: metadata.len(),
                        name,
                    });
                }
            }
        } else if let Ok(metadata) = path.metadata() {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            records.push(LsRecord {
                extension: get_extension(&name),
                is_dir: false,
                size: metadata.len(),
                name,
            });
        }
    }
    Ok(records)
}

/// Synthesizes the compact, token-optimized string from a list of records.
pub fn synthesize_output(records: Vec<LsRecord>) -> (String, String) {
    if records.is_empty() {
        return ("(empty)\n".to_string(), String::new());
    }

    let mut dirs: Vec<&LsRecord> = records.iter().filter(|r| r.is_dir).collect();
    let mut files: Vec<&LsRecord> = records.iter().filter(|r| !r.is_dir).collect();
    let mut by_ext = HashMap::new();

    for file in &files {
        *by_ext.entry(file.extension.clone()).or_insert(0) += 1;
    }

    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));

    let mut entries = String::new();
    for d in &dirs {
        entries.push_str(&format!("{}/\n", d.name));
    }
    for f in &files {
        entries.push_str(&format!("{}  {}\n", f.name, human_size(f.size)));
    }

    let mut summary = format!("\nSummary: {} files, {} dirs", files.len(), dirs.len());
    if !by_ext.is_empty() {
        let mut ext_counts: Vec<_> = by_ext.iter().collect();
        ext_counts.sort_by(|a, b| b.1.cmp(a.1));
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

/// Entry point called by ls::run on Windows.
pub fn run_native(paths: &[&str], show_all: bool) -> Result<i32> {
    let timer = crate::core::tracking::TimedExecution::start();
    
    let records = fetch_entries(paths, show_all)?;
    let (entries, summary) = synthesize_output(records);

    let is_tty = std::io::stdout().is_terminal();
    let output = if is_tty {
        format!("{}{}", entries, summary)
    } else {
        entries
    };

    print!("{}", output);

    timer.track(
        &format!("ls (native) {}", paths.join(" ")),
        &format!("rtk ls {}", paths.join(" ")),
        "", 
        &output,
    );

    Ok(0)
}

fn get_extension(name: &str) -> String {
    if let Some(pos) = name.rfind('.') {
        name[pos..].to_string()
    } else {
        "no ext".to_string()
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}
