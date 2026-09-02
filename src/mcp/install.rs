use anyhow::{Context, Result};
use serde_json::Value;
use std::path::PathBuf;

/// Install RTK as an MCP server in Claude Desktop's configuration.
pub fn run() -> Result<()> {
    let rtk_exe = std::env::current_exe().context("Could not determine RTK binary path")?;
    let rtk_path = rtk_exe.to_string_lossy().into_owned();

    let config_path = claude_desktop_config_path()
        .context("Could not determine Claude Desktop config path — is Claude Desktop installed?")?;

    if !config_path.exists() {
        // Create the file with a fresh config
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Could not create config directory: {}", parent.display())
            })?;
        }
        let fresh = build_config_with_rtk(&rtk_path);
        let json = serde_json::to_string_pretty(&fresh).context("Failed to serialize config")?;
        std::fs::write(&config_path, json)
            .with_context(|| format!("Failed to write {}", config_path.display()))?;
        println!("Created: {}", config_path.display());
        println!("RTK MCP server registered. Restart Claude Desktop to activate.");
        return Ok(());
    }

    // Patch existing config
    let existing = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let mut config: Value = serde_json::from_str(&existing)
        .with_context(|| format!("Failed to parse {}: not valid JSON", config_path.display()))?;

    if is_already_installed(&config) {
        println!(
            "RTK MCP server is already registered in {}",
            config_path.display()
        );
        println!("Run `rtk mcp-serve` to verify it works.");
        return Ok(());
    }

    inject_rtk_server(&mut config, &rtk_path);

    let patched =
        serde_json::to_string_pretty(&config).context("Failed to serialize patched config")?;
    std::fs::write(&config_path, patched)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;

    println!("Updated: {}", config_path.display());
    println!("RTK MCP server registered. Restart Claude Desktop to activate.");
    Ok(())
}

fn is_already_installed(config: &Value) -> bool {
    config
        .get("mcpServers")
        .and_then(|s| s.get("rtk"))
        .is_some()
}

fn inject_rtk_server(config: &mut Value, rtk_path: &str) {
    let servers = config
        .as_object_mut()
        .map(|obj| {
            obj.entry("mcpServers")
                .or_insert_with(|| Value::Object(serde_json::Map::new()))
        })
        .and_then(|v| v.as_object_mut());

    if let Some(servers_map) = servers {
        servers_map.insert(
            "rtk".to_string(),
            serde_json::json!({
                "command": rtk_path,
                "args": ["mcp-serve"]
            }),
        );
    }
}

fn build_config_with_rtk(rtk_path: &str) -> Value {
    serde_json::json!({
        "mcpServers": {
            "rtk": {
                "command": rtk_path,
                "args": ["mcp-serve"]
            }
        }
    })
}

fn claude_desktop_config_path() -> Option<PathBuf> {
    let base = dirs::config_dir()?;

    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Library/Application Support/Claude/claude_desktop_config.json
        // dirs::config_dir() on macOS returns ~/Library/Application Support
        Some(base.join("Claude").join("claude_desktop_config.json"))
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: %APPDATA%\Claude\claude_desktop_config.json
        // dirs::config_dir() on Windows returns %APPDATA%\Roaming
        Some(base.join("Claude").join("claude_desktop_config.json"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux: ~/.config/Claude/claude_desktop_config.json
        Some(base.join("Claude").join("claude_desktop_config.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_is_already_installed_true() {
        let config = json!({
            "mcpServers": {
                "rtk": {"command": "/usr/bin/rtk", "args": ["mcp-serve"]}
            }
        });
        assert!(is_already_installed(&config));
    }

    #[test]
    fn test_is_already_installed_false_empty() {
        let config = json!({});
        assert!(!is_already_installed(&config));
    }

    #[test]
    fn test_is_already_installed_false_other_servers() {
        let config = json!({
            "mcpServers": {
                "other": {"command": "other-binary"}
            }
        });
        assert!(!is_already_installed(&config));
    }

    #[test]
    fn test_inject_rtk_server_creates_mcp_servers() {
        let mut config = json!({});
        inject_rtk_server(&mut config, "/usr/bin/rtk");
        assert!(config["mcpServers"]["rtk"]["command"].as_str().is_some());
        assert_eq!(
            config["mcpServers"]["rtk"]["args"][0].as_str().unwrap(),
            "mcp-serve"
        );
    }

    #[test]
    fn test_inject_rtk_server_preserves_existing_servers() {
        let mut config = json!({
            "mcpServers": {
                "other": {"command": "other-binary"}
            }
        });
        inject_rtk_server(&mut config, "/usr/bin/rtk");
        assert!(config["mcpServers"]["other"].is_object());
        assert!(config["mcpServers"]["rtk"].is_object());
    }

    #[test]
    fn test_build_config_with_rtk() {
        let config = build_config_with_rtk("/usr/local/bin/rtk");
        assert_eq!(
            config["mcpServers"]["rtk"]["command"].as_str().unwrap(),
            "/usr/local/bin/rtk"
        );
        assert_eq!(
            config["mcpServers"]["rtk"]["args"][0].as_str().unwrap(),
            "mcp-serve"
        );
    }

    #[test]
    fn test_inject_idempotent_after_double_inject() {
        let mut config = json!({});
        inject_rtk_server(&mut config, "/usr/bin/rtk");
        inject_rtk_server(&mut config, "/usr/bin/rtk");
        // Still just one rtk entry
        let servers = config["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
    }
}
