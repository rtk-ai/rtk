use anyhow::{Context, Result};
use colored::Colorize;
use ignore::WalkBuilder;
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::process::Command;
use crate::tracking;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Tree,
    Flat,
    Json,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tree" => Ok(OutputFormat::Tree),
            "flat" => Ok(OutputFormat::Flat),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("Unknown format: {}", s)),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Tree => write!(f, "tree"),
            OutputFormat::Flat => write!(f, "flat"),
            OutputFormat::Json => write!(f, "json"),
        }
    }
}

lazy_static::lazy_static! {
    static ref ALWAYS_IGNORE: HashSet<&'static str> = {
        let mut set = HashSet::new();
        set.insert(".git");
        set.insert("node_modules");
        set.insert("target");
        set.insert("__pycache__");
        set.insert(".pytest_cache");
        set.insert(".mypy_cache");
        set.insert(".tox");
        set.insert(".venv");
        set.insert("venv");
        set.insert(".env");
        set.insert("dist");
        set.insert("build");
        set.insert(".next");
        set.insert(".nuxt");
        set.insert("coverage");
        set.insert(".coverage");
        set.insert(".nyc_output");
        set.insert(".cache");
        set.insert(".parcel-cache");
        set.insert(".turbo");
        set.insert("vendor");
        set.insert("Pods");
        set.insert(".gradle");
        set.insert(".idea");
        set.insert(".vscode");
        set.insert(".DS_Store");
        set
    };
}

#[derive(Debug, Clone)]
struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    depth: usize,
}

pub fn run(path: &Path, max_depth: usize, show_hidden: bool, format: OutputFormat, verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("Scanning: {}", path.display());
    }

    let entries = collect_entries(path, max_depth, show_hidden)?;

    // Capture output for tracking
    let output = match format {
        OutputFormat::Tree => format_tree(&entries),
        OutputFormat::Flat => format_flat(&entries),
        OutputFormat::Json => format_json(&entries)?,
    };

    println!("{}", output);

    // Get raw ls output for comparison
    let raw = get_raw_ls(path);
    tracking::track("ls -la", "rtk ls", &raw, &output);

    Ok(())
}

fn get_raw_ls(path: &Path) -> String {
    Command::new("ls")
        .args(["-la"])
        .arg(path)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn collect_entries(path: &Path, max_depth: usize, show_hidden: bool) -> Result<Vec<DirEntry>> {
    let walker = WalkBuilder::new(path)
        .max_depth(Some(max_depth))
        .hidden(!show_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !ALWAYS_IGNORE.contains(name.as_ref())
        })
        .build();

    let mut entries: Vec<DirEntry> = Vec::new();
    let base_depth = path.components().count();

    for result in walker {
        let entry = result.context("Failed to read directory entry")?;
        let entry_path = entry.path();

        // Skip the root itself
        if entry_path == path {
            continue;
        }

        let depth = entry_path.components().count() - base_depth;
        let name = entry_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);

        entries.push(DirEntry {
            name,
            path: entry_path.display().to_string(),
            is_dir,
            depth,
        });
    }

    // Sort: directories first, then alphabetically
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Ok(entries)
}

fn format_tree(entries: &[DirEntry]) -> String {
    let mut output = String::new();
    let mut depth_has_more: Vec<bool> = vec![false; 32];

    for (i, entry) in entries.iter().enumerate() {
        let is_last = entries
            .get(i + 1)
            .map(|next| next.depth <= entry.depth)
            .unwrap_or(true);

        let mut prefix = String::new();
        for d in 1..entry.depth {
            if depth_has_more.get(d).copied().unwrap_or(false) {
                prefix.push_str("│ ");
            } else {
                prefix.push_str("  ");
            }
        }

        if entry.depth > 0 {
            if is_last || entries.get(i + 1).map(|n| n.depth < entry.depth).unwrap_or(true) {
                prefix.push_str("└─");
                if entry.depth < depth_has_more.len() {
                    depth_has_more[entry.depth] = false;
                }
            } else {
                prefix.push_str("├─");
                if entry.depth < depth_has_more.len() {
                    depth_has_more[entry.depth] = true;
                }
            }
        }

        let display_name = if entry.is_dir {
            format!("{}/", entry.name).blue().bold().to_string()
        } else {
            colorize_by_extension(&entry.name)
        };

        output.push_str(&format!("{}{}\n", prefix, display_name));
    }
    output
}

fn colorize_by_extension(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "rs" => name.yellow().to_string(),
        "py" => name.green().to_string(),
        "js" | "ts" | "jsx" | "tsx" => name.cyan().to_string(),
        "go" => name.blue().to_string(),
        "md" | "txt" | "rst" => name.white().to_string(),
        "json" | "yaml" | "yml" | "toml" => name.magenta().to_string(),
        "sh" | "bash" | "zsh" => name.red().to_string(),
        _ => name.to_string(),
    }
}

fn format_flat(entries: &[DirEntry]) -> String {
    let mut output = String::new();
    for entry in entries {
        if entry.is_dir {
            output.push_str(&format!("{}/\n", entry.name));
        } else {
            output.push_str(&format!("{}\n", entry.name));
        }
    }
    output
}

fn format_json(entries: &[DirEntry]) -> Result<String> {
    #[derive(serde::Serialize)]
    struct JsonEntry {
        path: String,
        is_dir: bool,
        depth: usize,
    }

    let json_entries: Vec<JsonEntry> = entries
        .iter()
        .map(|e| JsonEntry {
            path: e.path.clone(),
            is_dir: e.is_dir,
            depth: e.depth,
        })
        .collect();

    Ok(serde_json::to_string_pretty(&json_entries)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parsing() {
        assert_eq!(OutputFormat::from_str("tree").unwrap(), OutputFormat::Tree);
        assert_eq!(OutputFormat::from_str("flat").unwrap(), OutputFormat::Flat);
        assert_eq!(OutputFormat::from_str("json").unwrap(), OutputFormat::Json);
    }

    #[test]
    fn test_always_ignore_contains_common_dirs() {
        assert!(ALWAYS_IGNORE.contains(".git"));
        assert!(ALWAYS_IGNORE.contains("node_modules"));
        assert!(ALWAYS_IGNORE.contains("target"));
    }
}
