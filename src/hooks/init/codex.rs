//! Codex agent: hook install/uninstall helpers.

use super::*;
use crate::hooks::constants::CODEX_HOOK_COMMAND;
use crate::hooks::is_codex_hook_command;

pub(crate) fn uninstall_codex(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let removed = if global {
        let codex_dir = resolve_codex_dir()?;
        uninstall_codex_at(&codex_dir, ctx)?
    } else {
        uninstall_codex_with_paths(
            Path::new(AGENTS_MD),
            Path::new(RTK_MD),
            &Path::new(CODEX_DIR).join(HOOKS_JSON),
            &[RTK_MD_REF],
            ctx,
        )?
    };

    if removed.is_empty() {
        println!("RTK was not installed for Codex CLI (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Codex CLI:"
        } else {
            "RTK uninstalled for Codex CLI:"
        };
        println!("{}", header);
        for item in removed {
            println!("  - {}", item);
        }
    }

    Ok(())
}

pub(crate) fn uninstall_codex_at(codex_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let absolute_rtk_md_ref = codex_rtk_md_ref(codex_dir);
    uninstall_codex_with_paths(
        &codex_dir.join(AGENTS_MD),
        &codex_dir.join(RTK_MD),
        &codex_dir.join(HOOKS_JSON),
        &[RTK_MD_REF, absolute_rtk_md_ref.as_str()],
        ctx,
    )
}

pub(crate) fn run_codex_mode(global: bool, ctx: InitContext) -> Result<()> {
    let (agents_md_path, rtk_md_path, hooks_json_path) = if global {
        let codex_dir = resolve_codex_dir()?;
        (
            codex_dir.join(AGENTS_MD),
            codex_dir.join(RTK_MD),
            codex_dir.join(HOOKS_JSON),
        )
    } else {
        (
            PathBuf::from(AGENTS_MD),
            PathBuf::from(RTK_MD),
            PathBuf::from(CODEX_DIR).join(HOOKS_JSON),
        )
    };

    run_codex_mode_with_paths(agents_md_path, rtk_md_path, hooks_json_path, global, ctx)
}

pub(crate) fn run_codex_mode_with_paths(
    agents_md_path: PathBuf,
    rtk_md_path: PathBuf,
    hooks_json_path: PathBuf,
    global: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if global
        && !dry_run
        && let Some(parent) = agents_md_path.parent()
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create Codex config directory: {}",
                parent.display()
            )
        })?;
    }

    // ISSUE #892: In global mode, use absolute path so @RTK.md resolves
    // from any CWD (worktrees, nested projects). Codex resolves @ references
    // relative to CWD, not the AGENTS.md file location.
    let rtk_md_ref = if global {
        codex_rtk_md_ref(
            rtk_md_path
                .parent()
                .context("RTK.md path missing parent directory")?,
        )
    } else {
        RTK_MD_REF.to_string()
    };

    write_if_changed(&rtk_md_path, awareness_content(ctx.awareness), RTK_MD, ctx)?;
    let added_ref = patch_agents_md(&agents_md_path, &rtk_md_ref, ctx)?;
    let hook_added = patch_codex_hooks_json(&hooks_json_path, ctx)?;

    if !dry_run {
        println!("\nRTK configured for Codex CLI.\n");
        println!("  RTK.md:    {}", rtk_md_path.display());
        println!(
            "  Hook:      {} ({})",
            hooks_json_path.display(),
            if hook_added {
                "registered"
            } else {
                "already present"
            }
        );
        if added_ref {
            println!("  AGENTS.md: {} reference added", rtk_md_ref);
        } else {
            println!("  AGENTS.md: {} reference already present", rtk_md_ref);
        }
        if global {
            println!(
                "\n  Codex global instructions path: {}",
                agents_md_path.display()
            );
        } else {
            println!(
                "\n  Codex project instructions path: {}",
                agents_md_path.display()
            );
        }
        println!(
            "\n  Restart Codex. For a project hook, approve it when Codex asks you to trust it."
        );
        match crate::core::tracking::get_db_path().and_then(|path| codex_tracking_config(&path)) {
            Ok(config) => {
                println!("\n  Optional: if history recording fails with SQLITE_CANTOPEN in the");
                println!(
                    "  workspace-write sandbox, grant write access to the tracking directory:"
                );
                println!("\n{config}");
                println!(
                    "  Merge this into Codex config.toml only if you want persistent history."
                );
                println!("  Keep existing writable_roots; do not add a duplicate table.");
                println!("  This grants access to the directory, including SQLite sidecar files.");
                println!("  No Codex permission settings have been changed.");
            }
            Err(error) => eprintln!("rtk: warning: could not prepare tracking guidance: {error:#}"),
        }
    }

    Ok(())
}

pub(crate) fn resolve_codex_dir() -> Result<PathBuf> {
    resolve_codex_dir_from(
        std::env::var_os("CODEX_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
}

pub(crate) fn resolve_codex_dir_from(
    codex_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = codex_home.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(path);
    }

    home_dir
        .map(|home| home.join(CODEX_DIR))
        .context("Cannot determine Codex config directory. Set $CODEX_HOME or $HOME.")
}

pub(crate) fn codex_rtk_md_ref(codex_dir: &Path) -> String {
    format!("@{}", codex_dir.join(RTK_MD).display())
}

pub(crate) fn show_codex_config() -> Result<()> {
    let codex_dir = resolve_codex_dir()?;
    let global_agents_md = codex_dir.join(AGENTS_MD);
    let global_rtk_md = codex_dir.join(RTK_MD);
    let global_hooks_json = codex_dir.join(HOOKS_JSON);
    let global_rtk_md_ref = codex_rtk_md_ref(&codex_dir);
    let local_agents_md = PathBuf::from(AGENTS_MD);
    let local_rtk_md = PathBuf::from(RTK_MD);
    let local_hooks_json = PathBuf::from(CODEX_DIR).join(HOOKS_JSON);

    println!("rtk Configuration (Codex CLI):\n");

    if global_rtk_md.exists() {
        println!("[ok] Global RTK.md: {}", global_rtk_md.display());
    } else {
        println!("[--] Global RTK.md: not found");
    }

    if global_hooks_json.exists() {
        let content = fs::read_to_string(&global_hooks_json).with_context(|| {
            format!(
                "Failed to read global Codex hooks: {}",
                global_hooks_json.display()
            )
        })?;
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(root) if codex_hook_already_present(&root) => {
                println!("[ok] Global hook: {}", global_hooks_json.display());
            }
            Ok(_) => println!("[--] Global hooks.json exists but RTK hook is not configured"),
            Err(_) => println!("[!!] Global hooks.json is invalid JSON"),
        }
    } else {
        println!("[--] Global hook: not found");
    }

    if global_agents_md.exists() {
        let content = fs::read_to_string(&global_agents_md).with_context(|| {
            format!(
                "Failed to read global Codex instructions: {}",
                global_agents_md.display()
            )
        })?;
        if has_rtk_reference(&content, &[RTK_MD_REF, global_rtk_md_ref.as_str()]) {
            println!("[ok] Global AGENTS.md: RTK.md reference");
        } else if content.contains(RTK_BLOCK_START) {
            println!("[!!] Global AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Global AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Global AGENTS.md: not found");
    }

    if local_rtk_md.exists() {
        println!("[ok] Local RTK.md: {}", local_rtk_md.display());
    } else {
        println!("[--] Local RTK.md: not found");
    }

    if local_hooks_json.exists() {
        let content = fs::read_to_string(&local_hooks_json).with_context(|| {
            format!(
                "Failed to read local Codex hooks: {}",
                local_hooks_json.display()
            )
        })?;
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(root) if codex_hook_already_present(&root) => {
                println!("[ok] Local hook: {}", local_hooks_json.display());
            }
            Ok(_) => println!("[--] Local hooks.json exists but RTK hook is not configured"),
            Err(_) => println!("[!!] Local hooks.json is invalid JSON"),
        }
    } else {
        println!("[--] Local hook: not found");
    }

    if local_agents_md.exists() {
        let content = fs::read_to_string(&local_agents_md).with_context(|| {
            format!(
                "Failed to read local Codex instructions: {}",
                local_agents_md.display()
            )
        })?;
        if has_rtk_reference(&content, &[RTK_MD_REF]) {
            println!("[ok] Local AGENTS.md: @RTK.md reference");
        } else if content.contains(RTK_BLOCK_START) {
            println!("[!!] Local AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Local AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Local AGENTS.md: not found");
    }

    println!("\nUsage:");
    println!("  rtk init --codex              # Configure local AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init -g --codex           # Configure global AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init --codex --uninstall     # Remove local Codex RTK artifacts");
    println!("  rtk init -g --codex --uninstall  # Remove global Codex RTK artifacts");

    Ok(())
}

fn uninstall_codex_with_paths(
    agents_md_path: &Path,
    rtk_md_path: &Path,
    hooks_json_path: &Path,
    rtk_md_refs: &[&str],
    ctx: InitContext,
) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    if remove_codex_hook_from_file(hooks_json_path, ctx)? {
        removed.push(format!("hooks.json: removed {} entry", CODEX_HOOK_COMMAND));
    }

    if rtk_md_path.exists() {
        if dry_run {
            println!("[dry-run] would remove RTK.md: {}", rtk_md_path.display());
        } else {
            // nosemgrep: filesystem-deletion
            fs::remove_file(rtk_md_path)
                .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
            if verbose > 0 {
                eprintln!("Removed RTK.md: {}", rtk_md_path.display());
            }
        }
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }

    if agents_md_path.exists() {
        let content = fs::read_to_string(agents_md_path)
            .with_context(|| format!("Failed to read AGENTS.md: {}", agents_md_path.display()))?;

        let mut working_content = content.clone();
        let mut agents_changed = false;

        if working_content.contains(RTK_BLOCK_START) {
            let (cleaned, did_remove) = remove_rtk_block(&working_content);
            if did_remove {
                working_content = cleaned;
                agents_changed = true;
                removed.push("AGENTS.md: removed rtk-instructions block".to_string());
            }
        }

        if agents_changed {
            atomic_write(agents_md_path, &working_content).with_context(|| {
                format!("Failed to write AGENTS.md: {}", agents_md_path.display())
            })?;
        }
    }

    if remove_rtk_reference_from_agents(agents_md_path, rtk_md_refs, ctx)? {
        removed.push("AGENTS.md: removed @RTK.md reference".to_string());
    }

    Ok(removed)
}

fn codex_tracking_config(db_path: &Path) -> Result<String> {
    let absolute = std::path::absolute(db_path).context("Failed to resolve RTK database path")?;
    let parent = absolute
        .parent()
        .context("RTK database path has no parent")?;
    let directory = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    let directory = directory
        .to_str()
        .context("RTK database directory is not valid UTF-8")?;
    let quoted = toml::Value::String(directory.to_string());
    Ok(format!(
        "sandbox_mode = \"workspace-write\"\n\n[sandbox_workspace_write]\nwritable_roots = [{quoted}]\n"
    ))
}

fn codex_hook_already_present(root: &serde_json::Value) -> bool {
    root.pointer("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .any(is_codex_hook_command)
}

fn patch_codex_hooks_json(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut root = read_json_file(path)?.unwrap_or_else(|| serde_json::json!({}));

    if codex_hook_already_present(&root) {
        return Ok(false);
    }

    insert_hook_entry(&mut root, CODEX_HOOK_COMMAND)?;
    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize Codex hooks.json")?;

    if dry_run {
        println!("[dry-run] would patch Codex hooks: {}", path.display());
        if verbose > 0 {
            println!("[dry-run] content:\n{}", serialized);
        }
        return Ok(true);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create Codex config directory: {}",
                parent.display()
            )
        })?;
    }
    backup_and_atomic_write(path, &serialized)?;
    if verbose > 0 {
        eprintln!("Patched Codex hooks: {}", path.display());
    }

    Ok(true)
}

fn remove_codex_hook_from_json(root: &mut serde_json::Value) -> bool {
    let Some(pre_tool_use) = root
        .pointer_mut("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return false;
    };

    let mut removed = false;
    for entry in pre_tool_use.iter_mut() {
        let Some(hooks) = entry
            .get_mut("hooks")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        let before = hooks.len();
        hooks.retain(|hook| {
            !hook
                .get("command")
                .and_then(serde_json::Value::as_str)
                .is_some_and(is_codex_hook_command)
        });
        removed |= hooks.len() != before;
    }
    pre_tool_use.retain(|entry| {
        entry
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|hooks| !hooks.is_empty())
    });

    removed
}

fn remove_codex_hook_from_file(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let Some(mut root) = read_json_file(path)? else {
        return Ok(false);
    };
    if !remove_codex_hook_from_json(&mut root) {
        return Ok(false);
    }

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize Codex hooks.json")?;
    if dry_run {
        println!(
            "[dry-run] would remove RTK hook entry from {}",
            path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", serialized);
        }
        return Ok(true);
    }

    backup_and_atomic_write(path, &serialized)?;
    if verbose > 0 {
        eprintln!("Removed Codex RTK hook: {}", path.display());
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::constants::CODEX_HOOK_COMMAND;
    use tempfile::TempDir;

    #[test]
    fn test_codex_mode_rejects_auto_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --auto-patch"
        );
    }

    #[test]
    fn test_codex_mode_rejects_no_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Skip,
            InitContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --no-patch"
        );
    }

    #[test]
    fn test_run_codex_mode_global_writes_absolute_reference_to_codex_dir() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        let rtk_md = temp.path().join("RTK.md");
        let hooks_json = temp.path().join(HOOKS_JSON);

        run_codex_mode_with_paths(
            agents_md.clone(),
            rtk_md.clone(),
            hooks_json.clone(),
            true,
            InitContext::default(),
        )
        .unwrap();

        assert!(rtk_md.exists());
        assert_eq!(fs::read_to_string(&rtk_md).unwrap(), RTK_AWARENESS_DEFAULT);
        assert_eq!(
            fs::read_to_string(&agents_md).unwrap(),
            format!("{}\n", codex_rtk_md_ref(temp.path()))
        );
        let hooks: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hooks_json).unwrap()).unwrap();
        assert!(codex_hook_already_present(&hooks));
    }

    #[test]
    fn test_resolve_codex_dir_prefers_codex_home_and_ignores_empty_value() {
        let codex_home = PathBuf::from("/tmp/custom-codex-home");
        let home_dir = PathBuf::from("/tmp/home");

        let preferred =
            resolve_codex_dir_from(Some(codex_home.clone()), Some(home_dir.clone())).unwrap();
        let empty_falls_back =
            resolve_codex_dir_from(Some(PathBuf::new()), Some(home_dir.clone())).unwrap();
        let missing_falls_back = resolve_codex_dir_from(None, Some(home_dir.clone())).unwrap();

        assert_eq!(preferred, codex_home);
        assert_eq!(empty_falls_back, home_dir.join(".codex"));
        assert_eq!(missing_falls_back, home_dir.join(".codex"));
    }

    #[test]
    fn test_uninstall_codex_at_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");

        fs::write(&agents_md, "# Team rules\n\n@RTK.md\n").unwrap();
        fs::write(&rtk_md, "codex config").unwrap();

        let removed_first = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();
        let removed_second = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!rtk_md.exists());

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("@RTK.md"));
        assert!(content.contains("# Team rules"));
    }

    #[test]
    fn test_uninstall_codex_at_removes_absolute_reference() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");
        let absolute_ref = codex_rtk_md_ref(codex_dir);

        fs::write(&agents_md, format!("# Team rules\n\n{}\n", absolute_ref)).unwrap();
        fs::write(&rtk_md, "codex config").unwrap();

        let removed = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        assert_eq!(removed.len(), 2);
        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains(&absolute_ref));
        assert!(content.contains("# Team rules"));
    }

    #[test]
    fn test_run_codex_mode_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        let rtk_md = temp.path().join("RTK.md");
        let hooks_json = temp.path().join(HOOKS_JSON);

        run_codex_mode_with_paths(
            agents_md.clone(),
            rtk_md.clone(),
            hooks_json.clone(),
            true,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            !rtk_md.exists(),
            "dry-run must not create RTK.md: {}",
            rtk_md.display()
        );
        assert!(
            !agents_md.exists(),
            "dry-run must not create AGENTS.md: {}",
            agents_md.display()
        );
        assert!(
            !hooks_json.exists(),
            "dry-run must not create hooks.json: {}",
            hooks_json.display()
        );
    }

    #[test]
    fn test_uninstall_codex_at_removes_rtk_instructions_block() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");

        fs::write(
            &agents_md,
            format!(
                "# Team rules\n\n{} v2 -->\nOLD RTK STUFF\n{}\n\nMore content",
                RTK_BLOCK_START, RTK_BLOCK_END
            ),
        )
        .unwrap();
        fs::write(&rtk_md, "codex config").unwrap();

        let removed = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("OLD RTK STUFF"));
        assert!(content.contains("# Team rules"));
        assert!(content.contains("More content"));
        assert!(removed.iter().any(|r| r.contains("rtk-instructions block")));
    }
    #[test]
    fn test_codex_tracking_config_limits_root_to_database_parent() {
        let temp = TempDir::new().unwrap();
        let directory = temp.path().join("tracking data");
        fs::create_dir(&directory).unwrap();
        let config = codex_tracking_config(&directory.join("custom.db")).unwrap();
        let parsed: toml::Value = toml::from_str(&config).unwrap();
        assert_eq!(parsed["sandbox_mode"].as_str(), Some("workspace-write"));
        let roots = parsed["sandbox_workspace_write"]["writable_roots"]
            .as_array()
            .unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0].as_str().unwrap(),
            fs::canonicalize(&directory).unwrap().to_str().unwrap()
        );
        assert!(!directory.join("custom.db").exists());
    }

    #[test]
    fn test_codex_tracking_config_escapes_paths_without_creating_files() {
        let temp = TempDir::new().unwrap();
        let directory = temp.path().join("quoted \"directory\"");
        let config = codex_tracking_config(&directory.join("history.db")).unwrap();
        let parsed: toml::Value = toml::from_str(&config).unwrap();
        assert_eq!(
            parsed["sandbox_workspace_write"]["writable_roots"][0]
                .as_str()
                .unwrap(),
            directory.to_str().unwrap()
        );
        assert!(!directory.exists());
    }

    #[test]
    fn test_patch_codex_hooks_is_idempotent_and_preserves_existing_hooks() {
        let temp = TempDir::new().unwrap();
        let hooks_json = temp.path().join(HOOKS_JSON);
        fs::write(
            &hooks_json,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [{ "type": "command", "command": "echo existing" }]
                    }],
                    "Stop": [{
                        "hooks": [{ "type": "command", "command": "echo stop" }]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(patch_codex_hooks_json(&hooks_json, InitContext::default()).unwrap());
        assert!(!patch_codex_hooks_json(&hooks_json, InitContext::default()).unwrap());

        let root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hooks_json).unwrap()).unwrap();
        assert!(codex_hook_already_present(&root));
        assert_eq!(root["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo existing"
        );
        assert_eq!(root["hooks"]["Stop"][0]["hooks"][0]["command"], "echo stop");
        assert!(hooks_json.with_extension("json.bak").exists());
    }

    #[test]
    fn test_local_codex_install_can_be_uninstalled() {
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        fs::write(AGENTS_MD, "# Team rules\n").unwrap();
        run_codex_mode(false, InitContext::default()).unwrap();
        let uninstall_result = uninstall_codex(false, InitContext::default());

        std::env::set_current_dir(original_cwd).unwrap();
        uninstall_result.unwrap();

        assert!(!temp.path().join(RTK_MD).exists());
        assert!(!codex_hook_already_present(
            &serde_json::from_str(
                &fs::read_to_string(temp.path().join(CODEX_DIR).join(HOOKS_JSON)).unwrap()
            )
            .unwrap()
        ));
        let agents = fs::read_to_string(temp.path().join(AGENTS_MD)).unwrap();
        assert_eq!(agents.trim_end(), "# Team rules");
        assert!(!agents.contains(RTK_MD_REF));
    }

    #[test]
    fn test_remove_codex_hook_preserves_other_hooks_in_same_entry() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [
                        { "type": "command", "command": "echo user hook" },
                        { "type": "command", "command": CODEX_HOOK_COMMAND }
                    ]
                }],
                "Stop": [{
                    "hooks": [{ "type": "command", "command": "echo stop" }]
                }]
            }
        });

        assert!(remove_codex_hook_from_json(&mut root));
        assert!(!codex_hook_already_present(&root));
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo user hook"
        );
        assert_eq!(root["hooks"]["Stop"][0]["hooks"][0]["command"], "echo stop");
    }

    #[test]
    fn test_uninstall_codex_at_removes_hook_and_preserves_other_hooks() {
        let temp = TempDir::new().unwrap();
        let hooks_json = temp.path().join(HOOKS_JSON);
        fs::write(
            &hooks_json,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "echo user hook" },
                            { "type": "command", "command": CODEX_HOOK_COMMAND }
                        ]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let removed = uninstall_codex_at(temp.path(), InitContext::default()).unwrap();

        assert_eq!(removed.len(), 1);
        let root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hooks_json).unwrap()).unwrap();
        assert!(!codex_hook_already_present(&root));
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo user hook"
        );
        assert!(hooks_json.with_extension("json.bak").exists());
    }
}
