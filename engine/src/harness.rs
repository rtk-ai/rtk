use crate::block::{Event, Subject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    AwarenessFile,
    SessionStart,
    PerTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigChannel {
    Env {
        var: &'static str,
    },
    ConfigKey {
        file: &'static str,
        key: &'static str,
    },
    ProviderTable {
        file: &'static str,
        provider: &'static str,
    },
    SettingsEnv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCapability {
    pub event: Event,
    pub can_replace_input: bool,
    pub can_replace_output: bool,
    pub can_deny: bool,
    pub can_ask: bool,
    pub can_append: bool,
    pub can_redirect_tool: bool,
    pub subjects: &'static [Subject],
}

impl EventCapability {
    const NONE: EventCapability = EventCapability {
        event: Event::BeforeTool,
        can_replace_input: false,
        can_replace_output: false,
        can_deny: false,
        can_ask: false,
        can_append: false,
        can_redirect_tool: false,
        subjects: crate::block::HOOK_SUBJECTS,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpNaming {
    DoubleUnderscore,
    ColonPrefix,
    DotServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookRuntime {
    Native,
    Plugin,
    RulesFile,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub docs: Option<&'static str>,
    pub verified: Option<&'static str>,
    pub runtime: HookRuntime,
    pub evaluates: &'static [Event],
    pub events: &'static [EventCapability],
    pub tool_names: &'static [(&'static str, Subject)],
    pub mcp: McpNaming,
    pub unreachable: &'static [Subject],
    pub session_start: bool,
    pub directive_channels: &'static [Channel],
    pub base_url_config: Option<ConfigChannel>,
    /// How `rtk init` writes this harness's hook config. `None` for a harness
    /// with no hook surface to write.
    pub wiring: Option<Wiring>,
}

/// What `rtk init` needs to turn a pipeline into this harness's own config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wiring {
    /// The file to write, relative to the scope's base directory.
    pub path: &'static str,
    pub format: ConfigFormat,
    /// This harness's spelling of each event we can register.
    pub before_tool: Option<&'static str>,
    pub after_tool: Option<&'static str>,
    pub session_start: Option<&'static str>,
    pub user_prompt: Option<&'static str>,
    /// How a set of tool names becomes one matcher value.
    pub matcher: MatcherStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    /// `{"hooks": {"<Event>": [{matcher, hooks: [{type, command}]}]}}` — Claude
    /// Code's shape, also read natively by Devin and Trae.
    ClaudeNested,
    /// Same nesting, but the event map is the whole file with no `hooks` key.
    BareNested,
    /// `{"version": 1, "hooks": {"<event>": [{command, matcher}]}}` — one flat
    /// entry per hook rather than a matcher group.
    FlatEntries,
    /// `{"<our-name>": {"<Event>": [{matcher, hooks: [...]}]}}` — namespaced by
    /// hook name at the top level.
    NamedGroups,
    /// TOML `[[hooks]]` array-of-tables.
    TomlTables,
    /// A generated plugin file rather than a config block.
    Plugin,
}

/// How a harness expects several tool names in one matcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatcherStyle {
    /// Regex alternation, anchored: `^(?:Bash|Read)$`.
    AnchoredAlternation,
    /// Regex alternation, bare: `Bash|Read`.
    Alternation,
    RegexPrefixed,
    /// The harness ignores matchers; the hook filters on tool name itself.
    None,
}

const CLAUDE_TOOLS: &[(&str, Subject)] = &[
    ("Read", Subject::Read),
    ("NotebookRead", Subject::Read),
    ("TaskOutput", Subject::Read),
    ("Write", Subject::Write),
    ("Edit", Subject::Edit),
    ("MultiEdit", Subject::Edit),
    ("NotebookEdit", Subject::Edit),
    ("Bash", Subject::Shell),
    ("PowerShell", Subject::Shell),
    ("Monitor", Subject::Shell),
    ("Grep", Subject::Search),
    ("Glob", Subject::Search),
    ("LSP", Subject::Search),
    ("WebFetch", Subject::Web),
    ("WebSearch", Subject::Web),
    ("Task", Subject::Subagent),
    ("Agent", Subject::Subagent),
    ("SendMessage", Subject::Subagent),
    ("ListAgents", Subject::Subagent),
    ("Workflow", Subject::Subagent),
    ("ToolSearch", Subject::Mcp),
    ("WaitForMcpServers", Subject::Mcp),
    ("ListMcpResourcesTool", Subject::Mcp),
    ("ReadMcpResourceTool", Subject::Mcp),
];

const CURSOR_TOOLS: &[(&str, Subject)] = &[
    ("Read", Subject::Read),
    ("Write", Subject::Write),
    ("Delete", Subject::Write),
    ("Shell", Subject::Shell),
    ("Grep", Subject::Search),
    ("Task", Subject::Subagent),
];

const GEMINI_TOOLS: &[(&str, Subject)] = &[
    ("read_file", Subject::Read),
    ("read_many_files", Subject::Read),
    ("write_file", Subject::Write),
    ("replace", Subject::Edit),
    ("run_shell_command", Subject::Shell),
    ("grep_search", Subject::Search),
    ("search_file_content", Subject::Search),
    ("glob", Subject::Search),
    ("list_directory", Subject::Search),
    ("google_web_search", Subject::Web),
    ("web_fetch", Subject::Web),
    ("invoke_agent", Subject::Subagent),
    ("read_mcp_resource", Subject::Mcp),
    ("list_mcp_resources", Subject::Mcp),
];

const COPILOT_TOOLS: &[(&str, Subject)] = &[
    ("view", Subject::Read),
    ("create", Subject::Write),
    ("edit", Subject::Edit),
    ("bash", Subject::Shell),
    ("powershell", Subject::Shell),
    ("glob", Subject::Search),
    ("grep", Subject::Search),
    ("web_fetch", Subject::Web),
    ("task", Subject::Subagent),
];

const TRAE_TOOLS: &[(&str, Subject)] = &[
    ("Read", Subject::Read),
    ("Write", Subject::Write),
    ("Edit", Subject::Edit),
    ("RunCommand", Subject::Shell),
    ("Grep", Subject::Search),
    ("Glob", Subject::Search),
    ("LS", Subject::Search),
    ("WebSearch", Subject::Web),
    ("WebFetch", Subject::Web),
];

const CODEX_TOOLS: &[(&str, Subject)] = &[
    ("Bash", Subject::Shell),
    ("apply_patch", Subject::Edit),
    ("Edit", Subject::Edit),
    ("Write", Subject::Write),
    ("spawn_agent", Subject::Subagent),
    ("Agent", Subject::Subagent),
];

const DROID_TOOLS: &[(&str, Subject)] = &[
    ("Read", Subject::Read),
    ("LS", Subject::Read),
    ("Create", Subject::Write),
    ("Edit", Subject::Edit),
    ("ApplyPatch", Subject::Edit),
    ("Execute", Subject::Shell),
    ("Grep", Subject::Search),
    ("Glob", Subject::Search),
    ("WebSearch", Subject::Web),
    ("FetchUrl", Subject::Web),
    ("Task", Subject::Subagent),
];

const VIBE_TOOLS: &[(&str, Subject)] = &[
    ("read_file", Subject::Read),
    ("write_file", Subject::Write),
    ("edit", Subject::Edit),
    ("bash", Subject::Shell),
    ("file_system.bash", Subject::Shell),
    ("powershell", Subject::Shell),
    ("git_bash", Subject::Shell),
    ("grep", Subject::Search),
    ("web_search", Subject::Web),
    ("web_fetch", Subject::Web),
    ("task", Subject::Subagent),
];

const OPENCODE_TOOLS: &[(&str, Subject)] = &[
    ("read", Subject::Read),
    ("skill", Subject::Read),
    ("write", Subject::Write),
    ("edit", Subject::Edit),
    ("apply_patch", Subject::Edit),
    ("bash", Subject::Shell),
    ("execute", Subject::Shell),
    ("grep", Subject::Search),
    ("glob", Subject::Search),
    ("lsp", Subject::Search),
    ("webfetch", Subject::Web),
    ("websearch", Subject::Web),
    ("task", Subject::Subagent),
    ("list_mcp_resources", Subject::Mcp),
    ("read_mcp_resource", Subject::Mcp),
];

const PI_TOOLS: &[(&str, Subject)] = &[
    ("read", Subject::Read),
    ("ls", Subject::Read),
    ("write", Subject::Write),
    ("edit", Subject::Edit),
    ("bash", Subject::Shell),
    ("powershell", Subject::Shell),
    ("grep", Subject::Search),
    ("find", Subject::Search),
];

const OMP_TOOLS: &[(&str, Subject)] = &[
    ("read", Subject::Read),
    ("write", Subject::Write),
    ("edit", Subject::Edit),
    ("ast_edit", Subject::Edit),
    ("bash", Subject::Shell),
    ("eval", Subject::Shell),
    ("grep", Subject::Search),
    ("glob", Subject::Search),
    ("find", Subject::Search),
    ("ast_grep", Subject::Search),
    ("recall", Subject::Search),
    ("web_search", Subject::Web),
    ("github", Subject::Web),
    ("task", Subject::Subagent),
];

const HERMES_TOOLS: &[(&str, Subject)] = &[
    ("read_file", Subject::Read),
    ("write_file", Subject::Write),
    ("patch", Subject::Edit),
    ("terminal", Subject::Shell),
    ("process_manage", Subject::Shell),
    ("search_files", Subject::Search),
    ("session_search", Subject::Search),
    ("web_search", Subject::Web),
    ("web_extract", Subject::Web),
    ("delegate_task", Subject::Subagent),
];

const OPENCLAW_TOOLS: &[(&str, Subject)] = &[
    ("read", Subject::Read),
    ("write", Subject::Write),
    ("edit", Subject::Edit),
    ("apply_patch", Subject::Edit),
    ("exec", Subject::Shell),
    ("terminal", Subject::Shell),
    ("process", Subject::Shell),
    ("code_execution", Subject::Shell),
    ("web_search", Subject::Web),
    ("web_fetch", Subject::Web),
    ("browser", Subject::Web),
    ("subagents", Subject::Subagent),
];

const KILOCODE_TOOLS: &[(&str, Subject)] = &[
    ("read", Subject::Read),
    ("write", Subject::Write),
    ("edit", Subject::Edit),
    ("apply_patch", Subject::Edit),
    ("bash", Subject::Shell),
    ("grep", Subject::Search),
    ("glob", Subject::Search),
    ("webfetch", Subject::Web),
    ("websearch", Subject::Web),
    ("browser_open", Subject::Web),
    ("task", Subject::Subagent),
    ("agent_manager", Subject::Subagent),
];

const CLINE_TOOLS: &[(&str, Subject)] = &[
    ("read_file", Subject::Read),
    ("write_to_file", Subject::Write),
    ("replace_in_file", Subject::Edit),
    ("apply_patch", Subject::Edit),
    ("execute_command", Subject::Shell),
    ("search_files", Subject::Search),
    ("list_files", Subject::Search),
    ("list_code_definition_names", Subject::Search),
    ("web_fetch", Subject::Web),
    ("web_search", Subject::Web),
    ("browser_action", Subject::Web),
    ("new_task", Subject::Subagent),
    ("use_subagents", Subject::Subagent),
    ("use_mcp_tool", Subject::Mcp),
    ("access_mcp_resource", Subject::Mcp),
];

const WINDSURF_TOOLS: &[(&str, Subject)] = &[
    ("read", Subject::Read),
    ("notebook_read", Subject::Read),
    ("write", Subject::Write),
    ("edit", Subject::Edit),
    ("apply_patch", Subject::Edit),
    ("notebook_edit", Subject::Edit),
    ("exec", Subject::Shell),
    ("get_output", Subject::Shell),
    ("write_to_process", Subject::Shell),
    ("kill_shell", Subject::Shell),
    ("grep", Subject::Search),
    ("glob", Subject::Search),
    ("webfetch", Subject::Web),
];

const ANTIGRAVITY_TOOLS: &[(&str, Subject)] = &[
    ("view_file", Subject::Read),
    ("write_to_file", Subject::Write),
    ("replace_file_content", Subject::Edit),
    ("multi_replace_file_content", Subject::Edit),
    ("run_command", Subject::Shell),
    ("grep_search", Subject::Search),
    ("find_by_name", Subject::Search),
    ("list_dir", Subject::Search),
    ("search_web", Subject::Web),
    ("read_url_content", Subject::Web),
    ("invoke_subagent", Subject::Subagent),
    ("define_subagent", Subject::Subagent),
    ("send_message", Subject::Subagent),
    ("manage_subagents", Subject::Subagent),
];

const KIMI_TOOLS: &[(&str, Subject)] = &[
    ("Read", Subject::Read),
    ("ReadMediaFile", Subject::Read),
    ("Write", Subject::Write),
    ("Edit", Subject::Edit),
    ("Bash", Subject::Shell),
    ("Grep", Subject::Search),
    ("Glob", Subject::Search),
    ("WebSearch", Subject::Web),
    ("FetchURL", Subject::Web),
    ("Agent", Subject::Subagent),
    ("AgentSwarm", Subject::Subagent),
];

const BEFORE_REPLACE_INPUT: EventCapability = EventCapability {
    event: Event::BeforeTool,
    can_replace_input: true,
    can_deny: true,
    ..EventCapability::NONE
};

const BEFORE_REPLACE_INPUT_APPEND: EventCapability = EventCapability {
    can_append: true,
    ..BEFORE_REPLACE_INPUT
};

const BEFORE_FULL: EventCapability = EventCapability {
    can_ask: true,
    ..BEFORE_REPLACE_INPUT_APPEND
};

/// The harness can stop a tool call and say why, but not rewrite its arguments.
const BEFORE_DENY_ONLY: EventCapability = EventCapability {
    event: Event::BeforeTool,
    can_deny: true,
    ..EventCapability::NONE
};

/// The common `after_tool`: the result reaches the model untouched, and a hook
/// can only add text beside it. Compression is impossible here — rewrite the
/// command at `before_tool` instead.
const AFTER_APPEND_ONLY: EventCapability = EventCapability {
    event: Event::AfterTool,
    can_append: true,
    ..EventCapability::NONE
};

/// The `after_tool` RTK's posthook needs: the tool result itself is replaceable.
const AFTER_REPLACE_OUTPUT: EventCapability = EventCapability {
    event: Event::AfterTool,
    can_replace_output: true,
    can_append: true,
    ..EventCapability::NONE
};

/// What each supported harness can actually do, checked against its vendor docs.
///
/// `events` is the contract: a position RTK evaluates is only reachable on a
/// harness that declares the matching capability. Absences are real — see
pub static HARNESSES: &[HarnessProfile] = &[
    HarnessProfile {
        id: "claude-code",
        name: "Claude Code",
        docs: Some("https://code.claude.com/docs/en/hooks"),
        verified: None,
        runtime: HookRuntime::Native,
        evaluates: &[Event::BeforeTool, Event::AfterTool],
        events: &[BEFORE_FULL, AFTER_REPLACE_OUTPUT],
        tool_names: CLAUDE_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[
            Channel::AwarenessFile,
            Channel::SessionStart,
            Channel::PerTurn,
        ],
        base_url_config: Some(ConfigChannel::Env {
            var: "ANTHROPIC_BASE_URL",
        }),
        wiring: Some(Wiring {
            path: ".claude/settings.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            user_prompt: Some("UserPromptSubmit"),
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "cursor",
        name: "Cursor",
        docs: Some("https://cursor.com/docs/agent/hooks"),
        verified: Some("2026-10-23"),
        runtime: HookRuntime::Native,
        evaluates: &[Event::BeforeTool],
        events: &[
            BEFORE_REPLACE_INPUT_APPEND,
            EventCapability {
                subjects: &[Subject::Mcp],
                ..AFTER_REPLACE_OUTPUT
            },
        ],
        tool_names: CURSOR_TOOLS,
        mcp: McpNaming::ColonPrefix,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".cursor/hooks.json",
            format: ConfigFormat::FlatEntries,
            before_tool: Some("preToolUse"),
            after_tool: Some("postToolUse"),
            session_start: Some("sessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "gemini",
        name: "Gemini CLI",
        docs: None,
        verified: None,
        runtime: HookRuntime::Native,
        evaluates: &[],
        events: &[
            BEFORE_REPLACE_INPUT,
            EventCapability {
                can_redirect_tool: true,
                ..AFTER_APPEND_ONLY
            },
        ],
        tool_names: GEMINI_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".gemini/settings.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("BeforeTool"),
            after_tool: Some("AfterTool"),
            session_start: Some("SessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "copilot-cli",
        name: "Copilot CLI",
        docs: None,
        verified: None,
        runtime: HookRuntime::Native,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: COPILOT_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".copilot/hooks/rtk.json",
            format: ConfigFormat::FlatEntries,
            before_tool: Some("preToolUse"),
            after_tool: Some("postToolUse"),
            session_start: Some("sessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "vscode-copilot",
        name: "Copilot (VS Code)",
        docs: None,
        verified: None,
        runtime: HookRuntime::Native,
        evaluates: &[],
        events: &[
            EventCapability {
                can_ask: true,
                ..BEFORE_REPLACE_INPUT
            },
            AFTER_APPEND_ONLY,
        ],
        tool_names: COPILOT_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".github/hooks/rtk.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "trae",
        name: "Trae",
        docs: None,
        verified: None,
        runtime: HookRuntime::Native,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_APPEND_ONLY],
        tool_names: TRAE_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".trae/hooks.json",
            format: ConfigFormat::FlatEntries,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "codex",
        name: "Codex",
        docs: Some("https://learn.chatgpt.com/docs/hooks"),
        verified: Some("2026-10-23"),
        runtime: HookRuntime::Native,
        evaluates: &[Event::BeforeTool],
        events: &[BEFORE_REPLACE_INPUT_APPEND, AFTER_APPEND_ONLY],
        tool_names: CODEX_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[Subject::Web],
        session_start: true,
        directive_channels: &[
            Channel::AwarenessFile,
            Channel::SessionStart,
            Channel::PerTurn,
        ],
        base_url_config: Some(ConfigChannel::ConfigKey {
            file: ".codex/config.toml",
            key: "openai_base_url",
        }),
        wiring: Some(Wiring {
            path: ".codex/hooks.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            user_prompt: Some("UserPromptSubmit"),
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "droid",
        name: "Factory Droid",
        docs: None,
        verified: None,
        runtime: HookRuntime::Native,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_APPEND_ONLY],
        tool_names: DROID_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".factory/hooks.json",
            format: ConfigFormat::BareNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "vibe",
        name: "Mistral Vibe",
        docs: Some("https://docs.mistral.ai/vibe/code/cli/hooks"),
        verified: Some("2026-10-23"),
        runtime: HookRuntime::Native,
        evaluates: &[Event::BeforeTool, Event::AfterTool],
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: VIBE_TOOLS,
        mcp: McpNaming::DotServer,
        unreachable: &[],
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::ProviderTable {
            file: ".vibe/config.toml",
            provider: "mistral",
        }),
        wiring: Some(Wiring {
            path: ".vibe/hooks.toml",
            format: ConfigFormat::TomlTables,
            before_tool: Some("pre_tool"),
            after_tool: Some("post_tool"),
            session_start: None,
            user_prompt: None,
            matcher: MatcherStyle::RegexPrefixed,
        }),
    },
    HarnessProfile {
        id: "opencode",
        name: "OpenCode",
        docs: None,
        verified: None,
        runtime: HookRuntime::Plugin,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: OPENCODE_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".opencode/plugin/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool.execute.before"),
            after_tool: Some("tool.execute.after"),
            session_start: None,
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "pi",
        name: "Pi",
        docs: None,
        verified: None,
        runtime: HookRuntime::Plugin,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: PI_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".pi/extensions/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool_call"),
            after_tool: Some("tool_result"),
            session_start: Some("session_start"),
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "omp",
        name: "Oh My Pi",
        docs: None,
        verified: None,
        runtime: HookRuntime::Plugin,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: OMP_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".omp/hooks/pre/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool_call"),
            after_tool: Some("tool_result"),
            session_start: Some("session_start"),
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "hermes",
        name: "Hermes",
        docs: None,
        verified: None,
        runtime: HookRuntime::Plugin,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: HERMES_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".hermes/plugins/rtk/__init__.py",
            format: ConfigFormat::Plugin,
            before_tool: Some("pre_tool_call"),
            after_tool: Some("transform_tool_result"),
            session_start: Some("on_session_start"),
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "openclaw",
        name: "OpenClaw",
        docs: None,
        verified: None,
        runtime: HookRuntime::Plugin,
        evaluates: &[],
        events: &[
            BEFORE_REPLACE_INPUT,
            EventCapability {
                event: Event::AfterTool,
                ..EventCapability::NONE
            },
        ],
        tool_names: OPENCLAW_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".openclaw/extensions/rtk/src/index.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("before_tool_call"),
            after_tool: None,
            session_start: Some("session_start"),
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "kilocode",
        name: "Kilo Code",
        docs: None,
        verified: None,
        runtime: HookRuntime::RulesFile,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: KILOCODE_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".kilo/plugin/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool.execute.before"),
            after_tool: Some("tool.execute.after"),
            session_start: None,
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "cline",
        name: "Cline",
        docs: None,
        verified: None,
        runtime: HookRuntime::RulesFile,
        evaluates: &[],
        events: &[BEFORE_DENY_ONLY, AFTER_APPEND_ONLY],
        tool_names: CLINE_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".clinerules/hooks",
            format: ConfigFormat::Plugin,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("TaskStart"),
            user_prompt: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "windsurf",
        name: "Windsurf / Devin",
        docs: None,
        verified: None,
        runtime: HookRuntime::RulesFile,
        evaluates: &[],
        events: &[BEFORE_REPLACE_INPUT, AFTER_APPEND_ONLY],
        tool_names: WINDSURF_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".devin/hooks.v1.json",
            format: ConfigFormat::BareNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "antigravity",
        name: "Antigravity",
        docs: Some("https://antigravity.google/docs/hooks"),
        verified: Some("2026-10-23"),
        runtime: HookRuntime::Native,
        evaluates: &[Event::BeforeTool],
        events: &[
            EventCapability {
                can_ask: true,
                ..BEFORE_REPLACE_INPUT
            },
            EventCapability {
                event: Event::AfterTool,
                ..EventCapability::NONE
            },
        ],
        tool_names: ANTIGRAVITY_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[
            Channel::AwarenessFile,
            Channel::SessionStart,
            Channel::PerTurn,
        ],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".agents/plugins/rtk/hooks.json",
            format: ConfigFormat::NamedGroups,
            before_tool: Some("PreToolUse"),
            after_tool: None,
            session_start: Some("SessionStart"),
            user_prompt: Some("PreInvocation"),
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "kimi",
        name: "Kimi Code",
        docs: None,
        verified: None,
        runtime: HookRuntime::RulesFile,
        evaluates: &[],
        events: &[
            BEFORE_DENY_ONLY,
            EventCapability {
                event: Event::AfterTool,
                ..EventCapability::NONE
            },
        ],
        tool_names: KIMI_TOOLS,
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".kimi-code/config.toml",
            format: ConfigFormat::TomlTables,
            before_tool: Some("PreToolUse"),
            after_tool: None,
            session_start: Some("SessionStart"),
            user_prompt: None,
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "roo",
        name: "Roo Code",
        docs: None,
        verified: None,
        runtime: HookRuntime::None,
        evaluates: &[],
        events: &[],
        tool_names: &[],
        mcp: McpNaming::DoubleUnderscore,
        unreachable: &[],
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: None,
    },
];

/// The spellings `rtk init --agent <name>` accepts, mapped to harness ids.
/// Both vocabularies reach the same profile so one word means one thing.
const ID_ALIASES: &[(&str, &str)] = &[
    ("claude", "claude-code"),
    ("copilot", "copilot-cli"),
    ("vscode-copilot-chat", "vscode-copilot"),
    ("roo-code", "roo"),
    ("kilo", "kilocode"),
    ("devin", "windsurf"),
];

/// One hook `rtk init` should register on a harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedHook {
    /// The harness's own spelling of the event.
    pub event: &'static str,
    pub which: Position,
    /// The harness's own matcher value, or `None` when it takes no matcher.
    pub matcher: Option<String>,
}

/// What a planned or skipped hook serves. Wider than [`Event`] because session
/// start is a real registration but not a pipeline position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Tool(Event),
    SessionStart,
}

/// A position the pipeline asked for that this harness cannot serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedHook {
    pub which: Position,
    pub subject: Option<Subject>,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatcherPlan {
    pub matcher: Option<String>,
    pub served: Vec<Subject>,
    pub unserved: Vec<Subject>,
}

/// Turn the events a pipeline uses into the hooks this harness can register.
///
/// This is the whole translation: the pipeline names positions and subjects,
/// and each harness spells them its own way. A caller passes what the pipeline
/// uses; it gets back what to write, and what could not be written and why —
/// so `rtk init` can say so instead of installing something inert.
pub fn plan_hooks(
    profile: &HarnessProfile,
    used: &[(Event, Vec<Subject>)],
    wants_session_start: bool,
) -> (Vec<PlannedHook>, Vec<SkippedHook>) {
    let (mut planned, mut skipped) = (Vec::new(), Vec::new());
    let Some(wiring) = profile.wiring else {
        for (event, _) in used {
            skipped.push(SkippedHook {
                which: Position::Tool(*event),
                subject: None,
                reason: "this agent has no hook system",
            });
        }
        if wants_session_start {
            skipped.push(SkippedHook {
                which: Position::SessionStart,
                subject: None,
                reason: "this agent has no hook system",
            });
        }
        return (planned, skipped);
    };

    if wants_session_start {
        match wiring.session_start {
            Some(name) if profile.session_start => planned.push(PlannedHook {
                event: name,
                which: Position::SessionStart,
                matcher: None,
            }),
            _ => skipped.push(SkippedHook {
                which: Position::SessionStart,
                subject: None,
                reason: "this agent has no session-start event",
            }),
        }
    }

    for (event, subjects) in used {
        let name = match event {
            Event::BeforeTool => wiring.before_tool,
            Event::AfterTool => wiring.after_tool,
            Event::Request => continue,
        };
        let Some(name) = name else {
            skipped.push(SkippedHook {
                which: Position::Tool(*event),
                subject: None,
                reason: match event {
                    Event::AfterTool => "this agent cannot change a tool's output",
                    _ => "this agent does not offer this event",
                },
            });
            continue;
        };
        let Some(cap) = profile.event(*event) else {
            skipped.push(SkippedHook {
                which: Position::Tool(*event),
                subject: None,
                reason: "this agent does not offer this event",
            });
            continue;
        };
        if *event == Event::AfterTool && !cap.can_replace_output {
            skipped.push(SkippedHook {
                which: Position::Tool(*event),
                subject: None,
                reason: "this agent cannot change a tool's output",
            });
            continue;
        }
        let evaluated = profile.evaluates.contains(event);
        let mut in_scope: Vec<Subject> = Vec::new();
        for subject in subjects {
            if !evaluated && (*event == Event::AfterTool || *subject != Subject::Shell) {
                skipped.push(SkippedHook {
                    which: Position::Tool(*event),
                    subject: Some(*subject),
                    reason: if *event == Event::AfterTool {
                        "rtk's hook for this agent does not compress tool output"
                    } else {
                        "rtk's hook for this agent runs the command rewrite only"
                    },
                });
            } else if *event == Event::AfterTool
                && !cap.subjects.contains(subject)
                && *subject != Subject::Any
            {
                skipped.push(SkippedHook {
                    which: Position::Tool(*event),
                    subject: Some(*subject),
                    reason: "this agent cannot change this tool's output",
                });
            } else {
                in_scope.push(*subject);
            }
        }
        if in_scope.is_empty() {
            continue;
        }
        let plan = matcher_for(profile, &in_scope);
        for subject in &plan.unserved {
            skipped.push(SkippedHook {
                which: Position::Tool(*event),
                subject: Some(*subject),
                reason: if profile.unreachable.contains(subject) {
                    "this agent's hooks never see this tool"
                } else {
                    "this agent has no known tool for this subject"
                },
            });
        }
        if plan.served.is_empty() {
            continue;
        }
        planned.push(PlannedHook {
            event: name,
            which: Position::Tool(*event),
            matcher: plan.matcher,
        });
    }
    (planned, skipped)
}

fn regex_escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if "\\.+*?()[]{}|^$".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// This harness's matcher for a set of subjects, built from its own tool names.
pub fn matcher_for(profile: &HarnessProfile, subjects: &[Subject]) -> MatcherPlan {
    let Some(wiring) = profile.wiring else {
        return MatcherPlan {
            matcher: None,
            served: Vec::new(),
            unserved: subjects.to_vec(),
        };
    };
    let mcp_token = match profile.mcp {
        McpNaming::DoubleUnderscore => Some("mcp__.*"),
        McpNaming::ColonPrefix => Some("MCP:.*"),
        McpNaming::DotServer => None,
    };
    let mut served: Vec<Subject> = Vec::new();
    let mut unserved: Vec<Subject> = Vec::new();
    for subject in subjects {
        let hit = !profile.unreachable.contains(subject)
            && (*subject == Subject::Any
                || profile
                    .tool_names
                    .iter()
                    .any(|(_, mapped)| mapped == subject)
                || (*subject == Subject::Mcp && mcp_token.is_some()));
        if hit {
            served.push(*subject);
        } else {
            unserved.push(*subject);
        }
    }
    let mut tools: Vec<String> = Vec::new();
    for (tool, mapped) in profile.tool_names {
        if !served.contains(mapped) {
            continue;
        }
        let escaped = match wiring.matcher {
            MatcherStyle::None | MatcherStyle::Alternation => tool.to_string(),
            _ => regex_escape(tool),
        };
        if !tools.contains(&escaped) {
            tools.push(escaped);
        }
    }
    if let (true, Some(token)) = (served.contains(&Subject::Mcp), mcp_token) {
        tools.push(token.to_string());
    }
    let matcher = if wiring.matcher == MatcherStyle::None
        || subjects.contains(&Subject::Any)
        || tools.is_empty()
    {
        None
    } else {
        Some(match wiring.matcher {
            MatcherStyle::AnchoredAlternation => format!("^(?:{})$", tools.join("|")),
            MatcherStyle::RegexPrefixed => format!("re:^(?:{})$", tools.join("|")),
            _ => tools.join("|"),
        })
    };
    MatcherPlan {
        matcher,
        served,
        unserved,
    }
}

pub fn profile(id: &str) -> Option<&'static HarnessProfile> {
    let id = ID_ALIASES
        .iter()
        .find_map(|(alias, target)| (*alias == id).then_some(*target))
        .unwrap_or(id);
    HARNESSES.iter().find(|h| h.id == id)
}

impl HarnessProfile {
    pub fn event(&self, event: Event) -> Option<&'static EventCapability> {
        self.events.iter().find(|e| e.event == event)
    }

    pub fn can_replace_output(&self) -> bool {
        self.events.iter().any(|e| e.can_replace_output)
    }

    pub fn can(&self, event: Event, check: impl Fn(&EventCapability) -> bool) -> bool {
        self.event(event).is_some_and(check)
    }

    pub fn is_mcp_tool(&self, tool_name: &str) -> bool {
        match self.mcp {
            McpNaming::DoubleUnderscore => tool_name.starts_with("mcp__"),
            McpNaming::ColonPrefix => tool_name.starts_with("MCP:"),
            McpNaming::DotServer => {
                tool_name.contains('.') && !self.tool_names.iter().any(|(n, _)| *n == tool_name)
            }
        }
    }

    pub fn mcp_server<'a>(&self, tool_name: &'a str) -> Option<&'a str> {
        if !self.is_mcp_tool(tool_name) {
            return None;
        }
        let server = match self.mcp {
            McpNaming::DoubleUnderscore => tool_name
                .strip_prefix("mcp__")?
                .split("__")
                .next()
                .unwrap_or(""),
            McpNaming::ColonPrefix => return None,
            McpNaming::DotServer => tool_name.split('.').next().unwrap_or(""),
        };
        (!server.is_empty()).then_some(server)
    }

    pub fn map_tool(&self, tool_name: &str) -> Subject {
        if let Some((_, subject)) = self.tool_names.iter().find(|(name, _)| *name == tool_name) {
            return *subject;
        }
        if self.is_mcp_tool(tool_name) {
            return Subject::Mcp;
        }
        Subject::Any
    }

    pub fn tools_for(&self, subject: Subject) -> Vec<&'static str> {
        self.tool_names
            .iter()
            .filter(|(_, mapped)| *mapped == subject)
            .map(|(name, _)| *name)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Transform(Event),
    DenyWithSuggestion(Event),
    Directive(Channel),
    Inert,
}

/// How far RTK can compress `subject`'s output on this harness, best first.
///
/// `subject` matters because a capability can be scoped: Cursor replaces MCP
/// output and nothing else, so asking about a shell tool has to fall through to
/// the next rung rather than claim a transform it cannot perform.
pub fn resolve_output_transform(
    profile: &HarnessProfile,
    subject: Subject,
    proxy_available: bool,
) -> Resolution {
    if let Some(cap) = profile.event(Event::AfterTool) {
        if cap.can_replace_output && cap.subjects.contains(&subject) {
            return Resolution::Transform(Event::AfterTool);
        }
    }
    if proxy_available {
        return Resolution::Transform(Event::Request);
    }
    if let Some(cap) = profile.event(Event::BeforeTool) {
        if cap.can_deny {
            return Resolution::DenyWithSuggestion(Event::BeforeTool);
        }
    }
    if let Some(channel) = profile.directive_channels.first() {
        return Resolution::Directive(*channel);
    }
    Resolution::Inert
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputResolution {
    Rewrite,
    Suggest,
    Directive(Channel),
    Inert,
}

pub fn resolve_input_rewrite(profile: &HarnessProfile) -> InputResolution {
    if let Some(cap) = profile.event(Event::BeforeTool) {
        if cap.can_replace_input {
            return InputResolution::Rewrite;
        }
        if cap.can_deny {
            return InputResolution::Suggest;
        }
    }
    if let Some(channel) = profile.directive_channels.first() {
        return InputResolution::Directive(*channel);
    }
    InputResolution::Inert
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRow {
    pub name: &'static str,
    pub id: &'static str,
    pub pipeline: bool,
    pub rewrite: bool,
    pub block: bool,
    pub compress: Compress,
    pub add_context: bool,
    pub session_start: bool,
    pub proxy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compress {
    Yes,
    McpOnly,
    No,
}

pub fn coverage_row(profile: &HarnessProfile) -> CoverageRow {
    let before = profile.event(Event::BeforeTool);
    let after = profile.event(Event::AfterTool);
    let compress = match after {
        Some(cap) if cap.can_replace_output && cap.subjects == crate::block::HOOK_SUBJECTS => {
            Compress::Yes
        }
        Some(cap) if cap.can_replace_output => Compress::McpOnly,
        _ => Compress::No,
    };
    CoverageRow {
        name: profile.name,
        id: profile.id,
        pipeline: !profile.evaluates.is_empty(),
        rewrite: before.is_some_and(|c| c.can_replace_input),
        block: before.is_some_and(|c| c.can_deny),
        compress,
        add_context: before.is_some_and(|c| c.can_append) || after.is_some_and(|c| c.can_append),
        session_start: profile.session_start,
        proxy: profile.base_url_config.is_some(),
    }
}

pub fn coverage_markdown() -> String {
    let mut rows: Vec<CoverageRow> = HARNESSES.iter().map(coverage_row).collect();
    let rank = |row: &CoverageRow| match (row.compress, row.rewrite) {
        (Compress::Yes, _) => 0,
        (Compress::McpOnly, _) => 1,
        (Compress::No, true) => 2,
        (Compress::No, false) => 3,
    };
    rows.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(b.name)));
    let yes_no = |flag: bool| if flag { "yes" } else { "—" };
    let mut out = String::from(
        "| Agent | `rtk init` id | Pipeline rules | Rewrite the command | Block it | Compress tool output | Add context | Session start | Route via proxy |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    for row in rows {
        let compress = match row.compress {
            Compress::Yes => "**yes**",
            Compress::McpOnly => "MCP only",
            Compress::No => "—",
        };
        out.push_str(&format!(
            "| **{}** | `{}` | {} | {} | {} | {} | {} | {} | {} |\n",
            row.name,
            row.id,
            yes_no(row.pipeline),
            yes_no(row.rewrite),
            yes_no(row.block),
            compress,
            yes_no(row.add_context),
            yes_no(row.session_start),
            yes_no(row.proxy),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_has_a_unique_id() {
        for (i, a) in HARNESSES.iter().enumerate() {
            for b in &HARNESSES[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.name, b.name);
            }
        }
    }

    #[test]
    fn output_replacers_are_the_harnesses_whose_docs_confirm_it() {
        let mut replacers: Vec<&str> = HARNESSES
            .iter()
            .filter(|h| {
                h.event(Event::AfterTool).is_some_and(|c| {
                    c.can_replace_output && c.subjects == crate::block::HOOK_SUBJECTS
                })
            })
            .map(|h| h.id)
            .collect();
        replacers.sort_unstable();
        assert_eq!(
            replacers,
            vec![
                "claude-code",
                "copilot-cli",
                "hermes",
                "kilocode",
                "omp",
                "opencode",
                "pi",
                "vibe",
            ]
        );
    }

    #[test]
    fn claude_code_tool_names_cover_the_agnostic_subjects() {
        let claude = profile("claude-code").unwrap();
        assert_eq!(claude.map_tool("Bash"), Subject::Shell);
        assert_eq!(claude.map_tool("Read"), Subject::Read);
        assert_eq!(claude.map_tool("NotebookRead"), Subject::Read);
        assert_eq!(claude.map_tool("Grep"), Subject::Search);
        assert_eq!(claude.map_tool("Glob"), Subject::Search);
        assert_eq!(claude.map_tool("WebFetch"), Subject::Web);
        assert_eq!(claude.map_tool("WebSearch"), Subject::Web);
        assert_eq!(claude.map_tool("MultiEdit"), Subject::Edit);
        assert_eq!(claude.map_tool("Task"), Subject::Subagent);
        assert_eq!(claude.map_tool("mcp__linear__create_issue"), Subject::Mcp);
        assert_eq!(claude.map_tool("SomethingNew"), Subject::Any);
    }

    #[test]
    fn mcp_naming_follows_each_harness() {
        let claude = profile("claude-code").unwrap();
        assert_eq!(
            claude.mcp_server("mcp__linear__create_issue"),
            Some("linear")
        );
        assert_eq!(claude.mcp_server("Bash"), None);

        let cursor = profile("cursor").unwrap();
        assert_eq!(cursor.map_tool("MCP:create_issue"), Subject::Mcp);
        assert_eq!(cursor.map_tool("mcp__linear__create_issue"), Subject::Any);
        assert_eq!(cursor.mcp_server("MCP:create_issue"), None);

        let vibe = profile("vibe").unwrap();
        assert_eq!(vibe.map_tool("linear.create_issue"), Subject::Mcp);
        assert_eq!(vibe.mcp_server("linear.create_issue"), Some("linear"));
        assert_eq!(vibe.map_tool("file_system.bash"), Subject::Shell);
        assert_eq!(vibe.map_tool("bash"), Subject::Shell);
    }

    #[test]
    fn cursor_replaces_mcp_output_and_nothing_else() {
        let cursor = profile("cursor").unwrap();
        let after = cursor.event(Event::AfterTool).unwrap();
        assert!(after.can_replace_output);
        assert_eq!(after.subjects, &[Subject::Mcp]);
        assert_eq!(
            resolve_output_transform(cursor, Subject::Mcp, false),
            Resolution::Transform(Event::AfterTool)
        );
        assert_eq!(
            resolve_output_transform(cursor, Subject::Shell, false),
            Resolution::DenyWithSuggestion(Event::BeforeTool)
        );
    }

    #[test]
    fn degradation_ladder_orders_transform_proxy_deny_directive() {
        let claude = profile("claude-code").unwrap();
        assert_eq!(
            resolve_output_transform(claude, Subject::Shell, false),
            Resolution::Transform(Event::AfterTool)
        );

        let codex = profile("codex").unwrap();
        assert_eq!(
            resolve_output_transform(codex, Subject::Shell, true),
            Resolution::Transform(Event::Request)
        );
        assert_eq!(
            resolve_output_transform(codex, Subject::Shell, false),
            Resolution::DenyWithSuggestion(Event::BeforeTool)
        );

        let roo = profile("roo").unwrap();
        assert_eq!(
            resolve_output_transform(roo, Subject::Shell, false),
            Resolution::Directive(Channel::AwarenessFile)
        );
    }

    #[test]
    fn input_ladder_orders_rewrite_suggest_directive() {
        for id in ["claude-code", "codex", "cursor", "antigravity", "vibe"] {
            assert_eq!(
                resolve_input_rewrite(profile(id).unwrap()),
                InputResolution::Rewrite,
                "{id}"
            );
        }
        assert_eq!(
            resolve_input_rewrite(profile("cline").unwrap()),
            InputResolution::Suggest
        );
        assert_eq!(
            resolve_input_rewrite(profile("roo").unwrap()),
            InputResolution::Directive(Channel::AwarenessFile)
        );
    }

    #[test]
    fn every_redirectable_harness_records_its_base_url_channel() {
        let mut routable: Vec<&str> = HARNESSES
            .iter()
            .filter(|h| h.base_url_config.is_some())
            .map(|h| h.id)
            .collect();
        routable.sort_unstable();
        assert_eq!(
            routable,
            vec![
                "claude-code",
                "cline",
                "codex",
                "copilot-cli",
                "gemini",
                "hermes",
                "kilocode",
                "kimi",
                "omp",
                "openclaw",
                "roo",
                "vibe",
            ]
        );
        let channel = |id: &str| profile(id).unwrap().base_url_config;
        assert_eq!(
            channel("claude-code"),
            Some(ConfigChannel::Env {
                var: "ANTHROPIC_BASE_URL"
            })
        );
        assert_eq!(
            channel("codex"),
            Some(ConfigChannel::ConfigKey {
                file: ".codex/config.toml",
                key: "openai_base_url"
            })
        );
        assert_eq!(
            channel("vibe"),
            Some(ConfigChannel::ProviderTable {
                file: ".vibe/config.toml",
                provider: "mistral"
            })
        );
        for id in ["cursor", "antigravity"] {
            assert_eq!(channel(id), None, "{id} offers no base-URL override");
        }
    }

    #[test]
    fn a_hookless_harness_still_reaches_the_awareness_channel() {
        for h in HARNESSES.iter().filter(|h| h.events.is_empty()) {
            assert!(h.directive_channels.contains(&Channel::AwarenessFile));
            assert_eq!(
                resolve_output_transform(h, Subject::Shell, false),
                Resolution::Directive(Channel::AwarenessFile)
            );
        }
    }

    #[test]
    fn every_harness_maps_its_own_shell_tool() {
        for h in HARNESSES.iter().filter(|h| !h.tool_names.is_empty()) {
            assert!(
                h.tool_names.iter().any(|(_, s)| *s == Subject::Shell),
                "{} has a tool map with no shell tool",
                h.id
            );
        }
    }

    #[test]
    fn pipeline_harnesses_are_native_and_verified() {
        let mut evaluating: Vec<&str> = HARNESSES
            .iter()
            .filter(|h| !h.evaluates.is_empty())
            .inspect(|h| {
                assert_eq!(h.runtime, HookRuntime::Native, "{}", h.id);
                assert!(
                    h.docs.is_some(),
                    "{} evaluates pipelines without docs",
                    h.id
                );
            })
            .map(|h| h.id)
            .collect();
        evaluating.sort_unstable();
        assert_eq!(
            evaluating,
            vec!["antigravity", "claude-code", "codex", "cursor", "vibe"]
        );
    }

    #[test]
    fn one_pipeline_translates_into_each_harness_own_spelling() {
        let used = vec![(Event::BeforeTool, vec![Subject::Shell])];
        let got = |id: &str| {
            let p = profile(id).unwrap();
            let (planned, _) = plan_hooks(p, &used, false);
            planned
                .into_iter()
                .map(|h| (h.event, h.matcher.unwrap_or_default()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            got("claude-code"),
            vec![("PreToolUse", "^(?:Bash|PowerShell|Monitor)$".to_string())]
        );
        assert_eq!(got("cursor"), vec![("preToolUse", "Shell".to_string())]);
        assert_eq!(
            got("gemini"),
            vec![("BeforeTool", "^(?:run_shell_command)$".to_string())]
        );
        assert_eq!(got("codex"), vec![("PreToolUse", "^(?:Bash)$".to_string())]);
        assert_eq!(got("droid"), vec![("PreToolUse", "Execute".to_string())]);
        assert_eq!(got("trae"), vec![("PreToolUse", "RunCommand".to_string())]);
        assert_eq!(
            got("vibe"),
            vec![(
                "pre_tool",
                "re:^(?:bash|file_system\\.bash|powershell|git_bash)$".to_string()
            )]
        );
        assert_eq!(
            got("antigravity"),
            vec![("PreToolUse", "run_command".to_string())]
        );
        assert_eq!(got("kimi"), vec![("PreToolUse", "Bash".to_string())]);
    }

    #[test]
    fn a_harness_that_ignores_matchers_is_registered_without_one() {
        let used = vec![(Event::BeforeTool, vec![Subject::Shell])];
        let p = profile("vscode-copilot").unwrap();
        let (planned, _) = plan_hooks(p, &used, false);
        assert_eq!(planned[0].matcher, None);
    }

    #[test]
    fn a_subject_without_a_tool_is_skipped_not_registered_as_catch_all() {
        let used = vec![(Event::BeforeTool, vec![Subject::Web])];
        let (planned, skipped) = plan_hooks(profile("codex").unwrap(), &used, false);
        assert!(planned.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].subject, Some(Subject::Web));
        assert_eq!(skipped[0].reason, "this agent's hooks never see this tool");

        let (planned, skipped) = plan_hooks(profile("cursor").unwrap(), &used, false);
        assert!(planned.is_empty());
        assert_eq!(
            skipped[0].reason,
            "this agent has no known tool for this subject"
        );
    }

    #[test]
    fn a_mixed_event_registers_the_served_subjects_and_reports_the_rest() {
        let used = vec![(Event::BeforeTool, vec![Subject::Shell, Subject::Web])];
        let (planned, skipped) = plan_hooks(profile("codex").unwrap(), &used, false);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].matcher.as_deref(), Some("^(?:Bash)$"));
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].subject, Some(Subject::Web));
    }

    #[test]
    fn mcp_subject_matches_by_each_harness_spelling() {
        let used = vec![(Event::BeforeTool, vec![Subject::Mcp])];
        let matcher = |id: &str| {
            plan_hooks(profile(id).unwrap(), &used, false)
                .0
                .into_iter()
                .next()
                .and_then(|h| h.matcher)
        };
        let claude = matcher("claude-code").unwrap();
        assert!(claude.starts_with("^(?:ToolSearch|") && claude.ends_with("|mcp__.*)$"));
        assert_eq!(matcher("cursor").as_deref(), Some("MCP:.*"));
        assert_eq!(matcher("vibe"), None);
        let (_, skipped) = plan_hooks(profile("vibe").unwrap(), &used, false);
        assert_eq!(skipped[0].subject, Some(Subject::Mcp));
    }

    #[test]
    fn an_after_tool_rule_is_skipped_where_output_cannot_be_changed() {
        let used = vec![(Event::AfterTool, vec![Subject::Shell])];
        for id in ["codex", "trae", "droid", "windsurf", "antigravity"] {
            let p = profile(id).unwrap();
            let (planned, skipped) = plan_hooks(p, &used, false);
            assert!(planned.is_empty(), "{id} should register nothing");
            assert_eq!(
                skipped[0].reason,
                "this agent cannot change a tool's output"
            );
        }
    }

    #[test]
    fn an_after_tool_rule_is_registered_where_rtk_can_change_output() {
        let used = vec![(Event::AfterTool, vec![Subject::Shell])];
        for id in ["claude-code", "vibe"] {
            let p = profile(id).unwrap();
            let (planned, _) = plan_hooks(p, &used, false);
            assert_eq!(planned.len(), 1, "{id} should register an after_tool hook");
        }
        for id in ["copilot-cli", "hermes", "kilocode", "opencode", "pi", "omp"] {
            let p = profile(id).unwrap();
            let (planned, skipped) = plan_hooks(p, &used, false);
            assert!(planned.is_empty(), "{id}");
            assert_eq!(
                skipped[0].reason, "rtk's hook for this agent does not compress tool output",
                "{id}"
            );
        }
    }

    #[test]
    fn a_rewrite_only_hook_still_registers_the_shell_rewrite() {
        let used = vec![(Event::BeforeTool, vec![Subject::Shell, Subject::Edit])];
        let (planned, skipped) = plan_hooks(profile("gemini").unwrap(), &used, false);
        assert_eq!(planned.len(), 1);
        assert_eq!(
            planned[0].matcher.as_deref(),
            Some("^(?:run_shell_command)$")
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].subject, Some(Subject::Edit));
    }

    #[test]
    fn an_after_tool_rule_outside_the_replaceable_subjects_is_skipped() {
        let cursor_with_posthook = HarnessProfile {
            evaluates: &[Event::BeforeTool, Event::AfterTool],
            ..*profile("cursor").unwrap()
        };
        let used = vec![(Event::AfterTool, vec![Subject::Shell, Subject::Mcp])];
        let (planned, skipped) = plan_hooks(&cursor_with_posthook, &used, false);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].matcher.as_deref(), Some("MCP:.*"));
        assert!(skipped.iter().any(|s| s.subject == Some(Subject::Shell)
            && s.reason == "this agent cannot change this tool's output"));
    }

    #[test]
    fn a_hookless_harness_reports_every_position_as_unservable() {
        let used = vec![
            (Event::BeforeTool, vec![Subject::Shell]),
            (Event::AfterTool, vec![Subject::Shell]),
        ];
        let (planned, skipped) = plan_hooks(profile("roo").unwrap(), &used, false);
        assert!(planned.is_empty());
        assert_eq!(skipped.len(), 2);
        assert!(
            skipped
                .iter()
                .all(|s| s.reason == "this agent has no hook system")
        );
    }

    #[test]
    fn every_wired_harness_can_spell_before_tool_and_name_its_shell_tool() {
        let used = vec![(Event::BeforeTool, vec![Subject::Shell])];
        for h in HARNESSES.iter().filter(|h| h.wiring.is_some()) {
            let (planned, skipped) = plan_hooks(h, &used, false);
            assert!(
                !planned.is_empty(),
                "{} is wired but plans no before_tool hook ({skipped:?})",
                h.id
            );
        }
    }

    #[test]
    fn session_start_registers_only_where_the_harness_has_one() {
        let used: Vec<(Event, Vec<Subject>)> = Vec::new();
        let mut with: Vec<&str> = Vec::new();
        let mut without: Vec<&str> = Vec::new();
        for h in HARNESSES {
            let (planned, skipped) = plan_hooks(h, &used, true);
            if planned.iter().any(|p| p.which == Position::SessionStart) {
                with.push(h.id);
            } else {
                assert!(
                    skipped.iter().any(|s| s.which == Position::SessionStart),
                    "{} neither registered nor reported session start",
                    h.id
                );
                without.push(h.id);
            }
        }
        for id in ["claude-code", "cursor", "codex", "antigravity"] {
            assert!(with.contains(&id), "{id}");
        }
        for id in ["vibe", "kilocode", "opencode"] {
            assert!(without.contains(&id), "{id}");
        }
    }

    #[test]
    fn session_start_carries_each_harness_own_spelling() {
        let used: Vec<(Event, Vec<Subject>)> = Vec::new();
        let name = |id: &str| {
            plan_hooks(profile(id).unwrap(), &used, true)
                .0
                .into_iter()
                .find(|p| p.which == Position::SessionStart)
                .map(|p| p.event)
        };
        assert_eq!(name("claude-code"), Some("SessionStart"));
        assert_eq!(name("cursor"), Some("sessionStart"));
        assert_eq!(name("gemini"), Some("SessionStart"));
        assert_eq!(name("antigravity"), Some("SessionStart"));
        assert_eq!(name("pi"), Some("session_start"));
        assert_eq!(name("hermes"), Some("on_session_start"));
        assert_eq!(name("vibe"), None);
    }

    #[test]
    fn per_turn_channel_implies_a_user_prompt_event() {
        for h in HARNESSES {
            let claims = h.directive_channels.contains(&Channel::PerTurn);
            let spelled = h.wiring.is_some_and(|w| w.user_prompt.is_some());
            assert_eq!(claims, spelled, "{}", h.id);
        }
    }

    #[test]
    fn a_proxy_position_registers_no_hook_on_any_harness() {
        let used = vec![(Event::Request, vec![Subject::ToolResult])];
        for h in HARNESSES {
            let (planned, _) = plan_hooks(h, &used, false);
            assert!(
                planned.is_empty(),
                "{} planned a hook for a proxy position",
                h.id
            );
        }
    }

    #[test]
    fn antigravity_rewrites_and_asks_but_never_touches_output() {
        let p = profile("antigravity").unwrap();
        let before = p.event(Event::BeforeTool).unwrap();
        assert!(before.can_replace_input && before.can_deny && before.can_ask);
        assert!(!p.can_replace_output());
        assert!(p.session_start);
    }

    #[test]
    fn codex_cannot_ask_and_never_sees_web_tools() {
        let p = profile("codex").unwrap();
        let before = p.event(Event::BeforeTool).unwrap();
        assert!(before.can_replace_input && before.can_deny && before.can_append);
        assert!(!before.can_ask);
        assert!(p.unreachable.contains(&Subject::Web));
    }

    #[test]
    fn the_published_coverage_table_is_the_generated_one() {
        let doc =
            include_str!("../../docs/guide/resources/agent-coverage.md").replace("\r\n", "\n");
        let table = coverage_markdown();
        assert!(
            doc.contains(&table),
            "docs/guide/resources/agent-coverage.md is out of date; paste this table between its markers:\n{table}"
        );
    }
}
