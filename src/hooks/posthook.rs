//! PostToolUse output filtering for Claude Code — the `hook.after_tool`
//! adapter.
//!
//! The tool runs raw; Claude Code sends the event JSON on stdin; this module
//! maps the tool name to its agnostic subject, evaluates the resolved
//! pipeline's `hook.after_tool.<subject>` rules, runs the winning chain
//! through `rtk-engine`, and returns `hookSpecificOutput.updatedToolOutput`.
//! Everything that knows the words "claude", "Read", "tool_response" lives
//! here — engine methods stay agent-agnostic.
//!
//! Protocol invariants:
//! - **Fail-open everywhere**: any error emits nothing and exits 0; Claude
//!   Code keeps the original output. A broken pipeline file falls back to
//!   `rtk:default` (no after_tool rules → passthrough) and surfaces via
//!   pipeline-status.json, never in the session.
//! - The only stdout write is the final serialized `hookSpecificOutput`.
//! - Never emit an unchanged or larger output — emit nothing instead.
//! - One `Config::load()` + one pipeline load per invocation.

use super::constants::POST_TOOL_USE_KEY;
use crate::core::config::Config;
use crate::core::pipeline_config::{self, LoadedPipeline};
use crate::core::pipeline_log;
use crate::core::tee;
use crate::core::tracking::{Tracker, estimate_tokens};
use regex::Regex;
use rtk_engine::block::{Block, ContentFormat, Event, Origin, Provenance, Subject};
use rtk_engine::method::{Method, MethodCtx};
use rtk_engine::pipeline::{Action, DecisionKind, Fact, Facts, Position, Resolved, evaluate};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::sync::LazyLock;

/// Read→Edit fidelity guard, emitted as `additionalContext` whenever a Read
/// output was format-converted (content no longer textually identical to the
/// on-disk file).
const READ_CONVERSION_NOTE: &str = "RTK compressed this Read output (format conversion, \
     e.g. json->toon, lockfile summary). The on-disk file is unchanged; re-read with rtk \
     disabled or check the recall file before constructing Edit old_string values.";

fn apply_terminal(
    text: String,
    ctx: &MethodCtx,
    subject: Subject,
    format: ContentFormat,
) -> String {
    let Some(redact) = rtk_engine::registry::redact() else {
        return text;
    };
    let origin = Origin::default();
    let provenance = Provenance::default();
    let block = Block {
        text: &text,
        subject,
        format,
        origin: &origin,
        provenance: &provenance,
    };
    rtk_engine::chain::run_chain(&[redact], &block, ctx)
}

/// Entry point: never panics, never errors, emits at most one stdout line.
/// The decision log flushes only after that line — it never sits in front
/// of the response.
pub fn run(event: &Value) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process(event)));
    if let Ok(Some((output, loaded))) = outcome {
        let _ = writeln!(io::stdout(), "{output}");
        pipeline_config::write_status(&loaded);
        pipeline_log::flush();
    } else if let Ok(None) = outcome {
        pipeline_log::flush();
    }
}

/// Full pipeline behind the fail-open envelope. `None` = emit nothing.
fn process(event: &Value) -> Option<(String, LoadedPipeline)> {
    let (config, loaded) = load_context()?;
    let output = process_with_config(event, &config, &loaded.resolved)?;
    Some((output, loaded))
}

/// One config load and one pipeline load per invocation, behind the two
/// session-wide off switches.
fn load_context() -> Option<(Config, LoadedPipeline)> {
    if crate::core::user_env::var("RTK_DISABLED").as_deref() == Some("1") {
        return None;
    }
    if crate::core::user_env::var("RTK_POSTHOOK").as_deref() == Some("0") {
        return None;
    }

    Some((Config::load().unwrap_or_default(), pipeline_config::load()))
}

/// What a plain-text host (OpenCode's `tool.execute.after`, either API shape)
/// hands the after path.
///
/// `tool`, `args` and `output` are the minimum; `status`, `exit_code`,
/// `description` and `duration_ms` are the metadata the host already has and
/// the Claude path gets for free (its event carries `duration_ms` and its shell
/// response carries the exit code). Each is optional because older OpenCode
/// versions and non-shell tools do not carry all of them.
#[derive(Debug, Default, Clone)]
pub struct AgentToolResult {
    pub tool: String,
    pub args: Value,
    pub output: String,
    pub status: Option<String>,
    pub exit_code: Option<i64>,
    pub description: Option<String>,
    pub duration_ms: u64,
}

/// Entry point for a host that hands RTK plain tool text instead of Claude
/// Code's event shape — OpenCode's `tool.execute.after`.
///
/// Same fail-open envelope as [`run`], with the payload reduced to the tool
/// name, its arguments, and the text the tool produced. The only stdout write
/// is the replacement text, and only when it is strictly smaller in bytes than
/// the input; every early return leaves the host's original output in place.
pub fn run_agent_text(agent: &str, result: &AgentToolResult) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        process_agent_text(agent, result)
    }));
    match outcome {
        Ok(Some((output, loaded))) => {
            let _ = io::stdout().write_all(output.as_bytes());
            pipeline_config::write_status(&loaded);
            pipeline_log::flush();
        }
        Ok(None) | Err(_) => pipeline_log::flush(),
    }
}

/// Full pipeline behind the plain-text envelope. `None` = emit nothing.
fn process_agent_text(agent: &str, result: &AgentToolResult) -> Option<(String, LoadedPipeline)> {
    if result.output.is_empty() {
        return None;
    }
    let (config, loaded) = load_context()?;
    prime_call_context(result);
    let output = agent_text_with_config(agent, result, &config, &loaded.resolved)?;
    Some((output, loaded))
}

/// Feed the host's `description` to the call-context reader the Claude path
/// fills from its event. A pipeline whose rules read `intent` then sees the
/// sentence the model wrote for the call instead of nothing; `goal` and
/// `recent_calls` come from a transcript OpenCode does not hand over, so they
/// stay absent. `set_source` keeps the first call in the process, which is
/// this one: a hook process handles exactly one tool result.
fn prime_call_context(result: &AgentToolResult) {
    let mut event = json!({ "tool_input": {} });
    if let Some(description) = &result.description {
        event["tool_input"]["description"] = json!(description);
    }
    crate::core::call_context::set_source(&event);
}

/// A status the host reports for a call whose result is worth compressing.
///
/// OpenCode 2.x sends `completed` for a finished call; anything else (an error,
/// a cancellation) is passed through, mirroring PostToolUse, which only fires
/// on success. `None` covers 1.x, which sends no status at all.
fn is_completed(status: Option<&str>) -> bool {
    matches!(status, None | Some("") | Some("completed"))
}

/// Testable core: map tool → subject/format/source, then run the same
/// `hook.after_tool.<subject>` evaluation and guards as the Claude adapter.
fn agent_text_with_config(
    agent: &str,
    result: &AgentToolResult,
    config: &Config,
    pipeline: &Resolved,
) -> Option<String> {
    let tool_name = result.tool.as_str();
    let raw = result.output.as_str();
    if raw.is_empty() || !is_completed(result.status.as_deref()) {
        return None;
    }
    // Size floor first, in the same order as the Claude adapter: a result below
    // `guards.min_size` must not earn a `rtk gain` row on its way to being
    // dropped.
    if raw.len() < pipeline.guards.min_size {
        return None;
    }
    let subject = subject_of_for(agent, tool_name);
    let source = source_of_args(subject, &result.args);
    let Some(format) = text_format(tool_name, subject, &source, raw) else {
        // Unsupported shape for a subject whose output RTK would otherwise
        // rewrite (e.g. a `glob` result, which is not grep output): the text
        // passes through, but the zero-saving row keeps coverage visible in
        // `rtk gain`, exactly as the Claude adapter does for Grep count mode.
        // Write/Edit results carry no output to compress at all, so they are
        // not tracked.
        if !matches!(subject, Subject::Write | Subject::Edit) {
            track(tool_name, "raw", &source, raw, raw, result.duration_ms);
        }
        return None;
    };
    let composed = filter_and_compose(
        config,
        pipeline,
        subject,
        tool_name,
        &source,
        raw,
        format,
        result.duration_ms,
        result.exit_code,
        Gate::Bytes,
    )?;
    // `filter_and_compose`'s byte gate already refused anything the host would
    // not shrink, so this text is the answer.
    let output = composed;
    // The row `rtk gain` reads, labelled `rtk posthook <tool> <format>` — the
    // same label the Claude transport writes. Without it an OpenCode
    // compression lived only in the session.
    track(
        tool_name,
        format.as_str(),
        &source,
        raw,
        &output,
        result.duration_ms,
    );
    Some(output)
}

/// Path, URL or command the facts and tracking rows key on, taken from the
/// host's per-tool arguments. OpenCode spells the read path `filePath`; the
/// snake_case variants keep this usable for other plugin hosts.
fn source_of_args(subject: Subject, args: &Value) -> String {
    let keys: &[&str] = match subject {
        Subject::Shell => &["command"],
        Subject::Read => &["filePath", "file_path", "path"],
        Subject::Search => &["path", "filePath", "file_path"],
        Subject::Web => &["url"],
        _ => return String::new(),
    };
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// Content format for a host that provides plain text. Mirrors the Claude
/// adapter's mapping where the shapes correspond; `None` means the host's
/// output cannot be put back confidently and the text passes through.
fn text_format(
    tool_name: &str,
    subject: Subject,
    source: &str,
    raw: &str,
) -> Option<ContentFormat> {
    match subject {
        Subject::Read => Some(if is_lockfile_name(&basename_of(source)) {
            ContentFormat::Lockfile
        } else if extension_of(source) == "json" {
            ContentFormat::Json
        } else {
            ContentFormat::Term
        }),
        Subject::Shell | Subject::Subagent => Some(ContentFormat::Term),
        // Search covers both grep and file listings; only grep is `matches`.
        Subject::Search if tool_name.eq_ignore_ascii_case("grep") => Some(ContentFormat::Matches),
        Subject::Search => None,
        Subject::Web => Some(ContentFormat::Web),
        Subject::Mcp => Some(if serde_json::from_str::<Value>(raw.trim()).is_ok() {
            ContentFormat::Json
        } else {
            ContentFormat::Term
        }),
        Subject::Write | Subject::Edit => None,
        _ => Some(ContentFormat::Term),
    }
}

/// The agent whose PostToolUse payload this process is handling.
///
/// Only Claude Code dispatches a posthook today, but the tool-name vocabulary
/// is per-agent — `Read` on Claude Code, `view` on Copilot, `view_file` on
/// Antigravity all mean the same subject — so the lookup takes the agent rather
/// than assuming one. `rtk hook <agent>` sets it; the default keeps the
/// existing behaviour for any caller that does not.
fn subject_of(tool_name: &str) -> Subject {
    subject_of_for(
        super::hook_agent().as_deref().unwrap_or("claude-code"),
        tool_name,
    )
}

fn subject_of_for(agent: &str, tool_name: &str) -> Subject {
    rtk_engine::harness::profile(agent)
        .map(|p| p.map_tool(tool_name))
        .unwrap_or(Subject::Any)
}

/// Testable core: gates → extract → classify → evaluate rules → guard → re-wrap.
fn process_with_config(event: &Value, config: &Config, pipeline: &Resolved) -> Option<String> {
    let tool_name = event.get("tool_name")?.as_str()?;
    let subject = subject_of(tool_name);

    let source = source_of(event, tool_name);
    let duration_ms = event
        .get("duration_ms")
        .and_then(|d| d.as_u64())
        .unwrap_or(0);

    if tool_name == "WebSearch" {
        return process_websearch(event, config, pipeline, &source, duration_ms);
    }
    if subject == Subject::Shell {
        return process_shell(event, config, pipeline, duration_ms);
    }

    let (raw, format) = extract(event, tool_name, &source)?;
    if raw.len() < pipeline.guards.min_size {
        return None;
    }

    let Some(format) = format else {
        // Unsupported shape (e.g. Grep count mode): passthrough, but tracked
        // so coverage stays visible in `rtk gain`.
        track(tool_name, "raw", &source, &raw, &raw, duration_ms);
        return None;
    };

    let composed = filter_and_compose(
        config,
        pipeline,
        subject,
        tool_name,
        &source,
        &raw,
        format,
        duration_ms,
        None,
        Gate::Tokens,
    )?;

    let (updated, converted) = rebuild(event, tool_name, &composed)?;
    let mut hook_output = json!({
        "hookEventName": POST_TOOL_USE_KEY,
        "updatedToolOutput": updated,
    });
    if converted {
        hook_output["additionalContext"] = json!(READ_CONVERSION_NOTE);
    }

    track(
        tool_name,
        format.as_str(),
        &source,
        &raw,
        &composed,
        duration_ms,
    );
    Some(json!({ "hookSpecificOutput": hook_output }).to_string())
}

fn facts_for(
    subject: Subject,
    format: ContentFormat,
    source: &str,
    raw: &str,
    duration_ms: u64,
    is_rtk_command: Option<bool>,
    exit_code: Option<i64>,
) -> Facts {
    let mut facts = Facts::new();
    facts.insert("tool".into(), Fact::Str(subject.as_str().to_string()));
    facts.insert("format".into(), Fact::Str(format.as_str().to_string()));
    facts.insert("bytes".into(), Fact::Num(raw.len() as f64));
    facts.insert("lines".into(), Fact::Num(raw.lines().count() as f64));
    facts.insert("tokens".into(), Fact::Num(estimate_tokens(raw) as f64));
    facts.insert("duration_ms".into(), Fact::Num(duration_ms as f64));
    if let Some(value) = is_rtk_command {
        facts.insert("is_rtk_command".into(), Fact::Bool(value));
    }
    if let Some(code) = exit_code {
        facts.insert("exit_code".into(), Fact::Num(code as f64));
    }
    if !source.is_empty() {
        if source.starts_with("http://") || source.starts_with("https://") {
            facts.insert("url".into(), Fact::Str(source.to_string()));
        } else if subject == Subject::Shell {
            facts.insert("command".into(), Fact::Str(source.to_string()));
        } else {
            facts.insert("file_path".into(), Fact::Str(source.to_string()));
            facts.insert(
                "extension".into(),
                Fact::Str(extension_of(source).to_string()),
            );
            facts.insert("basename".into(), Fact::Str(basename_of(source)));
        }
    }
    facts
}

struct Applied {
    text: String,
    methods: Vec<&'static str>,
    rules: Vec<String>,
    pipeline: String,
    subject: Subject,
    shadow: bool,
}

fn log_entry_for(
    subject: Subject,
    command: Option<&str>,
    source: &str,
    applied: &Applied,
    before: usize,
    after: usize,
) -> pipeline_log::LogEntry {
    pipeline_log::LogEntry {
        ts: pipeline_log::now_ts(),
        session: None,
        site: "hook".to_string(),
        event: "after_tool".to_string(),
        subject: subject.as_str().to_string(),
        tool: Some(subject.as_str().to_string()),
        command: command
            .map(str::to_string)
            .or_else(|| (!source.is_empty()).then(|| source.to_string())),
        rule: applied.rules.join("+"),
        action: None,
        methods: applied.methods.iter().map(|m| m.to_string()).collect(),
        tokens_before: Some(before),
        tokens_after: Some(after),
        shadow: applied.shadow,
        recovery: None,
    }
}

fn queue_log(
    subject: Subject,
    command: Option<&str>,
    source: &str,
    applied: &Applied,
    before: usize,
    after: usize,
) {
    if !pipeline_log::enabled() {
        return;
    }
    pipeline_log::queue(log_entry_for(
        subject, command, source, applied, before, after,
    ));
}

#[allow(clippy::too_many_arguments)]
fn run_selected(
    pipeline: &Resolved,
    subject: Subject,
    tool_name: &str,
    format: ContentFormat,
    source: &str,
    raw: &str,
    duration_ms: u64,
    is_rtk_command: Option<bool>,
    exit_code: Option<i64>,
) -> Option<Applied> {
    let mut facts = facts_for(
        subject,
        format,
        source,
        raw,
        duration_ms,
        is_rtk_command,
        exit_code,
    );
    if subject == Subject::Mcp
        && let Some(rest) = tool_name.strip_prefix("mcp__")
    {
        let server = rest.split("__").next().unwrap_or(rest);
        if !server.is_empty() {
            facts.insert("server".into(), Fact::Str(server.to_string()));
        }
    }
    let position = Position::hook(Event::AfterTool, subject);
    crate::core::pipeline_config::insert_call_context(pipeline, position, &mut facts);
    let call = crate::core::pipeline_config::call_state(&facts, None);
    crate::core::classify::insert_classifier_facts(
        pipeline,
        position,
        raw,
        Some(&call),
        &mut facts,
    );
    shadow_eval_candidate(subject, format, source, raw, &facts);
    let decisions = evaluate(pipeline, position, &facts);
    if decisions.is_empty() {
        return None;
    }
    let mut methods: Vec<&'static dyn Method> = Vec::new();
    let mut method_names: Vec<&'static str> = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    let mut shadow = false;
    for decision in &decisions {
        match &decision.kind {
            DecisionKind::Transform {
                methods: selected, ..
            } => {
                methods.extend_from_slice(selected);
                method_names.extend(selected.iter().map(|m| m.name()));
                rules.push(decision.rule.name.clone());
                if decision.shadow() {
                    shadow = true;
                }
            }
            DecisionKind::Act(Action::Route) => {
                let task = decision.rule.def.task.as_deref()?;
                let label = if source.is_empty() { tool_name } else { source };
                let text = crate::core::task_cmd::digest_output(task, raw, label)?;
                return Some(Applied {
                    text,
                    methods: vec!["digest"],
                    rules: vec![decision.rule.name.clone()],
                    pipeline: pipeline.name.clone(),
                    subject,
                    shadow: decision.shadow(),
                });
            }
            DecisionKind::Act(_) => return None,
        }
    }
    if methods.is_empty() {
        return None;
    }
    let ctx = MethodCtx::new(source);
    let origin = Origin {
        tool: Some(tool_name.to_string()),
        path: (!source.is_empty()).then(|| source.to_string()),
        command: None,
        block_index: None,
    };
    let provenance = rtk_engine::marker::provenance_of(raw);
    let block = Block {
        text: raw,
        subject,
        format,
        origin: &origin,
        provenance: &provenance,
    };
    // Panic containment per chain rather than per invocation. `run` catches
    // panics too, but there it costs the whole response: this returns None,
    // the caller passes the raw output through, and tracking and the decision
    // log still run. The size guard stays with the caller, which needs the
    // unguarded text to record what the chain actually did.
    let text = rtk_engine::chain::run_chain_caught(&methods, &block, &ctx)?;
    Some(Applied {
        text,
        methods: method_names,
        rules,
        pipeline: pipeline.name.clone(),
        subject,
        shadow,
    })
}

fn shadow_eval_candidate(
    subject: Subject,
    format: ContentFormat,
    source: &str,
    raw: &str,
    facts: &Facts,
) {
    let Some((name, loaded)) = crate::core::pipeline_config::load_eval_candidate() else {
        return;
    };
    let mut facts = facts.clone();
    facts.retain(|k, _| {
        !k.starts_with("class.") && !k.starts_with("score.") && !k.starts_with("noul.")
    });
    let position = Position::hook(Event::AfterTool, subject);
    let decisions = evaluate(&loaded.resolved, position, &facts);
    let mut methods: Vec<&'static dyn Method> = Vec::new();
    let mut method_names: Vec<&'static str> = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    for decision in &decisions {
        if let DecisionKind::Transform {
            methods: selected, ..
        } = &decision.kind
        {
            methods.extend_from_slice(selected);
            method_names.extend(selected.iter().map(|m| m.name()));
            rules.push(decision.rule.name.clone());
        }
    }
    if methods.is_empty() {
        return;
    }
    let ctx = MethodCtx::new(source);
    let origin = Origin {
        tool: None,
        path: (!source.is_empty()).then(|| source.to_string()),
        command: None,
        block_index: None,
    };
    let provenance = rtk_engine::marker::provenance_of(raw);
    let block = Block {
        text: raw,
        subject,
        format,
        origin: &origin,
        provenance: &provenance,
    };
    let Some(text) = rtk_engine::chain::run_chain_caught(&methods, &block, &ctx) else {
        return;
    };
    let applied = Applied {
        text,
        methods: method_names,
        rules,
        pipeline: name,
        subject,
        shadow: true,
    };
    attribute(&applied, raw, &applied.text);
}

#[cfg(not(test))]
fn tracker() -> anyhow::Result<Tracker> {
    Tracker::new()
}

#[cfg(test)]
fn tracker() -> anyhow::Result<Tracker> {
    Tracker::new_in_memory()
}

fn attribute(applied: &Applied, raw: &str, emitted: &str) {
    let Ok(tracker) = tracker() else {
        return;
    };
    let tokens_in = estimate_tokens(raw);
    let tokens_out = estimate_tokens(emitted);
    let kind = if applied.shadow { "shadow" } else { "hook" };
    for method in &applied.methods {
        let _ = tracker.record_attribution(kind, "method", method, tokens_in, tokens_out);
    }
    for rule in &applied.rules {
        let _ = tracker.record_attribution(kind, "rule", rule, tokens_in, tokens_out);
    }
    if !applied.pipeline.is_empty() {
        let _ =
            tracker.record_attribution(kind, "pipeline", &applied.pipeline, tokens_in, tokens_out);
        let slice =
            crate::core::tracking::pipeline_slice_key(&applied.pipeline, applied.subject.as_str());
        let _ = tracker.record_attribution(kind, "pipeline-slice", &slice, tokens_in, tokens_out);
    }
}

static RECALL_SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"posthook/\d+_\d+-\d+_([a-z]+)").expect("valid regex"));

static RTK_RECALL_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[|&;(\s])rtk\s+recall\b").expect("valid regex"));

fn record_recovery(command: &str) {
    let tool = RECALL_SLUG
        .captures(command)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| {
            if RTK_RECALL_COMMAND.is_match(command) {
                "recall".to_string()
            } else {
                "unknown".to_string()
            }
        });
    if let Ok(tracker) = tracker() {
        let _ = tracker.record_attribution("hook", "recovery", &tool, 0, 0);
    }
}

/// Which size guard the caller wants.
///
/// The Claude path keeps the engine's token guard: its host swaps a structured
/// response back in and `rtk gain` reports tokens. A plain-text host answers in
/// bytes, so the OpenCode path gates on bytes instead.
///
/// The bytes gate also sits earlier: it rejects before `attribute`, `queue_log`
/// and `apply_terminal` run, and it takes back the recall write the hint made
/// ([`tee::discard_posthook_recall`]), so a rejected result is never recorded as
/// a saving and leaves nothing in the recall store. Redaction (`apply_terminal`)
/// is a terminal step on both paths — in this build the registry registers no
/// redactor at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Tokens,
    Bytes,
}

/// Chain + recall store + guard for a single content string.
/// `None` = no matching rule, chain was a no-op, not smaller, or shadow.
#[allow(clippy::too_many_arguments)]
fn filter_and_compose(
    config: &Config,
    pipeline: &Resolved,
    subject: Subject,
    tool_name: &str,
    source: &str,
    raw: &str,
    format: ContentFormat,
    duration_ms: u64,
    exit_code: Option<i64>,
    gate: Gate,
) -> Option<String> {
    // `is_rtk_command` is only meaningful for shell output. The Claude shell
    // path computes it in `process_shell`; the plain-text path (OpenCode
    // `tool.execute.after`) has to compute it here, or the shipped
    // `term-default` rule (`when = { is_rtk_command = false }`) can never
    // match and every shell result falls through uncompressed.
    let is_rtk_command = (subject == Subject::Shell).then(|| RTK_IN_COMMAND.is_match(source));

    // Recall-recovery gate: a command that reads the recall store must see the
    // raw bytes, else content elided by `cap` could never be read back.
    // Mirrors the gate in `process_shell`.
    if subject == Subject::Shell
        && (source.contains("tee/posthook")
            || RTK_RECALL_COMMAND.is_match(source)
            || tee::posthook_recall_dir(config)
                .is_some_and(|d| source.contains(&d.display().to_string())))
    {
        record_recovery(source);
        return None;
    }

    let applied = run_selected(
        pipeline,
        subject,
        tool_name,
        format,
        source,
        raw,
        duration_ms,
        is_rtk_command,
        exit_code,
    )?;
    let filtered = applied.text.clone();

    if filtered == raw {
        // No-op chain: skip the recall store entirely.
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
        return None;
    }

    // Byte gate, taken before the hint stores the raw output and before
    // attribution claims a saving: a result the host would not accept must not
    // be recorded as one. The Claude path keeps its token guard below.
    if gate == Gate::Bytes && filtered.len() >= raw.len() {
        return None;
    }

    // Ordering invariant (emit_guarded pattern): compose filtered + hint, then
    // guard — the hint can never push output above raw. The receipt is kept
    // because the hint stores the raw output before its own length is known.
    let slug = format!("{}_{}", tool_name.to_lowercase(), basename_of(source));
    let recall_hint = tee::posthook_tee_hint_with_receipt(config, raw, &slug);
    let composed = match &recall_hint {
        Some((hint, _)) => format!("{filtered}\n{hint}"),
        None => filtered,
    };

    if gate == Gate::Tokens && estimate_tokens(&composed) > estimate_tokens(raw) {
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
        return None;
    }
    // Same decision in bytes, after the hint: a marginal saving plus the hint
    // can exceed the raw output, and then nothing is emitted — so nothing is
    // attributed, logged or tracked, and the store write is taken back out.
    // Otherwise `rtk recall --list` would keep listing an entry nothing points
    // at, with the raw (unredacted) bytes still readable through `rtk recall`.
    if gate == Gate::Bytes && composed.len() >= raw.len() {
        if let Some((_, recall)) = &recall_hint {
            tee::discard_posthook_recall(recall, &config.retriever);
        }
        return None;
    }
    attribute(&applied, raw, &composed);
    queue_log(
        subject,
        None,
        source,
        &applied,
        estimate_tokens(raw),
        estimate_tokens(&composed),
    );
    if applied.shadow {
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
        return None;
    }
    let ctx = MethodCtx::new(source);
    Some(apply_terminal(composed, &ctx, subject, format))
}

/// WebSearch responses carry text as string elements inside a mixed
/// `results` array (strings + link objects). Each string element runs the
/// web rules; the array is rebuilt in place with link objects untouched.
fn process_websearch(
    event: &Value,
    config: &Config,
    pipeline: &Resolved,
    source: &str,
    duration_ms: u64,
) -> Option<String> {
    let results = event.pointer("/tool_response/results")?.as_array()?;

    let raw_concat: String = results
        .iter()
        .filter_map(|r| r.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if raw_concat.len() < pipeline.guards.min_size {
        return None;
    }

    let mut changed = false;
    let mut shadow = false;
    let mut last_applied: Option<Applied> = None;
    let mut new_results: Vec<Value> = Vec::with_capacity(results.len());
    let mut filtered_concat = String::new();
    for r in results {
        match r.as_str() {
            Some(text) => {
                let filtered = match run_selected(
                    pipeline,
                    Subject::Web,
                    "WebSearch",
                    ContentFormat::Web,
                    source,
                    text,
                    duration_ms,
                    None,
                    None,
                ) {
                    Some(applied) => {
                        let text_out = applied.text.clone();
                        shadow |= applied.shadow;
                        last_applied = Some(applied);
                        text_out
                    }
                    None => text.to_string(),
                };
                if filtered != text {
                    changed = true;
                }
                if !filtered_concat.is_empty() {
                    filtered_concat.push_str("\n\n");
                }
                filtered_concat.push_str(&filtered);
                new_results.push(Value::String(filtered));
            }
            None => new_results.push(r.clone()),
        }
    }

    if !changed || shadow {
        if let Some(applied) = &last_applied
            && shadow
        {
            attribute(applied, &raw_concat, &filtered_concat);
        }
        track(
            "WebSearch",
            "web",
            source,
            &raw_concat,
            &raw_concat,
            duration_ms,
        );
        return None;
    }

    // Compose-then-guard: hint rides in the last string element.
    let hint = tee::posthook_tee_hint(config, &raw_concat, "websearch_results");
    if let (Some(h), Some(last)) = (
        hint,
        new_results.iter_mut().rev().find_map(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        }),
    ) {
        last.push('\n');
        last.push_str(&h);
        filtered_concat.push('\n');
        filtered_concat.push_str(last);
    }

    if estimate_tokens(&filtered_concat) > estimate_tokens(&raw_concat) {
        track(
            "WebSearch",
            "web",
            source,
            &raw_concat,
            &raw_concat,
            duration_ms,
        );
        return None;
    }

    let mut updated = event.get("tool_response")?.clone();
    updated["results"] = Value::Array(new_results);

    if let Some(applied) = &last_applied {
        attribute(applied, &raw_concat, &filtered_concat);
        queue_log(
            Subject::Web,
            None,
            source,
            applied,
            estimate_tokens(&raw_concat),
            estimate_tokens(&filtered_concat),
        );
    }
    track(
        "WebSearch",
        "web",
        source,
        &raw_concat,
        &filtered_concat,
        duration_ms,
    );
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": POST_TOOL_USE_KEY,
                "updatedToolOutput": updated,
            }
        })
        .to_string(),
    )
}

/// `rtk` invoked anywhere in the command (start or after a shell operator):
/// feeds the `is_rtk_command` condition — the shipped `term-default` rule
/// skips such output because it already went through RTK's own filters.
static RTK_IN_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[|&;(\s])rtk\s").expect("valid regex"));

fn combine_streams(stdout: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n--- stderr ---\n{stderr}")
    }
}

fn process_shell(
    event: &Value,
    config: &Config,
    pipeline: &Resolved,
    duration_ms: u64,
) -> Option<String> {
    let response = event.get("tool_response")?;
    if response
        .get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || response
            .get("isImage")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return None;
    }

    let command = event
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let is_rtk_command = RTK_IN_COMMAND.is_match(command);
    // Recall-recovery gate: a command reading the recall store must see the
    // raw bytes (else e.g. an elided base64 blob could never be recovered).
    if command.contains("tee/posthook")
        || RTK_RECALL_COMMAND.is_match(command)
        || tee::posthook_recall_dir(config)
            .is_some_and(|d| command.contains(&d.display().to_string()))
    {
        record_recovery(command);
        return None;
    }

    let stdout = response
        .get("stdout")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let stderr = response
        .get("stderr")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if stdout.len() + stderr.len() < pipeline.guards.min_size {
        return None;
    }

    let first_word = command.split_whitespace().next().unwrap_or("");
    let applied_stdout = run_selected(
        pipeline,
        Subject::Shell,
        "Bash",
        ContentFormat::Term,
        command,
        stdout,
        duration_ms,
        Some(is_rtk_command),
        None,
    );
    let shadow = applied_stdout.as_ref().map(|a| a.shadow).unwrap_or(false);
    let filtered_stdout = applied_stdout
        .as_ref()
        .map(|a| a.text.clone())
        .unwrap_or_else(|| stdout.to_string());
    let filtered_stderr = if stderr.is_empty() {
        String::new()
    } else {
        run_selected(
            pipeline,
            Subject::Shell,
            "Bash",
            ContentFormat::Term,
            command,
            stderr,
            duration_ms,
            Some(is_rtk_command),
            None,
        )
        .map(|a| a.text)
        .unwrap_or_else(|| stderr.to_string())
    };

    let raw_combined = combine_streams(stdout, stderr);

    if (filtered_stdout == stdout && filtered_stderr == stderr) || shadow {
        if shadow && let Some(applied) = &applied_stdout {
            attribute(applied, &raw_combined, &filtered_stdout);
            queue_log(
                Subject::Shell,
                Some(command),
                "",
                applied,
                estimate_tokens(&raw_combined),
                estimate_tokens(&filtered_stdout),
            );
        }
        track(
            "Bash",
            "term",
            first_word,
            &raw_combined,
            &raw_combined,
            duration_ms,
        );
        return None;
    }

    let slug = format!("bash_{first_word}");
    let hint = tee::posthook_tee_hint(config, &raw_combined, &slug);
    let composed_stdout = match hint {
        Some(h) => format!("{filtered_stdout}\n{h}"),
        None => filtered_stdout,
    };

    let emitted_combined = combine_streams(&composed_stdout, &filtered_stderr);
    if estimate_tokens(&emitted_combined) > estimate_tokens(&raw_combined) {
        track(
            "Bash",
            "term",
            first_word,
            &raw_combined,
            &raw_combined,
            duration_ms,
        );
        return None;
    }

    let mut updated = response.clone();
    let obj = updated.as_object_mut()?;
    obj.insert("stdout".into(), json!(composed_stdout));
    obj.insert("stderr".into(), json!(filtered_stderr));

    if let Some(applied) = &applied_stdout {
        attribute(applied, &raw_combined, &emitted_combined);
        queue_log(
            Subject::Shell,
            Some(command),
            "",
            applied,
            estimate_tokens(&raw_combined),
            estimate_tokens(&emitted_combined),
        );
    }
    track(
        "Bash",
        "term",
        first_word,
        &raw_combined,
        &emitted_combined,
        duration_ms,
    );
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": POST_TOOL_USE_KEY,
                "updatedToolOutput": updated,
            }
        })
        .to_string(),
    )
}

/// Path or URL the tracking rows and `when` facts key on.
fn source_of(event: &Value, tool_name: &str) -> String {
    let pointer = match tool_name {
        "Read" | "Grep" | "Glob" => "/tool_input/file_path",
        "WebFetch" => "/tool_input/url",
        _ => return String::new(),
    };
    event
        .pointer(pointer)
        .or_else(|| event.pointer("/tool_input/path"))
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Content string the model would read + its format
/// (`None` format = unsupported → tracked passthrough).
fn extract(
    event: &Value,
    tool_name: &str,
    source: &str,
) -> Option<(String, Option<ContentFormat>)> {
    match tool_name {
        "Read" => {
            let content = event.pointer("/tool_response/file/content")?.as_str()?;
            // Lockfile check first: package-lock.json would otherwise
            // classify as plain json by extension.
            let format = if is_lockfile_name(&basename_of(source)) {
                ContentFormat::Lockfile
            } else if extension_of(source) == "json" {
                ContentFormat::Json
            } else {
                // Plain text/code read: reaches the rules as `term` so
                // conditions like `strip-line-numbers` can fire; no shipped
                // rule matches it, so the default behaviour is passthrough.
                ContentFormat::Term
            };
            Some((content.to_string(), Some(format)))
        }
        "Grep" => {
            let response = event.get("tool_response")?;
            let mode = response.get("mode").and_then(|m| m.as_str())?;
            let content = response.get("content").and_then(|c| c.as_str());
            match (mode, content) {
                ("content", Some(text)) => Some((text.to_string(), Some(ContentFormat::Matches))),
                // count / files_with_matches are already compact.
                (_, Some(text)) => Some((text.to_string(), None)),
                _ => None,
            }
        }
        "WebFetch" => {
            let result = event.pointer("/tool_response/result")?.as_str()?;
            Some((result.to_string(), Some(ContentFormat::Web)))
        }
        name if name.starts_with("mcp__") => {
            let text = mcp_text(event.get("tool_response")?)?;
            let format = if serde_json::from_str::<Value>(text.trim()).is_ok() {
                ContentFormat::Json
            } else {
                ContentFormat::Term
            };
            Some((text, Some(format)))
        }
        // Glob output (filenames array) is not `matches` format.
        "Glob" => {
            let filenames = event.pointer("/tool_response/filenames")?.as_array()?;
            let joined = filenames
                .iter()
                .filter_map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            Some((joined, None))
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => None,
        _ => {
            let text = mcp_text(event.get("tool_response")?)?;
            let format = if serde_json::from_str::<Value>(text.trim()).is_ok() {
                ContentFormat::Json
            } else {
                ContentFormat::Term
            };
            Some((text, Some(format)))
        }
    }
}

/// Lockfile basenames served by the `lockfile` format (type is then
/// content-sniffed inside the method).
fn is_lockfile_name(basename: &str) -> bool {
    matches!(
        basename,
        "package-lock.json" | "Cargo.lock" | "yarn.lock" | "pnpm-lock.yaml"
    )
}

/// Re-wrap the filtered text in the same output shape the tool produced.
/// Returns `(updatedToolOutput, format_converted)`; `None` when the shape
/// can't be reproduced confidently → emit nothing.
fn rebuild(event: &Value, tool_name: &str, composed: &str) -> Option<(Value, bool)> {
    let mut response = event.get("tool_response")?.clone();
    match tool_name {
        "Read" => {
            let file = response.get_mut("file")?.as_object_mut()?;
            file.insert("content".into(), json!(composed));
            // Host renders line numbers from raw content: keep metadata
            // coherent with the replacement.
            let lines = composed.lines().count();
            file.insert("numLines".into(), json!(lines));
            file.insert("totalLines".into(), json!(lines));
            // Converted content can no longer anchor Edit.old_string.
            Some((response, true))
        }
        "Grep" => {
            let obj = response.as_object_mut()?;
            obj.insert("content".into(), json!(composed));
            obj.insert("numLines".into(), json!(composed.lines().count()));
            Some((response, false))
        }
        "WebFetch" => {
            let obj = response.as_object_mut()?;
            obj.insert("result".into(), json!(composed));
            Some((response, false))
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => None,
        _ => {
            let obj = response.as_object_mut()?;
            if obj.get("result").and_then(Value::as_str).is_some() {
                obj.insert("result".into(), json!(composed));
            } else {
                obj.get("content")?.as_array()?;
                obj.insert(
                    "content".into(),
                    json!([{"type": "text", "text": composed}]),
                );
            }
            Some((response, false))
        }
    }
}

/// The text an MCP tool returned, for the two shapes RTK can put back
/// unambiguously: a `result` string, or a `content` array whose every block is
/// text. Anything else — an image, an embedded resource, a mixed array —
/// returns `None`, and the output passes through untouched.
fn mcp_text(response: &Value) -> Option<String> {
    if let Some(text) = response.get("result").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let blocks = response.get("content")?.as_array()?;
    let texts: Vec<&str> = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    if texts.is_empty() || texts.len() != blocks.len() {
        return None;
    }
    Some(texts.join("\n"))
}

fn extension_of(source: &str) -> &str {
    std::path::Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
}

fn basename_of(source: &str) -> String {
    if let Some(host) = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
    {
        return host.split('/').next().unwrap_or(host).to_string();
    }
    std::path::Path::new(source)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output")
        .to_string()
}

/// One row `track` writes: what `rtk gain` aggregates over.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackRow {
    original_cmd: String,
    rtk_cmd: String,
    input_tokens: usize,
    output_tokens: usize,
    duration_ms: u64,
}

#[cfg(test)]
thread_local! {
    /// Rows `track` wrote on this test thread, in order. `tracker()` is an
    /// in-memory database built per call, so a test cannot read the real table
    /// back; the thread-local sink is per test because libtest gives each one
    /// its own thread.
    static TRACKED: std::cell::RefCell<Vec<TrackRow>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn tracked_rows() -> Vec<TrackRow> {
    TRACKED.with(|rows| rows.borrow().clone())
}

#[cfg(test)]
fn clear_tracked_rows() {
    TRACKED.with(|rows| rows.borrow_mut().clear());
}

/// Record a tracking row without ever crashing (mirrors
/// `record_parse_failure_silent`). Every posthook code path tracks, including
/// passthroughs — otherwise gains silently fail (pipe_cmd precedent).
fn track(
    tool_name: &str,
    format_token: &str,
    source: &str,
    raw: &str,
    emitted: &str,
    duration_ms: u64,
) {
    let row = TrackRow {
        original_cmd: if source.is_empty() {
            tool_name.to_string()
        } else {
            format!("{tool_name} {source}")
        },
        rtk_cmd: format!("rtk posthook {} {}", tool_name.to_lowercase(), format_token),
        input_tokens: estimate_tokens(raw),
        output_tokens: estimate_tokens(emitted),
        duration_ms,
    };
    #[cfg(test)]
    TRACKED.with(|rows| rows.borrow_mut().push(row.clone()));
    if let Ok(tracker) = tracker() {
        let _ = tracker.record(
            &row.original_cmd,
            &row.rtk_cmd,
            row.input_tokens,
            row.output_tokens,
            row.duration_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::retriever::RecoveryMode;

    #[test]
    fn recall_commands_pass_the_recovery_gate() {
        for cmd in [
            "rtk recall 3f9a1c",
            "cd x && rtk recall 3f9a1c --tail 50",
            "(rtk recall abc)",
        ] {
            assert!(RTK_RECALL_COMMAND.is_match(cmd), "{cmd}");
        }
        for cmd in ["rtk gain", "echo recall", "myrtk recall x", "rtk recalled"] {
            assert!(!RTK_RECALL_COMMAND.is_match(cmd), "{cmd}");
        }
    }
    use rtk_engine::pipeline::{Scope, SourceFile, parse, resolve};

    fn test_config() -> Config {
        let mut config = Config::default();
        config.retriever.mode = crate::core::retriever::RecoveryMode::Disabled;
        config
    }

    fn pipeline_from(text: &str) -> Resolved {
        let parsed = parse(text, "test.toml");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let (resolved, diagnostics) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "test.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        resolved
    }

    fn posthook_pipeline() -> Resolved {
        pipeline_from("[pipeline]\nextends = \"rtk:posthook\"\n")
    }

    fn json_reads_pipeline() -> Resolved {
        pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.json-reads]\nenabled = true\n",
        )
    }

    fn default_pipeline() -> Resolved {
        let (resolved, _) = resolve(&[]);
        resolved
    }

    fn run_with(event: &Value, config: &Config, pipeline: &Resolved) -> Option<String> {
        process_with_config(event, config, pipeline)
    }

    fn run_posthook(event: &Value) -> Option<String> {
        run_with(event, &test_config(), &posthook_pipeline())
    }

    fn fixture(name: &str) -> Value {
        let raw = match name {
            "read_flat" => include_str!("../../tests/fixtures/posthook/read_json_flat.json"),
            "read_nested" => include_str!("../../tests/fixtures/posthook/read_json_nested.json"),
            "grep_small" => include_str!("../../tests/fixtures/posthook/grep_content_small.json"),
            "grep_large" => {
                include_str!("../../tests/fixtures/posthook/grep_content_large_truncated.json")
            }
            "grep_count" => include_str!("../../tests/fixtures/posthook/grep_count.json"),
            "webfetch_md" => {
                include_str!("../../tests/fixtures/posthook/webfetch_markdown_url.json")
            }
            "websearch" => include_str!("../../tests/fixtures/posthook/websearch_results.json"),
            "glob" => include_str!("../../tests/fixtures/posthook/glob_json.json"),
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(raw).expect("fixture parses")
    }

    #[test]
    fn combine_streams_is_symmetric_so_the_separator_is_not_counted_as_savings() {
        assert_eq!(combine_streams("out", ""), "out");
        assert_eq!(combine_streams("out", "err"), "out\n--- stderr ---\nerr");
        let raw = combine_streams("a\nb", "e1\ne2");
        let emitted = combine_streams("a", "e1");
        let sep = "\n--- stderr ---\n";
        assert!(raw.contains(sep) && emitted.contains(sep));
        assert_eq!(
            estimate_tokens(&raw) - estimate_tokens(&emitted),
            estimate_tokens("a\nb") + estimate_tokens("e1\ne2")
                - estimate_tokens("a")
                - estimate_tokens("e1"),
            "the shared separator must cancel out of the saved-token delta"
        );
    }

    #[test]
    fn test_default_pipeline_has_no_after_tool_rules_and_passes_everything_through() {
        let pipeline = default_pipeline();
        for name in ["read_flat", "grep_large", "webfetch_md", "websearch"] {
            assert!(
                run_with(&fixture(name), &test_config(), &pipeline).is_none(),
                "{name} must pass through on rtk:default: upgrading is a no-op"
            );
        }
    }

    fn repeated_output() -> String {
        std::iter::repeat_n("repeated build line", 400)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A host payload with the fields a shell result carries; metadata fields
    /// are set per test.
    fn agent_result(tool: &str, args: &Value, raw: &str) -> AgentToolResult {
        AgentToolResult {
            tool: tool.to_string(),
            args: args.clone(),
            output: raw.to_string(),
            ..AgentToolResult::default()
        }
    }

    #[test]
    fn test_agent_text_tracks_a_compressed_result() {
        // Before this, an OpenCode compression reached the model but never
        // `rtk gain`: the plain-text path returned the text without tracking.
        clear_tracked_rows();
        let pipeline =
            pipeline_from("[[hook.after_tool.shell]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n");
        let raw = repeated_output();
        let result = agent_result("bash", &serde_json::json!({"command": "make build"}), &raw);
        assert!(agent_text_with_config("opencode", &result, &test_config(), &pipeline).is_some());

        let rows = tracked_rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row.rtk_cmd, "rtk posthook bash term");
        assert_eq!(row.original_cmd, "bash make build");
        assert!(
            row.output_tokens < row.input_tokens,
            "a saving must be recorded: {row:?}"
        );
    }

    #[test]
    fn test_agent_text_tracks_a_shape_it_cannot_rewrite_as_a_passthrough() {
        // Mirrors the Claude adapter's Grep-count row: nothing is emitted, but
        // the call still shows up in `rtk gain` with zero savings.
        clear_tracked_rows();
        let pipeline =
            pipeline_from("[[hook.after_tool.search]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n");
        let result = agent_result(
            "glob",
            &serde_json::json!({"pattern": "**/*.rs"}),
            &repeated_output(),
        );
        assert!(agent_text_with_config("opencode", &result, &test_config(), &pipeline).is_none());

        let rows = tracked_rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].rtk_cmd, "rtk posthook glob raw");
        assert_eq!(rows[0].input_tokens, rows[0].output_tokens);
    }

    /// A pipeline whose chain saves only a few bytes, so the recall hint the
    /// host would also receive pushes the composed result above the raw size.
    fn thin_saving_pipeline() -> Resolved {
        pipeline_from("[[hook.after_tool.shell]]\nname = \"ansi\"\nuse = [\"ansi\"]\n")
    }

    /// Three long lines carrying an ANSI prefix each: the chain saves ~27 bytes.
    fn thin_saving_output(secret: Option<&str>) -> String {
        let body = match secret {
            Some(secret) => format!("{secret} {}", "x".repeat(300)),
            None => "x".repeat(300),
        };
        format!("\x1b[31m{body}\x1b[0m\n").repeat(3)
    }

    /// A config that writes the recall store into `dir`, in the given mode.
    fn recall_config(dir: &std::path::Path, mode: RecoveryMode) -> Config {
        let mut config = test_config();
        config.retriever.mode = mode;
        config.retriever.tee_directory = Some(dir.join("tee"));
        config.retriever.database_path = Some(dir.join("recall.db"));
        config
    }

    /// Rows in the recall store the config points at; 0 when nothing was stored.
    fn recall_rows(config: &Config) -> i64 {
        let path = config
            .retriever
            .database_path
            .as_ref()
            .expect("test config sets a path");
        let Ok(conn) = rusqlite::Connection::open(path) else {
            return 0;
        };
        conn.query_row("SELECT COUNT(*) FROM recall", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Files under the posthook tee directory, which a rejected result must
    /// leave empty.
    fn posthook_tee_files(config: &Config) -> usize {
        let dir = tee::posthook_recall_dir(config).expect("tee dir resolves");
        std::fs::read_dir(dir)
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    #[test]
    fn test_agent_text_tracks_nothing_when_the_guarded_result_is_rejected() {
        // A saving smaller than the recall hint appended after it: the composed
        // result is larger than the raw one, so it must not reach the model.
        // Every effect of accepting it has to be absent — no `rtk gain` row, no
        // decision-log line, and nothing left in the recall store or the tee
        // directory, since `rtk recall --list` enumerates it either way.
        for mode in [RecoveryMode::Tee, RecoveryMode::Sqlite] {
            clear_tracked_rows();
            pipeline_log::clear_offered_entries();
            let dir = tempfile::tempdir().expect("tempdir");
            let config = recall_config(dir.path(), mode);

            let result = agent_result(
                "bash",
                &serde_json::json!({"command": "make build"}),
                &thin_saving_output(None),
            );
            assert!(
                agent_text_with_config("opencode", &result, &config, &thin_saving_pipeline())
                    .is_none(),
                "{mode:?}: the hint pushes the result above the raw size"
            );

            let rows = tracked_rows();
            assert!(rows.is_empty(), "{mode:?}: `rtk gain` recorded {rows:?}");
            let offered = pipeline_log::offered_entries();
            assert!(
                offered.is_empty(),
                "{mode:?}: the decision log recorded {offered:?}"
            );
            assert_eq!(recall_rows(&config), 0, "{mode:?}: `rtk recall --list`");
            assert_eq!(posthook_tee_files(&config), 0, "{mode:?}: tee directory");
        }
    }

    #[test]
    fn test_agent_text_carries_the_host_exit_code_into_the_facts() {
        // `exit_code` is a legal `hook.after_tool.shell` condition, and the
        // host's `metadata.exit` is the only place it can come from on the
        // OpenCode path.
        let pipeline = pipeline_from(
            "[[hook.after_tool.shell]]\nname = \"failed-only\"\nuse = [\"dedup\"]\n\
             when = { exit_code = 1 }\n",
        );
        let raw = repeated_output();
        let args = serde_json::json!({"command": "make build"});
        for (exit, want) in [(Some(1), true), (Some(0), false), (None, false)] {
            let mut result = agent_result("shell", &args, &raw);
            result.exit_code = exit;
            assert_eq!(
                agent_text_with_config("opencode", &result, &test_config(), &pipeline).is_some(),
                want,
                "exit_code = {exit:?} must {}match",
                if want { "" } else { "not " }
            );
        }
    }

    #[test]
    fn test_agent_text_carries_the_host_duration_into_the_facts() {
        // The plain-text path used to hard-code duration_ms to 0, so a rule
        // keyed on how long the call took could never fire.
        let pipeline = pipeline_from(
            "[[hook.after_tool.shell]]\nname = \"slow-only\"\nuse = [\"dedup\"]\n\
             when = { duration_ms_gt = 0 }\n",
        );
        let raw = repeated_output();
        let args = serde_json::json!({"command": "make build"});
        let mut result = agent_result("shell", &args, &raw);
        result.duration_ms = 12;
        assert!(
            agent_text_with_config("opencode", &result, &test_config(), &pipeline).is_some(),
            "a non-zero duration_ms must reach the facts"
        );
        result.duration_ms = 0;
        assert!(
            agent_text_with_config("opencode", &result, &test_config(), &pipeline).is_none(),
            "duration_ms = 0 must not match"
        );
    }

    #[test]
    fn test_agent_text_passes_through_when_no_after_tool_rule_matches() {
        let raw = repeated_output();
        assert!(
            agent_text_with_config(
                "opencode",
                &agent_result("bash", &serde_json::json!({"command": "make build"}), &raw),
                &test_config(),
                &default_pipeline(),
            )
            .is_none(),
            "rtk:default has no after_tool rules"
        );
    }

    #[test]
    fn test_agent_text_applies_a_matching_after_tool_rule() {
        let pipeline =
            pipeline_from("[[hook.after_tool.shell]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n");
        let raw = repeated_output();
        let out = agent_text_with_config(
            "opencode",
            &agent_result("bash", &serde_json::json!({"command": "make build"}), &raw),
            &test_config(),
            &pipeline,
        )
        .expect("shell rule must run for an OpenCode bash result");
        assert!(out.contains("[x400]"), "{out}");
        assert!(out.len() < raw.len());
    }

    #[test]
    fn test_agent_text_maps_the_opencode_tool_vocabulary() {
        let pipeline =
            pipeline_from("[[hook.after_tool.read]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n");
        let raw = repeated_output();
        assert!(
            agent_text_with_config(
                "opencode",
                &agent_result(
                    "read",
                    &serde_json::json!({"filePath": "/tmp/build.log"}),
                    &raw
                ),
                &test_config(),
                &pipeline,
            )
            .is_some(),
            "read maps to the read subject"
        );
        assert!(
            agent_text_with_config(
                "opencode",
                &agent_result("bash", &serde_json::json!({"command": "make build"}), &raw),
                &test_config(),
                &pipeline,
            )
            .is_none(),
            "bash is not a read"
        );
    }

    #[test]
    fn test_agent_text_emits_nothing_when_the_result_is_unchanged() {
        let pipeline =
            pipeline_from("[[hook.after_tool.shell]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n");
        let raw = (0..400)
            .map(|i| format!("unique line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            agent_text_with_config(
                "opencode",
                &agent_result("bash", &serde_json::json!({"command": "make build"}), &raw),
                &test_config(),
                &pipeline,
            )
            .is_none(),
            "a no-op chain must not replace the output"
        );
    }

    /// Reviewer's repro: 600 uniform lines must hit the shipped `term-default`
    /// rule on the OpenCode after-tool path and come back capped, not verbatim.
    fn numbered_output(n: usize) -> String {
        (1..=n)
            .map(|i| format!("q{i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_agent_text_term_default_matches_a_shell_result() {
        let raw = numbered_output(600);
        let out = agent_text_with_config(
            "opencode",
            &agent_result("shell", &serde_json::json!({"command": "seq"}), &raw),
            &test_config(),
            &posthook_pipeline(),
        )
        .expect("the shipped term-default rule must match a non-rtk shell result");

        let lines = out.lines().count();
        assert!(lines < 300, "expected a head+tail cap, got {lines} lines");
        assert!(out.contains("q1"), "the head must survive");
        assert!(out.contains("q600"), "the tail must survive");
        assert!(!out.contains("q350"), "the middle must be elided");
    }

    #[test]
    fn test_agent_text_sets_is_rtk_command_for_shell_subjects() {
        let raw = numbered_output(600);
        // `term-default` is `when = { is_rtk_command = false }`: a command that
        // already went through rtk must be skipped, not compressed again.
        for command in ["rtk seq", "cd x && rtk ls -la"] {
            assert!(
                agent_text_with_config(
                    "opencode",
                    &agent_result("shell", &serde_json::json!({ "command": command }), &raw),
                    &test_config(),
                    &posthook_pipeline(),
                )
                .is_none(),
                "{command} runs rtk already and must pass through"
            );
        }
    }

    #[test]
    fn test_agent_text_skips_compression_for_recall_recovery_commands() {
        let raw = numbered_output(600);
        // Reading the recall store back must see the raw bytes, else content
        // elided by `cap` could never be recovered.
        for command in [
            "rtk recall ab12cd34",
            "cd x && rtk recall ab12cd34 --tail 50",
            "cat tee/posthook/12_3-4_read",
        ] {
            assert!(
                agent_text_with_config(
                    "opencode",
                    &agent_result("shell", &serde_json::json!({ "command": command }), &raw),
                    &test_config(),
                    &posthook_pipeline(),
                )
                .is_none(),
                "{command} reads the recall store and must pass through"
            );
        }
    }

    #[test]
    fn test_agent_text_maps_the_opencode_2x_tool_ids() {
        for (tool, want) in [
            ("shell", Subject::Shell),
            ("bash", Subject::Shell),
            ("patch", Subject::Edit),
            ("read", Subject::Read),
            ("write", Subject::Write),
            ("edit", Subject::Edit),
            ("grep", Subject::Search),
            ("glob", Subject::Search),
            ("webfetch", Subject::Web),
            ("websearch", Subject::Web),
            ("task", Subject::Subagent),
        ] {
            assert_eq!(subject_of_for("opencode", tool), want, "{tool}");
        }
        // No subject is a todo list; OpenCode's own confirmation is passed on.
        assert_eq!(subject_of_for("opencode", "todowrite"), Subject::Any);
    }

    #[test]
    fn test_read_json_flat_rewrites_to_toon_with_note_when_json_reads_enabled() {
        let out = run_with(
            &fixture("read_flat"),
            &test_config(),
            &json_reads_pipeline(),
        )
        .expect("flat json must be rewritten");
        let out: Value = serde_json::from_str(&out).expect("valid JSON");
        let hook = &out["hookSpecificOutput"];
        assert_eq!(hook["hookEventName"], "PostToolUse");

        let file = &hook["updatedToolOutput"]["file"];
        let content = file["content"].as_str().unwrap();
        assert!(
            content.starts_with("[120]{id,name,email,active,score}:"),
            "TOON content"
        );
        assert!(
            file["filePath"]
                .as_str()
                .unwrap()
                .ends_with("data_flat.json")
        );
        assert_eq!(
            file["numLines"].as_u64().unwrap() as usize,
            content.lines().count()
        );
        assert_eq!(hook["updatedToolOutput"]["type"], "text");

        assert!(
            hook["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Edit old_string")
        );
    }

    #[test]
    fn test_read_json_passes_through_on_plain_posthook() {
        assert!(
            run_posthook(&fixture("read_flat")).is_none(),
            "json-reads ships disabled"
        );
    }

    #[test]
    fn test_read_nested_above_min_size_minifies_when_json_reads_enabled() {
        let out = run_with(
            &fixture("read_nested"),
            &test_config(),
            &json_reads_pipeline(),
        );
        if let Some(out) = out {
            let v: Value = serde_json::from_str(&out).expect("valid JSON");
            let content = v["hookSpecificOutput"]["updatedToolOutput"]["file"]["content"]
                .as_str()
                .unwrap();
            assert!(content.starts_with('{'), "minified JSON expected");
        }
    }

    #[test]
    fn test_grep_content_groups_by_file() {
        let out = run_posthook(&fixture("grep_large")).expect("large grep content rewritten");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let hook = &v["hookSpecificOutput"];
        let response = &hook["updatedToolOutput"];
        let content = response["content"].as_str().unwrap();
        assert!(content.contains("matches in"), "grouped header");
        assert_eq!(response["mode"], "content");
        assert_eq!(
            response["numLines"].as_u64().unwrap() as usize,
            content.lines().count()
        );
        assert!(hook.get("additionalContext").is_none());
    }

    #[test]
    fn test_mcp_text_reads_both_shapes_and_refuses_the_rest() {
        let result = serde_json::json!({"result": "hello"});
        assert_eq!(mcp_text(&result).as_deref(), Some("hello"));

        let blocks = serde_json::json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "text", "text": "b"}
        ]});
        assert_eq!(mcp_text(&blocks).as_deref(), Some("a\nb"));

        // A non-text block cannot be put back, so the output is left alone.
        let mixed = serde_json::json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "image", "data": "AAAA"}
        ]});
        assert!(mcp_text(&mixed).is_none());
        assert!(mcp_text(&serde_json::json!({"content": []})).is_none());
        assert!(mcp_text(&serde_json::json!({"other": 1})).is_none());
    }

    fn mcp_event(tool: &str, response: serde_json::Value) -> Value {
        serde_json::json!({
            "tool_name": tool,
            "tool_input": {},
            "tool_response": response,
            "duration_ms": 5
        })
    }

    fn mcp_pipeline() -> Resolved {
        pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n\
             [[hook.after_tool.mcp]]\nname = \"squeeze\"\nuse = [\"dedup\", \"cap\"]\n",
        )
    }

    #[test]
    fn test_mcp_output_is_filtered_and_rebuilt_in_place() {
        let noisy = std::iter::repeat_n("repeated mcp line", 400)
            .collect::<Vec<_>>()
            .join("\n");
        for (tool, response, pointer) in [
            (
                "mcp__linear__get_issue",
                serde_json::json!({"result": noisy.clone()}),
                "/hookSpecificOutput/updatedToolOutput/result",
            ),
            (
                "mcp__linear__list",
                serde_json::json!({"content": [{"type": "text", "text": noisy.clone()}]}),
                "/hookSpecificOutput/updatedToolOutput/content/0/text",
            ),
        ] {
            let event = mcp_event(tool, response);
            let out = run_with(&event, &test_config(), &mcp_pipeline())
                .unwrap_or_else(|| panic!("{tool}: expected a replacement"));
            let parsed: Value = serde_json::from_str(&out).expect("valid json");
            let text = parsed
                .pointer(pointer)
                .and_then(Value::as_str)
                .expect(pointer);
            assert!(text.contains("[x400]"), "{tool}: {text}");
        }
    }

    #[test]
    fn test_subagent_output_is_filtered_and_rebuilt_in_place() {
        let noisy = std::iter::repeat_n("repeated subagent finding", 400)
            .collect::<Vec<_>>()
            .join("\n");
        let pipeline =
            pipeline_from("[[hook.after_tool.subagent]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n");
        for tool in ["Task", "Agent"] {
            let event = mcp_event(
                tool,
                serde_json::json!({
                    "content": [{"type": "text", "text": noisy.clone()}],
                    "totalTokens": 1234
                }),
            );
            let out = run_with(&event, &test_config(), &pipeline)
                .unwrap_or_else(|| panic!("{tool}: expected a replacement"));
            let parsed: Value = serde_json::from_str(&out).expect("valid json");
            let text = parsed
                .pointer("/hookSpecificOutput/updatedToolOutput/content/0/text")
                .and_then(Value::as_str)
                .expect("text block");
            assert!(text.contains("[x400]"), "{tool}: {text}");
            assert_eq!(
                parsed.pointer("/hookSpecificOutput/updatedToolOutput/totalTokens"),
                Some(&serde_json::json!(1234)),
                "the rest of the response is kept"
            );
        }
    }

    #[test]
    fn test_write_and_edit_output_is_never_rewritten() {
        let pipeline = pipeline_from("[[hook.after_tool.any]]\nname = \"a\"\nuse = [\"dedup\"]\n");
        let body = std::iter::repeat_n("same line", 400)
            .collect::<Vec<_>>()
            .join("\n");
        for tool in ["Write", "Edit"] {
            let event = mcp_event(
                tool,
                serde_json::json!({"content": body.clone(), "filePath": "a.rs"}),
            );
            assert!(
                run_with(&event, &test_config(), &pipeline).is_none(),
                "{tool}"
            );
        }
    }

    #[test]
    fn test_mcp_output_with_an_image_block_passes_through() {
        let event = mcp_event(
            "mcp__x__screenshot",
            serde_json::json!({"content": [
                {"type": "text", "text": "x".repeat(4000)},
                {"type": "image", "data": "AAAA"}
            ]}),
        );
        assert!(run_with(&event, &test_config(), &mcp_pipeline()).is_none());
    }

    /// A route whose worker cannot be reached must leave the agent's output
    /// alone. `digest-none` has no upstream, which is the cheapest reachable
    /// failure: no network call is attempted and nothing is emitted.
    #[test]
    fn test_after_tool_route_without_an_upstream_passes_output_through() {
        let pipeline = pipeline_from(
            "[pipeline]\nname = \"t\"\n\
             [task.digest-none]\nkind = \"digest\"\n\
             [[hook.after_tool.shell]]\nname = \"route-all\"\n\
             action = \"route\"\ntask = \"digest-none\"\n",
        );
        let noisy = (0..400)
            .map(|i| format!("line {i}: verbose build output"))
            .collect::<Vec<_>>()
            .join("\n");
        let event = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "make build"},
            "tool_response": {"stdout": noisy, "stderr": ""},
            "duration_ms": 10
        });
        assert!(
            run_with(&event, &test_config(), &pipeline).is_none(),
            "an unreachable worker must not cost the agent its output"
        );
    }

    /// The route arm must not fire for a task that is not a `digest`: a
    /// `summarize` instance reads files, and handing it a tool's stdout would
    /// send the worker a prompt its template never expected.
    #[test]
    fn test_after_tool_route_ignores_a_non_digest_task() {
        let pipeline = pipeline_from(
            "[pipeline]\nname = \"t\"\n\
             [upstream.u]\nprovider = \"anthropic\"\nmodel = \"m\"\n\
             [task.reader]\nkind = \"summarize\"\nupstream = \"u\"\n\
             [[hook.after_tool.shell]]\nname = \"route-all\"\n\
             action = \"route\"\ntask = \"reader\"\n",
        );
        let event = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "make build"},
            "tool_response": {"stdout": "x\n".repeat(400), "stderr": ""},
            "duration_ms": 10
        });
        assert!(run_with(&event, &test_config(), &pipeline).is_none());
    }

    #[test]
    fn test_never_worse_guard_hint_overhead_emits_nothing() {
        let tee_dir = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.retriever.tee_directory = Some(tee_dir.path().to_path_buf());

        let pairs: Vec<String> = (0..40).map(|i| format!("\"k{i}\":{i}")).collect();
        let almost_min = format!("{{\n{}}}", pairs.join(",\n"));
        let minified_len = serde_json::to_string(
            &serde_json::from_str::<serde_json::Value>(&almost_min).expect("valid"),
        )
        .expect("serialize")
        .len();
        assert!(
            almost_min.len() - minified_len < 60,
            "premise: savings smaller than hint length"
        );

        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/almost.json"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/almost.json",
                "content": almost_min,
                "numLines": 42, "startLine": 1, "totalLines": 42
            }},
            "duration_ms": 1
        });
        assert!(
            run_with(&event, &config, &json_reads_pipeline()).is_none(),
            "composed (filtered+hint) > raw must emit nothing"
        );
    }

    #[test]
    fn test_grep_count_mode_not_rewritten() {
        assert!(
            run_posthook(&fixture("grep_count")).is_none(),
            "count mode is compact already"
        );
    }

    #[test]
    fn test_webfetch_markdown_passes_through() {
        assert!(run_posthook(&fixture("webfetch_md")).is_none());
    }

    #[test]
    fn test_websearch_passes_through_when_chain_noop() {
        assert!(run_posthook(&fixture("websearch")).is_none());
    }

    #[test]
    fn test_glob_unsupported_format_passes_through() {
        assert!(run_posthook(&fixture("glob")).is_none());
    }

    #[test]
    fn test_malformed_event_emits_nothing() {
        assert!(run_posthook(&serde_json::json!({})).is_none());
        assert!(run_posthook(&serde_json::json!({"tool_name": 42})).is_none());
        assert!(
            run_posthook(
                &serde_json::json!({"tool_name": "Read", "tool_response": "not an object"})
            )
            .is_none()
        );
    }

    #[test]
    fn test_unknown_tool_shape_emits_nothing() {
        let event = serde_json::json!({
            "tool_name": "Bash",
            "tool_response": {"output": "x".repeat(2000)}
        });
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_override_disables_the_term_rule() {
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.term-default]\nenabled = false\n",
        );
        assert!(run_with(&bash_fixture("repetitive"), &test_config(), &pipeline).is_none());
    }

    #[test]
    fn test_shadow_rule_records_but_emits_nothing() {
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.term-default]\nshadow = true\n",
        );
        assert!(run_with(&bash_fixture("repetitive"), &test_config(), &pipeline).is_none());
    }

    #[test]
    fn test_gate_min_size() {
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/small.json"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/small.json",
                "content": "{\"a\": 1}",
                "numLines": 1, "startLine": 1, "totalLines": 1
            }}
        });
        assert!(run_with(&event, &test_config(), &json_reads_pipeline()).is_none());
    }

    #[test]
    fn test_gate_min_size_from_guards() {
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.json-reads]\nenabled = true\n[guards]\nmin_size = 100000\n",
        );
        assert!(
            run_with(&fixture("read_flat"), &test_config(), &pipeline).is_none(),
            "raised min_size must gate the read"
        );
    }

    #[test]
    fn test_read_non_json_tracked_passthrough_no_output() {
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/code.rs"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/code.rs",
                "content": "fn main() {}\n".repeat(100),
                "numLines": 100, "startLine": 1, "totalLines": 100
            }}
        });
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_strip_line_numbers_rule_fires_when_enabled() {
        let numbered: String = (1..=60)
            .map(|i| format!("{i}\u{2192}let value_{i} = compute({i});"))
            .collect::<Vec<_>>()
            .join("\n");
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/code.rs"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/code.rs",
                "content": numbered,
                "numLines": 60, "startLine": 1, "totalLines": 60
            }}
        });
        assert!(
            run_posthook(&event).is_none(),
            "strip-line-numbers ships off"
        );
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.strip-line-numbers]\nenabled = true\n",
        );
        let out = run_with(&event, &test_config(), &pipeline)
            .expect("enabled strip-line-numbers must fire");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let content = v["hookSpecificOutput"]["updatedToolOutput"]["file"]["content"]
            .as_str()
            .unwrap();
        assert!(content.starts_with("let value_1"));
        assert!(!content.contains('\u{2192}'));
    }

    #[test]
    fn test_read_cargo_lock_summarized_with_note() {
        let content = include_str!("../../Cargo.lock");
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/repo/Cargo.lock"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/repo/Cargo.lock",
                "content": content,
                "numLines": content.lines().count(),
                "startLine": 1,
                "totalLines": content.lines().count()
            }},
            "duration_ms": 3
        });
        let out = run_posthook(&event).expect("lockfile must summarize");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let hook = &v["hookSpecificOutput"];
        let new_content = hook["updatedToolOutput"]["file"]["content"]
            .as_str()
            .unwrap();
        assert!(new_content.starts_with("Cargo.lock: "), "summary header");
        assert!(new_content.contains("serde@"));
        assert!(
            hook["additionalContext"].as_str().is_some(),
            "Read conversion note"
        );
    }

    fn bash_fixture(name: &str) -> Value {
        let raw = match name {
            "ansi" => include_str!("../../tests/fixtures/posthook/bash_ansi_output.json"),
            "progress" => include_str!("../../tests/fixtures/posthook/bash_progress_cr.json"),
            "repetitive" => {
                include_str!("../../tests/fixtures/posthook/bash_repetitive_lines.json")
            }
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(raw).expect("fixture parses")
    }

    #[test]
    fn test_bash_ansi_output_stripped_and_rewrapped() {
        let out = run_posthook(&bash_fixture("ansi")).expect("ansi output must be filtered");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let response = &v["hookSpecificOutput"]["updatedToolOutput"];
        let stdout = response["stdout"].as_str().unwrap();
        assert!(!stdout.contains('\x1b'), "ANSI gone");
        assert!(stdout.contains("PASS test_0"), "content kept");
        assert_eq!(response["interrupted"], false, "metadata preserved");
        assert_eq!(response["isImage"], false);
        assert!(
            v["hookSpecificOutput"].get("additionalContext").is_none(),
            "no conversion note for the generic floor"
        );
    }

    #[test]
    fn test_bash_progress_frames_collapsed() {
        let out = run_posthook(&bash_fixture("progress")).expect("progress output filtered");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let stdout = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        assert!(stdout.contains("Downloading 100% done"));
        assert!(!stdout.contains("Downloading 0%"));
    }

    #[test]
    fn test_bash_repetitive_lines_deduped() {
        let out = run_posthook(&bash_fixture("repetitive")).expect("repetitive output filtered");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let stdout = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        assert!(stdout.contains("[x40]"), "explicit dedup marker: {stdout}");
    }

    #[test]
    fn test_bash_rtk_rewritten_command_skipped() {
        let mut event = bash_fixture("repetitive");
        event["tool_input"]["command"] = json!("rtk cargo test");
        assert!(run_posthook(&event).is_none());

        event["tool_input"]["command"] = json!("git add . && rtk cargo test");
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_bash_recall_recovery_command_skipped() {
        let mut event = bash_fixture("repetitive");
        event["tool_input"]["command"] =
            json!("cat \"$HOME/.local/share/rtk/tee/posthook/123_1-0_bash_x.log\"");
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_bash_interrupted_skipped() {
        let mut event = bash_fixture("repetitive");
        event["tool_response"]["interrupted"] = json!(true);
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_bash_stderr_filtered_independently() {
        let mut event = bash_fixture("ansi");
        event["tool_response"]["stderr"] = json!("\x1b[31merror line\x1b[0m\n".repeat(10));
        let out = run_posthook(&event).expect("stderr must be filtered too");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let stderr = v["hookSpecificOutput"]["updatedToolOutput"]["stderr"]
            .as_str()
            .unwrap();
        assert!(!stderr.contains('\x1b'));
        assert!(stderr.contains("[x10]"));
    }

    #[test]
    fn test_run_never_panics_on_garbage() {
        run(&serde_json::json!(null));
        run(&serde_json::json!("string event"));
        run(&serde_json::json!({"tool_name": "Read"}));
    }

    #[test]
    fn test_recall_hint_ordering_never_pushes_above_raw() {
        let raw = format!(
            "[{}]",
            (0..200)
                .map(|i| format!("{i}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/nums.json"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/nums.json",
                "content": raw,
                "numLines": 1, "startLine": 1, "totalLines": 1
            }}
        });
        assert!(run_with(&event, &test_config(), &json_reads_pipeline()).is_none());
    }

    #[test]
    fn applied_carries_the_resolved_pipeline_name_for_attribution() {
        let pipeline =
            pipeline_from("[pipeline]\nname = \"shared-pipe\"\nextends = \"rtk:posthook\"\n");
        let raw = "\x1b[32mcompiling module ok\x1b[0m\n".repeat(200);
        let applied = run_selected(
            &pipeline,
            Subject::Shell,
            "Bash",
            ContentFormat::Term,
            "",
            &raw,
            0,
            Some(false),
            None,
        )
        .expect("term chain applies");
        assert_eq!(applied.pipeline, "shared-pipe");
    }

    #[test]
    fn test_log_entry_never_contains_tool_output() {
        let sentinel = "SENTINEL-9f2c-THE-TOOL-OUTPUT";
        let applied = Applied {
            text: format!("filtered {sentinel} filtered"),
            methods: vec!["ansi", "cap"],
            rules: vec!["term-default".to_string()],
            pipeline: "rtk:posthook".to_string(),
            subject: Subject::Shell,
            shadow: false,
        };
        let entry = log_entry_for(
            Subject::Shell,
            Some("cargo test"),
            "",
            &applied,
            estimate_tokens(&applied.text),
            10,
        );
        let line = serde_json::to_string(&entry).expect("serializes");
        assert!(
            !line.contains(sentinel),
            "the decision log records decisions, never content: {line}"
        );
        assert!(line.contains("term-default"));
        assert!(line.contains("cargo test"));
    }

    #[test]
    fn test_source_and_helpers() {
        assert_eq!(extension_of("/a/b/data.json"), "json");
        assert_eq!(extension_of("https://docs.rs/toon"), "");
        assert_eq!(basename_of("/a/b/data.json"), "data.json");
        assert_eq!(basename_of("https://docs.rs/toon-format"), "docs.rs");
    }
}
