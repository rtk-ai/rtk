//! `rtk init` without an agent. In a terminal it shows a checkbox menu of the
//! agents the chosen scope supports, then (with `-g`) the reviewed-pipeline
//! menu, then runs each picked agent's own setup. Every other run sets up
//! Claude Code, as bare `rtk init` always did, and says how to pick another.

use super::{FilterTrust, InitContext, PatchMode};
use crate::core::user_dirs;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitTarget {
    Claude,
    Codex,
    Cursor,
    Gemini,
    Copilot,
    OpenCode,
    Windsurf,
    Cline,
    Kilocode,
    Kimi,
    Antigravity,
    Trae,
    Pi,
    Omp,
    Hermes,
    Droid,
    Vibe,
}

const ALL: [InitTarget; 17] = [
    InitTarget::Claude,
    InitTarget::Codex,
    InitTarget::Cursor,
    InitTarget::Gemini,
    InitTarget::Copilot,
    InitTarget::OpenCode,
    InitTarget::Windsurf,
    InitTarget::Cline,
    InitTarget::Kilocode,
    InitTarget::Kimi,
    InitTarget::Antigravity,
    InitTarget::Trae,
    InitTarget::Pi,
    InitTarget::Omp,
    InitTarget::Hermes,
    InitTarget::Droid,
    InitTarget::Vibe,
];

impl InitTarget {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Gemini => "Gemini CLI",
            Self::Copilot => "GitHub Copilot",
            Self::OpenCode => "OpenCode",
            Self::Windsurf => "Windsurf",
            Self::Cline => "Cline / Roo Code",
            Self::Kilocode => "Kilo Code",
            Self::Kimi => "Kimi AI",
            Self::Antigravity => "Google Antigravity",
            Self::Trae => "Trae",
            Self::Pi => "Pi",
            Self::Omp => "Oh My Pi",
            Self::Hermes => "Hermes",
            Self::Droid => "Factory Droid",
            Self::Vibe => "Mistral Vibe",
        }
    }

    pub fn supports(self, global: bool) -> bool {
        match self {
            Self::Claude
            | Self::Codex
            | Self::Copilot
            | Self::Antigravity
            | Self::Trae
            | Self::Pi
            | Self::Omp
            | Self::Hermes
            | Self::Droid => true,
            Self::Cursor | Self::Gemini | Self::OpenCode | Self::Windsurf | Self::Vibe => global,
            Self::Cline | Self::Kilocode | Self::Kimi => !global,
        }
    }

    fn config_dir(self) -> Option<PathBuf> {
        let home = user_dirs::home()?;
        Some(match self {
            Self::Claude => super::resolve_claude_dir().ok()?,
            Self::Codex => user_dirs::env_path("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
            Self::Copilot => user_dirs::env_path("COPILOT_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".copilot")),
            Self::Hermes => user_dirs::env_path("HERMES_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".hermes")),
            Self::Cursor => home.join(".cursor"),
            Self::Gemini => home.join(".gemini"),
            Self::OpenCode => home.join(".config").join("opencode"),
            Self::Windsurf => home.join(".codeium").join("windsurf"),
            Self::Trae => home.join(".trae"),
            Self::Pi => home.join(".pi"),
            Self::Omp => home.join(".omp"),
            Self::Droid => home.join(".factory"),
            Self::Vibe => home.join(".vibe"),
            Self::Cline | Self::Kilocode | Self::Kimi | Self::Antigravity => return None,
        })
    }

    fn detected(self) -> Option<PathBuf> {
        self.config_dir().filter(|dir| dir.is_dir())
    }
}

pub fn guided_allowed(explicit_agent: bool, patch_mode: PatchMode, dry_run: bool) -> bool {
    !explicit_agent
        && matches!(patch_mode, PatchMode::Ask)
        && !dry_run
        && crate::core::ask::interactive()
}

pub fn print_default_agent_hint(global: bool) {
    let scope = if global { " -g" } else { "" };
    eprintln!(
        "No agent given: setting up Claude Code. Choose one with `rtk init{scope} --agent <name>` \
         (see `rtk init --help`), or run `rtk init{scope}` in a terminal to pick from a menu."
    );
}

pub fn run_guided(
    global: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
    filter_trust: FilterTrust,
    pipeline: Option<&str>,
) -> Result<i32> {
    let targets: Vec<InitTarget> = ALL.into_iter().filter(|t| t.supports(global)).collect();
    let found: Vec<Option<PathBuf>> = targets.iter().map(|t| t.detected()).collect();
    let hints: Vec<String> = found
        .iter()
        .map(|dir| {
            dir.as_deref()
                .map(|d| format!("detected ({})", tilde(d)))
                .unwrap_or_default()
        })
        .collect();
    let items: Vec<(&str, &str)> = targets
        .iter()
        .zip(&hints)
        .map(|(t, hint)| (t.label(), hint.as_str()))
        .collect();
    let preselected: Vec<bool> = found.iter().map(Option::is_some).collect();
    let question = if global {
        "Which agents should RTK set up for your user account?"
    } else {
        "Which agents should RTK set up in this project?"
    };
    println!();
    let picked = match crate::core::ask::pick_many(question, &items, &preselected)? {
        Some(picked) if !picked.is_empty() => picked,
        _ => {
            let scope = if global { " -g" } else { "" };
            println!("Nothing set up. Pick one directly with `rtk init{scope} --agent <name>`.");
            return Ok(1);
        }
    };

    #[cfg(feature = "engine")]
    if global || pipeline.is_some() {
        pipeline_step(pipeline, patch_mode, ctx)?;
    }
    #[cfg(not(feature = "engine"))]
    if pipeline.is_some() {
        anyhow::bail!("--pipeline needs the full edition of rtk");
    }

    let mut shared_path = false;
    let mut failed: Vec<&str> = Vec::new();
    let several = picked.len() > 1;
    for target in picked.into_iter().map(|i| targets[i]) {
        if several {
            use colored::Colorize;
            println!("\n{}", format!("▸ {}", target.label()).bold());
        }
        match install(target, global, hook_only, patch_mode, ctx) {
            Ok(shared) => shared_path |= shared,
            Err(e) => {
                eprintln!("rtk: {} setup failed: {e:#}", target.label());
                failed.push(target.label());
            }
        }
    }
    if shared_path {
        super::finalize_filter_trust(global, ctx.dry_run, filter_trust)?;
    }
    if !failed.is_empty() {
        anyhow::bail!("setup failed for {}", failed.join(", "));
    }
    Ok(0)
}

fn install(
    target: InitTarget,
    global: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    let shared = |claude: bool,
                  opencode: bool,
                  cursor: bool,
                  windsurf: bool,
                  cline: bool,
                  codex: bool,
                  hook_only: bool| {
        super::run(
            global, claude, opencode, cursor, windsurf, cline, false, hook_only, codex, patch_mode,
            ctx,
        )
        .map(|()| true)
    };
    match target {
        InitTarget::Claude => shared(true, false, false, false, false, false, hook_only),
        InitTarget::OpenCode => shared(false, true, false, false, false, false, false),
        InitTarget::Cursor => shared(false, false, true, false, false, false, false),
        InitTarget::Windsurf => shared(false, false, false, true, false, false, false),
        InitTarget::Cline => shared(false, false, false, false, true, false, false),
        InitTarget::Codex => shared(false, false, false, false, false, true, false),
        InitTarget::Gemini => super::run_gemini(global, hook_only, patch_mode, ctx).map(|()| false),
        InitTarget::Copilot if global => super::run_copilot_global(ctx).map(|()| false),
        InitTarget::Copilot => super::run_copilot(ctx).map(|()| false),
        InitTarget::Pi => {
            super::run_pi_mode_with_patch_mode(global, patch_mode, ctx).map(|()| false)
        }
        InitTarget::Omp => {
            super::run_omp_mode_with_patch_mode(global, patch_mode, ctx).map(|()| false)
        }
        InitTarget::Trae => super::run_trae_mode(global, ctx).map(|()| false),
        InitTarget::Kilocode => super::run_kilocode_mode(ctx).map(|()| false),
        InitTarget::Kimi => super::run_kimi_mode(ctx).map(|()| false),
        InitTarget::Antigravity => super::run_antigravity_mode(global, ctx).map(|()| false),
        InitTarget::Hermes => super::run_hermes_mode(ctx).map(|()| false),
        InitTarget::Droid => super::run_droid_mode(global, ctx).map(|()| false),
        InitTarget::Vibe => {
            super::run_vibe_mode(global, hook_only, patch_mode, ctx).map(|()| false)
        }
    }
}

fn tilde(path: &Path) -> String {
    match user_dirs::home().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(feature = "engine")]
fn onramp_disabled() -> bool {
    crate::core::user_env::var("RTK_ONRAMP_DISABLED").as_deref() == Some("1")
}

#[cfg(feature = "engine")]
pub fn pipeline_step(choice: Option<&str>, patch_mode: PatchMode, ctx: InitContext) -> Result<()> {
    use crate::core::pipeline_catalog::{self, Origin};
    if let Some(name) = choice {
        let catalog = pipeline_catalog::load();
        if catalog.origin == Origin::Missing {
            anyhow::bail!(
                "--pipeline {name}: the pipeline catalog ({}) could not be reached and no earlier copy is saved",
                pipeline_catalog::CATALOG_URL
            );
        }
        let Some(entry) = pipeline_catalog::find(&catalog.entries, name) else {
            let names: Vec<&str> = catalog.entries.iter().map(|e| e.name.as_str()).collect();
            anyhow::bail!(
                "--pipeline {name}: not a reviewed pipeline. Reviewed: {} ({})",
                names.join(", "),
                pipeline_catalog::CATALOG_PAGE
            );
        };
        return switch_pipeline(entry, &catalog.entries, ctx);
    }
    if !matches!(patch_mode, PatchMode::Ask)
        || ctx.dry_run
        || !crate::core::ask::interactive()
        || onramp_disabled()
    {
        return Ok(());
    }
    pipeline_menu(ctx)
}

#[cfg(feature = "engine")]
fn pipeline_menu(ctx: InitContext) -> Result<()> {
    use crate::core::pipeline_catalog::{self, DEFAULT_PIPELINE, Origin};
    let catalog = pipeline_catalog::load();
    println!();
    if catalog.entries.is_empty() {
        let mine = !custom_pipelines(&[]).is_empty();
        let now = if mine {
            "Pipeline unchanged".to_string()
        } else {
            format!("Pipeline: {DEFAULT_PIPELINE}, RTK's default")
        };
        println!(
            "  {now}. The catalog at {} could not be reached;",
            pipeline_catalog::CATALOG_PAGE
        );
        println!("  pick another later with `rtk init -g --pipeline <name>`.");
        return Ok(());
    }
    let mut entries: Vec<&pipeline_catalog::Entry> = catalog.entries.iter().collect();
    entries.sort_by_key(|e| !e.is_default());
    let installed = catalog_installs(&catalog.entries);
    let custom = custom_pipelines(&installed);
    let is_current = |e: &pipeline_catalog::Entry| {
        installed.iter().any(|(_, id)| *id == e.id)
            || (e.is_default() && installed.is_empty() && custom.is_empty())
    };

    let width = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let note = if catalog.origin == Origin::Cached {
        " (saved copy, the site could not be reached)"
    } else {
        ""
    };
    println!(
        "  Reviewed pipelines from {}{note}:",
        pipeline_catalog::CATALOG_PAGE
    );
    for e in &entries {
        let summary = if e.is_default() {
            "RTK's default: compresses shell command output. Nothing to install."
        } else {
            e.summary.as_str()
        };
        println!("    {:<width$}  {summary}", e.name);
    }
    println!();

    let mut hints: Vec<String> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    if !custom.is_empty() {
        labels.push("Keep my pipelines".to_string());
        hints.push(format!("current · {}", custom.join(", ")));
    }
    for e in &entries {
        let mut parts: Vec<String> = Vec::new();
        if is_current(e) {
            parts.push("current".to_string());
        }
        if let Some(saves) = e.saves() {
            parts.push(saves);
        }
        parts.push(e.needs());
        labels.push(e.name.clone());
        hints.push(parts.join(" · "));
    }
    let label_width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    for label in &mut labels {
        let pad = label_width - label.chars().count();
        label.push_str(&" ".repeat(pad));
    }
    let items: Vec<(&str, &str)> = labels
        .iter()
        .zip(&hints)
        .map(|(l, h)| (l.as_str(), h.as_str()))
        .collect();
    let offset = usize::from(!custom.is_empty());
    let default = if custom.is_empty() {
        entries.iter().position(|e| is_current(e)).unwrap_or(0)
    } else {
        0
    };
    let Some(choice) = crate::core::ask::choose(
        "  Which pipeline should RTK run? (installed pinned to the reviewed commit)",
        &items,
        default,
    )?
    else {
        println!("  Pipeline unchanged.");
        return Ok(());
    };
    if choice < offset {
        println!("  Pipeline unchanged.");
        return Ok(());
    }
    switch_pipeline(entries[choice - offset], &catalog.entries, ctx)
}

#[cfg(feature = "engine")]
fn catalog_installs(entries: &[crate::core::pipeline_catalog::Entry]) -> Vec<(String, String)> {
    let Some(dir) = crate::core::pipeline_config::user_pipelines_dir() else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("toml"))
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_string();
            let origin = crate::core::pipeline_remote::install_origin(&name)?;
            let entry = entries.iter().find(|e| e.canonical() == origin.source)?;
            Some((name, entry.id.clone()))
        })
        .collect();
    out.sort();
    out
}

#[cfg(feature = "engine")]
fn custom_pipelines(catalog: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    if crate::core::pipeline_config::user_pipeline_path().is_some_and(|p| p.exists()) {
        out.push(crate::core::constants::PIPELINE_TOML.to_string());
    }
    if let Some(dir) = crate::core::pipeline_config::user_pipelines_dir()
        && let Ok(read) = std::fs::read_dir(&dir)
    {
        let mut names: Vec<String> = read
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("toml"))
            .filter_map(|p| p.file_stem()?.to_str().map(str::to_string))
            .filter(|name| !catalog.iter().any(|(n, _)| n == name))
            .collect();
        names.sort();
        out.extend(names);
    }
    out
}

#[cfg(feature = "engine")]
fn switch_pipeline(
    entry: &crate::core::pipeline_catalog::Entry,
    entries: &[crate::core::pipeline_catalog::Entry],
    ctx: InitContext,
) -> Result<()> {
    let installed = catalog_installs(entries);
    let others: Vec<&(String, String)> =
        installed.iter().filter(|(_, id)| *id != entry.id).collect();
    let already = installed.iter().any(|(_, id)| *id == entry.id);
    let Some(source) = entry.source() else {
        anyhow::bail!("{} has no pinned install source", entry.name);
    };
    if ctx.dry_run {
        for (name, _) in &others {
            println!("[dry-run] would remove pipeline {name}");
        }
        if !already && !entry.is_default() {
            println!("[dry-run] would install pipeline {} ({source})", entry.name);
        }
        return Ok(());
    }
    if let Some(dir) = crate::core::pipeline_config::user_pipelines_dir() {
        for (name, id) in &others {
            let path = dir.join(format!("{name}.toml"));
            std::fs::remove_file(&path)
                .map_err(|e| anyhow::anyhow!("remove {}: {e}", path.display()))?;
            let readd = entries
                .iter()
                .find(|e| e.id == *id)
                .map_or(id.as_str(), |e| e.name.as_str());
            println!("  Removed pipeline {name} (re-add: rtk init -g --pipeline {readd})");
        }
    }
    if already {
        println!("  Pipeline: {} (already installed)", entry.name);
        return Ok(());
    }
    if entry.is_default() {
        println!(
            "  Pipeline: {}, RTK's default. Nothing to install.",
            entry.name
        );
        return Ok(());
    }
    let code = crate::core::pipeline_cmd::install_from_init(source)?;
    if code != 0 {
        anyhow::bail!("pipeline {} was not installed", entry.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_at_least_one_scope() {
        for target in ALL {
            assert!(
                target.supports(true) || target.supports(false),
                "{target:?}"
            );
        }
    }

    #[test]
    fn scope_rules_match_the_existing_guards() {
        assert!(!InitTarget::Kilocode.supports(true));
        assert!(!InitTarget::Kimi.supports(true));
        assert!(!InitTarget::Gemini.supports(false));
        assert!(!InitTarget::Vibe.supports(false));
        assert!(!InitTarget::Cursor.supports(false));
        assert!(!InitTarget::OpenCode.supports(false));
        assert!(!InitTarget::Windsurf.supports(false));
        assert!(InitTarget::Claude.supports(true) && InitTarget::Claude.supports(false));
    }

    #[test]
    fn guided_menu_only_for_bare_interactive_ask_runs() {
        assert!(!guided_allowed(true, PatchMode::Ask, false));
        assert!(!guided_allowed(false, PatchMode::Auto, false));
        assert!(!guided_allowed(false, PatchMode::Skip, false));
        assert!(!guided_allowed(false, PatchMode::Ask, true));
    }
}
