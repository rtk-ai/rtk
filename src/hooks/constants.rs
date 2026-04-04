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

pub const OPENCODE_PLUGIN_PATH: &str = ".config/opencode/plugins/rtk.ts";
pub const CURSOR_DIR: &str = ".cursor";
pub const CODEX_DIR: &str = ".codex";
pub const GEMINI_DIR: &str = ".gemini";
