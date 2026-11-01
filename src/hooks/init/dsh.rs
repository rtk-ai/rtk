//! DeepSeek Harness (DSH) agent: hook install/uninstall helpers.
//!
//! DSH reads `AGENTS.md` (and `CLAUDE.md`) from the project root and, for
//! global installs, from `$DSH_HOME` (default `~/.dsh`). It has no
//! `PreToolUse` command hook, so RTK is configured through the `full`
//! awareness block written into `AGENTS.md`; the agent is taught to prefix
//! every shell command with `rtk` itself.
//!
//! On top of that awareness block, a global install (`-g`) also installs a
//! real cordis plugin (`hooks/dsh/rtk-rewrite/`) into every discovered DSH
//! profile. DSH's `tools/pre-execute` event cannot rewrite `exec.arguments`,
//! but every bash/pwsh tool call resolves through one cordis service,
//! `ctx.shell` — the plugin patches that service's `execute` method so the
//! spawned command text is RTK's rewritten form. See
//! `hooks/dsh/README.md` for the full mechanism and its known limitations.
//! A project-local install (no `-g`) only writes the `AGENTS.md` block: the
//! plugin is machine-level DSH configuration, out of scope for a per-project
//! install.

use super::*;
use crate::core::user_dirs;
use crate::hooks::constants::{
    DSH_DIR, DSH_HOME_ENV, DSH_PATCH_FILE, DSH_PLUGIN_NAME, DSH_PROFILES_SUBDIR,
};
use std::ffi::OsString;

const DSH_PLUGIN_PACKAGE_JSON: &str = include_str!("../../../hooks/dsh/rtk-rewrite/package.json");
const DSH_PLUGIN_INDEX_JS: &str = include_str!("../../../hooks/dsh/rtk-rewrite/lib/index.js");
const DSH_PLUGIN_PATCH_TEMPLATE: &str =
    include_str!("../../../hooks/dsh/rtk-rewrite/cordis.patch.yml.template");

// ── path helpers ─────────────────────────────────────────────────────────────

fn resolve_dsh_home() -> Result<PathBuf> {
    resolve_dsh_home_from(user_dirs::home(), user_dirs::env_path(DSH_HOME_ENV))
}

fn resolve_dsh_home_from(home_dir: Option<PathBuf>, dsh_home: Option<OsString>) -> Result<PathBuf> {
    resolve_config_dir(
        dsh_home,
        home_dir,
        DSH_DIR,
        "Cannot determine DSH home directory. Set $DSH_HOME or $HOME.",
    )
}

/// `$DSH_HOME/profiles/<profile>/`.
fn resolve_dsh_profile_dir(dsh_home: &Path, profile: &str) -> PathBuf {
    dsh_home.join(DSH_PROFILES_SUBDIR).join(profile)
}

/// Every DSH profile name under `$DSH_HOME/profiles/`.
///
/// A profile is a directory that has its own `package.json` — this excludes
/// `profiles/node_modules/`, the pnpm-hoisted dependency store every profile
/// shares (not a profile: it has no `package.json` of its own, only the
/// hoisted packages every profile's `node_modules` symlinks or resolves
/// into). Not hardcoded to any particular name: different machines may have
/// one profile, several, or custom names.
fn discover_dsh_profiles(dsh_home: &Path) -> Vec<String> {
    let profiles_dir = dsh_home.join(DSH_PROFILES_SUBDIR);
    let Ok(entries) = fs::read_dir(&profiles_dir) else {
        return Vec::new();
    };

    let mut profiles: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| entry.path().join("package.json").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    profiles.sort();
    profiles
}

/// `<profile>/node_modules/rtk-rewrite/`.
fn dsh_plugin_dir(profile_dir: &Path) -> PathBuf {
    profile_dir.join("node_modules").join(DSH_PLUGIN_NAME)
}

/// True if `<profile>/cordis.patch.yml` already carries this plugin's
/// `insert` entry (`id: rtk-rewrite` on its own line, exactly as the
/// template writes it).
fn patch_has_rtk_entry(existing: &str) -> bool {
    existing
        .lines()
        .any(|line| line.trim() == "- id: rtk-rewrite")
}

/// Appends [`DSH_PLUGIN_PATCH_TEMPLATE`] to an existing `cordis.patch.yml`
/// body, adding a separating blank line when `existing` is non-empty.
/// Callers check [`patch_has_rtk_entry`] first; this never checks for an
/// existing entry itself; calling it twice would duplicate the block.
fn append_dsh_patch(existing: &str) -> String {
    if existing.trim().is_empty() {
        return DSH_PLUGIN_PATCH_TEMPLATE.to_string();
    }
    let mut patched = existing.to_string();
    if !patched.ends_with('\n') {
        patched.push('\n');
    }
    patched.push('\n');
    patched.push_str(DSH_PLUGIN_PATCH_TEMPLATE);
    patched
}

/// Removes exactly the block [`append_dsh_patch`] added — the template text,
/// plus the blank separator line it inserted before it — leaving every other
/// line in `existing` untouched. Returns `(content, true)` when something
/// was removed, `(existing, false)` when `patch_has_rtk_entry` would be
/// false, or when the matching entry is not in the exact form this plugin
/// writes (hand-edited): in that case the file is left alone rather than
/// risk a wrong edit.
fn remove_dsh_patch_entry(existing: &str) -> (String, bool) {
    if !patch_has_rtk_entry(existing) {
        return (existing.to_string(), false);
    }

    let template = DSH_PLUGIN_PATCH_TEMPLATE;
    if existing == template {
        return (String::new(), true);
    }

    let with_blank_separator = format!("\n{template}");
    if let Some(pos) = existing.find(&with_blank_separator) {
        let mut result = existing.to_string();
        result.replace_range(pos..pos + with_blank_separator.len(), "");
        return (result, true);
    }

    if let Some(pos) = existing.find(template) {
        let mut result = existing.to_string();
        result.replace_range(pos..pos + template.len(), "");
        return (result, true);
    }

    (existing.to_string(), false)
}

/// Installs the plugin package and the `cordis.patch.yml` insert entry into
/// one DSH profile directory. Returns whether this call changed anything on
/// disk (false on a repeat run against an already-installed profile), so
/// callers can decide whether to mention a restart/HMR wait.
fn install_dsh_plugin_for_profile(profile_dir: &Path, ctx: InitContext) -> Result<bool> {
    let plugin_dir = dsh_plugin_dir(profile_dir);
    let mut changed = false;

    changed |= write_if_changed(
        &plugin_dir.join("package.json"),
        DSH_PLUGIN_PACKAGE_JSON,
        "DSH plugin package.json",
        ctx,
    )?;
    changed |= write_if_changed(
        &plugin_dir.join("lib").join("index.js"),
        DSH_PLUGIN_INDEX_JS,
        "DSH plugin index.js",
        ctx,
    )?;

    let patch_path = profile_dir.join(DSH_PATCH_FILE);
    let existing_patch = if patch_path.exists() {
        fs::read_to_string(&patch_path)
            .with_context(|| format!("Failed to read {}", patch_path.display()))?
    } else {
        String::new()
    };

    if !patch_has_rtk_entry(&existing_patch) {
        let patched = append_dsh_patch(&existing_patch);
        changed |= write_if_changed(&patch_path, &patched, "DSH profile patch", ctx)?;
    }

    Ok(changed)
}

/// Removes the plugin package directory and the `cordis.patch.yml` insert
/// entry from one DSH profile directory. Returns one human-readable string
/// per thing actually removed (or that would be removed, under dry-run),
/// matching the style [`uninstall_dsh`] prints for the `AGENTS.md` step.
fn uninstall_dsh_plugin_for_profile(profile_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    let plugin_dir = dsh_plugin_dir(profile_dir);
    if plugin_dir.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove DSH plugin directory: {}",
                plugin_dir.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- uninstall intentionally removes only RTK's DSH plugin directory.
            fs::remove_dir_all(&plugin_dir).with_context(|| {
                format!(
                    "Failed to remove DSH plugin directory: {}",
                    plugin_dir.display()
                )
            })?;
            if verbose > 0 {
                eprintln!("Removed DSH plugin directory: {}", plugin_dir.display());
            }
        }
        removed.push(format!("DSH plugin: {}", plugin_dir.display()));
    }

    let patch_path = profile_dir.join(DSH_PATCH_FILE);
    if patch_path.exists() {
        let existing = fs::read_to_string(&patch_path)
            .with_context(|| format!("Failed to read {}", patch_path.display()))?;
        let (cleaned, did_remove) = remove_dsh_patch_entry(&existing);
        if did_remove {
            if dry_run {
                println!(
                    "[dry-run] would update DSH profile patch: {}",
                    patch_path.display()
                );
            } else {
                atomic_write(&patch_path, &cleaned)
                    .with_context(|| format!("Failed to write {}", patch_path.display()))?;
                if verbose > 0 {
                    eprintln!("Updated DSH profile patch: {}", patch_path.display());
                }
            }
            removed.push(format!(
                "DSH profile patch: removed rtk-rewrite entry ({})",
                patch_path.display()
            ));
        }
    }

    Ok(removed)
}

// ── public entry points ───────────────────────────────────────────────────────

/// Returns true if the DSH `AGENTS.md` RTK instructions block is installed
/// (harness home first, then the project root).
pub fn is_configured() -> bool {
    let Ok(dsh_home) = resolve_dsh_home() else {
        return false;
    };
    let global_agents_md = dsh_home.join(AGENTS_MD);
    if global_agents_md.exists()
        && let Ok(content) = fs::read_to_string(&global_agents_md)
    {
        return content.contains(RTK_BLOCK_START);
    }
    // Also check the local project AGENTS.md.
    if let Ok(local) = user_dirs::current_dir() {
        let local_agents_md = local.join(AGENTS_MD);
        if local_agents_md.exists()
            && let Ok(content) = fs::read_to_string(&local_agents_md)
        {
            return content.contains(RTK_BLOCK_START);
        }
    }
    false
}

pub fn run_dsh_mode(global: bool, ctx: InitContext) -> Result<()> {
    if global {
        let dsh_home = resolve_dsh_home()?;
        run_dsh_mode_at(&dsh_home, global, ctx)
    } else {
        let base_dir = user_dirs::current_dir()?;
        run_dsh_mode_at(&base_dir, global, ctx)
    }
}

pub fn uninstall_dsh(global: bool, ctx: InitContext) -> Result<()> {
    let base_dir = if global {
        resolve_dsh_home()?
    } else {
        user_dirs::current_dir()?
    };
    let removed = uninstall_dsh_at(&base_dir, global, ctx)?;

    if removed.is_empty() {
        println!("RTK DSH support was not installed (nothing to remove)");
    } else {
        let header = if ctx.dry_run {
            "[dry-run] would uninstall RTK for DeepSeek Harness:"
        } else {
            "RTK uninstalled for DeepSeek Harness:"
        };
        println!("{header}");
        for item in &removed {
            println!("  - {item}");
        }
    }
    if ctx.dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

// ── internal helpers ──────────────────────────────────────────────────────────

fn run_dsh_mode_at(base_dir: &Path, global: bool, ctx: InitContext) -> Result<()> {
    let agents_md_path = base_dir.join(AGENTS_MD);

    write_rtk_block(
        &agents_md_path,
        &rtk_block(RTK_AWARENESS_FULL),
        "RTK instructions",
        if global {
            "rtk init -g --agent dsh"
        } else {
            "rtk init --agent dsh"
        },
        ctx,
    )?;

    print_instructions_agents_awareness_note("DeepSeek Harness", ctx);
    if !ctx.dry_run {
        println!("\nRTK configured for DeepSeek Harness (DSH).\n");
        println!("  AGENTS.md: {}", agents_md_path.display());
        println!("  DSH will now use rtk commands for token savings.");
        println!("  Test with: git status\n");
    }

    // The plugin is machine-level DSH configuration (every profile under
    // `$DSH_HOME/profiles/`), so it is only installed for a global install.
    // A project-local install only ever touches this project's AGENTS.md.
    if global {
        install_dsh_plugin(base_dir, ctx)?;
    }

    Ok(())
}

/// Installs the cordis plugin into every discovered DSH profile under
/// `dsh_home/profiles/`. Never errors out of `run_dsh_mode_at` just because
/// no profile was found — that leaves the AGENTS.md step's success
/// unaffected, since DSH's profile layout may simply not exist yet (not
/// installed, or using a non-default `$DSH_HOME`) or this is an
/// environment with no profile at all (e.g. the isolated one-off `dsh`
/// `--sdk`/headless modes that never create `profiles/`).
fn install_dsh_plugin(dsh_home: &Path, ctx: InitContext) -> Result<()> {
    let profiles = discover_dsh_profiles(dsh_home);
    if profiles.is_empty() {
        let prefix = if ctx.dry_run { "[dry-run] " } else { "" };
        println!(
            "{prefix}No DSH profile found under {}; skipping plugin install. \
             See hooks/dsh/README.md to install it manually.",
            dsh_home.join(DSH_PROFILES_SUBDIR).display()
        );
        return Ok(());
    }

    let mut installed_any = false;
    let mut profile_paths = Vec::new();
    for profile in &profiles {
        let profile_dir = resolve_dsh_profile_dir(dsh_home, profile);
        let changed = install_dsh_plugin_for_profile(&profile_dir, ctx)?;
        installed_any |= changed;
        profile_paths.push(dsh_plugin_dir(&profile_dir));
    }

    let prefix = if ctx.dry_run { "[dry-run] " } else { "" };
    println!(
        "{prefix}DSH plugin ({} profile{}):",
        profiles.len(),
        if profiles.len() == 1 { "" } else { "s" }
    );
    for path in &profile_paths {
        println!("  - {}", path.display());
    }
    if !ctx.dry_run {
        if installed_any {
            println!(
                "  DSH reloads its plugin tree via HMR within a few seconds. \
                 If rewrites don't take effect shortly, restart DeepSeek Harness."
            );
        } else {
            println!("  Already installed (no changes).");
        }
    }

    Ok(())
}

fn uninstall_dsh_at(base_dir: &Path, global: bool, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let agents_md_path = base_dir.join(AGENTS_MD);
    let mut removed = Vec::new();

    if agents_md_path.exists() {
        let content = fs::read_to_string(&agents_md_path)
            .with_context(|| format!("Failed to read AGENTS.md: {}", agents_md_path.display()))?;

        if content.contains(RTK_BLOCK_START) {
            let (cleaned, did_remove) = remove_rtk_block(&content);
            if did_remove {
                if dry_run {
                    println!(
                        "[dry-run] would remove RTK instructions block from {}",
                        agents_md_path.display()
                    );
                } else {
                    atomic_write(&agents_md_path, &cleaned).with_context(|| {
                        format!("Failed to write AGENTS.md: {}", agents_md_path.display())
                    })?;
                    if verbose > 0 {
                        eprintln!("Updated AGENTS.md: {}", agents_md_path.display());
                    }
                }
                removed.push(format!(
                    "AGENTS.md: removed rtk-instructions block ({})",
                    agents_md_path.display()
                ));
            }
        }
    }

    // The plugin is machine-level DSH configuration; only a global
    // uninstall touches any DSH profile, matching `run_dsh_mode_at`.
    if global {
        for profile in discover_dsh_profiles(base_dir) {
            let profile_dir = resolve_dsh_profile_dir(base_dir, &profile);
            removed.extend(uninstall_dsh_plugin_for_profile(&profile_dir, ctx)?);
        }
    }

    Ok(removed)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_dsh_mode_creates_agents_md_with_rtk_block() {
        let temp = TempDir::new().unwrap();
        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let agents_md = temp.path().join("AGENTS.md");
        assert!(agents_md.exists(), "AGENTS.md should be created");
        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(
            content.contains(RTK_BLOCK_START),
            "AGENTS.md should contain the RTK instructions block"
        );
        assert!(
            content.contains(RTK_AWARENESS_FULL),
            "AGENTS.md should contain full awareness text"
        );
    }

    #[test]
    fn test_dsh_mode_is_idempotent() {
        let temp = TempDir::new().unwrap();
        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();
        let first = fs::read_to_string(temp.path().join("AGENTS.md")).unwrap();

        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();
        let second = fs::read_to_string(temp.path().join("AGENTS.md")).unwrap();

        assert_eq!(first, second, "second run must be a no-op");
    }

    #[test]
    fn test_dsh_mode_preserves_existing_user_content() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        fs::write(&agents_md, "# Team rules\n\nDo not break prod.\n").unwrap();

        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(content.contains("# Team rules"), "user content preserved");
        assert!(
            content.contains("Do not break prod."),
            "user content preserved"
        );
        assert!(content.contains(RTK_BLOCK_START), "RTK block appended");
    }

    #[test]
    fn test_dsh_mode_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        run_dsh_mode_at(
            temp.path(),
            false,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            !temp.path().join("AGENTS.md").exists(),
            "dry-run must not create AGENTS.md"
        );
    }

    #[test]
    fn test_uninstall_dsh_removes_rtk_block() {
        let temp = TempDir::new().unwrap();
        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();
        assert!(temp.path().join("AGENTS.md").exists());

        let removed = uninstall_dsh_at(temp.path(), false, InitContext::default()).unwrap();
        assert_eq!(removed.len(), 1, "one item removed");

        let content = fs::read_to_string(temp.path().join("AGENTS.md")).unwrap();
        assert!(
            !content.contains(RTK_BLOCK_START),
            "RTK block must be removed"
        );
    }

    #[test]
    fn test_uninstall_dsh_preserves_user_content() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        fs::write(&agents_md, "# Team rules\n\nDo not break prod.\n").unwrap();

        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();
        uninstall_dsh_at(temp.path(), false, InitContext::default()).unwrap();

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(
            content.contains("# Team rules"),
            "user content preserved after uninstall"
        );
        assert!(!content.contains(RTK_BLOCK_START), "RTK block removed");
    }

    #[test]
    fn test_uninstall_dsh_is_idempotent() {
        let temp = TempDir::new().unwrap();
        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();

        let first = uninstall_dsh_at(temp.path(), false, InitContext::default()).unwrap();
        let second = uninstall_dsh_at(temp.path(), false, InitContext::default()).unwrap();

        assert_eq!(first.len(), 1);
        assert!(second.is_empty(), "second uninstall should be a no-op");
    }

    #[test]
    fn test_uninstall_dsh_missing_file_is_ok() {
        let temp = TempDir::new().unwrap();
        let removed = uninstall_dsh_at(temp.path(), false, InitContext::default()).unwrap();
        assert!(removed.is_empty());
    }

    #[test]
    fn test_uninstall_dsh_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();
        let before = fs::read_to_string(temp.path().join("AGENTS.md")).unwrap();

        uninstall_dsh_at(
            temp.path(),
            false,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

        let after = fs::read_to_string(temp.path().join("AGENTS.md")).unwrap();
        assert_eq!(before, after, "dry-run must not modify AGENTS.md");
    }

    #[test]
    fn test_resolve_dsh_home_prefers_dsh_home_env() {
        use std::ffi::OsString;
        let custom = OsString::from("/tmp/my-dsh-home");
        let home = PathBuf::from("/tmp/home");

        let result = resolve_dsh_home_from(Some(home.clone()), Some(custom.clone())).unwrap();
        assert_eq!(result, PathBuf::from(custom));
    }

    #[test]
    fn test_resolve_dsh_home_falls_back_to_home_dot_dsh() {
        use std::ffi::OsString;
        let home = PathBuf::from("/tmp/home");

        let from_empty = resolve_dsh_home_from(Some(home.clone()), Some(OsString::new())).unwrap();
        let from_missing = resolve_dsh_home_from(Some(home.clone()), None).unwrap();

        assert_eq!(from_empty, home.join(".dsh"));
        assert_eq!(from_missing, home.join(".dsh"));
    }

    #[test]
    fn test_is_configured_detects_local_agents_md() {
        use crate::core::test_isolation;
        let temp = test_isolation::tempdir();
        let _entered = test_isolation::enter(temp.path());

        assert!(!is_configured(), "not configured yet");

        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();
        assert!(is_configured(), "should be configured after install");
    }

    #[test]
    fn test_dsh_always_receives_full_awareness() {
        for level in [
            AwarenessLevel::Default,
            AwarenessLevel::High,
            AwarenessLevel::Full,
        ] {
            let temp = TempDir::new().unwrap();
            run_dsh_mode_at(
                temp.path(),
                false,
                InitContext {
                    awareness: level,
                    ..Default::default()
                },
            )
            .unwrap();
            let content = fs::read_to_string(temp.path().join("AGENTS.md")).unwrap();
            assert!(
                content.contains(RTK_AWARENESS_FULL),
                "dsh must always get full awareness, level={level}"
            );
        }
    }

    // ── plugin install/uninstall tests ──────────────────────────────────────

    /// Creates `<dsh_home>/profiles/<name>/package.json` so
    /// [`discover_dsh_profiles`] recognizes it as a real profile.
    fn make_profile(dsh_home: &Path, name: &str) -> PathBuf {
        let profile_dir = resolve_dsh_profile_dir(dsh_home, name);
        fs::create_dir_all(&profile_dir).unwrap();
        fs::write(
            profile_dir.join("package.json"),
            format!("{{\"name\": \"dsh-profile-{name}\"}}"),
        )
        .unwrap();
        profile_dir
    }

    #[test]
    fn test_discover_dsh_profiles_finds_only_dirs_with_package_json() {
        let temp = TempDir::new().unwrap();
        make_profile(temp.path(), "desktop");
        make_profile(temp.path(), "web");
        // The shared pnpm-hoisted store: a directory with no package.json.
        fs::create_dir_all(temp.path().join("profiles").join("node_modules")).unwrap();

        let mut profiles = discover_dsh_profiles(temp.path());
        profiles.sort();
        assert_eq!(profiles, vec!["desktop".to_string(), "web".to_string()]);
    }

    #[test]
    fn test_discover_dsh_profiles_missing_dir_is_empty() {
        let temp = TempDir::new().unwrap();
        assert!(discover_dsh_profiles(temp.path()).is_empty());
    }

    #[test]
    fn test_patch_has_rtk_entry() {
        assert!(!patch_has_rtk_entry(""));
        assert!(!patch_has_rtk_entry("- id: other-plugin\n  name: other\n"));
        assert!(patch_has_rtk_entry(
            "- insert:\n    - id: rtk-rewrite\n      name: rtk-rewrite\n"
        ));
    }

    #[test]
    fn test_append_dsh_patch_on_empty_file() {
        let patched = append_dsh_patch("");
        assert_eq!(patched, DSH_PLUGIN_PATCH_TEMPLATE);
        assert!(patch_has_rtk_entry(&patched));
    }

    #[test]
    fn test_append_dsh_patch_preserves_existing_content() {
        let existing = "# comment\n- id: ui-chat\n  name: \"@deepseek-ai/dsh-client-ui-chat\"\n";
        let patched = append_dsh_patch(existing);
        assert!(patched.starts_with(existing));
        assert!(patched.contains("id: rtk-rewrite"));
        assert_eq!(patched.matches("id: rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_append_dsh_patch_adds_newline_before_separator_when_missing() {
        let existing = "- id: ui-chat\n  name: x"; // no trailing newline
        let patched = append_dsh_patch(existing);
        assert!(patched.starts_with("- id: ui-chat\n  name: x\n\n"));
        assert!(patch_has_rtk_entry(&patched));
    }

    #[test]
    fn test_remove_dsh_patch_entry_round_trips_with_append() {
        let existing = "- id: ui-chat\n  name: \"@deepseek-ai/dsh-client-ui-chat\"\n";
        let patched = append_dsh_patch(existing);
        let (cleaned, did_remove) = remove_dsh_patch_entry(&patched);
        assert!(did_remove);
        assert_eq!(cleaned, existing);
    }

    #[test]
    fn test_remove_dsh_patch_entry_on_pure_template() {
        let (cleaned, did_remove) = remove_dsh_patch_entry(DSH_PLUGIN_PATCH_TEMPLATE);
        assert!(did_remove);
        assert_eq!(cleaned, "");
    }

    #[test]
    fn test_remove_dsh_patch_entry_no_entry_present_is_noop() {
        let existing = "- id: ui-chat\n  name: x\n";
        let (cleaned, did_remove) = remove_dsh_patch_entry(existing);
        assert!(!did_remove);
        assert_eq!(cleaned, existing);
    }

    #[test]
    fn test_install_dsh_plugin_for_profile_writes_package_and_patches_yaml() {
        let temp = TempDir::new().unwrap();
        let profile_dir = make_profile(temp.path(), "desktop");
        fs::write(
            profile_dir.join(DSH_PATCH_FILE),
            "- id: ui-chat\n  name: \"@deepseek-ai/dsh-client-ui-chat\"\n",
        )
        .unwrap();

        let changed = install_dsh_plugin_for_profile(&profile_dir, InitContext::default()).unwrap();
        assert!(changed);

        let plugin_dir = dsh_plugin_dir(&profile_dir);
        assert_eq!(
            fs::read_to_string(plugin_dir.join("package.json")).unwrap(),
            DSH_PLUGIN_PACKAGE_JSON
        );
        assert_eq!(
            fs::read_to_string(plugin_dir.join("lib").join("index.js")).unwrap(),
            DSH_PLUGIN_INDEX_JS
        );

        let patch = fs::read_to_string(profile_dir.join(DSH_PATCH_FILE)).unwrap();
        assert!(patch.contains("id: ui-chat"), "existing entry preserved");
        assert!(patch.contains("id: rtk-rewrite"));
    }

    #[test]
    fn test_install_dsh_plugin_for_profile_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let profile_dir = make_profile(temp.path(), "desktop");

        let first = install_dsh_plugin_for_profile(&profile_dir, InitContext::default()).unwrap();
        assert!(first);
        let patch_after_first = fs::read_to_string(profile_dir.join(DSH_PATCH_FILE)).unwrap();

        let second = install_dsh_plugin_for_profile(&profile_dir, InitContext::default()).unwrap();
        assert!(!second, "second install must be a no-op");
        let patch_after_second = fs::read_to_string(profile_dir.join(DSH_PATCH_FILE)).unwrap();
        assert_eq!(patch_after_first, patch_after_second);
        assert_eq!(patch_after_second.matches("id: rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_install_dsh_plugin_for_profile_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let profile_dir = make_profile(temp.path(), "desktop");

        let changed = install_dsh_plugin_for_profile(
            &profile_dir,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(changed, "dry-run still reports it WOULD change something");
        assert!(!dsh_plugin_dir(&profile_dir).exists());
        assert!(!profile_dir.join(DSH_PATCH_FILE).exists());
    }

    #[test]
    fn test_uninstall_dsh_plugin_for_profile_removes_dir_and_cleans_patch() {
        let temp = TempDir::new().unwrap();
        let profile_dir = make_profile(temp.path(), "desktop");
        fs::write(
            profile_dir.join(DSH_PATCH_FILE),
            "- id: ui-chat\n  name: x\n",
        )
        .unwrap();
        install_dsh_plugin_for_profile(&profile_dir, InitContext::default()).unwrap();
        assert!(dsh_plugin_dir(&profile_dir).exists());

        let removed_first =
            uninstall_dsh_plugin_for_profile(&profile_dir, InitContext::default()).unwrap();
        let removed_second =
            uninstall_dsh_plugin_for_profile(&profile_dir, InitContext::default()).unwrap();

        assert_eq!(removed_first.len(), 2, "plugin dir + patch entry");
        assert!(removed_second.is_empty(), "second uninstall is a no-op");
        assert!(!dsh_plugin_dir(&profile_dir).exists());

        let patch = fs::read_to_string(profile_dir.join(DSH_PATCH_FILE)).unwrap();
        assert!(patch.contains("id: ui-chat"), "other entries preserved");
        assert!(!patch.contains("rtk-rewrite"));
    }

    #[test]
    fn test_uninstall_dsh_plugin_for_profile_missing_is_noop() {
        let temp = TempDir::new().unwrap();
        let profile_dir = make_profile(temp.path(), "desktop");
        let removed =
            uninstall_dsh_plugin_for_profile(&profile_dir, InitContext::default()).unwrap();
        assert!(removed.is_empty());
    }

    #[test]
    fn test_run_dsh_mode_at_global_installs_plugin_into_every_profile() {
        let temp = TempDir::new().unwrap();
        make_profile(temp.path(), "desktop");
        make_profile(temp.path(), "web");

        run_dsh_mode_at(temp.path(), true, InitContext::default()).unwrap();

        for name in ["desktop", "web"] {
            let profile_dir = resolve_dsh_profile_dir(temp.path(), name);
            assert!(
                dsh_plugin_dir(&profile_dir).join("package.json").exists(),
                "{name} should have the plugin"
            );
            let patch = fs::read_to_string(profile_dir.join(DSH_PATCH_FILE)).unwrap();
            assert!(patch.contains("id: rtk-rewrite"));
        }
    }

    #[test]
    fn test_run_dsh_mode_at_local_does_not_touch_any_profile() {
        let temp = TempDir::new().unwrap();
        let profile_dir = make_profile(temp.path(), "desktop");

        run_dsh_mode_at(temp.path(), false, InitContext::default()).unwrap();

        assert!(!dsh_plugin_dir(&profile_dir).exists());
    }

    #[test]
    fn test_run_dsh_mode_at_global_no_profiles_does_not_error() {
        let temp = TempDir::new().unwrap();
        // No profiles/ directory at all.
        run_dsh_mode_at(temp.path(), true, InitContext::default()).unwrap();
    }

    #[test]
    fn test_uninstall_dsh_at_global_removes_plugin_from_every_profile() {
        let temp = TempDir::new().unwrap();
        make_profile(temp.path(), "desktop");
        make_profile(temp.path(), "web");
        run_dsh_mode_at(temp.path(), true, InitContext::default()).unwrap();

        let removed = uninstall_dsh_at(temp.path(), true, InitContext::default()).unwrap();
        // AGENTS.md block + 2 profiles * (plugin dir + patch entry) = 5
        assert_eq!(removed.len(), 5);

        for name in ["desktop", "web"] {
            let profile_dir = resolve_dsh_profile_dir(temp.path(), name);
            assert!(!dsh_plugin_dir(&profile_dir).exists());
        }
    }

    #[test]
    fn test_uninstall_dsh_at_local_does_not_touch_any_profile() {
        let temp = TempDir::new().unwrap();
        let profile_dir = make_profile(temp.path(), "desktop");
        run_dsh_mode_at(temp.path(), true, InitContext::default()).unwrap();
        assert!(dsh_plugin_dir(&profile_dir).exists());

        // A local (non-global) uninstall against the same base_dir must not
        // reach into any DSH profile.
        uninstall_dsh_at(temp.path(), false, InitContext::default()).unwrap();
        assert!(
            dsh_plugin_dir(&profile_dir).exists(),
            "local uninstall must not remove machine-level plugin state"
        );
    }

    #[test]
    fn test_run_dsh_mode_at_global_is_idempotent_end_to_end() {
        let temp = TempDir::new().unwrap();
        make_profile(temp.path(), "desktop");

        run_dsh_mode_at(temp.path(), true, InitContext::default()).unwrap();
        let profile_dir = resolve_dsh_profile_dir(temp.path(), "desktop");
        let first = fs::read_to_string(profile_dir.join(DSH_PATCH_FILE)).unwrap();

        run_dsh_mode_at(temp.path(), true, InitContext::default()).unwrap();
        let second = fs::read_to_string(profile_dir.join(DSH_PATCH_FILE)).unwrap();

        assert_eq!(first, second, "second global install must be a no-op");
    }
}
