use rtk_engine::block::{Event, Subject};
use rtk_engine::harness::{self, HarnessProfile, PlannedHook, Position, SkippedHook};
use rtk_engine::pipeline::{Resolved, SiteKind};

pub struct AgentPlan {
    pub profile: &'static HarnessProfile,
    pub planned: Vec<PlannedHook>,
    pub skipped: Vec<SkippedHook>,
}

pub(super) type Entry = (&'static str, Option<String>);

pub(super) fn hook_events_used(resolved: &Resolved) -> Vec<(Event, Vec<Subject>)> {
    resolved
        .events_used()
        .into_iter()
        .filter(|(site, _)| *site == SiteKind::Hook)
        .map(|(site, event)| (event, resolved.subjects_used(site, event)))
        .collect()
}

pub(super) fn plan_for(agent: &str, global: bool) -> Option<AgentPlan> {
    let profile = harness::profile(agent)?;
    let loaded = if global {
        crate::core::pipeline_config::load_user_scope()
    } else {
        crate::core::pipeline_config::load()
    };
    let routed = crate::hooks::session_start::routed_through_rtk(agent);
    Some(plan_with_routed(profile, &loaded.resolved, routed))
}

pub fn plan_with(profile: &'static HarnessProfile, resolved: &Resolved) -> AgentPlan {
    plan_with_routed(profile, resolved, false)
}

pub(super) fn plan_with_routed(
    profile: &'static HarnessProfile,
    resolved: &Resolved,
    routed_through_rtk: bool,
) -> AgentPlan {
    let used = hook_events_used(resolved);
    let keepalive = routed_through_rtk
        && profile.session_start
        && profile
            .wiring
            .as_ref()
            .is_some_and(|wiring| wiring.session_start.is_some());
    let wants_session_start =
        crate::core::pipeline_config::session_start_text(resolved).is_some() || keepalive;
    let (planned, skipped) = harness::plan_hooks(profile, &used, wants_session_start);
    AgentPlan {
        profile,
        planned,
        skipped,
    }
}

impl AgentPlan {
    pub fn tool_entries(&self) -> Vec<Entry> {
        self.planned
            .iter()
            .filter(|hook| matches!(hook.which, Position::Tool(_)))
            .map(|hook| (hook.event, hook.matcher.clone()))
            .collect()
    }

    pub fn session_start_event(&self) -> Option<&'static str> {
        self.planned
            .iter()
            .find(|hook| hook.which == Position::SessionStart)
            .map(|hook| hook.event)
    }

    pub fn entries(&self) -> Vec<Entry> {
        let mut entries = self.tool_entries();
        if let Some(event) = self.session_start_event() {
            entries.push((event, None));
        }
        entries
    }

    pub fn skipped_lines(&self) -> Vec<String> {
        self.skipped
            .iter()
            .map(|skip| {
                let position = match (skip.which, skip.subject) {
                    (Position::Tool(event), Some(subject)) => {
                        format!("hook.{}.{}", event.as_str(), subject.as_str())
                    }
                    (Position::Tool(event), None) => format!("hook.{}.*", event.as_str()),
                    (Position::SessionStart, _) => "the session_start directive".to_string(),
                };
                format!(
                    "pipeline: {position} does not run on {}: {}",
                    self.profile.name, skip.reason
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(text: &str) -> Resolved {
        crate::core::pipeline_config::load_candidate(text, "test.toml").resolved
    }

    #[test]
    fn the_default_pipeline_plans_the_shell_rewrite_alone_on_every_agent() {
        let resolved = resolved("");
        for id in ["codex", "cursor", "antigravity", "vibe"] {
            let plan = plan_with(harness::profile(id).unwrap(), &resolved);
            let entries = plan.tool_entries();
            assert_eq!(entries.len(), 1, "{id}: {entries:?}");
            assert!(plan.skipped.is_empty(), "{id}: {:?}", plan.skipped_lines());
        }
        let codex = plan_with(harness::profile("codex").unwrap(), &resolved);
        assert_eq!(
            codex.tool_entries(),
            vec![("PreToolUse", Some("^(?:Bash)$".to_string()))]
        );
        let vibe = plan_with(harness::profile("vibe").unwrap(), &resolved);
        assert_eq!(
            vibe.tool_entries(),
            vec![(
                "pre_tool",
                Some("re:^(?:bash|file_system\\.bash|powershell|git_bash)$".to_string())
            )]
        );
    }

    #[test]
    fn session_start_directives_register_the_event_where_one_exists() {
        let resolved = resolved(
            "[[directive]]\nname = \"greeting\"\ntext = \"hi\"\nchannel = \"session_start\"\n",
        );
        let event =
            |id: &str| plan_with(harness::profile(id).unwrap(), &resolved).session_start_event();
        assert_eq!(event("codex"), Some("SessionStart"));
        assert_eq!(event("cursor"), Some("sessionStart"));
        assert_eq!(event("antigravity"), Some("SessionStart"));
        assert_eq!(event("vibe"), None);
        let vibe = plan_with(harness::profile("vibe").unwrap(), &resolved);
        assert!(
            vibe.skipped_lines()
                .iter()
                .any(|l| l.contains("the session_start directive"))
        );
        let codex = plan_with(harness::profile("codex").unwrap(), &resolved);
        assert_eq!(codex.entries().last(), Some(&("SessionStart", None)));
        let plain = plan_with(
            harness::profile("codex").unwrap(),
            &super::tests::resolved(""),
        );
        assert_eq!(plain.session_start_event(), None);
    }

    #[test]
    fn a_proxied_agent_registers_session_start_for_the_keepalive_where_one_exists() {
        let resolved = resolved("");
        let codex = plan_with_routed(harness::profile("codex").unwrap(), &resolved, true);
        assert_eq!(codex.session_start_event(), Some("SessionStart"));
        assert!(codex.skipped.is_empty(), "{:?}", codex.skipped_lines());
        let vibe = plan_with_routed(harness::profile("vibe").unwrap(), &resolved, true);
        assert_eq!(vibe.session_start_event(), None);
        assert!(
            vibe.skipped.is_empty(),
            "the keep-alive is not a directive and must not be reported as unserved: {:?}",
            vibe.skipped_lines()
        );
        let unrouted = plan_with_routed(harness::profile("codex").unwrap(), &resolved, false);
        assert_eq!(unrouted.session_start_event(), None);
    }

    #[test]
    fn posthook_registers_after_tool_only_where_rtk_serves_it() {
        let resolved = resolved("[pipeline]\nextends = \"rtk:posthook\"\n");
        let vibe = plan_with(harness::profile("vibe").unwrap(), &resolved);
        assert!(vibe.tool_entries().iter().any(|(e, _)| *e == "post_tool"));

        let codex = plan_with(harness::profile("codex").unwrap(), &resolved);
        assert!(codex.tool_entries().iter().all(|(e, _)| *e == "PreToolUse"));
        let lines = codex.skipped_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("hook.after_tool.*") && l.contains("Codex")),
            "{lines:?}"
        );
    }

    #[test]
    fn an_edit_rule_widens_the_matcher_in_each_agent_spelling() {
        let resolved = resolved(
            "[[hook.before_tool.edit]]\nname = \"no-lockfile-edits\"\nwhen = { basename = \"Cargo.lock\" }\naction = \"deny\"\n",
        );
        let matcher = |id: &str| {
            plan_with(harness::profile(id).unwrap(), &resolved)
                .tool_entries()
                .remove(0)
                .1
                .unwrap()
        };
        assert_eq!(matcher("codex"), "^(?:Bash|apply_patch|Edit)$");
        assert_eq!(
            matcher("antigravity"),
            "replace_file_content|multi_replace_file_content|run_command"
        );
        assert_eq!(
            matcher("vibe"),
            "re:^(?:edit|bash|file_system\\.bash|powershell|git_bash)$"
        );
        let cursor = plan_with(harness::profile("cursor").unwrap(), &resolved);
        assert_eq!(cursor.tool_entries()[0].1.as_deref(), Some("Shell"));
        assert!(
            cursor
                .skipped_lines()
                .iter()
                .any(|l| l.contains("hook.before_tool.edit"))
        );
    }
}
