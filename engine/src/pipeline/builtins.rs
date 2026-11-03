use super::schema::{self, PipelineFile};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const RTK_DEFAULT: &str = "rtk:default";
pub const RTK_POSTHOOK: &str = "rtk:posthook";
pub const RTK_PROXY: &str = "rtk:proxy";
pub const RTK_DELEGATE_READS: &str = "rtk:delegate-reads";
pub const RTK_VERBATIM_DIFFS: &str = "rtk:verbatim-diffs";
pub const RTK_TERSE: &str = "rtk:terse";

pub const BUILTIN_NAMES: &[&str] = &[
    RTK_DEFAULT,
    RTK_POSTHOOK,
    RTK_PROXY,
    RTK_DELEGATE_READS,
    RTK_VERBATIM_DIFFS,
    RTK_TERSE,
];

const LITERALS: &[(&str, &str)] = &[
    (RTK_DEFAULT, include_str!("../../builtin/default.toml")),
    (RTK_POSTHOOK, include_str!("../../builtin/posthook.toml")),
    (RTK_PROXY, include_str!("../../builtin/proxy.toml")),
    (
        RTK_DELEGATE_READS,
        include_str!("../../builtin/delegate-reads.toml"),
    ),
    (
        RTK_VERBATIM_DIFFS,
        include_str!("../../builtin/verbatim-diffs.toml"),
    ),
    (RTK_TERSE, include_str!("../../builtin/terse.toml")),
];

pub fn builtin_text(name: &str) -> Option<&'static str> {
    LITERALS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

fn parsed() -> &'static BTreeMap<&'static str, PipelineFile> {
    static PARSED: OnceLock<BTreeMap<&'static str, PipelineFile>> = OnceLock::new();
    PARSED.get_or_init(|| {
        LITERALS
            .iter()
            .map(|(name, text)| {
                let result = schema::parse(text, name);
                let file = result.file.unwrap_or_default();
                (*name, file)
            })
            .collect()
    })
}

pub fn builtin(name: &str) -> Option<&'static PipelineFile> {
    parsed().get(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Event, Subject};
    use crate::pipeline::position::Position;

    #[test]
    fn every_builtin_parses_without_diagnostics() {
        for (name, text) in LITERALS {
            let result = schema::parse(text, name);
            assert!(
                result.diagnostics.is_empty(),
                "{name} has diagnostics: {:#?}",
                result.diagnostics
            );
            assert_eq!(result.file.expect("parses").name.as_deref(), Some(*name));
        }
    }

    #[test]
    fn default_is_todays_oss_and_nothing_more() {
        let default = builtin(RTK_DEFAULT).expect("compiled in");
        assert!(default.extends.is_empty());
        let mut positions: Vec<String> = default.rules.keys().map(|p| p.render()).collect();
        positions.sort();
        // Category A ships in the default pipeline: it is the only proxy-side
        // removal that never costs more than it saves, and it is inert unless
        // a daemon runs.
        assert_eq!(
            positions,
            vec!["hook.before_tool.shell", "proxy.request.tool_result"]
        );
        let norm = &default.rules[&Position::proxy(Subject::ToolResult)][0];
        assert_eq!(norm.name.as_deref(), Some("normalize"));
        assert_eq!(norm.use_, vec!["ansi", "progress", "dedup"]);
        let rewrite = &default.rules[&Position::hook(Event::BeforeTool, Subject::Shell)][0];
        assert_eq!(rewrite.name.as_deref(), Some("rtk-rewrite"));
        assert_eq!(rewrite.action.as_deref(), Some("rewrite"));
        assert_eq!(default.directives.len(), 1);
        assert_eq!(default.directives[0].use_.as_deref(), Some("rtk-awareness"));
        assert_eq!(default.guards.min_size, Some(512));
    }

    #[test]
    fn posthook_carries_the_branch_chains() {
        let posthook = builtin(RTK_POSTHOOK).expect("compiled in");
        assert_eq!(posthook.extends, vec![RTK_DEFAULT]);
        let shell = &posthook.rules[&Position::hook(Event::AfterTool, Subject::Shell)];
        assert_eq!(shell[0].name.as_deref(), Some("term-default"));
        assert_eq!(shell[0].use_, vec!["term"]);
        let read = &posthook.rules[&Position::hook(Event::AfterTool, Subject::Read)];
        let names: Vec<&str> = read.iter().filter_map(|r| r.name.as_deref()).collect();
        assert_eq!(names, vec!["lockfiles", "json-reads", "strip-line-numbers"]);
        assert!(!read[1].enabled, "json-reads ships off");
        assert!(!read[2].enabled, "strip-line-numbers ships off");
    }

    #[test]
    fn proxy_ships_only_cache_stable_methods() {
        let proxy = builtin(RTK_PROXY).expect("compiled in");
        // `normalize` is inherited from rtk:default, not redeclared here.
        let results = proxy
            .rules
            .get(&Position::proxy(Subject::ToolResult))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let cache = &proxy.rules[&Position::proxy(Subject::Cache)];
        assert_eq!(cache[0].use_, vec!["cache-align"]);
        // Both cost more than they save against a warm prompt cache, so the
        // documented on-ramp must not turn either of them on. Opting in stays possible
        // by naming the method in your own pipeline file.
        for rule in results {
            assert!(
                !rule.use_.iter().any(|m| m == "ccr"),
                "ccr must not ship on"
            );
        }
        assert!(
            proxy
                .rules
                .get(&Position::proxy(Subject::ToolUse))
                .is_none_or(|rules| rules
                    .iter()
                    .all(|r| !r.use_.iter().any(|m| m == "retire-pointer"))),
            "retire-pointer must not ship on"
        );
    }

    #[test]
    fn no_builtin_names_an_agent_or_an_agents_tool() {
        let agent_words = [
            "claude",
            "cursor",
            "gemini",
            "copilot",
            "Bash",
            "Grep",
            "Glob",
            "WebFetch",
            "WebSearch",
            "NotebookRead",
            "MultiEdit",
        ];
        for (name, text) in LITERALS {
            for word in agent_words {
                assert!(
                    !text.contains(word),
                    "{name} names a harness or a harness tool: {word}"
                );
            }
        }
    }

    #[test]
    fn github_extends_both_reference_pipelines() {
        let github = builtin(RTK_VERBATIM_DIFFS).expect("compiled in");
        assert_eq!(github.extends, vec![RTK_POSTHOOK, RTK_PROXY]);
        assert!(github.overrides.contains_key("strip-line-numbers"));
    }

    #[test]
    fn delegate_reads_declares_tasks_without_upstreams() {
        let delegate = builtin(RTK_DELEGATE_READS).expect("compiled in");
        assert_eq!(delegate.tasks["bulk-read"].kind, "summarize");
        assert_eq!(delegate.tasks["code-write"].kind, "generate");
        assert!(delegate.tasks["bulk-read"].upstream.is_none());
        let read = &delegate.rules[&Position::hook(Event::BeforeTool, Subject::Read)];
        assert_eq!(read[0].action.as_deref(), Some("route"));
        assert_eq!(read[0].task.as_deref(), Some("bulk-read"));
    }
}
