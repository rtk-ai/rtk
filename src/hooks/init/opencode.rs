//! OpenCode plugin: install/uninstall helpers.
use super::*;
use crate::hooks::constants::{CONFIG_DIR, OPENCODE_PLUGIN_FILE, OPENCODE_SUBDIR, PLUGIN_SUBDIR};
use std::process::Command;

// Embedded OpenCode plugin (auto-rewrite)
const OPENCODE_PLUGIN: &str = include_str!("../../../hooks/opencode/rtk.ts");

/// Oldest OpenCode that can load `OPENCODE_PLUGIN`.
///
/// The plugin default-exports an object carrying a V1 `server()` and a V2
/// `setup()`, which is the shape OpenCode's own V2 plugin docs prescribe, and
/// those docs put the object form at 1.18.29: older V1 releases call every
/// export as `fn(input)`, so they read the object as a plugin function and fail
/// on the first hook. 2.x reads `id` and `setup()` and ignores `server()`.
const MIN_OPENCODE: (u32, u32, u32) = (1, 18, 29);

/// Pull `major.minor.patch` out of `opencode --version` ("1.3.3", "opencode 1.3.3",
/// "v2.0.22"). `None` when there is no such run of digits.
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let word = s.split_whitespace().find(|w| {
        w.trim_start_matches(|c: char| !c.is_ascii_digit())
            .starts_with(|c: char| c.is_ascii_digit())
    })?;
    let mut it = word
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// The installed OpenCode version banner, or `None` when there is none we can read.
fn opencode_version() -> Option<String> {
    let out = Command::new("opencode").arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The installed version banner when that OpenCode is older than `MIN_OPENCODE`.
///
/// `None` for an OpenCode we could not ask, could not parse, or one that is new
/// enough, so an unreadable version never turns into a complaint.
fn too_old_opencode() -> Option<String> {
    let out = opencode_version()?;
    is_too_old(&out).then_some(out)
}

/// Is this `opencode --version` banner older than `MIN_OPENCODE`?
fn is_too_old(banner: &str) -> bool {
    let Some((major, minor, patch)) = parse_version(banner) else {
        return false;
    };
    (major, minor, patch) < MIN_OPENCODE
}

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

/// Prepare OpenCode plugin directory and return install path
pub(super) fn prepare_opencode_plugin_path() -> Result<PathBuf> {
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    // Directory creation is deferred to install time (caller guards on dry_run).
    Ok(path)
}

/// Write OpenCode plugin file if missing or outdated
pub(super) fn ensure_opencode_plugin_installed(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { dry_run, .. } = ctx;
    // Ensure parent dir exists (skip in dry-run)
    if !dry_run && let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create OpenCode plugin directory: {}",
                parent.display()
            )
        })?;
    }
    if !dry_run && let Some(found) = too_old_opencode() {
        let need = format!("{}.{}.{}", MIN_OPENCODE.0, MIN_OPENCODE.1, MIN_OPENCODE.2);
        println!(
            "  [rtk] OpenCode {found} is older than {need}, and the rtk plugin will not load there.\n        \
             Upgrade OpenCode, then re-run `rtk init -g --opencode`."
        );
    }
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

    // One shape, two loaders: the 2.x loader reads `id` and `setup()` off the
    // default export, and 1.x calls `server()` on it. A callable default fails
    // the 2.x schema check, so neither half may become a function again.
    #[test]
    fn plugin_is_an_object_default_with_both_entrypoints() {
        assert!(OPENCODE_PLUGIN.contains("const RtkOpenCodePlugin = {"));
        assert!(OPENCODE_PLUGIN.contains("id: \"rtk\""));
        assert!(OPENCODE_PLUGIN.contains("setup(ctx"));
        assert!(OPENCODE_PLUGIN.contains("server()"));
        assert!(OPENCODE_PLUGIN.contains("\"execute.before\""));
        assert!(OPENCODE_PLUGIN.contains("\"tool.execute.before\""));
        assert!(!OPENCODE_PLUGIN.contains("export default RtkOpenCodePlugin("));
    }

    // The floor is load-bearing: below it OpenCode calls the object export as a
    // plugin function and refuses to start, which is why init says so out loud.
    #[test]
    fn version_floor_is_pinned() {
        assert!(is_too_old("1.18.28"));
        assert!(is_too_old("opencode 1.3.3"));
        assert!(is_too_old("1.1.4"));
        assert!(is_too_old("0.14.2"));
        assert!(!is_too_old("1.18.29"));
        assert!(!is_too_old("1.18.34"));
        assert!(!is_too_old("2.0.22"));
        // unreadable or unparsable is not evidence of an old OpenCode
        assert!(!is_too_old(""));
        assert!(!is_too_old("opencode (unknown)"));
        assert!(!is_too_old("v"));
    }

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
