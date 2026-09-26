//! Droid agent: hook install/uninstall helpers.

use super::*;
use crate::hooks::constants::{
    DROID_DIR, DROID_EXECUTE_MATCHER, DROID_HOME_ENV, DROID_HOOK_COMMAND, DROID_HOOKS_FILE,
    DROID_HOOKS_SUBDIR, DROID_SETTINGS_FILE, PRE_TOOL_USE_KEY,
};
use std::ffi::OsString;

// Factory Droid support

/// Resolve Droid config directory, honouring `FACTORY_HOME_OVERRIDE`.
///
/// Droid resolves its home as `$FACTORY_HOME_OVERRIDE || $HOME`, then joins
/// `.factory` onto it (verified against Droid v0.164.0).
/// - Global: `$FACTORY_HOME_OVERRIDE/.factory` or `~/.factory`.
/// - Project: caller passes `.factory` relative to project root.
fn resolve_droid_dir() -> Result<PathBuf> {
    resolve_droid_dir_from_env(dirs::home_dir(), std::env::var_os(DROID_HOME_ENV))
}

/// Unlike the other agents, `FACTORY_HOME_OVERRIDE` replaces the home directory and
/// `.factory` is still appended, mirroring Droid's own resolution.
fn resolve_droid_dir_from_env(
    home_dir: Option<PathBuf>,
    factory_home_override: Option<OsString>,
) -> Result<PathBuf> {
    factory_home_override
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or(home_dir)
        .map(|home| home.join(DROID_DIR))
        .context("Cannot determine Droid config directory. Set $FACTORY_HOME_OVERRIDE or $HOME.")
}

/// How hook events are stored in a Droid config file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DroidLayout {
    /// `hooks.json`: the event map (`PreToolUse`, …) is the file's root object.
    Root,
    /// `settings.json`: the event map lives under a top-level `hooks` key.
    Nested,
}

struct DroidHookFile {
    path: PathBuf,
    layout: DroidLayout,
}

/// Every file Droid may read PreToolUse hooks from, in its own precedence
/// order: root `hooks.json`, legacy `hooks/hooks.json` (only read when the
/// root file is absent), then the `hooks` key of `settings.json` (merged
/// under `hooks.json` per event key).
fn droid_hook_file_candidates(droid_dir: &Path) -> [DroidHookFile; 3] {
    [
        DroidHookFile {
            path: droid_dir.join(DROID_HOOKS_FILE),
            layout: DroidLayout::Root,
        },
        DroidHookFile {
            path: droid_dir.join(DROID_HOOKS_SUBDIR).join(DROID_HOOKS_FILE),
            layout: DroidLayout::Root,
        },
        DroidHookFile {
            path: droid_dir.join(DROID_SETTINGS_FILE),
            layout: DroidLayout::Nested,
        },
    ]
}

/// The JSON object holding hook events for the given layout, if present.
fn droid_events(root: &serde_json::Value, layout: DroidLayout) -> &serde_json::Value {
    match layout {
        DroidLayout::Root => root,
        DroidLayout::Nested => root.get("hooks").unwrap_or(&serde_json::Value::Null),
    }
}

fn droid_has_pre_tool_use(root: &serde_json::Value, layout: DroidLayout) -> bool {
    droid_events(root, layout)
        .get(PRE_TOOL_USE_KEY)
        .and_then(|p| p.as_array())
        .is_some_and(|arr| !arr.is_empty())
}

/// Pick the file whose `PreToolUse` hooks Droid will actually run.
///
/// Droid loads root `hooks.json` (falling back to legacy `hooks/hooks.json`
/// when the root file is absent) and merges it OVER the `hooks` key of
/// `settings.json`, per event key (verified against Droid v0.164.0).
/// Installing into a shadowed file silently does nothing, and adding
/// `PreToolUse` to `hooks.json` would shadow a user's live `settings.json`
/// hooks. Hence:
/// 1. the live `hooks.json`, when it already defines `PreToolUse`;
/// 2. else `settings.json`, when its `hooks.PreToolUse` is live;
/// 3. else the live `hooks.json`, when one exists;
/// 4. else create the canonical root `hooks.json` (where Droid's own
///    `/hooks` UI writes).
fn resolve_droid_install_target(droid_dir: &Path) -> Result<DroidHookFile> {
    let root = droid_dir.join(DROID_HOOKS_FILE);
    let legacy = droid_dir.join(DROID_HOOKS_SUBDIR).join(DROID_HOOKS_FILE);
    let settings = droid_dir.join(DROID_SETTINGS_FILE);

    let live_hooks_json = if root.exists() {
        Some(root.clone())
    } else if legacy.exists() {
        Some(legacy)
    } else {
        None
    };

    if let Some(path) = &live_hooks_json
        && let Some(json) = read_json_file(path)?
        && droid_has_pre_tool_use(&json, DroidLayout::Root)
    {
        return Ok(DroidHookFile {
            path: path.clone(),
            layout: DroidLayout::Root,
        });
    }

    if let Some(json) = read_json_file(&settings)?
        && droid_has_pre_tool_use(&json, DroidLayout::Nested)
    {
        return Ok(DroidHookFile {
            path: settings,
            layout: DroidLayout::Nested,
        });
    }

    Ok(DroidHookFile {
        path: live_hooks_json.unwrap_or(root),
        layout: DroidLayout::Root,
    })
}

/// Install Factory Droid PreToolUse hook.
///
/// - Global (`-g`): under `~/.factory` (or `$FACTORY_HOME_OVERRIDE/.factory`).
/// - Project: under `<cwd>/.factory` so the hook can be committed.
pub fn run_droid_mode(global: bool, ctx: InitContext) -> Result<()> {
    let droid_dir = if global {
        resolve_droid_dir()?
    } else {
        std::env::current_dir()
            .context("Failed to read current directory")?
            .join(DROID_DIR)
    };
    run_droid_mode_at(&droid_dir, global, ctx)
}

fn run_droid_mode_at(droid_dir: &Path, global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;

    let target = resolve_droid_install_target(droid_dir)?;

    if !dry_run {
        let dir = target.path.parent().unwrap_or(droid_dir);
        fs::create_dir_all(dir).with_context(|| {
            format!("Failed to create Droid config directory: {}", dir.display())
        })?;
    }

    let patched = patch_droid_hook_file(&target, ctx)?;

    // Migrate stale copies (e.g. an earlier RTK install into settings.json
    // that hooks.json now shadows) so exactly one live entry remains.
    // Best-effort: a corrupt secondary file must not block the install.
    for candidate in droid_hook_file_candidates(droid_dir) {
        if candidate.path == target.path {
            continue;
        }
        match remove_droid_hook_from_file(&candidate, ctx) {
            Ok(true) => println!(
                "  Removed stale RTK entry from {}",
                candidate.path.display()
            ),
            Ok(false) => {}
            Err(e) => eprintln!("rtk: warning: {e:#}"),
        }
    }

    if dry_run {
        print_dry_run_footer();
    } else {
        let scope = if global { "global" } else { "project" };
        println!("\nFactory Droid hook registered ({scope}).\n");
        println!("  Command:    {}", DROID_HOOK_COMMAND);
        println!("  Hooks file: {}", target.path.display());
        if patched {
            println!("  RTK PreToolUse entry added");
        } else {
            println!("  RTK PreToolUse entry already present");
        }
        println!("  Restart Droid. Test with: git status\n");
    }

    Ok(())
}

/// Insert RTK PreToolUse entry into a Droid hook file.
/// Returns true if the file was modified.
fn patch_droid_hook_file(file: &DroidHookFile, ctx: InitContext) -> Result<bool> {
    let InitContext { verbose, .. } = ctx;
    let path = &file.path;
    let mut root = read_json_file(path)?.unwrap_or_else(|| serde_json::json!({}));

    if droid_hook_already_present(&root, file.layout) {
        if verbose > 0 {
            eprintln!("{}: RTK hook already present", path.display());
        }
        return Ok(false);
    }

    insert_droid_hook_entry(&mut root, file.layout)?;

    update_json_file(
        path,
        &root,
        ctx,
        "Droid hook file",
        &format!("[dry-run] would patch Droid hook file: {}", path.display()),
        true,
        Written::Backup,
    )?;
    Ok(true)
}

/// Check if the RTK PreToolUse Execute hook is already in a Droid hook file.
fn droid_hook_already_present(root: &serde_json::Value, layout: DroidLayout) -> bool {
    let pre = match droid_events(root, layout)
        .get(PRE_TOOL_USE_KEY)
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    pre.iter().any(|matcher_entry| {
        let hooks = matcher_entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|arr| arr.as_slice())
            .unwrap_or(&[]);
        hooks.iter().any(|hook| {
            hook.get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|cmd| cmd == DROID_HOOK_COMMAND)
        })
    })
}

/// Insert the RTK Execute matcher into a Droid hook file.
fn insert_droid_hook_entry(root: &mut serde_json::Value, layout: DroidLayout) -> Result<()> {
    let root_obj = match root.as_object_mut() {
        Some(obj) => obj,
        None => {
            *root = serde_json::json!({});
            root.as_object_mut().expect("just-created json object")
        }
    };

    let events = match layout {
        DroidLayout::Root => root_obj,
        DroidLayout::Nested => root_obj
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("hooks value is not an object")?,
    };

    let pre_tool_use = events
        .entry(PRE_TOOL_USE_KEY)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .context("PreToolUse value is not an array")?;

    // Reuse the existing Execute matcher group if one exists, otherwise create
    // a new one so we don't trample user-supplied hooks on the same matcher.
    for entry in pre_tool_use.iter_mut() {
        let matcher = entry
            .get("matcher")
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        if matcher == DROID_EXECUTE_MATCHER
            && let Some(hook_array) = entry.get_mut("hooks").and_then(|h| h.as_array_mut())
        {
            hook_array.push(serde_json::json!({
                "type": "command",
                "command": DROID_HOOK_COMMAND
            }));
            return Ok(());
        }
    }

    pre_tool_use.push(serde_json::json!({
        "matcher": DROID_EXECUTE_MATCHER,
        "hooks": [
            { "type": "command", "command": DROID_HOOK_COMMAND }
        ]
    }));
    Ok(())
}

/// Uninstall Factory Droid integration: strip RTK hook entry from settings.json.
pub fn uninstall_droid(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let droid_dir = if global {
        resolve_droid_dir()?
    } else {
        std::env::current_dir()
            .context("Failed to read current directory")?
            .join(DROID_DIR)
    };
    let removed = uninstall_droid_at(&droid_dir, ctx)?;

    if removed.is_empty() {
        println!("RTK Droid support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Factory Droid:"
        } else {
            "RTK uninstalled for Factory Droid:"
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

fn uninstall_droid_at(droid_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for candidate in droid_hook_file_candidates(droid_dir) {
        match remove_droid_hook_from_file(&candidate, ctx) {
            Ok(true) => {
                removed.push(format!("Droid hook file: {}", candidate.path.display()));
            }
            Ok(false) => {}
            Err(error) => errors.push(format!("{}: {error:#}", candidate.path.display())),
        }
    }

    if !errors.is_empty() {
        return Err(anyhow::anyhow!(
            "Failed to uninstall RTK from one or more Droid hook files:\n  - {}",
            errors.join("\n  - ")
        ));
    }

    Ok(removed)
}

/// Strip the RTK entry from one Droid hook file. Returns true if the file
/// held an RTK entry (and was rewritten, unless dry-run).
fn remove_droid_hook_from_file(file: &DroidHookFile, ctx: InitContext) -> Result<bool> {
    let path = &file.path;

    let mut root = match read_json_file(path)? {
        Some(v) => v,
        None => return Ok(false),
    };

    if !remove_droid_hook_from_json(&mut root, file.layout) {
        return Ok(false);
    }

    update_json_file(
        path,
        &root,
        ctx,
        "Droid hook file",
        &format!(
            "[dry-run] would remove RTK entry from Droid hook file: {}",
            path.display()
        ),
        false,
        Written::Line(format!("Removed RTK hook from {}", path.display())),
    )?;
    Ok(true)
}

fn remove_droid_hook_from_json(root: &mut serde_json::Value, layout: DroidLayout) -> bool {
    let events_obj = match layout {
        DroidLayout::Root => root.as_object_mut(),
        DroidLayout::Nested => root.get_mut("hooks").and_then(|h| h.as_object_mut()),
    };
    let events_obj = match events_obj {
        Some(o) => o,
        None => return false,
    };

    let pre_tool_use = match events_obj
        .get_mut(PRE_TOOL_USE_KEY)
        .and_then(|p| p.as_array_mut())
    {
        Some(arr) => arr,
        None => return false,
    };

    let mut modified = false;

    for entry in pre_tool_use.iter_mut() {
        if let Some(hook_arr) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            let original = hook_arr.len();
            hook_arr.retain(|hook| {
                hook.get("command")
                    .and_then(|c| c.as_str())
                    .is_none_or(|cmd| cmd != DROID_HOOK_COMMAND)
            });
            if hook_arr.len() < original {
                modified = true;
            }
        }
    }

    // Drop matcher entries that lost all their hooks.
    let before = pre_tool_use.len();
    pre_tool_use.retain(|entry| {
        entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(true)
    });
    if pre_tool_use.len() < before {
        modified = true;
    }

    // Drop the PreToolUse key once empty; in settings.json also drop the
    // then-empty `hooks` object.
    if pre_tool_use.is_empty() {
        events_obj.remove(PRE_TOOL_USE_KEY);
        modified = true;
    }
    if layout == DroidLayout::Nested
        && root
            .get("hooks")
            .and_then(|h| h.as_object())
            .is_some_and(|o| o.is_empty())
        && let Some(obj) = root.as_object_mut()
    {
        obj.remove("hooks");
    }

    modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_droid_dir_prefers_home_override() {
        // FACTORY_HOME_OVERRIDE replaces the HOME directory; `.factory` is
        // appended to it (mirrors Droid's own resolution, v0.164.0).
        let override_home = OsString::from("/custom/home");
        let resolved =
            resolve_droid_dir_from_env(Some(PathBuf::from("/tmp/home")), Some(override_home))
                .unwrap();
        assert_eq!(resolved, PathBuf::from("/custom/home/.factory"));
    }

    #[test]
    fn test_resolve_droid_dir_falls_back_to_home() {
        let home = PathBuf::from("/tmp/home");
        let resolved =
            resolve_droid_dir_from_env(Some(home.clone()), Some(OsString::new())).unwrap();
        assert_eq!(resolved, home.join(".factory"));
    }

    #[test]
    fn test_insert_droid_hook_entry_empty_nested() {
        let mut root = serde_json::json!({});
        insert_droid_hook_entry(&mut root, DroidLayout::Nested).unwrap();
        let pre = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["matcher"], "Execute");
        let hooks = pre[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks[0]["type"], "command");
        assert_eq!(hooks[0]["command"], DROID_HOOK_COMMAND);
    }

    #[test]
    fn test_insert_droid_hook_entry_empty_root() {
        // hooks.json holds the event map at the file root — no `hooks` wrapper.
        let mut root = serde_json::json!({});
        insert_droid_hook_entry(&mut root, DroidLayout::Root).unwrap();
        assert!(
            root.get("hooks").is_none(),
            "no hooks wrapper in hooks.json"
        );
        let pre = root["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["matcher"], "Execute");
        assert_eq!(pre[0]["hooks"][0]["command"], DROID_HOOK_COMMAND);
    }

    #[test]
    fn test_insert_droid_hook_entry_reuses_execute_group() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Execute",
                    "hooks": [{ "type": "command", "command": "echo user-hook" }]
                }]
            }
        });
        insert_droid_hook_entry(&mut root, DroidLayout::Nested).unwrap();
        let pre = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1, "must not duplicate the matcher entry");
        let hooks = pre[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0]["command"], "echo user-hook");
        assert_eq!(hooks[1]["command"], DROID_HOOK_COMMAND);
    }

    #[test]
    fn test_droid_hook_already_present_detects_rtk() {
        let root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Execute",
                    "hooks": [{ "type": "command", "command": DROID_HOOK_COMMAND }]
                }]
            }
        });
        assert!(droid_hook_already_present(&root, DroidLayout::Nested));
        // The same document read with the Root layout must NOT match: in
        // hooks.json a top-level `hooks` key is not the event map.
        assert!(!droid_hook_already_present(&root, DroidLayout::Root));
    }

    #[test]
    fn test_droid_hook_already_present_false_for_other_command() {
        let root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Execute",
                    "hooks": [{ "type": "command", "command": "echo unrelated" }]
                }]
            }
        });
        assert!(!droid_hook_already_present(&root, DroidLayout::Nested));
    }

    #[test]
    fn test_remove_droid_hook_keeps_other_hooks() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Execute",
                    "hooks": [
                        { "type": "command", "command": "echo user-hook" },
                        { "type": "command", "command": DROID_HOOK_COMMAND }
                    ]
                }]
            }
        });
        assert!(remove_droid_hook_from_json(&mut root, DroidLayout::Nested));
        let hooks = root["hooks"]["PreToolUse"][0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "echo user-hook");
    }

    #[test]
    fn test_remove_droid_hook_drops_empty_matcher() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Execute",
                    "hooks": [
                        { "type": "command", "command": DROID_HOOK_COMMAND }
                    ]
                }]
            }
        });
        assert!(remove_droid_hook_from_json(&mut root, DroidLayout::Nested));
        assert!(
            root.get("hooks").is_none(),
            "hooks key should be removed when empty"
        );
    }

    #[test]
    fn test_remove_droid_hook_root_layout() {
        let mut root = serde_json::json!({
            "PreToolUse": [{
                "matcher": "Execute",
                "hooks": [{ "type": "command", "command": DROID_HOOK_COMMAND }]
            }],
            "PostToolUse": [{ "matcher": "Edit", "hooks": [] }]
        });
        assert!(remove_droid_hook_from_json(&mut root, DroidLayout::Root));
        assert!(root.get("PreToolUse").is_none());
        assert!(
            root.get("PostToolUse").is_some(),
            "unrelated events must survive"
        );
    }

    #[test]
    fn test_droid_target_defaults_to_hooks_json() {
        // Fresh setup: the canonical hooks.json is created (Droid's own
        // /hooks UI location), not the settings.json fallback.
        let temp = TempDir::new().unwrap();
        let droid_dir = temp.path().join(".factory");
        let target = resolve_droid_install_target(&droid_dir).unwrap();
        assert_eq!(target.path, droid_dir.join("hooks.json"));
        assert!(target.layout == DroidLayout::Root);
    }

    #[test]
    fn test_droid_target_prefers_hooks_json_with_pre_tool_use() {
        // hooks.json defines PreToolUse: it shadows settings.json's
        // PreToolUse entirely, so RTK must ride the hooks.json array.
        let temp = TempDir::new().unwrap();
        let droid_dir = temp.path().join(".factory");
        fs::create_dir_all(&droid_dir).unwrap();
        fs::write(
            droid_dir.join("hooks.json"),
            r#"{"PreToolUse":[{"matcher":"Execute","hooks":[{"type":"command","command":"echo user"}]}]}"#,
        )
        .unwrap();
        fs::write(
            droid_dir.join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Execute","hooks":[{"type":"command","command":"echo shadowed"}]}]}}"#,
        )
        .unwrap();
        let target = resolve_droid_install_target(&droid_dir).unwrap();
        assert_eq!(target.path, droid_dir.join("hooks.json"));
    }

    #[test]
    fn test_droid_target_uses_settings_when_its_pre_tool_use_is_live() {
        // hooks.json exists but has no PreToolUse key, so settings.json's
        // PreToolUse is live; adding PreToolUse to hooks.json would shadow
        // (silently disable) the user's settings hooks.
        let temp = TempDir::new().unwrap();
        let droid_dir = temp.path().join(".factory");
        fs::create_dir_all(&droid_dir).unwrap();
        fs::write(droid_dir.join("hooks.json"), r#"{"PostToolUse":[]}"#).unwrap();
        fs::write(
            droid_dir.join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Execute","hooks":[{"type":"command","command":"echo user"}]}]}}"#,
        )
        .unwrap();
        let target = resolve_droid_install_target(&droid_dir).unwrap();
        assert_eq!(target.path, droid_dir.join("settings.json"));
        assert!(target.layout == DroidLayout::Nested);
    }

    #[test]
    fn test_droid_target_uses_legacy_hooks_json_when_root_absent() {
        // Droid still reads .factory/hooks/hooks.json when the root file is
        // absent; creating a root hooks.json would shadow the whole legacy
        // file, so RTK patches the legacy one.
        let temp = TempDir::new().unwrap();
        let droid_dir = temp.path().join(".factory");
        let legacy_dir = droid_dir.join("hooks");
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::write(
            legacy_dir.join("hooks.json"),
            r#"{"PreToolUse":[{"matcher":"Execute","hooks":[{"type":"command","command":"echo user"}]}]}"#,
        )
        .unwrap();
        let target = resolve_droid_install_target(&droid_dir).unwrap();
        assert_eq!(target.path, legacy_dir.join("hooks.json"));
    }

    #[test]
    fn test_droid_install_then_uninstall_round_trip() {
        let temp = TempDir::new().unwrap();
        let droid_dir = temp.path().join(".factory");
        let ctx = InitContext {
            verbose: 0,
            dry_run: false,
            ..Default::default()
        };

        run_droid_mode_at(&droid_dir, true, ctx).unwrap();
        let hooks_json = droid_dir.join("hooks.json");
        assert!(hooks_json.exists(), "hooks.json should be created");

        // Second run is a no-op (idempotent).
        run_droid_mode_at(&droid_dir, true, ctx).unwrap();
        let content = fs::read_to_string(&hooks_json).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        let hooks = v["PreToolUse"][0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1, "install must be idempotent");

        // Uninstall wipes the entry.
        let removed = uninstall_droid_at(&droid_dir, ctx).unwrap();
        assert!(!removed.is_empty());
        let post = fs::read_to_string(&hooks_json).unwrap();
        let v: serde_json::Value = serde_json::from_str(&post).unwrap();
        assert!(v.get("PreToolUse").is_none());
    }

    #[test]
    fn test_droid_install_migrates_stale_settings_entry() {
        // Simulate an old RTK install in settings.json now shadowed by a
        // user-created hooks.json: install must move RTK into hooks.json and
        // strip the dead settings.json copy.
        let temp = TempDir::new().unwrap();
        let droid_dir = temp.path().join(".factory");
        fs::create_dir_all(&droid_dir).unwrap();
        fs::write(
            droid_dir.join("hooks.json"),
            r#"{"PreToolUse":[{"matcher":"Execute","hooks":[{"type":"command","command":"echo user"}]}]}"#,
        )
        .unwrap();
        fs::write(
            droid_dir.join("settings.json"),
            format!(
                r#"{{"model":"custom","hooks":{{"PreToolUse":[{{"matcher":"Execute","hooks":[{{"type":"command","command":"{}"}}]}}]}}}}"#,
                DROID_HOOK_COMMAND
            ),
        )
        .unwrap();

        let ctx = InitContext {
            verbose: 0,
            dry_run: false,
            ..Default::default()
        };
        run_droid_mode_at(&droid_dir, true, ctx).unwrap();

        let hooks: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(droid_dir.join("hooks.json")).unwrap())
                .unwrap();
        assert!(
            droid_hook_already_present(&hooks, DroidLayout::Root),
            "RTK entry must now live in hooks.json"
        );
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(droid_dir.join("settings.json")).unwrap())
                .unwrap();
        assert!(
            !droid_hook_already_present(&settings, DroidLayout::Nested),
            "stale settings.json entry must be removed"
        );
        assert_eq!(settings["model"], "custom", "user settings must survive");
    }
    #[test]
    fn test_remove_droid_hook_propagates_backup_failure_and_preserves_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(DROID_HOOKS_FILE);
        let original = serde_json::to_string_pretty(&serde_json::json!({
            "PreToolUse": [{
                "matcher": DROID_EXECUTE_MATCHER,
                "hooks": [{ "type": "command", "command": DROID_HOOK_COMMAND }]
            }]
        }))
        .unwrap();
        fs::write(&path, &original).unwrap();
        fs::create_dir(path.with_extension("json.bak")).unwrap();

        let err = remove_droid_hook_from_file(
            &DroidHookFile {
                path: path.clone(),
                layout: DroidLayout::Root,
            },
            InitContext::default(),
        )
        .unwrap_err();

        assert!(format!("{err:#}").contains("backup"));
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn test_uninstall_droid_continues_after_candidate_failure() {
        let temp = TempDir::new().unwrap();
        let droid_dir = temp.path().join(DROID_DIR);
        let root_path = droid_dir.join(DROID_HOOKS_FILE);
        let legacy_path = droid_dir.join(DROID_HOOKS_SUBDIR).join(DROID_HOOKS_FILE);
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();

        let hook_json = serde_json::to_string_pretty(&serde_json::json!({
            "PreToolUse": [{
                "matcher": DROID_EXECUTE_MATCHER,
                "hooks": [{ "type": "command", "command": DROID_HOOK_COMMAND }]
            }]
        }))
        .unwrap();
        fs::write(&root_path, &hook_json).unwrap();
        fs::write(&legacy_path, &hook_json).unwrap();
        fs::create_dir(root_path.with_extension("json.bak")).unwrap();

        let err = uninstall_droid_at(&droid_dir, InitContext::default()).unwrap_err();

        assert!(format!("{err:#}").contains(&root_path.display().to_string()));
        assert_eq!(fs::read_to_string(&root_path).unwrap(), hook_json);
        let legacy: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&legacy_path).unwrap()).unwrap();
        assert!(!droid_hook_already_present(&legacy, DroidLayout::Root));
        assert!(legacy_path.with_extension("json.bak").exists());
    }
}
