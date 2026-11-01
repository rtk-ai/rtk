//! Muse Agent Plugins bundle generation. Installation and trust remain explicit
//! Muse operations; RTK never invokes Muse or edits its installed registry.

use super::*;
use crate::core::user_dirs;

const MANIFEST: &str = r#"{
  "name": "rtk",
  "version": "1.0.0",
  "description": "Muse Code command rewriting with RTK"
}
"#;
const HOOKS: &str = r#"{
  "hooks": {
    "PreToolUse": [{
      "matcher": "Bash",
      "hooks": [{"type": "command", "command": "rtk hook muse", "timeout": 10}]
    }]
  }
}
"#;

fn bundle_dir(global: bool) -> Result<PathBuf> {
    if global {
        Ok(user_dirs::config()
            .context("Could not resolve RTK config directory")?
            .join("plugins/muse"))
    } else {
        Ok(user_dirs::current_dir()
            .context("Could not resolve project directory")?
            .join(".rtk/muse-plugin"))
    }
}

fn install_at(dir: &Path, ctx: InitContext) -> Result<()> {
    for (file, contents) in [
        (".claude-plugin/plugin.json", MANIFEST),
        ("hooks/hooks.json", HOOKS),
    ] {
        let path = dir.join(file);
        if !ctx.dry_run {
            let parent = path.parent().context("Muse bundle file has no parent")?;
            fs::create_dir_all(parent).context("Failed to create Muse bundle directory")?;
        }
        write_if_changed(&path, contents, "Muse plugin bundle", ctx)?;
    }
    Ok(())
}

pub fn run_muse_mode(global: bool, ctx: InitContext) -> Result<()> {
    let dir = bundle_dir(global)?;
    install_at(&dir, ctx)?;
    if ctx.dry_run {
        print_dry_run_footer();
    } else {
        println!("\nMuse bundle generated. Review it, then run:");
        // POSIX shell quoting keeps spaces and apostrophes in a bundle path intact.
        let quoted = dir.display().to_string().replace('\'', "'\\''");
        println!(
            "  muse plugins install '{}' --scope {}",
            quoted,
            if global { "user" } else { "project" }
        );
        println!("  muse plugins approve rtk");
        println!("RTK does not install or approve Muse plugins.");
        println!(
            "Rewrites require a configured :ask-me default and no persisted rule touching either command."
        );
    }
    Ok(())
}

pub fn uninstall_muse(global: bool, ctx: InitContext) -> Result<()> {
    let dir = bundle_dir(global)?;
    // Delete only our generated files; preserve any additional user content.
    for file in [".claude-plugin/plugin.json", "hooks/hooks.json"] {
        let path = dir.join(file);
        if path.exists() {
            if ctx.dry_run {
                println!("[dry-run] would remove {}", path.display());
            } else {
                // nosemgrep: filesystem-deletion -- only RTK-generated bundle files.
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
            }
        }
    }
    println!("To remove Muse's installed copy, run: muse plugins remove rtk");
    Ok(())
}

pub fn show_muse_config(global: bool) -> Result<()> {
    let dir = bundle_dir(global)?;
    println!("Muse bundle: {}", dir.display());
    println!("Enabled RTK plugin installed: {}", is_configured());
    println!("Capability trust must be inspected with: muse plugins inspect rtk --json");
    Ok(())
}

fn muse_data_dir() -> Option<PathBuf> {
    user_dirs::env_path("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| user_dirs::home().map(|home| home.join(".local/share")))
        .map(|base| base.join("muse"))
}

pub fn is_configured() -> bool {
    muse_data_dir().is_some_and(|dir| installed_at(&dir))
}

fn installed_at(data: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(data.join("plugins/installed.json")) else {
        return false;
    };
    let Ok(registry) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    if registry.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return false;
    }
    let Some(record) = registry.pointer("/plugins/rtk") else {
        return false;
    };
    if record.get("enabled").and_then(serde_json::Value::as_bool) != Some(true)
        || record.get("id").and_then(serde_json::Value::as_str) != Some("rtk")
    {
        return false;
    }
    let Some(cache) = record.get("cache_path").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let cache = Path::new(cache);
    // Registry paths are relative to Muse's data root. Do not follow foreign
    // absolute paths or traversal entries supplied by a malformed registry.
    if cache
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return false;
    }
    let dir = data.join(cache);
    let Ok(raw) = fs::read_to_string(dir.join(".claude-plugin/plugin.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    if manifest.get("name").and_then(serde_json::Value::as_str) != Some("rtk") {
        return false;
    }
    let Ok(raw) = fs::read_to_string(dir.join("hooks/hooks.json")) else {
        return false;
    };
    let Ok(hooks) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    hooks
        .pointer("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry.get("matcher").and_then(serde_json::Value::as_str) == Some("Bash")
                    && entry
                        .get("hooks")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|hooks| {
                            hooks.iter().any(|hook| {
                                hook.get("type").and_then(serde_json::Value::as_str)
                                    == Some("command")
                                    && hook
                                        .get("command")
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|cmd| {
                                            crate::hooks::is_rtk_hook_command(cmd, "muse")
                                        })
                            })
                        })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detection_requires_enabled_installed_copy_not_generated_bundle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data = tmp.path();
        let bundle = data.join("plugins/cache/local/rtk/hash/package");
        let ctx = InitContext {
            verbose: 0,
            dry_run: false,
            awareness: Default::default(),
        };
        install_at(&bundle, ctx).expect("bundle");
        assert!(!installed_at(data));
        let registry = data.join("plugins/installed.json");
        for enabled in [false, true] {
            fs::write(
                &registry,
                json!({"version":1,"plugins":{"rtk":{
                    "id":"rtk","enabled":enabled,"cache_path":"plugins/cache/local/rtk/hash/package"
                }}})
                .to_string(),
            )
            .expect("registry");
            assert_eq!(installed_at(data), enabled);
        }
        fs::write(
            bundle.join("hooks/hooks.json"),
            HOOKS.replace("rtk hook muse", "echo rtk hook muse"),
        )
        .expect("foreign hook");
        assert!(!installed_at(data));
        fs::write(&registry, "bad json").expect("broken registry");
        assert!(!installed_at(data));
    }

    #[test]
    fn dry_run_does_not_create_bundle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bundle = tmp.path().join("absent");
        install_at(
            &bundle,
            InitContext {
                verbose: 0,
                dry_run: true,
                awareness: Default::default(),
            },
        )
        .expect("dry run");
        assert!(!bundle.exists());
    }
}
