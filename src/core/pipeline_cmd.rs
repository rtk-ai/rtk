//! `rtk pipeline` and `rtk methods` command surface over the engine.

use crate::core::pipeline_config::{self, LoadedPipeline};
use anyhow::{Context, Result};
use regex::Regex;
use rtk_engine::block::{Event, Subject};
use rtk_engine::pipeline::{
    BUILTIN_NAMES, DecisionKind, Fact, Facts, Position, Severity, ValidateCtx, builtin_text,
    evaluate, has_errors,
};
use std::sync::LazyLock;

pub(crate) fn load_for_cli() -> LoadedPipeline {
    pipeline_config::load_with_ctx(&ValidateCtx {
        proxy_running: Some(crate::llm_proxy::is_reachable()),
        reserved_commands: pipeline_config::RESERVED_COMMAND_NAMES,
        is_reserved_command: Some(pipeline_config::is_reserved_command_name),
        ..ValidateCtx::default()
    })
}

pub fn run_show(name: Option<&str>) -> Result<i32> {
    if let Some(name) = name {
        let Some(text) = builtin_text(name) else {
            println!(
                "unknown built-in `{name}` — built-ins: {}",
                BUILTIN_NAMES.join(", ")
            );
            return Ok(1);
        };
        print!("{text}");
        return Ok(0);
    }

    let loaded = load_for_cli();
    let pipeline = &loaded.resolved;
    println!("Pipeline: {}", pipeline.name);
    println!("Inherits: {}", pipeline.extends.join(" -> "));
    if !loaded.has_user_files {
        println!("(no pipeline file present — this is the compiled rtk:default)");
    }
    if loaded.fell_back {
        println!("(fell back to the built-in default — see diagnostics below)");
    }
    for file in &loaded.dropped {
        println!(
            "(not loaded, has errors: {file} — the other files are active; see diagnostics below)"
        );
    }
    println!();

    println!("Rules (per position; first match wins, project before user before inherited):");
    for (position, rules) in &pipeline.rules {
        println!("  {}", position.render());
        for rule in rules {
            let what = match rule.def.action.as_deref() {
                Some(action) => {
                    let mut out = format!("action={action}");
                    if let Some(task) = &rule.def.task {
                        out.push_str(&format!(" task={task}"));
                    }
                    out
                }
                None => format!("use=[{}]", rule.def.use_.join(", ")),
            };
            let when = if rule.def.when.is_empty() {
                "always".to_string()
            } else {
                rule.def
                    .when
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let mut flags = Vec::new();
            // A proxy-resident method has no implementation at a hook site, so
            // the rule fires and runs nothing. Say so where the user reads
            // their pipeline, not only in `validate`.
            let inert_here = position.site != rtk_engine::pipeline::SiteKind::Proxy
                && rule
                    .def
                    .use_
                    .iter()
                    .all(|m| !rtk_engine::pipeline::proxy_members(pipeline, m).is_empty());
            if !rule.def.use_.is_empty() && inert_here {
                flags.push("inert: proxy-only");
            }
            if !rule.def.use_.is_empty()
                && position.site == rtk_engine::pipeline::SiteKind::Hook
                && position.event == Event::AfterTool
                && !rtk_engine::pipeline::after_tool_extractable(position.subject)
            {
                flags.push("inert: no extraction");
            }
            if !rule.def.enabled {
                flags.push("disabled");
            }
            if rule.def.shadow {
                flags.push("shadow");
            }
            if rule.def.continue_ {
                flags.push("continue");
            }
            let overridden = if rule.overridden_by.is_empty() {
                String::new()
            } else {
                format!("  [overridden by {}]", rule.overridden_by.join(", "))
            };
            println!(
                "    {:<22} [{}] when {} -> {}{}{}",
                rule.name,
                rule.origin,
                when,
                what,
                if flags.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", flags.join(", "))
                },
                overridden
            );
        }
    }
    println!();

    if !pipeline.directives.is_empty() {
        println!("Directives (merged in order):");
        for directive in &pipeline.directives {
            let mut line = format!("  {:<22} [{}]", directive.name, directive.origin);
            if !directive.def.enabled {
                line.push_str(" (disabled)");
            }
            if let Some(level) = directive.def.options.get("level").and_then(|v| v.as_str()) {
                line.push_str(&format!(" level={level}"));
            }
            println!("{line}");
            if let Some(text) = pipeline_config::directive_text(directive) {
                for text_line in text.lines() {
                    println!("      {text_line}");
                }
            } else if let Some(file) = &directive.def.file {
                println!("      [file not readable: {file}]");
            }
        }
        println!();
    }

    if !pipeline.tasks.is_empty() {
        println!("Tasks:");
        for (name, (task, origin)) in &pipeline.tasks {
            println!(
                "  {:<22} kind={} upstream={} [{}]",
                name,
                task.kind,
                task.upstream.as_deref().unwrap_or("(none — inert)"),
                origin
            );
        }
        println!();
    }

    println!(
        "Guards: never_worse={} min_size={}",
        pipeline.guards.never_worse, pipeline.guards.min_size
    );
    println!();

    print_diagnostics(&loaded);
    Ok(0)
}

pub fn run_validate() -> Result<i32> {
    let loaded = load_for_cli();
    print_diagnostics(&loaded);
    if has_errors(&loaded.diagnostics) {
        Ok(1)
    } else {
        println!("pipeline: ok ({})", loaded.resolved.name);
        print_auth_guidance(&loaded.resolved);
        print_hook_gaps();
        Ok(0)
    }
}

pub fn print_hook_gaps() {
    for gap in crate::hooks::init::claude_hook_gaps() {
        println!(
            "hook needed: {gap} — your pipeline has rules there; run `rtk init -g` so Claude Code sends those tool calls to rtk"
        );
    }
}

/// The pipeline resolved, but can it authenticate? Every upstream credential
/// the pipeline reads is checked against the live environment and each
/// missing one is printed with the exact export to run — adopting a pipeline
/// that needs auth must say so, not fail silently at first use.
pub fn print_auth_guidance(resolved: &rtk_engine::pipeline::Resolved) {
    for (env, upstream, users) in resolved.required_env() {
        if std::env::var(&env).is_ok_and(|v| !v.is_empty()) {
            continue;
        }
        println!(
            "auth needed: export {env}=...   (upstream `{upstream}` — {})",
            users.join(", ")
        );
    }
}

fn read_source(source: &str) -> Result<String> {
    if source == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("read pipeline from stdin")?;
        return Ok(buf);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        let url = gist_raw_url(source);
        // GET of a URL the user typed on the CLI; inbound content only.
        // nosemgrep: ureq-outside-telemetry
        return ureq::get(&url)
            .call()
            .context("fetch pipeline")?
            .into_string()
            .context("read fetched pipeline");
    }
    std::fs::read_to_string(source).with_context(|| format!("read {source}"))
}

fn gist_raw_url(source: &str) -> String {
    if let Some(rest) = source.strip_prefix("https://gist.github.com/") {
        let rest = rest.trim_end_matches('/');
        if !rest.is_empty() && !rest.contains("/raw") {
            return format!("https://gist.githubusercontent.com/{rest}/raw");
        }
    }
    source.to_string()
}

fn extract_toml(raw: &str) -> String {
    let mut lines = raw.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("```") && line.contains("toml") {
            let mut body = String::new();
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    return body;
                }
                body.push_str(inner);
                body.push('\n');
            }
            return body;
        }
    }
    raw.to_string()
}

fn valid_pipeline_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_builtin_method(name: &str) -> bool {
    rtk_engine::registry::METHODS
        .iter()
        .any(|m| m.name() == name)
        || rtk_engine::registry::chain(name).is_some()
        || rtk_engine::registry::proxy_info(name).is_some()
}

fn referenced_plugin_methods(
    resolved: &rtk_engine::pipeline::Resolved,
) -> (Vec<String>, Vec<String>) {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let consider = |m: &str, names: &mut std::collections::BTreeSet<String>| {
        if !is_builtin_method(m) && !resolved.methods.contains_key(m) {
            names.insert(m.to_string());
        }
    };
    for rules in resolved.rules.values() {
        for rule in rules {
            if rule.def.pro_inert {
                continue;
            }
            for m in &rule.def.use_ {
                consider(m, &mut names);
            }
        }
    }
    for (def, _) in resolved.methods.values() {
        if let Some(chain) = &def.chain {
            for m in chain {
                consider(m, &mut names);
            }
        }
    }
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for n in names {
        if rtk_engine::registry::find(&n).is_some() {
            present.push(n);
        } else {
            missing.push(n);
        }
    }
    (present, missing)
}

static SECRET_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        ("sk- API key", Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap()),
        (
            "GitHub token",
            Regex::new(r"gh[pousr]_[A-Za-z0-9]{20,}").unwrap(),
        ),
        (
            "GitHub PAT",
            Regex::new(r"github_pat_[A-Za-z0-9_]{20,}").unwrap(),
        ),
        ("AWS access key", Regex::new(r"AKIA[0-9A-Z]{16}").unwrap()),
        (
            "Google API key",
            Regex::new(r"AIza[0-9A-Za-z_\-]{20,}").unwrap(),
        ),
        (
            "Slack token",
            Regex::new(r"xox[baprs]-[0-9A-Za-z-]{10,}").unwrap(),
        ),
        (
            "private key block",
            Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
        ),
    ]
});

static HOME_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"/(Users|home)/[^/\s"']+"#).unwrap());
static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap());

fn mask(s: &str) -> String {
    let n = s.chars().count();
    if n <= 8 {
        return "\u{2022}".repeat(n);
    }
    let head: String = s.chars().take(4).collect();
    let tail: String = s.chars().skip(n - 2).collect();
    format!("{head}\u{2026}{tail}")
}

pub(crate) fn scan_for_secrets(sources: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (_, text) in sources {
        for (label, re) in SECRET_PATTERNS.iter() {
            if let Some(m) = re.find(text) {
                out.push(format!("{label}: {}", mask(m.as_str())));
            }
        }
        if let Some(m) = HOME_PATH_RE.find(text) {
            out.push(format!("absolute home path: {}", m.as_str()));
        }
        if let Some(m) = EMAIL_RE.find(text) {
            out.push(format!("email address: {}", m.as_str()));
        }
    }
    out.sort();
    out.dedup();
    out
}

pub(crate) fn gh_available() -> bool {
    std::process::Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Word-wrapped yellow panel in the cards' own box language — the one visual
/// voice every security warning speaks in, impossible to miss in a scrollback.
pub(crate) fn warning_panel_text(title: &str, lines: &[&str]) -> String {
    use colored::Colorize;
    const W: usize = 74;
    let wrap = |text: &str| -> Vec<String> {
        if text.is_empty() {
            return vec![String::new()];
        }
        let mut out = Vec::new();
        let mut line = String::new();
        for word in text.split_whitespace() {
            let next = if line.is_empty() {
                word.chars().count()
            } else {
                line.chars().count() + 1 + word.chars().count()
            };
            if next > W - 3 && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            out.push(line);
        }
        out
    };
    let mut out = String::new();
    let t = format!(" {title} ");
    let fill = W.saturating_sub(t.chars().count() + 2);
    out.push_str(&format!(
        "{}\n",
        format!("┌─{t}{}┐", "─".repeat(fill)).yellow().bold()
    ));
    for raw in lines {
        for line in wrap(raw) {
            let pad = W.saturating_sub(line.chars().count() + 3);
            out.push_str(&format!(
                "{} {}{} {}\n",
                "│".yellow().bold(),
                line.yellow(),
                " ".repeat(pad),
                "│".yellow().bold()
            ));
        }
    }
    out.push_str(&format!(
        "{}\n",
        format!("└{}┘", "─".repeat(W - 1)).yellow().bold()
    ));
    out
}

pub(crate) fn warning_panel(title: &str, lines: &[&str]) {
    print!("{}", warning_panel_text(title, lines));
}

pub(crate) fn warning_panel_err(title: &str, lines: &[&str]) {
    eprint!("{}", warning_panel_text(title, lines));
}

pub(crate) fn print_publish_warning() {
    eprintln!();
    warning_panel_err(
        "Publishing is public, permanent, and yours",
        &[
            "This makes the pipeline PUBLIC, permanent, and indexable.",
            "You are responsible for its contents — RTK does not review or endorse shared pipelines.",
            "Remove any API keys, tokens, absolute paths, or private data before sharing.",
        ],
    );
    eprintln!();
}

pub(crate) fn print_publish_warning_private() {
    eprintln!();
    warning_panel_err(
        "Private, but still published",
        &[
            "This creates a PRIVATE repo: only accounts you grant access can see it.",
            "Private is one visibility toggle away from public, and anyone with access can copy everything — still remove API keys, tokens, and private data.",
            "You are responsible for its contents. RTK does not review or endorse shared pipelines.",
        ],
    );
    eprintln!();
}

pub(crate) fn publish_gist(
    name: &str,
    sources: &[(String, String)],
    card_md: &str,
    force: bool,
) -> Result<i32> {
    print_publish_warning();
    if crate::core::pipeline_remote::interactive() {
        if !crate::core::pipeline_remote::prompt("I understand — continue? [y/N] ")?
            .eq_ignore_ascii_case("y")
        {
            println!("not published");
            return Ok(1);
        }
    } else if !force {
        anyhow::bail!(
            "publishing non-interactively needs --force — the warning above is yours to accept"
        );
    }

    let findings = scan_for_secrets(sources);
    if !findings.is_empty() {
        let mut lines: Vec<String> = vec![
            "The pre-flight scan flagged content that may be sensitive:".to_string(),
            String::new(),
        ];
        lines.extend(findings.iter().map(|f| format!("  - {f}")));
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        warning_panel_err("Possible secrets or private data", &refs);
        if !force {
            eprintln!();
            eprintln!("Not published. Remove the above, or re-run with --force to publish anyway.");
            return Ok(1);
        }
        eprintln!("--force given \u{2014} publishing despite the above.");
        eprintln!();
    }

    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let file = std::env::temp_dir().join(format!("{safe}.md"));
    std::fs::write(&file, card_md).with_context(|| format!("write {}", file.display()))?;

    if !gh_available() {
        println!("The `gh` CLI is not available, so rtk cannot create the gist for you.");
        println!("Publish manually:");
        println!("  1. open https://gist.github.com");
        println!("  2. create a PUBLIC gist from: {}", file.display());
        println!(
            "  3. share the gist URL \u{2014} others install with: rtk pipeline add <gist-url>"
        );
        return Ok(0);
    }

    let desc =
        format!("{name} \u{2014} rtk compression pipeline (install: rtk pipeline add <this url>)");
    let output = std::process::Command::new("gh")
        .args(["gist", "create"])
        .arg(&file)
        .args(["--public", "--desc", &desc])
        .output()
        .context("run gh gist create")?;
    if !output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        eprintln!("gh could not create the gist \u{2014} is `gh auth login` done?");
        return Ok(1);
    }
    let page_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    crate::core::pipeline_remote::record_publish(name, &page_url);
    println!();
    println!("published \u{2192} {page_url}");
    println!("others install it with:  rtk pipeline add {page_url}");
    Ok(0)
}

pub fn run_add(
    source: &str,
    name_override: Option<&str>,
    force: bool,
    shadow: bool,
) -> Result<i32> {
    let (raw, pin) = match crate::core::pipeline_remote::parse_github_source(source) {
        Some(src) => {
            let fetched = crate::core::pipeline_remote::fetch_github_pipeline(&src)?;
            (fetched.toml, Some((fetched.canonical, fetched.sha)))
        }
        None => {
            let raw = read_source(source)?;
            let origin = (source.starts_with("http://") || source.starts_with("https://"))
                .then(|| (source.to_string(), None));
            (raw, origin)
        }
    };
    let toml = extract_toml(&raw);
    if toml.trim().is_empty() {
        anyhow::bail!("the source contains no pipeline TOML — nothing to install");
    }

    let name = name_override
        .map(str::to_string)
        .or_else(|| pipeline_config::declared_name(&toml, source))
        .or_else(|| {
            (source != "-")
                .then(|| std::path::Path::new(source).file_stem())
                .flatten()
                .map(|s| s.to_string_lossy().to_string())
        });
    let Some(name) = name else {
        anyhow::bail!("no `[pipeline] name` — give the pipeline a name with `--name <name>`");
    };
    if !valid_pipeline_name(&name) {
        anyhow::bail!("pipeline name `{name}` must be alphanumeric with `-` or `_`");
    }

    let commands = pipeline_config::credential_commands(&toml, &name);
    if !commands.is_empty() {
        let mut lines: Vec<String> = vec![
            "This pipeline runs shell commands to obtain credentials:".to_string(),
            String::new(),
        ];
        lines.extend(commands.iter().map(|(upstream, command)| {
            format!(
                "  upstream `{upstream}`: {}",
                crate::core::utils::sanitize_for_terminal(command)
            )
        }));
        lines.push(String::new());
        lines.push(
            "rtk pipeline add never installs a pipeline that runs commands. Ask the author to use api_key_env instead, or, if you wrote this file yourself and trust the command, place it directly in your pipelines folder."
                .to_string(),
        );
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        warning_panel_err("Refused: api_key_command", &refs);
        return Ok(1);
    }
    let loaded = pipeline_config::load_candidate(&toml, &name);
    print_diagnostics(&loaded);
    let (present_plugins, missing_plugins) = referenced_plugin_methods(&loaded.resolved);
    if has_errors(&loaded.diagnostics) {
        if !missing_plugins.is_empty() {
            eprintln!();
            warning_panel_err(
                "This pipeline needs plugins you do not have",
                &[
                    &format!(
                        "Methods your rtk does not have: {}",
                        missing_plugins.join(", ")
                    ),
                    "",
                    "Those come from PLUGINS — third-party native code compiled into rtk, not sandboxed TOML. To run this pipeline you would have to obtain each plugin's source from its author, add it to your build, and rebuild rtk.",
                    "",
                    "Install a plugin only if you trust its author and have read its source — you alone are responsible for any native code you build and run.",
                ],
            );
        }
        eprintln!("not installed — fix the errors above");
        return Ok(1);
    }

    println!();
    warning_panel(
        "You are installing a third-party pipeline",
        &[
            "You are responsible for what it does: it can rewrite the commands your agent runs, filter and replace what it reads, inject instructions, and send content to any upstream endpoint it declares.",
            "",
            "Review what this one does:",
        ],
    );
    println!();
    match pipeline_config::trust_summary(&toml) {
        Ok(lines) => {
            for line in &lines {
                for part in line.lines() {
                    if part.trim_start().starts_with("[warn]") {
                        use colored::Colorize;
                        println!("    {}", part.yellow());
                    } else {
                        println!("    {part}");
                    }
                }
            }
        }
        Err(e) => println!("    (summary unavailable: {e})"),
    }
    if !present_plugins.is_empty() {
        println!(
            "    PLUGIN methods (third-party native code already in your rtk): {}",
            present_plugins.join(", ")
        );
        println!("    you alone are responsible for any plugin you build and run.");
    }
    if let Some((src, sha)) = &pin {
        match sha {
            Some(sha) => println!(
                "    source: {src} · pinned at commit {}",
                crate::core::pipeline_remote::short_sha(sha)
            ),
            None => println!("    source: {src} · UNPINNED (commit could not be resolved)"),
        }
        println!(
            "    any numbers on its card are self-reported by the author — rtk verifies nothing."
        );
    }
    println!();
    let dir = pipeline_config::user_pipelines_dir().context("no config directory")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let dest = dir.join(format!("{name}.toml"));
    let existing = dest
        .exists()
        .then(|| std::fs::read_to_string(&dest).unwrap_or_default());

    if let Some(old) = &existing {
        if *old == toml {
            println!("`{name}` is already installed and identical — nothing to change");
            if let Some((src, sha)) = &pin {
                crate::core::pipeline_remote::record_install(&name, src, sha.as_deref());
            }
            return Ok(0);
        }
        let mut lines = vec![format!(
            "`{name}` is already installed — what you install REPLACES the live version."
        )];
        if let (Some(origin), Some((_, Some(new_sha)))) = (
            crate::core::pipeline_remote::install_origin(&name),
            pin.as_ref().map(|(s, sha)| (s, sha.as_ref())),
        ) && let Some(old_sha) = &origin.sha
        {
            lines.push(format!(
                "pinned {} -> {}",
                crate::core::pipeline_remote::short_sha(old_sha),
                crate::core::pipeline_remote::short_sha(new_sha)
            ));
        }
        lines.push(String::new());
        lines.push(
            "You are responsible for what you pull and use — review the change below.".to_string(),
        );
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        warning_panel("Updating an installed pipeline", &refs);
        print!(
            "{}",
            crate::core::utils::sanitize_for_terminal(&crate::core::pipeline_remote::line_diff(
                old, &toml
            ))
        );
        println!();
    }

    match choose_install_mode(existing.is_some(), force, shadow)? {
        InstallMode::Quit => {
            println!("nothing installed");
            return Ok(1);
        }
        InstallMode::Shadow => {
            let sdir = pipeline_config::shadow_pipelines_dir().context("no config directory")?;
            std::fs::create_dir_all(&sdir).with_context(|| format!("create {}", sdir.display()))?;
            let sdest = sdir.join(format!("{name}.toml"));
            std::fs::write(&sdest, &toml).with_context(|| format!("write {}", sdest.display()))?;
            pipeline_config::set_eval_candidate(Some(&name)).context("write eval state")?;
            if let Some((src, sha)) = &pin {
                crate::core::pipeline_remote::record_install(&name, src, sha.as_deref());
            }
            println!("trying `{name}` silently — measured on your sessions, applied to nothing.");
            if existing.is_some() {
                println!("your currently installed `{name}` stays live and untouched.");
            }
            println!("  measure:   rtk pipeline eval {name}      (after a few sessions)");
            println!("  go live:   rtk pipeline promote {name}");
            println!("  discard:   rtk pipeline promote {name} --discard");
            return Ok(0);
        }
        InstallMode::Live => {}
    }
    std::fs::write(&dest, &toml).with_context(|| format!("write {}", dest.display()))?;
    if let Some((src, sha)) = &pin {
        crate::core::pipeline_remote::record_install(&name, src, sha.as_deref());
    }

    println!("installed: {}", dest.display());
    print_auth_guidance(&loaded.resolved);
    let wants_proxy = loaded.resolved.extends.iter().any(|e| e == "rtk:proxy")
        || loaded
            .resolved
            .rules
            .iter()
            .filter(|(p, _)| p.site == rtk_engine::pipeline::SiteKind::Proxy)
            .flat_map(|(_, rules)| rules)
            .any(|r| !BUILTIN_NAMES.contains(&r.origin.as_str()));
    if wants_proxy {
        println!("this pipeline has proxy rules — start the daemon: rtk llm-proxy start");
    }
    let has_after_tool = loaded
        .resolved
        .rules
        .keys()
        .any(|p| p.site == rtk_engine::pipeline::SiteKind::Hook && p.event == Event::AfterTool);
    let has_directives = loaded.resolved.directives.iter().any(|d| d.def.enabled);
    if has_after_tool || has_directives {
        let what = match (has_after_tool, has_directives) {
            (true, true) => "output compression and directives",
            (true, false) => "output compression",
            _ => "directives",
        };
        println!(
            "this pipeline uses {what} — if you haven't enabled that before, run `rtk init -g` once, then restart your agent session"
        );
    }
    println!("active now, merged with your other pipelines — see `rtk pipeline show`");
    Ok(0)
}

enum InstallMode {
    Live,
    Shadow,
    Quit,
}

fn choose_install_mode(updating: bool, force: bool, shadow: bool) -> Result<InstallMode> {
    if shadow {
        return Ok(InstallMode::Shadow);
    }
    if force {
        return Ok(InstallMode::Live);
    }
    if !crate::core::pipeline_remote::interactive() {
        if updating {
            anyhow::bail!(
                "refusing to replace an installed pipeline non-interactively — pass --force \
                 to replace it, or --shadow to try the new version silently first"
            );
        }
        anyhow::bail!(
            "installing non-interactively needs an explicit opt-in — pass --force to accept \
             the disclosure above, or --shadow to try it without applying anything"
        );
    }
    let (items, default): (&[(&str, &str)], usize) = if updating {
        (
            &[
                ("Replace the live version", "the change shown above applies"),
                (
                    "Try the new version silently",
                    "shadow — measured on your sessions, changes nothing",
                ),
                ("Keep the current version", "nothing installed"),
            ],
            2,
        )
    } else {
        (
            &[
                (
                    "Install now",
                    "active immediately, merged with your pipelines",
                ),
                (
                    "Try it silently first",
                    "shadow — measured on your sessions, changes nothing",
                ),
                ("Quit", "nothing installed"),
            ],
            0,
        )
    };
    Ok(
        match crate::core::pipeline_remote::select_menu(items, default)? {
            Some(0) => InstallMode::Live,
            Some(1) => InstallMode::Shadow,
            _ => InstallMode::Quit,
        },
    )
}

/// `rtk pipeline promote <name>` — move a shadow install live (or discard
/// it). The shadow slot is the only thing this touches; promoting an update
/// replaces the live file after the same review an install gets.
pub fn run_promote(name: &str, discard: bool, force: bool) -> Result<i32> {
    if !valid_pipeline_name(name) {
        anyhow::bail!("pipeline name `{name}` must be alphanumeric with `-` or `_`");
    }
    let sdir = pipeline_config::shadow_pipelines_dir().context("no config directory")?;
    let spath = sdir.join(format!("{name}.toml"));
    if !spath.is_file() {
        anyhow::bail!(
            "no shadow install of `{name}` — shadow installs come from `rtk pipeline add` \
             choosing `try silently` (or --shadow)"
        );
    }
    let clear_candidate = || {
        if pipeline_config::eval_candidate().as_deref() == Some(name) {
            let _ = pipeline_config::set_eval_candidate(None);
        }
    };
    if discard {
        std::fs::remove_file(&spath).with_context(|| format!("remove {}", spath.display()))?;
        clear_candidate();
        println!("discarded the shadow install of `{name}` — nothing was ever applied");
        return Ok(0);
    }
    let text =
        std::fs::read_to_string(&spath).with_context(|| format!("read {}", spath.display()))?;
    let loaded = pipeline_config::load_candidate(&text, name);
    print_diagnostics(&loaded);
    if has_errors(&loaded.diagnostics) {
        anyhow::bail!("`{name}` does not validate — fix it (or --discard) before promoting");
    }
    let dir = pipeline_config::user_pipelines_dir().context("no config directory")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let dest = dir.join(format!("{name}.toml"));
    if let Ok(old) = std::fs::read_to_string(&dest)
        && old != text
    {
        println!("promoting replaces the live `{name}` — the change:");
        print!(
            "{}",
            crate::core::utils::sanitize_for_terminal(&crate::core::pipeline_remote::line_diff(
                &old, &text
            ))
        );
    }
    warning_panel(
        &format!("Promoting makes `{name}` active"),
        &[
            "It will rewrite, filter, and act on your live sessions.",
            "You are responsible for what you run.",
        ],
    );
    if !force {
        if !crate::core::pipeline_remote::interactive() {
            anyhow::bail!("refusing to promote non-interactively — pass --force");
        }
        if !crate::core::pipeline_remote::prompt("promote? [y/N] ")?.eq_ignore_ascii_case("y") {
            println!("kept in shadow — nothing changed");
            return Ok(1);
        }
    }
    std::fs::rename(&spath, &dest)
        .with_context(|| format!("move {} to {}", spath.display(), dest.display()))?;
    clear_candidate();
    println!("`{name}` is live — see `rtk pipeline show`");
    print_auth_guidance(&loaded.resolved);
    Ok(0)
}

pub fn run_eval(name: &str, start: bool, stop: bool) -> Result<i32> {
    if stop {
        pipeline_config::set_eval_candidate(None).context("clear eval state")?;
        println!("stopped shadow-evaluating `{name}`");
        return Ok(0);
    }
    if start {
        let dir = pipeline_config::user_pipelines_dir().context("no config directory")?;
        let shadow = pipeline_config::shadow_pipelines_dir()
            .map(|d| d.join(format!("{name}.toml")))
            .filter(|p| p.is_file());
        let path = shadow.unwrap_or_else(|| dir.join(format!("{name}.toml")));
        if !path.is_file() {
            anyhow::bail!(
                "no installed pipeline `{name}` at {} — install it first: rtk pipeline add <source> --name {name}",
                path.display()
            );
        }
        let text = std::fs::read_to_string(&path)?;
        let loaded = pipeline_config::load_candidate(&text, name);
        print_diagnostics(&loaded);
        if has_errors(&loaded.diagnostics) {
            anyhow::bail!("`{name}` does not validate — fix it before evaluating");
        }
        pipeline_config::set_eval_candidate(Some(name)).context("write eval state")?;
        println!("shadow-evaluating `{name}` on your live sessions — it changes nothing.");
        println!("classifier/route effects are excluded (deterministic transforms only).");
        println!("after a few sessions: rtk pipeline eval {name}");
        return Ok(0);
    }

    let tracker = crate::core::tracking::Tracker::new().context("open tracking database")?;
    let candidate = tracker
        .attribution_site("shadow", "pipeline", 30)?
        .into_iter()
        .find(|r| r.name == name);
    let active: Vec<_> = tracker.attribution_site("hook", "pipeline", 30)?;
    let armed = pipeline_config::eval_candidate().as_deref() == Some(name);

    println!("Pipeline eval — `{name}` · last 30 days · vs raw, same trajectory");
    println!("{}", "─".repeat(60));
    match candidate {
        Some(row) if row.tokens_in > 0 => {
            let pct = row.saved().max(0) as f64 / row.tokens_in as f64 * 100.0;
            println!(
                "  candidate      {} → {} tokens  ({pct:.0}% removed · {} outputs)   est.",
                crate::core::utils::format_tokens(row.tokens_in.max(0) as usize),
                crate::core::utils::format_tokens(row.tokens_out.max(0) as usize),
                row.calls,
            );
        }
        _ => {
            println!("  candidate      no measurements yet");
            if !armed {
                println!("  start with: rtk pipeline eval {name} --start");
            }
            println!();
            return Ok(0);
        }
    }
    let (a_in, a_out, a_calls) = active.iter().fold((0i64, 0i64, 0i64), |(i, o, c), r| {
        (i + r.tokens_in, o + r.tokens_out, c + r.calls)
    });
    if a_in > 0 {
        let pct = (a_in - a_out).max(0) as f64 / a_in as f64 * 100.0;
        println!(
            "  active now     {} → {} tokens  ({pct:.0}% removed · {} outputs)   est.",
            crate::core::utils::format_tokens(a_in.max(0) as usize),
            crate::core::utils::format_tokens(a_out.max(0) as usize),
            a_calls,
        );
    }
    println!("  ~ tokens est. ≈ bytes/4; both measured on the same live outputs");
    println!("  ~ classifier/route effects excluded from the candidate");
    if armed {
        println!("  (still collecting — stop with: rtk pipeline eval {name} --stop)");
    }
    println!();
    Ok(0)
}

pub fn run_test() -> Result<i32> {
    let paths = pipeline_config::load_paths();
    if paths.is_empty() {
        println!("no pipeline files present — the compiled rtk:default is active");
        println!("(custom filter tests in filters.toml run under `rtk verify`)");
        return Ok(0);
    }
    for (scope, path) in &paths {
        println!("{} ({:?})", path.display(), scope);
    }
    let loaded = load_for_cli();
    print_diagnostics(&loaded);
    if has_errors(&loaded.diagnostics) {
        println!("pipeline files have errors — hooks fall back to rtk:default");
        Ok(1)
    } else {
        println!(
            "resolved: {} ({} positions with rules)",
            loaded.resolved.name,
            loaded.resolved.rules.len()
        );
        Ok(0)
    }
}

/// Everything a share card is made of, owned — the terminal card, the
/// markdown card, and the publisher all draw from one collection pass.
pub(crate) struct CardData {
    pub display: String,
    pub is_active: bool,
    pub sources: Vec<(String, String)>,
    what: Vec<(String, String, String)>,
    filtering: Option<crate::core::tracking::GainSummary>,
    hook: Option<crate::core::tracking::AttributionRow>,
    slices: Vec<(String, i64, i64, i64)>,
    tasks: Vec<crate::core::tracking::AttributionRow>,
    proxy: Option<crate::llm_proxy::ProxyPipelineRow>,
    published: Option<String>,
    date: String,
}

impl CardData {
    fn stats(&self) -> CardStats<'_> {
        CardStats {
            what: &self.what,
            filtering: self.filtering.as_ref(),
            hook: self.hook.as_ref(),
            hook_slices: &self.slices,
            tasks: &self.tasks,
            proxy: self.proxy.as_ref(),
        }
    }

    pub fn card_md(&self) -> String {
        share_card(
            &self.display,
            &self.sources,
            &self.stats(),
            env!("CARGO_PKG_VERSION"),
            &self.date,
        )
    }

    pub fn print_terminal(&self) {
        print_terminal_card(
            &self.display,
            &self.stats(),
            self.published.as_deref(),
            env!("CARGO_PKG_VERSION"),
            &self.date,
        );
    }

    /// The one number worth putting in a repo description, if any lever has
    /// measured activity — the same figures the card shows, nothing new.
    pub fn headline(&self) -> Option<String> {
        if let Some(f) = self
            .filtering
            .as_ref()
            .filter(|f| f.total_commands > 0 && f.avg_savings_pct >= 1.0)
        {
            return Some(format!(
                "{:.0}% of command output removed",
                f.avg_savings_pct
            ));
        }
        if let Some(h) = self.hook.as_ref().filter(|r| r.tokens_in > 0) {
            let pct = h.saved().max(0) as f64 / h.tokens_in as f64 * 100.0;
            if pct >= 1.0 {
                return Some(format!("{pct:.0}% of tool output removed"));
            }
        }
        if let Some(p) = self.proxy.as_ref() {
            let prefix = p.cache_read + p.cache_created;
            if prefix > 0 {
                return Some(format!(
                    "{:.0}% prompt-cache hit",
                    p.cache_read as f64 / prefix as f64 * 100.0
                ));
            }
        }
        None
    }
}

/// One collection pass for a share card; `None` means the name was unknown
/// (the message is already printed).
pub(crate) fn collect_card(name: Option<&str>) -> Result<Option<CardData>> {
    let loaded = load_for_cli();
    let active = loaded.resolved.name.clone();
    let (display, sources) = match name {
        Some(n) => {
            if let Some(text) = builtin_text(n) {
                (
                    n.to_string(),
                    vec![(format!("{n} (built-in)"), text.to_string())],
                )
            } else if n == active {
                (active.clone(), active_sources(&active))
            } else {
                println!(
                    "unknown pipeline `{}` — shareable: `{active}` (active), built-ins: {}",
                    crate::core::utils::sanitize_for_terminal(n),
                    BUILTIN_NAMES.join(", ")
                );
                return Ok(None);
            }
        }
        None => (active.clone(), active_sources(&active)),
    };
    // The behaviour summary comes from the resolved rules, which describe the
    // ACTIVE pipeline — a built-in shared by name keeps its own file comments
    // as the summary instead of inheriting the active pipeline rules.
    let is_active = display == active;
    let what = if is_active {
        describe_rules(&loaded.resolved)
    } else {
        Vec::new()
    };
    let tracker = crate::core::tracking::Tracker::new().ok();
    // The PreTool rewrite (grep -> rtk grep, which compresses) is RTK's primary
    // bash-savings lever, recorded in the commands table — the "Command
    // filtering" panel. Scoped to the cwd so a shared project's card reflects
    // its own work, matching how `rtk gain` filters.
    let filtering = tracker.as_ref().and_then(|t| {
        let cwd = crate::core::user_dirs::current_dir().ok()?;
        t.get_summary_filtered(cwd.to_str()).ok()
    });
    let hook = tracker
        .as_ref()
        .and_then(|t| t.attribution_site("hook", "pipeline", 30).ok())
        .and_then(|rows| rows.into_iter().find(|r| r.name == display));
    let slices = tracker
        .as_ref()
        .and_then(|t| crate::analytics::gain::hook_slices_for(t, &display, 30).ok())
        .unwrap_or_default();
    // Delegation records under site=task, keyed by task name. Sessions that
    // only delegate (the common case for a read-heavy pipeline) would show
    // "no recorded activity" without this — the session's biggest effect.
    let tasks = tracker
        .as_ref()
        .and_then(|t| t.attribution_site("task", "task", 30).ok())
        .unwrap_or_default();
    // The daemon stamps rows with ITS pipeline (the user-scope one driving the
    // proxy), not the project pipeline that drives hooks. A project pipeline
    // therefore never matches by name, so fall back to the single daemon
    // summary when the named lookup finds nothing.
    let proxy = crate::llm_proxy::savings_by_pipeline()
        .and_then(|rows| rows.into_iter().find(|r| r.name == display))
        .or_else(crate::llm_proxy::read_gain_summary_as_pipeline);
    let published = crate::core::pipeline_remote::published_source(&display);
    Ok(Some(CardData {
        display,
        is_active,
        sources,
        what,
        filtering,
        hook,
        slices,
        tasks,
        proxy,
        published,
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
    }))
}

/// `rtk pipeline share [name]` — a paste-ready markdown card: the pipeline's
/// measured savings (both sites) plus the pipeline file itself, so sharing
/// the numbers and sharing the pipeline are one artifact.
pub fn run_share(name: Option<&str>, force_md: bool, gist: bool, force: bool) -> Result<i32> {
    let Some(card) = collect_card(name)? else {
        return Ok(1);
    };
    // A terminal gets the screenshot-able card; a pipe or redirect gets the
    // paste-ready markdown, so `rtk pipeline share > card.md` keeps working
    // and the two renderings never mix on one stream.
    if gist {
        let card_md = card.card_md();
        return publish_gist(&card.display, &card.sources, &card_md, force);
    }
    use std::io::IsTerminal;
    if force_md || !std::io::stdout().is_terminal() {
        print!("{}", card.card_md());
    } else {
        card.print_terminal();
    }
    Ok(0)
}

/// The active pipeline's file set, labeled by scope; the compiled built-in
/// text when no file is on disk.
fn active_sources(active: &str) -> Vec<(String, String)> {
    let files: Vec<(String, String)> = pipeline_config::load_paths()
        .into_iter()
        .filter_map(|(scope, path)| {
            let file_name = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "pipeline.toml".to_string());
            let scope_label = match scope {
                rtk_engine::pipeline::Scope::Project => "project",
                _ => "user",
            };
            std::fs::read_to_string(&path)
                .ok()
                .map(|text| (format!("{scope_label} {file_name}"), text))
        })
        .collect();
    if !files.is_empty() {
        return files;
    }
    match builtin_text(active) {
        Some(text) => vec![(format!("{active} (built-in)"), text.to_string())],
        None => Vec::new(),
    }
}

/// A 10-step bar for a percentage, monospace-safe on every platform that
/// renders the card. Shares the visual language of `rtk gain`'s meters.
fn pct_bar(pct: f64) -> String {
    let filled = ((pct / 10.0).round() as usize).min(10);
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}

/// `hook.before_tool.read` -> `before read`, `proxy.request.cache` ->
/// `proxy cache`: the card speaks positions in plain words.
fn friendly_position(rendered: &str) -> String {
    rendered
        .replace("hook.before_tool.", "before ")
        .replace("hook.after_tool.", "after ")
        .replace("proxy.request.", "proxy ")
        .replace('_', " ")
}

/// One plain-language line per live rule — what the pipeline actually does,
/// derived from the resolved rules rather than hand-written, so the summary
/// cannot drift from the file below it. Disabled, shadowed and overridden
/// rules are not part of the pipeline's behaviour and are left out.
fn describe_rules(resolved: &rtk_engine::pipeline::Resolved) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (position, rules) in &resolved.rules {
        for rule in rules {
            // `overridden_by` means an [override] patched the rule — the def
            // already carries the merged values — so it stays in the summary.
            if !rule.def.enabled || rule.def.shadow {
                continue;
            }
            let does = match rule.def.action.as_deref() {
                Some("route") => format!(
                    "delegate -> {}",
                    rule.def.task.as_deref().unwrap_or("bulk-read")
                ),
                Some("deny") => "block".to_string(),
                Some("passthrough") => "passthrough".to_string(),
                Some("rewrite") => "rewrite -> rtk".to_string(),
                Some(other) => format!("action {other}"),
                None => format!("compress [{}]", rule.def.use_.join(", ")),
            };
            let when = if rule.def.when.is_empty() {
                String::new()
            } else {
                rule.def
                    .when
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push((friendly_position(&position.render()), does, when));
        }
    }
    out
}

/// The measured side of a share card — everything that came from the
/// trackers rather than from the pipeline file.
struct CardStats<'a> {
    what: &'a [(String, String, String)],
    filtering: Option<&'a crate::core::tracking::GainSummary>,
    hook: Option<&'a crate::core::tracking::AttributionRow>,
    hook_slices: &'a [(String, i64, i64, i64)],
    tasks: &'a [crate::core::tracking::AttributionRow],
    proxy: Option<&'a crate::llm_proxy::ProxyPipelineRow>,
}

/// The terminal rendering of a share card: the same facts as the markdown,
/// drawn as panels in `rtk gain`'s visual language so a screenshot of it is
/// presentable as-is. The markdown stays the paste artifact — this never
/// prints TOML, and ends by saying how to get the markdown.
fn print_terminal_card(
    name: &str,
    stats: &CardStats,
    published: Option<&str>,
    version: &str,
    date: &str,
) {
    use crate::core::utils::format_tokens;
    use colored::Colorize;
    const W: usize = 74;
    let hr = |title: &str| {
        let title = format!(" {title} ");
        let fill = W.saturating_sub(title.chars().count() + 2);
        println!("{}", format!("┌─{title}{}┐", "─".repeat(fill)).cyan());
    };
    let row_cols = |left: String, right: String| {
        let text = format!("{left} {right}");
        let pad = W.saturating_sub(text.chars().count() + 3);
        println!("{} {}{} {}", "│".cyan(), text, " ".repeat(pad), "│".cyan());
    };
    let end_panel = || println!("{}", format!("└{}┘", "─".repeat(W - 1)).cyan());
    let trunc = |text: &str, max: usize| -> String {
        if text.chars().count() <= max {
            text.to_string()
        } else {
            let cut: String = text.chars().take(max.saturating_sub(1)).collect();
            format!("{cut}…")
        }
    };

    hr(&format!("rtk pipeline · {name}"));
    if stats.what.is_empty() {
        row_cols(
            "(built-in — see its file comments for what it does)".to_string(),
            String::new(),
        );
    }
    for (position, does, when) in stats.what {
        let left = format!("{position:<18} {does:<22}");
        let right = if when.is_empty() {
            String::new()
        } else {
            trunc(when, W.saturating_sub(left.chars().count() + 5))
        };
        row_cols(left, right);
    }
    end_panel();

    hr("Measured impact");
    let mut any = false;
    // Each row carries its own unit word (removed / cached), so the panel
    // explains itself inline instead of through a legend below it.
    let bar_row = |label: &str, activity: String, pct: f64, unit: &str| {
        let bar = pct_bar(pct);
        let colored_bar = if pct >= 60.0 {
            bar.green().to_string()
        } else if pct >= 25.0 {
            bar.yellow().to_string()
        } else {
            bar.normal().to_string()
        };
        let text = format!("{label:<28} {activity:>14}  ");
        let unit_str = format!(" {unit}");
        let pad = W.saturating_sub(text.chars().count() + 10 + 4 + 5 + unit_str.chars().count());
        println!(
            "{} {}{} {:>4.0}%{}{} {}",
            "│".cyan(),
            text,
            colored_bar,
            pct,
            unit_str.dimmed(),
            " ".repeat(pad),
            "│".cyan()
        );
    };
    if let Some(f) = stats.filtering.filter(|f| f.total_commands > 0) {
        bar_row(
            "command filtering",
            format!("{} commands", f.total_commands),
            f.avg_savings_pct,
            "removed",
        );
        any = true;
    }
    if let Some(hook_row) = stats.hook.filter(|r| r.tokens_in > 0) {
        let pct = hook_row.saved().max(0) as f64 / hook_row.tokens_in as f64 * 100.0;
        bar_row(
            "agent hooks (30d)",
            format!("{} outputs", hook_row.calls),
            pct,
            "removed",
        );
        any = true;
    }
    for (label, calls, tokens_in, saved) in stats.hook_slices {
        if *tokens_in <= 0 {
            continue;
        }
        let pct = (*saved).max(0) as f64 / *tokens_in as f64 * 100.0;
        bar_row(
            &format!("· {label}"),
            format!("{calls} outputs"),
            pct,
            "removed",
        );
        any = true;
    }
    for task in stats.tasks {
        // Delegation's reduction is ~100% by construction (the file never
        // enters context), so a bar carries no information. The informative,
        // user-agnostic figure is the absolute context it displaced — it
        // varies by what was read, and it is what a reader actually wants.
        // Laid out on the bar rows' grid (label column, activity column) so
        // the panel reads as one table.
        let kept = (task.tokens_in - task.tokens_out).max(0);
        row_cols(
            format!(
                "{:<28} {:>14}",
                trunc(&format!("delegated · {}", task.name), 28),
                format!("{} reads", task.calls)
            ),
            format!(" {} kept out of context", format_tokens(kept as usize)),
        );
        any = true;
    }
    if let Some(proxy_row) = stats.proxy {
        let raw = proxy_row.tokens_in + proxy_row.tokens_saved;
        // A 0% bar is noise, not information — the proxy's real win on most
        // sessions is the cache line below, not its own transforms. Only show
        // the input bar when it actually moved.
        if raw > 0 && proxy_row.tokens_saved > 0 {
            let pct = proxy_row.tokens_saved.max(0) as f64 / raw as f64 * 100.0;
            if pct >= 1.0 {
                bar_row(
                    "llm proxy input",
                    format!("{} requests", proxy_row.requests),
                    pct,
                    "removed",
                );
                any = true;
            }
        }
        let prefix = proxy_row.cache_read + proxy_row.cache_created;
        if prefix > 0 {
            let hit = proxy_row.cache_read as f64 / prefix as f64 * 100.0;
            bar_row(
                "prompt-cache hit",
                format!("{} requests", proxy_row.requests),
                hit,
                "cached",
            );
            any = true;
        }
    }
    if !any {
        row_cols(
            "no recorded activity yet — run sessions with this pipeline active".to_string(),
            String::new(),
        );
    }
    // The one caveat that cannot live inline: sizes are estimates. A single
    // dimmed row inside the panel, instead of a legend paragraph below it.
    if any {
        let note = "~ estimates (≈ bytes/4) — ratios reliable, absolutes approximate";
        let pad = W.saturating_sub(note.chars().count() + 3);
        println!(
            "{} {}{} {}",
            "│".cyan(),
            note.dimmed(),
            " ".repeat(pad),
            "│".cyan()
        );
    }
    end_panel();
    // The share CTA is the card's one next action — a panel in the card's
    // own visual language, not a footnote, with the command ready to paste.
    // Once published it flips to the repo link and the line others run.
    let row_accent = |text: &str| {
        let pad = W.saturating_sub(text.chars().count() + 3);
        println!(
            "{} {}{} {}",
            "│".cyan(),
            text.green().bold(),
            " ".repeat(pad),
            "│".cyan()
        );
    };
    match published {
        Some(install) => {
            hr("Published");
            let kv = |label: &str, value: &str| {
                row_cols(trunc(&format!("{label:<18} {value}"), W - 4), String::new());
            };
            if let Some(repo) = install.strip_prefix("github:") {
                kv("live at:", &format!("https://github.com/{repo}"));
            }
            kv("others install it:", &format!("rtk pipeline add {install}"));
            kv("update the repo:", "rtk pipeline share publish");
            end_panel();
        }
        None => {
            hr("Share it");
            let hook = if any {
                "Proud of these numbers? Put your pipeline in everyone's hands:"
            } else {
                "Put your pipeline in everyone's hands — one command:"
            };
            row_cols(hook.to_string(), String::new());
            row_cols(String::new(), String::new());
            row_accent("   ▶ rtk pipeline share publish");
            row_cols(String::new(), String::new());
            end_panel();
        }
    }
    println!(
        "{}",
        format!("  rtk v{version} · {date} · markdown: rtk pipeline share --md").dimmed()
    );
}

fn share_card(
    name: &str,
    sources: &[(String, String)],
    stats: &CardStats,
    version: &str,
    date: &str,
) -> String {
    let CardStats {
        what,
        filtering,
        hook,
        hook_slices,
        tasks,
        proxy,
    } = *stats;
    let has_filtering = filtering.is_some_and(|f| f.total_commands > 0);
    let mut out = String::new();
    out.push_str(&format!("# rtk pipeline · {name}\n\n"));

    // ---- what it does: derived from the rules, never hand-written
    if !what.is_empty() {
        out.push_str("## What it does\n\n");
        for (position, does, when) in what {
            if when.is_empty() {
                out.push_str(&format!("- **{position}** — {does}\n"));
            } else {
                out.push_str(&format!("- **{position}** — {does} when {when}\n"));
            }
        }
        out.push('\n');
    }

    // ---- measured impact: percentages only. Reductions are byte ratios the
    // pipeline measured on its own traffic — exact, and portable between
    // users in a way tokens and dollars are not (those depend on each user's
    // models, prices and usage, so they would make the card about the author
    // rather than the pipeline).
    if !has_filtering && hook.is_none() && proxy.is_none() && tasks.is_empty() {
        out.push_str(
            "_No recorded activity yet — run some sessions with this pipeline active, then re-run `rtk pipeline share`._\n\n",
        );
    } else {
        out.push_str("## Measured impact\n\n");
        out.push_str("| lever | activity | reduction |\n");
        out.push_str("|-------|----------|-----------|\n");
        if let Some(f) = filtering.filter(|f| f.total_commands > 0) {
            out.push_str(&format!(
                "| command filtering (rtk git/npm/…) | {} commands | `{}` **{:.0}%** removed |\n",
                f.total_commands,
                pct_bar(f.avg_savings_pct),
                f.avg_savings_pct,
            ));
        }
        if let Some(row) = hook {
            let pct = if row.tokens_in > 0 {
                row.saved().max(0) as f64 / row.tokens_in as f64 * 100.0
            } else {
                0.0
            };
            out.push_str(&format!(
                "| agent hooks (last 30d) | {} outputs | `{}` **{pct:.0}%** removed |\n",
                row.calls,
                pct_bar(pct),
            ));
        }
        for (label, calls, tokens_in, saved) in hook_slices {
            if *tokens_in <= 0 {
                continue;
            }
            let pct = (*saved).max(0) as f64 / *tokens_in as f64 * 100.0;
            out.push_str(&format!(
                "| · {label} | {calls} outputs | `{}` **{pct:.0}%** removed |\n",
                pct_bar(pct),
            ));
        }
        for row in tasks {
            // ~100% by construction, so the informative figure is the absolute
            // context displaced, not a bar. A pure token count — no prices.
            let kept = (row.tokens_in - row.tokens_out).max(0);
            out.push_str(&format!(
                "| delegated reads (`{}`) | {} reads | **{}** kept out of context |\n",
                row.name,
                row.calls,
                crate::core::utils::format_tokens(kept as usize),
            ));
        }
        if let Some(row) = proxy {
            let raw = row.tokens_in + row.tokens_saved;
            if raw > 0 {
                let pct = row.tokens_saved.max(0) as f64 / raw as f64 * 100.0;
                if pct >= 1.0 {
                    out.push_str(&format!(
                        "| llm proxy input (all time) | {} requests | `{}` **{pct:.0}%** removed |\n",
                        row.requests,
                        pct_bar(pct),
                    ));
                }
            }
            let prefix = row.cache_read + row.cache_created;
            if prefix > 0 {
                let hit = row.cache_read as f64 / prefix as f64 * 100.0;
                out.push_str(&format!(
                    "| prompt-cache hit rate | {} requests | `{}` **{hit:.0}%** cached |\n",
                    row.requests,
                    pct_bar(hit),
                ));
            }
        }
        out.push('\n');
        out.push_str("_Estimates (≈ bytes/4): ratios reliable, absolutes approximate. Percentages travel across setups; token counts and costs do not._\n\n");
    }

    // ---- the pipeline itself, paste-ready
    for (label, text) in sources {
        out.push_str(&format!("## {label}\n\n```toml\n"));
        out.push_str(text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }
    out.push_str(&format!(
        "Measured by [rtk](https://github.com/rtk-ai/rtk) v{version} on {date}.\n",
    ));
    out.push_str("\nInstall this pipeline: `rtk pipeline add <url-of-this-card>`\n");
    out
}

pub fn run_status() -> Result<i32> {
    match pipeline_config::read_status() {
        None => {
            println!("pipeline: ok — no recorded load problems");
            let loaded = load_for_cli();
            println!("active: {}", loaded.resolved.name);
            print_auth_guidance(&loaded.resolved);
            let has_proxy_rules = loaded
                .resolved
                .rules
                .keys()
                .any(|p| p.site == rtk_engine::pipeline::SiteKind::Proxy);
            if has_proxy_rules && !crate::llm_proxy::is_reachable() {
                println!(
                    "note: proxy.request rules are present but the RTK proxy is not running — they are inert"
                );
            }
            if !has_proxy_rules {
                println!("note: no proxy.request rules — a running daemon forwards unchanged");
            }
            Ok(0)
        }
        Some(status) => {
            println!("pipeline: problems recorded at the last hook invocation");
            if let Some(diags) = status.get("diagnostics").and_then(|d| d.as_array()) {
                for diag in diags {
                    if let Some(message) = diag.get("message").and_then(|m| m.as_str()) {
                        println!("{message}");
                    }
                }
            }
            if status
                .get("fell_back")
                .and_then(|f| f.as_bool())
                .unwrap_or(false)
            {
                println!("hooks are running the built-in rtk:default until this is fixed");
            }
            Ok(1)
        }
    }
}

pub(crate) fn print_diagnostics(loaded: &LoadedPipeline) {
    for diagnostic in &loaded.diagnostics {
        let tag = match diagnostic.severity() {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        // Diagnostics can quote source lines from an untrusted shared file;
        // a raw ESC byte there would drive the adopter's terminal (color
        // spoofing, screen clears). Strip control bytes at the display
        // boundary, keeping layout whitespace.
        println!(
            "{tag}: {}",
            crate::core::utils::sanitize_for_terminal(&diagnostic.render())
        );
    }
    if !loaded.diagnostics.is_empty() {
        println!();
    }
}

/// Output piped into `explain`, for the size conditions an `after_tool` rule
/// reads.
///
/// Bounded, because stdin inherited from a parent that never writes is
/// indistinguishable from a pipe that is about to: `rtk pipeline explain` run
/// from a script without a redirect would otherwise wait forever. A redirect
/// from a file and a pipe carrying data both complete immediately; an idle pipe
/// gives up and the size conditions simply have no values.
fn read_piped_output() -> Option<String> {
    use std::io::{IsTerminal, Read};
    use std::sync::mpsc;
    use std::time::Duration;

    if std::io::stdin().is_terminal() {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = std::io::stdin().read_to_string(&mut text);
        let _ = tx.send(read.ok().map(|_| text));
    });
    match rx.recv_timeout(Duration::from_millis(250)) {
        Ok(Some(text)) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// `route` names the task that will answer; without it `explain` reports the
/// action but not where the work goes.
fn task_suffix(decision: &rtk_engine::pipeline::Decision) -> String {
    match decision.rule.def.task.as_deref() {
        Some(task) => format!(" -> task `{task}`"),
        None => String::new(),
    }
}

pub fn run_explain(
    agent: &str,
    tool: &str,
    format: Option<&str>,
    command: Option<&str>,
    file: Option<&std::path::Path>,
    whole_file: bool,
) -> Result<i32> {
    let Some(profile) = rtk_engine::harness::profile(agent) else {
        println!(
            "unknown agent `{agent}` — known: {}",
            rtk_engine::harness::HARNESSES
                .iter()
                .map(|h| h.id)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(1);
    };

    let loaded = load_for_cli();
    let pipeline = &loaded.resolved;
    let subject = profile.map_tool(tool);

    let mut facts = Facts::new();
    facts.insert("tool".into(), Fact::Str(subject.as_str().to_string()));
    if let Some(file) = file {
        for (key, value) in crate::core::pipeline_config::read_facts(file, whole_file) {
            facts.insert(key, value);
        }
    }
    // Piped output stands in for a real tool result, so the `after_tool` size
    // conditions (`tokens_gt`, `bytes_gt`, `lines_gt`) can be evaluated.
    let piped = read_piped_output();
    if let Some(text) = &piped {
        facts.insert("bytes".into(), Fact::Num(text.len() as f64));
        facts.insert("lines".into(), Fact::Num(text.lines().count() as f64));
        facts.insert(
            "tokens".into(),
            Fact::Num(rtk_engine::tokens::estimate_tokens(text) as f64),
        );
    }
    if let Some(format) = format {
        facts.insert("format".into(), Fact::Str(format.to_string()));
    }
    if let Some(command) = command {
        facts.insert("command".into(), Fact::Str(command.to_string()));
        facts.insert(
            "is_rtk_command".into(),
            Fact::Bool(command.split_whitespace().next() == Some("rtk")),
        );
    }

    let proxy_running = crate::llm_proxy::is_reachable();
    let resolution = rtk_engine::harness::resolve_output_transform(profile, subject, proxy_running);

    println!(
        "agent: {agent}   tool: {tool} -> subject: {}",
        subject.as_str()
    );

    let before = Position::hook(Event::BeforeTool, subject);
    if profile.event(Event::BeforeTool).is_some() {
        let decisions = evaluate(pipeline, before, &facts);
        for decision in &decisions {
            match &decision.kind {
                DecisionKind::Act(action) => println!(
                    "{}: rule `{}` -> action {}{}",
                    before.render(),
                    decision.rule.name,
                    action.as_str(),
                    task_suffix(decision)
                ),
                DecisionKind::Transform { .. } => {}
            }
        }
    }

    let after = Position::hook(Event::AfterTool, subject);
    match resolution {
        rtk_engine::harness::Resolution::Transform(Event::AfterTool) => {
            let decisions = evaluate(pipeline, after, &facts);
            if decisions.is_empty() {
                println!(
                    "{}: no rule matches these facts — output passes through",
                    after.render()
                );
                if !pipeline.rules.contains_key(&after) {
                    println!(
                        "(the active pipeline `{}` has no rules at this position — \
                         `extends = \"rtk:posthook\"` adds output compression)",
                        pipeline.name
                    );
                }
            } else {
                for decision in &decisions {
                    match &decision.kind {
                        DecisionKind::Transform {
                            methods,
                            unresolved,
                            ..
                        } => println!(
                            "{}: rule `{}` -> [{}]{}{}{}",
                            after.render(),
                            decision.rule.name,
                            methods
                                .iter()
                                .map(|m| m.name())
                                .collect::<Vec<_>>()
                                .join(", "),
                            // An empty chain is the visible symptom of a rule
                            // naming methods that do not exist at this site.
                            if methods.is_empty() {
                                "  (inert: nothing runs here — see `rtk pipeline validate`)"
                            } else {
                                ""
                            },
                            if decision.shadow() { " (shadow)" } else { "" },
                            // Belt behind validate's E007: a name that failed
                            // to resolve at evaluation time was write-only
                            // state nothing ever read back.
                            if unresolved.is_empty() {
                                String::new()
                            } else {
                                format!("  [warn] unresolved: {}", unresolved.join(", "))
                            }
                        ),
                        DecisionKind::Act(action) => println!(
                            "{}: rule `{}` -> action {}{}",
                            after.render(),
                            decision.rule.name,
                            action.as_str(),
                            task_suffix(decision)
                        ),
                    }
                }
            }
        }
        rtk_engine::harness::Resolution::Transform(_) => {
            println!(
                "output transforms run at the proxy for {agent} (no output replacement in its hooks)"
            );
        }
        rtk_engine::harness::Resolution::DenyWithSuggestion(_) => {
            println!(
                "after_tool rules are inert: {agent} cannot replace tool output — \
                 RTK degrades to deny-with-suggestion before the tool"
            );
            println!("start the RTK proxy to enable output transforms: rtk llm-proxy start");
        }
        rtk_engine::harness::Resolution::Directive(channel) => {
            println!(
                "after_tool rules are inert: {agent} exposes no usable hook site — \
                 RTK reaches it through the {channel:?} directive channel only"
            );
            println!("start the RTK proxy to enable transforms: rtk llm-proxy start");
        }
        rtk_engine::harness::Resolution::Inert => {
            println!("inert: {agent} has no hook events and no directive channel");
        }
    }
    print_diagnostics(&loaded);
    Ok(0)
}

pub fn run_log_toggle(on: bool) -> Result<i32> {
    crate::core::pipeline_log::set_enabled(on)
        .with_context(|| "failed to toggle the decision log")?;
    if on {
        println!(
            "decision log enabled — entries at {}",
            crate::core::pipeline_log::log_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
        println!("(decisions and commands only — never tool output or file contents)");
    } else {
        println!("decision log disabled");
    }
    Ok(0)
}

pub fn run_log_show(limit: usize, rule: Option<&str>) -> Result<i32> {
    if !crate::core::pipeline_log::enabled() {
        println!("decision log is off — enable with `rtk pipeline log on`");
    }
    let entries = crate::core::pipeline_log::read_last(limit, rule);
    if entries.is_empty() {
        println!("no entries");
        return Ok(0);
    }
    for entry in &entries {
        println!("{}", crate::core::pipeline_log::render(entry));
    }
    Ok(0)
}

pub fn run_explain_last() -> Result<i32> {
    let entries = crate::core::pipeline_log::read_last(1, None);
    let Some(entry) = entries.first() else {
        println!(
            "no logged decisions — enable the log with `rtk pipeline log on` and run a command"
        );
        return Ok(1);
    };
    println!("{}", crate::core::pipeline_log::render(entry));
    let position = format!("{}.{}.{}", entry.site, entry.event, entry.subject);
    match (&entry.action, entry.methods.is_empty()) {
        (Some(action), _) => println!(
            "rule `{}` fired at {position} with action `{action}`",
            entry.rule
        ),
        (None, false) => {
            println!(
                "rule `{}` fired at {position}; methods applied: [{}]",
                entry.rule,
                entry.methods.join(", ")
            );
            if let (Some(before), Some(after)) = (entry.tokens_before, entry.tokens_after) {
                let savings = if before > 0 {
                    100.0 - (after as f64 / before as f64 * 100.0)
                } else {
                    0.0
                };
                println!("tokens {before} -> {after} ({savings:.1}% saved)");
            }
        }
        _ => {}
    }
    if entry.shadow {
        println!("shadow rule: recorded only, nothing was emitted");
    }
    Ok(0)
}

pub fn run_bench(file: &std::path::Path) -> Result<i32> {
    let raw = std::fs::read_to_string(file)
        .with_context(|| format!("Failed to read {}", file.display()))?;
    let basename = file.file_name().and_then(|n| n.to_str()).unwrap_or("input");
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("");

    let (subject, format) = match basename {
        "package-lock.json" | "Cargo.lock" | "yarn.lock" | "pnpm-lock.yaml" => {
            (Subject::Read, rtk_engine::block::ContentFormat::Lockfile)
        }
        _ if extension == "json" => (Subject::Read, rtk_engine::block::ContentFormat::Json),
        _ if extension == "html" || extension == "htm" => {
            (Subject::Web, rtk_engine::block::ContentFormat::Web)
        }
        _ => (Subject::Shell, rtk_engine::block::ContentFormat::Term),
    };

    let loaded = load_for_cli();
    let mut facts = Facts::new();
    facts.insert("tool".into(), Fact::Str(subject.as_str().to_string()));
    facts.insert("format".into(), Fact::Str(format.as_str().to_string()));
    facts.insert("basename".into(), Fact::Str(basename.to_string()));
    facts.insert("is_rtk_command".into(), Fact::Bool(false));
    facts.insert(
        "tokens".into(),
        Fact::Num(rtk_engine::tokens::estimate_tokens(&raw) as f64),
    );
    facts.insert("bytes".into(), Fact::Num(raw.len() as f64));
    facts.insert("lines".into(), Fact::Num(raw.lines().count() as f64));

    let position = Position::hook(Event::AfterTool, subject);
    let decisions = evaluate(&loaded.resolved, position, &facts);
    let mut methods: Vec<&'static dyn rtk_engine::method::Method> = Vec::new();
    let mut rule_names = Vec::new();
    for decision in &decisions {
        if let DecisionKind::Transform {
            methods: selected, ..
        } = &decision.kind
        {
            methods.extend_from_slice(selected);
            rule_names.push(decision.rule.name.clone());
        }
    }
    if methods.is_empty() {
        println!(
            "no rule matches ({}, format={}) in `{}` — nothing to bench",
            position.render(),
            format.as_str(),
            loaded.resolved.name
        );
        if !loaded.resolved.rules.contains_key(&position) {
            println!("hint: `extends = \"rtk:posthook\"` adds the output-compression rules");
        }
        return Ok(0);
    }

    let ctx = rtk_engine::method::MethodCtx::new(basename);
    let origin = rtk_engine::block::Origin::default();
    let provenance = rtk_engine::block::Provenance::default();
    let block = rtk_engine::block::Block {
        text: &raw,
        subject,
        format,
        origin: &origin,
        provenance: &provenance,
    };
    let started = std::time::Instant::now();
    // Unlike the hook path, this command has no outer panic boundary: a
    // method that panics on one input would abort `rtk pipeline bench` with
    // a backtrace instead of naming the chain that failed.
    let out = rtk_engine::chain::run_chain_caught(&methods, &block, &ctx).with_context(|| {
        format!(
            "method chain [{}] panicked on this input",
            methods
                .iter()
                .map(|m| m.name())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let elapsed = started.elapsed();

    let before = rtk_engine::tokens::estimate_tokens(&raw);
    let after = rtk_engine::tokens::estimate_tokens(&out);
    let savings = if before > 0 {
        100.0 - (after as f64 / before as f64 * 100.0)
    } else {
        0.0
    };
    println!(
        "rule [{}] chain [{}] ({}, format={})",
        rule_names.join(", "),
        methods
            .iter()
            .map(|m| m.name())
            .collect::<Vec<_>>()
            .join(", "),
        position.render(),
        format.as_str()
    );
    println!(
        "before: {before} tokens   after: {after} tokens   savings: {savings:.1}%   ({elapsed:?})"
    );
    Ok(0)
}

pub fn run_methods_list() -> Result<i32> {
    println!(
        "{:<20} {:<9} {:<6} {:<10} {:<8} site",
        "method", "kind", "lossy", "recovery", "cost"
    );
    let print_info = |name: &str, info: rtk_engine::method::MethodInfo, site: &str| {
        println!(
            "{:<20} {:<9} {:<6} {:<10} {:<8} {}",
            name,
            match info.kind {
                rtk_engine::method::MethodKind::Transform => "transform",
                rtk_engine::method::MethodKind::Directive => "directive",
            },
            info.lossy,
            format!("{:?}", info.recovery).to_lowercase(),
            format!("{:?}", info.cost).to_lowercase(),
            site
        );
    };
    for method in rtk_engine::registry::METHODS {
        print_info(method.name(), method.info(), "hook/proxy");
    }
    for (name, info) in rtk_engine::registry::PROXY_METHOD_INFO {
        print_info(name, *info, "proxy");
    }
    println!();
    println!("chains (composites, usable anywhere a method is):");
    for name in rtk_engine::registry::CHAIN_NAMES {
        // CHAIN_NAMES and the chain table are compiled-in and never disagree,
        // but an `.expect()` on a listing command is still a panic in a
        // production path (house rule); skip a name that somehow resolves to
        // nothing rather than abort the whole `methods list`.
        let Some(chain) = rtk_engine::registry::chain(name) else {
            continue;
        };
        println!(
            "  {:<10} = [{}]",
            name,
            chain
                .iter()
                .map(|m| m.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!();
    println!("directives:");
    for directive in rtk_engine::directive::DIRECTIVES {
        println!("  {}", directive.name());
    }
    println!("  rtk-awareness (levels: default | high | full)");
    println!();
    println!(
        "reserved names: {}",
        rtk_engine::registry::names::RESERVED.join(", ")
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use rtk_engine::pipeline::{
        BUILTIN_NAMES, Scope, SourceFile, ValidateCtx, builtin_text, has_errors, parse, resolve,
        validate,
    };

    #[test]
    fn extract_toml_pulls_the_fenced_block_from_a_share_card() {
        let card = "# rtk pipeline: demo\n\nprose\n\n```toml\n[pipeline]\nname = \"demo\"\n```\n\ntrailing";
        assert_eq!(super::extract_toml(card), "[pipeline]\nname = \"demo\"\n");
    }

    #[test]
    fn extract_toml_passes_raw_toml_through_unchanged() {
        let raw = "[pipeline]\nname = \"demo\"\n";
        assert_eq!(super::extract_toml(raw), raw);
    }

    #[test]
    fn share_card_renders_both_sites_and_the_file() {
        let hook = crate::core::tracking::AttributionRow {
            name: "my-pipe".into(),
            calls: 42,
            tokens_in: 1000,
            tokens_out: 300,
        };
        let proxy = crate::llm_proxy::ProxyPipelineRow {
            name: "my-pipe".into(),
            requests: 7,
            tokens_in: 450,
            tokens_saved: 1050,
            saved_ccr: 700,
            saved_normalize: 350,
            saved_ingest_filter: 0,
            saved_pixel_compress: 0,
            cache_read: 4000,
            cache_created: 1000,
        };
        let sources = vec![(
            "user pipeline.toml".to_string(),
            "[pipeline]\nname = \"my-pipe\"\n".to_string(),
        )];
        let slices = vec![
            ("shell output (bash)".to_string(), 3, 1000, 510),
            ("file reads".to_string(), 1, 400, 190),
        ];
        let what = vec![(
            "before read".to_string(),
            "delegate -> bulk-read".to_string(),
            "file_lines_gt=1200".to_string(),
        )];
        let card = super::share_card(
            "my-pipe",
            &sources,
            &super::CardStats {
                what: &what,
                filtering: None,
                hook: Some(&hook),
                hook_slices: &slices,
                tasks: &[],
                proxy: Some(&proxy),
            },
            "1.0.0",
            "2026-09-22",
        );
        assert!(card.starts_with("# rtk pipeline · my-pipe\n"));
        // The behaviour summary precedes the numbers: a reader decides whether
        // the pipeline fits before caring how well it performed elsewhere.
        assert!(card.contains("## What it does"));
        assert!(card.contains("- **before read** — delegate -> bulk-read when file_lines_gt=1200"));
        // Percentages only — no token counts, no dollars. 700/1000 = 70%.
        // The unit word rides inline on every row (removed / cached), so the
        // card needs no legend paragraph.
        assert!(
            card.contains("| agent hooks (last 30d) | 42 outputs | `███████░░░` **70%** removed |")
        );
        assert!(
            card.contains("| · shell output (bash) | 3 outputs | `█████░░░░░` **51%** removed |")
        );
        assert!(card.contains("| · file reads | 1 outputs | `█████░░░░░` **48%** removed |"));
        // proxy: 1050/(450+1050) = 70%; cache hit 4000/5000 = 80%.
        assert!(card.contains(
            "| llm proxy input (all time) | 7 requests | `███████░░░` **70%** removed |"
        ));
        assert!(
            card.contains("| prompt-cache hit rate | 7 requests | `████████░░` **80%** cached |")
        );
        assert!(
            !card.contains("1.1K") && !card.contains("4.0K"),
            "token counts must not appear — they are the author's usage, not the pipeline's"
        );
        assert!(card.contains("## user pipeline.toml"));
        assert!(card.contains("```toml\n[pipeline]\nname = \"my-pipe\"\n```"));
        assert!(card.contains("v1.0.0 on 2026-09-22"));
    }

    #[test]
    fn share_card_without_data_says_so_instead_of_an_empty_table() {
        let card = super::share_card(
            "fresh",
            &[("user pipeline.toml".to_string(), "x = 1\n".to_string())],
            &super::CardStats {
                what: &[],
                filtering: None,
                hook: None,
                hook_slices: &[],
                tasks: &[],
                proxy: None,
            },
            "1.0.0",
            "2026-09-22",
        );
        assert!(card.contains("No recorded activity yet"));
        assert!(!card.contains("| lever |"));
        assert!(card.contains("```toml\nx = 1\n```"));
    }

    #[test]
    fn show_prints_the_builtin_literals_verbatim() {
        for name in BUILTIN_NAMES {
            let text = builtin_text(name).expect("literal exists");
            let parsed = parse(text, name);
            assert!(parsed.diagnostics.is_empty(), "{name}");
            assert!(
                text.contains(&format!("name    = \"{name}\""))
                    || text.contains(&format!("name = \"{name}\""))
            );
        }
    }

    #[test]
    fn example_pipelines_validate_without_errors() {
        let examples = [
            (
                "opt-in.toml",
                include_str!("../../examples/pipelines/opt-in.toml"),
            ),
            (
                "tune-one-thing.toml",
                include_str!("../../examples/pipelines/tune-one-thing.toml"),
            ),
            (
                "delegate-reads-adoption.toml",
                include_str!("../../examples/pipelines/delegate-reads-adoption.toml"),
            ),
            (
                "classify-routing.toml",
                include_str!("../../examples/pipelines/classify-routing.toml"),
            ),
            (
                "laya-route.toml",
                include_str!("../../examples/pipelines/laya-route.toml"),
            ),
            (
                "judge-before-tool.toml",
                include_str!("../../examples/pipelines/judge-before-tool.toml"),
            ),
        ];
        for (name, text) in examples {
            let parsed = parse(text, name);
            assert!(
                parsed.diagnostics.is_empty(),
                "{name}: {:#?}",
                parsed.diagnostics
            );
            let (resolved, mut diagnostics) = resolve(&[SourceFile {
                scope: Scope::User,
                label: name.to_string(),
                file: parsed.file.expect("parses"),
            }]);
            diagnostics.extend(validate(&resolved, &ValidateCtx::default()));
            assert!(
                !has_errors(&diagnostics),
                "example {name} has validation errors: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn gist_raw_url_rewrites_a_page_url_to_raw() {
        assert_eq!(
            super::gist_raw_url("https://gist.github.com/alice/abc123"),
            "https://gist.githubusercontent.com/alice/abc123/raw"
        );
        assert_eq!(
            super::gist_raw_url("https://gist.github.com/alice/abc123/"),
            "https://gist.githubusercontent.com/alice/abc123/raw"
        );
    }

    #[test]
    fn gist_raw_url_leaves_other_urls_untouched() {
        let raw = "https://gist.githubusercontent.com/alice/abc123/raw";
        assert_eq!(super::gist_raw_url(raw), raw);
        let other = "https://example.com/pipe.toml";
        assert_eq!(super::gist_raw_url(other), other);
    }

    #[test]
    fn scan_flags_known_secret_shapes_and_private_paths() {
        let sources = vec![(
            "user pipeline.toml".to_string(),
            "key = \"sk-abcdefghijklmnopqrstuvwx\"\npath = \"/Users/alice/secret\"\n".to_string(),
        )];
        let hits = super::scan_for_secrets(&sources);
        assert!(hits.iter().any(|h| h.contains("sk- API key")), "{hits:?}");
        assert!(
            hits.iter().any(|h| h.contains("absolute home path")),
            "{hits:?}"
        );
    }

    #[test]
    fn scan_is_empty_for_a_clean_pipeline() {
        let sources = vec![(
            "user pipeline.toml".to_string(),
            "[pipeline]\nname = \"demo\"\napi_key_env = \"ANTHROPIC_API_KEY\"\n".to_string(),
        )];
        assert!(super::scan_for_secrets(&sources).is_empty());
    }

    #[test]
    fn mask_hides_the_middle_of_a_token() {
        assert_eq!(super::mask("sk-abcdefghij"), "sk-a\u{2026}ij");
        assert_eq!(
            super::mask("short"),
            "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"
        );
    }

    #[test]
    fn referenced_plugin_methods_flags_an_unknown_method_as_missing() {
        let text = "[pipeline]\nname = \"t\"\n\n[[hook.after_tool.shell]]\nuse = [\"totallynotarealmethod\"]\n";
        let parsed = parse(text, "t.toml");
        let (resolved, _) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "t.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        let (present, missing) = super::referenced_plugin_methods(&resolved);
        assert!(present.is_empty(), "present={present:?}");
        assert_eq!(missing, vec!["totallynotarealmethod".to_string()]);
    }

    #[test]
    fn referenced_plugin_methods_ignores_builtin_methods() {
        let text = "[pipeline]\nname = \"t\"\n\n[[hook.after_tool.shell]]\nuse = [\"dedup\"]\n";
        let parsed = parse(text, "t.toml");
        let (resolved, _) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "t.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        let (present, missing) = super::referenced_plugin_methods(&resolved);
        assert!(present.is_empty(), "present={present:?}");
        assert!(missing.is_empty(), "missing={missing:?}");
    }
}
