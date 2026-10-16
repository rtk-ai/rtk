//! Task execution: `rtk <task> ...` delegates work to a cheaper model
//! The engine owns the kinds (`summarize`, `generate`, `digest`)
//! and the
//! prompt assembly; this module owns file I/O, the provider call, and the
//! net accounting. The files never enter the agent's context — that is the
//! saving.

use crate::core::arg_tokenizer::{Dialect, Flag, Grammar, TokenKind, ValueSpec, tokenize_grammar};
use crate::core::pipeline_config;
use crate::core::tracking::{Tracker, estimate_tokens};
use anyhow::{Context, Result, bail};
use rtk_engine::pipeline::{TaskDef, UpstreamDef};
use rtk_engine::task::{CorpusFile, TaskIntent, TaskOutcome, WorkerRequest, find_kind};

pub struct ResolvedTask {
    pub task: TaskDef,
    pub upstream: Option<UpstreamDef>,
}

pub fn tasks_unavailable() -> bool {
    pipeline_config::any_user_pipeline_files() && pipeline_config::load().fell_back
}

pub fn resolve_task(name: &str) -> Option<ResolvedTask> {
    if !pipeline_config::any_user_pipeline_files() {
        return None;
    }
    let loaded = pipeline_config::load();
    if loaded.fell_back {
        return None;
    }
    let (task, _) = loaded.resolved.tasks.get(name)?.clone();
    let upstream = task
        .upstream
        .as_deref()
        .and_then(|u| loaded.resolved.upstreams.get(u))
        .map(|(def, _)| def.clone());
    Some(ResolvedTask { task, upstream })
}

pub fn run_tasks_list() -> Result<i32> {
    let loaded = pipeline_config::load();
    if loaded.resolved.tasks.is_empty() {
        println!(
            "no tasks configured — adopt one with `extends = \"rtk:delegate-reads\"` and bind an upstream"
        );
        for kind in rtk_engine::task::kinds() {
            println!("  kind {:<10} {}", kind.name(), kind.usage());
        }
        return Ok(0);
    }
    println!("{:<16} {:<11} {:<14} usage", "task", "kind", "upstream");
    for (name, (task, _)) in &loaded.resolved.tasks {
        let usage = find_kind(&task.kind)
            .map(|k| k.usage().replace("<task>", name))
            .unwrap_or_default();
        println!(
            "{:<16} {:<11} {:<14} {}",
            name,
            task.kind,
            task.upstream.as_deref().unwrap_or("(none — inert)"),
            usage
        );
    }
    Ok(0)
}

struct TaskArgs {
    intent: TaskIntent,
    reference: Option<String>,
    paths: Vec<String>,
    command: Option<Vec<String>>,
}

/// `rtk task`'s own options, each taking a value.
const TASK_FLAGS: &[Flag] = &[
    Flag::long("question").takes(ValueSpec::value()),
    Flag::long("spec").takes(ValueSpec::value()),
    Flag::long("target").takes(ValueSpec::value()),
    Flag::long("reference").takes(ValueSpec::value()),
];

/// Posix: a literal `--` ends option parsing, and everything after it is the
/// command, verbatim.
const TASK_GRAMMAR: Grammar = Grammar::new(Dialect::Posix, &[TASK_FLAGS]);

fn parse_args(args: &[String]) -> Result<TaskArgs> {
    // Classify through the tokenizer rather than a literal `arg == "--flag"`
    // scan (house rule), so the attached form `--question=why` works, not only
    // the separate `--question why`.
    let tokens = tokenize_grammar(args, &TASK_GRAMMAR);

    let mut parsed = TaskArgs {
        intent: TaskIntent::default(),
        reference: None,
        paths: Vec::new(),
        command: None,
    };

    let dash_dash = tokens.iter().position(|t| t.kind == TokenKind::DashDash);
    for (i, tok) in tokens.iter().enumerate() {
        // Everything at or past the `--` boundary is the command, collected below.
        if let Some(pos) = dash_dash
            && i >= pos
        {
            break;
        }
        match tok.kind {
            TokenKind::Long => {
                let value = || {
                    tok.value(&tokens)
                        .map(str::to_string)
                        .with_context(|| format!("--{} needs a value", tok.text))
                };
                match tok.text {
                    "question" => parsed.intent.question = Some(value()?),
                    "spec" => parsed.intent.spec = Some(value()?),
                    "target" => parsed.intent.target = Some(value()?),
                    "reference" => parsed.reference = Some(value()?),
                    other => bail!("unknown flag --{other}"),
                }
            }
            TokenKind::Positional if tok.is_free_positional() => {
                parsed.paths.push(tok.text.to_string());
            }
            // A Short token (unexpected here) or a value positional consumed by
            // a flag above; nothing to do.
            _ => {}
        }
    }

    if let Some(pos) = dash_dash {
        let command: Vec<String> = tokens[pos + 1..]
            .iter()
            .filter(|t| t.kind == TokenKind::Positional)
            .map(|t| t.text.to_string())
            .collect();
        if command.is_empty() {
            bail!("`--` needs a command after it");
        }
        parsed.command = Some(command);
    }

    Ok(parsed)
}

/// Run a command and capture its output as the corpus a `digest` task reads.
/// Returns the command's exit code alongside it: a routed `npm test` must
/// still fail, or routing would hide the result it was asked about.
fn run_for_output(command: &[String]) -> Result<(CorpusFile, i32)> {
    let (program, rest) = command
        .split_first()
        .context("`--` needs a command after it")?;
    let output = std::process::Command::new(program)
        .args(rest)
        .output()
        .with_context(|| format!("Failed to run {program}"))?;
    let mut content = String::new();
    content.push_str(&String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&stderr);
    }
    if let Some(code) = output.status.code() {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("[exit status {code}]"));
    }
    Ok((
        CorpusFile {
            path: command.join(" "),
            content,
        },
        output.status.code().unwrap_or(1),
    ))
}

fn read_corpus(paths: &[String]) -> Result<Vec<CorpusFile>> {
    paths
        .iter()
        .map(|path| {
            let meta = std::fs::metadata(path).with_context(|| format!("Failed to read {path}"))?;
            if !meta.is_file() {
                anyhow::bail!("{path} is not a regular file");
            }
            let content =
                std::fs::read_to_string(path).with_context(|| format!("Failed to read {path}"))?;
            Ok(CorpusFile {
                path: path.clone(),
                content,
            })
        })
        .collect()
}

pub fn run_task(name: &str, args: &[String]) -> Result<i32> {
    let Some(resolved) = resolve_task(name) else {
        bail!("`{name}` is not a configured task");
    };
    let Some(kind) = find_kind(&resolved.task.kind) else {
        bail!(
            "task `{name}` has unknown kind `{}` — run `rtk pipeline validate`",
            resolved.task.kind
        );
    };
    let Some(upstream) = &resolved.upstream else {
        bail!(
            "task `{name}` has no upstream — bind one in pipeline.toml:\n\
             [upstream.flash]\nprovider = \"google\"\nmodel = \"gemini-2.5-flash\"\n\
             api_key_env = \"GEMINI_API_KEY\"\n\n[task.{name}]\nupstream = \"flash\""
        );
    };

    let parsed = parse_args(args)?;
    let mut command_status = 0;
    let corpus = match resolved.task.kind.as_str() {
        "generate" => {
            if parsed.intent.target.is_none() {
                bail!("generate needs --target <file>");
            }
            read_corpus(std::slice::from_ref(
                parsed
                    .reference
                    .as_ref()
                    .context("generate refuses to run without --reference <file>")?,
            ))?
        }
        "digest" => match &parsed.command {
            Some(command) => {
                let (captured, code) = run_for_output(command)?;
                command_status = code;
                vec![captured]
            }
            None => read_corpus(&parsed.paths)?,
        },
        _ => read_corpus(&parsed.paths)?,
    };

    let request = kind
        .assemble(
            &parsed.intent,
            &corpus,
            resolved.task.instructions.as_deref(),
            resolved.task.prompt.as_deref(),
        )
        .map_err(|e| anyhow::anyhow!("{e}\nusage: {}", kind.usage().replace("<task>", name)))?;

    let (output, worker_in, worker_out) =
        match call_worker(upstream, &request, resolved.task.temperature) {
            Ok(answer) => answer,
            Err(e) if parsed.command.is_some() => {
                eprintln!("rtk: `{name}` worker failed ({e:#}) — showing the command's own output");
                if let Some(captured) = corpus.first() {
                    print!("{}", captured.content);
                    if !captured.content.ends_with('\n') {
                        println!();
                    }
                }
                return Ok(command_status);
            }
            Err(e) => return Err(e),
        };

    // Context-reduction accounting: `tokens_in` is what the corpus WOULD have
    // put into the agent's context; `tokens_out` is the summary that actually
    // did. Their difference is the context the delegation kept out — a pure,
    // user-agnostic token ratio. The worker's own spend (`worker_in +
    // worker_out`) is a cost, captured proxy-side on the Routed line, and is
    // deliberately NOT mixed in here: netting cheap worker tokens against
    // expensive context tokens would be a category error.
    let corpus_tokens: usize = corpus.iter().map(|f| estimate_tokens(&f.content)).sum();
    let summary_tokens = estimate_tokens(&output);
    let _ = (worker_in, worker_out);
    if let Ok(tracker) = Tracker::new() {
        let _ = tracker.record_attribution("task", "task", name, corpus_tokens, summary_tokens);
    }

    match kind.handle(&output) {
        TaskOutcome::Print(text) => {
            println!("{text}");
        }
        TaskOutcome::WriteFile(content) => {
            let target = parsed
                .intent
                .target
                .as_deref()
                .context("generate needs --target <file>")?;
            std::fs::write(target, &content)
                .with_context(|| format!("Failed to write {target}"))?;
            println!(
                "wrote {target} ({} lines) from {}",
                content.lines().count(),
                corpus
                    .first()
                    .map(|f| f.path.as_str())
                    .unwrap_or("reference")
            );
        }
    }
    Ok(command_status)
}

/// The environment variable a provider's key conventionally lives in, so an
/// upstream that uses the obvious one needs no `api_key_env` at all.
fn default_key_env(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        "google" => Some("GEMINI_API_KEY"),
        _ => None,
    }
}

/// Resolve the worker's credential.
///
/// `api_key_command` first, for a key that should not sit in the environment —
/// a secrets manager, a keychain read, the same shape as Claude Code's
/// `apiKeyHelper`. Otherwise the named environment variable, or the provider's
/// conventional one when the upstream does not name it.
fn api_key(upstream: &UpstreamDef) -> Result<String> {
    if let Some(command) = upstream.api_key_command.as_deref() {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .with_context(|| format!("running api_key_command: {command}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("api_key_command failed: {}", err.trim());
        }
        let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if key.is_empty() {
            anyhow::bail!("api_key_command printed nothing: {command}");
        }
        return Ok(key);
    }
    let var = upstream
        .api_key_env
        .as_deref()
        .or_else(|| default_key_env(&upstream.provider))
        .context(
            "upstream has no credential — set `api_key_env`, or `api_key_command` for a key \
             that should not sit in the environment",
        )?;
    std::env::var(var).with_context(|| format!("environment variable {var} is not set"))
}

fn timeout(upstream: &UpstreamDef) -> std::time::Duration {
    std::time::Duration::from_secs(upstream.timeout_secs.unwrap_or(30))
}

/// One blocking call to the worker model. Returns (text, tokens_in, tokens_out).
fn call_worker(
    upstream: &UpstreamDef,
    request: &WorkerRequest,
    temperature: Option<f64>,
) -> Result<(String, usize, usize)> {
    match upstream.provider.as_str() {
        "anthropic" => call_anthropic(upstream, request, temperature),
        "openai" => call_openai(upstream, request, temperature),
        "google" => call_google(upstream, request, temperature),
        other => bail!("unknown provider `{other}` (one of: anthropic, openai, google)"),
    }
}

fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    limit: std::time::Duration,
) -> Result<serde_json::Value> {
    let mut req = ureq::post(url)
        .timeout(limit)
        .set("Content-Type", "application/json");
    for (key, value) in headers {
        req = req.set(key, value);
    }
    let response = req.send_string(&body.to_string()).map_err(|e| match e {
        ureq::Error::Status(code, response) => {
            let detail = response.into_string().unwrap_or_default();
            anyhow::anyhow!("worker call failed ({code}): {detail}")
        }
        other => anyhow::anyhow!("worker call failed: {other}"),
    })?;
    let text = response
        .into_string()
        .context("worker response read failed")?;
    serde_json::from_str(&text).context("worker response was not JSON")
}

/// The dialects the local daemon speaks. A `google` worker has no route
/// through it, so it keeps going direct and keeps needing its own key.
const PROXY_SPEAKS: &[&str] = &["anthropic", "openai"];

/// Whether a base URL is the machine itself. A worker pointed at loopback is
/// talking to the daemon, whether a pipeline spelled the address out or we
/// chose it, and either way the credential can be borrowed.
fn is_loopback_base(base: &str) -> bool {
    ["127.0.0.1", "localhost", "[::1]", "0.0.0.0"]
        .iter()
        .any(|host| base.contains(host))
}

/// The worker's endpoint, and whether it is the local daemon.
///
/// An explicit `base_url` always wins. Otherwise the running daemon is
/// preferred over the provider: it already relays the credential the agent
/// authenticated with, so routing through it is what makes delegation need no
/// key of its own. With no daemon up, the provider default stands and a key is
/// required as before.
fn worker_base(upstream: &UpstreamDef, provider_default: &str) -> (String, bool) {
    if let Some(explicit) = upstream.base_url.as_deref() {
        return (explicit.to_string(), is_loopback_base(explicit));
    }
    if PROXY_SPEAKS.contains(&upstream.provider.as_str()) && crate::llm_proxy::is_reachable() {
        return (format!("http://{}", crate::llm_proxy::daemon_bind()), true);
    }
    (provider_default.to_string(), false)
}

/// The worker's credential, which may legitimately be absent.
///
/// `None` only when the call goes through the local daemon, which lends the
/// credential the agent is already sending. Anywhere else a missing key stays
/// an error: the request would be rejected, and failing here says why.
fn borrowable_api_key(upstream: &UpstreamDef, via_daemon: bool) -> Result<Option<String>> {
    match api_key(upstream) {
        Ok(key) => Ok(Some(key)),
        Err(_) if via_daemon => Ok(None),
        Err(e) => Err(e),
    }
}

/// The model this delegation displaced — what would have read the content if
/// the work had not been handed to a worker.
///
/// Sent to the proxy as `x-rtk-routed-from` so the counterfactual prices the
/// request against that model rather than the cheap one that served it.
/// Without it a delegated read looks like pure added spend: the worker call is
/// real traffic, while the expensive tokens it avoided were never requested and
/// so are nowhere in the ledger.
///
/// Read from the caller's own session transcript, newest first. Fail-open: no
/// transcript, no header, and the pricing simply stays as it was.
fn displaced_model() -> Option<String> {
    let cwd = crate::core::user_dirs::current_dir().ok()?;
    let dir = claude_project_dir(&cwd)?;
    let mut sessions: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .filter_map(|p| {
            let seen = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((seen, p))
        })
        .collect();
    sessions.sort_by_key(|(seen, _)| std::cmp::Reverse(*seen));
    sessions
        .into_iter()
        .find_map(|(_, path)| last_assistant_model(&path))
}

/// Claude Code's transcript directory for a working directory: the absolute
/// path with every non-alphanumeric run turned into `-`.
fn claude_project_dir(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let slug: String = cwd
        .to_str()?
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let dir = crate::core::user_dirs::home()?
        .join(".claude")
        .join("projects")
        .join(slug);
    dir.is_dir().then_some(dir)
}

/// The last model named by an assistant turn in a transcript.
fn last_assistant_model(path: &std::path::Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;
    if std::fs::metadata(path).ok()?.len() > MAX_TRANSCRIPT_BYTES {
        return None;
    }
    let reader = BufReader::new(std::fs::File::open(path).ok()?);
    let mut found = None;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(model) = value
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty() && *m != "<synthetic>")
        {
            found = Some(model.to_string());
        }
    }
    found
}

fn call_anthropic(
    upstream: &UpstreamDef,
    request: &WorkerRequest,
    temperature: Option<f64>,
) -> Result<(String, usize, usize)> {
    let (base, via_daemon) = worker_base(upstream, "https://api.anthropic.com");
    let key = borrowable_api_key(upstream, via_daemon)?;
    let mut body = serde_json::json!({
        "model": upstream.model,
        "max_tokens": request.max_tokens,
        "system": request.system,
        "messages": [{"role": "user", "content": request.user}],
    });
    if let Some(t) = temperature {
        body["temperature"] = serde_json::json!(t);
    }
    let mut headers: Vec<(&str, &str)> = vec![("anthropic-version", "2023-06-01")];
    if let Some(key) = key.as_deref() {
        headers.push(("x-api-key", key));
    }
    // Only the daemon reads this, and only it can price the counterfactual.
    let displaced = via_daemon.then(displaced_model).flatten();
    if let Some(from) = displaced.as_deref() {
        headers.push(("x-rtk-routed-from", from));
    }
    let response = post_json(
        &format!("{base}/v1/messages"),
        &headers,
        &body,
        timeout(upstream),
    )?;
    let text = response["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let tokens_in = response["usage"]["input_tokens"].as_u64().unwrap_or(0) as usize;
    let tokens_out = response["usage"]["output_tokens"].as_u64().unwrap_or(0) as usize;
    Ok((text, tokens_in, tokens_out))
}

fn call_openai(
    upstream: &UpstreamDef,
    request: &WorkerRequest,
    temperature: Option<f64>,
) -> Result<(String, usize, usize)> {
    let (base, via_daemon) = worker_base(upstream, "https://api.openai.com");
    let key = borrowable_api_key(upstream, via_daemon)?;
    let mut body = serde_json::json!({
        "model": upstream.model,
        "messages": [
            {"role": "system", "content": request.system},
            {"role": "user", "content": request.user},
        ],
        "max_completion_tokens": request.max_tokens,
    });
    if let Some(t) = temperature {
        body["temperature"] = serde_json::json!(t);
    }
    let auth = key.as_deref().map(|k| format!("Bearer {k}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(auth) = auth.as_deref() {
        headers.push(("Authorization", auth));
    }
    let displaced = via_daemon.then(displaced_model).flatten();
    if let Some(from) = displaced.as_deref() {
        headers.push(("x-rtk-routed-from", from));
    }
    let response = post_json(
        &format!("{base}/v1/chat/completions"),
        &headers,
        &body,
        timeout(upstream),
    )?;
    let text = response["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let tokens_in = response["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as usize;
    let tokens_out = response["usage"]["completion_tokens"].as_u64().unwrap_or(0) as usize;
    Ok((text, tokens_in, tokens_out))
}

fn call_google(
    upstream: &UpstreamDef,
    request: &WorkerRequest,
    temperature: Option<f64>,
) -> Result<(String, usize, usize)> {
    let base = upstream
        .base_url
        .as_deref()
        .unwrap_or("https://generativelanguage.googleapis.com");
    let key = api_key(upstream)?;
    let mut generation = serde_json::json!({"maxOutputTokens": request.max_tokens});
    if let Some(t) = temperature {
        generation["temperature"] = serde_json::json!(t);
    }
    let body = serde_json::json!({
        "system_instruction": {"parts": [{"text": request.system}]},
        "contents": [{"role": "user", "parts": [{"text": request.user}]}],
        "generationConfig": generation,
    });
    let response = post_json(
        &format!("{base}/v1beta/models/{}:generateContent", upstream.model),
        &[("x-goog-api-key", &key)],
        &body,
        timeout(upstream),
    )?;
    let text = response["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let tokens_in = response["usageMetadata"]["promptTokenCount"]
        .as_u64()
        .unwrap_or(0) as usize;
    let tokens_out = response["usageMetadata"]["candidatesTokenCount"]
        .as_u64()
        .unwrap_or(0) as usize;
    Ok((text, tokens_in, tokens_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::arg_tokenizer::assert_takes_value_table;

    #[test]
    fn task_grammar_matches_its_table() {
        assert_takes_value_table(
            &TASK_GRAMMAR,
            &[(
                TokenKind::Long,
                &["question", "spec", "target", "reference"],
                Some(ValueSpec::value()),
            )],
        );
    }

    #[test]
    fn args_parse_flags_and_paths() {
        let parsed = parse_args(&[
            "--question".to_string(),
            "how does auth work?".to_string(),
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
        ])
        .expect("parses");
        assert_eq!(
            parsed.intent.question.as_deref(),
            Some("how does auth work?")
        );
        assert_eq!(parsed.paths, vec!["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn generate_flags_parse() {
        let parsed = parse_args(&[
            "--spec".to_string(),
            "a controller".to_string(),
            "--reference".to_string(),
            "src/ref.rs".to_string(),
            "--target".to_string(),
            "src/new.rs".to_string(),
        ])
        .expect("parses");
        assert_eq!(parsed.reference.as_deref(), Some("src/ref.rs"));
        assert_eq!(parsed.intent.target.as_deref(), Some("src/new.rs"));
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert!(parse_args(&["--quesiton".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(&["--question".to_string()]).is_err());
    }
}

#[cfg(test)]
mod corpus_tests {
    use super::read_corpus;

    #[test]
    fn read_corpus_refuses_non_regular_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = read_corpus(&[dir.path().display().to_string()])
            .expect_err("a directory must be refused");
        assert!(err.to_string().contains("not a regular file"), "{err}");
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_double_dash_captures_the_rest_as_a_command() {
        let parsed = parse_args(&args(&[
            "--question",
            "why",
            "--",
            "npm",
            "test",
            "--silent",
        ]))
        .expect("parses");
        assert_eq!(parsed.intent.question.as_deref(), Some("why"));
        assert_eq!(
            parsed.command.as_deref(),
            Some(&args(&["npm", "test", "--silent"])[..])
        );
        assert!(parsed.paths.is_empty());
    }

    #[test]
    fn flags_after_the_double_dash_belong_to_the_command() {
        let parsed = parse_args(&args(&["--", "ls", "--color=never"])).expect("parses");
        assert_eq!(
            parsed.command.as_deref(),
            Some(&args(&["ls", "--color=never"])[..])
        );
    }

    #[test]
    fn a_bare_double_dash_is_refused() {
        assert!(parse_args(&args(&["--question", "q", "--"])).is_err());
    }

    #[test]
    fn without_a_double_dash_paths_still_parse() {
        let parsed = parse_args(&args(&["--question", "q", "a.rs", "b.rs"])).expect("parses");
        assert!(parsed.command.is_none());
        assert_eq!(parsed.paths, args(&["a.rs", "b.rs"]));
    }

    #[test]
    fn captured_output_carries_stdout_stderr_and_exit_status() {
        let (captured, code) =
            run_for_output(&args(&["sh", "-c", "echo out; echo err 1>&2; exit 3"])).expect("runs");
        assert_eq!(code, 3);
        assert!(captured.content.contains("out"));
        assert!(captured.content.contains("err"));
        assert!(captured.content.contains("[exit status 3]"));
        assert_eq!(captured.path, "sh -c echo out; echo err 1>&2; exit 3");
    }

    #[test]
    fn a_successful_command_reports_zero() {
        let (_, code) = run_for_output(&args(&["sh", "-c", "exit 0"])).expect("runs");
        assert_eq!(code, 0);
    }
}

/// Answer for a tool's output with a `digest` task, for a `route` rule at
/// `hook.after_tool.*`. Every failure returns `None` so the caller emits the
/// original output: a worker that is slow, down or misconfigured must never
/// cost the agent its result. Never prints — the hook owns stdout.
pub fn digest_output(task_name: &str, output: &str, label: &str) -> Option<String> {
    let resolved = resolve_task(task_name)?;
    if resolved.task.kind != "digest" {
        return None;
    }
    let corpus = vec![CorpusFile {
        path: label.to_string(),
        content: output.to_string(),
    }];
    answer_with(task_name, &resolved, &corpus, None)
}

pub const IN_HOOK_QUESTION: &str = "What in this does the agent need for its current step? Keep exact names, numbers, paths, line numbers and error lines.";

pub fn context_question(context: &crate::core::call_context::CallContext) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(goal) = &context.goal {
        parts.push(format!("The user asked: {goal}"));
    }
    if let Some(intent) = &context.intent {
        parts.push(format!("The agent's current step: {intent}"));
    }
    if !context.recent_calls.is_empty() {
        parts.push(format!(
            "Its recent tool calls: {}",
            context.recent_calls.join(" | ")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    parts.push(IN_HOOK_QUESTION.to_string());
    Some(parts.join("\n"))
}

fn task_question(resolved: &ResolvedTask) -> Option<String> {
    if !resolved.task.context {
        return None;
    }
    context_question(crate::core::call_context::get())
}

fn answer_with(
    task_name: &str,
    resolved: &ResolvedTask,
    corpus: &[CorpusFile],
    fallback_question: Option<&str>,
) -> Option<String> {
    let kind = find_kind(&resolved.task.kind)?;
    let upstream = resolved.upstream.as_ref()?;
    let intent = TaskIntent {
        question: task_question(resolved).or_else(|| fallback_question.map(str::to_string)),
        ..TaskIntent::default()
    };
    let request = kind
        .assemble(
            &intent,
            corpus,
            resolved.task.instructions.as_deref(),
            resolved.task.prompt.as_deref(),
        )
        .ok()?;
    let (text, worker_in, worker_out) =
        call_worker(upstream, &request, resolved.task.temperature).ok()?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let corpus_tokens: usize = corpus.iter().map(|f| estimate_tokens(&f.content)).sum();
    let _ = (worker_in, worker_out);
    if let Ok(tracker) = Tracker::new() {
        let _ = tracker.record_attribution(
            "task",
            "task",
            task_name,
            corpus_tokens,
            estimate_tokens(&text),
        );
    }
    Some(text)
}

/// `mode = "answer"` on a `route` rule: run the task inside the hook and hand
/// back its answer, so the agent reads it in the same turn. `None` on any
/// failure, which lets the original tool run.
pub fn answer_call(task_name: &str, facts: &rtk_engine::pipeline::Facts) -> Option<String> {
    use rtk_engine::pipeline::Fact;
    let text = |key: &str| match facts.get(key) {
        Some(Fact::Str(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    };
    let resolved = resolve_task(task_name)?;
    let (corpus, target) = match text("tool").as_deref() {
        Some("read") => {
            let path = text("file_path")?;
            (read_corpus(std::slice::from_ref(&path)).ok()?, path)
        }
        Some("search") => {
            let pattern = text("input.pattern")?;
            let path = text("input.path").unwrap_or_else(|| ".".to_string());
            let command = vec!["rg".to_string(), "-n".to_string(), pattern.clone(), path];
            let (captured, _) = run_for_output(&command).ok()?;
            (vec![captured], format!("the search for `{pattern}`"))
        }
        Some("web") => {
            let url = text("url").or_else(|| text("input.url"))?;
            let command = vec!["curl".to_string(), "-sL".to_string(), url.clone()];
            let (captured, code) = run_for_output(&command).ok()?;
            if code != 0 {
                return None;
            }
            (vec![captured], url)
        }
        _ => return None,
    };
    let answer = answer_with(task_name, &resolved, &corpus, Some(IN_HOOK_QUESTION))?;
    Some(format!(
        "A worker model answered for {target} instead of running this call. Its answer, for your current step:\n\n{answer}"
    ))
}

/// `mode = "swap"` on a `route` rule before a shell command: the command the
/// agent runs instead — the task, wrapping the original command.
pub fn swap_command(task_name: &str, command: &str) -> Option<String> {
    let resolved = resolve_task(task_name)?;
    let question = task_question(&resolved).unwrap_or_else(|| IN_HOOK_QUESTION.to_string());
    Some(format!(
        "rtk {task_name} --question {} -- sh -c {}",
        sh_single_quote(&question),
        sh_single_quote(command)
    ))
}

fn sh_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod in_hook_tests {
    use super::*;
    use crate::core::call_context::CallContext;

    #[test]
    fn context_question_carries_goal_intent_and_calls() {
        let context = CallContext {
            intent: Some("find why the build fails".to_string()),
            goal: Some("fix the CI".to_string()),
            recent_calls: vec!["shell mvn package".to_string()],
        };
        let question = context_question(&context).expect("a question");
        assert!(question.contains("The user asked: fix the CI"));
        assert!(question.contains("current step: find why the build fails"));
        assert!(question.contains("shell mvn package"));
        assert!(question.ends_with(IN_HOOK_QUESTION));
    }

    #[test]
    fn no_context_gives_no_question() {
        assert_eq!(context_question(&CallContext::default()), None);
    }

    #[test]
    fn single_quotes_survive_the_shell() {
        let quoted = sh_single_quote("it's a | b");
        assert_eq!(quoted, "'it'\\''s a | b'");
        let output = std::process::Command::new("sh")
            .args(["-c", &format!("printf %s {quoted}")])
            .output()
            .expect("sh runs");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "it's a | b");
    }
}

#[cfg(test)]
mod arg_forms {
    use super::*;

    /// parse_args classifies through arg_tokenizer rather than comparing
    /// `arg == "--question"` literally, so the attached-value form
    /// `--question=X` and (for flags with one) short forms work.
    #[test]
    fn attached_long_value_forms_work() {
        let args: Vec<String> = vec!["--question=why".into(), "--target=src/x.rs".into()];
        let parsed = parse_args(&args).expect("parses");
        assert_eq!(parsed.intent.question.as_deref(), Some("why"));
        assert_eq!(parsed.intent.target.as_deref(), Some("src/x.rs"));
    }

    #[test]
    fn separate_value_form_still_works() {
        let args: Vec<String> = vec!["--question".into(), "why".into()];
        let parsed = parse_args(&args).expect("parses");
        assert_eq!(parsed.intent.question.as_deref(), Some("why"));
    }

    #[test]
    fn trailing_command_after_dashdash_is_preserved() {
        let args: Vec<String> = vec![
            "--question=x".into(),
            "--".into(),
            "npm".into(),
            "test".into(),
        ];
        let parsed = parse_args(&args).expect("parses");
        assert_eq!(parsed.intent.question.as_deref(), Some("x"));
        assert_eq!(
            parsed.command.as_deref(),
            Some(&["npm".to_string(), "test".to_string()][..])
        );
    }
}

/// Delegation through the local daemon, which lends the credential the agent
/// already relays so a worker needs none of its own.
#[cfg(test)]
mod borrowed_credential {
    use super::*;

    fn upstream(provider: &str, base: Option<&str>) -> UpstreamDef {
        UpstreamDef {
            provider: provider.to_string(),
            base_url: base.map(String::from),
            ..UpstreamDef::default()
        }
    }

    #[test]
    fn loopback_is_recognised_however_it_is_spelled() {
        assert!(is_loopback_base("http://127.0.0.1:8080"));
        assert!(is_loopback_base("http://localhost:8080"));
        assert!(is_loopback_base("http://[::1]:8080"));
        assert!(!is_loopback_base("https://api.anthropic.com"));
        assert!(!is_loopback_base("https://gateway.corp.example.com"));
    }

    #[test]
    fn an_explicit_base_url_wins_over_the_daemon() {
        let (base, via_daemon) = worker_base(
            &upstream("anthropic", Some("https://gateway.corp.example.com")),
            "https://api.anthropic.com",
        );
        assert_eq!(base, "https://gateway.corp.example.com");
        assert!(!via_daemon, "a remote gateway lends nothing");
    }

    #[test]
    fn an_explicit_loopback_base_still_borrows() {
        let (base, via_daemon) = worker_base(
            &upstream("anthropic", Some("http://127.0.0.1:8080")),
            "https://api.anthropic.com",
        );
        assert_eq!(base, "http://127.0.0.1:8080");
        assert!(
            via_daemon,
            "spelling the daemon out by hand must work like choosing it"
        );
    }

    #[test]
    fn a_provider_the_daemon_cannot_speak_goes_direct() {
        let (base, via_daemon) = worker_base(
            &upstream("google", None),
            "https://generativelanguage.googleapis.com",
        );
        assert_eq!(base, "https://generativelanguage.googleapis.com");
        assert!(!via_daemon, "no route through the daemon, so no borrowing");
    }

    #[test]
    fn a_missing_key_is_an_error_only_when_going_direct() {
        let no_key = UpstreamDef {
            provider: "anthropic".to_string(),
            api_key_env: Some("RTK_TEST_KEY_THAT_IS_NOT_SET".to_string()),
            ..UpstreamDef::default()
        };
        assert!(
            borrowable_api_key(&no_key, false).is_err(),
            "going direct without a key would just be rejected upstream"
        );
        assert_eq!(
            borrowable_api_key(&no_key, true).expect("borrowing tolerates no key"),
            None
        );
    }
}
