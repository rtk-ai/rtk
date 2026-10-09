//! OpenCode plugin: install/uninstall helpers.
use super::*;
use crate::hooks::constants::{CONFIG_DIR, OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR};

// Embedded OpenCode plugin (auto-rewrite)
const OPENCODE_PLUGIN: &str = include_str!("../../../hooks/opencode/rtk.ts");

// Embedded Pi extension (auto-rewrite)
// Stable code marker used to recognize a modified RTK extension without
// relying on explanatory comments that users may remove. The marker matches
// both the current `pi.exec` call and older stock revisions that imported
// `exec` locally before invoking it.
// SHA-256 hashes of stock Pi extension revisions that may exist on user
// machines, including the current embedded file. Keep historical entries
// when changing the extension so an untouched older install can still be
// removed safely. Hashes are computed after normalizing CRLF to LF and
// trimming trailing whitespace, matching the comparisons below.
// The history-based test below verifies that this list remains append-only
// for every revision in the current checkout's ancestor history.
pub(super) fn resolve_opencode_dir() -> Result<PathBuf> {
    resolve_home_subdir(CONFIG_DIR).map(|p| p.join(OPENCODE_SUBDIR))
}

// Pi coding agent support

/// Return OpenCode plugin path: ~/.config/opencode/plugins/rtk.ts
pub(super) fn opencode_plugin_path(opencode_dir: &Path) -> PathBuf {
    opencode_dir.join(PLUGIN_SUBDIR).join(OPENCODE_PLUGIN_FILE)
}

/// Return the OpenCode plugin install path; the write creates its directory.
pub(super) fn prepare_opencode_plugin_path() -> Result<PathBuf> {
    Ok(opencode_plugin_path(&resolve_opencode_dir()?))
}

/// Write OpenCode plugin file if missing or outdated
pub(super) fn ensure_opencode_plugin_installed(path: &Path, ctx: InitContext) -> Result<bool> {
    write_if_changed(path, OPENCODE_PLUGIN, "OpenCode plugin", ctx)
}

/// Remove OpenCode plugin file
pub(super) fn remove_opencode_plugin(ctx: InitContext) -> Result<Vec<PathBuf>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    let mut removed = Vec::new();

    if path.exists() {
        if dry_run {
            println!("[dry-run] would remove OpenCode plugin: {}", path.display());
        } else {
            // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove OpenCode plugin: {}", path.display()))?;
            if verbose > 0 {
                eprintln!("Removed OpenCode plugin: {}", path.display());
            }
        }
        removed.push(path);
    }

    Ok(removed)
}

pub(super) fn run_opencode_only_mode(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let opencode_plugin_path = prepare_opencode_plugin_path()?;
    ensure_opencode_plugin_installed(&opencode_plugin_path, ctx)?;
    if !dry_run {
        println!("\nOpenCode plugin installed (global).\n");
        println!("  OpenCode: {}", opencode_plugin_path.display());
        println!("  Restart OpenCode. Test with: git status\n");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_opencode_plugin_install_and_update() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);

        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        assert!(!plugin_path.exists());

        let changed =
            ensure_opencode_plugin_installed(&plugin_path, InitContext::default()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content, OPENCODE_PLUGIN);

        fs::write(&plugin_path, "// old").unwrap();
        let changed_again =
            ensure_opencode_plugin_installed(&plugin_path, InitContext::default()).unwrap();
        assert!(changed_again);
        let content_updated = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content_updated, OPENCODE_PLUGIN);
    }

    #[test]
    fn test_opencode_plugin_remove() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);
        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        fs::write(&plugin_path, OPENCODE_PLUGIN).unwrap();

        assert!(plugin_path.exists());
        // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
        fs::remove_file(&plugin_path).unwrap();
        assert!(!plugin_path.exists());
    }

    // Pi integration tests
}
