//! Remote pipeline sharing over GitHub: one public repo per pipeline in the
//! author's own account, installs pinned to a commit, provenance recorded in
//! a local ledger, discovery through the `rtk-pipeline` topic. RTK hosts
//! nothing, reviews nothing, and never holds a token — everything goes
//! through the user's own `gh`.

use crate::core::pipeline_config;
use anyhow::{Context, Result};
use rtk_engine::pipeline::has_errors;

pub struct GithubSource {
    pub owner: String,
    pub repo: String,
    pub reference: Option<String>,
}

impl GithubSource {
    pub fn canonical(&self) -> String {
        format!("github:{}/{}", self.owner, self.repo)
    }
}

/// `github:owner/repo[@ref]` and plain `https://github.com/owner/repo` browser
/// URLs both resolve to a repo install; anything deeper (blob/tree paths) is
/// not a repo and falls through to the generic URL fetch.
pub fn parse_github_source(source: &str) -> Option<GithubSource> {
    let rest = source
        .strip_prefix("github:")
        .or_else(|| source.strip_prefix("https://github.com/"))?;
    let rest = rest.trim_end_matches('/');
    let (path, reference) = match rest.split_once('@') {
        Some((p, r)) if !r.is_empty() => (p, Some(r.to_string())),
        Some(_) => return None,
        None => (rest, None),
    };
    let path = path.trim_end_matches(".git");
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    };
    if !ok(owner) || !ok(repo) {
        return None;
    }
    Some(GithubSource {
        owner: owner.to_string(),
        repo: repo.to_string(),
        reference,
    })
}

pub struct PinnedFetch {
    pub toml: String,
    pub sha: Option<String>,
    pub canonical: String,
}

pub fn fetch_github_pipeline(src: &GithubSource) -> Result<PinnedFetch> {
    let sha = resolve_sha(src);
    if sha.is_none() {
        eprintln!(
            "rtk: could not resolve a commit for {} — installing UNPINNED from the default branch",
            src.canonical()
        );
    }
    let reference = sha
        .clone()
        .or_else(|| src.reference.clone())
        .unwrap_or_else(|| "HEAD".to_string());
    let url = format!(
        "https://raw.githubusercontent.com/{}/{}/{}/pipeline.toml",
        src.owner, src.repo, reference
    );
    // Private repos (and rate-limited public fetches) are not readable raw;
    // the user's own gh can read anything they have access to.
    let toml = match http_get(&url) {
        Ok(toml) => toml,
        Err(raw_err) => gh(&gh_args(&[
            "api",
            "-H",
            "Accept: application/vnd.github.raw",
            &format!(
                "repos/{}/{}/contents/pipeline.toml?ref={}",
                src.owner, src.repo, reference
            ),
        ]))
        .map_err(|_| {
            raw_err.context(format!(
                "no readable pipeline.toml at the root of {} — is this an rtk pipeline repo? \
                 (a private repo needs `gh auth login` with access to it)",
                src.canonical()
            ))
        })?,
    };
    Ok(PinnedFetch {
        toml,
        sha,
        canonical: src.canonical(),
    })
}

fn resolve_sha(src: &GithubSource) -> Option<String> {
    let reference = src.reference.as_deref().unwrap_or("HEAD");
    if reference.len() == 40 && reference.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(reference.to_string());
    }
    let api = format!("repos/{}/{}/commits/{}", src.owner, src.repo, reference);
    if let Ok(body) = http_get(&format!("https://api.github.com/{api}"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(sha) = v.get("sha").and_then(|s| s.as_str())
        && sha.len() == 40
    {
        return Some(sha.to_string());
    }
    let out = std::process::Command::new("gh")
        .args(["api", &api, "--jq", ".sha"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (sha.len() == 40).then_some(sha)
}

fn http_get(url: &str) -> Result<String> {
    // GET of a public GitHub endpoint the user asked for; inbound content only.
    // nosemgrep: ureq-outside-telemetry
    ureq::get(url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .into_string()
        .context("read response")
}

pub struct InstallOrigin {
    pub source: String,
    pub sha: Option<String>,
}

fn read_ledger() -> toml::Table {
    pipeline_config::installs_ledger_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| t.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

/// Merge updates into a ledger entry, keeping keys the update does not name —
/// an install must not erase the publish record and vice versa.
fn ledger_upsert(table: &mut toml::Table, name: &str, updates: &[(&str, String)]) {
    let entry = table
        .entry(name.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !entry.is_table() {
        *entry = toml::Value::Table(toml::Table::new());
    }
    if let Some(entry) = entry.as_table_mut() {
        for (key, value) in updates {
            entry.insert(key.to_string(), toml::Value::String(value.clone()));
        }
    }
}

/// Best-effort: the ledger is advisory (it powers the diff/pin display, the
/// fork offer, and the card's publish CTA), so a failed write warns and
/// never blocks anything.
fn ledger_write(name: &str, updates: &[(&str, String)]) {
    let Some(path) = pipeline_config::installs_ledger_path() else {
        return;
    };
    let mut table = read_ledger();
    ledger_upsert(&mut table, name, updates);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, table.to_string()) {
        eprintln!("rtk: could not update the install ledger: {e}");
    }
}

pub fn record_install(name: &str, source: &str, sha: Option<&str>) {
    let mut updates = vec![
        ("source", source.to_string()),
        ("date", chrono::Local::now().format("%Y-%m-%d").to_string()),
    ];
    if let Some(sha) = sha {
        updates.push(("sha", sha.to_string()));
    }
    ledger_write(name, &updates);
}

/// Where this pipeline was published, as the install source others run
/// (`github:owner/repo`, or a gist URL).
pub fn record_publish(name: &str, install_source: &str) {
    ledger_write(name, &[("published", install_source.to_string())]);
}

pub fn published_source(name: &str) -> Option<String> {
    read_ledger()
        .get(name)?
        .as_table()?
        .get("published")?
        .as_str()
        .map(str::to_string)
}

pub fn install_origin(name: &str) -> Option<InstallOrigin> {
    let table = read_ledger();
    let entry = table.get(name)?.as_table()?;
    Some(InstallOrigin {
        source: entry.get("source")?.as_str()?.to_string(),
        sha: entry
            .get("sha")
            .and_then(|s| s.as_str())
            .map(str::to_string),
    })
}

pub fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// Line diff with 2 lines of context, built on a plain LCS — pipeline files
/// are small, and this keeps the update review self-contained with no new
/// dependency. Oversized inputs fall back to a pointer instead of a wall.
pub fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > 500 || b.len() > 500 {
        return "  (too large to diff inline — review the full source yourself)\n".to_string();
    }
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(('-', a[i]));
            i += 1;
        } else {
            ops.push(('+', b[j]));
            j += 1;
        }
    }
    ops.extend(a[i..].iter().map(|l| ('-', *l)));
    ops.extend(b[j..].iter().map(|l| ('+', *l)));

    let keep: Vec<bool> = ops
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            ops.iter()
                .enumerate()
                .any(|(k, (c, _))| *c != ' ' && idx.abs_diff(k) <= 2)
        })
        .collect();
    let mut out = String::new();
    let mut elided = false;
    for (idx, (c, line)) in ops.iter().enumerate() {
        if keep[idx] {
            out.push_str(&format!("  {c} {line}\n"));
            elided = false;
        } else if !elided {
            out.push_str("    ⋯\n");
            elided = true;
        }
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub fn repo_slug(name: &str) -> String {
    let safe: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-').to_string();
    if safe.starts_with("rtk-pipeline") {
        safe
    } else {
        format!("rtk-pipeline-{safe}")
    }
}

fn description(login: &str, repo: &str, headline: Option<&str>) -> String {
    match headline {
        Some(h) => {
            format!("RTK pipeline · {h} (self-reported) · rtk pipeline add github:{login}/{repo}")
        }
        None => format!("RTK pipeline · rtk pipeline add github:{login}/{repo}"),
    }
}

/// The README is the repo's card: install line first, self-reported label on
/// every number, the standard review-before-install warning, then the share
/// card itself (which embeds the TOML, so the README alone stays installable).
fn repo_readme(card_md: &str, full: &str, headline: Option<&str>) -> String {
    let (title, body) = match card_md.split_once('\n') {
        Some((t, b)) => (t, b),
        None => (card_md, ""),
    };
    let mut out = String::new();
    out.push_str(title);
    out.push('\n');
    if let Some(h) = headline {
        out.push_str(&format!("\n**{h}** — self-reported by the author's own rtk, measured on their traffic. rtk verifies nothing.\n"));
    } else {
        out.push_str("\nNumbers on this card are self-reported by the author's own rtk. rtk verifies nothing.\n");
    }
    out.push_str(&format!(
        "\n## Install\n\n```\nrtk pipeline add github:{full}\n```\n\n\
         rtk shows this pipeline's rules and asks before anything installs, and pins the \
         install to a commit. A pipeline can rewrite the commands your agent runs, filter \
         and replace what it reads, and send content to any upstream it declares — review \
         the rules and install only what you trust. RTK does not review or endorse shared \
         pipelines; the author is responsible for this repo, and you are responsible for \
         what you install.\n"
    ));
    out.push_str(body);
    out
}

/// Arrow-key menu on crossterm (the same backend as the gain TUI): ↑/↓ or
/// j/k move, enter confirms, 1-9 pick directly, esc/q cancels. The menu
/// erases itself and leaves one line echoing the choice, so transcripts
/// stay readable. Terminals that refuse raw mode get a numbered prompt.
pub(crate) fn select_menu(items: &[(&str, &str)], default: usize) -> Result<Option<usize>> {
    use colored::Colorize;
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    use crossterm::{cursor, execute, terminal};
    use std::io::Write;

    if items.is_empty() {
        return Ok(None);
    }
    if terminal::enable_raw_mode().is_err() {
        return select_menu_plain(items, default);
    }
    let mut out = std::io::stdout();
    let mut idx = default.min(items.len() - 1);
    let render = |out: &mut std::io::Stdout, idx: usize| -> Result<()> {
        for (i, (label, hint)) in items.iter().enumerate() {
            if i == idx {
                write!(
                    out,
                    "  {} {}  {}\r\n",
                    "▶".green(),
                    label.green().bold(),
                    hint.dimmed()
                )
                .context("render menu")?;
            } else {
                write!(out, "    {label}  {}\r\n", hint.dimmed()).context("render menu")?;
            }
        }
        write!(
            out,
            "{}\r\n",
            "  ↑/↓ choose · enter confirm · esc cancel".dimmed()
        )
        .context("render menu")?;
        out.flush().context("flush menu")?;
        Ok(())
    };
    let lines = items.len() as u16 + 1;
    let mut run = || -> Result<Option<usize>> {
        render(&mut std::io::stdout(), idx)?;
        loop {
            let Event::Key(key) = read().context("read key")? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    idx = if idx == 0 { items.len() - 1 } else { idx - 1 };
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    idx = (idx + 1) % items.len();
                }
                KeyCode::Char(c @ '1'..='9') => {
                    let n = c as usize - '1' as usize;
                    if n < items.len() {
                        return Ok(Some(n));
                    }
                }
                KeyCode::Enter => return Ok(Some(idx)),
                KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                _ => {}
            }
            execute!(
                out,
                cursor::MoveUp(lines),
                terminal::Clear(terminal::ClearType::FromCursorDown)
            )
            .context("redraw menu")?;
            render(&mut std::io::stdout(), idx)?;
        }
    };
    let result = run();
    let _ = terminal::disable_raw_mode();
    let result = result?;
    execute!(
        out,
        cursor::MoveUp(lines),
        terminal::Clear(terminal::ClearType::FromCursorDown)
    )
    .ok();
    match result {
        Some(i) => println!("  {} {}", "✓".green(), items[i].0),
        None => println!("  ✗ cancelled"),
    }
    Ok(result)
}

fn select_menu_plain(items: &[(&str, &str)], default: usize) -> Result<Option<usize>> {
    for (i, (label, hint)) in items.iter().enumerate() {
        println!("  {}. {label}  ({hint})", i + 1);
    }
    let answer = prompt(&format!("choice [{}]: ", default + 1))?;
    if answer.is_empty() {
        return Ok(Some(default.min(items.len() - 1)));
    }
    match answer.parse::<usize>() {
        Ok(n) if (1..=items.len()).contains(&n) => Ok(Some(n - 1)),
        _ => Ok(None),
    }
}

pub(crate) fn prompt(question: &str) -> Result<String> {
    use std::io::Write;
    print!("{question}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("read answer")?;
    Ok(line.trim().to_string())
}

fn gh(args: &[String]) -> Result<String> {
    let out = std::process::Command::new("gh")
        .args(args)
        .output()
        .context("run gh")?;
    if !out.status.success() {
        anyhow::bail!(
            "gh {} failed: {}",
            args.first().map(String::as_str).unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn gh_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn gh_login() -> Result<String> {
    let login = gh(&gh_args(&["api", "user", "--jq", ".login"]))
        .context("`gh` is not logged in — run `gh auth login` first")?;
    if login.is_empty() {
        anyhow::bail!("`gh` returned no login — run `gh auth login` first");
    }
    Ok(login)
}

fn gh_repo_exists(full: &str) -> bool {
    gh(&gh_args(&[
        "api",
        &format!("repos/{full}"),
        "--jq",
        ".name",
    ]))
    .is_ok()
}

fn try_put_file(full: &str, path: &str, content: &str, message: &str) -> Result<()> {
    let existing_sha = gh(&gh_args(&[
        "api",
        &format!("repos/{full}/contents/{path}"),
        "--jq",
        ".sha",
    ]))
    .ok()
    .filter(|s| !s.is_empty());
    let mut args = gh_args(&[
        "api",
        "-X",
        "PUT",
        &format!("repos/{full}/contents/{path}"),
        "-f",
        &format!("message={message}"),
        "-f",
    ]);
    args.push(format!("content={}", base64(content.as_bytes())));
    if let Some(sha) = existing_sha {
        args.push("-f".to_string());
        args.push(format!("sha={sha}"));
    }
    gh(&args)?;
    Ok(())
}

/// Each attempt re-reads the file's current sha, so a PUT whose response was
/// lost in transit (server applied it, client saw a network error) succeeds
/// on the retry instead of failing the sha precondition.
fn gh_put_file(full: &str, path: &str, content: &str, message: &str) -> Result<()> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            eprintln!(
                "rtk: upload of {path} failed — retrying ({}/3)",
                attempt + 1
            );
            std::thread::sleep(std::time::Duration::from_secs(2 * attempt as u64));
        }
        match try_put_file(full, path, content, message) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("upload failed")))
        .with_context(|| format!("upload {path} to {full}"))
}

pub(crate) fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// `rtk pipeline share publish` — the interactive publisher. GitHub repo is
/// the recommended channel (searchable, starrable, forkable, one install
/// line); gists stay available for a single low-key link.
pub fn run_publish(name: Option<&str>, force: bool) -> Result<i32> {
    if !interactive() {
        anyhow::bail!(
            "`rtk pipeline share publish` is interactive — run it in a terminal \
             (the non-interactive path is `rtk pipeline share --gist`)"
        );
    }
    let Some(card) = crate::core::pipeline_cmd::collect_card(name)? else {
        return Ok(1);
    };
    if card.is_active {
        let loaded = crate::core::pipeline_cmd::load_for_cli();
        if has_errors(&loaded.diagnostics) {
            crate::core::pipeline_cmd::print_diagnostics(&loaded);
            anyhow::bail!(
                "`{}` has validation errors — fix them before publishing",
                card.display
            );
        }
    }
    println!("Publish «{}» publicly — where?", card.display);
    println!();
    let choice = select_menu(
        &[
            (
                "GitHub repository",
                "recommended — public, searchable, starrable, one install link",
            ),
            (
                "GitHub repository (private)",
                "team or personal use, only accounts with access see or install it",
            ),
            ("GitHub gist", "a single public link, no discovery"),
        ],
        0,
    )?;
    let card_md = card.card_md();
    match choice {
        Some(0) => publish_repo(&card, &card_md, force, false),
        Some(1) => publish_repo(&card, &card_md, force, true),
        Some(_) => {
            crate::core::pipeline_cmd::publish_gist(&card.display, &card.sources, &card_md, force)
        }
        None => {
            println!("nothing published");
            Ok(1)
        }
    }
}

fn deep_merge(base: &mut toml::Value, add: toml::Value) {
    match (base, add) {
        (toml::Value::Table(base), toml::Value::Table(add)) => {
            for (key, value) in add {
                match base.get_mut(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (toml::Value::Array(base), toml::Value::Array(add)) => base.extend(add),
        (base, add) => *base = add,
    }
}

/// One installable file from however many files the active pipeline spans:
/// tables merge recursively, rule arrays concatenate, `extends` is unioned —
/// the same spirit as the engine's own multi-file resolution. A single file
/// publishes verbatim (comments included). The merged result must validate
/// or nothing is published.
fn merge_sources(display: &str, sources: &[(String, String)]) -> Result<String> {
    // The same file installed twice under two names (a gist re-add, a copied
    // config) must not merge into duplicated rules — identical content
    // counts once.
    let mut seen = std::collections::BTreeSet::new();
    let sources: Vec<&(String, String)> = sources
        .iter()
        .filter(|(_, text)| seen.insert(text.trim().to_string()))
        .collect();
    if sources.is_empty() {
        anyhow::bail!("nothing to publish — no pipeline file found");
    }
    if sources.len() == 1 {
        let only = sources[0].1.trim_end();
        return Ok(format!("{only}\n"));
    }
    let mut merged = toml::Value::Table(toml::Table::new());
    for (label, text) in sources {
        let mut table = text
            .parse::<toml::Table>()
            .with_context(|| format!("parse {label}"))?;
        if let Some(pipeline) = table.get_mut("pipeline").and_then(|p| p.as_table_mut())
            && let Some(extends) = pipeline.get_mut("extends")
            && extends.is_str()
        {
            *extends = toml::Value::Array(vec![extends.clone()]);
        }
        deep_merge(&mut merged, toml::Value::Table(table));
    }
    if let Some(pipeline) = merged.get_mut("pipeline").and_then(|p| p.as_table_mut()) {
        pipeline.insert("name".to_string(), toml::Value::String(display.to_string()));
        if let Some(toml::Value::Array(extends)) = pipeline.get_mut("extends") {
            let mut seen = std::collections::BTreeSet::new();
            extends.retain(|e| {
                e.as_str()
                    .map(|s| seen.insert(s.to_string()))
                    .unwrap_or(true)
            });
        }
    }
    toml::to_string_pretty(&merged).context("serialize the merged pipeline")
}

fn publishable_toml(card: &crate::core::pipeline_cmd::CardData) -> Result<String> {
    let text = merge_sources(&card.display, &card.sources)?;
    let loaded = pipeline_config::load_candidate(&text, &card.display);
    if has_errors(&loaded.diagnostics) {
        crate::core::pipeline_cmd::print_diagnostics(&loaded);
        anyhow::bail!(
            "your pipeline spans {} files whose merge does not validate (errors above) — \
             fix them or consolidate into a single file before publishing",
            card.sources.len()
        );
    }
    Ok(text)
}

fn publish_repo(
    card: &crate::core::pipeline_cmd::CardData,
    card_md: &str,
    force: bool,
    private: bool,
) -> Result<i32> {
    if private {
        crate::core::pipeline_cmd::print_publish_warning_private();
    } else {
        crate::core::pipeline_cmd::print_publish_warning();
    }
    if !prompt("I understand — continue? [y/N] ")?.eq_ignore_ascii_case("y") {
        println!("not published");
        return Ok(1);
    }
    let toml_text = publishable_toml(card)?;

    let findings = crate::core::pipeline_cmd::scan_for_secrets(&card.sources);
    if !findings.is_empty() {
        let mut lines: Vec<String> = vec![
            "The pre-flight scan flagged content that may be sensitive:".to_string(),
            String::new(),
        ];
        lines.extend(findings.iter().map(|f| format!("  - {f}")));
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        crate::core::pipeline_cmd::warning_panel_err("Possible secrets or private data", &refs);
        if !force {
            eprintln!();
            let answer =
                prompt("type `publish anyway` to publish despite this, anything else aborts: ")?;
            if answer != "publish anyway" {
                println!("not published");
                return Ok(1);
            }
        }
    }

    if !crate::core::pipeline_cmd::gh_available() {
        println!("The `gh` CLI is not available, so rtk cannot create the repo for you.");
        println!("Publish manually:");
        println!(
            "  1. create a PUBLIC repo named {}",
            repo_slug(&card.display)
        );
        println!(
            "  2. add pipeline.toml (your pipeline file) and README.md (rtk pipeline share --md)"
        );
        println!("  3. add the repo topic `rtk-pipeline` so `rtk pipeline search` finds it");
        println!(
            "  4. share: rtk pipeline add github:<you>/{}",
            repo_slug(&card.display)
        );
        return Ok(0);
    }
    let login = gh_login()?;

    let mut repo = repo_slug(&card.display);
    if let Some(origin) = install_origin(&card.display)
        && let Some(src) = parse_github_source(&origin.source)
        && src.owner != login
    {
        println!();
        println!(
            "`{}` was installed from {} — a fork keeps the attribution chain visible on GitHub.",
            card.display,
            src.canonical()
        );
        match select_menu(
            &[
                (
                    "Fork the original",
                    "publish into your fork — attribution via GitHub's fork graph",
                ),
                ("Fresh repo", "a standalone repo under your account"),
            ],
            0,
        )? {
            Some(0) => {
                gh(&gh_args(&[
                    "repo",
                    "fork",
                    &format!("{}/{}", src.owner, src.repo),
                    "--clone=false",
                ]))
                .context("gh repo fork")?;
                repo = src.repo.clone();
            }
            Some(_) => {}
            None => {
                println!("not published");
                return Ok(1);
            }
        }
    }

    let full = format!("{login}/{repo}");
    let exists = gh_repo_exists(&full);
    let headline = card.headline();
    let desc = description(&login, &repo, headline.as_deref());

    println!();
    println!("This will leave your machine — nothing else does:");
    for (label, text) in &card.sources {
        println!("  - {} ({} bytes of pipeline TOML)", label, text.len());
    }
    println!(
        "  - the aggregate percentages from your share card (no commands, no paths, no history)"
    );
    let visibility = if private { "private" } else { "public" };
    println!();
    println!(
        "Repo:         https://github.com/{full}  ({})",
        if exists {
            "updates the existing repo, visibility unchanged"
        } else if private {
            "new private repo"
        } else {
            "new public repo"
        }
    );
    println!("Description:  {desc}");
    println!();
    let question = if exists {
        format!("Update repo {full}? [y/N] ")
    } else if private {
        format!("Create private repo {full}? Anyone you grant access can see and copy it. [y/N] ")
    } else {
        format!("Create public repo {full}? Public, permanent, indexable, yours. [y/N] ")
    };
    if !prompt(&question)?.eq_ignore_ascii_case("y") {
        println!("not published");
        return Ok(1);
    }

    if exists {
        gh(&gh_args(&["repo", "edit", &full, "--description", &desc])).context("gh repo edit")?;
    } else {
        gh(&gh_args(&[
            "repo",
            "create",
            &repo,
            &format!("--{visibility}"),
            "--description",
            &desc,
        ]))
        .context("gh repo create")?;
    }
    // The topic is the discovery channel — meaningless on a private repo,
    // where it would only suggest a listing that never happens.
    if !private
        && let Err(e) = gh(&gh_args(&[
            "repo",
            "edit",
            &full,
            "--add-topic",
            "rtk-pipeline",
        ]))
    {
        eprintln!("rtk: could not add the rtk-pipeline topic (search/leaderboard listing): {e}");
    }
    let message = if exists {
        "update pipeline"
    } else {
        "publish pipeline"
    };
    let upload = gh_put_file(&full, "pipeline.toml", &toml_text, message).and_then(|_| {
        gh_put_file(
            &full,
            "README.md",
            &repo_readme(card_md, &full, headline.as_deref()),
            message,
        )
    });
    if let Err(e) = upload {
        eprintln!("rtk: {e:#}");
        eprintln!();
        eprintln!("The repo exists but its files are incomplete. Re-run when the network is back:");
        eprintln!("  rtk pipeline share publish");
        eprintln!("It finds the same repo and finishes the upload — same link, no duplicate.");
        return Ok(1);
    }

    record_publish(&card.display, &format!("github:{full}"));
    println!();
    println!("published → https://github.com/{full}");
    println!("install:    rtk pipeline add github:{full}");
    if private {
        println!("            (private: installing needs `gh auth login` with access to the repo)");
        println!();
        println!("grant access: https://github.com/{full}/settings/access");
        return Ok(0);
    }
    println!();
    println!("Suggested post — copy, edit, share:");
    match &headline {
        Some(h) => println!(
            "  My rtk pipeline «{}»: {h}, measured on my own sessions.",
            card.display
        ),
        None => println!(
            "  My rtk pipeline «{}» — rules and numbers on the card.",
            card.display
        ),
    }
    println!("  Install: rtk pipeline add github:{full}");
    println!("  https://github.com/{full}  #rtk-pipeline");
    Ok(0)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '&' => out.push_str("%26"),
            '#' => out.push_str("%23"),
            '+' => out.push_str("%2B"),
            _ => out.push(c),
        }
    }
    out
}

/// `rtk pipeline search` — discovery is GitHub's own index of the
/// `rtk-pipeline` topic; the stars page is the leaderboard. RTK runs no
/// directory service and every description is the author's own claim.
pub fn run_search(query: Option<&str>) -> Result<i32> {
    let q = match query {
        Some(q) if !q.trim().is_empty() => format!("topic:rtk-pipeline {}", q.trim()),
        _ => "topic:rtk-pipeline".to_string(),
    };
    let path = format!(
        "search/repositories?q={}&sort=stars&order=desc&per_page=20",
        urlencode(&q)
    );
    let body = http_get(&format!("https://api.github.com/{path}"))
        .or_else(|_| gh(&gh_args(&["api", &path])))
        .context(
            "GitHub search is unreachable — browse https://github.com/topics/rtk-pipeline instead",
        )?;
    let v: serde_json::Value = serde_json::from_str(&body).context("parse search results")?;
    let items = v.get("items").and_then(|i| i.as_array());
    let Some(items) = items.filter(|i| !i.is_empty()) else {
        println!("no shared pipelines found for `{q}`");
        println!("browse: https://github.com/topics/rtk-pipeline");
        return Ok(0);
    };
    for item in items {
        let full = item
            .get("full_name")
            .and_then(|s| s.as_str())
            .unwrap_or("?");
        let stars = item
            .get("stargazers_count")
            .and_then(|s| s.as_i64())
            .unwrap_or(0);
        let desc = item
            .get("description")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let line = format!("{stars:>5}★  {full:<42} {desc}");
        println!("{}", crate::core::utils::sanitize_for_terminal(&line));
    }
    println!();
    println!(
        "install one:  rtk pipeline add github:<owner>/<repo>   (rules shown before anything installs)"
    );
    println!("descriptions and numbers are self-reported by their authors — rtk verifies nothing");
    println!("leaderboard:  https://github.com/topics/rtk-pipeline");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_source_accepts_scheme_url_and_ref() {
        let s = parse_github_source("github:alice/rtk-pipeline-fast").unwrap();
        assert_eq!(s.owner, "alice");
        assert_eq!(s.repo, "rtk-pipeline-fast");
        assert_eq!(s.reference, None);

        let s = parse_github_source("github:alice/repo@abc123").unwrap();
        assert_eq!(s.reference.as_deref(), Some("abc123"));

        let s = parse_github_source("https://github.com/alice/repo.git/").unwrap();
        assert_eq!(s.repo, "repo");
        assert_eq!(s.canonical(), "github:alice/repo");
    }

    #[test]
    fn parse_github_source_rejects_deep_paths_and_junk() {
        assert!(parse_github_source("https://github.com/a/b/blob/main/x.toml").is_none());
        assert!(parse_github_source("github:onlyowner").is_none());
        assert!(parse_github_source("github:a/b@").is_none());
        assert!(parse_github_source("github:a/b;rm -rf").is_none());
        assert!(parse_github_source("https://gist.github.com/a/b").is_none());
        assert!(parse_github_source("/local/path").is_none());
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn line_diff_shows_changes_with_context_and_elides_the_rest() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl";
        let new = "a\nb\nc\nd\ne\nf\nCHANGED\nh\ni\nj\nk\nl";
        let diff = line_diff(old, new);
        assert!(diff.contains("  - g\n"));
        assert!(diff.contains("  + CHANGED\n"));
        assert!(diff.contains("⋯"));
        assert!(!diff.contains("  a\n"));
    }

    #[test]
    fn line_diff_of_identical_text_has_no_change_markers() {
        let diff = line_diff("x\ny", "x\ny");
        assert!(!diff.contains("  - "));
        assert!(!diff.contains("  + "));
    }

    #[test]
    fn repo_slug_prefixes_and_sanitizes() {
        assert_eq!(repo_slug("fast-git"), "rtk-pipeline-fast-git");
        assert_eq!(repo_slug("rtk-pipeline-x"), "rtk-pipeline-x");
        assert_eq!(repo_slug("My Pipe!"), "rtk-pipeline-my-pipe");
        assert_eq!(repo_slug("rtk:proxy"), "rtk-pipeline-rtk-proxy");
    }

    #[test]
    fn description_carries_the_install_line_and_self_reported_label() {
        let d = description(
            "alice",
            "rtk-pipeline-x",
            Some("68% of command output removed"),
        );
        assert!(d.contains("self-reported"));
        assert!(d.contains("rtk pipeline add github:alice/rtk-pipeline-x"));
        let d = description("alice", "r", None);
        assert!(d.starts_with("RTK pipeline · rtk pipeline add"));
    }

    #[test]
    fn repo_readme_leads_with_install_and_responsibility() {
        let card = "# rtk pipeline · demo\n\n## What it does\n\n- stuff\n";
        let readme = repo_readme(card, "alice/rtk-pipeline-demo", Some("68% cut"));
        assert!(readme.starts_with("# rtk pipeline · demo\n"));
        assert!(readme.contains("rtk pipeline add github:alice/rtk-pipeline-demo"));
        assert!(readme.contains("self-reported"));
        assert!(readme.contains("does not review or endorse"));
        assert!(readme.contains("## What it does"));
    }

    #[test]
    fn merge_sources_combines_user_and_project_files_into_one_valid_pipeline() {
        let user = r#"
[pipeline]
name = "mine"
extends = "rtk:default"

[[hook.after_tool.shell]]
name = "tidy"
use  = ["ansi"]
"#;
        let project = r#"
[pipeline]
name = "proj"
extends = ["rtk:default", "rtk:proxy"]

[[hook.after_tool.shell]]
name = "squeeze"
use  = ["dedup"]
"#;
        let merged = merge_sources(
            "mine",
            &[
                ("user pipeline.toml".to_string(), user.to_string()),
                ("project pipeline.toml".to_string(), project.to_string()),
            ],
        )
        .unwrap();
        let table: toml::Table = merged.parse().unwrap();
        let pipeline = table["pipeline"].as_table().unwrap();
        assert_eq!(pipeline["name"].as_str(), Some("mine"));
        let extends: Vec<&str> = pipeline["extends"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.as_str())
            .collect();
        assert_eq!(extends, vec!["rtk:default", "rtk:proxy"]);
        assert!(merged.contains("tidy") && merged.contains("squeeze"));
        let loaded = pipeline_config::load_candidate(&merged, "mine");
        assert!(
            !has_errors(&loaded.diagnostics),
            "merged pipeline must validate: {:?}",
            loaded.diagnostics
        );
    }

    #[test]
    fn merge_sources_counts_identical_files_once() {
        let text = "# kept verbatim because the twin is identical\n[pipeline]\nname = \"solo\"\n";
        let merged = merge_sources(
            "solo",
            &[
                ("user pipeline.toml".to_string(), text.to_string()),
                ("user d66c800b.toml".to_string(), text.to_string()),
            ],
        )
        .unwrap();
        assert!(merged.contains("# kept verbatim because the twin is identical"));
    }

    #[test]
    fn merge_sources_keeps_a_single_file_verbatim() {
        let text = "# a comment that must survive\n[pipeline]\nname = \"solo\"\n";
        let merged = merge_sources(
            "solo",
            &[("user pipeline.toml".to_string(), text.to_string())],
        )
        .unwrap();
        assert!(merged.contains("# a comment that must survive"));
    }

    #[test]
    fn ledger_upsert_merges_without_erasing_other_keys() {
        let mut table = toml::Table::new();
        ledger_upsert(
            &mut table,
            "demo",
            &[
                ("source", "github:a/b".to_string()),
                ("sha", "x".to_string()),
            ],
        );
        ledger_upsert(
            &mut table,
            "demo",
            &[("published", "github:me/rtk-pipeline-demo".to_string())],
        );
        ledger_upsert(&mut table, "demo", &[("sha", "y".to_string())]);
        let entry = table.get("demo").unwrap().as_table().unwrap();
        assert_eq!(entry.get("source").unwrap().as_str(), Some("github:a/b"));
        assert_eq!(entry.get("sha").unwrap().as_str(), Some("y"));
        assert_eq!(
            entry.get("published").unwrap().as_str(),
            Some("github:me/rtk-pipeline-demo")
        );
    }

    #[test]
    fn urlencode_escapes_query_metacharacters() {
        assert_eq!(
            urlencode("topic:rtk-pipeline fast & lean"),
            "topic:rtk-pipeline%20fast%20%26%20lean"
        );
    }
}
