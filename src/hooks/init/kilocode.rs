//! Kilo Code plugin install and uninstall helpers.

use super::*;
use crate::hooks::constants::{
    CONFIG_DIR, KILOCODE_PLUGIN_FILE, KILOCODE_PLUGIN_SUBDIR, KILOCODE_SUBDIR,
};

const KILOCODE_PLUGIN: &str = include_str!("../../../hooks/kilocode/rtk.ts");

pub fn run_kilocode_mode(ctx: InitContext) -> Result<()> {
    run_kilocode_mode_at(&resolve_kilocode_dir()?, ctx)
}

fn run_kilocode_mode_at(kilocode_dir: &Path, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let plugin_path = kilocode_plugin_path(kilocode_dir);
    let installed = ensure_kilocode_plugin_installed(&plugin_path, ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else {
        let status = if installed {
            "installed"
        } else {
            "already current"
        };
        println!("\nKilo Code plugin {status}.\n");
        println!("  Plugin: {}", plugin_path.display());
        println!("  Restart Kilo Code. Test with: git status\n");
    }

    Ok(())
}

fn resolve_kilocode_dir() -> Result<PathBuf> {
    if let Some(config_dir) = std::env::var_os("KILO_CONFIG_DIR").filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(config_dir));
    }

    let home = dirs::home_dir().context(if cfg!(windows) {
        "Cannot determine home directory. Is %USERPROFILE% set?"
    } else {
        "Cannot determine home directory. Is $HOME set?"
    })?;
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(CONFIG_DIR));

    Ok(config_dir.join(KILOCODE_SUBDIR))
}

fn kilocode_plugin_path(kilocode_dir: &Path) -> PathBuf {
    kilocode_dir
        .join(KILOCODE_PLUGIN_SUBDIR)
        .join(KILOCODE_PLUGIN_FILE)
}

pub fn kilocode_plugin_installed() -> bool {
    resolve_kilocode_dir()
        .map(|path| kilocode_plugin_path(&path).is_file())
        .unwrap_or(false)
}

fn ensure_kilocode_plugin_installed(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { dry_run, .. } = ctx;
    if !dry_run && let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create Kilo Code plugin directory: {}",
                parent.display()
            )
        })?;
    }
    write_if_changed(path, KILOCODE_PLUGIN, "Kilo Code plugin", ctx)
}

pub fn uninstall_kilocode(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let plugin_path = kilocode_plugin_path(&resolve_kilocode_dir()?);

    if !plugin_path.exists() {
        println!("RTK Kilo Code plugin was not installed (nothing to remove)");
        return Ok(());
    }

    if dry_run {
        println!(
            "[dry-run] would remove Kilo Code plugin: {}",
            plugin_path.display()
        );
        print_dry_run_footer();
        return Ok(());
    }

    fs::remove_file(&plugin_path).with_context(|| {
        format!(
            "Failed to remove Kilo Code plugin: {}",
            plugin_path.display()
        )
    })?;
    println!("RTK Kilo Code plugin removed: {}", plugin_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_kilocode_mode_creates_plugin_file() {
        let temp = TempDir::new().unwrap();
        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();

        let plugin_path = temp.path().join("plugin/rtk.ts");
        assert!(plugin_path.exists(), "Plugin file should be created");
        let content = fs::read_to_string(&plugin_path).unwrap();
        assert!(content.contains("@kilocode/plugin"));
    }

    #[test]
    fn test_kilocode_mode_is_idempotent() {
        let temp = TempDir::new().unwrap();
        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();

        let path = temp.path().join("plugin/rtk.ts");
        let first = fs::read_to_string(&path).unwrap();

        run_kilocode_mode_at(temp.path(), InitContext::default()).unwrap();
        let second = fs::read_to_string(&path).unwrap();
        assert_eq!(first, second);
    }
}
