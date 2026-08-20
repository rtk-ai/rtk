//! Pure-Rust native tree implementation using walkdir and ignore crates.
//!
//! Provides a cross-platform `tree` command replacement that doesn't rely on
//! external binaries (especially important on Windows where `tree.com` has
//! incompatible flags).

use anyhow::Result;
use ignore::WalkBuilder;
use std::fs;
use std::path::{Path, PathBuf};

use super::constants::NOISE_DIRS;

/// Configuration for the native tree walker.
#[derive(Debug, Clone, Default)]
pub struct TreeConfig {
    /// Maximum depth to traverse (0 = unlimited)
    pub max_depth: Option<usize>,
    /// Show all files (don't filter noise directories)
    pub show_all: bool,
    /// Custom ignore patterns (in addition to NOISE_DIRS)
    pub ignore_patterns: Vec<String>,
    /// Show file sizes
    pub show_sizes: bool,
    /// Show permissions (Unix-style)
    pub show_permissions: bool,
    /// Colorize output
    pub color: bool,
}

/// A node in the tree structure.
#[derive(Debug)]
struct TreeNode {
    // Carried while the tree is built but not rendered: `path` disambiguates
    // nodes during the walk, and `permissions` is collected ahead of the
    // `-p` flag that `render_tree` does not print yet.
    #[allow(dead_code)]
    path: PathBuf,
    name: String,
    is_dir: bool,
    size: u64,
    #[allow(dead_code)]
    permissions: Option<String>,
    children: Vec<TreeNode>,
    depth: usize,
}

impl TreeNode {
    fn new(path: PathBuf, depth: usize) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let is_dir = path.is_dir();
        let metadata = fs::metadata(&path)?;
        let size = metadata.len();
        let permissions = None; // TODO: implement on Unix

        Ok(Self {
            path,
            name,
            is_dir,
            size,
            permissions,
            children: Vec::new(),
            depth,
        })
    }
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

/// Check if a directory name matches any noise pattern.
fn is_noise_dir(name: &str, config: &TreeConfig) -> bool {
    if config.show_all {
        return false;
    }
    for noise in NOISE_DIRS {
        if name == *noise || (noise.contains('*') && matches_glob(name, noise)) {
            return true;
        }
    }
    for pattern in &config.ignore_patterns {
        if matches_glob(name, pattern) {
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

/// Build the tree structure from a root path.
fn build_tree(root: &Path, config: &TreeConfig) -> Result<TreeNode> {
    let mut root_node = TreeNode::new(root.to_path_buf(), 0)?;
    root_node.name = ".".to_string(); // Root displays as "."

    let walker = WalkBuilder::new(root)
        .max_depth(config.max_depth)
        .hidden(false) // We handle hidden files ourselves via noise filtering
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    // Use a map to track parent nodes by path
    let mut nodes: Vec<(PathBuf, TreeNode)> = Vec::new();
    nodes.push((root.to_path_buf(), root_node));

    for entry in walker {
        let entry = entry?;
        let path = entry.path().to_path_buf();
        let depth = entry.depth();

        // Skip the root itself (depth 0)
        if depth == 0 {
            continue;
        }

        let parent_path = path.parent().unwrap().to_path_buf();

        // Check if parent is filtered out
        let parent_name = parent_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_noise_dir(parent_name, config) {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if is_noise_dir(&name, config) {
            continue;
        }

        let metadata = fs::metadata(&path)?;
        let is_dir = metadata.is_dir();
        let size = metadata.len();

        let node = TreeNode {
            path: path.clone(),
            name,
            is_dir,
            size,
            permissions: None,
            children: Vec::new(),
            depth,
        };

        nodes.push((path, node));
    }

    // Sort by depth to ensure parents are processed before children
    nodes.sort_by_key(|(_, n)| n.depth);

    // Rebuild tree structure
    let mut path_to_index = std::collections::HashMap::new();
    for (i, (path, _)) in nodes.iter().enumerate() {
        path_to_index.insert(path.clone(), i);
    }

    // Move children to parents
    for i in (1..nodes.len()).rev() {
        let (path, node) = nodes.remove(i);
        if let Some(parent_path) = path.parent() {
            if let Some(&parent_idx) = path_to_index.get(parent_path) {
                nodes[parent_idx].1.children.push(node);
            }
        }
    }

    // Sort children: directories first, then files, both alphabetically
    fn sort_children(node: &mut TreeNode) {
        node.children.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });
        for child in &mut node.children {
            sort_children(child);
        }
    }
    sort_children(&mut nodes[0].1);

    // `nodes` is discarded right after this, so take the root out by value
    // rather than cloning a whole subtree just to satisfy the borrow checker.
    Ok(nodes.swap_remove(0).1)
}

/// Render the tree as a string with ASCII box-drawing characters.
fn render_tree(node: &TreeNode, config: &TreeConfig, prefix: &str, is_last: bool, is_root: bool) -> String {
    let mut output = String::new();

    if is_root {
        output.push_str(".\n");
    } else {
        let connector = if is_last { "└── " } else { "├── " };
        output.push_str(prefix);
        output.push_str(connector);
        output.push_str(&node.name);
        if node.is_dir {
            output.push('/');
        }
        if config.show_sizes && !node.is_dir {
            output.push_str("  ");
            output.push_str(&human_size(node.size));
        }
        output.push('\n');
    }

    let child_prefix = if is_root {
        String::new()
    } else if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    };

    let child_count = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        let is_last_child = i == child_count - 1;
        output.push_str(&render_tree(child, config, &child_prefix, is_last_child, false));
    }

    output
}

/// Run the native tree command.
pub fn run_native_tree(args: &[String], verbose: u8) -> Result<i32> {
    let mut config = TreeConfig::default();

    // Parse arguments
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-a" | "--all" => config.show_all = true,
            "-L" | "--level" => {
                if i + 1 < args.len() {
                    if let Ok(depth) = args[i + 1].parse::<usize>() {
                        config.max_depth = Some(depth);
                        i += 1;
                    }
                }
            }
            "-I" | "--ignore" => {
                if i + 1 < args.len() {
                    config.ignore_patterns.push(args[i + 1].clone());
                    i += 1;
                }
            }
            "-h" | "--human-readable" => config.show_sizes = true,
            "-p" | "--permissions" => config.show_permissions = true,
            "-C" | "--color" => config.color = true,
            _ if arg.starts_with('-') => {
                // Unknown flag, ignore
            }
            _ => paths.push(arg.clone()),
        }
        i += 1;
    }

    let root = if paths.is_empty() {
        std::env::current_dir()?
    } else {
        PathBuf::from(&paths[0])
    };

    let tree = build_tree(&root, &config)?;
    let output = render_tree(&tree, &config, "", true, true);

    if verbose > 0 {
        eprintln!("Native tree rendered for: {}", root.display());
    }

    println!("{}", output);
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
    fn test_matches_glob() {
        assert!(matches_glob("node_modules", "node_modules"));
        assert!(matches_glob("build", "build*"));
        assert!(matches_glob("build123", "build*"));
        assert!(matches_glob("error.log", "*.log"));
        assert!(matches_glob("test.log", "*.log"));
        assert!(!matches_glob("test.txt", "*.log"));
        assert!(matches_glob("anything", "*"));
    }

    #[test]
    fn test_is_noise_dir() {
        let config = TreeConfig::default();
        assert!(is_noise_dir("node_modules", &config));
        assert!(is_noise_dir(".git", &config));
        assert!(is_noise_dir("target", &config));
        assert!(is_noise_dir("__pycache__", &config));
        assert!(!is_noise_dir("src", &config));
        assert!(!is_noise_dir("main.rs", &config));

        // With show_all = true
        let config_all = TreeConfig { show_all: true, ..Default::default() };
        assert!(!is_noise_dir("node_modules", &config_all));
    }

    #[test]
    fn test_native_tree_basic() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        // Create test structure
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/lib.rs"), "pub mod foo;").unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/test.rs"), "#[test] fn test() {}").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        fs::write(root.join("README.md"), "# Test").unwrap();

        // Create noise dirs that should be filtered
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/index.js"), "// fake").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "[core]").unwrap();

        let config = TreeConfig::default();
        let tree = build_tree(root, &config).unwrap();
        let output = render_tree(&tree, &config, "", true, true);

        // Should contain our files
        assert!(output.contains("src/"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("lib.rs"));
        assert!(output.contains("tests/"));
        assert!(output.contains("test.rs"));
        assert!(output.contains("Cargo.toml"));
        assert!(output.contains("README.md"));

        // Should NOT contain noise dirs
        assert!(!output.contains("node_modules"));
        assert!(!output.contains(".git"));
    }

    #[test]
    fn test_native_tree_with_all_flag() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/index.js"), "// fake").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let config = TreeConfig { show_all: true, ..Default::default() };
        let tree = build_tree(root, &config).unwrap();
        let output = render_tree(&tree, &config, "", true, true);

        // Should now contain noise dirs
        assert!(output.contains("node_modules"));
        assert!(output.contains("index.js"));
        assert!(output.contains("src/"));
    }

    #[test]
    fn test_native_tree_max_depth() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::write(root.join("a/b/c/deep.txt"), "deep").unwrap();
        fs::write(root.join("a/top.txt"), "top").unwrap();

        let config = TreeConfig { max_depth: Some(2), ..Default::default() };
        let tree = build_tree(root, &config).unwrap();
        let output = render_tree(&tree, &config, "", true, true);

        // -L 2 keeps two levels below the root: `a` and `b` are in, `c` is not.
        assert!(output.contains("a/"), "{output}");
        assert!(output.contains("top.txt"), "{output}");
        assert!(!output.contains("c/"), "{output}");
        assert!(!output.contains("deep.txt"), "{output}");
    }

    #[test]
    fn test_native_tree_custom_ignore() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/output.bin"), "binary").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let config = TreeConfig {
            ignore_patterns: vec!["build*".to_string()],
            ..Default::default()
        };
        let tree = build_tree(root, &config).unwrap();
        let output = render_tree(&tree, &config, "", true, true);

        assert!(!output.contains("build"));
        assert!(output.contains("src/"));
    }
}