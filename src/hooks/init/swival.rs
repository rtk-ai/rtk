//! Swival agent: `command_middleware` adapter install/uninstall helpers.
//!
//! Swival's `command_middleware` config key accepts a path to a subprocess that
//! receives shell commands as JSON on stdin and answers with an allow/deny/rewrite
//! decision on stdout. `rtk init --agent swival` drops a Python adapter next to the
//! project (or under Swival's global config directory with `--global`) and points
//! `command_middleware` at it.

use super::*;
use toml_edit::{DocumentMut, Item};

// Embedded Swival adapter
const SWIVAL_ADAPTER: &str = include_str!("../../../hooks/swival/rtk-adapter.py");

const SWIVAL_CONFIG_KEY: &str = "command_middleware";
const SWIVAL_LOCAL_ADAPTER: &str = ".rtk/swival-rtk-adapter.py";

/// Returns `(adapter_path, config_path, middleware_command)` for the chosen scope.
fn resolve_swival_paths(global: bool) -> Result<(PathBuf, PathBuf, String)> {
    if global {
        let config_dir = resolve_swival_global_dir()?;
        let adapter = config_dir.join("rtk-adapter.py");
        // Swival shell-splits the command, so a home directory with a space in
        // it has to stay a single argument.
        let command = shell_quote(&adapter.to_string_lossy());
        Ok((adapter, config_dir.join("config.toml"), command))
    } else {
        // The config value stays project-relative so `swival.toml` is portable;
        // only the filesystem paths are anchored to the working directory.
        let cwd = user_dirs::current_dir().context("Failed to read current directory")?;
        Ok((
            cwd.join(SWIVAL_LOCAL_ADAPTER),
            cwd.join("swival.toml"),
            SWIVAL_LOCAL_ADAPTER.to_string(),
        ))
    }
}

/// Same lookup as Swival's own `global_config_dir`: `$XDG_CONFIG_HOME/swival`
/// when that is set, `~/.config/swival` otherwise.
fn resolve_swival_global_dir() -> Result<PathBuf> {
    match user_dirs::env_path("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir).join("swival")),
        None => resolve_home_subdir(".config/swival"),
    }
}

/// Python's `shlex.quote`, because Swival reads the command back with `shlex.split`.
fn shell_quote(s: &str) -> String {
    let is_safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./_-".contains(c);
    if !s.is_empty() && s.chars().all(is_safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r#"'"'"'"#))
    }
}

/// Parse a Swival config, treating a missing file as an empty one.
fn read_swival_config(path: &Path) -> Result<DocumentMut> {
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };
    content
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))
}

/// Whether `item` is the middleware command RTK manages for this scope.
fn is_rtk_middleware(item: &Item, command: &str) -> bool {
    item.as_str() == Some(command)
}

/// How a `command_middleware` value reads in the config, for messages.
fn describe_middleware(item: &Item) -> String {
    match item.as_value() {
        Some(value) => {
            let mut value = value.clone();
            value.decor_mut().clear();
            value.to_string()
        }
        None => item.to_string().trim().to_string(),
    }
}

/// Entry point for `rtk init --agent swival` (and `-g --agent swival`).
pub fn run_swival(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let (adapter_path, config_path, command) = resolve_swival_paths(global)?;
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

    let config_status = upsert_swival_config(&config_path, &command, verbose, dry_run)?;

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

/// Add `command_middleware = <command>` to the Swival config unless it is already set.
/// Returns a short status string: `added`, `up to date`, or `conflict: ...`.
///
/// The key goes into the root table, the only place Swival reads it. Appending
/// it as text would put it under whatever table the file ends with.
fn upsert_swival_config(path: &Path, command: &str, verbose: u8, dry_run: bool) -> Result<String> {
    let mut doc = read_swival_config(path)?;

    if let Some(current) = doc.get(SWIVAL_CONFIG_KEY) {
        if is_rtk_middleware(current, command) {
            return Ok("up to date".to_string());
        }
        let current = describe_middleware(current);
        if verbose > 0 {
            eprintln!(
                "rtk: {} already has {} = {} — not overwriting",
                path.display(),
                SWIVAL_CONFIG_KEY,
                current
            );
        }
        return Ok(format!("conflict: existing value {}", current));
    }

    if dry_run {
        return Ok("would add".to_string());
    }

    doc.insert(SWIVAL_CONFIG_KEY, toml_edit::value(command));
    fs::write(path, doc.to_string())
        .with_context(|| format!("Failed to write {}", path.display()))?;
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

    let (adapter_path, config_path, command) = resolve_swival_paths(global)?;

    // The config goes first: if it cannot be parsed, nothing is removed, rather
    // than leaving Swival pointed at an adapter that no longer exists.
    if config_path.exists() {
        let mut doc = read_swival_config(&config_path)?;
        match doc.get(SWIVAL_CONFIG_KEY) {
            Some(current) if is_rtk_middleware(current, &command) => {
                if !dry_run {
                    doc.remove(SWIVAL_CONFIG_KEY);
                    fs::write(&config_path, doc.to_string())
                        .with_context(|| format!("Failed to write {}", config_path.display()))?;
                }
                removed.push(format!(
                    "{}: removed {}",
                    config_path.display(),
                    SWIVAL_CONFIG_KEY
                ));
            }
            Some(current) if verbose > 0 => eprintln!(
                "rtk: leaving {} = {} (not managed by rtk)",
                SWIVAL_CONFIG_KEY,
                describe_middleware(current)
            ),
            _ => {}
        }
    }

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

    Ok(removed)
}

/// `rtk init --show --agent swival`: report adapter and config state.
pub fn show_swival_config(global: bool) -> Result<()> {
    let (adapter_path, config_path, command) = resolve_swival_paths(global)?;
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
        match read_swival_config(&config_path) {
            Err(e) => println!("[warn] Config: {:#}", e),
            Ok(doc) => match doc.get(SWIVAL_CONFIG_KEY) {
                None => println!(
                    "[missing] Config: {} has no {}",
                    config_path.display(),
                    SWIVAL_CONFIG_KEY
                ),
                Some(v) if is_rtk_middleware(v, &command) => println!(
                    "[ok] Config: {} ({} = {})",
                    config_path.display(),
                    SWIVAL_CONFIG_KEY,
                    describe_middleware(v)
                ),
                Some(v) => println!(
                    "[warn] Config: {} has {} = {} (not RTK's adapter)",
                    config_path.display(),
                    SWIVAL_CONFIG_KEY,
                    describe_middleware(v)
                ),
            },
        }
    } else {
        println!("[missing] Config: {} not found", config_path.display());
    }

    Ok(())
}

/// Returns true if a Swival config (project or global) points `command_middleware` at RTK's adapter.
pub fn is_configured() -> bool {
    for global in [false, true] {
        if let Ok((_, config_path, command)) = resolve_swival_paths(global)
            && let Ok(doc) = read_swival_config(&config_path)
            && doc
                .get(SWIVAL_CONFIG_KEY)
                .is_some_and(|item| is_rtk_middleware(item, &command))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A `command_middleware` entry encoded the way TOML needs it on every
    /// platform, since a Windows path carries backslashes.
    fn middleware_entry(command: &str) -> String {
        format!("{SWIVAL_CONFIG_KEY} = {}", toml_edit::Value::from(command))
    }

    fn root_middleware(path: &Path) -> Option<String> {
        let doc: DocumentMut = fs::read_to_string(path).unwrap().parse().unwrap();
        doc.get(SWIVAL_CONFIG_KEY)
            .and_then(Item::as_str)
            .map(str::to_string)
    }

    #[test]
    fn test_upsert_swival_config_absent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        let status = upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, false).unwrap();
        assert_eq!(status, "added");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "command_middleware = \".rtk/swival-rtk-adapter.py\"\n"
        );
    }

    #[test]
    fn test_upsert_swival_config_appends_to_existing_content() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(&path, "model = \"foo\"\n").unwrap();
        let status = upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, false).unwrap();
        assert_eq!(status, "added");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "model = \"foo\"\ncommand_middleware = \".rtk/swival-rtk-adapter.py\"\n"
        );
    }

    #[test]
    fn test_upsert_puts_key_in_root_table_when_file_ends_in_a_table() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(
            &path,
            "# my settings\nmodel = \"foo\"\n\n[audit]\nforce_review = []\n",
        )
        .unwrap();
        let status = upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, false).unwrap();
        assert_eq!(status, "added");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# my settings\nmodel = \"foo\"\ncommand_middleware = \".rtk/swival-rtk-adapter.py\"\n\n[audit]\nforce_review = []\n"
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
        let status = upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, false).unwrap();
        assert_eq!(status, "up to date");
    }

    #[test]
    fn test_upsert_swival_config_conflict() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(&path, "command_middleware = \"./other.py\"\n").unwrap();
        let status = upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, false).unwrap();
        assert_eq!(status, "conflict: existing value \"./other.py\"");
        assert!(fs::read_to_string(&path).unwrap().contains("./other.py"));
    }

    #[test]
    fn test_upsert_ignores_commented_prefixed_and_nested_keys() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(
            &path,
            "# command_middleware = \"./something.py\"\ncommand_middleware_extra = \"./other.py\"\n\n[profiles.fast]\ncommand_middleware = \"./fast.py\"\n",
        )
        .unwrap();
        let status = upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, false).unwrap();
        assert_eq!(status, "added");
        assert_eq!(
            root_middleware(&path).as_deref(),
            Some(SWIVAL_LOCAL_ADAPTER)
        );
    }

    #[test]
    fn test_upsert_swival_config_rejects_invalid_toml() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        fs::write(&path, "model = \n").unwrap();
        assert!(upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "model = \n");
    }

    #[test]
    fn test_upsert_swival_config_dry_run_does_not_write() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("swival.toml");
        let status = upsert_swival_config(&path, SWIVAL_LOCAL_ADAPTER, 0, true).unwrap();
        assert_eq!(status, "would add");
        assert!(!path.exists());
    }

    #[test]
    fn test_shell_quote_matches_python_shlex() {
        assert_eq!(
            shell_quote("/home/me/.config/swival/rtk-adapter.py"),
            "/home/me/.config/swival/rtk-adapter.py"
        );
        assert_eq!(
            shell_quote("/Users/Jane Doe/.config/swival/rtk-adapter.py"),
            "'/Users/Jane Doe/.config/swival/rtk-adapter.py'"
        );
        assert_eq!(shell_quote("/tmp/it's/a.py"), r#"'/tmp/it'"'"'s/a.py'"#);
        assert_eq!(
            shell_quote(r"C:\Users\me\rtk-adapter.py"),
            r"'C:\Users\me\rtk-adapter.py'"
        );
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn test_global_dir_follows_xdg_config_home() {
        let temp = TempDir::new().unwrap();
        let xdg = temp.path().join("xdg");
        let xdg_str = xdg.to_str().unwrap();

        let (adapter, config, _) = test_isolation::with_root(temp.path(), || {
            crate::core::user_env::with_vars(&[("XDG_CONFIG_HOME", Some(xdg_str))], || {
                resolve_swival_paths(true).unwrap()
            })
        });
        assert_eq!(adapter, xdg.join("swival").join("rtk-adapter.py"));
        assert_eq!(config, xdg.join("swival").join("config.toml"));

        let (adapter, _, _) = test_isolation::with_root(temp.path(), || {
            crate::core::user_env::with_vars(&[("XDG_CONFIG_HOME", Some(""))], || {
                resolve_swival_paths(true).unwrap()
            })
        });
        assert_eq!(
            adapter,
            temp.path()
                .join(".config")
                .join("swival")
                .join("rtk-adapter.py")
        );
    }

    #[test]
    fn test_remove_swival_strips_managed_line_only() {
        let temp = TempDir::new().unwrap();
        let (adapter, config, command) =
            test_isolation::with_root(temp.path(), || resolve_swival_paths(true).unwrap());
        fs::create_dir_all(adapter.parent().unwrap()).unwrap();
        fs::write(&adapter, SWIVAL_ADAPTER).unwrap();
        fs::write(
            &config,
            format!("model = \"foo\"\n{}\n", middleware_entry(&command)),
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
    fn test_remove_swival_matches_value_with_inline_comment() {
        let temp = TempDir::new().unwrap();
        let (_, config, command) =
            test_isolation::with_root(temp.path(), || resolve_swival_paths(true).unwrap());
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            format!("{} # RTK\nmodel = \"foo\"\n", middleware_entry(&command)),
        )
        .unwrap();

        let removed = test_isolation::with_root(temp.path(), || {
            remove_swival(true, InitContext::default()).unwrap()
        });

        assert_eq!(removed.len(), 1, "{removed:?}");
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
    fn test_remove_swival_keeps_adapter_when_config_is_invalid() {
        let temp = TempDir::new().unwrap();
        let (adapter, config, _) =
            test_isolation::with_root(temp.path(), || resolve_swival_paths(true).unwrap());
        fs::create_dir_all(adapter.parent().unwrap()).unwrap();
        fs::write(&adapter, SWIVAL_ADAPTER).unwrap();
        fs::write(&config, "model = \n").unwrap();

        let result =
            test_isolation::with_root(temp.path(), || remove_swival(true, InitContext::default()));

        assert!(result.is_err());
        assert!(adapter.exists());
    }

    #[test]
    fn test_remove_swival_dry_run_does_not_touch_files() {
        let temp = TempDir::new().unwrap();
        let (adapter, config, command) =
            test_isolation::with_root(temp.path(), || resolve_swival_paths(true).unwrap());
        fs::create_dir_all(adapter.parent().unwrap()).unwrap();
        fs::write(&adapter, SWIVAL_ADAPTER).unwrap();
        let config_content = format!("{}\n", middleware_entry(&command));
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

        let (adapter, config, command) = test_isolation::with_root(temp.path(), || {
            run_swival(true, InitContext::default()).unwrap();
            assert!(is_configured());
            resolve_swival_paths(true).unwrap()
        });

        assert_eq!(fs::read_to_string(&adapter).unwrap(), SWIVAL_ADAPTER);
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            format!("{}\n", middleware_entry(&command))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&adapter).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "adapter must be executable");
        }
    }

    #[test]
    fn test_run_swival_global_quotes_a_home_with_spaces() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("Jane Doe");
        fs::create_dir_all(&home).unwrap();

        let adapter = test_isolation::with_root(&home, || {
            run_swival(true, InitContext::default()).unwrap();
            assert!(is_configured());
            resolve_swival_paths(true).unwrap().0
        });

        assert_eq!(
            root_middleware(&adapter.with_file_name("config.toml")),
            Some(format!("'{}'", adapter.display()))
        );
    }

    #[test]
    fn test_show_config_swival_routes_to_swival_output() {
        let temp = TempDir::new().unwrap();
        let result = test_isolation::with_root(temp.path(), || show_swival_config(true));
        assert!(result.is_ok());
    }
}
