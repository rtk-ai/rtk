//! Hermes agent: hook install/uninstall helpers.

use super::*;
use crate::core::user_dirs;
use crate::hooks::constants::{
    HERMES_DIR, HERMES_PLUGIN_INIT_FILE, HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME,
    HERMES_PLUGINS_SUBDIR,
};
use std::ffi::OsString;

// Hermes support

const HERMES_PLUGIN_INIT: &str = include_str!("../../../hooks/hermes/rtk-rewrite/__init__.py");

const HERMES_PLUGIN_YAML: &str = include_str!("../../../hooks/hermes/rtk-rewrite/plugin.yaml");

/// Sentinel value accepted by `--profile` to target all discovered profiles.
const PROFILE_ALL: &str = "all";

/// Subdirectory inside the Hermes home that holds named profiles.
const HERMES_PROFILES_SUBDIR: &str = "profiles";

pub fn run_hermes_mode(ctx: InitContext, profiles: &[String]) -> Result<()> {
    let hermes_home = resolve_hermes_home()?;
    run_hermes_mode_at(&hermes_home, ctx, profiles)
}

fn hermes_plugin_dir(hermes_home: &Path) -> PathBuf {
    hermes_home
        .join(HERMES_PLUGINS_SUBDIR)
        .join(HERMES_PLUGIN_NAME)
}

/// Return the path to a named profile's directory.
fn hermes_profile_dir(hermes_home: &Path, profile: &str) -> PathBuf {
    hermes_home.join(HERMES_PROFILES_SUBDIR).join(profile)
}

/// Return the path to a named profile's plugin directory.
fn hermes_profile_plugin_dir(hermes_home: &Path, profile: &str) -> PathBuf {
    hermes_profile_dir(hermes_home, profile)
        .join(HERMES_PLUGINS_SUBDIR)
        .join(HERMES_PLUGIN_NAME)
}

/// Write the RTK rewrite plugin files (`__init__.py` and `plugin.yaml`) into `plugin_dir`.
fn write_hermes_plugin_files(plugin_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !dry_run {
        fs::create_dir_all(plugin_dir).with_context(|| {
            format!(
                "Failed to create Hermes plugin directory: {}",
                plugin_dir.display()
            )
        })?;
    }
    let init_path = plugin_dir.join(HERMES_PLUGIN_INIT_FILE);
    let manifest_path = plugin_dir.join(HERMES_PLUGIN_MANIFEST_FILE);
    write_if_changed(&init_path, HERMES_PLUGIN_INIT, "Hermes plugin", ctx)?;
    write_if_changed(
        &manifest_path,
        HERMES_PLUGIN_YAML,
        "Hermes plugin manifest",
        ctx,
    )?;
    Ok(())
}

/// List the names of all profiles that exist under `~/.hermes/profiles/`.
/// Returns an empty vec when the directory is absent (not an error).
fn list_hermes_profiles(hermes_home: &Path) -> Result<Vec<String>> {
    let profiles_dir = hermes_home.join(HERMES_PROFILES_SUBDIR);
    if !profiles_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = fs::read_dir(&profiles_dir)
        .with_context(|| {
            format!(
                "Failed to read Hermes profiles directory: {}",
                profiles_dir.display()
            )
        })?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.file_type().ok()?.is_dir() {
                let name = entry.file_name().into_string().ok()?;
                if !name.starts_with('.') {
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    names.sort();
    Ok(names)
}

/// Normalize, trim, split, and deduplicate profile names, filtering out empty entries.
fn sanitize_profiles(profiles: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for p in profiles {
        for segment in p.split(',') {
            let trimmed = segment.trim();
            if !trimmed.is_empty() && !result.iter().any(|existing: &String| existing == trimmed) {
                result.push(trimmed.to_string());
            }
        }
    }
    result
}

/// Validate that every requested profile name exists on disk.
/// Returns a formatted error listing the missing names and available profiles.
fn validate_profiles(hermes_home: &Path, profiles: &[String]) -> Result<()> {
    let mut missing: Vec<&str> = Vec::new();
    for name in profiles {
        if name == PROFILE_ALL {
            continue;
        }
        if name.trim().is_empty() || !hermes_profile_dir(hermes_home, name).is_dir() {
            missing.push(name.as_str());
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let available = list_hermes_profiles(hermes_home).unwrap_or_default();
    let available_str = if available.is_empty() {
        "  (no profiles found under ~/.hermes/profiles/)".to_string()
    } else {
        available
            .iter()
            .map(|n| format!("  - {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let missing_str = missing.join(", ");
    anyhow::bail!(
        "Hermes profile{} not found: {missing_str}\n\
         Available profiles:\n{available_str}\n\
         Use --profile all to target every profile.",
        if missing.len() == 1 { "" } else { "s" }
    );
}

/// Patch a single Hermes config file at `config_path`.
fn patch_hermes_config_file(config_path: &Path, ctx: InitContext, label: &str) -> Result<()> {
    let existing = if config_path.exists() {
        fs::read_to_string(config_path)
            .with_context(|| format!("Failed to read {label}: {}", config_path.display()))?
    } else {
        String::new()
    };
    let patched = patch_hermes_config(&existing);
    write_if_changed(config_path, &patched, label, ctx)?;
    Ok(())
}

/// Unpatch a single Hermes config file at `config_path`.
/// Returns `true` if the file was (or would be) modified.
fn unpatch_hermes_config_file(config_path: &Path, ctx: InitContext, label: &str) -> Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let existing = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {label}: {}", config_path.display()))?;
    let patched = unpatch_hermes_config(&existing);
    if patched == existing {
        return Ok(false);
    }
    if dry_run {
        println!("[dry-run] would update {label}: {}", config_path.display());
        if verbose > 0 {
            println!("[dry-run] content:\n{}", patched);
        }
    } else {
        atomic_write(config_path, &patched)
            .with_context(|| format!("Failed to write {label}: {}", config_path.display()))?;
        if verbose > 0 {
            eprintln!("Updated {label}: {}", config_path.display());
        }
    }
    Ok(true)
}

fn run_hermes_mode_at(hermes_home: &Path, ctx: InitContext, raw_profiles: &[String]) -> Result<()> {
    let profiles = sanitize_profiles(raw_profiles);
    if !raw_profiles.is_empty() && profiles.is_empty() {
        anyhow::bail!(
            "Hermes profile name cannot be empty.\nUsage: rtk init --agent hermes --profile <name|all>"
        );
    }

    // Resolve the full set of configs to patch.
    let use_all = profiles.iter().any(|p| p == PROFILE_ALL);
    let named: Vec<&str> = profiles
        .iter()
        .filter(|p| p.as_str() != PROFILE_ALL)
        .map(String::as_str)
        .collect();

    // Validate named profiles before writing anything.
    if !named.is_empty() {
        validate_profiles(
            hermes_home,
            &named.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )?;
    }

    let InitContext { dry_run, .. } = ctx;

    let target_profile_names: Vec<String> = if use_all {
        list_hermes_profiles(hermes_home)?
    } else {
        named.iter().map(|s| s.to_string()).collect()
    };

    let no_profiles_requested = profiles.is_empty();
    let include_default = no_profiles_requested || use_all;

    // Collect plugin directories to install.
    let mut plugin_dirs: Vec<PathBuf> = Vec::new();
    if include_default {
        plugin_dirs.push(hermes_plugin_dir(hermes_home));
    }
    for profile_name in &target_profile_names {
        plugin_dirs.push(hermes_profile_plugin_dir(hermes_home, profile_name));
    }

    // Write plugin files to each plugin directory.
    for p_dir in &plugin_dirs {
        write_hermes_plugin_files(p_dir, ctx)?;
    }

    // Collect config paths to patch.
    let mut config_paths: Vec<(PathBuf, String)> = Vec::new();
    if include_default {
        let default_config = hermes_home.join("config.yaml");
        config_paths.push((default_config, "Hermes default config".to_string()));
    }
    for profile_name in &target_profile_names {
        let path = hermes_profile_dir(hermes_home, profile_name).join("config.yaml");
        config_paths.push((path, format!("Hermes profile '{profile_name}' config")));
    }

    // Patch each config.
    for (config_path, label) in &config_paths {
        patch_hermes_config_file(config_path, ctx, label)?;
    }

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nRTK configured for Hermes.\n");
        for p_dir in &plugin_dirs {
            println!("  Plugin: {}", p_dir.display());
        }
        for (config_path, label) in &config_paths {
            println!("  {label}: {}", config_path.display());
        }
        println!("  Hermes will now rewrite terminal commands through rtk.");
        if use_all {
            println!("  Restart Hermes (all profiles). Test with: git status\n");
        } else if named.len() == 1 {
            println!(
                "  Restart Hermes profile '{}'. Test with: git status\n",
                named[0]
            );
        } else if named.len() > 1 {
            let list = named
                .iter()
                .map(|n| format!("'{n}'"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  Restart Hermes profiles {list}. Test with: git status\n");
        } else {
            println!("  Restart Hermes. Test with: git status\n");
        }
    }

    Ok(())
}

pub fn uninstall_hermes(ctx: InitContext, profiles: &[String]) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let hermes_home = resolve_hermes_home()?;
    let removed = uninstall_hermes_at(&hermes_home, ctx, profiles)?;

    if removed.is_empty() {
        println!("RTK Hermes support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Hermes CLI:"
        } else {
            "RTK uninstalled for Hermes CLI:"
        };
        println!("{}", header);
        for item in removed {
            println!("  - {}", item);
        }
    }

    if dry_run {
        print_dry_run_footer();
    }

    Ok(())
}

fn uninstall_hermes_at(
    hermes_home: &Path,
    ctx: InitContext,
    raw_profiles: &[String],
) -> Result<Vec<String>> {
    let profiles = sanitize_profiles(raw_profiles);
    if !raw_profiles.is_empty() && profiles.is_empty() {
        anyhow::bail!(
            "Hermes profile name cannot be empty.\nUsage: rtk init --uninstall --agent hermes --profile <name|all>"
        );
    }

    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    // Determine which configs to unpatch.
    let use_all = profiles.iter().any(|p| p == PROFILE_ALL);
    let named: Vec<&str> = profiles
        .iter()
        .filter(|p| p.as_str() != PROFILE_ALL)
        .map(String::as_str)
        .collect();

    if !named.is_empty() {
        validate_profiles(
            hermes_home,
            &named.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )?;
    }

    let no_profiles_requested = profiles.is_empty();
    let include_default = no_profiles_requested || use_all;

    let target_profile_names: Vec<String> = if use_all {
        list_hermes_profiles(hermes_home)?
    } else {
        named.iter().map(|s| s.to_string()).collect()
    };

    let mut plugin_targets: Vec<(PathBuf, String)> = Vec::new();
    if include_default {
        plugin_targets.push((hermes_plugin_dir(hermes_home), "Hermes plugin".to_string()));
    }
    for profile_name in &target_profile_names {
        plugin_targets.push((
            hermes_profile_plugin_dir(hermes_home, profile_name),
            format!("Hermes profile '{profile_name}' plugin"),
        ));
    }

    for (p_dir, label) in plugin_targets {
        if p_dir.exists() {
            if dry_run {
                println!(
                    "[dry-run] would remove {label} directory: {}",
                    p_dir.display()
                );
            } else {
                // nosemgrep: filesystem-deletion -- uninstall intentionally removes only RTK's Hermes plugin directory.
                fs::remove_dir_all(&p_dir).with_context(|| {
                    format!("Failed to remove {label} directory: {}", p_dir.display())
                })?;
                if verbose > 0 {
                    eprintln!("Removed {label} directory: {}", p_dir.display());
                }
            }
            removed.push(format!("{label}: {}", p_dir.display()));
        }
    }

    let mut config_targets: Vec<(PathBuf, String)> = Vec::new();
    if include_default {
        config_targets.push((
            hermes_home.join("config.yaml"),
            "Hermes default config".to_string(),
        ));
    }
    for profile_name in &target_profile_names {
        let path = hermes_profile_dir(hermes_home, profile_name).join("config.yaml");
        config_targets.push((path, format!("Hermes profile '{profile_name}' config")));
    }

    for (config_path, label) in &config_targets {
        if unpatch_hermes_config_file(config_path, ctx, label)? {
            removed.push(format!("{label}: removed RTK plugin entry"));
        }
    }

    Ok(removed)
}

fn patch_hermes_config(existing: &str) -> String {
    rewrite_hermes_config(existing, true)
}

fn unpatch_hermes_config(existing: &str) -> String {
    rewrite_hermes_config(existing, false)
}

fn rewrite_hermes_config(existing: &str, add_rtk: bool) -> String {
    if existing.trim().is_empty() {
        return if add_rtk {
            hermes_plugins_block()
        } else {
            String::new()
        };
    }

    let mut lines = split_yaml_lines(existing);
    let Some(plugins_idx) = find_yaml_key_line(&lines, "plugins", 0, None) else {
        return if add_rtk {
            append_hermes_plugins_block(existing)
        } else {
            existing.to_string()
        };
    };

    let plugins_indent = yaml_indent(&lines[plugins_idx]);
    let plugins_end = yaml_block_end(&lines, plugins_idx, plugins_indent);
    let Some(enabled_idx) = find_yaml_key_line(
        &lines,
        "enabled",
        plugins_idx + 1,
        Some((plugins_end, plugins_indent)),
    ) else {
        if add_rtk {
            let (enabled_indent, item_indent) =
                hermes_missing_enabled_indents(&lines, plugins_idx, plugins_end, plugins_indent);
            let enabled_block = format!(
                "{}enabled:\n{}- {}\n",
                " ".repeat(enabled_indent),
                " ".repeat(item_indent),
                HERMES_PLUGIN_NAME
            );
            ensure_previous_yaml_line_ends_with_newline(&mut lines, plugins_end);
            lines.insert(plugins_end, enabled_block);
        }
        return lines.concat();
    };

    if yaml_line_without_ending(&lines[enabled_idx]).contains('[') {
        rewrite_inline_hermes_enabled(&mut lines, enabled_idx, add_rtk);
        return lines.concat();
    }

    rewrite_block_hermes_enabled(&mut lines, enabled_idx, add_rtk);
    lines.concat()
}

fn split_yaml_lines(input: &str) -> Vec<String> {
    if input.is_empty() {
        Vec::new()
    } else {
        input.split_inclusive('\n').map(str::to_string).collect()
    }
}

fn ensure_previous_yaml_line_ends_with_newline(lines: &mut [String], insert_idx: usize) {
    if insert_idx == 0 {
        return;
    }

    if let Some(previous) = lines.get_mut(insert_idx - 1)
        && !previous.ends_with('\n')
    {
        previous.push('\n');
    }
}

fn hermes_plugins_block() -> String {
    format!("plugins:\n  enabled:\n    - {}\n", HERMES_PLUGIN_NAME)
}

fn append_hermes_plugins_block(existing: &str) -> String {
    let mut patched = existing.to_string();
    if !patched.ends_with('\n') {
        patched.push('\n');
    }
    patched.push_str(&hermes_plugins_block());
    patched
}

fn find_yaml_key_line(
    lines: &[String],
    key: &str,
    start: usize,
    block: Option<(usize, usize)>,
) -> Option<usize> {
    let end = block.map_or(lines.len(), |(end, _)| end);
    let min_indent = block.map(|(_, indent)| indent);

    lines[start..end]
        .iter()
        .enumerate()
        .find_map(|(offset, line)| {
            let raw = yaml_line_without_ending(line);
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }

            if min_indent.is_some_and(|indent| yaml_indent(line) <= indent) {
                return None;
            }

            let is_key = trimmed == format!("{key}:") || trimmed.starts_with(&format!("{key}:"));
            is_key.then_some(start + offset)
        })
}

fn yaml_block_end(lines: &[String], start: usize, parent_indent: usize) -> usize {
    lines[start + 1..]
        .iter()
        .enumerate()
        .find_map(|(offset, line)| {
            let raw = yaml_line_without_ending(line);
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }

            (yaml_indent(line) <= parent_indent).then_some(start + 1 + offset)
        })
        .unwrap_or(lines.len())
}

fn rewrite_inline_hermes_enabled(lines: &mut [String], enabled_idx: usize, add_rtk: bool) {
    let line_ending = yaml_line_ending(&lines[enabled_idx]);
    let raw = yaml_line_without_ending(&lines[enabled_idx]);
    let Some((prefix, rest)) = raw.split_once('[') else {
        return;
    };
    let Some((items_raw, suffix)) = rest.rsplit_once(']') else {
        return;
    };

    let mut items = Vec::new();
    let mut saw_rtk = false;
    for item in items_raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_hermes_plugin_name(trimmed) {
            if add_rtk && !saw_rtk {
                items.push(trimmed.to_string());
                saw_rtk = true;
            }
        } else {
            items.push(trimmed.to_string());
        }
    }

    if add_rtk && !saw_rtk {
        items.push(HERMES_PLUGIN_NAME.to_string());
    }

    let replacement = if items.is_empty() {
        format!("{}[]{}{}", prefix, suffix, line_ending)
    } else {
        format!("{}[{}]{}{}", prefix, items.join(", "), suffix, line_ending)
    };
    lines[enabled_idx] = replacement;
}

fn rewrite_block_hermes_enabled(lines: &mut Vec<String>, enabled_idx: usize, add_rtk: bool) {
    let enabled_end = hermes_enabled_list_end(lines, enabled_idx);
    let item_indent = hermes_enabled_list_item_indent(lines, enabled_idx, enabled_end);
    let mut kept = Vec::with_capacity(lines.len() + 1);
    let mut saw_rtk = false;

    for line in &lines[enabled_idx + 1..enabled_end] {
        if is_yaml_list_item_named(line, HERMES_PLUGIN_NAME) {
            if add_rtk && !saw_rtk {
                kept.push(line.clone());
                saw_rtk = true;
            }
            continue;
        }

        kept.push(line.clone());
    }

    if add_rtk && !saw_rtk {
        let insert_idx = kept.len();
        ensure_previous_yaml_line_ends_with_newline(&mut kept, insert_idx);
        kept.push(format!(
            "{}- {}\n",
            " ".repeat(item_indent),
            HERMES_PLUGIN_NAME
        ));
    }

    let mut enabled_line = if add_rtk || kept.iter().any(|line| is_yaml_list_item_line(line)) {
        lines[enabled_idx].clone()
    } else {
        collapse_yaml_list_key_to_empty(&lines[enabled_idx])
    };

    if add_rtk
        && kept
            .iter()
            .any(|line| is_yaml_list_item_named(line, HERMES_PLUGIN_NAME))
        && !enabled_line.ends_with('\n')
    {
        enabled_line.push('\n');
    }

    let mut patched = Vec::with_capacity(lines.len() + 1);
    patched.extend_from_slice(&lines[..enabled_idx]);
    patched.push(enabled_line);
    patched.extend(kept);
    patched.extend_from_slice(&lines[enabled_end..]);
    *lines = patched;
}

fn hermes_enabled_list_end(lines: &[String], enabled_idx: usize) -> usize {
    let enabled_indent = yaml_indent(&lines[enabled_idx]);

    lines[enabled_idx + 1..]
        .iter()
        .enumerate()
        .find_map(|(offset, line)| {
            let raw = yaml_line_without_ending(line);
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }

            let indent = yaml_indent(line);
            if indent < enabled_indent
                || (indent == enabled_indent && !is_yaml_list_item_line(line))
            {
                return Some(enabled_idx + 1 + offset);
            }

            None
        })
        .unwrap_or(lines.len())
}

fn hermes_enabled_list_item_indent(
    lines: &[String],
    enabled_idx: usize,
    enabled_end: usize,
) -> usize {
    lines[enabled_idx + 1..enabled_end]
        .iter()
        .find(|line| is_yaml_list_item_line(line))
        .map(|line| yaml_indent(line))
        .unwrap_or_else(|| yaml_indent(&lines[enabled_idx]) + 2)
}

fn hermes_missing_enabled_indents(
    lines: &[String],
    plugins_idx: usize,
    plugins_end: usize,
    plugins_indent: usize,
) -> (usize, usize) {
    let child_indent = lines[plugins_idx + 1..plugins_end]
        .iter()
        .filter_map(|line| {
            let raw = yaml_line_without_ending(line);
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }

            let indent = yaml_indent(line);
            (indent > plugins_indent).then_some(indent)
        })
        .min()
        .unwrap_or(plugins_indent + 2);

    let uses_indentationless_sequences = lines[plugins_idx + 1..plugins_end]
        .iter()
        .any(|line| is_yaml_list_item_line(line) && yaml_indent(line) == child_indent);

    let item_indent = if uses_indentationless_sequences {
        child_indent
    } else {
        child_indent + 2
    };

    (child_indent, item_indent)
}

fn yaml_line_without_ending(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn yaml_line_ending(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn yaml_indent(line: &str) -> usize {
    yaml_line_without_ending(line)
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .count()
}

fn is_yaml_list_item_named(line: &str, expected: &str) -> bool {
    let trimmed = yaml_line_without_ending(line).trim();
    let Some(item) = trimmed.strip_prefix("- ") else {
        return false;
    };

    normalized_yaml_scalar(item).is_some_and(|item| item == expected)
}

fn is_yaml_list_item_line(line: &str) -> bool {
    yaml_line_without_ending(line).trim().starts_with("- ")
}

fn is_hermes_plugin_name(value: &str) -> bool {
    normalized_yaml_scalar(value).is_some_and(|item| item == HERMES_PLUGIN_NAME)
}

fn collapse_yaml_list_key_to_empty(line: &str) -> String {
    let raw = yaml_line_without_ending(line);
    let indent = yaml_indent(line);
    let Some((key, suffix)) = raw.split_once(':') else {
        return format!("{}enabled: []\n", " ".repeat(indent));
    };

    let comment = suffix
        .find('#')
        .map(|idx| format!(" {}", suffix[idx..].trim_start()))
        .unwrap_or_default();

    format!("{}: []{}\n", key, comment)
}

fn normalized_yaml_scalar(value: &str) -> Option<String> {
    let without_comment = value.split_once('#').map_or(value, |(item, _)| item);
    let trimmed = without_comment.trim().trim_matches(['\'', '"']);
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn resolve_hermes_home() -> Result<PathBuf> {
    resolve_hermes_home_from_env(user_dirs::home(), user_dirs::env_path("HERMES_HOME"))
}

fn resolve_hermes_home_from_env(
    home_dir: Option<PathBuf>,
    hermes_home: Option<OsString>,
) -> Result<PathBuf> {
    resolve_config_dir(
        hermes_home,
        home_dir,
        HERMES_DIR,
        "Cannot determine Hermes home directory. Set $HERMES_HOME or $HOME.",
    )
}

/// Returns true if Hermes RTK plugin is installed or enabled in config.yaml.
pub fn is_configured() -> bool {
    let Ok(hermes_home) = resolve_hermes_home() else {
        return false;
    };
    let plugin_dir = hermes_plugin_dir(&hermes_home);
    if plugin_dir.join(HERMES_PLUGIN_INIT_FILE).is_file() {
        return true;
    }
    let config_path = hermes_home.join("config.yaml");
    if let Ok(content) = fs::read_to_string(&config_path)
        && content
            .lines()
            .any(|l| is_yaml_list_item_named(l, HERMES_PLUGIN_NAME))
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_hermes_mode_creates_plugin_files() {
        let temp = TempDir::new().unwrap();
        run_hermes_mode_at(temp.path(), InitContext::default(), &[]).unwrap();

        let plugin_dir = temp.path().join("plugins/rtk-rewrite");
        let init_path = plugin_dir.join("__init__.py");
        let manifest_path = plugin_dir.join("plugin.yaml");
        let config_path = temp.path().join("config.yaml");

        assert!(init_path.exists(), "Python plugin should be created");
        assert!(manifest_path.exists(), "Plugin manifest should be created");
        assert_eq!(
            fs::read_to_string(&init_path).unwrap(),
            include_str!("../../../hooks/hermes/rtk-rewrite/__init__.py")
        );
        assert_eq!(
            fs::read_to_string(&manifest_path).unwrap(),
            include_str!("../../../hooks/hermes/rtk-rewrite/plugin.yaml")
        );

        let config = fs::read_to_string(&config_path).unwrap();
        assert!(config.contains("plugins:\n"));
        assert!(config.contains("  enabled:\n"));
        assert_eq!(config.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_mode_preserves_config_and_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.yaml");
        fs::write(
            &config_path,
            "theme: dark\nplugins:\n  enabled:\n    - existing-plugin\n  search_path: ./plugins\nother: true\n",
        )
        .unwrap();

        run_hermes_mode_at(temp.path(), InitContext::default(), &[]).unwrap();
        let first = fs::read_to_string(&config_path).unwrap();
        run_hermes_mode_at(temp.path(), InitContext::default(), &[]).unwrap();
        let second = fs::read_to_string(&config_path).unwrap();

        assert_eq!(first, second, "Hermes config patch should be idempotent");
        assert!(first.contains("theme: dark\n"));
        assert!(first.contains("    - existing-plugin\n"));
        assert!(first.contains("  search_path: ./plugins\n"));
        assert!(first.contains("other: true\n"));
        assert_eq!(first.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_mode_preserves_pyyaml_same_indent_config_and_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.yaml");
        fs::write(
            &config_path,
            "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n search_path: ./plugins\nother: true\n",
        )
        .unwrap();

        run_hermes_mode_at(temp.path(), InitContext::default(), &[]).unwrap();
        let first = fs::read_to_string(&config_path).unwrap();
        run_hermes_mode_at(temp.path(), InitContext::default(), &[]).unwrap();
        let second = fs::read_to_string(&config_path).unwrap();

        let expected = "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n - rtk-rewrite\n search_path: ./plugins\nother: true\n";
        assert_eq!(first, expected);
        assert_eq!(
            second, expected,
            "Hermes PyYAML config patch should be idempotent"
        );
        assert_eq!(first.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_mode_patches_and_uninstalls_pyyaml_same_indent_missing_enabled_idempotently() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();
        let plugin_dir = hermes_home.join("plugins").join(HERMES_PLUGIN_NAME);
        let other_plugin_dir = hermes_home.join("plugins/keep-me");
        let other_plugin_file = other_plugin_dir.join("plugin.yaml");
        let config_path = hermes_home.join("config.yaml");

        fs::create_dir_all(&other_plugin_dir).unwrap();
        fs::write(&other_plugin_file, "keep").unwrap();
        fs::write(
            &config_path,
            "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n search_path: ./plugins\nother: true\n",
        )
        .unwrap();

        run_hermes_mode_at(hermes_home, InitContext::default(), &[]).unwrap();
        let first = fs::read_to_string(&config_path).unwrap();
        run_hermes_mode_at(hermes_home, InitContext::default(), &[]).unwrap();
        let second = fs::read_to_string(&config_path).unwrap();

        let installed = "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n search_path: ./plugins\n enabled:\n - rtk-rewrite\nother: true\n";
        assert_eq!(first, installed);
        assert_eq!(second, installed);
        assert_eq!(first.matches("rtk-rewrite").count(), 1);
        assert!(plugin_dir.exists());
        assert_eq!(fs::read_to_string(&other_plugin_file).unwrap(), "keep");

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!plugin_dir.exists());
        assert!(other_plugin_dir.exists());
        assert_eq!(fs::read_to_string(&other_plugin_file).unwrap(), "keep");

        let uninstalled = fs::read_to_string(&config_path).unwrap();
        assert_eq!(
            uninstalled,
            "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n search_path: ./plugins\n enabled: []\nother: true\n"
        );
        assert!(!uninstalled.contains("\n - \n"));
        assert!(!uninstalled.contains("\n -\n"));
        assert_eq!(uninstalled.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_uninstall_hermes_at_removes_plugin_dir_and_cleans_config() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();
        let plugin_dir = hermes_home.join("plugins").join(HERMES_PLUGIN_NAME);
        let nested_plugin_file = plugin_dir.join("nested/marker.txt");
        let other_plugin_dir = hermes_home.join("plugins/keep-me");
        let other_plugin_file = other_plugin_dir.join("plugin.yaml");
        let config_path = hermes_home.join("config.yaml");

        fs::create_dir_all(nested_plugin_file.parent().unwrap()).unwrap();
        fs::write(&nested_plugin_file, "rtk").unwrap();
        fs::create_dir_all(&other_plugin_dir).unwrap();
        fs::write(&other_plugin_file, "keep").unwrap();
        fs::write(
            &config_path,
            "theme: dark\nplugins:\n  enabled:\n    - existing-plugin\n    - rtk-rewrite\n  search_path: ./plugins\nother: true\n",
        )
        .unwrap();

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!plugin_dir.exists());
        assert!(other_plugin_dir.exists());
        assert_eq!(fs::read_to_string(&other_plugin_file).unwrap(), "keep");

        let config = fs::read_to_string(&config_path).unwrap();
        assert!(config.contains("theme: dark\n"));
        assert!(config.contains("    - existing-plugin\n"));
        assert!(config.contains("  search_path: ./plugins\n"));
        assert!(config.contains("other: true\n"));
        assert_eq!(config.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_uninstall_hermes_at_cleans_pyyaml_same_indent_config_idempotently() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();
        let plugin_dir = hermes_home.join("plugins").join(HERMES_PLUGIN_NAME);
        let nested_plugin_file = plugin_dir.join("nested/marker.txt");
        let other_plugin_dir = hermes_home.join("plugins/keep-me");
        let other_plugin_file = other_plugin_dir.join("plugin.yaml");
        let config_path = hermes_home.join("config.yaml");

        fs::create_dir_all(nested_plugin_file.parent().unwrap()).unwrap();
        fs::write(&nested_plugin_file, "rtk").unwrap();
        fs::create_dir_all(&other_plugin_dir).unwrap();
        fs::write(&other_plugin_file, "keep").unwrap();
        fs::write(
            &config_path,
            "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n - rtk-rewrite\n search_path: ./plugins\nother: true\n",
        )
        .unwrap();

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!plugin_dir.exists());
        assert!(other_plugin_dir.exists());
        assert_eq!(fs::read_to_string(&other_plugin_file).unwrap(), "keep");

        let config = fs::read_to_string(&config_path).unwrap();
        assert_eq!(
            config,
            "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n search_path: ./plugins\nother: true\n"
        );
        assert!(!config.contains("\n - \n"));
        assert!(!config.contains("\n -\n"));
        assert_eq!(config.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_uninstall_hermes_at_missing_files_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default(), &[]).unwrap();

        assert!(removed_first.is_empty());
        assert!(removed_second.is_empty());
        assert!(!hermes_home.join("plugins").exists());
        assert!(!hermes_home.join("config.yaml").exists());
    }

    #[test]
    fn test_hermes_config_patch_adds_missing_enabled_list() {
        let existing = "theme: dark\nplugins:\n  search_path: ./plugins\nother: true\n";
        let patched = patch_hermes_config(existing);

        assert!(patched.contains("theme: dark\n"));
        assert!(patched.contains("plugins:\n"));
        assert!(patched.contains("  search_path: ./plugins\n"));
        assert!(patched.contains("  enabled:\n    - rtk-rewrite\n"));
        assert!(patched.contains("other: true\n"));
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_removes_duplicate_rtk_rewrite() {
        let existing = "plugins:\n  enabled:\n    - rtk-rewrite\n    - other\n    - rtk-rewrite\n";
        let patched = patch_hermes_config(existing);

        assert!(patched.contains("    - other\n"));
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_pyyaml_indentationless_enabled_list() {
        let existing =
            "plugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n";

        let patched = patch_hermes_config(existing);

        assert_eq!(
            patched,
            "plugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n - rtk-rewrite\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_pyyaml_default_compact_enabled_list() {
        let existing = "plugins:\n  enabled:\n  - foo\n";

        let patched = patch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n  enabled:\n  - foo\n  - rtk-rewrite\n");
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_pyyaml_indentationless_missing_enabled_list() {
        let existing =
            "plugins:\n disabled:\n - google_meet\n - spotify\n search_path: ./plugins\n";

        let patched = patch_hermes_config(existing);

        assert_eq!(
            patched,
            "plugins:\n disabled:\n - google_meet\n - spotify\n search_path: ./plugins\n enabled:\n - rtk-rewrite\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_pyyaml_indentationless_enabled_is_idempotent() {
        let existing = "plugins:\n enabled:\n - disk-cleanup\n disabled:\n - spotify\n";

        let patched_once = patch_hermes_config(existing);
        let patched_twice = patch_hermes_config(&patched_once);

        assert_eq!(
            patched_once,
            "plugins:\n enabled:\n - disk-cleanup\n - rtk-rewrite\n disabled:\n - spotify\n"
        );
        assert_eq!(patched_twice, patched_once);
        assert_eq!(patched_once.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_pyyaml_indentationless_final_line_without_newline() {
        let existing = "plugins:\n enabled:\n - disk-cleanup";

        let patched = patch_hermes_config(existing);

        assert_eq!(
            patched,
            "plugins:\n enabled:\n - disk-cleanup\n - rtk-rewrite\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_block_enabled_final_line_without_newline() {
        let existing = "plugins:\n  enabled:\n    - existing-plugin";

        let patched = patch_hermes_config(existing);

        assert_eq!(
            patched,
            "plugins:\n  enabled:\n    - existing-plugin\n    - rtk-rewrite\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_missing_enabled_after_final_child_without_newline() {
        let existing = "plugins:\n  search_path: ./plugins";

        let patched = patch_hermes_config(existing);

        assert_eq!(
            patched,
            "plugins:\n  search_path: ./plugins\n  enabled:\n    - rtk-rewrite\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_empty_enabled_final_line_without_newline() {
        let existing = "plugins:\n  enabled:";

        let patched = patch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n  enabled:\n    - rtk-rewrite\n");
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_inline_enabled_is_idempotent() {
        let existing = "theme: dark\nplugins:\n  enabled: [existing-plugin, rtk-rewrite] # keep\n  search_path: ./plugins\nother: true\n";

        let patched = patch_hermes_config(existing);

        assert_eq!(patched, existing);
        assert_eq!(patch_hermes_config(&patched), patched);
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_patch_inline_enabled_without_final_newline_is_idempotent() {
        let existing = "plugins:\n  enabled: [existing-plugin, rtk-rewrite]";

        let patched = patch_hermes_config(existing);

        assert_eq!(patched, existing);
        assert_eq!(patch_hermes_config(&patched), patched);
        assert_eq!(patched.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_hermes_config_unpatch_inline_enabled_without_rtk_preserves_missing_final_newline() {
        let existing = "plugins:\n  enabled: [existing-plugin]";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, existing);
    }

    #[test]
    fn test_hermes_config_unpatch_inline_enabled_preserves_unrelated_entries() {
        let existing = "theme: dark\nplugins:\n  enabled: [alpha, rtk-rewrite, beta] # keep comment\n  search_path: ./plugins\nother: true\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(
            patched,
            "theme: dark\nplugins:\n  enabled: [alpha, beta] # keep comment\n  search_path: ./plugins\nother: true\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_inline_enabled_final_line_without_newline() {
        let existing = "plugins:\n  enabled: [existing-plugin, rtk-rewrite]";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n  enabled: [existing-plugin]");
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_removes_duplicate_inline_rtk_rewrite() {
        let existing = "plugins:\n  enabled: [alpha, rtk-rewrite, beta, rtk-rewrite]\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n  enabled: [alpha, beta]\n");
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_removes_duplicate_block_rtk_rewrite() {
        let existing = "plugins:\n  enabled:\n    - rtk-rewrite\n    - other\n    - rtk-rewrite\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n  enabled:\n    - other\n");
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_pyyaml_indentationless_enabled_list() {
        let existing = "plugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n - rtk-rewrite\n search_path: ./plugins\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(
            patched,
            "plugins:\n disabled:\n - google_meet\n - spotify\n enabled:\n - disk-cleanup\n search_path: ./plugins\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_pyyaml_indentationless_only_rtk_collapses_to_empty() {
        let existing = "plugins:\n enabled:\n - rtk-rewrite\n search_path: ./plugins\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n enabled: []\n search_path: ./plugins\n");
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_block_enabled_final_line_without_newline() {
        let existing = "plugins:\n  enabled:\n    - existing-plugin\n    - rtk-rewrite";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n  enabled:\n    - existing-plugin\n");
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_block_enabled_without_rtk_preserves_missing_final_newline() {
        let existing = "plugins:\n  enabled:\n    - existing-plugin";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, existing);
    }

    #[test]
    fn test_hermes_config_unpatch_preserves_quoted_exact_values() {
        let existing = "plugins:\n  enabled:\n    - 'alpha'\n    - \"rtk-rewrite\"\n    - 'beta'\n  search_path: ./plugins\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(
            patched,
            "plugins:\n  enabled:\n    - 'alpha'\n    - 'beta'\n  search_path: ./plugins\n"
        );
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_hermes_config_unpatch_leaves_missing_enabled_list_unchanged() {
        let existing = "theme: dark\nplugins:\n  search_path: ./plugins\nother: true\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, existing);
    }

    #[test]
    fn test_hermes_config_unpatch_collapses_empty_enabled_list() {
        let existing = "plugins:\n  enabled:\n    - rtk-rewrite\n";

        let patched = unpatch_hermes_config(existing);

        assert_eq!(patched, "plugins:\n  enabled: []\n");
        assert_eq!(patched.matches("rtk-rewrite").count(), 0);
    }

    #[test]
    fn test_resolve_hermes_home_prefers_hermes_home() {
        let hermes_home = OsString::from("~/custom hermes home");
        let home_dir = PathBuf::from("/tmp/home");

        let resolved =
            resolve_hermes_home_from_env(Some(home_dir), Some(hermes_home.clone())).unwrap();

        assert_eq!(resolved, PathBuf::from(hermes_home));
    }

    #[test]
    fn test_resolve_hermes_home_empty_env_falls_back_to_home() {
        let home_dir = PathBuf::from("/tmp/home");

        let empty_falls_back =
            resolve_hermes_home_from_env(Some(home_dir.clone()), Some(OsString::new())).unwrap();
        let missing_falls_back =
            resolve_hermes_home_from_env(Some(home_dir.clone()), None).unwrap();

        assert_eq!(empty_falls_back, home_dir.join(".hermes"));
        assert_eq!(missing_falls_back, home_dir.join(".hermes"));
    }

    #[test]
    fn test_hermes_mode_named_profile_patches_only_profile_config() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        let profile_dir = hermes_home.join("profiles").join("work");
        fs::create_dir_all(&profile_dir).unwrap();
        let profile_config = profile_dir.join("config.yaml");
        fs::write(&profile_config, "model: gpt-4\n").unwrap();

        let profiles = vec!["work".to_string()];
        run_hermes_mode_at(hermes_home, InitContext::default(), &profiles).unwrap();

        assert!(!hermes_home.join("config.yaml").exists());

        let content = fs::read_to_string(&profile_config).unwrap();
        assert!(content.contains("rtk-rewrite"));

        // Profile's own plugin dir should have the plugin files
        assert!(
            hermes_profile_plugin_dir(hermes_home, "work")
                .join(HERMES_PLUGIN_INIT_FILE)
                .exists()
        );
    }

    #[test]
    fn test_hermes_mode_sanitizes_commas_and_whitespace() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        let p1 = hermes_home.join("profiles").join("reviewer");
        let p2 = hermes_home.join("profiles").join("writer");
        fs::create_dir_all(&p1).unwrap();
        fs::create_dir_all(&p2).unwrap();

        // Simulate "reviewer," and " writer" (with trailing comma and spaces)
        let raw_profiles = vec![
            "reviewer,".to_string(),
            " writer".to_string(),
            "".to_string(),
        ];
        run_hermes_mode_at(hermes_home, InitContext::default(), &raw_profiles).unwrap();

        // Must NOT create a config.yaml directly in ~/.hermes/profiles/
        assert!(!hermes_home.join("profiles").join("config.yaml").exists());

        // Both intended profiles must have the plugin and config
        assert!(hermes_profile_plugin_dir(hermes_home, "reviewer").exists());
        assert!(hermes_profile_plugin_dir(hermes_home, "writer").exists());
        assert!(p1.join("config.yaml").exists());
        assert!(p2.join("config.yaml").exists());
    }

    #[test]
    fn test_hermes_mode_rejects_empty_profile_name() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        let raw_profiles = vec!["".to_string(), "   ".to_string(), ",".to_string()];
        let err =
            run_hermes_mode_at(hermes_home, InitContext::default(), &raw_profiles).unwrap_err();
        assert!(
            err.to_string()
                .contains("Hermes profile name cannot be empty")
        );
    }

    #[test]
    fn test_hermes_mode_missing_profile_errors_with_available_list() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        fs::create_dir_all(hermes_home.join("profiles").join("alpha")).unwrap();
        fs::create_dir_all(hermes_home.join("profiles").join("beta")).unwrap();

        let profiles = vec!["nonexistent".to_string()];
        let err = run_hermes_mode_at(hermes_home, InitContext::default(), &profiles).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Hermes profile not found: nonexistent"));
        assert!(msg.contains("alpha"));
        assert!(msg.contains("beta"));
        assert!(msg.contains("--profile all"));
    }

    #[test]
    fn test_hermes_mode_all_profiles_patches_default_and_every_named_profile() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        let p1 = hermes_home.join("profiles").join("dev");
        let p2 = hermes_home.join("profiles").join("prod");
        fs::create_dir_all(&p1).unwrap();
        fs::create_dir_all(&p2).unwrap();
        fs::write(p1.join("config.yaml"), "env: dev\n").unwrap();
        fs::write(p2.join("config.yaml"), "env: prod\n").unwrap();
        fs::write(hermes_home.join("config.yaml"), "env: default\n").unwrap();

        let profiles = vec!["all".to_string()];
        run_hermes_mode_at(hermes_home, InitContext::default(), &profiles).unwrap();

        assert!(
            fs::read_to_string(hermes_home.join("config.yaml"))
                .unwrap()
                .contains("rtk-rewrite")
        );
        assert!(
            fs::read_to_string(p1.join("config.yaml"))
                .unwrap()
                .contains("rtk-rewrite")
        );
        assert!(
            fs::read_to_string(p2.join("config.yaml"))
                .unwrap()
                .contains("rtk-rewrite")
        );

        // Plugins installed in both default and profile directories
        assert!(hermes_plugin_dir(hermes_home).exists());
        assert!(hermes_profile_plugin_dir(hermes_home, "dev").exists());
        assert!(hermes_profile_plugin_dir(hermes_home, "prod").exists());
    }

    #[test]
    fn test_uninstall_hermes_named_profile_preserves_default_plugin_dir() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        // Simulate default profile installed
        run_hermes_mode_at(hermes_home, InitContext::default(), &[]).unwrap();
        assert!(hermes_plugin_dir(hermes_home).exists());

        // Install for named profile
        let profile_dir = hermes_home.join("profiles").join("work");
        fs::create_dir_all(&profile_dir).unwrap();
        let profiles = vec!["work".to_string()];
        run_hermes_mode_at(hermes_home, InitContext::default(), &profiles).unwrap();
        assert!(hermes_profile_plugin_dir(hermes_home, "work").exists());

        // Uninstall only the named profile
        let removed = uninstall_hermes_at(hermes_home, InitContext::default(), &profiles).unwrap();
        assert!(removed.iter().any(|r| r.contains("work")));
        // Profile plugin dir is removed, but default plugin dir is preserved!
        assert!(!hermes_profile_plugin_dir(hermes_home, "work").exists());
        assert!(hermes_plugin_dir(hermes_home).exists());
    }

    #[test]
    fn test_uninstall_hermes_all_profiles_removes_all_plugin_dirs() {
        let temp = TempDir::new().unwrap();
        let hermes_home = temp.path();

        let profile_dir = hermes_home.join("profiles").join("work");
        fs::create_dir_all(&profile_dir).unwrap();

        let profiles = vec!["all".to_string()];
        run_hermes_mode_at(hermes_home, InitContext::default(), &profiles).unwrap();
        assert!(hermes_plugin_dir(hermes_home).exists());
        assert!(hermes_profile_plugin_dir(hermes_home, "work").exists());

        let removed = uninstall_hermes_at(hermes_home, InitContext::default(), &profiles).unwrap();
        assert!(removed.iter().any(|r| r.contains("Hermes plugin")));
        assert!(removed.iter().any(|r| r.contains("work")));
        assert!(!hermes_plugin_dir(hermes_home).exists());
        assert!(!hermes_profile_plugin_dir(hermes_home, "work").exists());
    }
}
