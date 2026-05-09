//! Filters Perforce (p4) output — describe, changes, diff, filelog — keeping just the essentials.
//!
//! P4 commands produce extremely verbose output (hundreds of affected files, full diff context).
//! This module compresses that output for LLM consumption while preserving actionable information.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

/// Supported p4 subcommands with specialized filters.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum P4Command {
    Describe,
    Changes,
    Diff,
    Diff2,
    Filelog,
    Print,
}

/// Main entry point: route p4 subcommand to appropriate filter.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough_str(&[], verbose);
    }

    // p4 global flags (appear before the subcommand) that take a value argument.
    // e.g. `p4 -C utf8 -p ssl:1666 -u user changes ...`
    let p4_global_flags_with_value: &[&str] = &[
        "-C", "-c", "-d", "-H", "-I", "-L", "-p", "-P", "-q", "-r", "-u", "-x", "-z",
    ];
    // p4 global flags that are standalone (no value).
    let p4_global_flags_standalone: &[&str] = &["-b", "-G", "-s", "-e", "-Q"];

    // Separate global flags from the subcommand + its args.
    let mut global_args: Vec<&String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if p4_global_flags_with_value.contains(&a) {
            global_args.push(&args[i]);
            if i + 1 < args.len() {
                i += 1;
                global_args.push(&args[i]);
            }
        } else if p4_global_flags_standalone.contains(&a) {
            global_args.push(&args[i]);
        } else {
            break;
        }
        i += 1;
    }

    if i >= args.len() {
        return run_passthrough_str(args, verbose);
    }

    let subcmd = args[i].as_str();
    let sub_args = &args[i + 1..];

    match subcmd {
        "describe" => run_describe(sub_args, &global_args, verbose),
        "changes" => run_changes(sub_args, &global_args, verbose),
        "diff" => run_diff(sub_args, &global_args, verbose),
        "diff2" => run_diff2(sub_args, &global_args, verbose),
        "filelog" => run_filelog(sub_args, &global_args, verbose),
        "opened" => run_opened(sub_args, &global_args, verbose),
        "files" => run_files(sub_args, &global_args, verbose),
        "fstat" => run_fstat(sub_args, &global_args, verbose),
        "annotate" => run_annotate(sub_args, &global_args, verbose),
        "edit" | "add" | "delete" | "revert" | "sync" | "submit" | "shelve" | "unshelve" | "resolve" | "move" | "lock" | "unlock" => run_action(subcmd, sub_args, &global_args, verbose),
        _ => run_passthrough_str(args, verbose),
    }
}

/// Passthrough for unsupported subcommands.
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    runner::run_passthrough("p4", args, verbose)
}

fn run_passthrough_str(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    run_passthrough(&os_args, verbose)
}

/// Build a `p4` Command with global flags already injected (before the subcommand).
fn p4_cmd_with_globals(global_args: &[&String]) -> std::process::Command {
    let mut cmd = resolved_command("p4");
    for g in global_args {
        cmd.arg(g.as_str());
    }
    cmd
}

// ─── p4 describe ───────────────────────────────────────────────────────────────

fn run_describe(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("describe").arg("-s"); // -s = omit diffs (summary only)
    for arg in args {
        // Don't double-add -s if user already passed it
        if arg != "-s" {
            cmd.arg(arg);
        }
    }

    let args_display = format!("describe -s {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_describe,
        RunOptions::stdout_only().tee("p4 describe"),
    )
}

/// Filter p4 describe output:
/// - Keep header (Change, Date, User, Description)
/// - Render file list as a collapsed directory tree (like `tree` command)
///   - Common depot prefix shown once at top
///   - Single-child directories collapsed (e.g. "Assets/Scripts/Combat/")
///   - Files shown as leaves with #rev and action
/// - Strip "Differences ..." section entirely (use p4 diff for that)
fn filter_describe(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() / 2);
    let mut in_affected_files = false;
    let mut in_differences = false;
    let mut file_paths: Vec<&str> = Vec::new();

    for line in raw.lines() {
        if line.starts_with("Affected files ...") {
            in_affected_files = true;
            in_differences = false;
            continue;
        }
        if line.starts_with("Differences ...") {
            in_affected_files = false;
            in_differences = true;
            continue;
        }

        if in_differences {
            continue;
        }

        if in_affected_files {
            let trimmed = line.trim();
            if trimmed.starts_with("... //") {
                file_paths.push(trimmed);
            }
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    if file_paths.is_empty() {
        return result;
    }

    // Find common prefix
    let common_prefix = find_common_depot_prefix(&file_paths);

    result.push_str(&format!(
        "\nAffected files ({} total):\n",
        file_paths.len()
    ));

    if !common_prefix.is_empty() {
        result.push_str(&format!("  [{}]\n", common_prefix.trim_end_matches('/')));
    }

    // Build tree and render
    let rel_paths: Vec<FileEntry> = file_paths
        .iter()
        .map(|p| parse_file_entry(p, &common_prefix))
        .collect();

    let tree = build_tree(&rel_paths);
    render_tree(&tree, "  ", &mut result);

    result
}

/// A parsed file entry: relative path segments + revision suffix (e.g. "#3 edit")
struct FileEntry {
    segments: Vec<String>, // directory components + filename
    suffix: String,        // "#3 edit" or "#5 add" etc.
}

/// Parse "... //depot/proj/dir/file.cs#3 edit" into FileEntry
fn parse_file_entry(raw_line: &str, common_prefix: &str) -> FileEntry {
    let stripped = strip_prefix_from_path(raw_line, common_prefix);
    // Split off the #rev action: "dir/file.cs#3 edit" -> path="dir/file.cs", suffix="#3 edit"
    let (path_part, suffix) = if let Some(hash_pos) = stripped.find('#') {
        (&stripped[..hash_pos], stripped[hash_pos..].to_string())
    } else {
        (stripped, String::new())
    };

    let segments: Vec<String> = path_part.split('/').map(|s| s.to_string()).collect();
    FileEntry { segments, suffix }
}

/// Tree node: either a directory (with children) or implicit via the path structure.
/// We use a simple ordered map approach.
enum TreeNode {
    Dir(Vec<(String, TreeNode)>), // name -> subtree (ordered)
    File(String),                 // suffix like "#3 edit"
}

/// Build a directory tree from file entries
fn build_tree(entries: &[FileEntry]) -> Vec<(String, TreeNode)> {
    let mut root: Vec<(String, TreeNode)> = Vec::new();
    for entry in entries {
        insert_into_tree(&mut root, &entry.segments, &entry.suffix);
    }
    root
}

fn insert_into_tree(nodes: &mut Vec<(String, TreeNode)>, segments: &[String], suffix: &str) {
    if segments.is_empty() {
        return;
    }
    if segments.len() == 1 {
        // Leaf file
        nodes.push((segments[0].clone(), TreeNode::File(suffix.to_string())));
        return;
    }

    // Find or create directory node
    let dir_name = &segments[0];
    let rest = &segments[1..];

    // Look for existing dir with this name
    let dir_pos = nodes.iter().position(|(name, node)| {
        name == dir_name && matches!(node, TreeNode::Dir(_))
    });

    if let Some(pos) = dir_pos {
        if let TreeNode::Dir(ref mut children) = nodes[pos].1 {
            insert_into_tree(children, rest, suffix);
        }
    } else {
        let mut children = Vec::new();
        insert_into_tree(&mut children, rest, suffix);
        nodes.push((dir_name.clone(), TreeNode::Dir(children)));
    }
}

/// Render tree with collapsed single-child directories
fn render_tree(nodes: &[(String, TreeNode)], indent: &str, output: &mut String) {
    let len = nodes.len();
    for (i, (name, node)) in nodes.iter().enumerate() {
        let is_last = i == len - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_indent = if is_last {
            format!("{}    ", indent)
        } else {
            format!("{}│   ", indent)
        };

        match node {
            TreeNode::File(suffix) => {
                output.push_str(&format!("{}{}{}{}\n", indent, connector, name, suffix));
            }
            TreeNode::Dir(children) => {
                // Collapse single-child directory chains:
                // If a dir has exactly 1 child and that child is also a dir, merge them
                let (collapsed_name, final_children) = collapse_single_child(name, children);

                let file_count = count_files_recursive(final_children);
                output.push_str(&format!(
                    "{}{}{} ({})\n",
                    indent, connector, collapsed_name, file_count
                ));

                // For large dirs, limit displayed children
                let max_children = 20;
                if final_children.len() <= max_children {
                    render_tree(final_children, &child_indent, output);
                } else {
                    render_tree(&final_children[..max_children], &child_indent, output);
                    let remaining = final_children.len() - max_children;
                    output.push_str(&format!(
                        "{}    ... +{} more entries\n",
                        indent, remaining
                    ));
                }
            }
        }
    }
}

/// Collapse chains of single-child directories: A/B/C/ with one child each -> "A/B/C/"
fn collapse_single_child<'a>(
    name: &'a str,
    children: &'a [(String, TreeNode)],
) -> (String, &'a [(String, TreeNode)]) {
    let mut collapsed = name.to_string();
    let mut current = children;

    loop {
        if current.len() == 1 {
            if let TreeNode::Dir(ref grandchildren) = current[0].1 {
                collapsed = format!("{}/{}", collapsed, current[0].0);
                current = grandchildren;
                continue;
            }
        }
        break;
    }

    (collapsed, current)
}

/// Count total files (leaves) under a tree node list
fn count_files_recursive(nodes: &[(String, TreeNode)]) -> usize {
    let mut count = 0;
    for (_, node) in nodes {
        match node {
            TreeNode::File(_) => count += 1,
            TreeNode::Dir(children) => count += count_files_recursive(children),
        }
    }
    count
}

/// Find common depot path prefix across all file paths.
fn find_common_depot_prefix(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }

    let clean_paths: Vec<&str> = paths
        .iter()
        .filter_map(|p| {
            let stripped = p.trim_start_matches("... ");
            stripped.split('#').next()
        })
        .collect();

    if clean_paths.is_empty() {
        return String::new();
    }

    let first_segments: Vec<&str> = clean_paths[0].split('/').collect();
    let mut common_len = first_segments.len();

    for path in &clean_paths[1..] {
        let segments: Vec<&str> = path.split('/').collect();
        let mut match_len = 0;
        for (i, seg) in segments.iter().enumerate() {
            if i >= common_len {
                break;
            }
            if i < first_segments.len() && seg == &first_segments[i] {
                match_len = i + 1;
            } else {
                break;
            }
        }
        common_len = match_len;
    }

    if common_len <= 3 {
        return String::new();
    }

    first_segments[..common_len].join("/") + "/"
}

/// Strip common prefix from a file path line.
fn strip_prefix_from_path<'a>(path: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() {
        return path;
    }
    let stripped = path.trim_start_matches("... ");
    if let Some(rest) = stripped.strip_prefix(prefix) {
        rest
    } else {
        stripped
    }
}

// ─── p4 changes ────────────────────────────────────────────────────────────────

fn run_changes(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("changes");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("changes {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_changes,
        RunOptions::stdout_only(),
    )
}

/// Filter p4 changes output:
/// - Strip verbose workspace names: "user@Long_Workspace_Name" -> "user"
/// - Keep all entries (they're already one-liners)
/// - Align into a compact table-like format
fn filter_changes(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "No changelists found.".to_string();
    }

    let mut result = String::with_capacity(raw.len());

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("Change ") {
            result.push_str(&compact_change_line(trimmed));
            result.push('\n');
        } else {
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result
}

/// Compact a single "Change NNN on DATE by user@workspace 'desc'" line:
/// - Strip "Change" prefix and workspace name
///   Output: "12345 2026/05/07 user desc..."
fn compact_change_line(line: &str) -> String {
    let parts: Vec<&str> = line.splitn(2, " on ").collect();
    if parts.len() < 2 {
        return line.to_string();
    }

    let cl_num = parts[0].trim_start_matches("Change ").trim();

    let rest = parts[1];
    let by_parts: Vec<&str> = rest.splitn(2, " by ").collect();
    if by_parts.len() < 2 {
        return line.to_string();
    }

    let date = by_parts[0].trim();
    let user_and_desc = by_parts[1];

    let (user, desc) = if let Some(space_pos) = user_and_desc.find(' ') {
        let user_full = &user_and_desc[..space_pos];
        let desc = user_and_desc[space_pos..].trim();
        let user = user_full.split('@').next().unwrap_or(user_full);
        (user, desc)
    } else {
        (user_and_desc.split('@').next().unwrap_or(user_and_desc), "")
    };

    let desc_clean = desc.trim_matches('\'').trim();

    format!("{} {} {} {}", cl_num, date, user, desc_clean)
}

// ─── p4 diff ───────────────────────────────────────────────────────────────────

fn run_diff(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("diff");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("diff {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_diff,
        RunOptions::stdout_only().tee("p4 diff"),
    )
}

fn run_diff2(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("diff2");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("diff2 {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_diff,
        RunOptions::stdout_only().tee("p4 diff2"),
    )
}

/// Filter p4 diff / diff2 output:
/// - Strip file header lines (==== ...) — redundant with command context
///   For multi-file diffs, emit just the filename as separator
/// - Keep changed lines (+ / - / @@ hunk headers)
/// - Strip excessive unchanged context
/// - Add summary at end
fn filter_diff(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return "No differences.".to_string();
    }

    let mut result = String::with_capacity(raw.len() / 3);
    let mut file_count = 0;
    let mut add_count = 0;
    let mut del_count = 0;
    let mut context_run = 0;
    let max_context = 3;

    for line in &lines {
        // File separator: "==== //depot/path/file.ext#rev - ... ===="
        if line.starts_with("====") {
            file_count += 1;
            context_run = 0;
            // For multi-file diffs, emit just filename as separator
            if file_count > 1 {
                let name = extract_filename_from_header(line);
                result.push_str(&format!("\n--- {} ---\n", name));
            }
            continue;
        }

        // Hunk headers
        if line.starts_with("@@") || line.starts_with("---") || line.starts_with("+++") {
            context_run = 0;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Added lines
        if line.starts_with('+') || line.starts_with('>') {
            add_count += 1;
            context_run = 0;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Deleted lines
        if line.starts_with('-') || line.starts_with('<') {
            del_count += 1;
            context_run = 0;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Context lines — limit consecutive unchanged lines
        context_run += 1;
        if context_run <= max_context {
            result.push_str(line);
            result.push('\n');
        } else if context_run == max_context + 1 {
            result.push_str("  ...\n");
        }
    }

    // Summary
    result.push_str(&format!(
        "\n[{} files, +{} -{} lines]\n",
        file_count, add_count, del_count
    ));

    result
}

/// Extract just the filename from a diff header line
fn extract_filename_from_header(line: &str) -> &str {
    // "==== //depot/proj/dir/File.cs#3 (text) - ... ===="
    let inner = line.trim_start_matches("====").trim();
    // Get first depot path, extract filename
    let path = inner.split_whitespace().next().unwrap_or(inner);
    // Strip #rev
    let path = path.split('#').next().unwrap_or(path);
    // Get filename
    path.rsplit('/').next().unwrap_or(path)
}

// ─── p4 filelog ────────────────────────────────────────────────────────────────

fn run_filelog(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("filelog");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("filelog {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_filelog,
        RunOptions::stdout_only(),
    )
}

/// Filter p4 filelog output:
/// - Compact revision lines: strip workspace name, tighten format
/// - Compact integration lines: collapse long depot paths into stream names
///   "... ... copy into //ABC_Project/MiHoYoSDK/Proj/.../File.cs#6"
///   -> "      copy into → MiHoYoSDK#6"
/// - Group consecutive same-action integrations into one line
/// - Keep max 20 revisions per file
fn filter_filelog(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() / 2);
    let mut rev_count = 0;
    let max_revs = 20;

    // Collect integration lines to group them
    let mut pending_integrations: Vec<(&str, String)> = Vec::new(); // (action, "stream#rev")

    let flush_integrations = |integrations: &mut Vec<(&str, String)>, out: &mut String| {
        if integrations.is_empty() {
            return;
        }
        // Group by action: "copy into", "branch from", "branch into", etc.
        let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
        for (action, target) in integrations.drain(..) {
            if let Some(grp) = groups.iter_mut().find(|(a, _)| *a == action) {
                grp.1.push(target);
            } else {
                groups.push((action, vec![target]));
            }
        }
        for (action, targets) in &groups {
            let arrow = if action.contains("from") { "←" } else { "→" };
            out.push_str(&format!(
                "      {} {} {}\n",
                action,
                arrow,
                targets.join(", ")
            ));
        }
    };

    for line in raw.lines() {
        let trimmed = line.trim();

        // File header: "//depot/path/file.ext"
        if trimmed.starts_with("//") && !trimmed.starts_with("... ") {
            flush_integrations(&mut pending_integrations, &mut result);
            rev_count = 0;
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        // Revision line: "... #3 change 12345 edit on 2026/01/15 by user@ws (text) 'desc'"
        if trimmed.starts_with("... #") {
            flush_integrations(&mut pending_integrations, &mut result);
            rev_count += 1;
            if rev_count <= max_revs {
                result.push_str(&compact_filelog_rev(trimmed));
                result.push('\n');
            } else if rev_count == max_revs + 1 {
                result.push_str("  ... (older revisions omitted)\n");
            }
            continue;
        }

        // Integration line: "... ... copy into //depot/.../file.cs#6"
        if trimmed.starts_with("... ...") {
            if rev_count > max_revs {
                continue; // Skip integration details for omitted revisions
            }
            let content = trimmed.trim_start_matches("... ...");
            let content = content.trim();
            // Parse action and path
            if let Some((action, target)) = parse_integration_line(content) {
                pending_integrations.push((action, target));
            }
            continue;
        }
    }

    flush_integrations(&mut pending_integrations, &mut result);
    result
}

/// Parse an integration line content like "copy into //ABC_Project/main/Proj/.../File.cs#6"
/// Returns (action, compact_target) like ("copy into", "main#6")
fn parse_integration_line(content: &str) -> Option<(&str, String)> {
    // content = "copy into //ABC_Project/main/Proj/.../File.cs#6"
    // or "branch from //ABC_Project/main/Proj/.../File.cs#1,#100"

    // Find the depot path (starts with //)
    let depot_start = content.find("//")?;
    let action = content[..depot_start].trim();
    let depot_path = &content[depot_start..];

    // Extract stream name: //ABC_Project/STREAM/rest... -> "STREAM"
    let segments: Vec<&str> = depot_path.split('/').collect();
    // segments: ["", "", "ABC_Project", "STREAM", "Proj", ...]
    let stream = if segments.len() > 3 {
        segments[3]
    } else {
        depot_path
    };

    // Extract revision: everything after first # in the depot path
    // Could be "#6" or "#1,#100" (revision range)
    let rev_suffix = if let Some(hash_pos) = depot_path.find('#') {
        &depot_path[hash_pos..]
    } else {
        ""
    };

    Some((action, format!("{}{}", stream, rev_suffix)))
}

/// Compact a filelog revision line:
/// Input:  "... #3 change 12345 edit on 2026/01/15 by user@ws (text+m) 'some description'"
/// Output: "  #3 edit @12345 2026/01/15 user 'some description'"
fn compact_filelog_rev(line: &str) -> String {
    let stripped = line.trim_start_matches("... ");
    let parts: Vec<&str> = stripped.split_whitespace().collect();
    if parts.len() < 8 {
        return format!("  {}", stripped);
    }

    let rev = parts[0];           // #3
    let cl = parts[2];            // 12345
    let action = parts[3];        // edit
    let date = parts[5];          // 2026/01/15
    let user_full = parts[7];     // user@ws
    let user = user_full.split('@').next().unwrap_or(user_full);

    // Collect description (everything after the (type) marker, in quotes)
    let desc = if let Some(desc_start) = stripped.find('\'') {
        &stripped[desc_start..]
    } else {
        ""
    };

    format!("  {} {} @{} {} {} {}", rev, action, cl, date, user, desc)
}

// ─── p4 opened ────────────────────────────────────────────────────────────────

fn run_opened(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("opened");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("opened {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_opened,
        RunOptions::stdout_only().tee("p4 opened"),
    )
}

/// Filter p4 opened output:
/// Each line: "//depot/path/file.ext#rev - action change CLNUM (type)"
/// Compress using tree structure (same as describe).
fn filter_opened(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return "No opened files.".to_string();
    }

    // Convert opened format to "... //path#rev action" format for reuse
    let converted: Vec<String> = lines
        .iter()
        .filter(|l| l.starts_with("//"))
        .map(|l| convert_opened_line(l))
        .collect();

    if converted.is_empty() {
        return raw.to_string();
    }

    let converted_refs: Vec<&str> = converted.iter().map(|s| s.as_str()).collect();
    let common_prefix = find_common_depot_prefix(&converted_refs);

    let mut result = String::with_capacity(raw.len() / 2);
    result.push_str(&format!("Opened files ({} total):\n", converted.len()));

    if !common_prefix.is_empty() {
        result.push_str(&format!("  [{}]\n", common_prefix.trim_end_matches('/')));
    }

    let rel_paths: Vec<FileEntry> = converted_refs
        .iter()
        .map(|p| parse_file_entry(p, &common_prefix))
        .collect();

    let tree = build_tree(&rel_paths);
    render_tree(&tree, "  ", &mut result);

    result
}

/// Convert "//depot/path/file.cs#3 - edit change 12345 (text+m)"
/// to     "... //depot/path/file.cs#3 edit @12345"
/// This format is compatible with parse_file_entry.
fn convert_opened_line(line: &str) -> String {
    // Split on " - " to get path#rev and the rest
    let parts: Vec<&str> = line.splitn(2, " - ").collect();
    if parts.len() < 2 {
        return format!("... {}", line);
    }

    let path_rev = parts[0].trim(); // "//depot/path/file.cs#3"
    let rest = parts[1].trim(); // "edit change 12345 (text+m)"

    // Extract action and CL from rest
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let action = tokens.first().copied().unwrap_or("");
    let cl = if tokens.len() >= 3 && tokens[1] == "change" {
        tokens[2]
    } else {
        ""
    };

    // Rebuild as "... //depot/path/file.cs#3 edit @12345"
    // parse_file_entry expects suffix after '#': "3 edit @12345"
    if cl.is_empty() {
        format!("... {} {}", path_rev, action)
    } else {
        format!("... {} {} @{}", path_rev, action, cl)
    }
}

// ─── p4 files ─────────────────────────────────────────────────────────────────

fn run_files(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("files");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("files {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_files,
        RunOptions::stdout_only().tee("p4 files"),
    )
}

/// Filter p4 files output — same format as opened:
/// "//depot/path/file.ext#rev - action change CLNUM (type)"
/// Compress using tree structure.
fn filter_files(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return "No files.".to_string();
    }

    let converted: Vec<String> = lines
        .iter()
        .filter(|l| l.starts_with("//"))
        .map(|l| convert_opened_line(l))
        .collect();

    if converted.is_empty() {
        return raw.to_string();
    }

    let converted_refs: Vec<&str> = converted.iter().map(|s| s.as_str()).collect();
    let common_prefix = find_common_depot_prefix(&converted_refs);

    let mut result = String::with_capacity(raw.len() / 2);
    result.push_str(&format!("{} files:\n", converted.len()));

    if !common_prefix.is_empty() {
        result.push_str(&format!("  [{}]\n", common_prefix.trim_end_matches('/')));
    }

    let rel_paths: Vec<FileEntry> = converted_refs
        .iter()
        .map(|p| parse_file_entry(p, &common_prefix))
        .collect();

    let tree = build_tree(&rel_paths);
    render_tree(&tree, "  ", &mut result);

    result
}

// ─── p4 fstat ─────────────────────────────────────────────────────────────────

fn run_fstat(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("fstat");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("fstat {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_fstat,
        RunOptions::stdout_only().tee("p4 fstat"),
    )
}

/// Filter p4 fstat output:
/// - Keep key fields: depotFile, headAction, headRev, headChange, action, change, actionOwner
/// - Collapse otherOpen into one summary line: "otherOpen: user1(action), user2(action)..."
/// - Drop noise: headType, headTime, headModTime, clientFile, workRev, haveRev, type, resolved, etc.
fn filter_fstat(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() / 3);

    // Key fields to keep (in order)
    let keep_fields = [
        "headAction",
        "headType",
        "headTime",
        "headRev",
        "headChange",
        "headModTime",
        "haveRev",
        "action",
        "change",
        "actionOwner",
    ];

    let mut other_users: Vec<(String, String)> = Vec::new(); // (user, action)
    let mut in_file_block = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            // Block separator — flush otherOpen summary
            if !other_users.is_empty() {
                let summary: Vec<String> = other_users
                    .iter()
                    .map(|(u, a)| {
                        let user = u.split('@').next().unwrap_or(u);
                        format!("{}({})", user, a)
                    })
                    .collect();
                result.push_str(&format!("... otherOpen: {}\n", summary.join(", ")));
                other_users.clear();
            }
            if in_file_block {
                result.push('\n');
            }
            in_file_block = false;
            continue;
        }

        // Parse "... fieldName value" or "... ... otherOpenN user@ws"
        if !trimmed.starts_with("...") {
            continue;
        }

        let content = trimmed.trim_start_matches("...").trim();

        // Handle otherOpen lines: "... otherOpenN user@workspace"
        if content.starts_with("... ") {
            // Nested field like "... otherOpen0 user@ws"
            let nested = content.trim_start_matches("... ").trim();
            if nested.starts_with("otherOpen") && !nested.starts_with("otherAction") && !nested.starts_with("otherChange") {
                // "otherOpen0 user@workspace" or "otherOpen 18" (count line, skip)
                let parts: Vec<&str> = nested.splitn(2, ' ').collect();
                if parts.len() == 2 && !parts[0].contains("Action") && !parts[0].contains("Change") {
                    let user = parts[1].trim();
                    // Skip the count-only line "otherOpen 18" (no '@' = not a real user)
                    if user.contains('@') {
                        other_users.push((user.to_string(), String::new()));
                    }
                }
            } else if nested.starts_with("otherAction") {
                // "otherAction0 edit"
                let parts: Vec<&str> = nested.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    if let Some(last) = other_users.last_mut() {
                        if last.1.is_empty() {
                            last.1 = parts[1].to_string();
                        }
                    }
                }
            }
            // Skip otherChange lines entirely
            continue;
        }

        // Regular field: "fieldName value"
        let parts: Vec<&str> = content.splitn(2, ' ').collect();
        let field_name = parts[0];

        if keep_fields.contains(&field_name) {
            in_file_block = true;
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    // Flush any remaining otherOpen
    if !other_users.is_empty() {
        let summary: Vec<String> = other_users
            .iter()
            .map(|(u, a)| {
                let user = u.split('@').next().unwrap_or(u);
                format!("{}({})", user, a)
            })
            .collect();
        result.push_str(&format!("... otherOpen: {}\n", summary.join(", ")));
    }

    if result.is_empty() {
        raw.to_string()
    } else {
        result
    }
}

// ─── p4 annotate ──────────────────────────────────────────────────────────────

fn run_annotate(args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg("annotate");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("annotate {}", args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_annotate,
        RunOptions::stdout_only().tee("p4 annotate"),
    )
}

/// Filter p4 annotate output:
/// - Strip the first line (file header, redundant with command args)
/// - Collapse consecutive lines with same CL+user+date prefix into groups
fn filter_annotate(raw: &str) -> String {
    let mut lines = raw.lines().peekable();

    // Skip header line: "//depot/path/file#rev - action change CL (type)"
    if let Some(first) = lines.peek() {
        if first.starts_with("//") {
            lines.next();
        }
    }

    let mut result = String::with_capacity(raw.len());
    let mut current_prefix = String::new();
    let mut group_start: usize = 1;
    let mut line_num: usize = 0;
    let mut group_lines: Vec<&str> = Vec::new();

    for line in lines {
        line_num += 1;

        // Parse prefix and content from "CL: user date content" or "CL: content"
        let (prefix, content) = parse_annotate_line(line);

        if prefix != current_prefix {
            // Flush previous group
            if !group_lines.is_empty() {
                flush_annotate_group(&current_prefix, group_start, line_num - 1, &group_lines, &mut result);
                group_lines.clear();
            }
            current_prefix = prefix;
            group_start = line_num;
        }

        group_lines.push(content);
    }

    // Flush last group
    if !group_lines.is_empty() {
        flush_annotate_group(&current_prefix, group_start, line_num, &group_lines, &mut result);
    }

    result
}

/// Parse an annotate line into (prefix, content).
/// Input formats:
///   "1499519: packer.abc 2026/03/24 using System;" -> ("1499519: packer.abc 2026/03/24", "using System;")
///   "1499519: using System;"                       -> ("1499519:", "using System;")
fn parse_annotate_line(line: &str) -> (String, &str) {
    // Find the CL number prefix "NNNNNN: "
    if let Some(colon_pos) = line.find(':') {
        let after_colon = &line[colon_pos + 1..];
        let trimmed = after_colon.trim_start();

        // Check if next part is "user date" pattern: "word YYYY/MM/DD"
        // Try to match: username YYYY/MM/DD
        let tokens: Vec<&str> = trimmed.splitn(3, ' ').collect();
        if tokens.len() >= 2 && is_date_like(tokens[1]) {
            // prefix = "CL: user date", content = rest
            let user = tokens[0];
            let date = tokens[1];
            let cl = &line[..colon_pos];
            let prefix = format!("@{} {} {}", cl.trim(), user, date);
            let content = if tokens.len() == 3 { tokens[2] } else { "" };
            return (prefix, content);
        }

        // No user/date, just "CL: content"
        let cl = &line[..colon_pos];
        let prefix = format!("@{}", cl.trim());
        return (prefix, trimmed);
    }

    // No colon found, treat whole line as content
    (String::new(), line)
}

fn is_date_like(s: &str) -> bool {
    // Match YYYY/MM/DD pattern loosely
    s.len() == 10 && s.chars().nth(4) == Some('/') && s.chars().nth(7) == Some('/')
}

fn flush_annotate_group(prefix: &str, start: usize, end: usize, lines: &[&str], output: &mut String) {
    if start == end {
        output.push_str(&format!("--- {} L{} ---\n", prefix, start));
    } else {
        output.push_str(&format!("--- {} L{}-{} ---\n", prefix, start, end));
    }
    for line in lines {
        output.push_str(line);
        output.push('\n');
    }
}

// ─── p4 action commands (edit/add/delete/revert/sync/submit/...) ─────────────

fn run_action(subcmd: &str, args: &[String], global_args: &[&String], _verbose: u8) -> Result<i32> {
    let mut cmd = p4_cmd_with_globals(global_args);
    cmd.arg(subcmd);
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = format!("{} {}", subcmd, args.join(" "));
    runner::run_filtered(
        cmd,
        "p4",
        &args_display,
        filter_action,
        RunOptions::with_tee(&format!("p4 {}", subcmd)),
    )
}

/// Filter action command output into git-style summary:
/// - Count successes and failures
/// - On success: "OK: N file(s) edited/added/reverted/synced/..."
/// - Collapse "also opened by" into summary
/// - On failure: keep error lines verbatim
fn filter_action(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "OK: (no output)\n".to_string();
    }

    let mut ok_count: usize = 0;
    let mut error_lines: Vec<&str> = Vec::new();
    let mut also_opened_users: Vec<&str> = Vec::new();
    let mut action_word: Option<&'static str> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // "also opened by" lines — collect user names
        if trimmed.contains("also opened by") {
            if let Some(user) = extract_also_opened_user(trimmed) {
                also_opened_users.push(user);
            }
            continue;
        }

        // Error/warning patterns
        if is_error_line(trimmed) {
            error_lines.push(trimmed);
            continue;
        }

        // Success line: "//path#rev - opened for edit" or "//path#rev - was edit, reverted" etc.
        if trimmed.starts_with("//") && trimmed.contains(" - ") {
            ok_count += 1;
            // Try to extract action word from the first success line
            if action_word.is_none() {
                action_word = extract_action_word(trimmed);
            }
        } else {
            // Other informational lines (e.g. submit progress "Submitting change...")
            error_lines.push(trimmed);
        }
    }

    let mut result = String::new();

    if ok_count > 0 {
        let action = action_word.unwrap_or("processed");
        result.push_str(&format!("OK: {} file(s) {}\n", ok_count, action));
    }

    if !also_opened_users.is_empty() {
        // Deduplicate
        also_opened_users.sort();
        also_opened_users.dedup();
        result.push_str(&format!("also opened by: {}\n", also_opened_users.join(", ")));
    }

    if !error_lines.is_empty() {
        if ok_count > 0 || !also_opened_users.is_empty() {
            result.push_str("---\n");
        }
        for line in &error_lines {
            result.push_str(line);
            result.push('\n');
        }
    }

    if result.is_empty() {
        raw.to_string()
    } else {
        result
    }
}

/// Extract username from "... //path - also opened by user@workspace"
fn extract_also_opened_user(line: &str) -> Option<&str> {
    if let Some(pos) = line.find("also opened by ") {
        let after = &line[pos + "also opened by ".len()..];
        // "user@workspace" — take just the user part
        let user = after.split('@').next().unwrap_or(after).trim();
        if !user.is_empty() {
            return Some(user);
        }
    }
    None
}

fn is_error_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("error") ||
    lower.contains("can't") ||
    lower.contains("cannot") ||
    lower.contains("must resolve") ||
    lower.contains("must revert") ||
    lower.contains("out of date") ||
    lower.contains("no file(s)") ||
    lower.contains("no files to submit") ||
    lower.contains("not opened") ||
    lower.contains("not in client view") ||
    lower.contains("failed") ||
    lower.contains("missing") ||
    lower.contains("abandoned") ||
    lower.contains("no permission") ||
    lower.contains("locked by") ||
    lower.contains("warning:")
}

fn extract_action_word(line: &str) -> Option<&'static str> {
    // Match patterns in the "//path#rev - DESCRIPTION" part
    if let Some(dash_pos) = line.find(" - ") {
        let desc = &line[dash_pos + 3..];
        let desc_lower = desc.to_lowercase();

        if desc_lower.contains("opened for edit") || desc_lower.contains("currently opened for edit") {
            return Some("edited");
        }
        if desc_lower.contains("opened for add") {
            return Some("added");
        }
        if desc_lower.contains("opened for delete") {
            return Some("deleted");
        }
        if desc_lower.contains("reverted") {
            return Some("reverted");
        }
        if desc_lower.contains("updating") || desc_lower.contains("added as") || desc_lower.contains("deleted as") || desc_lower.contains("refreshing") {
            return Some("synced");
        }
        if desc_lower.contains("shelved") {
            return Some("shelved");
        }
        if desc_lower.contains("unshelved") {
            return Some("unshelved");
        }
        if desc_lower.contains("moved") || desc_lower.contains("move/") {
            return Some("moved");
        }
        if desc_lower.contains("resolved") {
            return Some("resolved");
        }
        if desc_lower.contains("locked") {
            return Some("locked");
        }
        if desc_lower.contains("unlocked") {
            return Some("unlocked");
        }
        if desc_lower.contains("submitted") {
            return Some("submitted");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_describe_small() {
        let input = r#"Change 12345 by user@workspace on 2026/05/01 12:00:00

	[Fix] Some bug fix description

Affected files ...

... //depot/project/src/main.rs#5 edit
... //depot/project/src/lib.rs#3 edit
... //depot/project/tests/test1.rs#2 add
"#;
        let filtered = filter_describe(input);
        assert!(filtered.contains("Change 12345"));
        assert!(filtered.contains("3 total"));
        assert!(filtered.contains("main.rs"));
        assert!(filtered.contains("lib.rs"));
        assert!(filtered.contains("test1.rs"));
        assert!(!filtered.contains("Differences"));
    }

    #[test]
    fn test_filter_describe_strips_diffs() {
        let input = r#"Change 99 by dev@ws on 2026/01/01

	desc

Affected files ...

... //depot/a/b/c.rs#1 edit

Differences ...

==== //depot/a/b/c.rs#1 (text) ====
@@ -1,3 +1,3 @@
-old
+new
"#;
        let filtered = filter_describe(input);
        assert!(filtered.contains("Change 99"));
        assert!(filtered.contains("1 total"));
        assert!(filtered.contains("c.rs"));
        assert!(!filtered.contains("+new"));
        assert!(!filtered.contains("-old"));
    }

    #[test]
    fn test_filter_describe_tree_structure() {
        let input = r#"Change 100 by u@w on 2026/01/01

	test

Affected files ...

... //depot/proj/Assets/Scripts/Combat/Fighter.cs#2 edit
... //depot/proj/Assets/Scripts/Combat/Mage.cs#3 edit
... //depot/proj/Assets/Scripts/UI/Panel.cs#1 add
... //depot/proj/Assets/Config/data.json#4 edit
"#;
        let filtered = filter_describe(input);
        // Should have tree connectors
        assert!(filtered.contains("├──") || filtered.contains("└──"));
        // Should have all filenames
        assert!(filtered.contains("Fighter.cs"));
        assert!(filtered.contains("Mage.cs"));
        assert!(filtered.contains("Panel.cs"));
        assert!(filtered.contains("data.json"));
        // Should show collapsed dirs
        assert!(filtered.contains("Combat"));
    }

    #[test]
    fn test_filter_describe_large_truncates() {
        let mut input = String::from("Change 555 by user@ws on 2026/01/01\n\n\tdesc\n\nAffected files ...\n\n");
        for i in 0..300 {
            input.push_str(&format!(
                "... //ABC/v0.60/Proj/Assets/Dir{}/file{}.cs#{} edit\n",
                i / 10,
                i,
                i + 1
            ));
        }
        let filtered = filter_describe(&input);
        assert!(filtered.contains("300 total"));
        // Should have tree structure
        assert!(filtered.contains("├──") || filtered.contains("└──"));
        // Large dirs should be truncated with "+N more"
        assert!(filtered.contains("more entries") || filtered.len() < input.len());
    }

    #[test]
    fn test_filter_changes_compact() {
        let input = "Change 100 on 2026/01/01 by user@Very_Long_Workspace_Name 'fix something'\nChange 99 on 2025/12/31 by admin@another_ws 'feat: new thing'\n";
        let filtered = filter_changes(input);
        // Should strip workspace
        assert!(!filtered.contains("Very_Long_Workspace_Name"));
        assert!(!filtered.contains("another_ws"));
        // Should keep user
        assert!(filtered.contains("user"));
        assert!(filtered.contains("admin"));
        // Should keep CL number and desc
        assert!(filtered.contains("100"));
        assert!(filtered.contains("fix something"));
    }

    #[test]
    fn test_filter_diff_basic() {
        let input = r#"==== //depot/proj/src/file.rs#3 (text) - //depot/proj/src/file.rs#4 (text) ==== content
@@ -10,5 +10,5 @@
 context1
 context2
-old_line
+new_line
 context3
"#;
        let filtered = filter_diff(input);
        // Single-file diff: header should be stripped entirely
        assert!(!filtered.contains("===="));
        assert!(!filtered.contains("//depot"));
        assert!(filtered.contains("-old_line"));
        assert!(filtered.contains("+new_line"));
        assert!(filtered.contains("[1 files, +1 -1 lines]"));
    }

    #[test]
    fn test_filter_diff_limits_context() {
        let mut input = String::from("==== //depot/f.rs#1 ====\n");
        for i in 0..20 {
            input.push_str(&format!(" context_line_{}\n", i));
        }
        input.push_str("+added\n");

        let filtered = filter_diff(&input);
        // Should not contain all 20 context lines
        assert!(filtered.contains("..."));
        assert!(filtered.contains("+added"));
    }

    #[test]
    fn test_filter_filelog_compact() {
        let input = r#"//depot/project/src/main.rs
... #5 change 500 edit on 2026/05/01 by user@Very_Long_WS (text) 'fix bug'
... ... copy into //depot/other_stream/proj/src/main.rs#6
... ... copy into //depot/main/proj/src/main.rs#101
... #4 change 400 edit on 2026/04/01 by admin@ws2 (text) 'refactor'
... #3 change 300 branch on 2026/03/01 by dev@ws3 (text+m) 'initial'
... ... branch from //depot/main/proj/src/main.rs#1,#100
"#;
        let filtered = filter_filelog(input);
        assert!(filtered.contains("//depot/project/src/main.rs"));
        // Should compact workspace names
        assert!(!filtered.contains("Very_Long_WS"));
        assert!(!filtered.contains("ws2"));
        // Should keep users
        assert!(filtered.contains("user"));
        assert!(filtered.contains("admin"));
        // Should keep actions and CLs
        assert!(filtered.contains("#5 edit @500"));
        assert!(filtered.contains("#4 edit @400"));
        // Integration lines should be preserved but compacted
        assert!(filtered.contains("copy into"));
        assert!(filtered.contains("other_stream#6"));
        assert!(filtered.contains("main#101"));
        assert!(filtered.contains("branch from"));
        assert!(filtered.contains("main#1,#100"));
        // Should keep descriptions
        assert!(filtered.contains("'fix bug'"));
    }

    #[test]
    fn test_filter_filelog_limits_revisions() {
        let mut input = String::from("//depot/file.rs\n");
        for i in (1..=30).rev() {
            input.push_str(&format!(
                "... #{} change {} edit on 2026/01/01 by u@w (text) 'desc'\n",
                i,
                i * 100
            ));
            input.push_str(&format!(
                "... ... copy into //depot/other/file.rs#{}\n",
                i
            ));
        }
        let filtered = filter_filelog(&input);
        assert!(filtered.contains("#30 edit @3000"));
        assert!(filtered.contains("older revisions omitted"));
        // Revision #1 should be omitted (beyond 20 limit)
        assert!(!filtered.contains("@100 "));
        // But integration for visible revisions should still appear
        assert!(filtered.contains("copy into"));
    }
}
