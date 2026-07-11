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
/// Native Rust hook command for Factory Droid.
pub const DROID_HOOK_COMMAND: &str = "rtk hook droid";

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

/// Factory Droid config directory, joined onto the resolved home directory.
pub const DROID_DIR: &str = ".factory";
/// Canonical Droid hooks file (Droid's own /hooks UI reads and writes this).
pub const DROID_HOOKS_FILE: &str = "hooks.json";
/// Legacy nested hooks location (`.factory/hooks/hooks.json`), still read by
/// Droid when the root `hooks.json` is absent.
pub const DROID_HOOKS_SUBDIR: &str = "hooks";
/// Droid settings file. Its `hooks` key is a fallback config surface: Droid
/// merges `hooks.json` OVER it per event key, so a `PreToolUse` entry here is
/// silently ignored once `hooks.json` defines `PreToolUse`.
pub const DROID_SETTINGS_FILE: &str = "settings.json";
/// Tool matcher used by Droid for shell command execution.
pub const DROID_EXECUTE_MATCHER: &str = "Execute";
/// Environment variable Droid uses to override its HOME directory (the
/// `.factory` segment is appended to it): `$FACTORY_HOME_OVERRIDE/.factory`.
pub const DROID_HOME_ENV: &str = "FACTORY_HOME_OVERRIDE";

pub const HERMES_DIR: &str = ".hermes";
pub const HERMES_PLUGINS_SUBDIR: &str = "plugins";
pub const HERMES_PLUGIN_NAME: &str = "rtk-rewrite";
pub const HERMES_PLUGIN_INIT_FILE: &str = "__init__.py";
pub const HERMES_PLUGIN_MANIFEST_FILE: &str = "plugin.yaml";
