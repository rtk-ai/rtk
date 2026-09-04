//! Gemini agent: hook install/uninstall helpers.

use super::*;

// ─── Gemini CLI support ───────────────────────────────────────────

/// Gemini hook wrapper script — delegates to `rtk hook gemini`
pub(crate) const GEMINI_HOOK_SCRIPT: &str = r#"#!/bin/bash
exec rtk hook gemini
"#;

pub(crate) fn resolve_gemini_dir() -> Result<PathBuf> {
    resolve_home_subdir(GEMINI_DIR)
}

/// Entry point for `rtk init --gemini`
pub fn run_gemini(
    global: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        anyhow::bail!("Gemini support is global-only. Use: rtk init -g --gemini");
    }

    let gemini_dir = resolve_gemini_dir()?;
    if !dry_run {
        fs::create_dir_all(&gemini_dir).with_context(|| {
            format!(
                "Failed to create Gemini config dir: {}",
                gemini_dir.display()
            )
        })?;
    }

    // 1. Install hook script
    let hook_dir = gemini_dir.join("hooks");
    if !dry_run {
        fs::create_dir_all(&hook_dir)
            .with_context(|| format!("Failed to create hook dir: {}", hook_dir.display()))?;
    }
    let hook_path = hook_dir.join(GEMINI_HOOK_FILE);
    write_if_changed(&hook_path, GEMINI_HOOK_SCRIPT, "Gemini hook", ctx)?;

    #[cfg(unix)]
    if !dry_run {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("Failed to set hook permissions: {}", hook_path.display()))?;
    }

    // Store integrity baseline for tamper detection (skip in dry-run)
    if !dry_run {
        integrity::store_hash(&hook_path).with_context(|| {
            format!("Failed to store integrity hash for {}", hook_path.display())
        })?;
    }

    // 2. Install GEMINI.md (RTK awareness for Gemini)
    if !hook_only {
        let gemini_md_path = gemini_dir.join(GEMINI_MD);
        // Reuse the same slim RTK awareness content
        write_if_changed(&gemini_md_path, RTK_SLIM, GEMINI_MD, ctx)?;
    }

    // 3. Patch ~/.gemini/settings.json
    let settings_parse_failed = patch_gemini_settings(&gemini_dir, &hook_path, patch_mode, ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nGemini CLI hook installed (global).\n");
        println!("  Hook: {}", hook_path.display());
        if !hook_only {
            println!("  GEMINI.md: {}", gemini_dir.join(GEMINI_MD).display());
        }
        if settings_parse_failed {
            println!("  settings.json: NOT patched (existing file could not be parsed; see warning above)");
        }
        println!("  Restart Gemini CLI. Test with: git status\n");
    }
    Ok(())
}

/// Print the manual-setup instructions for ~/.gemini/settings.json, shared by
/// PatchMode::Skip and the unparseable-settings fallback in
/// `patch_gemini_settings`.
pub(crate) fn print_gemini_manual_setup(settings_path: &Path) {
    println!(
        "\nManual setup needed: add RTK hook to {}\n\
         See: https://github.com/rtk-ai/rtk#gemini-cli",
        settings_path.display()
    );
}

/// Patch ~/.gemini/settings.json with the BeforeTool hook.
///
/// Returns `Ok(true)` when the existing settings.json could not be parsed
/// (so the caller can tell an honest "hook installed, settings.json NOT
/// patched" apart from every other reason nothing changed — already
/// patched, `PatchMode::Skip`, declined at the `Ask` prompt, dry-run).
pub(crate) fn patch_gemini_settings(
    gemini_dir: &Path,
    hook_path: &Path,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    let InitContext { verbose, dry_run } = ctx;
    let settings_path = gemini_dir.join(SETTINGS_JSON);
    let hook_cmd = hook_path.to_string_lossy().to_string();

    // Read or create settings.json
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;
        let content = strip_leading_bom(&content);

        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            match from_json_str(content) {
                Ok(v) => v,
                Err(e) => {
                    // A parse failure must not abort the whole `rtk init` run
                    // (run_gemini has already written the hook script,
                    // GEMINI.md, and the integrity baseline by this point).
                    // Treat it like PatchMode::Skip: warn, tell the user how
                    // to patch it themselves, and leave the file untouched.
                    eprintln!(
                        "Warning: failed to parse {} as JSON: {}",
                        settings_path.display(),
                        e
                    );
                    print_gemini_manual_setup(&settings_path);
                    return Ok(true);
                }
            }
        }
    } else {
        serde_json::json!({})
    };

    let before_tool_pointer = format!("/hooks/{}", BEFORE_TOOL_KEY);
    if let Some(hooks) = settings.pointer(&before_tool_pointer) {
        if let Some(arr) = hooks.as_array() {
            if arr.iter().any(|h| {
                h.pointer("/hooks/0/command")
                    .and_then(|v| v.as_str())
                    .is_some_and(|c| c.contains("rtk"))
            }) {
                if verbose > 0 {
                    eprintln!("Gemini settings.json already has RTK hook");
                }
                return Ok(false);
            }
        }
    }

    // Ask user before patching
    if patch_mode == PatchMode::Skip {
        print_gemini_manual_setup(&settings_path);
        return Ok(false);
    }

    if patch_mode == PatchMode::Ask {
        if dry_run {
            println!(
                "[dry-run] would prompt before patching {}",
                settings_path.display()
            );
        } else {
            print!("Patch {} with RTK hook? [y/N] ", settings_path.display());
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !answer.trim().eq_ignore_ascii_case("y") {
                println!("Skipped. Add hook manually later.");
                return Ok(false);
            }
        }
    }

    // Build hook entry matching Gemini CLI format
    let hook_entry = serde_json::json!({
        "matcher": "run_shell_command",
        "hooks": [{
            "type": "command",
            "command": hook_cmd
        }]
    });

    // Insert into settings
    let hooks = settings
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    let before_tool = hooks
        .as_object_mut()
        .context("hooks is not an object")?
        .entry(BEFORE_TOOL_KEY)
        .or_insert(serde_json::json!([]));

    before_tool
        .as_array_mut()
        .context("BeforeTool is not an array")?
        .push(hook_entry);

    let content = serde_json::to_string_pretty(&settings)?;

    if dry_run {
        println!(
            "[dry-run] would patch Gemini settings.json: {}",
            settings_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", content);
        }
        return Ok(false);
    }

    // Write atomically
    let tmp = NamedTempFile::new_in(gemini_dir)?;
    fs::write(tmp.path(), &content)?;
    tmp.persist(&settings_path)
        .with_context(|| format!("Failed to write {}", settings_path.display()))?;

    if verbose > 0 {
        eprintln!("Patched {}", settings_path.display());
    }

    Ok(false)
}

/// Remove Gemini artifacts during uninstall
pub(crate) fn uninstall_gemini(ctx: InitContext) -> Result<Vec<String>> {
    let InitContext { verbose, dry_run } = ctx;
    let mut removed = Vec::new();
    let gemini_dir = match resolve_gemini_dir() {
        Ok(d) => d,
        Err(_) => return Ok(removed),
    };

    // Remove hook
    let hook_path = gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE);
    if hook_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove Gemini hook: {}",
                hook_path.display()
            );
        } else {
            fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove {}", hook_path.display()))?;
        }
        removed.push(format!("Gemini hook: {}", hook_path.display()));
    }

    // Remove GEMINI.md
    let gemini_md = gemini_dir.join(GEMINI_MD);
    if gemini_md.exists() {
        if dry_run {
            println!("[dry-run] would remove GEMINI.md: {}", gemini_md.display());
        } else {
            fs::remove_file(&gemini_md)
                .with_context(|| format!("Failed to remove {}", gemini_md.display()))?;
        }
        removed.push(format!("GEMINI.md: {}", gemini_md.display()));
    }

    // Remove hook from settings.json
    let settings_path = gemini_dir.join(SETTINGS_JSON);
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if let Ok(mut settings) = from_json_str::<serde_json::Value>(&content) {
            let bt_pointer = format!("/hooks/{}", BEFORE_TOOL_KEY);
            if let Some(arr) = settings
                .pointer_mut(&bt_pointer)
                .and_then(|v| v.as_array_mut())
            {
                let before = arr.len();
                arr.retain(|h| {
                    !h.pointer("/hooks/0/command")
                        .and_then(|v| v.as_str())
                        .is_some_and(|c| c.contains("rtk"))
                });
                if arr.len() < before {
                    if dry_run {
                        println!(
                            "[dry-run] would remove RTK hook from Gemini settings.json: {}",
                            settings_path.display()
                        );
                    } else {
                        let new_content = serde_json::to_string_pretty(&settings)?;
                        fs::write(&settings_path, new_content)?;
                    }
                    removed.push("Gemini settings.json: removed RTK hook entry".to_string());
                }
            }
        }
    }

    if verbose > 0 && !removed.is_empty() {
        eprintln!("Gemini artifacts removed");
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_patch_gemini_settings_malformed_json_warns_without_overwriting() {
        // A parse failure (not a BOM) must not abort `rtk init` — by the time
        // patch_gemini_settings runs, run_gemini has already written the hook
        // script, GEMINI.md, and the integrity baseline, so aborting here
        // would leave a half-installed state with no summary. Treat it like
        // PatchMode::Skip: warn and continue, and critically still never
        // overwrite the unparsed file. Unlike Skip (and every other
        // non-patch outcome), this path returns Ok(true) so run_gemini's
        // final summary can honestly say settings.json was NOT patched
        // instead of implying full success.
        let tmp = TempDir::new().unwrap();
        let gemini_dir = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini_dir).unwrap();
        let settings_path = gemini_dir.join(SETTINGS_JSON);
        let original = "{\"model\": \"foo\", \"mcpServers\": {},"; // trailing comma
        fs::write(&settings_path, original).unwrap();

        let result = patch_gemini_settings(
            &gemini_dir,
            Path::new("/fake/hook/path"),
            PatchMode::Auto,
            InitContext::default(),
        );

        assert!(
            result.unwrap(),
            "parse failure must be signaled so the summary can say settings.json was NOT patched"
        );

        let content = fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            content, original,
            "settings.json must not be overwritten when parsing fails"
        );
    }
}
