//! Pure-Rust native ls implementation using std::fs.
//!
//! Provides a cross-platform `ls` command replacement that doesn't rely on
//! external binaries (especially important on Windows where GNU ls is missing).

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::constants::NOISE_DIRS;

/// Configuration for the native ls command.
#[derive(Debug, Clone, Default)]
pub struct LsConfig {
    /// Show all files (don't filter noise directories)
    pub show_all: bool,
    /// Long listing format
    pub show_long: bool,
    /// Human-readable sizes
    pub human_readable: bool,
    /// Sort by time (newest first)
    pub sort_by_time: bool,
    /// Reverse sort order
    pub reverse: bool,
    /// Recursive listing
    pub recursive: bool,
    /// Show file type indicator (/, @, etc.)
    pub classify: bool,
}

/// A directory entry with metadata.
#[derive(Debug)]
struct LsEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    permissions: u32,
    modified: SystemTime,
    owner: String,
    group: String,
}

impl LsEntry {
    fn from_path(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        let file_type = metadata.file_type();
        
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // On Windows, owner/group are not available in the same way
        #[cfg(unix)]
        let (owner, group) = {
            use std::os::unix::fs::MetadataExt;
            let uid = metadata.uid();
            let gid = metadata.gid();
            // In a real implementation, we'd look up uid/gid
            // For now, just show numeric IDs
            (uid.to_string(), gid.to_string())
        };
        
        #[cfg(windows)]
        let (owner, group) = ("user".to_string(), "group".to_string());

        // Unix reports a real mode. Windows has no mode bits, so synthesize the
        // two facts the rest of this module actually reads back out of the
        // field: whether it is writable, and whether it counts as executable.
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::MetadataExt;
            metadata.mode() & 0o7777
        };

        #[cfg(windows)]
        let permissions = {
            let base = if file_type.is_dir() || is_windows_executable(&name) {
                0o755
            } else {
                0o644
            };
            if metadata.permissions().readonly() {
                base & 0o555
            } else {
                base
            }
        };

        Ok(Self {
            path: path.to_path_buf(),
            name,
            is_dir: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
            size: metadata.len(),
            permissions,
            modified: metadata.modified()?,
            owner,
            group,
        })
    }
}

/// Windows marks a file executable by extension, not by a mode bit.
#[cfg(windows)]
fn is_windows_executable(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe" | "bat" | "cmd" | "com" | "ps1"
    )
}

/// Format a size in human-readable format.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

/// Format permissions as octal string (e.g., "755", "644").
fn format_perms(mode: u32) -> String {
    let owner = ((mode >> 6) & 0o7) as u8;
    let group = ((mode >> 3) & 0o7) as u8;
    let other = (mode & 0o7) as u8;
    
    let special = (mode >> 9) & 0o7;
    if special > 0 {
        format!("{}{}{}{}", special, owner, group, other)
    } else {
        format!("{}{}{}", owner, group, other)
    }
}

/// Format file type character for -F/--classify.
fn file_type_char(entry: &LsEntry) -> char {
    if entry.is_dir {
        '/'
    } else if entry.is_symlink {
        '@'
    } else if entry.permissions & 0o111 != 0 {
        '*'
    } else {
        ' '
    }
}

/// Check if a directory name matches any noise pattern.
fn is_noise_dir(name: &str, config: &LsConfig) -> bool {
    if config.show_all {
        return false;
    }
    for noise in NOISE_DIRS {
        if name == *noise || (noise.contains('*') && matches_glob(name, noise)) {
            return true;
        }
    }
    false
}

/// Simple glob matching for patterns like `*.log` or `build*`.
fn matches_glob(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    name == pattern
}

/// Read directory entries with filtering and sorting.
fn read_dir_entries(path: &Path, config: &LsConfig) -> Result<Vec<LsEntry>> {
    let mut entries = Vec::new();
    
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        
        // Skip . and .. unless show_all
        if (name == "." || name == "..") && !config.show_all {
            continue;
        }
        
        // Filter noise directories
        if entry.file_type()?.is_dir() && is_noise_dir(&name, config) {
            continue;
        }
        
        entries.push(LsEntry::from_path(&entry.path())?);
    }
    
    // Sort entries
    entries.sort_by(|a, b| {
        // Directories first
        let dir_cmp = b.is_dir.cmp(&a.is_dir);
        if dir_cmp != std::cmp::Ordering::Equal {
            return dir_cmp;
        }
        
        if config.sort_by_time {
            let time_cmp = b.modified.cmp(&a.modified);
            if config.reverse {
                time_cmp.reverse()
            } else {
                time_cmp
            }
        } else {
            let name_cmp = a.name.cmp(&b.name);
            if config.reverse {
                name_cmp.reverse()
            } else {
                name_cmp
            }
        }
    });
    
    Ok(entries)
}

/// Format a single entry for output.
fn format_entry(entry: &LsEntry, config: &LsConfig) -> String {
    let mut output = String::new();
    
    if config.show_long {
        // Permission string (simplified)
        let perms = format_perms(entry.permissions);
        output.push_str(&perms);
        output.push(' ');
        
        // Owner and group
        output.push_str(&entry.owner);
        output.push(' ');
        output.push_str(&entry.group);
        output.push(' ');
        
        // Size
        let size_str = if config.human_readable {
            human_size(entry.size)
        } else {
            entry.size.to_string()
        };
        output.push_str(&format!("{:>10}", size_str));
        output.push(' ');
        
        // Time (simplified - just date and time)
        let datetime = chrono::DateTime::<chrono::Local>::from(entry.modified);
        output.push_str(&datetime.format("%b %d %H:%M").to_string());
        output.push(' ');
    }
    
    // Name
    output.push_str(&entry.name);
    if config.classify {
        output.push(file_type_char(entry));
    }
    
    if entry.is_dir {
        output.push('/');
    }
    
    output
}

/// Recursive listing.
fn list_recursive(path: &Path, config: &LsConfig, prefix: &str) -> Result<String> {
    let mut output = String::new();
    let entries = read_dir_entries(path, config)?;
    
    if !prefix.is_empty() {
        output.push_str(prefix);
        output.push_str(":\n");
    }
    
    for entry in &entries {
        output.push_str(&format_entry(entry, config));
        output.push('\n');
    }
    
    if config.recursive {
        for entry in &entries {
            if entry.is_dir && entry.name != "." && entry.name != ".." {
                output.push('\n');
                output.push_str(&list_recursive(&entry.path, config, &entry.path.to_string_lossy())?);
            }
        }
    }
    
    Ok(output)
}

/// Run the native ls command.
pub fn run_native_ls(args: &[String], verbose: u8) -> Result<i32> {
    let mut config = LsConfig::default();
    let mut paths = Vec::new();
    
    // Parse arguments (simplified - matches common ls flags)
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-a" | "--all" => config.show_all = true,
            "-l" => config.show_long = true,
            "-h" | "--human-readable" => config.human_readable = true,
            "-t" => config.sort_by_time = true,
            "-r" | "--reverse" => config.reverse = true,
            "-R" | "--recursive" => config.recursive = true,
            "-F" | "--classify" => config.classify = true,
            "-1" => config.show_long = false, // Force single column
            _ if arg.starts_with('-') => {
                // Handle combined flags like -la, -lh, etc.
                for ch in arg.chars().skip(1) {
                    match ch {
                        'a' => config.show_all = true,
                        'l' => config.show_long = true,
                        'h' => config.human_readable = true,
                        't' => config.sort_by_time = true,
                        'r' => config.reverse = true,
                        'R' => config.recursive = true,
                        'F' => config.classify = true,
                        '1' => config.show_long = false,
                        _ => {}
                    }
                }
            }
            _ => paths.push(arg.clone()),
        }
        i += 1;
    }
    
    if paths.is_empty() {
        paths.push(".".to_string());
    }
    
    let mut output = String::new();
    for (idx, path_str) in paths.iter().enumerate() {
        let path = PathBuf::from(path_str);
        if idx > 0 {
            output.push('\n');
        }
        output.push_str(&list_recursive(&path, &config, if paths.len() > 1 { path_str } else { "" })?);
    }
    
    if verbose > 0 {
        eprintln!("Native ls for: {:?}", paths);
    }
    
    print!("{}", output);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(500), "500B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(1_048_576), "1.0M");
        assert_eq!(human_size(1_073_741_824), "1.0G");
    }

    #[test]
    fn test_format_perms() {
        assert_eq!(format_perms(0o755), "755");
        assert_eq!(format_perms(0o644), "644");
        assert_eq!(format_perms(0o4755), "4755");
    }

    #[test]
    fn test_matches_glob() {
        assert!(matches_glob("node_modules", "node_modules"));
        assert!(matches_glob("build", "build*"));
        assert!(matches_glob("build123", "build*"));
        assert!(matches_glob("error.log", "*.log"));
        assert!(!matches_glob("test.txt", "*.log"));
    }

    #[test]
    fn test_is_noise_dir() {
        let config = LsConfig::default();
        assert!(is_noise_dir("node_modules", &config));
        assert!(is_noise_dir(".git", &config));
        assert!(is_noise_dir("target", &config));
        assert!(!is_noise_dir("src", &config));
        
        let config_all = LsConfig { show_all: true, ..Default::default() };
        assert!(!is_noise_dir("node_modules", &config_all));
    }

    #[test]
    fn test_native_ls_basic() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        fs::write(root.join("README.md"), "# Test").unwrap();

        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/index.js"), "// fake").unwrap();

        let config = LsConfig::default();
        let entries = read_dir_entries(root, &config).unwrap();
        
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"src".to_string()));
        assert!(names.contains(&"Cargo.toml".to_string()));
        assert!(names.contains(&"README.md".to_string()));
        assert!(!names.contains(&"node_modules".to_string()));
    }

    #[test]
    fn test_native_ls_show_all() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let config = LsConfig { show_all: true, ..Default::default() };
        let entries = read_dir_entries(root, &config).unwrap();
        
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"node_modules".to_string()));
        assert!(names.contains(&"src".to_string()));
    }

    #[test]
    fn test_native_ls_sort_by_time() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("a.txt"), "first").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(root.join("b.txt"), "second").unwrap();

        let config = LsConfig { sort_by_time: true, ..Default::default() };
        let entries = read_dir_entries(root, &config).unwrap();
        
        // Newest first (b.txt should come before a.txt)
        assert_eq!(entries[0].name, "b.txt");
        assert_eq!(entries[1].name, "a.txt");
    }
}