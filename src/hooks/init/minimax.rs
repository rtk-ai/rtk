//! MiniMax Code agent: plugin install/uninstall helpers.
//!
//! MiniMax Code has no user-level hooks file: command hooks ship inside a
//! plugin package (`.minimax-plugin/plugin.json` + a hooks JSON). It scans
//! `<data dir>/plugins/*` as its "local" marketplace and enables what it finds
//! by default, so installing is just writing that directory.

use super::*;
use crate::hooks::constants::{
    MINIMAX_DIR, MINIMAX_PLUGIN_HOOKS, MINIMAX_PLUGIN_ICON, MINIMAX_PLUGIN_MANIFEST,
    MINIMAX_PLUGIN_NAME, MINIMAX_PLUGINS_SUBDIR,
};
use std::ffi::OsString;

const MINIMAX_PLUGIN_JSON: &str =
    include_str!("../../../hooks/minimax/rtk-rewrite/.minimax-plugin/plugin.json");
const MINIMAX_HOOKS_JSON: &str =
    include_str!("../../../hooks/minimax/rtk-rewrite/hooks/hooks.json");
const MINIMAX_ICON_PNG: &[u8] = include_bytes!("../../../hooks/minimax/rtk-rewrite/icon.png");

/// The data directory MiniMax Code uses: `$MINIMAX_DATA_DIR`, then
/// `$MAVIS_DATA_DIR`, then `~/.minimax`. (A `--profile` selects
/// `~/.minimax-<profile>`; point `$MINIMAX_DATA_DIR` at it to install there.)
fn resolve_minimax_dir() -> Result<PathBuf> {
    resolve_minimax_dir_from(
        user_dirs::env_path("MINIMAX_DATA_DIR"),
        user_dirs::env_path("MAVIS_DATA_DIR"),
    )
}

fn resolve_minimax_dir_from(
    minimax_dir: Option<OsString>,
    legacy_dir: Option<OsString>,
) -> Result<PathBuf> {
    let explicit = [minimax_dir, legacy_dir]
        .into_iter()
        .flatten()
        .map(|v| v.to_string_lossy().trim().to_string())
        .find(|v| !v.is_empty());
    match explicit {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => resolve_home_subdir(MINIMAX_DIR),
    }
}

fn minimax_plugin_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .join(MINIMAX_PLUGINS_SUBDIR)
        .join(MINIMAX_PLUGIN_NAME)
}

/// True when the manifest at `path` is RTK's own (name + author match).
fn is_rtk_plugin(manifest_path: &Path) -> bool {
    fs::read_to_string(manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            let name = v.get("name")?.as_str()?.to_string();
            let author = v.get("author")?.as_str()?.to_string();
            Some(name == MINIMAX_PLUGIN_NAME && author == "rtk-ai")
        })
        .unwrap_or(false)
}

/// Entry point for `rtk init -g --agent minimax`.
pub fn run_minimax_mode(global: bool, ctx: InitContext) -> Result<()> {
    if !global {
        anyhow::bail!("MiniMax Code support is global-only. Use: rtk init -g --agent minimax");
    }
    let data_dir = resolve_minimax_dir()?;
    run_minimax_mode_at(&data_dir, ctx)
}

fn run_minimax_mode_at(data_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let plugin_dir = minimax_plugin_dir(data_dir);
    let manifest_path = plugin_dir.join(MINIMAX_PLUGIN_MANIFEST);
    let hooks_path = plugin_dir.join(MINIMAX_PLUGIN_HOOKS);
    let icon_path = plugin_dir.join(MINIMAX_PLUGIN_ICON);

    // Never overwrite a user's own plugin that happens to share the name.
    if manifest_path.exists() && !is_rtk_plugin(&manifest_path) {
        anyhow::bail!(
            "{} already contains a plugin that is not RTK's; refusing to overwrite it. \
             Move or remove it, then re-run `rtk init -g --agent minimax`.",
            manifest_path.display()
        );
    }

    if !dry_run {
        for dir in [manifest_path.parent(), hooks_path.parent()]
            .into_iter()
            .flatten()
        {
            fs::create_dir_all(dir).with_context(|| {
                format!(
                    "Failed to create MiniMax plugin directory: {}",
                    dir.display()
                )
            })?;
        }
    }

    write_if_changed(
        &manifest_path,
        MINIMAX_PLUGIN_JSON,
        "MiniMax plugin manifest",
        ctx,
    )?;
    write_if_changed(&hooks_path, MINIMAX_HOOKS_JSON, "MiniMax plugin hooks", ctx)?;

    // MiniMax requires the manifest's `icon` to resolve to a real image.
    let icon_current = fs::read(&icon_path).is_ok_and(|b| b == MINIMAX_ICON_PNG);
    if !icon_current {
        if dry_run {
            println!(
                "[dry-run] would write MiniMax plugin icon: {}",
                icon_path.display()
            );
        } else {
            fs::write(&icon_path, MINIMAX_ICON_PNG).with_context(|| {
                format!(
                    "Failed to write MiniMax plugin icon: {}",
                    icon_path.display()
                )
            })?;
            if verbose > 0 {
                eprintln!("Wrote MiniMax plugin icon: {}", icon_path.display());
            }
        }
    }

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nRTK plugin installed for MiniMax Code (global).\n");
        println!("  Plugin: {}", plugin_dir.display());
        println!("  Restart MiniMax Code. Verify with: mcode plugin list");
        println!("  Then test with: git status\n");
    }
    Ok(())
}

/// Public entry point for `rtk init -g --agent minimax --uninstall`.
pub fn uninstall_minimax(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let data_dir = match resolve_minimax_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("RTK MiniMax uninstall skipped: could not resolve data dir ({e})");
            return Ok(());
        }
    };
    let removed = uninstall_minimax_at(&data_dir, ctx)?;

    if removed.is_empty() {
        println!("RTK MiniMax Code support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for MiniMax Code:"
        } else {
            "RTK uninstalled for MiniMax Code:"
        };
        println!("{header}");
        for item in removed {
            println!("  - {item}");
        }
        if !dry_run {
            println!("\nRestart MiniMax Code to apply changes.");
        }
    }
    if dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Remove RTK's plugin directory. Only a directory carrying RTK's manifest is
/// removed, so a user's own plugin that happens to share the name survives.
fn uninstall_minimax_at(data_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();
    let plugin_dir = minimax_plugin_dir(data_dir);
    let manifest_path = plugin_dir.join(MINIMAX_PLUGIN_MANIFEST);

    if !is_rtk_plugin(&manifest_path) {
        if verbose > 0 && plugin_dir.exists() {
            eprintln!(
                "Skipping {}: not an RTK-installed plugin",
                plugin_dir.display()
            );
        }
        return Ok(removed);
    }

    if dry_run {
        println!(
            "[dry-run] would remove MiniMax plugin directory: {}",
            plugin_dir.display()
        );
    } else {
        // nosemgrep: filesystem-deletion -- uninstall removes only RTK's own MiniMax plugin directory, after checking its manifest.
        fs::remove_dir_all(&plugin_dir).with_context(|| {
            format!(
                "Failed to remove MiniMax plugin directory: {}",
                plugin_dir.display()
            )
        })?;
    }
    removed.push(format!("MiniMax plugin: {}", plugin_dir.display()));
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_minimax_dir_precedence() {
        let a = Some(OsString::from("/a"));
        let b = Some(OsString::from("/b"));
        assert_eq!(
            resolve_minimax_dir_from(a.clone(), b.clone()).unwrap(),
            PathBuf::from("/a")
        );
        assert_eq!(
            resolve_minimax_dir_from(None, b.clone()).unwrap(),
            PathBuf::from("/b")
        );
        // Blank values are ignored, like minimax-code's `.trim() ||` chain.
        assert_eq!(
            resolve_minimax_dir_from(Some(OsString::from("  ")), b.clone()).unwrap(),
            PathBuf::from("/b")
        );
        // Padded values are trimmed.
        assert_eq!(
            resolve_minimax_dir_from(Some(OsString::from(" /a \n")), b).unwrap(),
            PathBuf::from("/a")
        );
    }

    #[test]
    fn test_minimax_assets_are_consistent() {
        let manifest: serde_json::Value = serde_json::from_str(MINIMAX_PLUGIN_JSON).unwrap();
        assert_eq!(manifest["name"], MINIMAX_PLUGIN_NAME);
        assert_eq!(manifest["icon"], MINIMAX_PLUGIN_ICON);
        assert_eq!(manifest["hooks"][0], MINIMAX_PLUGIN_HOOKS);
        assert!(MINIMAX_ICON_PNG.starts_with(b"\x89PNG\r\n\x1a\n"));

        let hooks: serde_json::Value = serde_json::from_str(MINIMAX_HOOKS_JSON).unwrap();
        let entry = &hooks["hooks"]["PreToolUse"][0];
        // MiniMax matches its native lowercase tool name, not Claude's `Bash`.
        assert_eq!(entry["matcher"], "bash");
        let handler = &entry["hooks"][0];
        assert_eq!(handler["type"], "command");
        assert_eq!(handler["command"], "rtk hook minimax");
        let timeout = handler["timeout"].as_u64().unwrap();
        assert!((1..=10).contains(&timeout), "MiniMax accepts 1-10s");
        // These fields make MiniMax skip the handler entirely.
        for skipped in ["args", "shell", "if", "async", "asyncRewake"] {
            assert!(
                handler.get(skipped).is_none(),
                "{skipped} would skip the hook"
            );
        }
    }

    #[test]
    fn test_minimax_install_writes_plugin_package() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join(".minimax");
        run_minimax_mode_at(&data, InitContext::default()).unwrap();

        let dir = data.join("plugins").join("rtk-rewrite");
        assert_eq!(
            fs::read_to_string(dir.join(".minimax-plugin/plugin.json")).unwrap(),
            MINIMAX_PLUGIN_JSON
        );
        assert_eq!(
            fs::read_to_string(dir.join("hooks/hooks.json")).unwrap(),
            MINIMAX_HOOKS_JSON
        );
        assert_eq!(fs::read(dir.join("icon.png")).unwrap(), MINIMAX_ICON_PNG);
    }

    #[test]
    fn test_minimax_install_is_idempotent_and_repairs_drift() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join(".minimax");
        run_minimax_mode_at(&data, InitContext::default()).unwrap();
        let hooks = data.join("plugins/rtk-rewrite/hooks/hooks.json");
        fs::write(&hooks, "{}").unwrap();
        run_minimax_mode_at(&data, InitContext::default()).unwrap();
        run_minimax_mode_at(&data, InitContext::default()).unwrap();
        assert_eq!(fs::read_to_string(&hooks).unwrap(), MINIMAX_HOOKS_JSON);
    }

    #[test]
    fn test_minimax_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join(".minimax");
        let ctx = InitContext {
            dry_run: true,
            ..InitContext::default()
        };
        run_minimax_mode_at(&data, ctx).unwrap();
        assert!(!data.exists());
    }

    #[test]
    fn test_minimax_requires_global() {
        assert!(run_minimax_mode(false, InitContext::default()).is_err());
    }

    #[test]
    fn test_minimax_uninstall_removes_only_rtk_plugin() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join(".minimax");
        let other = data.join("plugins").join("my-plugin");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("keep.txt"), "x").unwrap();

        run_minimax_mode_at(&data, InitContext::default()).unwrap();
        let removed = uninstall_minimax_at(&data, InitContext::default()).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(!data.join("plugins/rtk-rewrite").exists());
        assert!(other.join("keep.txt").exists());

        // Nothing left to remove.
        assert!(
            uninstall_minimax_at(&data, InitContext::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_minimax_install_refuses_to_overwrite_foreign_plugin() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join(".minimax");
        let manifest = data.join("plugins/rtk-rewrite/.minimax-plugin/plugin.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let foreign = r#"{"name":"rtk-rewrite","author":"someone-else"}"#;
        fs::write(&manifest, foreign).unwrap();

        let err = run_minimax_mode_at(&data, InitContext::default()).unwrap_err();
        assert!(err.to_string().contains("not RTK's"));
        assert_eq!(fs::read_to_string(&manifest).unwrap(), foreign);
        assert!(!data.join("plugins/rtk-rewrite/hooks").exists());
    }

    #[test]
    fn test_minimax_uninstall_spares_a_foreign_plugin_with_the_same_name() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join(".minimax");
        let foreign = data.join("plugins/rtk-rewrite/.minimax-plugin");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(
            foreign.join("plugin.json"),
            r#"{"name":"rtk-rewrite","author":"someone-else"}"#,
        )
        .unwrap();
        assert!(
            uninstall_minimax_at(&data, InitContext::default())
                .unwrap()
                .is_empty()
        );
        assert!(foreign.join("plugin.json").exists());
    }
}
