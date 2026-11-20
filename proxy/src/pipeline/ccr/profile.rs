//! Layer P: the one agent-specific layer. Content classification needs no
//! names; argument compression and anchor protection need field semantics,
//! which differ per agent. Default-deny: an unrecognised agent, tool or
//! field is never compressed — a missing profile entry costs yield, never
//! correctness.

/// What a tool does with the filesystem/environment, driving its floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Returns exact file bytes; eliding them costs paged recovery reads.
    ReadFile,
    /// Bulk output whose recovery is one cheap re-run.
    Bulk,
    /// Unknown recovery cost — conservative floor.
    Unknown,
}

/// Role of one `tool_use` input field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRole {
    /// Byte-exact search anchor the model must reproduce verbatim. Never touched.
    Anchor,
    /// File body the tool wrote to disk; the file itself is the recovery path.
    WrittenBody,
    /// Command text; compressible through the shape pipeline.
    Command,
    /// Unrecognised — never touched.
    Opaque,
}

pub struct AgentProfile {
    pub name: &'static str,
    field_role: fn(&str, &str) -> FieldRole,
    capability: fn(&str) -> Capability,
}

impl AgentProfile {
    pub fn field_role(&self, tool: &str, field: &str) -> FieldRole {
        if field.starts_with("old_") {
            return FieldRole::Anchor;
        }
        (self.field_role)(tool, field)
    }
    pub fn capability(&self, tool: &str) -> Capability {
        (self.capability)(tool)
    }
}

fn claude_field_role(tool: &str, field: &str) -> FieldRole {
    match (tool, field) {
        ("Write", "content")
        | ("Edit", "new_string")
        | ("MultiEdit", "new_string")
        | ("NotebookEdit", "new_source") => FieldRole::WrittenBody,
        ("Bash", "command") => FieldRole::Command,
        _ => FieldRole::Opaque,
    }
}

pub(crate) fn tool_capability(tool: &str) -> Capability {
    match tool {
        "Read" | "NotebookRead" => Capability::ReadFile,
        "Bash" | "WebFetch" | "TaskOutput" | "ExitPlanMode" | "Agent" | "Task" => Capability::Bulk,
        _ => Capability::Unknown,
    }
}

fn deny_field_role(_tool: &str, _field: &str) -> FieldRole {
    FieldRole::Opaque
}

fn deny_capability(_tool: &str) -> Capability {
    Capability::Unknown
}

static CLAUDE_CODE: AgentProfile = AgentProfile {
    name: "claude-code",
    field_role: claude_field_role,
    capability: tool_capability,
};

static CONSERVATIVE: AgentProfile = AgentProfile {
    name: "conservative",
    field_role: deny_field_role,
    capability: deny_capability,
};

/// Profile for a detected client (see `crate::extract::detect_client`).
/// Anything not explicitly profiled gets [`CONSERVATIVE`].
pub fn profile_for(client: Option<&str>) -> &'static AgentProfile {
    match client {
        Some("claude-code") => &CLAUDE_CODE,
        _ => &CONSERVATIVE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_agent_gets_full_deny() {
        for c in [None, Some("codex"), Some("cursor"), Some("made-up")] {
            let p = profile_for(c);
            assert_eq!(p.name, "conservative");
            assert_eq!(p.field_role("Write", "content"), FieldRole::Opaque);
            assert_eq!(p.capability("Read"), Capability::Unknown);
        }
    }

    #[test]
    fn anchors_are_denied_in_every_profile() {
        for p in [profile_for(Some("claude-code")), profile_for(None)] {
            assert_eq!(p.field_role("Edit", "old_string"), FieldRole::Anchor);
            assert_eq!(p.field_role("FutureTool", "old_source"), FieldRole::Anchor);
        }
    }

    #[test]
    fn claude_code_profile_matches_shipped_behaviour() {
        let p = profile_for(Some("claude-code"));
        assert_eq!(p.field_role("Write", "content"), FieldRole::WrittenBody);
        assert_eq!(p.field_role("Edit", "new_string"), FieldRole::WrittenBody);
        assert_eq!(p.field_role("Bash", "command"), FieldRole::Command);
        assert_eq!(p.field_role("Write", "file_path"), FieldRole::Opaque);
        assert_eq!(p.capability("Read"), Capability::ReadFile);
        assert_eq!(p.capability("Bash"), Capability::Bulk);
        assert_eq!(p.capability("SomeMcpTool"), Capability::Unknown);
    }
}
