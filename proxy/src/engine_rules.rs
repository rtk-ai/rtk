//! Boot-time `proxy.request.*` rule resolution — the proxy-site adapter.
//!
//! The daemon reads the same user-scope pipeline files as the CLI, resolves
//! them through `rtk-engine`, and derives which transforms run, and their
//! floors, from the resolved `proxy.request.*` rules — wherever they were
//! declared. `rtk:default` carries `normalize`; `rtk:proxy` adds `align`; `ccr`
//! and `retire-pointer` ship in neither and activate only if a pipeline names
//! them. A daemon whose pipeline resolves to none of them forwards unchanged.
//!
//! Read once at boot, like every floor: a mid-session change would re-render
//! the history and invalidate the upstream prefix. `RTK_PROXY_CCR_*`
//! environment variables still override the rule floors.
//!
//! Project-scope files (`.rtk/pipeline.toml`) are not read here: they are
//! trust-gated per project directory and the daemon serves every project
//! from one process. Shadow-marked proxy rules count as inactive: the proxy
//! does not record shadow decisions.

use rtk_engine::block::Subject;
use rtk_engine::pipeline::{Position, Resolved, Scope, SourceFile, has_errors, parse, resolve};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct EngineRules {
    pub any_files: bool,
    pub pipeline: String,
    pub ccr: bool,
    pub align: bool,
    pub retire: bool,
    pub normalize: bool,
    pub methods: bool,
    pub min_size: Option<usize>,
    pub bulk_floor: Option<usize>,
    pub other_floor: Option<usize>,
    pub retire_floor: Option<usize>,
}

fn user_paths() -> Vec<PathBuf> {
    if let Ok(explicit) = std::env::var("RTK_PROXY_PIPELINE") {
        let path = PathBuf::from(explicit);
        return if path.exists() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let Some(base) = dirs::config_dir().map(|d| d.join("rtk")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let single = base.join("pipeline.toml");
    if single.exists() {
        out.push(single);
    }
    if let Ok(entries) = std::fs::read_dir(base.join("pipelines")) {
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

fn floor_of(resolved: &Resolved, rule_name: &str) -> Option<usize> {
    resolved
        .find_rule(rule_name)
        .and_then(|r| r.when_value("tokens_gt"))
        .and_then(|v| v.as_integer())
        .and_then(|n| usize::try_from(n).ok())
}

fn rules_use(resolved: &Resolved, part: Subject, method: &str) -> bool {
    resolved.rules_at(Position::proxy(part)).iter().any(|r| {
        r.def.enabled && !r.def.pro_inert && !r.def.shadow && r.def.use_.iter().any(|m| m == method)
    })
}

fn runs_engine_methods(resolved: &Resolved) -> bool {
    [Subject::ToolResult, Subject::Any].iter().any(|part| {
        resolved.rules_at(Position::proxy(*part)).iter().any(|r| {
            r.def.enabled
                && !r.def.pro_inert
                && !r.def.shadow
                && rtk_engine::pipeline::resolve_chain(resolved, &r.def.use_)
                    .0
                    .iter()
                    .any(|m| crate::pipeline::engine_methods::runs_in_proxy(*m))
        })
    })
}

pub fn from_resolved(resolved: &Resolved, any_files: bool) -> EngineRules {
    EngineRules {
        any_files,
        pipeline: resolved.name.clone(),
        ccr: rules_use(resolved, Subject::ToolResult, "ccr"),
        align: rules_use(resolved, Subject::Cache, "cache-align"),
        retire: rules_use(resolved, Subject::ToolUse, "retire-pointer"),
        // Category A is one daemon stage but three engine methods; naming any
        // of them activates it, and `normalize` itself decides per transform.
        normalize: ["ansi", "progress", "dedup", "strip-line-numbers"]
            .iter()
            .any(|m| rules_use(resolved, Subject::ToolResult, m)),
        methods: runs_engine_methods(resolved),
        min_size: (resolved.guards.min_size != rtk_engine::pipeline::Guards::default().min_size)
            .then_some(resolved.guards.min_size),
        bulk_floor: floor_of(resolved, "ccr-bulk"),
        other_floor: floor_of(resolved, "ccr"),
        retire_floor: floor_of(resolved, "retire-writes"),
    }
}

pub fn load() -> EngineRules {
    let (resolved, any_files) = load_resolved();
    let rules = from_resolved(&resolved, any_files);
    tracing::info!(
        pipeline = %resolved.name,
        ccr = rules.ccr,
        align = rules.align,
        retire = rules.retire,
        normalize = rules.normalize,
        methods = rules.methods,
        "proxy.request rules"
    );
    rules
}

pub fn load_resolved() -> (Resolved, bool) {
    let paths = user_paths();
    let any_files = !paths.is_empty();
    let mut sources = Vec::new();
    let mut diagnostics = Vec::new();
    for path in &paths {
        let label = path.display().to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let parsed = parse(&text, &label);
        diagnostics.extend(parsed.diagnostics);
        if let Some(file) = parsed.file {
            sources.push(SourceFile {
                scope: Scope::User,
                label,
                file,
            });
        }
    }
    let (resolved, resolve_diagnostics) = resolve(&sources);
    diagnostics.extend(resolve_diagnostics);
    if has_errors(&diagnostics) {
        tracing::warn!(
            "pipeline files have errors — the daemon runs rtk:default (no proxy rules); \
             run `rtk pipeline validate`"
        );
        let (fallback, _) = resolve(&[]);
        return (fallback, any_files);
    }
    (resolved, any_files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_for(text: &str) -> Resolved {
        let parsed = parse(text, "u.toml");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let (resolved, diagnostics) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "u.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        resolved
    }

    #[test]
    fn guard_min_size_reaches_the_daemon_only_when_set() {
        let (default, _) = resolve(&[]);
        assert_eq!(
            from_resolved(&default, false).min_size,
            None,
            "upgrade keeps the 400-char floor"
        );
        let set = resolved_for("[guards]\nmin_size = 2048\n");
        assert_eq!(from_resolved(&set, true).min_size, Some(2048));
    }

    #[test]
    fn engine_methods_stay_off_for_shipped_pipelines() {
        let (resolved, _) = resolve(&[]);
        assert!(!from_resolved(&resolved, false).methods);
        let proxy = resolved_for("[pipeline]\nextends = \"rtk:proxy\"\n");
        assert!(
            !from_resolved(&proxy, true).methods,
            "upgrading changes nothing"
        );
    }

    #[test]
    fn a_runnable_proxy_rule_turns_engine_methods_on() {
        let on = resolved_for("[[proxy.request.tool_result]]\nname = \"md\"\nuse = [\"web-md\"]\n");
        assert!(from_resolved(&on, true).methods);
        let unsafe_only =
            resolved_for("[[proxy.request.tool_result]]\nname = \"c\"\nuse = [\"cap\"]\n");
        assert!(
            !from_resolved(&unsafe_only, true).methods,
            "nothing would run"
        );
        let shadow = resolved_for(
            "[[proxy.request.tool_result]]\nname = \"md\"\nuse = [\"web-md\"]\nshadow = true\n",
        );
        assert!(!from_resolved(&shadow, true).methods);
    }

    #[test]
    fn rtk_default_activates_category_a_and_nothing_else() {
        let (resolved, _) = resolve(&[]);
        let rules = from_resolved(&resolved, false);
        // Category A ships in rtk:default: lossless, cache-stable, no
        // recovery path. A daemon you started should do that much.
        assert!(rules.normalize);
        // Everything that rewrites history stays opt-in.
        assert!(!rules.ccr && !rules.align && !rules.retire);
    }

    #[test]
    fn rtk_proxy_activates_only_cache_stable_methods() {
        let resolved = resolved_for("[pipeline]\nextends = \"rtk:proxy\"\n");
        let rules = from_resolved(&resolved, true);
        assert!(rules.normalize, "category A is the point of the on-ramp");
        assert!(rules.align);
        // Both cost more than they save against a warm prompt cache, so the
        // documented on-ramp must not switch either on.
        assert!(!rules.ccr, "ccr must not ship on");
        assert!(!rules.retire, "retire-pointer must not ship on");
        assert_eq!(rules.other_floor, None);
        assert_eq!(rules.retire_floor, None);
    }

    #[test]
    fn resolved_pipeline_name_reaches_the_usage_stamp() {
        let (resolved, _) = resolve(&[]);
        assert_eq!(from_resolved(&resolved, false).pipeline, "rtk:default");
        let named = resolved_for("[pipeline]\nname = \"my-pipe\"\nextends = \"rtk:proxy\"\n");
        assert_eq!(from_resolved(&named, true).pipeline, "my-pipe");
    }

    #[test]
    fn ccr_still_activates_when_a_user_asks_for_it_by_name() {
        let resolved = resolved_for(
            "[pipeline]\nextends = \"rtk:proxy\"\n\
             [[proxy.request.tool_result]]\nname = \"ccr\"\n\
             when = { tool_not_in = [\"read\"], tokens_gt = 1500 }\nuse = [\"ccr\"]\n",
        );
        let rules = from_resolved(&resolved, true);
        assert!(rules.ccr, "opting in by naming the method still works");
        assert_eq!(rules.other_floor, Some(1500));
    }

    #[test]
    fn override_raises_the_ccr_floor_for_a_user_who_opted_in() {
        let resolved = resolved_for(
            "[pipeline]\nextends = \"rtk:proxy\"\n\
             [[proxy.request.tool_result]]\nname = \"ccr\"\n\
             when = { tool_not_in = [\"read\"], tokens_gt = 1500 }\nuse = [\"ccr\"]\n\
             [override.ccr]\nwhen.tokens_gt = 500\n",
        );
        let rules = from_resolved(&resolved, true);
        assert_eq!(rules.other_floor, Some(500));
    }

    #[test]
    fn disabling_align_by_override_turns_it_off() {
        let resolved = resolved_for(
            "[pipeline]\nextends = \"rtk:proxy\"\n[override.align]\nenabled = false\n",
        );
        let rules = from_resolved(&resolved, true);
        assert!(rules.normalize && !rules.align);
    }

    #[test]
    fn shadow_proxy_rules_do_not_activate() {
        let resolved = resolved_for(
            "[pipeline]\nextends = \"rtk:proxy\"\n[override.normalize]\nshadow = true\n",
        );
        let rules = from_resolved(&resolved, true);
        assert!(!rules.normalize, "a shadow rule is recorded, never emitted");
    }
}
