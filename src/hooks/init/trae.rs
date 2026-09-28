//! Trae agent: install/uninstall RTK PreToolUse hooks.json entries.

use super::*;
use crate::hooks::constants::{
    HOOKS_JSON, PRE_TOOL_USE_KEY, TRAE_CN_DIR, TRAE_DIR, TRAE_HOOK_COMMAND,
    TRAE_RUN_COMMAND_MATCHER,
};

/// Return the hook files Trae uses below `home`.
///
/// `~/.trae/hooks.json` is always managed for global installation. Trae CN is
/// opt-in: it is updated too only when the user already has a `~/.trae-cn`
/// directory.
fn trae_hook_paths_at(home: &Path) -> Vec<PathBuf> {
    let mut paths = vec![home.join(TRAE_DIR).join(HOOKS_JSON)];
    if home.join(TRAE_CN_DIR).is_dir() {
        paths.push(home.join(TRAE_CN_DIR).join(HOOKS_JSON));
    }
    paths
}

fn resolve_trae_hook_paths(global: bool) -> Result<Vec<PathBuf>> {
    if global {
        let home = dirs::home_dir().context("Failed to resolve home directory for Trae")?;
        Ok(trae_hook_paths_at(&home))
    } else {
        Ok(vec![PathBuf::from(TRAE_DIR).join(HOOKS_JSON)])
    }
}

fn read_trae_hooks_json(path: &Path) -> Result<(serde_json::Value, bool)> {
    match read_json_file(path)? {
        Some(root) => Ok((root, true)),
        None => Ok((serde_json::json!({ "version": 1 }), false)),
    }
}

fn validate_trae_hooks_json(root: &serde_json::Value) -> Result<()> {
    let root_object = root
        .as_object()
        .context("Trae hooks.json root must be an object")?;
    if root_object
        .get("version")
        .is_some_and(|version| version != &serde_json::json!(1))
    {
        anyhow::bail!("Trae hooks.json version must be 1");
    }
    if root_object
        .get("hooks")
        .is_some_and(|hooks| !hooks.is_object())
    {
        anyhow::bail!("Trae hooks value is not an object");
    }
    if root_object
        .get("hooks")
        .and_then(|hooks| hooks.get(PRE_TOOL_USE_KEY))
        .is_some_and(|pre_tool_use| !pre_tool_use.is_array())
    {
        anyhow::bail!("Trae PreToolUse value is not an array");
    }
    Ok(())
}

/// Install RTK's Trae hook in every supplied target. All targets are read and
/// parsed before any write, so malformed `.trae` or `.trae-cn` configuration
/// cannot cause a partially-applied update.
fn patch_trae_hooks_json_paths(paths: &[PathBuf], ctx: InitContext) -> Result<Vec<PatchResult>> {
    struct PendingPatch {
        path: PathBuf,
        existed: bool,
        serialized: String,
    }

    let mut results = Vec::with_capacity(paths.len());
    let mut pending = Vec::new();
    let mut applied: Vec<String> = Vec::new();

    // Preflight every target before writing any one of them.
    for path in paths {
        let (mut root, existed) = read_trae_hooks_json(path)?;
        validate_trae_hooks_json(&root)?;
        let added_version = if root.get("version").is_none() {
            root.as_object_mut()
                .expect("validated object")
                .insert("version".into(), serde_json::json!(1));
            true
        } else {
            false
        };

        if trae_hook_already_present(&root) && !added_version {
            results.push(PatchResult::AlreadyPresent);
            // Already carries the hook, so it counts as updated when a later
            // target fails -- otherwise a rerun after fixing that failure
            // reports "none" and invites a duplicate hand-edit.
            applied.push(path.display().to_string());
            continue;
        }

        if !trae_hook_already_present(&root) {
            insert_trae_hook_entry(&mut root)?;
        }
        let serialized =
            serde_json::to_string_pretty(&root).context("Failed to serialize Trae hooks.json")?;
        pending.push(PendingPatch {
            path: path.clone(),
            existed,
            serialized,
        });
        results.push(if ctx.dry_run {
            PatchResult::WouldPatch
        } else {
            PatchResult::Patched
        });
    }

    for patch in pending {
        if ctx.dry_run {
            println!(
                "[dry-run] would patch Trae hooks.json: {}",
                patch.path.display()
            );
            if ctx.verbose > 0 {
                println!("[dry-run] content:\n{}", patch.serialized);
            }
            continue;
        }

        let write_result: Result<()> = (|| {
            let parent = patch.path.parent().with_context(|| {
                format!(
                    "Cannot write Trae hooks file {}: path has no parent directory",
                    patch.path.display()
                )
            })?;
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create Trae directory {}", parent.display()))?;

            if patch.existed {
                let backup_path = patch.path.with_extension("json.bak");
                fs::copy(&patch.path, &backup_path)
                    .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
                if ctx.verbose > 0 {
                    eprintln!("Backup: {}", backup_path.display());
                }
            }

            atomic_write(&patch.path, &patch.serialized)
        })();
        write_result.with_context(|| {
            format!(
                "Failed to update Trae hooks file {}. Already updated: {}",
                patch.path.display(),
                if applied.is_empty() {
                    "none".to_string()
                } else {
                    applied.join(", ")
                }
            )
        })?;
        applied.push(patch.path.display().to_string());
    }

    Ok(results)
}

/// Check whether a Trae config already contains RTK's native command hook.
/// Whether `group` would run for the `RunCommand` tool RTK serves.
///
/// A group with no `matcher` applies to every tool. Otherwise the matcher is
/// read as `|`-separated tool names. Anything unrecognised counts as *not*
/// covering `RunCommand`, so an install adds a registration that fires rather
/// than skipping one that never would.
fn trae_group_covers_run_command(group: &serde_json::Value) -> bool {
    match group.get("matcher") {
        None => true,
        Some(matcher) => matcher.as_str().is_some_and(|matcher| {
            matcher
                .split('|')
                .any(|tool| tool.trim() == TRAE_RUN_COMMAND_MATCHER)
        }),
    }
}

/// Whether `hook` is an entry RTK installed: our command, under the `command`
/// type we write. A missing `type` is ours too, since a hand-written
/// registration commonly omits it; any other explicit type is the user's.
fn is_trae_hook_entry(hook: &serde_json::Value) -> bool {
    is_command_hook(hook, is_trae_hook_command)
}

pub(super) fn trae_hook_already_present(root: &serde_json::Value) -> bool {
    hook_present(
        root,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        trae_group_covers_run_command,
        is_trae_hook_entry,
    )
}

pub(super) fn insert_trae_hook_entry(root: &mut serde_json::Value) -> Result<()> {
    validate_trae_hooks_json(root)?;
    root.as_object_mut()
        .expect("validated object")
        .entry("version")
        .or_insert(serde_json::json!(1));
    append_hook_entry(
        root,
        PRE_TOOL_USE_KEY,
        serde_json::json!({
            "matcher": TRAE_RUN_COMMAND_MATCHER,
            "hooks": [{"type": "command", "command": TRAE_HOOK_COMMAND, "timeout": 30}]
        }),
    )
}

/// Remove RTK commands from nested Trae PreToolUse groups. A group is pruned
/// only when removing RTK made its nested `hooks` array empty.
pub(super) fn remove_trae_hook_from_json(root: &mut serde_json::Value) -> bool {
    remove_hook_entries(
        root,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        is_trae_hook_entry,
    )
}

/// Install Trae's native PreToolUse hook in the project or user configuration.
pub fn run_trae_mode(global: bool, ctx: InitContext) -> Result<()> {
    let paths = resolve_trae_hook_paths(global)?;
    let results = patch_trae_hooks_json_paths(&paths, ctx)?;

    if ctx.dry_run {
        print_dry_run_footer();
        return Ok(());
    }

    let scope = if global { "global" } else { "project" };
    println!("\nTrae hook registered ({scope}).\n");
    println!("  Command: {}", TRAE_HOOK_COMMAND);
    for (path, result) in paths.iter().zip(results) {
        let status = match result {
            PatchResult::Patched => "RTK PreToolUse entry added",
            PatchResult::AlreadyPresent => "RTK PreToolUse entry already present",
            _ => continue,
        };
        println!("  hooks.json: {} ({status})", path.display());
    }
    println!("  Test with: git status\n");
    Ok(())
}

/// Remove Trae hooks from every selected config after parsing all configs
/// first, so malformed selected configs cannot cause a partial uninstall.
fn remove_trae_hooks_json_paths(paths: &[PathBuf], ctx: InitContext) -> Result<Vec<bool>> {
    struct PendingRemoval {
        path: PathBuf,
        serialized: String,
    }

    let mut results = Vec::with_capacity(paths.len());
    let mut pending = Vec::new();
    let mut applied: Vec<String> = Vec::new();

    for path in paths {
        let Some(mut root) = read_json_file(path)? else {
            results.push(false);
            continue;
        };
        let removed = remove_trae_hook_from_json(&mut root);
        results.push(removed);
        if !removed {
            // Present on disk and already free of RTK: same rerun reasoning as
            // the install path.
            applied.push(path.display().to_string());
        }
        if removed {
            pending.push(PendingRemoval {
                path: path.clone(),
                serialized: serde_json::to_string_pretty(&root)
                    .context("Failed to serialize Trae hooks.json")?,
            });
        }
    }

    for removal in pending {
        if ctx.dry_run {
            println!(
                "[dry-run] would remove RTK entry from Trae hooks.json: {}",
                removal.path.display()
            );
            continue;
        }

        let write_result: Result<()> = (|| {
            let backup_path = removal.path.with_extension("json.bak");
            fs::copy(&removal.path, &backup_path)
                .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
            atomic_write(&removal.path, &removal.serialized)
        })();
        write_result.with_context(|| {
            format!(
                "Failed to update Trae hooks file {}. Already updated: {}",
                removal.path.display(),
                if applied.is_empty() {
                    "none".to_string()
                } else {
                    applied.join(", ")
                }
            )
        })?;
        applied.push(removal.path.display().to_string());
    }

    Ok(results)
}

/// Uninstall Trae's native hook from project or selected global configs.
pub fn uninstall_trae_mode(global: bool, ctx: InitContext) -> Result<()> {
    let paths = resolve_trae_hook_paths(global)?;
    let removed = remove_trae_hooks_json_paths(&paths, ctx)?;

    if removed.iter().any(|removed| *removed) {
        let action = if ctx.dry_run {
            "[dry-run] would uninstall RTK (Trae):"
        } else {
            "RTK uninstalled (Trae):"
        };
        println!("{action}");
        for path in paths
            .iter()
            .zip(removed)
            .filter(|(_, removed)| *removed)
            .map(|(path, _)| path.display().to_string())
        {
            println!("  - Trae hooks.json: {path}");
        }
    } else {
        println!("RTK Trae support was not installed (nothing to remove)");
    }

    if ctx.dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Matches this agent's RTK hook command: `rtk hook trae` from a bare, absolute or
/// Windows `rtk` path, and nothing else.
fn is_trae_hook_command(command: &str) -> bool {
    crate::hooks::is_rtk_hook_command(command, "trae")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trae_hook_command_matches_bare_and_absolute_rtk() {
        assert!(is_trae_hook_command("rtk hook trae"));
        assert!(is_trae_hook_command("/opt/homebrew/bin/rtk hook trae"));
        assert!(is_trae_hook_command("\"/opt/homebrew/bin/rtk\" hook trae"));
        assert!(!is_trae_hook_command("rtk hook claude"));
    }

    #[test]
    fn trae_hook_command_matches_windows_rtk_and_rejects_other_commands() {
        assert!(is_trae_hook_command("rtk.exe hook trae"));
        assert!(is_trae_hook_command(
            r#""C:\Program Files\rtk.exe" hook trae"#
        ));
        for command in [
            "not-rtk.exe hook trae",
            "echo rtk.exe hook trae",
            "rtk.exe hook codex",
        ] {
            assert!(!is_trae_hook_command(command));
        }
    }

    #[test]
    fn test_trae_hook_paths_include_trae_cn_only_when_its_directory_exists() {
        let temp = TempDir::new().unwrap();
        let home = temp.path();

        assert_eq!(
            trae_hook_paths_at(home),
            vec![home.join(".trae").join("hooks.json")]
        );

        fs::create_dir(home.join(".trae-cn")).unwrap();
        assert_eq!(
            trae_hook_paths_at(home),
            vec![
                home.join(".trae").join("hooks.json"),
                home.join(".trae-cn").join("hooks.json"),
            ]
        );
    }

    #[test]
    fn test_insert_trae_hook_preserves_unrelated_configuration() {
        let mut root = serde_json::json!({
            "custom": { "keep": true },
            "hooks": {
                "PostToolUse": [{ "matcher": "WriteFile", "hooks": [] }],
                "PreToolUse": [{
                    "matcher": "ReadFile",
                    "hooks": [{ "type": "command", "command": "other read hook" }]
                }]
            }
        });

        insert_trae_hook_entry(&mut root).unwrap();

        assert_eq!(root["version"], 1);
        assert_eq!(root["custom"]["keep"], true);
        assert_eq!(root["hooks"]["PostToolUse"][0]["matcher"], "WriteFile");
        let groups = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1]["matcher"], "RunCommand");
        assert_eq!(groups[1]["hooks"][0]["type"], "command");
        assert_eq!(groups[1]["hooks"][0]["command"], "rtk hook trae");
        assert_eq!(groups[1]["hooks"][0]["timeout"], 30);
    }

    #[test]
    fn test_insert_trae_hook_rejects_non_object_or_non_v1_roots_without_mutation() {
        for mut root in [serde_json::json!(null), serde_json::json!({ "version": 2 })] {
            let original = root.clone();
            assert!(insert_trae_hook_entry(&mut root).is_err());
            assert_eq!(root, original);
        }
    }

    #[test]
    fn test_trae_customized_hooks_are_detected_without_duplicate_install_and_removed() {
        for (matcher, timeout, hook_type) in [
            (
                serde_json::json!("RunCommand|WriteFile"),
                serde_json::json!(30),
                serde_json::json!("command"),
            ),
            (
                serde_json::json!("RunCommand"),
                serde_json::json!(60),
                serde_json::json!("command"),
            ),
            (
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::Value::Null,
            ),
        ] {
            let temp = TempDir::new().unwrap();
            let path = temp.path().join("hooks.json");
            let unrelated = serde_json::json!({ "type": "command", "command": "other hook" });
            let mut group = serde_json::json!({
                "hooks": [{ "command": "rtk hook trae" }, unrelated.clone()]
            });
            if !matcher.is_null() {
                group["matcher"] = matcher;
            }
            if !timeout.is_null() {
                group["hooks"][0]["timeout"] = timeout;
            }
            if !hook_type.is_null() {
                group["hooks"][0]["type"] = hook_type;
            }
            let root = serde_json::json!({
                "version": 1,
                "custom": { "keep": true },
                "hooks": {
                    "PreToolUse": [group],
                    "PostToolUse": [{ "matcher": "WriteFile", "hooks": [] }]
                }
            });
            assert!(trae_hook_already_present(&root), "not detected: {root}");
            let original = serde_json::to_string_pretty(&root).unwrap();
            fs::write(&path, &original).unwrap();
            let paths = vec![path.clone()];
            assert_eq!(
                patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
                vec![PatchResult::AlreadyPresent]
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
            assert_eq!(
                remove_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
                vec![true]
            );
            let actual: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let mut expected = root;
            expected["hooks"]["PreToolUse"][0]["hooks"] = serde_json::json!([unrelated]);
            assert_eq!(actual, expected);
            assert!(!trae_hook_already_present(&actual));
        }
    }

    #[test]
    fn test_trae_registration_outside_run_command_does_not_count_as_installed() {
        // A registration under a matcher that never runs for RunCommand is not
        // a working install: reporting it as present would skip the one that
        // would actually fire.
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("hooks.json");
        let root = serde_json::json!({ "version": 1, "hooks": { PRE_TOOL_USE_KEY: [{
            "matcher": "ReadFile",
            "hooks": [{ "command": "rtk hook trae" }]
        }] } });
        assert!(!trae_hook_already_present(&root));
        fs::write(&path, root.to_string()).unwrap();

        let paths = vec![path.clone()];
        assert_eq!(
            patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
            vec![PatchResult::Patched]
        );
        let patched = read_json_file(&path).unwrap().unwrap();
        assert!(trae_hook_already_present(&patched));
        let groups = patched["hooks"][PRE_TOOL_USE_KEY].as_array().unwrap();
        assert!(
            groups
                .iter()
                .any(|group| group["matcher"] == TRAE_RUN_COMMAND_MATCHER),
            "{patched}"
        );
    }

    #[test]
    fn test_trae_matcher_alternation_counts_as_installed() {
        for matcher in ["RunCommand", "RunCommand|WriteFile", "ReadFile|RunCommand"] {
            let root = serde_json::json!({ "version": 1, "hooks": { PRE_TOOL_USE_KEY: [{
                "matcher": matcher,
                "hooks": [{ "command": "rtk hook trae" }]
            }] } });
            assert!(trae_hook_already_present(&root), "{matcher}");
        }
        // No matcher key at all applies to every tool, RunCommand included.
        let no_matcher = serde_json::json!({ "version": 1, "hooks": { PRE_TOOL_USE_KEY: [{
            "hooks": [{ "command": "rtk hook trae" }]
        }] } });
        assert!(trae_hook_already_present(&no_matcher));
    }

    #[test]
    fn test_trae_rerun_after_failure_still_names_the_completed_target() {
        // The docs tell the user to rerun after fixing the filesystem error.
        // On that rerun the first target preflights as AlreadyPresent, so it
        // must still be listed rather than reported as "none".
        let temp = TempDir::new().unwrap();
        let first = temp.path().join(".trae/hooks.json");
        let second = temp.path().join(".trae-cn/hooks.json");
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&second, "{}").unwrap();
        fs::create_dir(second.with_extension("json.bak")).unwrap();
        let paths = vec![first.clone(), second.clone()];

        let first_error = patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap_err();
        assert!(
            format!("{first_error:#}").contains(&format!("Already updated: {}: ", first.display())),
            "{first_error:#}"
        );

        // Rerun without fixing anything: the first target is installed now.
        let rerun_error = patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap_err();
        let message = format!("{rerun_error:#}");
        assert!(
            message.contains(&format!("Already updated: {}: ", first.display())),
            "{message}"
        );
        assert!(!message.contains("Already updated: none"), "{message}");
    }

    #[test]
    fn test_trae_uninstall_keeps_user_authored_non_command_entries() {
        // `type: "prompt"` is never something RTK installs, so it is the
        // user's -- removing it would also take the group and its keys.
        let mut root = serde_json::json!({ "version": 1, "hooks": { PRE_TOOL_USE_KEY: [{
            "matcher": "ReadFile",
            "description": "user-owned group",
            "hooks": [{ "type": "prompt", "command": "rtk hook trae" }]
        }] } });
        let original = root.clone();
        assert!(!remove_trae_hook_from_json(&mut root));
        assert_eq!(root, original);
    }

    #[test]
    fn test_trae_uninstall_write_failure_reports_completed_targets() {
        let temp = TempDir::new().unwrap();
        let installed = serde_json::json!({ "version": 1, "hooks": { PRE_TOOL_USE_KEY: [{
            "matcher": TRAE_RUN_COMMAND_MATCHER,
            "hooks": [{ "type": "command", "command": TRAE_HOOK_COMMAND }]
        }] } })
        .to_string();
        let first = temp.path().join(".trae/hooks.json");
        let second = temp.path().join(".trae-cn/hooks.json");
        for path in [&first, &second] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, &installed).unwrap();
        }
        // A directory at the backup path fails on every platform, even as root.
        fs::create_dir(second.with_extension("json.bak")).unwrap();

        let error =
            remove_trae_hooks_json_paths(&[first.clone(), second.clone()], InitContext::default())
                .unwrap_err();
        assert!(!trae_hook_already_present(
            &read_json_file(&first).unwrap().unwrap()
        ));
        assert_eq!(fs::read_to_string(&second).unwrap(), installed);
        let message = format!("{error:#}");
        assert!(message.contains(&second.display().to_string()), "{message}");
        assert!(
            message.contains(&format!("Already updated: {}: ", first.display())),
            "{message}"
        );
    }

    #[test]
    fn test_trae_hook_detection_and_removal_preserve_other_commands() {
        let mut root = serde_json::json!({
            "hooks": { "PreToolUse": [{
                "matcher": "RunCommand",
                "hooks": [
                    { "command": "rtk hook cursor" },
                    { "command": "echo rtk hook trae" },
                    { "command": "not-rtk hook trae" }
                ]
            }] }
        });
        let original = root.clone();
        assert!(!trae_hook_already_present(&root));
        assert!(!remove_trae_hook_from_json(&mut root));
        assert_eq!(root, original);
    }

    #[test]
    fn test_remove_trae_hook_only_removes_rtk_and_prunes_its_empty_group() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "RunCommand",
                        "hooks": [
                            { "type": "command", "command": "rtk hook trae" },
                            { "type": "command", "command": "other hook" }
                        ]
                    },
                    {
                        "matcher": "RunCommand",
                        "hooks": [{ "type": "command", "command": "/opt/bin/rtk hook trae" }]
                    }
                ],
                "PostToolUse": [{ "matcher": "WriteFile", "hooks": [] }]
            }
        });

        assert!(remove_trae_hook_from_json(&mut root));

        let groups = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], "other hook");
        assert_eq!(root["hooks"]["PostToolUse"][0]["matcher"], "WriteFile");
    }

    #[test]
    fn test_trae_bom_config_install_and_uninstall() {
        for prefix in ["\u{feff}", "\u{feff}\u{feff}"] {
            let temp = TempDir::new().unwrap();
            let path = temp.path().join("hooks.json");
            let original = serde_json::json!({"version": 1, "custom": true, "hooks": {
                "PreToolUse": [{"matcher": "ReadFile", "hooks": [{"command": "other hook"}]}]
            }});
            fs::write(&path, format!("{prefix}{original}")).unwrap();
            let paths = vec![path.clone()];
            assert_eq!(
                patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
                vec![PatchResult::Patched]
            );
            let installed = fs::read_to_string(&path).unwrap();
            assert!(trae_hook_already_present(
                &serde_json::from_str(&installed).unwrap()
            ));
            fs::write(&path, format!("{prefix}{installed}")).unwrap();
            assert_eq!(
                remove_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
                vec![true]
            );
            let removed: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(removed, original);
        }
    }

    #[test]
    fn test_trae_windows_command_install_is_idempotent_and_uninstalls() {
        for command in [
            "rtk.exe hook trae",
            r#""C:\Program Files\rtk.exe" hook trae"#,
        ] {
            let temp = TempDir::new().unwrap();
            let path = temp.path().join("hooks.json");
            let root = serde_json::json!({"version": 1, "hooks": {"PreToolUse": [{
                "matcher": "RunCommand|WriteFile", "hooks": [{"command": command, "timeout": 60}]
            }]}});
            let original = root.to_string();
            fs::write(&path, &original).unwrap();
            let paths = vec![path.clone()];
            assert_eq!(
                patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
                vec![PatchResult::AlreadyPresent]
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
            assert_eq!(
                remove_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
                vec![true]
            );
            let removed: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(removed["hooks"]["PreToolUse"], serde_json::json!([]));
        }
    }

    #[test]
    fn test_trae_write_failure_reports_completed_targets() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join(".trae/hooks.json");
        let second = temp.path().join(".trae-cn/hooks.json");
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&second, "{}").unwrap();
        // A directory at the backup path fails on every platform, even as root.
        fs::create_dir(second.with_extension("json.bak")).unwrap();
        let error =
            patch_trae_hooks_json_paths(&[first.clone(), second.clone()], InitContext::default())
                .unwrap_err();
        assert!(trae_hook_already_present(
            &read_json_file(&first).unwrap().unwrap()
        ));
        assert_eq!(fs::read_to_string(&second).unwrap(), "{}");
        let message = format!("{error:#}");
        assert!(message.contains(&second.display().to_string()), "{message}");
        // Pin the exact list: a prefix match also accepts the failing target
        // being reported as already updated.
        assert!(
            message.contains(&format!("Already updated: {}: ", first.display())),
            "{message}"
        );
    }

    #[test]
    fn test_trae_patch_preflights_all_global_targets_before_writing() {
        let temp = TempDir::new().unwrap();
        let trae = temp.path().join(".trae").join("hooks.json");
        let trae_cn = temp.path().join(".trae-cn").join("hooks.json");
        fs::create_dir_all(trae.parent().unwrap()).unwrap();
        fs::create_dir_all(trae_cn.parent().unwrap()).unwrap();
        let original = "{\n  \"existing\": true\n}\n";
        fs::write(&trae, original).unwrap();
        fs::write(&trae_cn, "not json").unwrap();

        let result = patch_trae_hooks_json_paths(&[trae.clone(), trae_cn], InitContext::default());

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(trae).unwrap(), original);
    }

    #[test]
    fn test_trae_patch_rejects_non_v1_config_before_idempotency_check() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(".trae").join("hooks.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"{
            "version": 2,
            "hooks": {
                "PreToolUse": [{
                    "matcher": "RunCommand",
                    "hooks": [{
                        "type": "command",
                        "command": "rtk hook trae",
                        "timeout": 30
                    }]
                }]
            }
        }"#;
        fs::write(&path, original).unwrap();

        assert!(
            patch_trae_hooks_json_paths(std::slice::from_ref(&path), InitContext::default())
                .is_err()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn test_trae_patch_adds_missing_version_without_duplicating_matching_hook() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(".trae").join("hooks.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "RunCommand",
                        "hooks": [{
                            "type": "command",
                            "command": "rtk hook trae",
                            "timeout": 30
                        }]
                    }]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            patch_trae_hooks_json_paths(std::slice::from_ref(&path), InitContext::default())
                .unwrap(),
            vec![PatchResult::Patched]
        );
        let root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(root["version"], 1);
        assert_eq!(root["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_trae_patch_updates_every_selected_target_and_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let trae = temp.path().join(".trae").join("hooks.json");
        let trae_cn = temp.path().join(".trae-cn").join("hooks.json");
        fs::create_dir_all(trae.parent().unwrap()).unwrap();
        fs::write(&trae, r#"{ "custom": { "keep": true } }"#).unwrap();

        let paths = vec![trae.clone(), trae_cn.clone()];
        let results = patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap();
        assert_eq!(results, vec![PatchResult::Patched, PatchResult::Patched]);
        assert!(trae.with_extension("json.bak").exists());

        for path in &paths {
            let root: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert!(trae_hook_already_present(&root));
        }
        let trae_root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&trae).unwrap()).unwrap();
        assert_eq!(trae_root["custom"]["keep"], true);

        let results = patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap();
        assert_eq!(
            results,
            vec![PatchResult::AlreadyPresent, PatchResult::AlreadyPresent]
        );
    }

    #[test]
    fn test_trae_uninstall_updates_every_selected_target_and_preserves_siblings() {
        let temp = TempDir::new().unwrap();
        let paths = vec![
            temp.path().join(".trae").join("hooks.json"),
            temp.path().join(".trae-cn").join("hooks.json"),
        ];
        for path in &paths {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                r#"{
                    "hooks": {
                        "PreToolUse": [{
                            "matcher": "RunCommand",
                            "hooks": [
                                { "type": "command", "command": "rtk hook trae" },
                                { "type": "command", "command": "other hook" }
                            ]
                        }]
                    }
                }"#,
            )
            .unwrap();
        }

        assert_eq!(
            remove_trae_hooks_json_paths(&paths, InitContext::default()).unwrap(),
            vec![true, true]
        );
        for path in &paths {
            assert!(path.with_extension("json.bak").exists());
            let root: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert!(!trae_hook_already_present(&root));
            assert_eq!(
                root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
                "other hook"
            );
        }
    }
}
