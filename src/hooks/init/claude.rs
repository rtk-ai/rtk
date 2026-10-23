//! Claude Code: the default `rtk init` flow -- settings.json hook registration, RTK.md and
//! CLAUDE.md patching, legacy-hook migration -- plus the `--claude-md` injection mode.

use super::opencode::{ensure_opencode_plugin_installed, prepare_opencode_plugin_path};
use super::*;
use crate::core::user_dirs;
use crate::hooks::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, CURSOR_DIR, HOOKS_SUBDIR, POST_TOOL_USE_KEY, PRE_TOOL_USE_KEY,
    REWRITE_HOOK_FILE, SESSION_START_KEY, SETTINGS_JSON,
};

/// Legacy mode (--claude-md): inject the full RTK_INSTRUCTIONS block into CLAUDE.md.
pub(super) fn run_claude_md_mode(
    global: bool,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    run_claude_md_mode_with(global, install_opencode, RTK_INSTRUCTIONS, ctx)
}

fn run_claude_md_mode_with(
    global: bool,
    install_opencode: bool,
    block: &str,
    ctx: InitContext,
) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let path = if global {
        resolve_claude_dir()?.join(CLAUDE_MD)
    } else {
        user_dirs::in_working_dir(CLAUDE_MD)
    };

    if global
        && !dry_run
        && let Some(parent) = path.parent()
    {
        fs::create_dir_all(parent)?;
    }

    if verbose > 0 {
        eprintln!("Writing rtk instructions to: {}", path.display());
    }

    let recovery_cmd = if global {
        "rtk init -g --claude-md"
    } else {
        "rtk init --claude-md"
    };

    let action = write_rtk_block(&path, block, "rtk instructions", recovery_cmd, ctx)?;

    // Runs before the Unchanged return so that re-running restores a deleted plugin.
    if global && install_opencode {
        let opencode_plugin_path = prepare_opencode_plugin_path()?;
        let installed = ensure_opencode_plugin_installed(&opencode_plugin_path, ctx)?;
        if !dry_run && installed {
            println!(
                "[ok] OpenCode plugin installed: {}",
                opencode_plugin_path.display()
            );
        }
    }

    if matches!(action, RtkBlockUpsert::Unchanged) {
        return Ok(());
    }

    if global {
        if !dry_run {
            println!("   Claude Code will now use rtk in all sessions");
        }
    } else if !dry_run {
        println!("   Claude Code will use rtk in this project");
    }

    Ok(())
}

/// Patch CLAUDE.md: add @RTK.md, migrate if old block exists
fn patch_claude_md(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let mut migrated = false;

    // Check for old block and migrate
    if content.contains(RTK_BLOCK_START) {
        let (new_content, did_migrate) = remove_rtk_block(&content);
        if did_migrate {
            content = new_content;
            migrated = true;
            if verbose > 0 {
                eprintln!("Migrated: removed old RTK block from CLAUDE.md");
            }
        }
    }

    // Check if @RTK.md already present
    if has_rtk_reference(&content, &[RTK_MD_REF]) {
        if verbose > 0 {
            eprintln!("@RTK.md reference already present in CLAUDE.md");
        }
        if migrated {
            if dry_run {
                println!(
                    "[dry-run] would migrate old RTK block in CLAUDE.md: {}",
                    path.display()
                );
            } else {
                fs::write(path, content)?;
            }
        }
        return Ok(migrated);
    }

    // Add @RTK.md
    let new_content = if content.is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim())
    };

    if dry_run {
        println!(
            "[dry-run] would add @RTK.md reference to CLAUDE.md: {}",
            path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", new_content);
        }
    } else {
        fs::write(path, new_content)?;

        if verbose > 0 {
            eprintln!("Added @RTK.md reference to CLAUDE.md");
        }
    }

    Ok(migrated)
}

pub(super) fn remove_hook_from_json(root: &mut serde_json::Value) -> bool {
    let mut modified = false;
    for event_key in [PRE_TOOL_USE_KEY, POST_TOOL_USE_KEY, SESSION_START_KEY] {
        if !remove_hook_entries(root, event_key, HookEntries::Grouped, |hook| {
            is_command_hook(hook, |cmd| {
                is_claude_hook_command(cmd) || cmd.contains(REWRITE_HOOK_FILE)
            })
        }) {
            continue;
        }
        modified = true;
        if let Some(hooks_obj) = root.get_mut("hooks").and_then(|h| h.as_object_mut())
            && hooks_obj
                .get(event_key)
                .and_then(|a| a.as_array())
                .is_some_and(|a| a.is_empty())
        {
            hooks_obj.remove(event_key);
        }
    }

    if modified
        && root
            .get("hooks")
            .and_then(|h| h.as_object())
            .is_some_and(|h| h.is_empty())
        && let Some(obj) = root.as_object_mut()
    {
        obj.remove("hooks");
    }

    modified
}

/// Remove the `env.ANTHROPIC_BASE_URL` that the proxy on-ramp of `rtk init -g`
/// writes, when it still points at the RTK daemon. Left behind, it sends every
/// Claude Code request to a daemon that uninstall no longer starts (#4604). A
/// base URL aimed anywhere else belongs to the user and is kept.
pub(super) fn remove_proxy_env_from_json(root: &mut serde_json::Value) -> bool {
    let rtk_url = format!("http://{}", crate::llm_proxy::DEFAULT_BIND);
    let Some(env) = root.get_mut("env").and_then(|e| e.as_object_mut()) else {
        return false;
    };
    if env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) != Some(rtk_url.as_str()) {
        return false;
    }
    env.remove("ANTHROPIC_BASE_URL");
    if env.is_empty()
        && let Some(obj) = root.as_object_mut()
    {
        obj.remove("env");
    }
    true
}

/// What `remove_settings_entries` took out of settings.json.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SettingsRemoval {
    pub hook: bool,
    pub proxy_base_url: bool,
}

/// Remove the RTK hook and the RTK proxy's `env.ANTHROPIC_BASE_URL` from
/// settings.json, reporting each separately so uninstall can say which.
/// Backs up before modification.
pub(super) fn remove_settings_entries(ctx: InitContext) -> Result<SettingsRemoval> {
    let InitContext { verbose, .. } = ctx;
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    let Some(mut root) = read_json_file(&settings_path)? else {
        if verbose > 0 {
            eprintln!("settings.json not found, nothing to remove");
        }
        return Ok(SettingsRemoval::default());
    };

    let removed_hook = remove_hook_from_json(&mut root);
    let removed_env = remove_proxy_env_from_json(&mut root);
    let removed = removed_hook || removed_env;
    if removed {
        update_json_file(
            &settings_path,
            &root,
            ctx,
            "settings.json",
            &format!(
                "[dry-run] would remove RTK hook entry from {}",
                settings_path.display()
            ),
            true,
            Written::Line("Removed RTK hook from settings.json".to_string()),
        )?;
    }

    Ok(SettingsRemoval {
        hook: removed_hook,
        proxy_base_url: removed_env,
    })
}

/// Orchestrator: patch settings.json with RTK hook (binary command variant)
/// Handles reading, checking, prompting, merging, backing up, and atomic writing
fn patch_settings_json_command(
    hook_command: &str,
    mode: PatchMode,
    include_opencode: bool,
    ctx: InitContext,
    scope: HookInstallScope,
) -> Result<PatchResult> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let settings_path = match scope {
        HookInstallScope::UserGlobal => resolve_claude_dir()?.join(SETTINGS_JSON),
        HookInstallScope::ProjectExtra => {
            crate::core::user_dirs::in_working_dir(".claude").join(SETTINGS_JSON)
        }
    };

    // Read or create settings.json
    let mut root = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;
        let content = strip_leading_bom(&content);

        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            from_json_str(content)
                .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?
        }
    } else {
        serde_json::json!({})
    };

    // Hook events follow the active pipeline. `rtk:default` uses
    // `hook.before_tool` only → exactly today's PreToolUse entry; enabling
    // `rtk:posthook` adds PostToolUse; removing it removes the entry again.
    let mut events = required_claude_events_scoped(scope);
    if scope == HookInstallScope::ProjectExtra {
        // An event the global file already carries fires here too (Claude
        // Code merges both files); registering it again would run the hook
        // twice on every tool call.
        if let Ok(dir) = resolve_claude_dir() {
            let global = dir.join(SETTINGS_JSON);
            if let Ok(content) = fs::read_to_string(&global)
                && let Ok(global_root) =
                    from_json_str::<serde_json::Value>(strip_leading_bom(&content))
            {
                events.retain(|(key, _)| !hook_already_present(&global_root, key, hook_command));
            }
        }
        if events.is_empty() {
            return Ok(PatchResult::AlreadyPresent);
        }
    }
    let events = events;
    let missing: Vec<(&'static str, String)> = events
        .iter()
        .filter(|(event_key, _)| !hook_already_present(&root, event_key, hook_command))
        .cloned()
        .collect();
    let wants_post = events.iter().any(|(k, _)| *k == POST_TOOL_USE_KEY);
    let mut refreshed: Vec<(&'static str, String)> = Vec::new();
    for (event_key, matcher) in &events {
        if missing.iter().any(|(k, _)| k == event_key) {
            continue;
        }
        if refresh_hook_matcher(&mut root, event_key, matcher) {
            refreshed.push((event_key, matcher.clone()));
        }
    }
    let matcher_stale = !refreshed.is_empty();
    // Never delete. An entry the pipeline no longer requires is a symptom to
    // report, not an instruction to act on: removing on an empty set would
    // wipe a user's global hooks because of the project directory
    // `init -g` happened to run from. Removal stays where the user
    // asks for it by name (`rtk init -g --uninstall`).
    let unrequired: Vec<&str> = [PRE_TOOL_USE_KEY, POST_TOOL_USE_KEY]
        .into_iter()
        .filter(|event_key| {
            !events.iter().any(|(k, _)| k == event_key)
                && hook_already_present(&root, event_key, hook_command)
        })
        .collect();
    if !unrequired.is_empty() {
        println!(
            "  {}: {} no longer required by the pipeline — left unchanged \
             (remove with `rtk init -g --uninstall`)",
            settings_path.display(),
            unrequired.join(", ")
        );
    }
    if events.is_empty() {
        println!(
            "  {}: the pipeline requires no hook events — file untouched",
            settings_path.display()
        );
        return Ok(PatchResult::AlreadyPresent);
    }
    if missing.is_empty() && !matcher_stale {
        if verbose > 0 {
            eprintln!("settings.json: hook already present");
        }
        return Ok(PatchResult::AlreadyPresent);
    }

    // Handle mode (one consent prompt covers every event being registered)
    match mode {
        PatchMode::Skip => {
            print_manual_instructions(hook_command, include_opencode);
            return Ok(PatchResult::Skipped);
        }
        PatchMode::Ask => {
            // Skip the interactive prompt in dry-run: we must not mutate state or block on stdin.
            if dry_run {
                println!(
                    "[dry-run] would prompt before patching {}",
                    settings_path.display()
                );
            } else {
                if wants_post {
                    eprintln!(
                        "\nRTK registers `{hook_command}` to rewrite Bash commands (PreToolUse)\nand compress tool outputs (PostToolUse — from your pipeline)."
                    );
                } else {
                    eprintln!(
                        "\nRTK registers `{hook_command}` to rewrite Bash commands (PreToolUse)."
                    );
                }
                if !prompt_user_consent(&settings_path)? {
                    print_manual_instructions(hook_command, include_opencode);
                    return Ok(PatchResult::Declined);
                }
            }
        }
        PatchMode::Auto => {
            // Proceed without prompting
        }
    }

    for (event_key, matcher) in &missing {
        insert_hook_entry(&mut root, event_key, matcher, hook_command)?;
    }

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;

    if dry_run {
        for (event_key, matcher) in &missing {
            println!(
                "[dry-run] would patch settings.json ({event_key}, matcher \"{matcher}\"): {}",
                settings_path.display()
            );
        }
        for (event_key, matcher) in &refreshed {
            println!(
                "[dry-run] would update {event_key} matcher to \"{matcher}\": {}",
                settings_path.display()
            );
        }
        if verbose > 0 {
            println!("[dry-run] content:\n{}", serialized);
        }
        return Ok(PatchResult::WouldPatch);
    }

    super::ensure_parent_dir(&settings_path)?;
    // Backup original
    if settings_path.exists() {
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(&settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
        if verbose > 0 {
            eprintln!("Backup: {}", backup_path.display());
        }
    }

    // Atomic write
    atomic_write(&settings_path, &serialized)?;

    println!("\n  {}: hook added", settings_path.display());
    if settings_path.with_extension("json.bak").exists() {
        println!(
            "  Backup: {}",
            settings_path.with_extension("json.bak").display()
        );
    }
    if include_opencode {
        println!("  Restart Claude Code and OpenCode. Test with: git status");
    } else {
        println!("  Restart Claude Code. Test with: git status");
    }

    Ok(PatchResult::Patched)
}

/// Check if RTK hook is already present under the given event key.
/// Which settings file a hook registration is tied to. Scope-matched on
/// purpose: the user pipeline drives `~/.claude/settings.json`, a project
/// pipeline drives only that project's own `.claude/settings.json` — a file
/// in the current directory must never reshape configuration that outlives
/// the directory (running `init -g` from a project that disables the rewrite
/// must not delete the global hook).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HookInstallScope {
    UserGlobal,
    ProjectExtra,
}

/// The Claude Code hook events the pipeline needs: one settings entry
/// per `hook.<event>` position with an enabled rule, derived per scope.
#[cfg(not(feature = "engine"))]
pub(super) fn required_claude_events_scoped(
    scope: HookInstallScope,
) -> Vec<(&'static str, String)> {
    match scope {
        HookInstallScope::UserGlobal => vec![(
            PRE_TOOL_USE_KEY,
            crate::hooks::constants::CLAUDE_PRE_MATCHER.to_string(),
        )],
        HookInstallScope::ProjectExtra => Vec::new(),
    }
}

#[cfg(feature = "engine")]
pub(super) fn claude_events_of(
    loaded: &crate::core::pipeline_config::LoadedPipeline,
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (site, event) in loaded.resolved.events_used() {
        if site != rtk_engine::pipeline::SiteKind::Hook {
            continue;
        }
        let key = match event {
            rtk_engine::block::Event::BeforeTool => PRE_TOOL_USE_KEY,
            rtk_engine::block::Event::AfterTool => POST_TOOL_USE_KEY,
            rtk_engine::block::Event::Request => continue,
        };
        let subjects = loaded.resolved.subjects_used(site, event);
        if let Some(matcher) = crate::hooks::constants::claude_matcher(&subjects) {
            out.push((key, matcher));
        }
    }
    out
}

#[cfg(feature = "engine")]
pub(super) fn required_claude_events_scoped(
    scope: HookInstallScope,
) -> Vec<(&'static str, String)> {
    match scope {
        HookInstallScope::UserGlobal => {
            claude_events_of(&crate::core::pipeline_config::load_user_scope())
        }
        HookInstallScope::ProjectExtra => {
            let user = claude_events_of(&crate::core::pipeline_config::load_user_scope());
            claude_events_of(&crate::core::pipeline_config::load())
                .into_iter()
                .filter(|(key, _)| !user.iter().any(|(u, _)| u == key))
                .collect()
        }
    }
}

/// Global-surface view, for `rtk init --show` status against the global file.
pub(super) fn required_claude_events() -> Vec<(&'static str, String)> {
    required_claude_events_scoped(HookInstallScope::UserGlobal)
}

/// Update the matcher of RTK's own entries under `event_key` when it drifted
/// from the current constant (e.g. Bash added to the PostToolUse matcher).
/// Returns true if anything changed. Never touches non-RTK entries.
pub(super) fn refresh_hook_matcher(
    root: &mut serde_json::Value,
    event_key: &str,
    matcher: &str,
) -> bool {
    let entries = match root
        .get_mut("hooks")
        .and_then(|h| h.get_mut(event_key))
        .and_then(|p| p.as_array_mut())
    {
        Some(arr) => arr,
        None => return false,
    };

    let mut changed = false;
    for entry in entries.iter_mut() {
        let has_rtk_command = entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks
                    .iter()
                    .any(|hook| is_command_hook(hook, is_claude_hook_command))
            })
            .unwrap_or(false);
        if has_rtk_command && entry.get("matcher").and_then(|m| m.as_str()) != Some(matcher) {
            entry["matcher"] = serde_json::json!(matcher);
            changed = true;
        }
    }
    changed
}

pub fn claude_hook_gaps() -> Vec<String> {
    let Ok(dir) = resolve_claude_dir() else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(dir.join(SETTINGS_JSON)) else {
        return Vec::new();
    };
    let Ok(root) = from_json_str::<serde_json::Value>(strip_leading_bom(&content)) else {
        return Vec::new();
    };
    hook_gaps(&root, &required_claude_events())
}

pub(super) fn hook_gaps(
    root: &serde_json::Value,
    required: &[(&'static str, String)],
) -> Vec<String> {
    let installed = [PRE_TOOL_USE_KEY, POST_TOOL_USE_KEY]
        .iter()
        .any(|key| hook_already_present(root, key, CLAUDE_HOOK_COMMAND));
    if !installed {
        return Vec::new();
    }
    let mut gaps = Vec::new();
    for (event, matcher) in required {
        if !hook_already_present(root, event, CLAUDE_HOOK_COMMAND) {
            gaps.push(format!("{event} hook missing"));
            continue;
        }
        let matchers = rtk_hook_matchers(root, event);
        if matchers.iter().any(|m| m.is_empty() || m == "*") {
            continue;
        }
        let have: Vec<&str> = matchers.iter().flat_map(|m| m.split('|')).collect();
        let missing: Vec<&str> = matcher.split('|').filter(|t| !have.contains(t)).collect();
        if !missing.is_empty() {
            gaps.push(format!("{event} does not cover {}", missing.join(", ")));
        }
    }
    gaps
}

fn rtk_hook_matchers(root: &serde_json::Value, event_key: &str) -> Vec<String> {
    root.get("hooks")
        .and_then(|h| h.get(event_key))
        .and_then(|p| p.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| {
                    entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|hook| hook.get("command")?.as_str())
                        .any(|cmd| {
                            cmd == CLAUDE_HOOK_COMMAND
                                || is_claude_hook_command(cmd)
                                || cmd.contains(REWRITE_HOOK_FILE)
                        })
                })
                .map(|entry| {
                    entry
                        .get("matcher")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Matches on legacy rtk-rewrite.sh path OR new `rtk hook claude` command
pub(super) fn hook_already_present(
    root: &serde_json::Value,
    event_key: &str,
    hook_command: &str,
) -> bool {
    let event_array = match root
        .get("hooks")
        .and_then(|h| h.get(event_key))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    event_array
        .iter()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .any(|hook| {
            is_command_hook(hook, |cmd| {
                cmd == hook_command
                    || is_claude_hook_command(cmd)
                    || cmd.contains(REWRITE_HOOK_FILE)
            })
        })
}

/// Default mode: hook + slim RTK.md + @RTK.md reference
pub(super) fn run_default_mode(
    global: bool,
    patch_mode: PatchMode,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        // Local init: inject the awareness block into CLAUDE.md + generate
        // project-local filters template. The legacy full-instruction block
        // stays behind the explicit --claude-md opt-in.
        run_claude_md_mode_with(
            false,
            install_opencode,
            &rtk_block(&instructions_body(ctx.awareness)),
            ctx,
        )?;
        generate_project_filters_template(ctx)?;
        if !dry_run {
            print_project_scope_footer();
        }
        return Ok(());
    }

    let claude_dir = resolve_claude_dir()?;
    let rtk_md_path = claude_dir.join(RTK_MD);
    let claude_md_path = claude_dir.join(CLAUDE_MD);

    // 1. Migrate old hook script if present
    migrate_old_hook_script(ctx);

    // 2. Write RTK.md
    write_if_changed(&rtk_md_path, &instructions_body(ctx.awareness), RTK_MD, ctx)?;
    if dry_run {
        println!("[dry-run] awareness level: {}", ctx.awareness);
    }

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, ctx)?.then_some(path)
    } else {
        None
    };

    // 3. Patch CLAUDE.md (add @RTK.md, migrate if needed)
    let migrated = patch_claude_md(&claude_md_path, ctx)?;

    // 4. Print success message (skip in dry-run)
    if !dry_run {
        println!("\nRTK hook registered (global).\n");
        println!("  Command:   {}", CLAUDE_HOOK_COMMAND);
        println!(
            "  RTK.md:    {} (awareness: {})",
            rtk_md_path.display(),
            ctx.awareness
        );
        if let Some(path) = &opencode_plugin_path {
            println!("  OpenCode:  {}", path.display());
        }
        println!("  CLAUDE.md: @RTK.md reference added");

        if migrated {
            println!("\n  [ok] Migrated: removed 137-line RTK block from CLAUDE.md");
            println!(
                "              replaced with @RTK.md (awareness: {})",
                ctx.awareness
            );
        }
    }

    // 5. Patch settings.json with binary command — global registration from
    // the user pipeline; a project pipeline's extra events go into the
    // project's own .claude/settings.json, tied to the directory they came from.
    let patch_result = patch_settings_json_command(
        CLAUDE_HOOK_COMMAND,
        patch_mode,
        opencode_plugin_path.is_some(),
        ctx,
        HookInstallScope::UserGlobal,
    )?;
    if !global {
        patch_settings_json_command(
            CLAUDE_HOOK_COMMAND,
            patch_mode,
            false,
            ctx,
            HookInstallScope::ProjectExtra,
        )?;
    }

    // Report result
    if !dry_run {
        match patch_result {
            PatchResult::Patched => {
                // Already printed by patch_settings_json_command
            }
            PatchResult::AlreadyPresent => {
                println!("\n  settings.json: hook already present");
                if opencode_plugin_path.is_some() {
                    println!("  Restart Claude Code and OpenCode. Test with: git status");
                } else {
                    println!("  Restart Claude Code. Test with: git status");
                }
            }
            PatchResult::Declined | PatchResult::Skipped => {
                // Manual instructions already printed
            }
            PatchResult::WouldPatch => {
                // Cannot happen outside dry_run
            }
        }
    }

    // 6. Generate user-global filters template (~/.config/rtk/filters.toml)
    generate_global_filters_template(ctx)?;

    // 7. Proxy on-ramp: best-effort, never fails init.
    if matches!(
        patch_result,
        PatchResult::Patched | PatchResult::AlreadyPresent
    ) {
        #[cfg(feature = "engine")]
        {
            if let Err(e) = offer_proxy_onramp(&claude_dir.join(SETTINGS_JSON), patch_mode, ctx) {
                eprintln!("  [warn] proxy on-ramp skipped: {e}");
            }
            if let Err(e) = offer_delegation_onramp(patch_mode, ctx) {
                eprintln!("  [warn] delegation on-ramp skipped: {e}");
            }
        }
    }

    if !dry_run {
        println!(); // Final newline
    }

    Ok(())
}

/// Migrate old hook script to new binary command.
/// Deletes `~/.claude/hooks/rtk-rewrite.sh` and `.rtk-hook.sha256` if present,
/// and removes the stale settings.json entry so the new `rtk hook claude` entry
/// can be registered.
fn migrate_old_hook_script(ctx: InitContext) {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    if let Some(home) = user_dirs::home() {
        let old_hook = home
            .join(CLAUDE_DIR)
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE);
        if old_hook.exists() {
            if dry_run {
                println!(
                    "[dry-run] would migrate legacy hook script: {}",
                    old_hook.display()
                );
            // nosemgrep: filesystem-deletion
            } else if let Err(e) = std::fs::remove_file(&old_hook) {
                if verbose > 0 {
                    eprintln!("  [warn] Failed to remove old hook script: {e}");
                }
            } else {
                if verbose > 0 {
                    eprintln!("  [ok] Removed old hook script: {}", old_hook.display());
                }
                // Clean up the stale settings.json entry that pointed to the deleted script
                if let Err(e) = remove_legacy_settings_entries(ctx)
                    && verbose > 0
                {
                    eprintln!("  [warn] Failed to clean legacy settings.json entry: {e}");
                }
            }
        }
        // Remove legacy hash file
        let hash_file = home
            .join(CLAUDE_DIR)
            .join(HOOKS_SUBDIR)
            .join(".rtk-hook.sha256");
        if hash_file.exists() {
            if dry_run {
                println!(
                    "[dry-run] would remove legacy hash file: {}",
                    hash_file.display()
                );
            } else {
                // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
                let _ = std::fs::remove_file(&hash_file);
            }
        }
        // Remove Cursor legacy hook
        let cursor_hook = home.join(CURSOR_DIR).join("hooks").join(REWRITE_HOOK_FILE);
        if cursor_hook.exists() {
            if dry_run {
                println!(
                    "[dry-run] would remove legacy Cursor hook: {}",
                    cursor_hook.display()
                );
            } else {
                // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
                let _ = std::fs::remove_file(&cursor_hook);
            }
        }
    }
}

/// Remove only legacy `rtk-rewrite.sh` entries from settings.json.
/// Preserves any existing `rtk hook claude` entries (new format).
fn remove_legacy_settings_entries(ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join(SETTINGS_JSON);

    if !settings_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;
    let content = strip_leading_bom(&content);
    if content.trim().is_empty() {
        return Ok(());
    }

    let mut root: serde_json::Value = from_json_str(content)
        .with_context(|| format!("Failed to parse {}", settings_path.display()))?;

    if !remove_legacy_hook_entries_from_json(&mut root) {
        return Ok(());
    }

    if dry_run {
        println!(
            "[dry-run] would remove legacy rtk-rewrite.sh entry from {}",
            settings_path.display()
        );
        return Ok(());
    }

    // Backup before modifying
    let backup_path = settings_path.with_extension("json.bak");
    fs::copy(&settings_path, &backup_path)
        .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;

    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
    atomic_write(&settings_path, &serialized)?;

    if verbose > 0 {
        eprintln!("  [ok] Removed legacy rtk-rewrite.sh entry from settings.json");
    }
    Ok(())
}

/// Remove only legacy `rtk-rewrite.sh` hook entries from a parsed settings.json.
/// Returns true if any entries were removed.
/// Does NOT remove `rtk hook claude` entries — those are the new format.
pub(super) fn remove_legacy_hook_entries_from_json(root: &mut serde_json::Value) -> bool {
    remove_hook_entries(root, PRE_TOOL_USE_KEY, HookEntries::Grouped, |hook| {
        is_command_hook(hook, |cmd| cmd.contains(REWRITE_HOOK_FILE))
    })
}

/// Hook-only mode: just the hook, no RTK.md
pub(super) fn run_hook_only_mode(
    global: bool,
    patch_mode: PatchMode,
    install_opencode: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        eprintln!("[warn] Warning: --hook-only only makes sense with --global");
        eprintln!("    For local projects, use default mode or --claude-md");
        return Ok(());
    }

    // Migrate old hook script if present
    migrate_old_hook_script(ctx);

    let opencode_plugin_path = if install_opencode {
        let path = prepare_opencode_plugin_path()?;
        ensure_opencode_plugin_installed(&path, ctx)?.then_some(path)
    } else {
        None
    };

    if !dry_run {
        println!("\nRTK hook registered (hook-only mode).\n");
        println!("  Command: {}", CLAUDE_HOOK_COMMAND);
        if let Some(path) = &opencode_plugin_path {
            println!("  OpenCode: {}", path.display());
        }
        println!(
            "  Note: No RTK.md created. Claude won't know about meta commands (gain, discover, bypass)."
        );
    }

    // Patch settings.json with binary command (scope-matched, as above).
    let patch_result = patch_settings_json_command(
        CLAUDE_HOOK_COMMAND,
        patch_mode,
        opencode_plugin_path.is_some(),
        ctx,
        HookInstallScope::UserGlobal,
    )?;
    if !global {
        patch_settings_json_command(
            CLAUDE_HOOK_COMMAND,
            patch_mode,
            false,
            ctx,
            HookInstallScope::ProjectExtra,
        )?;
    }

    // Report result
    if !dry_run {
        match patch_result {
            PatchResult::Patched => {
                // Already printed by patch_settings_json_command
            }
            PatchResult::AlreadyPresent => {
                println!("\n  settings.json: hook already present");
                if opencode_plugin_path.is_some() {
                    println!("  Restart Claude Code and OpenCode. Test with: git status");
                } else {
                    println!("  Restart Claude Code. Test with: git status");
                }
            }
            PatchResult::Declined | PatchResult::Skipped => {
                // Manual instructions already printed
            }
            PatchResult::WouldPatch => {
                // Cannot happen outside dry_run
            }
        }
    }

    if !dry_run {
        println!(); // Final newline
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;

    #[test]
    fn test_patch_claude_md_adds_reference_when_only_mentioned_in_prose() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let claude_md = temp.path().join("CLAUDE.md");
        fs::write(&claude_md, "# Mine\n\n@RTK.md is my notes file.\n").expect("write");

        patch_claude_md(&claude_md, InitContext::default()).expect("patch");

        let content = fs::read_to_string(&claude_md).expect("read");
        assert_eq!(content, "# Mine\n\n@RTK.md is my notes file.\n\n@RTK.md\n");
    }

    #[test]
    fn test_hook_already_present_exact_match() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(hook_already_present(
            &json_content,
            PRE_TOOL_USE_KEY,
            hook_command
        ));
    }

    #[test]
    fn test_hook_already_present_different_path() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "~/.claude/hooks/rtk-rewrite.sh";
        // Should match on rtk-rewrite.sh substring
        assert!(hook_already_present(
            &json_content,
            PRE_TOOL_USE_KEY,
            hook_command
        ));
    }

    #[test]
    fn test_hook_not_present_empty() {
        let json_content = serde_json::json!({});
        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(
            &json_content,
            PRE_TOOL_USE_KEY,
            hook_command
        ));
    }

    #[test]
    fn test_hook_already_present_new_command() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": CLAUDE_HOOK_COMMAND
                    }]
                }]
            }
        });

        assert!(hook_already_present(
            &json_content,
            PRE_TOOL_USE_KEY,
            CLAUDE_HOOK_COMMAND
        ));
    }

    #[test]
    fn test_hook_already_present_absolute_new_command() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/opt/homebrew/bin/rtk hook claude",
                        "timeout": 5
                    }]
                }]
            }
        });

        assert!(hook_already_present(
            &json_content,
            PRE_TOOL_USE_KEY,
            CLAUDE_HOOK_COMMAND
        ));
    }

    #[test]
    fn test_hook_not_present_other_hooks() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(
            &json_content,
            PRE_TOOL_USE_KEY,
            hook_command
        ));
    }

    #[test]
    fn test_remove_hook_from_json() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        // Should have only one hook left
        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);

        // Check it's the other hook
        let command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(command, "/some/other/hook.sh");
    }

    #[test]
    fn test_remove_hook_from_json_new_command() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": CLAUDE_HOOK_COMMAND
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/some/other/hook.sh"
        );
    }

    #[test]
    fn test_remove_hook_from_json_absolute_new_command() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/opt/homebrew/bin/rtk hook claude"
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap(),
            "/some/other/hook.sh"
        );
    }

    #[test]
    fn test_remove_hook_when_not_present() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(!removed);
    }

    #[test]
    fn test_remove_legacy_hook_entries_strips_old_script() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(arr.is_empty());
    }

    #[test]
    fn test_remove_legacy_hook_entries_preserves_new_command() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": CLAUDE_HOOK_COMMAND
                        }]
                    }
                ]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, CLAUDE_HOOK_COMMAND);
    }

    #[test]
    fn test_remove_legacy_hook_entries_noop_when_no_legacy() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": CLAUDE_HOOK_COMMAND
                    }]
                }]
            }
        });

        assert!(!remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn test_remove_legacy_hook_entries_preserves_third_party_hooks() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "some-other-tool --hook"
                        }]
                    }
                ]
            }
        });

        assert!(remove_legacy_hook_entries_from_json(&mut root));
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, "some-other-tool --hook");
    }

    // #4604: the proxy on-ramp writes env.ANTHROPIC_BASE_URL; uninstall must
    // take it back, or Claude Code keeps calling a daemon nothing starts.
    #[test]
    fn test_uninstall_removes_rtk_proxy_base_url() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            let rtk_url = format!("http://{}", crate::llm_proxy::DEFAULT_BIND);
            fs::write(
                &settings,
                serde_json::to_string_pretty(&serde_json::json!({
                    "env": { "ANTHROPIC_BASE_URL": rtk_url, "OTHER": "kept" },
                    "hooks": { (SESSION_START_KEY): [{
                        "hooks": [{ "type": "command", "command": CLAUDE_HOOK_COMMAND }]
                    }]}
                }))
                .unwrap(),
            )
            .unwrap();

            assert_eq!(
                remove_settings_entries(InitContext::default()).unwrap(),
                SettingsRemoval {
                    hook: true,
                    proxy_base_url: true
                }
            );

            let root: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
            assert_eq!(root.pointer("/env/ANTHROPIC_BASE_URL"), None, "{root}");
            assert_eq!(
                root.pointer("/env/OTHER").and_then(|v| v.as_str()),
                Some("kept")
            );
            assert_eq!(root.get("hooks"), None, "{root}");
        });
    }

    #[test]
    fn test_uninstall_removes_rtk_proxy_base_url_without_hook() {
        // Choice 2 of the on-ramp (background service) writes the URL but no
        // SessionStart hook; the URL alone must still be taken back.
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            let rtk_url = format!("http://{}", crate::llm_proxy::DEFAULT_BIND);
            fs::write(
                &settings,
                serde_json::json!({ "env": { "ANTHROPIC_BASE_URL": rtk_url } }).to_string(),
            )
            .unwrap();

            assert_eq!(
                remove_settings_entries(InitContext::default()).unwrap(),
                SettingsRemoval {
                    hook: false,
                    proxy_base_url: true
                }
            );

            let root: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
            assert_eq!(root, serde_json::json!({}), "an emptied env is dropped");
        });
    }

    #[test]
    fn test_uninstall_keeps_a_foreign_base_url() {
        let mut root = serde_json::json!({
            "env": { "ANTHROPIC_BASE_URL": "https://gateway.example.com" }
        });
        assert!(!remove_proxy_env_from_json(&mut root));
        assert_eq!(
            root.pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str()),
            Some("https://gateway.example.com")
        );
    }

    #[test]
    fn test_global_default_mode_creates_artifacts() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            assert!(
                claude_dir.join(CLAUDE_MD).exists(),
                "CLAUDE.md must be created"
            );

            let settings = claude_dir.join(SETTINGS_JSON);
            assert!(settings.exists(), "settings.json must be created");
            let content = fs::read_to_string(&settings).unwrap();
            assert!(
                content.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );

            // Global mode also writes the user-wide filters template, through
            // `user_dirs::config`, which resolves under this test's own root.
            let filters = user_dirs::config()
                .expect("test build always resolves a config dir")
                .join(crate::core::constants::FILTERS_TOML);
            assert_eq!(
                fs::read_to_string(&filters).ok().as_deref(),
                Some(super::super::FILTERS_GLOBAL_TEMPLATE),
                "global filters template must be written at {}",
                filters.display()
            );
        });
    }

    #[test]
    fn test_patch_settings_json_dry_run_idempotency_and_backup() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |dir| {
            let path = dir.join(SETTINGS_JSON);
            let original = "\u{feff}{\"permissions\":{\"allow\":[\"Bash(ls)\"]},\"hooks\":{\"PreToolUse\":[{\"matcher\":\"Bash\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo user\"}]}]}}";
            fs::write(&path, original).unwrap();
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            assert!(matches!(
                patch_settings_json_command(
                    CLAUDE_HOOK_COMMAND,
                    PatchMode::Auto,
                    false,
                    dry,
                    HookInstallScope::UserGlobal,
                )
                .unwrap(),
                PatchResult::WouldPatch
            ));
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
            assert!(!path.with_extension("json.bak").exists());
            assert!(matches!(
                patch_settings_json_command(
                    CLAUDE_HOOK_COMMAND,
                    PatchMode::Auto,
                    false,
                    InitContext::default(),
                    HookInstallScope::UserGlobal,
                )
                .unwrap(),
                PatchResult::Patched
            ));
            let installed = fs::read_to_string(&path).unwrap();
            assert!(matches!(
                patch_settings_json_command(
                    CLAUDE_HOOK_COMMAND,
                    PatchMode::Auto,
                    false,
                    InitContext::default(),
                    HookInstallScope::UserGlobal,
                )
                .unwrap(),
                PatchResult::AlreadyPresent
            ));
            assert_eq!(fs::read_to_string(&path).unwrap(), installed);
            assert_eq!(
                fs::read_to_string(path.with_extension("json.bak")).unwrap(),
                original
            );
            let root = read_json_file(&path).unwrap().unwrap();
            assert_eq!(root["permissions"]["allow"][0], "Bash(ls)");
            assert_eq!(
                root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
                "echo user"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_backup_failure_preserves_original() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |dir| {
            let path = dir.join(SETTINGS_JSON);
            fs::write(&path, "{}").unwrap();
            fs::create_dir(path.with_extension("json.bak")).unwrap();
            let error = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
                HookInstallScope::UserGlobal,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(&path.display().to_string()));
            assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        });
    }

    #[test]
    fn test_global_default_mode_creates_missing_claude_dir() {
        let tmp = test_isolation::tempdir();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(
                claude_dir.exists(),
                "missing Claude config dir must be created"
            );
            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            assert!(
                claude_dir.join(CLAUDE_MD).exists(),
                "CLAUDE.md must be created"
            );
            assert!(
                claude_dir.join(SETTINGS_JSON).exists(),
                "settings.json must be created"
            );
        });
    }

    /// `--opencode` is installed after RTK.md. A missing Claude dir must not
    /// abort before that install.
    #[test]
    fn test_global_opencode_installs_when_claude_dir_missing() {
        use crate::hooks::constants::{OPENCODE_PLUGIN_FILE, PLUGIN_SUBDIR};

        let tmp = test_isolation::tempdir();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, true, InitContext::default()).unwrap();

            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            let plugin = super::super::opencode::resolve_opencode_dir()
                .expect("opencode dir resolves in a test build")
                .join(PLUGIN_SUBDIR)
                .join(OPENCODE_PLUGIN_FILE);
            assert!(
                plugin.starts_with(tmp.path()),
                "the plugin must land in this test's own directory, not {}",
                plugin.display()
            );
            assert!(
                plugin.exists(),
                "OpenCode plugin must be installed when ~/.claude was missing"
            );
        });
    }

    #[test]
    fn test_patch_settings_json_tolerates_utf8_bom() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            // Notepad and PowerShell 5.1 `Out-File -Encoding utf8` prepend a BOM.
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}{\"foo\": 1}").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
                HookInstallScope::UserGlobal,
            );
            assert!(
                result.is_ok(),
                "BOM-prefixed settings.json must not abort init: {:?}",
                result.err()
            );

            let content = fs::read_to_string(&settings).unwrap();
            let v: serde_json::Value = serde_json::from_str(&content).unwrap();
            assert_eq!(v["foo"], 1, "existing keys must survive the patch");
            assert!(
                content.contains(CLAUDE_HOOK_COMMAND),
                "hook must be installed"
            );
        });
    }

    #[cfg(feature = "engine")]
    #[test]
    fn test_patch_settings_json_bom_plus_invalid_json_still_errors() {
        // Stripping the BOM must not mask genuinely broken JSON: the
        // parse-error context has to survive so the user gets blamed for
        // the right thing.
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}{not valid json").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
                HookInstallScope::UserGlobal,
            );
            let err = result.expect_err("invalid JSON must still fail");
            assert!(
                err.to_string().contains("Failed to parse"),
                "error must carry the parse context, got: {err:#}"
            );
        });
    }

    /// The directory `init -g` runs from must never shape the global hook
    /// set. A trusted project pipeline that disables the rewrite must not
    /// make the patcher compute an empty event set and delete the user's
    /// global hooks.
    #[cfg(feature = "engine")]
    #[test]
    fn global_hooks_ignore_the_cwd_project_pipeline_and_never_delete() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("proj");
        fs::create_dir_all(project.join(".rtk")).unwrap();
        fs::write(
            project.join(".rtk").join("pipeline.toml"),
            "schema_version = 1\n[pipeline]\nname = \"innocent\"\n[override.rtk-rewrite]\nenabled = false\n",
        )
        .unwrap();

        let _entered = test_isolation::enter(&project);
        with_claude_dir_override(&tmp, |claude_dir| {
            crate::core::user_env::with_vars(
                &[("CI", Some("1")), ("RTK_TRUST_PROJECT_FILTERS", Some("1"))],
                || {
                    let settings = claude_dir.join(SETTINGS_JSON);
                    let seeded = serde_json::json!({"hooks": {
                        PRE_TOOL_USE_KEY: [{"matcher": "Bash", "hooks":
                            [{"type": "command", "command": CLAUDE_HOOK_COMMAND}]}],
                        POST_TOOL_USE_KEY: [{"matcher": "*", "hooks":
                            [{"type": "command", "command": CLAUDE_HOOK_COMMAND}]}],
                    }});
                    fs::write(&settings, serde_json::to_string_pretty(&seeded).unwrap()).unwrap();

                    let events = required_claude_events_scoped(HookInstallScope::UserGlobal);
                    assert_eq!(
                        events.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
                        vec![PRE_TOOL_USE_KEY],
                        "user scope must not see the project's rewrite override"
                    );

                    patch_settings_json_command(
                        CLAUDE_HOOK_COMMAND,
                        PatchMode::Auto,
                        false,
                        InitContext::default(),
                        HookInstallScope::UserGlobal,
                    )
                    .unwrap();

                    let root: serde_json::Value =
                        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
                    assert!(
                        hook_already_present(&root, PRE_TOOL_USE_KEY, CLAUDE_HOOK_COMMAND),
                        "the required hook must survive"
                    );
                    assert!(
                        hook_already_present(&root, POST_TOOL_USE_KEY, CLAUDE_HOOK_COMMAND),
                        "an entry the pipeline no longer requires is reported, never deleted"
                    );
                },
            );
        });
    }

    /// Scope-matched registration — a project pipeline's extra events land
    /// in the project's own .claude/settings.json, and an event the global
    /// file already carries is not registered a second time.
    #[cfg(feature = "engine")]
    #[test]
    fn project_posthook_registers_in_the_projects_own_settings() {
        let tmp = test_isolation::tempdir();
        let project = tmp.path().join("proj");
        fs::create_dir_all(project.join(".rtk")).unwrap();
        fs::write(
            project.join(".rtk").join("pipeline.toml"),
            "schema_version = 1\n[pipeline]\nextends = \"rtk:posthook\"\n",
        )
        .unwrap();

        let _entered = test_isolation::enter(&project);
        with_claude_dir_override(&tmp, |claude_dir| {
            crate::core::user_env::with_vars(
                &[("CI", Some("1")), ("RTK_TRUST_PROJECT_FILTERS", Some("1"))],
                || {
                    patch_settings_json_command(
                        CLAUDE_HOOK_COMMAND,
                        PatchMode::Auto,
                        false,
                        InitContext::default(),
                        HookInstallScope::UserGlobal,
                    )
                    .unwrap();
                    patch_settings_json_command(
                        CLAUDE_HOOK_COMMAND,
                        PatchMode::Auto,
                        false,
                        InitContext::default(),
                        HookInstallScope::ProjectExtra,
                    )
                    .unwrap();

                    let global: serde_json::Value = serde_json::from_str(
                        &fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap(),
                    )
                    .unwrap();
                    assert!(hook_already_present(
                        &global,
                        PRE_TOOL_USE_KEY,
                        CLAUDE_HOOK_COMMAND
                    ));
                    assert!(
                        !hook_already_present(&global, POST_TOOL_USE_KEY, CLAUDE_HOOK_COMMAND),
                        "a project-only event must not touch the global file"
                    );

                    let proj_settings = project.join(".claude").join(SETTINGS_JSON);
                    let proj: serde_json::Value =
                        serde_json::from_str(&fs::read_to_string(&proj_settings).unwrap()).unwrap();
                    assert!(
                        hook_already_present(&proj, POST_TOOL_USE_KEY, CLAUDE_HOOK_COMMAND),
                        "the project's extra event lives beside the pipeline that needs it"
                    );
                    assert!(
                        !hook_already_present(&proj, PRE_TOOL_USE_KEY, CLAUDE_HOOK_COMMAND),
                        "an event the global file covers is not doubled (the hook would run twice)"
                    );
                },
            );
        });
    }

    #[test]
    fn test_patch_settings_json_bom_only_file() {
        // U+FEFF is not whitespace, so the `content.trim().is_empty()`
        // empty-file guard does not catch a BOM-only file.
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let settings = claude_dir.join(SETTINGS_JSON);
            fs::write(&settings, "\u{feff}").unwrap();

            let result = patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                InitContext::default(),
                HookInstallScope::UserGlobal,
            );
            assert!(
                result.is_ok(),
                "BOM-only settings.json must be treated as empty: {:?}",
                result.err()
            );
        });
    }

    #[test]
    fn test_global_uninstall_removes_artifacts() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            uninstall(
                true,
                false,
                false,
                false,
                false,
                false,
                InitContext::default(),
            )
            .unwrap();

            assert!(!claude_dir.join(RTK_MD).exists(), "RTK.md must be removed");
            let settings_content =
                fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap_or_default();
            assert!(
                !settings_content.contains(CLAUDE_HOOK_COMMAND),
                "hook entry must be removed from settings.json"
            );
        });
    }

    #[test]
    fn test_global_default_mode_idempotent() {
        let tmp = test_isolation::tempdir();
        // Events follow the ACTIVE pipeline, so the developer's own installed
        // pipelines (e.g. one with after_tool rules) would add PostToolUse and
        // break the rtk:default baseline this test asserts — pin the pipeline
        // load to an empty config root.
        crate::core::user_env::with_path(
            "RTK_CONFIG_DIR",
            Some(tmp.path().join("rtk-config")),
            || {
                with_claude_dir_override(&tmp, |claude_dir| {
                    run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
                    run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

                    let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
                    let count = settings.matches(CLAUDE_HOOK_COMMAND).count();
                    assert_eq!(
                        count, 1,
                        "rtk:default registers PreToolUse only — exactly one hook entry"
                    );
                });
            },
        );
    }

    #[test]
    fn test_local_init_no_hook() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        let result = run_default_mode(false, PatchMode::Auto, false, InitContext::default());

        result.unwrap();
        assert!(
            tmp.path().join(CLAUDE_MD).exists(),
            "local CLAUDE.md must be created"
        );
        assert!(
            !tmp.path().join(SETTINGS_JSON).exists(),
            "settings.json must not be created for local init"
        );
    }

    #[test]
    fn test_global_hook_only_mode_creates_settings() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_hook_only_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "RTK.md must NOT be created in hook-only mode"
            );
            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            assert!(
                settings.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );
        });
    }

    #[test]
    fn test_global_hook_only_mode_creates_missing_claude_dir() {
        let tmp = test_isolation::tempdir();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            run_hook_only_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(
                claude_dir.exists(),
                "missing Claude config dir must be created"
            );
            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "RTK.md must NOT be created in hook-only mode"
            );
            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            assert!(
                settings.contains(CLAUDE_HOOK_COMMAND),
                "settings.json must contain hook command"
            );
        });
    }

    #[test]
    fn test_run_default_mode_dry_run_writes_nothing() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            run_default_mode(true, PatchMode::Auto, false, dry).unwrap();

            assert!(
                !claude_dir.join(RTK_MD).exists(),
                "dry-run must not create RTK.md"
            );
            assert!(
                !claude_dir.join(CLAUDE_MD).exists(),
                "dry-run must not create CLAUDE.md"
            );
            assert!(
                !claude_dir.join(SETTINGS_JSON).exists(),
                "dry-run must not create settings.json"
            );
        });
    }

    #[test]
    fn test_run_default_mode_dry_run_does_not_create_missing_claude_dir() {
        let tmp = test_isolation::tempdir();
        with_missing_claude_dir_override(&tmp, |claude_dir| {
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            run_default_mode(true, PatchMode::Auto, false, dry).unwrap();

            assert!(
                !claude_dir.exists(),
                "dry-run must not create missing Claude config dir"
            );
        });
    }

    #[test]
    fn test_uninstall_dry_run_preserves_artifacts() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            // Stage a real install first
            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();
            assert!(claude_dir.join(RTK_MD).exists());
            assert!(claude_dir.join(SETTINGS_JSON).exists());

            let settings_before = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            let rtk_md_before = fs::read_to_string(claude_dir.join(RTK_MD)).unwrap();

            // Dry-run uninstall
            let dry = InitContext {
                dry_run: true,
                ..Default::default()
            };
            uninstall(true, false, false, false, false, false, dry).unwrap();

            // Files must still exist with identical content
            assert!(
                claude_dir.join(RTK_MD).exists(),
                "dry-run uninstall must not remove RTK.md"
            );
            assert!(
                claude_dir.join(SETTINGS_JSON).exists(),
                "dry-run uninstall must not remove settings.json"
            );
            assert_eq!(
                fs::read_to_string(claude_dir.join(RTK_MD)).unwrap(),
                rtk_md_before,
                "dry-run uninstall must not modify RTK.md"
            );
            assert_eq!(
                fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap(),
                settings_before,
                "dry-run uninstall must not modify settings.json"
            );
        });
    }

    #[test]
    fn test_write_if_changed_switches_awareness_level() {
        let temp = test_isolation::tempdir();
        let rtk_md_path = temp.path().join("RTK.md");

        let default_ctx = InitContext::default();
        assert!(
            write_if_changed(
                &rtk_md_path,
                awareness_content(default_ctx.awareness),
                RTK_MD,
                default_ctx
            )
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&rtk_md_path).unwrap(),
            RTK_AWARENESS_DEFAULT
        );

        let high_ctx = InitContext {
            awareness: AwarenessLevel::High,
            ..Default::default()
        };
        assert!(
            write_if_changed(
                &rtk_md_path,
                awareness_content(high_ctx.awareness),
                RTK_MD,
                high_ctx
            )
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&rtk_md_path).unwrap(),
            RTK_AWARENESS_HIGH
        );

        assert!(
            !write_if_changed(
                &rtk_md_path,
                awareness_content(high_ctx.awareness),
                RTK_MD,
                high_ctx
            )
            .unwrap()
        );
    }

    #[test]
    fn test_claude_md_mode_creates_full_injection() {
        // Just verify RTK_INSTRUCTIONS constant has the right content
        assert!(RTK_INSTRUCTIONS.contains(RTK_BLOCK_START));
        assert!(RTK_INSTRUCTIONS.contains("rtk cargo test"));
        assert!(RTK_INSTRUCTIONS.contains(RTK_BLOCK_END));
        assert!(RTK_INSTRUCTIONS.len() > 4000);
    }
    #[test]
    fn test_resolve_claude_dir_prefers_rtk_override() {
        let result = resolve_claude_dir_from(
            Some(PathBuf::from("/custom/rtk-claude")),
            Some(PathBuf::from("/home/user")),
        )
        .unwrap();
        assert_eq!(result, PathBuf::from("/custom/rtk-claude"));
    }

    #[test]
    fn test_resolve_claude_dir_uses_claude_config_dir() {
        let result = resolve_claude_dir_from(
            Some(PathBuf::from("/custom/claude-config")),
            Some(PathBuf::from("/home/user")),
        )
        .unwrap();
        assert_eq!(result, PathBuf::from("/custom/claude-config"));
    }

    #[test]
    fn test_resolve_claude_dir_falls_back_to_home() {
        let result = resolve_claude_dir_from(None, Some(PathBuf::from("/home/user"))).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/.claude"));
    }

    #[test]
    fn test_resolve_claude_dir_ignores_empty_overrides() {
        let empty =
            resolve_claude_dir_from(Some(PathBuf::new()), Some(PathBuf::from("/home/user")))
                .unwrap();
        assert_eq!(empty, PathBuf::from("/home/user/.claude"));
    }

    #[test]
    fn test_resolve_claude_dir_errors_without_home() {
        let err = resolve_claude_dir_from(None, None).unwrap_err();
        assert!(err.to_string().contains("Cannot determine Claude config"));
    }

    #[test]
    fn test_upgrade_from_claude_md_to_hook_mode() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            run_claude_md_mode(true, false, InitContext::default()).unwrap();
            let claude_md_content = fs::read_to_string(claude_dir.join(CLAUDE_MD)).unwrap();
            assert!(
                claude_md_content.contains(RTK_BLOCK_START),
                "pre-condition: old block must exist"
            );

            run_default_mode(true, PatchMode::Auto, false, InitContext::default()).unwrap();

            assert!(claude_dir.join(RTK_MD).exists(), "RTK.md must be created");
            let settings = fs::read_to_string(claude_dir.join(SETTINGS_JSON)).unwrap();
            assert!(
                settings.contains(CLAUDE_HOOK_COMMAND),
                "hook must be in settings.json after upgrade"
            );
        });
    }

    #[test]
    fn test_uninstall_integration_claude_md_only() {
        let (cleaned, did_remove) = remove_rtk_block(RTK_INSTRUCTIONS);
        assert!(did_remove, "remove_rtk_block must succeed for valid block");
        assert!(
            cleaned.trim().is_empty(),
            "CLAUDE.md with only RTK content should be empty after removal"
        );
    }

    #[test]
    fn test_claude_md_mode_refuses_malformed_block() {
        // Mirrors `copilot::tests::test_copilot_init_refuses_malformed_block`: a malformed
        // CLAUDE.md previously emitted a warning and exited 0, silently
        // skipping the OpenCode plugin step. The shared `write_rtk_block`
        // dispatcher now bails for both paths.
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let claude_md = claude_dir.join(CLAUDE_MD);
            let malformed = format!(
                "# Existing notes\n\n{}\nincomplete RTK block\n",
                RTK_BLOCK_START
            );
            fs::write(&claude_md, &malformed).unwrap();

            let result = run_claude_md_mode(true, false, InitContext::default());

            assert!(
                result.is_err(),
                "Malformed CLAUDE.md must cause a hard error, not silent skip"
            );

            let after = fs::read_to_string(&claude_md).unwrap();
            assert_eq!(after, malformed, "File must not be modified when malformed");
        });
    }
}
