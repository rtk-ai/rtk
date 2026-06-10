pub const REWRITE_HOOK_FILE: &str = "rtk-rewrite.sh";
pub const GEMINI_HOOK_FILE: &str = "rtk-hook-gemini.sh";
pub const CLAUDE_DIR: &str = ".claude";
pub const HOOKS_SUBDIR: &str = "hooks";
pub const SETTINGS_JSON: &str = "settings.json";
pub const SETTINGS_LOCAL_JSON: &str = "settings.local.json";
pub const HOOKS_JSON: &str = "hooks.json";
pub const PRE_TOOL_USE_KEY: &str = "PreToolUse";
pub const BEFORE_TOOL_KEY: &str = "BeforeTool";

/// Native Rust hook command for Claude Code (replaces rtk-rewrite.sh).
pub const CLAUDE_HOOK_COMMAND: &str = "rtk hook claude";
/// Native Rust hook command for Cursor (replaces rtk-rewrite.sh).
pub const CURSOR_HOOK_COMMAND: &str = "rtk hook cursor";

pub const CONFIG_DIR: &str = ".config";
pub const OPENCODE_SUBDIR: &str = "opencode";
pub const PLUGIN_SUBDIR: &str = "plugins";
pub const OPENCODE_PLUGIN_FILE: &str = "rtk.ts";

pub const CURSOR_DIR: &str = ".cursor";
pub const CODEX_DIR: &str = ".codex";
pub const GEMINI_DIR: &str = ".gemini";

pub const GITHUB_DIR: &str = ".github";
pub const COPILOT_HOOK_FILE: &str = "rtk-rewrite.json";
pub const COPILOT_INSTRUCTIONS_FILE: &str = "copilot-instructions.md";
pub const COPILOT_USER_DIR: &str = ".copilot";
pub const COPILOT_HOME_ENV: &str = "COPILOT_HOME";

pub const PI_DIR: &str = ".pi/agent";
pub const PI_LOCAL_DIR: &str = ".pi";
pub const PI_EXTENSIONS_SUBDIR: &str = "extensions";
pub const PI_PLUGIN_FILE: &str = "rtk.ts";
pub const PI_CODING_AGENT_DIR_ENV: &str = "PI_CODING_AGENT_DIR";

pub const HERMES_DIR: &str = ".hermes";
pub const HERMES_PLUGINS_SUBDIR: &str = "plugins";
pub const HERMES_PLUGIN_NAME: &str = "rtk-rewrite";
pub const HERMES_PLUGIN_INIT_FILE: &str = "__init__.py";
pub const HERMES_PLUGIN_MANIFEST_FILE: &str = "plugin.yaml";
