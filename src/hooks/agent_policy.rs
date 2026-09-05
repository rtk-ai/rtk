//! Shared command-selection policy rendered into agent instructions and MCP hints.

pub const PLACEHOLDER: &str = "{{RTK_DIRECT_FIRST_POLICY}}";

pub const MARKDOWN: &str = include_str!("../../hooks/shared/direct-first-policy.md");

pub const MCP_INSTRUCTIONS: &str = "Prefer the typed run_filtered tool for commands supported by \
RTK. Pass RTK arguments without a leading `rtk`, for example \
{\"rtk_args\":[\"git\",\"status\"]} or {\"rtk_args\":[\"read\",\"src/main.rs\"]}. \
For a Windows CMD expression, use `rtk cmd` directly or the dedicated run_cmd tool with one raw expression, for example \
{\"expression\":\"echo %CD% & dir /b\"}. Use a host shell only when the task requires \
shell built-ins, a script, or control flow that cannot be expressed through an RTK route. \
For a Windows PowerShell expression, use `rtk powershell` for Desktop 5.1 or `rtk pwsh` for \
PowerShell 7+, or the dedicated run_powershell tool with a host and one raw expression. \
On Windows, raw PowerShell/pwsh and cmd.exe are last-resort fallbacks for exact-output behavior; never wrap an RTK-supported \
command inside them. Execution tools return compact model-facing results by default; use \
`response_mode: \"legacy\"` only when a client needs the historical diagnostic envelope.";

pub const RUN_FILTERED_DESCRIPTION: &str = "Preferred execution tool for RTK-supported commands. \
Pass arguments without a leading `rtk` (for example [\"git\",\"status\"], [\"read\",\"file\"], or \
[\"rg\",\"TODO\",\"src\"]). Returns bounded filtered output. For a Windows CMD expression, \
use the dedicated run_cmd tool. Do not wrap supported commands in PowerShell/pwsh or cmd; use a \
host shell only for shell-only behavior.";

pub const RUN_CMD_DESCRIPTION: &str = "Execute one raw Windows CMD expression through `rtk cmd`, \
preserving CMD operators, expansion, state, and exit-code semantics while applying only safe \
terminal-facing filters. This tool is Windows-only. Use one expression string such as `echo %CD% & \
dir /b`; use raw `cmd.exe` (or `rtk proxy cmd.exe`) when exact output, interactive input, \
redirection, machine consumption, batch files, or opaque control syntax must remain native.";

pub const RUN_POWERSHELL_DESCRIPTION: &str =
    "Execute one raw Windows PowerShell expression through \
`rtk powershell` (Desktop 5.1) or `rtk pwsh` (PowerShell 7+). The route preserves native \
PowerShell parsing and streams, filtering only confident terminal-facing success output. Use the \
native host for interactive input, -File, -EncodedCommand, -NoExit, XML/machine output, \
redirection, remoting, jobs, or exact output.";

pub fn render(template: &str) -> String {
    debug_assert_eq!(
        template.matches(PLACEHOLDER).count(),
        1,
        "agent template must contain exactly one direct-first policy placeholder"
    );
    template.replace(PLACEHOLDER, MARKDOWN.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_the_single_policy_placeholder() {
        let rendered = render(&format!("before\n{PLACEHOLDER}\nafter"));
        assert!(rendered.starts_with("before\n## Command Selection Priority"));
        assert!(rendered.ends_with("\nafter"));
        assert!(!rendered.contains(PLACEHOLDER));
    }

    #[test]
    fn windows_cmd_policy_prefers_optimized_route_before_native_shell() {
        assert!(MCP_INSTRUCTIONS.contains("run_cmd"));
        assert!(MCP_INSTRUCTIONS.contains("rtk cmd"));
        assert!(RUN_CMD_DESCRIPTION.contains("Windows-only"));
        assert!(MARKDOWN.contains("rtk cmd"));
        assert!(MARKDOWN.contains("cmd.exe"));
    }

    #[test]
    fn powershell_policy_names_both_host_routes() {
        assert!(MCP_INSTRUCTIONS.contains("run_powershell"));
        assert!(RUN_POWERSHELL_DESCRIPTION.contains("powershell"));
        assert!(RUN_POWERSHELL_DESCRIPTION.contains("pwsh"));
        assert!(MARKDOWN.contains("rtk powershell"));
        assert!(MARKDOWN.contains("rtk pwsh"));
    }

    #[test]
    fn github_policy_uses_the_universal_internal_router() {
        assert!(MARKDOWN.contains("always use `rtk gh"));
        assert!(MARKDOWN.contains("never use `rtk proxy gh`"));
        assert!(MARKDOWN.contains("internally"));
    }
}
