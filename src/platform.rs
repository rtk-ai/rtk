use anyhow::{Context, Result};
use std::path::PathBuf;

/// Supported AI coding agent platforms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPlatform {
    ClaudeCode,
    OpenCode,
}

impl AgentPlatform {
    /// Display name for user-facing messages
    pub fn name(&self) -> &'static str {
        match self {
            AgentPlatform::ClaudeCode => "Claude Code",
            AgentPlatform::OpenCode => "OpenCode",
        }
    }

    /// The rules/instructions filename used by this platform
    pub fn rules_file(&self) -> &'static str {
        match self {
            AgentPlatform::ClaudeCode => "CLAUDE.md",
            AgentPlatform::OpenCode => "AGENTS.md",
        }
    }

    /// The global config directory for this platform
    pub fn config_dir(&self) -> Result<PathBuf> {
        let home = dirs::home_dir().context("Cannot determine home directory. Is $HOME set?")?;
        match self {
            AgentPlatform::ClaudeCode => Ok(home.join(".claude")),
            AgentPlatform::OpenCode => Ok(home.join(".config").join("opencode")),
        }
    }

    /// The hook/plugin mechanism name
    pub fn hook_mechanism(&self) -> &'static str {
        match self {
            AgentPlatform::ClaudeCode => "PreToolUse hook",
            AgentPlatform::OpenCode => "plugin",
        }
    }

    /// The hook/plugin filename
    pub fn hook_filename(&self) -> &'static str {
        match self {
            AgentPlatform::ClaudeCode => "rtk-rewrite.sh",
            AgentPlatform::OpenCode => "rtk-rewrite.ts",
        }
    }

    /// The hook/plugin subdirectory name within the config dir
    pub fn hook_subdir(&self) -> &'static str {
        match self {
            AgentPlatform::ClaudeCode => "hooks",
            AgentPlatform::OpenCode => "plugins",
        }
    }
}

/// Detect which platform is currently running by checking environment variables.
/// Returns None if we cannot determine the platform (e.g., running outside an agent session).
pub fn detect_platform() -> Option<AgentPlatform> {
    // OpenCode sets OPENCODE=1 in the environment
    if std::env::var("OPENCODE").is_ok() {
        return Some(AgentPlatform::OpenCode);
    }

    // Claude Code sets CLAUDE_CODE=1 in the environment
    if std::env::var("CLAUDE_CODE").is_ok() {
        return Some(AgentPlatform::ClaudeCode);
    }

    None
}

/// Detect platform, falling back to checking which config dirs exist on disk.
/// For init commands where we're not running inside an agent session.
pub fn detect_or_infer_platform() -> Option<AgentPlatform> {
    // First try env-based detection
    if let Some(p) = detect_platform() {
        return Some(p);
    }

    // Fallback: check which config directories exist
    let home = dirs::home_dir()?;
    let claude_dir = home.join(".claude");
    let opencode_dir = home.join(".config").join("opencode");

    match (claude_dir.exists(), opencode_dir.exists()) {
        (true, false) => Some(AgentPlatform::ClaudeCode),
        (false, true) => Some(AgentPlatform::OpenCode),
        // Both exist or neither exists — can't infer
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_name() {
        assert_eq!(AgentPlatform::ClaudeCode.name(), "Claude Code");
        assert_eq!(AgentPlatform::OpenCode.name(), "OpenCode");
    }

    #[test]
    fn test_rules_file() {
        assert_eq!(AgentPlatform::ClaudeCode.rules_file(), "CLAUDE.md");
        assert_eq!(AgentPlatform::OpenCode.rules_file(), "AGENTS.md");
    }

    #[test]
    fn test_config_dir_claude() {
        let dir = AgentPlatform::ClaudeCode.config_dir().unwrap();
        assert!(dir.ends_with(".claude"));
    }

    #[test]
    fn test_config_dir_opencode() {
        let dir = AgentPlatform::OpenCode.config_dir().unwrap();
        // Should end with .config/opencode
        assert!(dir.ends_with("opencode"));
        let parent = dir.parent().unwrap();
        assert!(parent.ends_with(".config"));
    }

    #[test]
    fn test_hook_mechanism() {
        assert_eq!(
            AgentPlatform::ClaudeCode.hook_mechanism(),
            "PreToolUse hook"
        );
        assert_eq!(AgentPlatform::OpenCode.hook_mechanism(), "plugin");
    }

    #[test]
    fn test_hook_filename() {
        assert_eq!(AgentPlatform::ClaudeCode.hook_filename(), "rtk-rewrite.sh");
        assert_eq!(AgentPlatform::OpenCode.hook_filename(), "rtk-rewrite.ts");
    }

    #[test]
    fn test_hook_subdir() {
        assert_eq!(AgentPlatform::ClaudeCode.hook_subdir(), "hooks");
        assert_eq!(AgentPlatform::OpenCode.hook_subdir(), "plugins");
    }

    #[test]
    fn test_rules_file_local_claude() {
        let path = PathBuf::from(AgentPlatform::ClaudeCode.rules_file());
        assert_eq!(path, PathBuf::from("CLAUDE.md"));
    }

    #[test]
    fn test_rules_file_local_opencode() {
        let path = PathBuf::from(AgentPlatform::OpenCode.rules_file());
        assert_eq!(path, PathBuf::from("AGENTS.md"));
    }

    #[test]
    fn test_rules_file_global_claude() {
        let path = AgentPlatform::ClaudeCode
            .config_dir()
            .unwrap()
            .join(AgentPlatform::ClaudeCode.rules_file());
        assert!(path.ends_with("CLAUDE.md"));
        assert!(path.to_string_lossy().contains(".claude"));
    }

    #[test]
    fn test_rules_file_global_opencode() {
        let path = AgentPlatform::OpenCode
            .config_dir()
            .unwrap()
            .join(AgentPlatform::OpenCode.rules_file());
        assert!(path.ends_with("AGENTS.md"));
        assert!(path.to_string_lossy().contains("opencode"));
    }

    #[test]
    fn test_platform_equality() {
        assert_eq!(AgentPlatform::ClaudeCode, AgentPlatform::ClaudeCode);
        assert_eq!(AgentPlatform::OpenCode, AgentPlatform::OpenCode);
        assert_ne!(AgentPlatform::ClaudeCode, AgentPlatform::OpenCode);
    }
}
