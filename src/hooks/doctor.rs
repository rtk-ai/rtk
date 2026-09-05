use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Read-only integration inspection. Presence is deliberately not promoted to live verification.
pub fn inspect(agent: &str) -> Map<String, Value> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".codex"));
    let (instructions, mcp, profile, hook, hook_trust, live_verification) = match agent {
        "codex" => {
            let agents = codex_home.join("AGENTS.md");
            let rtk_md = codex_home.join("RTK.md");
            let config = codex_home.join("config.toml");
            let instructions = codex_instructions_present(&agents, &rtk_md);
            let (mcp, profile) = codex_config_state(&config);
            (
                json!(if instructions { "present" } else { "missing" }),
                json!(if mcp { "present" } else { "missing" }),
                json!(profile),
                json!(super::hook_check::codex_status_in(&codex_home).as_str()),
                json!("host-managed"),
                json!("unverified"),
            )
        }
        "claude" => {
            let claude = home.join(".claude");
            (
                json!(if claude.join("CLAUDE.md").is_file() {
                    "present"
                } else {
                    "unknown"
                }),
                json!("unknown"),
                json!("default"),
                json!(format!("{:?}", super::hook_check::status())),
                json!("unknown"),
                json!("unverified"),
            )
        }
        _ => (
            json!("unsupported_agent"),
            json!("unsupported_agent"),
            json!("unknown"),
            json!("unsupported_agent"),
            json!("unknown"),
            json!("unverified"),
        ),
    };
    let mut report = Map::new();
    report.insert("instructions".into(), instructions);
    report.insert("mcp".into(), mcp);
    report.insert("hook".into(), hook);
    report.insert("hook_trust".into(), hook_trust);
    report.insert(
        "binary".into(),
        json!(std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string())),
    );
    report.insert("profile".into(), profile);
    report.insert("live_verification".into(), live_verification);
    report
}

fn codex_instructions_present(agents: &Path, rtk_md: &Path) -> bool {
    rtk_md.is_file()
        && std::fs::read_to_string(agents).ok().is_some_and(|content| {
            content.contains("RTK.md") || content.contains("<!-- rtk-instructions")
        })
}

fn codex_config_state(config: &Path) -> (bool, String) {
    let Ok(content) = std::fs::read_to_string(config) else {
        return (false, "unavailable".to_string());
    };
    let Ok(root) = content.parse::<toml::Value>() else {
        return (false, "invalid".to_string());
    };
    let mcp = root
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .and_then(|servers| servers.get("rtk"))
        .and_then(toml::Value::as_table)
        .is_some_and(|server| {
            server
                .get("command")
                .and_then(toml::Value::as_str)
                .is_some_and(|command| command.to_ascii_lowercase().contains("rtk"))
                && server
                    .get("args")
                    .and_then(toml::Value::as_array)
                    .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("mcp")))
        });
    let profile = root
        .get("profile")
        .and_then(toml::Value::as_str)
        .unwrap_or("default")
        .to_string();
    (mcp, profile)
}

pub fn run(agent: &str, format: &str) -> anyhow::Result<()> {
    let report = inspect(agent);
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&Value::Object(report))?),
        "text" => {
            for (key, value) in report {
                println!("{key}: {}", value.as_str().unwrap_or("unknown"));
            }
        }
        _ => anyhow::bail!("format must be text or json"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn doctor_report_has_independent_contract_fields() {
        let report = super::inspect("codex");
        assert!(report.contains_key("instructions"));
        assert!(report.contains_key("mcp"));
        assert!(report.contains_key("hook"));
        assert!(report.contains_key("hook_trust"));
        assert!(report.contains_key("binary"));
        assert!(report.contains_key("profile"));
        assert!(report.contains_key("live_verification"));
    }
}
