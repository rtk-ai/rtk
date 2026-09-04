//! Hermes agent: hook install/uninstall helpers.

use super::*;

// ─── Hermes support ────────────────────────────────────────────

pub(crate) const HERMES_PLUGIN_INIT: &str =
    include_str!("../../../hooks/hermes/rtk-rewrite/__init__.py");

pub(crate) const HERMES_PLUGIN_YAML: &str =
    include_str!("../../../hooks/hermes/rtk-rewrite/plugin.yaml");

pub fn run_hermes_mode(ctx: InitContext) -> Result<()> {
    let hermes_home = resolve_hermes_home()?;
    run_hermes_mode_at(&hermes_home, ctx)
}

pub(crate) fn hermes_plugin_dir(hermes_home: &Path) -> PathBuf {
    hermes_home
        .join(HERMES_PLUGINS_SUBDIR)
        .join(HERMES_PLUGIN_NAME)
}

pub(crate) fn run_hermes_mode_at(hermes_home: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let plugin_dir = hermes_plugin_dir(hermes_home);
    if !dry_run {
        fs::create_dir_all(&plugin_dir).with_context(|| {
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

    let config_path = hermes_home.join("config.yaml");
    let existing_config = if config_path.exists() {
        fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read Hermes config: {}", config_path.display()))?
    } else {
        String::new()
    };
    let patched_config = patch_hermes_config(&existing_config);
    write_if_changed(&config_path, &patched_config, "Hermes config", ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nRTK configured for Hermes.\n");
        println!("  Plugin: {}", plugin_dir.display());
        println!("  Config: {}", config_path.display());
        println!("  Hermes will now rewrite terminal commands through rtk.");
        println!("  Restart Hermes. Test with: git status\n");
    }

    Ok(())
}

pub fn uninstall_hermes(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let hermes_home = resolve_hermes_home()?;
    let removed = uninstall_hermes_at(&hermes_home, ctx)?;

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

pub(crate) fn uninstall_hermes_at(hermes_home: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { verbose, dry_run } = ctx;
    let mut removed = Vec::new();

    let plugin_dir = hermes_plugin_dir(hermes_home);
    if plugin_dir.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove Hermes plugin directory: {}",
                plugin_dir.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- uninstall intentionally removes only RTK's Hermes plugin directory.
            fs::remove_dir_all(&plugin_dir).with_context(|| {
                format!(
                    "Failed to remove Hermes plugin directory: {}",
                    plugin_dir.display()
                )
            })?;
            if verbose > 0 {
                eprintln!("Removed Hermes plugin directory: {}", plugin_dir.display());
            }
        }
        removed.push(format!("Hermes plugin: {}", plugin_dir.display()));
    }

    let config_path = hermes_home.join("config.yaml");
    if config_path.exists() {
        let existing_config = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read Hermes config: {}", config_path.display()))?;
        let patched_config = unpatch_hermes_config(&existing_config);

        if patched_config != existing_config {
            if dry_run {
                println!(
                    "[dry-run] would update Hermes config: {}",
                    config_path.display()
                );
                if verbose > 0 {
                    println!("[dry-run] content:\n{}", patched_config);
                }
            } else {
                atomic_write(&config_path, &patched_config).with_context(|| {
                    format!("Failed to write Hermes config: {}", config_path.display())
                })?;
                if verbose > 0 {
                    eprintln!("Updated Hermes config: {}", config_path.display());
                }
            }
            removed.push("Hermes config: removed RTK plugin entry".to_string());
        }
    }

    Ok(removed)
}

pub(crate) fn patch_hermes_config(existing: &str) -> String {
    rewrite_hermes_config(existing, true)
}

pub(crate) fn unpatch_hermes_config(existing: &str) -> String {
    rewrite_hermes_config(existing, false)
}

pub(crate) fn rewrite_hermes_config(existing: &str, add_rtk: bool) -> String {
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

pub(crate) fn split_yaml_lines(input: &str) -> Vec<String> {
    if input.is_empty() {
        Vec::new()
    } else {
        input.split_inclusive('\n').map(str::to_string).collect()
    }
}

pub(crate) fn ensure_previous_yaml_line_ends_with_newline(lines: &mut [String], insert_idx: usize) {
    if insert_idx == 0 {
        return;
    }

    if let Some(previous) = lines.get_mut(insert_idx - 1) {
        if !previous.ends_with('\n') {
            previous.push('\n');
        }
    }
}

pub(crate) fn hermes_plugins_block() -> String {
    format!("plugins:\n  enabled:\n    - {}\n", HERMES_PLUGIN_NAME)
}

pub(crate) fn append_hermes_plugins_block(existing: &str) -> String {
    let mut patched = existing.to_string();
    if !patched.ends_with('\n') {
        patched.push('\n');
    }
    patched.push_str(&hermes_plugins_block());
    patched
}

pub(crate) fn find_yaml_key_line(
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

pub(crate) fn yaml_block_end(lines: &[String], start: usize, parent_indent: usize) -> usize {
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

pub(crate) fn rewrite_inline_hermes_enabled(
    lines: &mut [String],
    enabled_idx: usize,
    add_rtk: bool,
) {
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

pub(crate) fn rewrite_block_hermes_enabled(
    lines: &mut Vec<String>,
    enabled_idx: usize,
    add_rtk: bool,
) {
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

pub(crate) fn hermes_enabled_list_end(lines: &[String], enabled_idx: usize) -> usize {
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

pub(crate) fn hermes_enabled_list_item_indent(
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

pub(crate) fn hermes_missing_enabled_indents(
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

pub(crate) fn yaml_line_without_ending(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

pub(crate) fn yaml_line_ending(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

pub(crate) fn yaml_indent(line: &str) -> usize {
    yaml_line_without_ending(line)
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .count()
}

pub(crate) fn is_yaml_list_item_named(line: &str, expected: &str) -> bool {
    let trimmed = yaml_line_without_ending(line).trim();
    let Some(item) = trimmed.strip_prefix("- ") else {
        return false;
    };

    normalized_yaml_scalar(item).is_some_and(|item| item == expected)
}

pub(crate) fn is_yaml_list_item_line(line: &str) -> bool {
    yaml_line_without_ending(line).trim().starts_with("- ")
}

pub(crate) fn is_hermes_plugin_name(value: &str) -> bool {
    normalized_yaml_scalar(value).is_some_and(|item| item == HERMES_PLUGIN_NAME)
}

pub(crate) fn collapse_yaml_list_key_to_empty(line: &str) -> String {
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

pub(crate) fn normalized_yaml_scalar(value: &str) -> Option<String> {
    let without_comment = value.split_once('#').map_or(value, |(item, _)| item);
    let trimmed = without_comment.trim().trim_matches(['\'', '"']);
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(crate) fn resolve_hermes_home() -> Result<PathBuf> {
    resolve_hermes_home_from_env(dirs::home_dir(), std::env::var_os("HERMES_HOME"))
}

pub(crate) fn resolve_hermes_home_from_env(
    home_dir: Option<PathBuf>,
    hermes_home: Option<OsString>,
) -> Result<PathBuf> {
    if let Some(path) = hermes_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    home_dir
        .map(|home| home.join(HERMES_DIR))
        .context("Cannot determine Hermes home directory. Set $HERMES_HOME or $HOME.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_hermes_mode_creates_plugin_files() {
        let temp = TempDir::new().unwrap();
        run_hermes_mode_at(temp.path(), InitContext::default()).unwrap();

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

        run_hermes_mode_at(temp.path(), InitContext::default()).unwrap();
        let first = fs::read_to_string(&config_path).unwrap();
        run_hermes_mode_at(temp.path(), InitContext::default()).unwrap();
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

        run_hermes_mode_at(temp.path(), InitContext::default()).unwrap();
        let first = fs::read_to_string(&config_path).unwrap();
        run_hermes_mode_at(temp.path(), InitContext::default()).unwrap();
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

        run_hermes_mode_at(hermes_home, InitContext::default()).unwrap();
        let first = fs::read_to_string(&config_path).unwrap();
        run_hermes_mode_at(hermes_home, InitContext::default()).unwrap();
        let second = fs::read_to_string(&config_path).unwrap();

        let installed = "theme: dark\nplugins:\n disabled:\n - google_meet\n - spotify\n search_path: ./plugins\n enabled:\n - rtk-rewrite\nother: true\n";
        assert_eq!(first, installed);
        assert_eq!(second, installed);
        assert_eq!(first.matches("rtk-rewrite").count(), 1);
        assert!(plugin_dir.exists());
        assert_eq!(fs::read_to_string(&other_plugin_file).unwrap(), "keep");

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();

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

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();

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

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();

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

        let removed_first = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();
        let removed_second = uninstall_hermes_at(hermes_home, InitContext::default()).unwrap();

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
}
