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
    if crate::core::user_env::var("RTK_DISABLED").as_deref() == Some("1") {
        return None;
    }
    if crate::core::user_env::var("RTK_POSTHOOK").as_deref() == Some("0") {
        return None;
    }

    let config = Config::load().unwrap_or_default();
    let loaded = pipeline_config::load();
    let output = process_with_config(event, &config, &loaded.resolved)?;
    Some((output, loaded))
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
) -> Option<Applied> {
    let mut facts = facts_for(subject, format, source, raw, duration_ms, is_rtk_command);
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
) -> Option<String> {
    let applied = run_selected(
        pipeline,
        subject,
        tool_name,
        format,
        source,
        raw,
        duration_ms,
        None,
    )?;
    let filtered = applied.text.clone();

    if filtered == raw {
        // No-op chain: skip the recall store entirely.
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
        return None;
    }

    // Ordering invariant (emit_guarded pattern): compose filtered + hint
    // FIRST, then guard — the hint can never push output above raw.
    let slug = format!("{}_{}", tool_name.to_lowercase(), basename_of(source));
    let hint = tee::posthook_tee_hint(config, raw, &slug);
    let composed = match hint {
        Some(h) => format!("{filtered}\n{h}"),
        None => filtered,
    };

    if estimate_tokens(&composed) > estimate_tokens(raw) {
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
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
    let rtk_cmd = format!("rtk posthook {} {}", tool_name.to_lowercase(), format_token);
    let original_cmd = if source.is_empty() {
        tool_name.to_string()
    } else {
        format!("{tool_name} {source}")
    };
    if let Ok(tracker) = tracker() {
        let _ = tracker.record(
            &original_cmd,
            &rtk_cmd,
            estimate_tokens(raw),
            estimate_tokens(emitted),
            duration_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
