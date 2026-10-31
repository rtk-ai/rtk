use crate::error::AppResult;
use crate::model::{InterceptedRequest, Provider};
use crate::pipeline::Transform;
use rtk_engine::block::Subject;
use rtk_engine::chain::{run_chain_caught, run_chain_guarded};
use rtk_engine::method::{Method, MethodCtx};
use rtk_engine::pipeline::{DecisionKind, Fact, Facts, Position, Resolved, evaluate};
use rtk_engine::tokens::estimate_tokens;
use rtk_engine::{Block, ContentFormat, Origin};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

pub const STAGE: &str = "engine-methods";

const MEMO_CAPACITY: usize = 4096;

pub fn runs_in_proxy(method: &dyn Method) -> bool {
    rtk_engine::registry::safe_without_recall(method)
        && !rtk_engine::registry::PROXY_NORMALIZE_METHODS.contains(&method.name())
}

pub struct EngineMethodsTransform {
    resolved: Resolved,
    needs_shape: bool,
    memo: Mutex<HashMap<(u64, u64), Option<String>>>,
}

impl EngineMethodsTransform {
    pub fn new(resolved: Resolved) -> Self {
        let needs_shape = [Subject::ToolResult, Subject::Any].iter().any(|part| {
            resolved.rules_at(Position::proxy(*part)).iter().any(|r| {
                std::iter::once(&r.def.when)
                    .chain(r.def.any.iter())
                    .flat_map(|group| group.keys())
                    .any(|key| key.starts_with("shape"))
            })
        });
        Self {
            resolved,
            needs_shape,
            memo: Mutex::new(HashMap::new()),
        }
    }

    fn chain_for(&self, facts: &Facts) -> Option<(String, Vec<&'static dyn Method>)> {
        let decisions = evaluate(&self.resolved, Position::proxy(Subject::ToolResult), facts);
        let mut methods: Vec<&'static dyn Method> = Vec::new();
        let mut key = String::new();
        for decision in &decisions {
            if decision.shadow() {
                continue;
            }
            match &decision.kind {
                DecisionKind::Transform {
                    methods: selected, ..
                } => {
                    for method in selected.iter().copied().filter(|m| runs_in_proxy(*m)) {
                        key.push_str(method.name());
                        key.push(',');
                        methods.push(method);
                    }
                }
                DecisionKind::Act(_) => return None,
            }
        }
        (!methods.is_empty()).then_some((key, methods))
    }

    fn transform(
        &self,
        tool: Subject,
        tool_name: &str,
        text: &str,
        succeeded: bool,
        req: &RequestMeta,
    ) -> Option<String> {
        if text.len() < self.resolved.guards.min_size
            || text.contains(crate::pipeline::TRUNCATION_MARKER_TAG)
        {
            return None;
        }
        let format = format_of(tool, text);
        let mut facts = facts_for(tool, format, text, succeeded, req);
        if self.needs_shape {
            let features = rtk_engine::shape::extract(
                &rtk_engine::shape::normalize(text),
                text.chars().count(),
            );
            let (shape, _) = rtk_engine::shape::classify(text, &features);
            facts.insert("shape".into(), Fact::Str(shape.as_str().to_string()));
        }
        let (key, methods) = self.chain_for(&facts)?;
        let id = memo_key(&key, text);
        if let Some(hit) = self.memo.lock().ok().and_then(|m| m.get(&id).cloned()) {
            return hit;
        }
        let origin = Origin {
            tool: Some(tool_name.to_string()),
            ..Origin::default()
        };
        let provenance = rtk_engine::marker::provenance_of(text);
        let block = Block {
            text,
            subject: tool,
            format,
            origin: &origin,
            provenance: &provenance,
        };
        let ctx = MethodCtx::new("");
        let out = if self.resolved.guards.never_worse {
            run_chain_guarded(&methods, &block, &ctx)
        } else {
            run_chain_caught(&methods, &block, &ctx).filter(|o| o != text)
        };
        if let Ok(mut memo) = self.memo.lock() {
            if memo.len() >= MEMO_CAPACITY {
                memo.clear();
            }
            memo.insert(id, out.clone());
        }
        out
    }
}

fn memo_key(chain: &str, text: &str) -> (u64, u64) {
    let mut a = DefaultHasher::new();
    chain.hash(&mut a);
    text.hash(&mut a);
    let mut b = DefaultHasher::new();
    text.len().hash(&mut b);
    text.hash(&mut b);
    chain.hash(&mut b);
    (a.finish(), b.finish())
}

fn format_of(tool: Subject, text: &str) -> ContentFormat {
    let head = text.trim_start();
    if (head.starts_with('{') || head.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(text).is_ok()
    {
        return ContentFormat::Json;
    }
    let lower = head.get(..15).unwrap_or(head).to_ascii_lowercase();
    if tool == Subject::Web || lower.starts_with("<!doctype") || lower.starts_with("<html") {
        return ContentFormat::Web;
    }
    if tool == Subject::Search {
        return ContentFormat::Matches;
    }
    ContentFormat::Term
}

struct RequestMeta {
    model: String,
    project: Option<String>,
    session: Option<String>,
}

impl RequestMeta {
    fn of(req: &InterceptedRequest) -> Self {
        Self {
            model: req.model.clone(),
            project: req.project.clone(),
            session: req.session.clone(),
        }
    }
}

fn facts_for(
    tool: Subject,
    format: ContentFormat,
    text: &str,
    succeeded: bool,
    req: &RequestMeta,
) -> Facts {
    let mut facts = Facts::new();
    facts.insert("tool".into(), Fact::Str(tool.as_str().to_string()));
    facts.insert("format".into(), Fact::Str(format.as_str().to_string()));
    facts.insert("tokens".into(), Fact::Num(estimate_tokens(text) as f64));
    facts.insert("succeeded".into(), Fact::Bool(succeeded));
    facts.insert("model".into(), Fact::Str(req.model.clone()));
    facts.insert("provider".into(), Fact::Str("anthropic".into()));
    if let Some(project) = &req.project {
        facts.insert("project".into(), Fact::Str(project.clone()));
    }
    if let Some(session) = &req.session {
        facts.insert("session".into(), Fact::Str(session.clone()));
    }
    facts
}

fn tool_names(body: &serde_json::Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) else {
        return map;
    };
    for block in msgs
        .iter()
        .filter_map(|m| m.get("content").and_then(|c| c.as_array()))
        .flatten()
    {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
            continue;
        }
        if let (Some(id), Some(name)) = (
            block.get("id").and_then(|v| v.as_str()),
            block.get("name").and_then(|v| v.as_str()),
        ) {
            map.insert(id.to_string(), name.to_string());
        }
    }
    map
}

fn subject_of(tool_name: &str) -> Subject {
    rtk_engine::harness::profile("claude-code")
        .map(|p| p.map_tool(tool_name))
        .unwrap_or(Subject::Any)
}

#[async_trait::async_trait]
impl Transform for EngineMethodsTransform {
    fn name(&self) -> &'static str {
        STAGE
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        if !matches!(req.provider, Provider::Anthropic) {
            return Ok(());
        }
        let names = tool_names(&req.body);
        let snapshot = RequestMeta::of(req);
        let mut before = 0usize;
        let mut after = 0usize;
        let Some(msgs) = req.body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
            return Ok(());
        };
        for content in msgs
            .iter_mut()
            .filter_map(|m| m.get_mut("content").and_then(|c| c.as_array_mut()))
        {
            for block in content.iter_mut() {
                if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                    continue;
                }
                let tool_name = block
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .and_then(|id| names.get(id))
                    .cloned()
                    .unwrap_or_default();
                let tool = subject_of(&tool_name);
                if tool == Subject::Read {
                    continue;
                }
                let succeeded = !block
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut rewrite = |text: &mut String| {
                    if let Some(new) = self.transform(tool, &tool_name, text, succeeded, &snapshot)
                    {
                        before += crate::tokenizer::count(text);
                        after += crate::tokenizer::count(&new);
                        *text = new;
                    }
                };
                match block.get_mut("content") {
                    Some(serde_json::Value::String(s)) => rewrite(s),
                    Some(serde_json::Value::Array(parts)) => {
                        for part in parts.iter_mut() {
                            if part.get("type").and_then(|t| t.as_str()) != Some("text") {
                                continue;
                            }
                            if let Some(serde_json::Value::String(s)) = part.get_mut("text") {
                                rewrite(s);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let saved = before.saturating_sub(after) as i64;
        if saved > 0 {
            req.record_saving(STAGE, saved);
            tracing::info!(
                tokens_saved = saved,
                "engine-methods: proxy.request rules applied"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtk_engine::pipeline::{Scope, SourceFile, parse, resolve};
    use serde_json::json;

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

    fn html_page() -> String {
        let mut body = String::from(
            "<!DOCTYPE html><html><head><title>Docs</title><style>body{margin:0}</style></head><body>",
        );
        for i in 0..12 {
            body.push_str(&format!(
                "<div class=\"section\"><h2>Section {i}</h2><p>Paragraph {i} explains the <a href=\"https://example.com/{i}\">reference {i}</a> in detail.</p></div>"
            ));
        }
        body.push_str("</body></html>");
        body
    }

    fn json_rows() -> String {
        let rows: Vec<_> = (0..40)
            .map(|i| json!({"id": i, "name": format!("item-{i}"), "status": "open"}))
            .collect();
        serde_json::to_string_pretty(&rows).expect("serializes")
    }

    fn request(tool: &str, output: &str) -> InterceptedRequest {
        let body = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": tool, "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": output}
                ]}
            ]
        });
        InterceptedRequest {
            provider: Provider::Anthropic,
            path: "/v1/messages".into(),
            model: "claude-sonnet-4-5".into(),
            stream: false,
            client: Some("claude-code".into()),
            project: None,
            session: Some("sess1".into()),
            api_key_hash: None,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            body,
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
        }
    }

    fn result_text(req: &InterceptedRequest) -> String {
        req.body["messages"][1]["content"][0]["content"]
            .as_str()
            .expect("string content")
            .to_string()
    }

    async fn run(pipeline: &str, tool: &str, output: &str) -> (String, i64) {
        let stage = EngineMethodsTransform::new(resolved_for(pipeline));
        let mut req = request(tool, output);
        stage.on_request(&mut req).await.expect("runs");
        (result_text(&req), req.saved_by(STAGE))
    }

    const WEB_MD: &str = "schema_version = 1\n[[proxy.request.tool_result]]\nname = \"web-to-md\"\nwhen = { format = \"web\" }\nuse = [\"web-md\"]\n";

    fn runnable(name: &str) -> bool {
        runs_in_proxy(rtk_engine::registry::find(name).expect("registered"))
    }

    #[test]
    fn the_gate_admits_only_methods_safe_without_a_recall_store() {
        for name in ["web-md", "md-slim", "base64-elide", "lockfile"] {
            assert!(runnable(name), "{name} needs no recovery and must run");
        }
        for name in [
            "cap",
            "toon",
            "minify-json",
            "rle",
            "grep-group",
            "truncate",
        ] {
            assert!(
                !runnable(name),
                "{name} relies on a recall copy the daemon cannot keep"
            );
        }
        for name in ["ansi", "progress", "dedup", "strip-line-numbers"] {
            assert!(!runnable(name), "{name} belongs to the normalize stage");
        }
    }

    #[tokio::test]
    async fn a_rule_naming_an_engine_method_runs_in_the_proxy() {
        let raw = html_page();
        let (out, saved) = run(WEB_MD, "WebFetch", &raw).await;
        assert_ne!(out, raw, "web-md must rewrite the HTML tool result");
        assert!(!out.contains("<div"), "{out}");
        assert!(
            out.contains("https://example.com/3"),
            "links survive: {out}"
        );
        assert!(saved > 0, "the saving is attributed to the stage");
    }

    #[tokio::test]
    async fn when_conditions_are_honoured() {
        let raw = "plain terminal output line that is long enough to matter\n".repeat(20);
        let (out, saved) = run(WEB_MD, "Bash", &raw).await;
        assert_eq!(
            out, raw,
            "a format = web rule must not touch terminal output"
        );
        assert_eq!(saved, 0);
    }

    #[tokio::test]
    async fn rule_order_is_first_match_wins() {
        let pipeline = "schema_version = 1\n[[proxy.request.tool_result]]\nname = \"keep-web\"\nwhen = { format = \"web\" }\naction = \"passthrough\"\n[[proxy.request.tool_result]]\nname = \"md\"\nuse = [\"web-md\"]\n";
        let raw = html_page();
        let (out, _) = run(pipeline, "WebFetch", &raw).await;
        assert_eq!(out, raw, "the earlier passthrough rule wins");
    }

    #[tokio::test]
    async fn read_results_are_never_rewritten() {
        let raw = html_page();
        let (control, _) = run(WEB_MD, "Bash", &raw).await;
        assert_ne!(
            control, raw,
            "the same HTML from a shell command is rewritten"
        );
        let (out, _) = run(WEB_MD, "Read", &raw).await;
        assert_eq!(out, raw, "a later Edit needs the bytes the agent read");
    }

    #[tokio::test]
    async fn methods_that_need_a_recall_store_are_refused() {
        let pipeline = "schema_version = 1\n[[proxy.request.tool_result]]\nname = \"json\"\nuse = [\"toon\"]\n";
        let raw = json_rows();
        let (out, _) = run(pipeline, "mcp__linear__list_issues", &raw).await;
        assert_eq!(
            out, raw,
            "toon could alter values with nothing to recover them from"
        );
    }

    #[tokio::test]
    async fn small_blocks_stay_under_the_min_size_guard() {
        let raw = "<!DOCTYPE html><html><body><p>tiny</p></body></html>";
        let (out, _) = run(WEB_MD, "WebFetch", raw).await;
        assert_eq!(out, raw);
    }

    #[tokio::test]
    async fn output_is_byte_identical_across_turns() {
        let stage = EngineMethodsTransform::new(resolved_for(WEB_MD));
        let raw = html_page();
        let mut first = request("WebFetch", &raw);
        let mut second = request("WebFetch", &raw);
        stage.on_request(&mut first).await.expect("runs");
        stage.on_request(&mut second).await.expect("runs");
        assert_ne!(result_text(&first), raw);
        assert_eq!(result_text(&first), result_text(&second));
        let fresh = EngineMethodsTransform::new(resolved_for(WEB_MD));
        let mut third = request("WebFetch", &raw);
        fresh.on_request(&mut third).await.expect("runs");
        assert_eq!(
            result_text(&first),
            result_text(&third),
            "memo never changes output"
        );
    }

    #[tokio::test]
    async fn shape_conditions_are_supplied() {
        let raw = html_page();
        let features =
            rtk_engine::shape::extract(&rtk_engine::shape::normalize(&raw), raw.chars().count());
        let shape = rtk_engine::shape::classify(&raw, &features).0.as_str();
        let matching = format!(
            "schema_version = 1\n[[proxy.request.tool_result]]\nname = \"s\"\nwhen = {{ shape = \"{shape}\" }}\nuse = [\"web-md\"]\n"
        );
        let (out, _) = run(&matching, "WebFetch", &raw).await;
        assert_ne!(out, raw, "a rule on the block's own shape ({shape}) fires");
        let other = if shape == "nested" {
            "repetitive"
        } else {
            "nested"
        };
        let missing = format!(
            "schema_version = 1\n[[proxy.request.tool_result]]\nname = \"s\"\nwhen = {{ shape = \"{other}\" }}\nuse = [\"web-md\"]\n"
        );
        let (out, _) = run(&missing, "WebFetch", &raw).await;
        assert_eq!(out, raw, "a rule on another shape does not");
    }

    #[tokio::test]
    async fn hook_truncated_blocks_are_skipped() {
        let raw = format!(
            "{}\n{}",
            html_page(),
            crate::pipeline::TRUNCATION_MARKER_TAG
        );
        let (out, _) = run(WEB_MD, "WebFetch", &raw).await;
        assert_eq!(out, raw);
    }
}
