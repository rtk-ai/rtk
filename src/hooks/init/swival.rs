//! Swival agent: `command_middleware` adapter install/uninstall helpers.
//!
//! Swival's `command_middleware` config key accepts a path to a subprocess that
//! receives shell commands as JSON on stdin and answers with an allow/deny/rewrite
//! decision on stdout. `rtk init --agent swival` drops a Python adapter next to the
//! project (or under `~/.config/swival/` with `--global`) and points
//! `command_middleware` at it.

use super::*;

// Embedded Swival adapter
const SWIVAL_ADAPTER: &str = include_str!("../../../hooks/swival/rtk-adapter.py");

const SWIVAL_CONFIG_KEY: &str = "command_middleware";
const SWIVAL_LOCAL_ADAPTER: &str = ".rtk/swival-rtk-adapter.py";

/// Returns `(adapter_path, config_path, quoted_config_value)` for the chosen scope.
fn resolve_swival_paths(global: bool) -> Result<(PathBuf, PathBuf, String)> {
    if global {
        let config_dir = resolve_home_subdir(".config/swival")?;
        let adapter = config_dir.join("rtk-adapter.py");
        let value = format!("\"{}\"", adapter.display());
        Ok((adapter, config_dir.join("config.toml"), value))
    } else {
        // The config value stays project-relative so `swival.toml` is portable;
        // only the filesystem paths are anchored to the working directory.
        let cwd = user_dirs::current_dir().context("Failed to read current directory")?;
        Ok((
            cwd.join(SWIVAL_LOCAL_ADAPTER),
            cwd.join("swival.toml"),
            format!("\"{}\"", SWIVAL_LOCAL_ADAPTER),
        ))
    }
}

/// Extract the value of a top-level `command_middleware = ...` line, if this line is one.
/// Comments and keys that merely share the prefix (e.g. `command_middleware_extra`) yield `None`.
fn swival_middleware_value(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.starts_with('#') {
        return None;
    }
    let (key, value) = trimmed.split_once('=')?;
    (key.trim() == SWIVAL_CONFIG_KEY).then(|| value.trim())
}

/// Entry point for `rtk init --agent swival` (and `-g --agent swival`).
pub fn run_swival(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let (adapter_path, config_path, config_value) = resolve_swival_paths(global)?;
    if !dry_run && let Some(dir) = adapter_path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    }

    let changed = write_if_changed(&adapter_path, SWIVAL_ADAPTER, "Swival adapter", ctx)?;
    #[cfg(unix)]
    if !dry_run {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&adapter_path, fs::Permissions::from_mode(0o755))
            .context("Failed to chmod Swival adapter")?;
    }

    let config_status = upsert_swival_config(&config_path, &config_value, verbose, dry_run)?;

    if dry_run {
        println!(
            "[dry-run] Swival config: {} ({})",
            config_path.display(),
            config_status
        );
        print_dry_run_footer();
        return Ok(());
    }

    println!("\nRTK configured for Swival.\n");
    println!(
        "  Adapter: {} ({})",
        adapter_path.display(),
        if changed { "updated" } else { "up to date" }
    );
    println!("  Config:  {} ({})", config_path.display(), config_status);
    println!("  Test with: swival \"run git status\"\n");
    Ok(())
}

/// Add `command_middleware = <value>` to the Swival config unless it is already set.
/// Returns a short status string: `added`, `up to date`, or `conflict: ...`.
fn upsert_swival_config(path: &Path, value: &str, verbose: u8, dry_run: bool) -> Result<String> {
    let existing = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };

    if let Some(current_value) = existing.lines().find_map(swival_middleware_value) {
        if current_value == value {
            return Ok("up to date".to_string());
        }
        if verbose > 0 {
            eprintln!(
                "rtk: {} already has {} = {} — not overwriting",
                path.display(),
                SWIVAL_CONFIG_KEY,
                current_value
            );
        }
        return Ok(format!("conflict: existing value {}", current_value));
    }

    if dry_run {
        return Ok("would add".to_string());
    }

    let new_line = format!("{} = {}\n", SWIVAL_CONFIG_KEY, value);
    let new_content = if existing.trim().is_empty() {
        new_line
    } else {
        format!("{}\n{}", existing.trim_end(), new_line)
    };
    fs::write(path, &new_content).with_context(|| format!("Failed to write {}", path.display()))?;
    if verbose > 0 {
        eprintln!("Patched {}", path.display());
    }
    Ok("added".to_string())
}

/// Uninstall Swival integration: remove the adapter and the managed
/// `command_middleware` line (a value RTK did not write is left alone).
pub fn uninstall_swival(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let removed = remove_swival(global, ctx)?;

    if removed.is_empty() {
        println!("RTK Swival support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK (Swival):"
        } else {
            "RTK uninstalled (Swival):"
        };
        println!("{}", header);
        for item in &removed {
            println!("  - {}", item);
        }
    }

    if dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

fn remove_swival(global: bool, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    let (adapter_path, config_path, managed_value) = resolve_swival_paths(global)?;

    if adapter_path.exists() {
        if !dry_run {
            fs::remove_file(&adapter_path)
                .with_context(|| format!("Failed to remove {}", adapter_path.display()))?;
            if verbose > 0 {
                eprintln!("Removed {}", adapter_path.display());
            }
        }
        removed.push(format!("Swival adapter: {}", adapter_path.display()));
    }

    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;

        let mut touched = false;
        let new_content: String = content
            .lines()
            .filter(|l| match swival_middleware_value(l) {
                None => true,
                Some(current) if current == managed_value => {
                    touched = true;
                    false
                }
                Some(current) => {
                    if verbose > 0 {
                        eprintln!(
                            "rtk: leaving {} = {} (not managed by rtk)",
                            SWIVAL_CONFIG_KEY, current
                        );
                    }
                    true
                }
            })
            .map(|l| format!("{}\n", l))
            .collect();

        if touched {
            if !dry_run {
                // `clean_double_blanks` joins lines without a trailing newline;
                // put it back so the config file stays well-formed.
                let mut cleaned = clean_double_blanks(&new_content);
                if !cleaned.is_empty() && !cleaned.ends_with('\n') {
                    cleaned.push('\n');
                }
                fs::write(&config_path, cleaned)
                    .with_context(|| format!("Failed to write {}", config_path.display()))?;
            }
            removed.push(format!(
                "{}: removed {}",
                config_path.display(),
                SWIVAL_CONFIG_KEY
            ));
        }
    }

    Ok(removed)
}

/// `rtk init --show --agent swival`: report adapter and config state.
pub fn show_swival_config(global: bool) -> Result<()> {
    let (adapter_path, config_path, managed_value) = resolve_swival_paths(global)?;
    println!("rtk Swival Configuration:\n");

    if adapter_path.exists() {
        let ok = fs::read_to_string(&adapter_path).ok().as_deref() == Some(SWIVAL_ADAPTER);
        println!(
            "[{}] Adapter: {}{}",
            if ok { "ok" } else { "warn" },
            adapter_path.display(),
            if ok {
                ""
            } else {
                " (stale — run: rtk init --agent swival)"
            },
        );
    } else {
        println!("[missing] Adapter: {}", adapter_path.display());
    }

    if config_path.exists() {
        let content = fs::read_to_string(&config_path).unwrap_or_default();
        match content.lines().find_map(swival_middleware_value) {
            None => println!(
                "[missing] Config: {} has no {}",
                config_path.display(),
                SWIVAL_CONFIG_KEY
            ),
            Some(v) if v == managed_value => println!(
                "[ok] Config: {} ({} = {})",
                config_path.display(),
                SWIVAL_CONFIG_KEY,
                v
            ),
            Some(v) => println!(
                "[warn] Config: {} has {} = {} (not RTK's adapter)",
                config_path.display(),
                SWIVAL_CONFIG_KEY,
                v
            ),
        }
    } else {
        println!("[missing] Config: {} not found", config_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const LOCAL_VALUE: &str = "\".rtk/swival-rtk-adapter.py\"";

    #[test]
    fn test_upsert_swival_config_absent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        let status = upsert_swival_config(&path, LOCAL_VALUE, 0, false).unwrap();
        assert_eq!(status, "added");
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("command_middleware = \".rtk/swival-rtk-adapter.py\"")
        );
    }

    #[test]
    fn test_upsert_swival_config_appends_to_existing_content() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(&path, "model = \"foo\"\n").unwrap();
        let status = upsert_swival_config(&path, LOCAL_VALUE, 0, false).unwrap();
        assert_eq!(status, "added");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "model = \"foo\"\ncommand_middleware = \".rtk/swival-rtk-adapter.py\"\n"
        );
    }

    #[test]
    fn test_upsert_swival_config_up_to_date() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(
            &path,
            "command_middleware = \".rtk/swival-rtk-adapter.py\"\n",
        )
        .unwrap();
        let status = upsert_swival_config(&path, LOCAL_VALUE, 0, false).unwrap();
        assert_eq!(status, "up to date");
    }

    #[test]
    fn test_upsert_swival_config_conflict() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(&path, "command_middleware = \"./other.py\"\n").unwrap();
        let status = upsert_swival_config(&path, LOCAL_VALUE, 0, false).unwrap();
        assert!(status.starts_with("conflict"));
        assert!(fs::read_to_string(&path).unwrap().contains("./other.py"));
    }

    #[test]
    fn test_upsert_ignores_commented_and_prefixed_keys() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(
            &path,
            "# command_middleware = \"./something.py\"\ncommand_middleware_extra = \"./other.py\"\n",
        )
        .unwrap();
        let status = upsert_swival_config(&path, LOCAL_VALUE, 0, false).unwrap();
        assert_eq!(status, "added");
    }

    #[test]
    fn test_upsert_swival_config_dry_run_does_not_write() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        let status = upsert_swival_config(&path, LOCAL_VALUE, 0, true).unwrap();
        assert_eq!(status, "would add");
        assert!(!path.exists());
    }

    #[test]
    fn test_remove_swival_strips_managed_line_only() {
        let temp = TempDir::new().unwrap();
        let config_dir = temp.path().join(".config/swival");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = config_dir.join("rtk-adapter.py");
        let config = config_dir.join("config.toml");
        fs::write(&adapter, SWIVAL_ADAPTER).unwrap();
        fs::write(
            &config,
            format!(
                "model = \"foo\"\ncommand_middleware = \"{}\"\n",
                adapter.display()
            ),
        )
        .unwrap();

        let removed = test_isolation::with_root(temp.path(), || {
            remove_swival(true, InitContext::default()).unwrap()
        });

        assert_eq!(removed.len(), 2, "{removed:?}");
        assert!(!adapter.exists());
        assert_eq!(fs::read_to_string(&config).unwrap(), "model = \"foo\"\n");
    }

    #[test]
    fn test_remove_swival_leaves_foreign_middleware_alone() {
        let temp = TempDir::new().unwrap();
        let config_dir = temp.path().join(".config/swival");
        fs::create_dir_all(&config_dir).unwrap();
        let config = config_dir.join("config.toml");
        fs::write(&config, "command_middleware = \"./other.py\"\n").unwrap();

        let removed = test_isolation::with_root(temp.path(), || {
            remove_swival(true, InitContext::default()).unwrap()
        });

        assert!(removed.is_empty(), "{removed:?}");
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            "command_middleware = \"./other.py\"\n"
        );
    }

    #[test]
    fn test_remove_swival_dry_run_does_not_touch_files() {
        let temp = TempDir::new().unwrap();
        let config_dir = temp.path().join(".config/swival");
        fs::create_dir_all(&config_dir).unwrap();
        let adapter = config_dir.join("rtk-adapter.py");
        let config = config_dir.join("config.toml");
        fs::write(&adapter, SWIVAL_ADAPTER).unwrap();
        let config_content = format!("command_middleware = \"{}\"\n", adapter.display());
        fs::write(&config, &config_content).unwrap();

        let dry = InitContext {
            dry_run: true,
            ..Default::default()
        };
        let removed = test_isolation::with_root(temp.path(), || remove_swival(true, dry).unwrap());

        assert_eq!(removed.len(), 2, "{removed:?}");
        assert!(adapter.exists());
        assert_eq!(fs::read_to_string(&config).unwrap(), config_content);
    }

    #[test]
    fn test_run_swival_global_installs_adapter_and_config() {
        let temp = TempDir::new().unwrap();

        test_isolation::with_root(temp.path(), || {
            run_swival(true, InitContext::default()).unwrap();
        });

        let config_dir = temp.path().join(".config/swival");
        let adapter = config_dir.join("rtk-adapter.py");
        assert_eq!(fs::read_to_string(&adapter).unwrap(), SWIVAL_ADAPTER);
        let config = fs::read_to_string(config_dir.join("config.toml")).unwrap();
        assert_eq!(
            config,
            format!("command_middleware = \"{}\"\n", adapter.display())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&adapter).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "adapter must be executable");
        }
    }

    #[test]
    fn test_show_config_swival_routes_to_swival_output() {
        let temp = TempDir::new().unwrap();
        let result = test_isolation::with_root(temp.path(), || show_swival_config(true));
        assert!(result.is_ok());
    }
}
