use crate::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::process::Command;
use std::str::FromStr;

lazy_static! {
    /// Matches timestamped INFO/WARNING/DEBUG lines from Bazel stderr
    /// e.g. "(10:23:45) INFO: Build option..."
    static ref NOISE_WITH_TIMESTAMP: Regex =
        Regex::new(r"^\(\d+:\d+:\d+\)\s*(INFO|WARNING|DEBUG):").unwrap();

    /// Matches plain INFO/WARNING/DEBUG lines without timestamp
    static ref NOISE_PLAIN: Regex =
        Regex::new(r"^(INFO|WARNING|DEBUG):").unwrap();

    /// Matches ERROR lines (with or without timestamp)
    static ref ERROR_WITH_TIMESTAMP: Regex =
        Regex::new(r"^\(\d+:\d+:\d+\)\s*ERROR:").unwrap();
    static ref ERROR_PLAIN: Regex =
        Regex::new(r"^ERROR:").unwrap();

    /// Matches Bazel target lines like //package/path:target_name or //:root_target
    static ref TARGET_LINE: Regex =
        Regex::new(r"^(//[^:]*):(.+)$").unwrap();
}

/// A limit value that can be a specific number or unlimited ("all").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    N(usize),
    All,
}

impl Limit {
    pub fn value(&self) -> usize {
        match self {
            Limit::N(n) => *n,
            Limit::All => usize::MAX,
        }
    }
}

impl FromStr for Limit {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("all") {
            Ok(Limit::All)
        } else {
            s.parse::<usize>()
                .map(Limit::N)
                .map_err(|_| format!("expected a number or 'all', got '{}'", s))
        }
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Limit::N(n) => write!(f, "{}", n),
            Limit::All => write!(f, "all"),
        }
    }
}

/// Detect the query root from args.
/// Scans for the first `//path/...` pattern and returns `(display_expr, root_path)`.
/// Fallback: `("//...", "//")`.
fn detect_query_root(args: &[String]) -> (String, String) {
    for arg in args {
        let trimmed = arg.trim_matches('\'').trim_matches('"');
        if trimmed.contains("//") && trimmed.contains("...") {
            let root = trimmed.trim_end_matches("...");
            let root = root.trim_end_matches('/');
            let root = if root.is_empty() { "//" } else { root };
            return (trimmed.to_string(), root.to_string());
        }
    }
    ("//...".to_string(), "//".to_string())
}

/// Count path components of a package relative to a root.
/// root="//" package="//src/lib/foo" → 3 (src, lib, foo)
/// root="//src" package="//src/lib/foo" → 2 (lib, foo)
#[cfg(test)]
fn package_depth(root: &str, package: &str) -> usize {
    let root_stripped = root.strip_prefix("//").unwrap_or(root);
    let pkg_stripped = package.strip_prefix("//").unwrap_or(package);

    let relative = if root_stripped.is_empty() {
        pkg_stripped
    } else {
        pkg_stripped
            .strip_prefix(root_stripped)
            .unwrap_or(pkg_stripped)
            .strip_prefix('/')
            .unwrap_or("")
    };

    if relative.is_empty() {
        0
    } else {
        relative.split('/').count()
    }
}

/// Extract the relative name of a child package under a parent.
/// parent="//examples", child="//examples/cpp" → "cpp"
/// parent="//", child="//src" → "src"
#[cfg(test)]
fn relative_name(parent: &str, child: &str) -> String {
    let parent_stripped = parent.strip_prefix("//").unwrap_or(parent);
    let child_stripped = child.strip_prefix("//").unwrap_or(child);

    if parent_stripped.is_empty() {
        // root parent, take first component
        child_stripped.split('/').next().unwrap_or("").to_string()
    } else {
        child_stripped
            .strip_prefix(parent_stripped)
            .unwrap_or(child_stripped)
            .strip_prefix('/')
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string()
    }
}

/// A node in the package tree for hierarchical rendering.
#[derive(Debug, Default)]
struct TreeNode {
    /// Targets directly in this package
    targets: Vec<String>,
    /// Child package nodes, keyed by their relative name
    children: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    /// Count cumulative targets in entire subtree (including self).
    fn cumulative_targets(&self) -> usize {
        self.targets.len()
            + self
                .children
                .values()
                .map(|c| c.cumulative_targets())
                .sum::<usize>()
    }

    /// Count cumulative sub-packages in entire subtree (not including self).
    fn cumulative_packages(&self) -> usize {
        let direct = self.children.len();
        direct
            + self
                .children
                .values()
                .map(|c| c.cumulative_packages())
                .sum::<usize>()
    }
}

/// Build a tree from a flat BTreeMap of packages under a given root.
fn build_tree(packages: &BTreeMap<String, Vec<String>>, root: &str) -> TreeNode {
    let mut tree = TreeNode::default();

    // Add root's own targets if present
    if let Some(targets) = packages.get(root) {
        tree.targets = targets.clone();
    }

    // Collect all packages under this root (excluding the root itself)
    let root_stripped = root.strip_prefix("//").unwrap_or(root);

    for (pkg, targets) in packages {
        let pkg_stripped = pkg.strip_prefix("//").unwrap_or(pkg);

        // Skip the root package itself
        if pkg_stripped == root_stripped {
            continue;
        }

        // Check if this package is under the root
        let relative = if root_stripped.is_empty() {
            if pkg_stripped.is_empty() {
                continue;
            }
            pkg_stripped.to_string()
        } else if let Some(rest) = pkg_stripped.strip_prefix(root_stripped) {
            if let Some(rest) = rest.strip_prefix('/') {
                rest.to_string()
            } else {
                continue;
            }
        } else {
            continue;
        };

        // Walk the path components and insert into tree
        let parts: Vec<&str> = relative.split('/').collect();
        let mut current = &mut tree;

        for part in &parts {
            current = current.children.entry(part.to_string()).or_default();
        }

        // Set targets on the leaf node
        current.targets = targets.clone();
    }

    tree
}

/// Format a count label like "5 targets" or "1 target", with optional package count.
fn format_counts(target_count: usize, package_count: usize) -> String {
    let mut parts = Vec::new();

    if target_count > 0 {
        let label = if target_count == 1 {
            "target"
        } else {
            "targets"
        };
        parts.push(format!("{} {}", target_count, label));
    }

    if package_count > 0 {
        let label = if package_count == 1 {
            "package"
        } else {
            "packages"
        };
        parts.push(format!("{} {}", package_count, label));
    }

    if parts.is_empty() {
        "0 targets".to_string()
    } else {
        parts.join(", ")
    }
}

/// Render a tree node's children at a given depth, with indentation.
fn render_tree(
    node: &TreeNode,
    max_depth: usize,
    width: usize,
    current_depth: usize,
    result: &mut String,
) {
    if current_depth >= max_depth {
        return;
    }

    let indent = "  ".repeat(current_depth);

    let child_count = node.children.len();
    let target_count = node.targets.len();

    // Width budget: sub-packages first, then targets
    let pkg_slots = width.min(child_count);
    let remaining_slots = width.saturating_sub(pkg_slots);
    let target_slots = remaining_slots.min(target_count);

    let hidden_packages = child_count.saturating_sub(pkg_slots);
    let hidden_targets = target_count.saturating_sub(target_slots);

    // Render sub-packages
    for (i, (name, child)) in node.children.iter().enumerate() {
        if i >= pkg_slots {
            break;
        }
        let cum_targets = child.cumulative_targets();
        let cum_packages = child.cumulative_packages();
        let counts = format_counts(cum_targets, cum_packages);
        result.push_str(&format!("{}📦 {} ({})\n", indent, name, counts));

        // Recurse into child if within depth
        render_tree(child, max_depth, width, current_depth + 1, result);
    }

    // Render targets
    for (i, target) in node.targets.iter().enumerate() {
        if i >= target_slots {
            break;
        }
        result.push_str(&format!("{}🎯 :{}\n", indent, target));
    }

    // Truncation line
    if hidden_packages > 0 || hidden_targets > 0 {
        let mut parts = Vec::new();
        if hidden_packages > 0 {
            parts.push(format!(
                "{} more sub-package{}",
                hidden_packages,
                if hidden_packages == 1 { "" } else { "s" }
            ));
        }
        if hidden_targets > 0 {
            parts.push(format!(
                "{} more target{}",
                hidden_targets,
                if hidden_targets == 1 { "" } else { "s" }
            ));
        }
        result.push_str(&format!("{}(+{})\n", indent, parts.join(", ")));
    }
}

/// Filter bazel query output with depth/width controls.
///
/// - `depth`: how many levels deep to show (usize::MAX for unlimited)
/// - `width`: max items per level (usize::MAX for unlimited)
/// - `root`: (display_expr, root_path) from detect_query_root
pub fn filter_bazel_query(
    stdout: &str,
    stderr: &str,
    depth: usize,
    width: usize,
    root: &(String, String),
) -> String {
    let mut result = String::new();

    // Collect ERROR lines from stderr
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if ERROR_WITH_TIMESTAMP.is_match(trimmed) || ERROR_PLAIN.is_match(trimmed) {
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    // Group targets by package, preserve non-target lines
    let mut packages: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut non_target_lines: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(caps) = TARGET_LINE.captures(trimmed) {
            let package = caps[1].to_string();
            let target = caps[2].to_string();
            packages.entry(package).or_default().push(target);
        } else {
            non_target_lines.push(trimmed.to_string());
        }
    }

    let (display_expr, root_path) = root;
    let tree = build_tree(&packages, root_path);

    let total_targets = tree.cumulative_targets();
    let total_packages = tree.cumulative_packages();
    let counts = format_counts(total_targets, total_packages);

    // Header line (no emoji)
    result.push_str(&format!("{} ({})\n", display_expr, counts));

    // Render children
    render_tree(&tree, depth, width, 0, &mut result);

    // Output non-target lines
    for line in &non_target_lines {
        result.push_str(line);
        result.push('\n');
    }

    result.trim_end().to_string()
}

pub fn run_query(args: &[String], depth: Limit, width: Limit, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let root = detect_query_root(args);

    let mut cmd = Command::new("bazel");
    cmd.arg("query");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bazel query {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run bazel query. Is Bazel installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_bazel_query(&stdout, &stderr, depth.value(), width.value(), &root);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "bazel_query", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("bazel query {}", args.join(" ")),
        &format!("rtk bazel query {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("bazel: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = Command::new("bazel");
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bazel {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run bazel {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("bazel {}", subcommand),
        &format!("rtk bazel {}", subcommand),
        &raw,
        &raw, // No filtering for unsupported commands
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_root() -> (String, String) {
        ("//...".to_string(), "//".to_string())
    }

    fn query(stdout: &str, stderr: &str, depth: usize, width: usize) -> String {
        filter_bazel_query(stdout, stderr, depth, width, &default_root())
    }

    // === Limit type tests ===

    #[test]
    fn test_limit_from_str() {
        assert_eq!("1".parse::<Limit>().unwrap(), Limit::N(1));
        assert_eq!("10".parse::<Limit>().unwrap(), Limit::N(10));
        assert_eq!("0".parse::<Limit>().unwrap(), Limit::N(0));
        assert_eq!("all".parse::<Limit>().unwrap(), Limit::All);
        assert_eq!("ALL".parse::<Limit>().unwrap(), Limit::All);
        assert_eq!("All".parse::<Limit>().unwrap(), Limit::All);
        assert!("invalid".parse::<Limit>().is_err());
        assert!("".parse::<Limit>().is_err());
    }

    #[test]
    fn test_limit_value() {
        assert_eq!(Limit::N(5).value(), 5);
        assert_eq!(Limit::All.value(), usize::MAX);
    }

    #[test]
    fn test_limit_display() {
        assert_eq!(Limit::N(5).to_string(), "5");
        assert_eq!(Limit::All.to_string(), "all");
    }

    // === Helper function tests ===

    #[test]
    fn test_detect_query_root() {
        // Single //...
        let root = detect_query_root(&["//...".to_string()]);
        assert_eq!(root, ("//...".to_string(), "//".to_string()));

        // Subpath
        let root = detect_query_root(&["//examples/...".to_string()]);
        assert_eq!(
            root,
            ("//examples/...".to_string(), "//examples".to_string())
        );

        // Quoted args
        let root = detect_query_root(&["'//src/...'".to_string()]);
        assert_eq!(root, ("//src/...".to_string(), "//src".to_string()));

        // No match → fallback
        let root = detect_query_root(&["--keep_going".to_string()]);
        assert_eq!(root, ("//...".to_string(), "//".to_string()));

        // Multiple args → takes first match
        let root = detect_query_root(&["--keep_going".to_string(), "//host/...".to_string()]);
        assert_eq!(root, ("//host/...".to_string(), "//host".to_string()));
    }

    #[test]
    fn test_package_depth() {
        assert_eq!(package_depth("//", "//src"), 1);
        assert_eq!(package_depth("//", "//src/lib"), 2);
        assert_eq!(package_depth("//", "//src/lib/foo"), 3);
        assert_eq!(package_depth("//", "//"), 0);
        assert_eq!(package_depth("//src", "//src"), 0);
        assert_eq!(package_depth("//src", "//src/lib"), 1);
        assert_eq!(package_depth("//src", "//src/lib/foo"), 2);
    }

    #[test]
    fn test_relative_name() {
        assert_eq!(relative_name("//", "//src"), "src");
        assert_eq!(relative_name("//", "//src/lib"), "src");
        assert_eq!(relative_name("//examples", "//examples/cpp"), "cpp");
        assert_eq!(
            relative_name("//examples", "//examples/java-native"),
            "java-native"
        );
    }

    // === Core filter tests ===

    #[test]
    fn test_strips_info_warning_noise() {
        let stderr = "\
(10:23:45) INFO: Invocation ID: abc-123
(10:23:45) INFO: Build options changed
(10:23:46) WARNING: some warning
(10:23:47) DEBUG: debug info
INFO: plain info line
WARNING: plain warning
DEBUG: plain debug";
        let stdout = "//pkg:target";
        let result = query(stdout, stderr, usize::MAX, usize::MAX);

        assert!(!result.contains("Invocation ID"));
        assert!(!result.contains("Build options changed"));
        assert!(!result.contains("some warning"));
        assert!(!result.contains("debug info"));
        assert!(!result.contains("plain info line"));
        assert!(!result.contains("plain warning"));
        assert!(!result.contains("plain debug"));
        assert!(result.contains("🎯 :target"));
    }

    #[test]
    fn test_keeps_error_lines() {
        let stderr = "\
(10:23:45) INFO: Build options changed
(10:23:46) ERROR: something went wrong
ERROR: another error";
        let stdout = "//pkg:target";
        let result = query(stdout, stderr, usize::MAX, usize::MAX);

        assert!(result.contains("ERROR: something went wrong"));
        assert!(result.contains("ERROR: another error"));
        assert!(!result.contains("Build options changed"));
    }

    #[test]
    fn test_empty_output() {
        let result = query("", "", usize::MAX, usize::MAX);
        // With default root, header is still produced
        assert!(result.contains("//... (0 targets)"));
    }

    #[test]
    fn test_non_target_lines_pass_through() {
        let stdout = "\
//pkg:target_a
some non-target output line
//:root_target";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        assert!(result.contains("some non-target output line"));
        assert!(result.contains("🎯 :target_a"));
        assert!(result.contains("🎯 :root_target"));
    }

    #[test]
    fn test_single_target_uses_singular() {
        let stdout = "//my/package:only_target";
        let result = query(stdout, "", usize::MAX, usize::MAX);
        assert!(result.contains("(1 target)"));
    }

    // === Header line tests ===

    #[test]
    fn test_header_line() {
        let stdout = "\
//src/lib:a
//src/lib:b
//tools:c";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        // Header has cumulative totals, no emoji
        assert!(result.starts_with("//... (3 targets, 3 packages)"));
    }

    // === Depth tests ===

    #[test]
    fn test_depth_1_collapses_to_summary() {
        let stdout = "\
//src/lib:a
//src/lib:b
//src/app:c
//tools/gen:d
//tools/gen:e
//tools/gen:f
//:root_target";
        let result = query(stdout, "", 1, usize::MAX);

        // Depth 1: should show src, tools as 📦 with cumulative counts
        assert!(result.contains("📦 src (3 targets, 2 packages)"));
        assert!(result.contains("📦 tools (3 targets, 1 package)"));
        // Root target shown as 🎯
        assert!(result.contains("🎯 :root_target"));
        // Should NOT show children (lib, app, gen)
        assert!(!result.contains("📦 lib"));
        assert!(!result.contains("📦 app"));
        assert!(!result.contains("📦 gen"));
    }

    #[test]
    fn test_depth_2_shows_two_levels() {
        let stdout = "\
//src/lib/math:a
//src/lib/math:b
//src/lib/io:c
//src/app:d
//tools:e";
        let result = query(stdout, "", 2, usize::MAX);

        // Level 1: src, tools visible
        assert!(result.contains("📦 src (4 targets, 4 packages)"));
        assert!(result.contains("📦 tools (1 target)"));
        // Level 2: lib and app visible under src with relative names
        assert!(result.contains("  📦 lib (3 targets, 2 packages)"));
        assert!(result.contains("  📦 app (1 target)"));
        // Level 3 (math, io) NOT expanded
        assert!(!result.contains("    📦 math"));
        assert!(!result.contains("    📦 io"));
    }

    #[test]
    fn test_depth_all_shows_everything() {
        let stdout = "\
//src/lib/math:a
//src/lib/io:b
//src/app:c";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        // All levels visible
        assert!(result.contains("📦 src"));
        assert!(result.contains("  📦 lib"));
        assert!(result.contains("    📦 math"));
        assert!(result.contains("    📦 io"));
        assert!(result.contains("  📦 app"));
        // Leaf targets shown
        assert!(result.contains("      🎯 :a"));
        assert!(result.contains("      🎯 :b"));
        assert!(result.contains("    🎯 :c"));
    }

    #[test]
    fn test_always_cumulative_counts() {
        // Even when expanded, parent shows full subtree count
        let stdout = "\
//examples/cpp:a
//examples/cpp:b
//examples/go:c
//examples/java/sub:d";
        let result = query(stdout, "", 2, usize::MAX);

        // examples shows cumulative: 4 targets, 4 packages (cpp, go, java, sub)
        // Note: java/sub is counted as an additional package node in the tree
        assert!(result.contains("📦 examples (4 targets, 4 packages)"));
        // Children are expanded but parent still shows full counts
        assert!(result.contains("  📦 cpp (2 targets)"));
        assert!(result.contains("  📦 go (1 target)"));
        assert!(result.contains("  📦 java (1 target, 1 package)"));
    }

    // === Width tests ===

    #[test]
    fn test_width_budget_packages_then_targets() {
        // Width 5: 3 sub-packages take 3 slots, 2 remaining for targets
        let stdout = "\
//src:a
//src:b
//src:c
//src:d
//tools:e
//lib:f
//:root_a
//:root_b
//:root_c";
        let result = query(stdout, "", 1, 5);

        // 3 sub-packages use 3 slots
        assert!(result.contains("📦 lib"));
        assert!(result.contains("📦 src"));
        assert!(result.contains("📦 tools"));
        // 2 remaining slots for targets
        assert!(result.contains("🎯 :root_a"));
        assert!(result.contains("🎯 :root_b"));
        // Third target hidden
        assert!(!result.contains("🎯 :root_c"));
        assert!(result.contains("(+1 more target)"));
    }

    #[test]
    fn test_width_limits_packages() {
        let stdout = "\
//a:t1
//b:t2
//c:t3
//d:t4
//e:t5";
        let result = query(stdout, "", 1, 3);

        // Only 3 packages shown (BTreeMap order: a, b, c)
        assert!(result.contains("📦 a"));
        assert!(result.contains("📦 b"));
        assert!(result.contains("📦 c"));
        assert!(!result.contains("📦 d"));
        assert!(!result.contains("📦 e"));
        assert!(result.contains("(+2 more sub-packages)"));
    }

    #[test]
    fn test_condensed_truncation_line() {
        // Both packages and targets truncated
        let stdout = "\
//a:t
//b:t
//c:t
//d:t
//:x
//:y
//:z";
        let result = query(stdout, "", 1, 3);

        // 3 width: 3 packages shown (a, b, c), d hidden, targets use 0 slots
        // All 3 root targets hidden
        assert!(result.contains("(+1 more sub-package, 3 more targets)"));
    }

    #[test]
    fn test_condensed_truncation_omits_zero_parts() {
        // Only packages truncated, no targets
        let stdout = "\
//a:t
//b:t
//c:t
//d:t";
        let result = query(stdout, "", 1, 3);

        // 3 packages shown, 1 hidden, no root targets
        assert!(result.contains("(+1 more sub-package)"));
        assert!(!result.contains("more target"));
    }

    // === Root target tests ===

    #[test]
    fn test_root_targets_inline() {
        let stdout = "\
//:bazel-distfile
//:bazel-srcs
//src:lib";
        let result = query(stdout, "", 1, usize::MAX);

        // Root targets shown as 🎯 at top level
        assert!(result.contains("🎯 :bazel-distfile"));
        assert!(result.contains("🎯 :bazel-srcs"));
        // Sub-package shown as 📦
        assert!(result.contains("📦 src"));
    }

    // === Relative names tests ===

    #[test]
    fn test_relative_names() {
        let stdout = "\
//examples/cpp:a
//examples/go:b";
        let result = query(stdout, "", 2, usize::MAX);

        // Children show relative names (cpp, go), not full path
        assert!(result.contains("  📦 cpp"));
        assert!(result.contains("  📦 go"));
        assert!(!result.contains("examples/cpp"));
        assert!(!result.contains("examples/go"));
    }

    // === Multi-root tests ===

    // === Backward-compatible tests (ported from old tests) ===

    #[test]
    fn test_groups_targets_by_package() {
        let stdout = "\
//src/lib/math/compute:target_a
//src/lib/math/compute:target_b
//src/lib/math/compute:target_c
//tools/codegen:foo
//tools/codegen:bar";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        // With full depth, targets are at leaf nodes
        assert!(result.contains("🎯 :target_a"));
        assert!(result.contains("🎯 :target_b"));
        assert!(result.contains("🎯 :target_c"));
        assert!(result.contains("🎯 :foo"));
        assert!(result.contains("🎯 :bar"));
    }

    #[test]
    fn test_real_bazel_output() {
        let stderr = "\
(10:23:45) INFO: Invocation ID: 8e2f4a91-abc1-4def-9012-345678abcdef
(10:23:45) INFO: Current date is 2026-03-01
(10:23:46) WARNING: Build option --config=remote has changed
(10:23:46) INFO: Repository rule @bazel_tools//tools/jdk:jdk configured
(10:23:47) INFO: Found 16 targets...
(10:23:47) INFO: Elapsed time: 1.234s";
        let stdout = "\
//src/app/foo/bar:bar
//src/app/foo/bar:bar_test
//src/app/foo/bar:bar_lib
//src/app/foo/bar:config
//src/app/foo/bar:config_test
//src/app/foo/bar:utils
//src/app/foo/bar:utils_test
//src/app/foo/bar:integration_test
//src/app/foo/bar:benchmark
//src/app/foo/bar:benchmark_lib
//src/app/foo/bar:data
//src/app/foo/bar:test_data
//src/app/foo/bar:model
//src/app/foo/bar:model_test
//src/app/foo/bar:runner
//src/app/foo/bar:runner_test";

        let result = query(stdout, stderr, usize::MAX, usize::MAX);

        // Should strip all INFO/WARNING noise
        assert!(!result.contains("Invocation ID"));
        assert!(!result.contains("Elapsed time"));

        // Header with total count
        assert!(result.contains("//... (16 targets, 4 packages)"));

        // All 16 targets should be present (depth=all)
        assert!(result.contains("🎯 :bar\n"));
        assert!(result.contains("🎯 :runner_test"));
    }
}
