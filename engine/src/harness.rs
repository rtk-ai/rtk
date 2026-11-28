use crate::block::{Event, Subject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    AwarenessFile,
    SessionStart,
    PerTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigChannel {
    SettingsEnv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCapability {
    pub event: Event,
    pub can_replace_input: bool,
    pub can_replace_output: bool,
    pub can_deny: bool,
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
        can_append: false,
        can_redirect_tool: false,
        subjects: crate::block::HOOK_SUBJECTS,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessProfile {
    pub id: &'static str,
    pub events: &'static [EventCapability],
    pub tool_names: &'static [(&'static str, Subject)],
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
    ("WebFetch", Subject::Web),
    ("WebSearch", Subject::Web),
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
/// `docs/guide/resources/agent-coverage.md` for the user-facing table.
pub static HARNESSES: &[HarnessProfile] = &[
    HarnessProfile {
        id: "claude-code",
        events: &[
            BEFORE_REPLACE_INPUT,
            // updatedToolOutput must match the tool's own output schema or it is
            // silently ignored; PostToolUse fires only on success.
            AFTER_REPLACE_OUTPUT,
        ],
        tool_names: CLAUDE_TOOLS,
        session_start: true,
        directive_channels: &[
            Channel::AwarenessFile,
            Channel::SessionStart,
            Channel::PerTurn,
        ],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".claude/settings.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "cursor",
        events: &[
            BEFORE_REPLACE_INPUT,
            // gap: updated_mcp_tool_output is MCP-only. Built-in tool output can
            // only be appended to, never replaced.
            EventCapability {
                subjects: &[Subject::Mcp],
                ..AFTER_REPLACE_OUTPUT
            },
        ],
        tool_names: CURSOR_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".cursor/hooks.json",
            format: ConfigFormat::FlatEntries,
            before_tool: Some("preToolUse"),
            after_tool: Some("postToolUse"),
            session_start: Some("sessionStart"),
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "gemini",
        events: &[
            // gap: input edits merge rather than replace, and each one appends a
            // "[System] Tool input parameters were modified" note to the output.
            BEFORE_REPLACE_INPUT,
            // gap: no field rewrites llmContent. tailToolCallRequest can run a
            // replacement tool, which is a redirect rather than a rewrite.
            EventCapability {
                can_redirect_tool: true,
                ..AFTER_APPEND_ONLY
            },
        ],
        tool_names: GEMINI_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".gemini/settings.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("BeforeTool"),
            after_tool: Some("AfterTool"),
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "copilot-cli",
        events: &[
            BEFORE_REPLACE_INPUT,
            // modifiedResult is documented but unverified against a shipped binary,
            // and GitHub's own tutorial page contradicts the reference.
            AFTER_REPLACE_OUTPUT,
        ],
        tool_names: COPILOT_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".copilot/hooks/rtk.json",
            format: ConfigFormat::FlatEntries,
            before_tool: Some("preToolUse"),
            after_tool: Some("postToolUse"),
            session_start: Some("sessionStart"),
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "vscode-copilot",
        events: &[
            BEFORE_REPLACE_INPUT,
            // gap: the Local harness has no modifiedResult, unlike Copilot CLI.
            AFTER_APPEND_ONLY,
        ],
        tool_names: COPILOT_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".github/hooks/rtk.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "trae",
        events: &[
            BEFORE_REPLACE_INPUT,
            // gap: a finished tool call "cannot be undone" — append or block only.
            AFTER_APPEND_ONLY,
        ],
        tool_names: TRAE_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".trae/hooks.json",
            format: ConfigFormat::FlatEntries,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "codex",
        events: &[
            // updatedInput is honoured only alongside permissionDecision "allow";
            // without it the hook fails and the original command runs.
            BEFORE_REPLACE_INPUT,
            // gap: updatedToolOutput is parsed but unsupported — Codex marks the
            // hook failed and keeps the original result (openai/codex#38135).
            AFTER_APPEND_ONLY,
        ],
        tool_names: CODEX_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".codex/hooks.json",
            format: ConfigFormat::ClaudeNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "droid",
        events: &[BEFORE_REPLACE_INPUT, AFTER_APPEND_ONLY],
        tool_names: DROID_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".factory/hooks.json",
            format: ConfigFormat::BareNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "vibe",
        events: &[
            BEFORE_REPLACE_INPUT,
            // Replacement rides on decision "deny" + reason and reaches only
            // tool_output_text; the structured result stays read-only.
            AFTER_REPLACE_OUTPUT,
        ],
        tool_names: VIBE_TOOLS,
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".vibe/hooks.toml",
            format: ConfigFormat::TomlTables,
            before_tool: Some("pre_tool"),
            after_tool: Some("post_tool"),
            session_start: None,
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "opencode",
        events: &[
            // args must be mutated in place; reassigning the object is ignored.
            BEFORE_REPLACE_INPUT,
            // gap: MCP results are rebuilt after the hook, so only built-in tool
            // output is replaceable — and it arrives already truncated.
            AFTER_REPLACE_OUTPUT,
        ],
        tool_names: OPENCODE_TOOLS,
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".opencode/plugin/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool.execute.before"),
            after_tool: Some("tool.execute.after"),
            session_start: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "pi",
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: PI_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".pi/extensions/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool_call"),
            after_tool: Some("tool_result"),
            session_start: Some("session_start"),
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "omp",
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: OMP_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".omp/hooks/pre/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool_call"),
            after_tool: Some("tool_result"),
            session_start: Some("session_start"),
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "hermes",
        events: &[
            BEFORE_REPLACE_INPUT,
            // transform_tool_result replaces the result; post_tool_call itself is
            // an observer. Known not to fire for agent-runtime tools.
            AFTER_REPLACE_OUTPUT,
        ],
        tool_names: HERMES_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".hermes/plugins/rtk/__init__.py",
            format: ConfigFormat::Plugin,
            before_tool: Some("pre_tool_call"),
            after_tool: Some("transform_tool_result"),
            session_start: Some("on_session_start"),
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "openclaw",
        events: &[
            BEFORE_REPLACE_INPUT,
            // gap: after_tool_call is observe-only. tool_result_persist rewrites
            // the transcript, not what the model reads.
            EventCapability {
                event: Event::AfterTool,
                ..EventCapability::NONE
            },
        ],
        tool_names: OPENCLAW_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".openclaw/extensions/rtk/src/index.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("before_tool_call"),
            after_tool: None,
            session_start: Some("session_start"),
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "kilocode",
        events: &[BEFORE_REPLACE_INPUT, AFTER_REPLACE_OUTPUT],
        tool_names: KILOCODE_TOOLS,
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".kilo/plugin/rtk.ts",
            format: ConfigFormat::Plugin,
            before_tool: Some("tool.execute.before"),
            after_tool: Some("tool.execute.after"),
            session_start: None,
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "cline",
        events: &[
            // gap: the VS Code extension drops overrideInput and cannot replace
            // output. Both need the SDK/CLI plugin runtime, which is a different
            // install than the one `rtk init --agent cline` targets.
            BEFORE_DENY_ONLY,
            AFTER_APPEND_ONLY,
        ],
        tool_names: CLINE_TOOLS,
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".clinerules/hooks",
            format: ConfigFormat::Plugin,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("TaskStart"),
            matcher: MatcherStyle::None,
        }),
    },
    HarnessProfile {
        id: "windsurf",
        events: &[
            // Windsurf is now Devin Desktop; Cascade was removed in 2026-09.
            // Pre-removal installs are block-only, with no input rewrite.
            BEFORE_REPLACE_INPUT,
            AFTER_APPEND_ONLY,
        ],
        tool_names: WINDSURF_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile, Channel::SessionStart],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".devin/hooks.v1.json",
            format: ConfigFormat::BareNested,
            before_tool: Some("PreToolUse"),
            after_tool: Some("PostToolUse"),
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::AnchoredAlternation,
        }),
    },
    HarnessProfile {
        id: "antigravity",
        events: &[
            // gap: hooks can gate a call but never rewrite its arguments.
            BEFORE_DENY_ONLY,
            // gap: PostToolUse returns {} — tool output is immutable.
            EventCapability {
                event: Event::AfterTool,
                ..EventCapability::NONE
            },
        ],
        tool_names: ANTIGRAVITY_TOOLS,
        session_start: false,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: None,
        wiring: Some(Wiring {
            path: ".agents/hooks.json",
            format: ConfigFormat::NamedGroups,
            before_tool: Some("PreToolUse"),
            after_tool: None,
            session_start: None,
            matcher: MatcherStyle::Alternation,
        }),
    },
    HarnessProfile {
        id: "kimi",
        events: &[
            // gap: no input rewrite anywhere in the hook API.
            BEFORE_DENY_ONLY,
            // gap: every event but PreToolUse/Stop/UserPromptSubmit is fire-and-forget.
            EventCapability {
                event: Event::AfterTool,
                ..EventCapability::NONE
            },
        ],
        tool_names: KIMI_TOOLS,
        session_start: true,
        directive_channels: &[Channel::AwarenessFile],
        base_url_config: Some(ConfigChannel::SettingsEnv),
        wiring: Some(Wiring {
            path: ".kimi-code/config.toml",
            format: ConfigFormat::TomlTables,
            before_tool: Some("PreToolUse"),
            after_tool: None,
            session_start: Some("SessionStart"),
            matcher: MatcherStyle::Alternation,
        }),
    },
    // Roo Code is archived (read-only since 2026-05) and never shipped hooks;
    // it stays an awareness-file integration with no events.
    HarnessProfile {
        id: "roo",
        events: &[],
        tool_names: &[],
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
    pub reason: &'static str,
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
                reason: "this agent has no hook system",
            });
        }
        if wants_session_start {
            skipped.push(SkippedHook {
                which: Position::SessionStart,
                reason: "this agent has no hook system",
            });
        }
        return (planned, skipped);
    };

    if wants_session_start {
        match wiring.session_start {
            // `profile.session_start` is the second gate: a harness can name
            // the event and still not deliver one we can use.
            Some(name) if profile.session_start => planned.push(PlannedHook {
                event: name,
                which: Position::SessionStart,
                matcher: None,
            }),
            _ => skipped.push(SkippedHook {
                which: Position::SessionStart,
                reason: "this agent has no session-start event",
            }),
        }
    }

    for (event, subjects) in used {
        let name = match event {
            Event::BeforeTool => wiring.before_tool,
            Event::AfterTool => wiring.after_tool,
            // Proxy positions are served by the daemon; there is no hook to
            // register for them on any harness.
            Event::Request => continue,
        };
        let Some(name) = name else {
            skipped.push(SkippedHook {
                which: Position::Tool(*event),
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
                reason: "this agent does not offer this event",
            });
            continue;
        };
        if *event == Event::AfterTool && !cap.can_replace_output {
            skipped.push(SkippedHook {
                which: Position::Tool(*event),
                reason: "this agent cannot change a tool's output",
            });
            continue;
        }
        planned.push(PlannedHook {
            event: name,
            which: Position::Tool(*event),
            matcher: matcher_for(profile, subjects),
        });
    }
    (planned, skipped)
}

/// This harness's matcher for a set of subjects, built from its own tool names.
///
/// `None` means "register without a matcher": either the harness ignores them,
/// or every tool it has is in scope and naming them all would only go stale.
pub fn matcher_for(profile: &HarnessProfile, subjects: &[Subject]) -> Option<String> {
    let wiring = profile.wiring?;
    if wiring.matcher == MatcherStyle::None {
        return None;
    }
    if subjects.contains(&Subject::Any) {
        return None;
    }
    let mut tools: Vec<&str> = Vec::new();
    for (tool, subject) in profile.tool_names {
        if subjects.contains(subject) && !tools.contains(tool) {
            tools.push(tool);
        }
    }
    if subjects.contains(&Subject::Mcp) {
        tools.push("mcp__.*");
    }
    if tools.is_empty() {
        return None;
    }
    Some(match wiring.matcher {
        MatcherStyle::AnchoredAlternation => format!("^(?:{})$", tools.join("|")),
        _ => tools.join("|"),
    })
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

    pub fn map_tool(&self, tool_name: &str) -> Subject {
        if let Some((_, subject)) = self.tool_names.iter().find(|(name, _)| *name == tool_name) {
            return *subject;
        }
        if tool_name.starts_with("mcp__") {
            return Subject::Mcp;
        }
        Subject::Any
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_has_a_unique_id() {
        for (i, a) in HARNESSES.iter().enumerate() {
            for b in &HARNESSES[i + 1..] {
                assert_ne!(a.id, b.id);
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

        // No after_tool replacement: the proxy is the next rung, then a spoken deny.
        let codex = profile("codex").unwrap();
        assert_eq!(
            resolve_output_transform(codex, Subject::Shell, true),
            Resolution::Transform(Event::Request)
        );
        assert_eq!(
            resolve_output_transform(codex, Subject::Shell, false),
            Resolution::DenyWithSuggestion(Event::BeforeTool)
        );

        // Archived, hookless: only the awareness file is left.
        let roo = profile("roo").unwrap();
        assert_eq!(
            resolve_output_transform(roo, Subject::Shell, false),
            Resolution::Directive(Channel::AwarenessFile)
        );
    }

    #[test]
    fn base_url_channels_match_the_documented_env_vars() {
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
            ]
        );
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
        assert_eq!(
            got("cursor"),
            vec![("preToolUse", "^(?:Shell)$".to_string())]
        );
        assert_eq!(
            got("gemini"),
            vec![("BeforeTool", "^(?:run_shell_command)$".to_string())]
        );
        assert_eq!(got("codex"), vec![("PreToolUse", "^(?:Bash)$".to_string())]);
        assert_eq!(got("droid"), vec![("PreToolUse", "Execute".to_string())]);
        assert_eq!(got("trae"), vec![("PreToolUse", "RunCommand".to_string())]);
        assert_eq!(
            got("vibe"),
            vec![("pre_tool", "bash|powershell|git_bash".to_string())]
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
    fn an_after_tool_rule_is_skipped_where_output_cannot_be_changed() {
        let used = vec![(Event::AfterTool, vec![Subject::Shell])];
        for id in ["codex", "trae", "droid", "windsurf"] {
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
    fn an_after_tool_rule_is_registered_where_output_can_be_changed() {
        let used = vec![(Event::AfterTool, vec![Subject::Shell])];
        for id in [
            "claude-code",
            "copilot-cli",
            "hermes",
            "kilocode",
            "opencode",
            "pi",
            "omp",
        ] {
            let p = profile(id).unwrap();
            let (planned, _) = plan_hooks(p, &used, false);
            assert_eq!(planned.len(), 1, "{id} should register an after_tool hook");
        }
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
        // vibe, kilocode, antigravity and opencode have no session-start event;
        // cline's is TaskStart, which the profile does not claim.
        assert!(with.contains(&"claude-code") && with.contains(&"cursor"));
        assert!(without.contains(&"vibe") && without.contains(&"antigravity"));
        assert!(without.contains(&"kilocode") && without.contains(&"opencode"));
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
        assert_eq!(name("pi"), Some("session_start"));
        assert_eq!(name("hermes"), Some("on_session_start"));
        assert_eq!(name("vibe"), None);
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
}
