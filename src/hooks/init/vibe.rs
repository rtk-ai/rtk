//! Vibe agent: hook install/uninstall helpers.

use super::*;
use crate::hooks::constants::{
    VIBE_BASH_MATCH, VIBE_DIR, VIBE_HOOK_COMMAND, VIBE_HOOK_NAME, VIBE_HOOKS_FILE,
    VIBE_POSTHOOK_NAME, VIBE_PROMPT_FILE, VIBE_PROMPTS_SUBDIR,
};

// Vibe integration

fn resolve_vibe_dir() -> Result<PathBuf> {
    resolve_home_subdir(VIBE_DIR)
}

pub(crate) const VIBE_PROXY_PATH: &str = "/mistral/v1";
const VIBE_CONFIG_FILE: &str = "config.toml";
const MISTRAL_PROVIDER: &str = "mistral";
const MISTRAL_API_BASE_DEFAULT: &str = "https://api.mistral.ai/v1";
#[cfg(any(feature = "engine", test))]
const MISTRAL_API_KEY_ENV: &str = "MISTRAL_API_KEY";
const RTK_PROVIDER_KEYS: &[&str] = &["name", "api_base", "api_key_env_var", "backend"];

pub(crate) fn mistral_api_base() -> Option<String> {
    let path = resolve_vibe_dir().ok()?.join(VIBE_CONFIG_FILE);
    let text = fs::read_to_string(path).ok()?;
    mistral_api_base_in(&text)
}

fn is_mistral(table: &toml_edit::Table) -> bool {
    table.get("name").and_then(toml_edit::Item::as_str) == Some(MISTRAL_PROVIDER)
}

fn mistral_api_base_in(text: &str) -> Option<String> {
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    doc.get("providers")?
        .as_array_of_tables()?
        .iter()
        .find(|table| is_mistral(table))?
        .get("api_base")?
        .as_str()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

#[cfg(any(feature = "engine", test))]
fn set_mistral_api_base(text: &str, url: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("parse Vibe config.toml")?;
    if doc
        .get("providers")
        .is_some_and(|item| item.as_array_of_tables().is_none())
    {
        anyhow::bail!(
            "`providers` in config.toml is not a [[providers]] table array; set the mistral api_base by hand"
        );
    }
    if doc.get("providers").is_none() {
        doc["providers"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let providers = doc
        .get_mut("providers")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .context("providers table")?;
    let existing = providers.iter().position(is_mistral);
    match existing {
        Some(idx) => {
            if let Some(table) = providers.get_mut(idx) {
                table["api_base"] = toml_edit::value(url);
            }
        }
        None => {
            let mut table = toml_edit::Table::new();
            table["name"] = toml_edit::value(MISTRAL_PROVIDER);
            table["api_base"] = toml_edit::value(url);
            table["api_key_env_var"] = toml_edit::value(MISTRAL_API_KEY_ENV);
            table["backend"] = toml_edit::value(MISTRAL_PROVIDER);
            providers.push(table);
        }
    }
    Ok(doc.to_string())
}

fn remove_rtk_mistral_provider(text: &str, rtk_url: &str) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().ok()?;
    let providers = doc
        .get_mut("providers")
        .and_then(toml_edit::Item::as_array_of_tables_mut)?;
    let idx = providers.iter().position(is_mistral)?;
    let ours_only = {
        let table = providers.get(idx)?;
        let api_base = table.get("api_base").and_then(toml_edit::Item::as_str)?;
        if api_base.trim().trim_end_matches('/') != rtk_url.trim_end_matches('/') {
            return None;
        }
        table
            .iter()
            .all(|(key, _)| RTK_PROVIDER_KEYS.contains(&key))
    };
    if ours_only {
        providers.remove(idx);
        let empty = providers.is_empty();
        if empty {
            doc.remove("providers");
        }
    } else if let Some(table) = providers.get_mut(idx) {
        table["api_base"] = toml_edit::value(MISTRAL_API_BASE_DEFAULT);
    }
    Some(doc.to_string())
}

#[cfg(feature = "engine")]
const VIBE_SPEC: super::proxy_onramp::OnrampSpec = super::proxy_onramp::OnrampSpec {
    label: "Mistral Vibe",
    config: "config.toml",
    key: "providers[mistral].api_base",
    path: VIBE_PROXY_PATH,
    manual_option: "Neither — print the provider entry; config.toml is not touched",
    autostart_option: None,
    autostart_done: "",
    autostart_persistence: "",
};

#[cfg(feature = "engine")]
struct VibeOnramp {
    config_path: PathBuf,
}

#[cfg(feature = "engine")]
impl VibeOnramp {
    fn text(&self) -> Result<String> {
        if !self.config_path.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(&self.config_path)
            .with_context(|| format!("Failed to read {}", self.config_path.display()))
    }
}

#[cfg(feature = "engine")]
impl super::proxy_onramp::ProxyOnramp for VibeOnramp {
    fn spec(&self) -> &'static super::proxy_onramp::OnrampSpec {
        &VIBE_SPEC
    }

    fn current_target(&self) -> Result<Option<String>> {
        Ok(mistral_api_base_in(&self.text()?))
    }

    fn autostart_present(&self) -> Result<bool> {
        Ok(false)
    }

    fn manual_line(&self, base_url: &str) -> String {
        format!(
            "[[providers]]\n    name = \"{MISTRAL_PROVIDER}\"\n    api_base = \"{base_url}\"\n    api_key_env_var = \"{MISTRAL_API_KEY_ENV}\"\n    backend = \"{MISTRAL_PROVIDER}\""
        )
    }

    fn write(&self, base_url: &str, _autostart: bool) -> Result<Vec<String>> {
        let updated = set_mistral_api_base(&self.text()?, base_url)?;
        let backup = backup_and_atomic_write(&self.config_path, &updated)?;
        let mut lines = vec![format!(
            "  config.toml: providers[mistral].api_base = {base_url}"
        )];
        if let Some(path) = backup {
            lines.push(format!("  Backup: {}", path.display()));
        }
        Ok(lines)
    }
}

/// Entry point for `rtk init -g --agent vibe`.
///
/// Installs a `pre_tool` hook into `~/.vibe/hooks.toml` (Vibe CLI's hook
/// registry, see <https://docs.mistral.ai/vibe/code/cli/hooks>) that routes
/// bash tool calls through the native `rtk hook vibe` binary. When not
/// `hook_only`, also drops an `~/.vibe/prompts/rtk.md` system prompt file
/// as a belt-and-suspenders fallback if the hook is disabled.
pub fn run_vibe_mode(
    global: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    if !global {
        anyhow::bail!("Vibe support is global-only. Use: rtk init -g --agent vibe");
    }
    let vibe_dir = resolve_vibe_dir()?;
    run_vibe_mode_at(&vibe_dir, hook_only, patch_mode, ctx)
}

fn run_vibe_mode_at(
    vibe_dir: &Path,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !dry_run {
        fs::create_dir_all(vibe_dir)
            .with_context(|| format!("Failed to create Vibe config dir: {}", vibe_dir.display()))?;
    }

    let hooks_path = vibe_dir.join(VIBE_HOOKS_FILE);
    let (entries, skipped) = vibe_entries();
    let hook_outcome = patch_vibe_hooks_toml(&hooks_path, &entries, patch_mode, ctx)?;

    if !hook_only {
        let prompts_dir = vibe_dir.join(VIBE_PROMPTS_SUBDIR);
        if !dry_run {
            fs::create_dir_all(&prompts_dir).with_context(|| {
                format!("Failed to create prompts dir: {}", prompts_dir.display())
            })?;
        }
        let prompt_path = prompts_dir.join(VIBE_PROMPT_FILE);
        write_if_changed(
            &prompt_path,
            awareness_content(ctx.awareness),
            VIBE_PROMPT_FILE,
            ctx,
        )?;
    }

    if dry_run {
        for line in &skipped {
            println!("[dry-run] {line}");
        }
        print_dry_run_footer();
    } else if let Some(summary_verb) = hook_outcome.summary_verb() {
        println!("\nMistral Vibe CLI hook {summary_verb} (global).\n");
        println!("  Hook registry: {}", hooks_path.display());
        if !hook_only {
            println!(
                "  Prompt: {}",
                vibe_dir
                    .join(VIBE_PROMPTS_SUBDIR)
                    .join(VIBE_PROMPT_FILE)
                    .display()
            );
        }
        println!("  Restart Vibe. Test with: git status\n");
        for line in &skipped {
            println!("  {line}");
        }
    }
    #[cfg(feature = "engine")]
    if !dry_run && hook_outcome != VibeHookPatchOutcome::Skipped {
        let onramp = VibeOnramp {
            config_path: vibe_dir.join(VIBE_CONFIG_FILE),
        };
        if let Err(e) = super::proxy_onramp::offer_proxy_onramp(&onramp, patch_mode, ctx) {
            eprintln!("  [warn] proxy on-ramp skipped: {e}");
        }
    }
    Ok(())
}

#[cfg(feature = "engine")]
fn vibe_entries() -> (Vec<super::plan::Entry>, Vec<String>) {
    match super::plan::plan_for("vibe", true) {
        Some(plan) => (plan.entries(), plan.skipped_lines()),
        None => (legacy_vibe_entries(), Vec::new()),
    }
}

#[cfg(not(feature = "engine"))]
fn vibe_entries() -> (Vec<(&'static str, Option<String>)>, Vec<String>) {
    (legacy_vibe_entries(), Vec::new())
}

fn legacy_vibe_entries() -> Vec<(&'static str, Option<String>)> {
    vec![("pre_tool", Some(VIBE_BASH_MATCH.to_string()))]
}

fn vibe_hook_name(event: &str) -> &'static str {
    if event == "post_tool" {
        VIBE_POSTHOOK_NAME
    } else {
        VIBE_HOOK_NAME
    }
}

/// Outcome of `patch_vibe_hooks_toml`. Distinguishes installed / already-present /
/// skipped so the caller can decide whether the "installed" summary is truthful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VibeHookPatchOutcome {
    Installed,
    AlreadyPresent,
    Skipped,
}

impl VibeHookPatchOutcome {
    fn summary_verb(self) -> Option<&'static str> {
        match self {
            Self::Installed => Some("installed"),
            Self::AlreadyPresent => Some("already present"),
            Self::Skipped => None,
        }
    }
}

/// Append the RTK `[[hooks]]` entry to `~/.vibe/hooks.toml` if not already present.
///
/// Uses append-based patching (string level) rather than parse-serialize round-trip
/// to preserve any user comments and formatting in the file.
fn patch_vibe_hooks_toml(
    hooks_path: &Path,
    entries: &[(&'static str, Option<String>)],
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<VibeHookPatchOutcome> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;

    let existing = if hooks_path.exists() {
        fs::read_to_string(hooks_path)
            .with_context(|| format!("Failed to read {}", hooks_path.display()))?
    } else {
        String::new()
    };

    let wanted: Vec<(&'static str, String)> = entries
        .iter()
        .map(|(event, matcher)| {
            (
                vibe_hook_name(event),
                vibe_hook_entry(event, matcher.as_deref()),
            )
        })
        .collect();
    let stale: Vec<&str> = wanted
        .iter()
        .filter(|(name, text)| {
            vibe_hooks_toml_has_entry(&existing, name) && !existing.contains(text.as_str())
        })
        .map(|(name, _)| *name)
        .collect();
    let missing: Vec<&(&'static str, String)> = wanted
        .iter()
        .filter(|(name, _)| !vibe_hooks_toml_has_entry(&existing, name) || stale.contains(name))
        .collect();
    if missing.is_empty() {
        if verbose > 0 {
            eprintln!("Vibe hooks.toml already has RTK hook");
        }
        return Ok(VibeHookPatchOutcome::AlreadyPresent);
    }

    if patch_mode == PatchMode::Skip {
        println!(
            "\nManual setup needed: add RTK hook to {}\n\
             See: https://www.rtk-ai.app/guide/getting-started/supported-agents#mistral-vibe",
            hooks_path.display()
        );
        return Ok(VibeHookPatchOutcome::Skipped);
    }

    if patch_mode == PatchMode::Ask {
        if dry_run {
            println!(
                "[dry-run] would prompt before patching {}",
                hooks_path.display()
            );
        } else {
            print!("Patch {} with RTK hook? [y/N] ", hooks_path.display());
            std::io::stdout().flush().ok();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).ok();
            if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                println!(
                    "Skipped. Re-run with --auto-patch, or add the hook manually to {}",
                    hooks_path.display()
                );
                return Ok(VibeHookPatchOutcome::Skipped);
            }
        }
    }

    let mut existing = existing;
    for name in &stale {
        if let Some(stripped) = strip_vibe_entry(&existing, name) {
            existing = stripped;
        }
    }
    let entry = missing
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let new_content = if existing.trim().is_empty() {
        entry.clone()
    } else if existing.ends_with("\n\n") {
        format!("{existing}{entry}")
    } else if existing.ends_with('\n') {
        format!("{existing}\n{entry}")
    } else {
        format!("{existing}\n\n{entry}")
    };

    if dry_run {
        println!(
            "[dry-run] would patch Vibe hooks.toml: {}",
            hooks_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] appended entry:\n{entry}");
        }
    } else {
        atomic_write(hooks_path, &new_content)
            .with_context(|| format!("Failed to write {}", hooks_path.display()))?;
    }
    Ok(VibeHookPatchOutcome::Installed)
}

fn vibe_hook_entry(event: &str, matcher: Option<&str>) -> String {
    let description = if event == "post_tool" {
        "Compress tool output through rtk before the model reads it."
    } else {
        "Rewrite bash commands through rtk to save tokens."
    };
    format!(
        r#"[[hooks]]
name = "{name}"
type = "{event}"
match = '{matcher}'
command = "{command}"
timeout = 10.0
strict = false
description = "{description}"
"#,
        name = vibe_hook_name(event),
        matcher = matcher.unwrap_or("*"),
        command = VIBE_HOOK_COMMAND,
    )
}

/// Detect an existing RTK entry by looking for the hook `name` field. Scanning
/// the raw string is enough because `name` is required by Vibe and must be
/// unique, so a substring match is both necessary and sufficient.
///
/// Tradeoff: matches the exact spacing `name = "rtk-rewrite"`. A reformatted
/// file (`name="rtk-rewrite"` or extra whitespace) would defeat idempotency
/// and cause a duplicate append on re-install. Acceptable because our own
/// installer only ever writes the canonical spacing, and the alternative
/// (parse-serialize round-trip via toml_edit) would clobber user comments
/// and formatting in the file.
fn vibe_hooks_toml_has_entry(content: &str, name: &str) -> bool {
    let needle = format!(r#"name = "{name}""#);
    content.contains(&needle)
}

#[cfg(test)]
fn vibe_hooks_toml_has_rtk(content: &str) -> bool {
    vibe_hooks_toml_has_entry(content, VIBE_HOOK_NAME)
}

/// Public entry point for `rtk init -g --agent vibe --uninstall`.
pub fn uninstall_vibe(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let vibe_dir = match resolve_vibe_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("RTK Vibe uninstall skipped: could not resolve ~/.vibe/ ({e})");
            return Ok(());
        }
    };
    let removed = uninstall_vibe_at(&vibe_dir, ctx)?;

    if removed.is_empty() {
        println!("RTK Vibe support was not installed (nothing to remove)");
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Mistral Vibe CLI:"
        } else {
            "RTK uninstalled for Mistral Vibe CLI:"
        };
        println!("{}", header);
        for item in removed {
            println!("  - {}", item);
        }
        if !dry_run {
            println!("\nRestart Vibe CLI to apply changes.");
        }
    }

    if dry_run {
        print_dry_run_footer();
    }
    Ok(())
}

/// Remove the RTK hook entry (and, when non-empty, the surrounding blank
/// lines) from `~/.vibe/hooks.toml` and the sibling `~/.vibe/prompts/rtk.md`
/// prompt file. Leaves any other user-declared hooks intact.
fn uninstall_vibe_at(vibe_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    let prompt_path = vibe_dir.join(VIBE_PROMPTS_SUBDIR).join(VIBE_PROMPT_FILE);
    if prompt_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove Vibe RTK prompt: {}",
                prompt_path.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- uninstall path removes only RTK's own prompt file
            fs::remove_file(&prompt_path)
                .with_context(|| format!("Failed to remove {}", prompt_path.display()))?;
        }
        removed.push(format!("Vibe prompt: {}", prompt_path.display()));
    }

    let hooks_path = vibe_dir.join(VIBE_HOOKS_FILE);
    if hooks_path.exists() {
        let content = fs::read_to_string(&hooks_path)
            .with_context(|| format!("Failed to read {}", hooks_path.display()))?;
        if let Some(new_content) = strip_vibe_rtk_entries(&content) {
            if dry_run {
                println!(
                    "[dry-run] would remove RTK hook from Vibe hooks.toml: {}",
                    hooks_path.display()
                );
            } else if new_content.trim().is_empty() {
                // nosemgrep: filesystem-deletion -- uninstall removes hooks.toml only when it becomes empty after stripping the RTK entry
                fs::remove_file(&hooks_path)
                    .with_context(|| format!("Failed to remove {}", hooks_path.display()))?;
            } else {
                atomic_write(&hooks_path, &new_content)
                    .with_context(|| format!("Failed to write {}", hooks_path.display()))?;
            }
            removed.push(format!(
                "Vibe hooks.toml: removed RTK entry ({})",
                hooks_path.display()
            ));
        }
    }

    let config_path = vibe_dir.join(VIBE_CONFIG_FILE);
    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        let rtk_url = format!(
            "http://{}{}",
            crate::llm_proxy::DEFAULT_BIND,
            VIBE_PROXY_PATH
        );
        if let Some(new_content) = remove_rtk_mistral_provider(&content, &rtk_url) {
            if dry_run {
                println!(
                    "[dry-run] would stop routing Vibe through the RTK proxy: {}",
                    config_path.display()
                );
            } else {
                atomic_write(&config_path, &new_content)
                    .with_context(|| format!("Failed to write {}", config_path.display()))?;
            }
            removed.push(format!(
                "Vibe config.toml: removed the RTK proxy from the mistral provider ({})",
                config_path.display()
            ));
        }
    }

    if verbose > 0 && !removed.is_empty() {
        eprintln!("Vibe artifacts removed");
    }

    Ok(removed)
}

/// Extract and drop the `[[hooks]]` block whose `name = "rtk-rewrite"` field
/// is set. Returns `None` when the entry is absent, `Some(new_content)` after
/// removal (with surrounding blank lines collapsed). The scan walks `[[hooks]]`
/// section boundaries — anything else in the file is preserved verbatim.
fn strip_vibe_rtk_entries(content: &str) -> Option<String> {
    let mut out: Option<String> = None;
    for name in [VIBE_HOOK_NAME, VIBE_POSTHOOK_NAME] {
        if let Some(stripped) = strip_vibe_entry(out.as_deref().unwrap_or(content), name) {
            out = Some(stripped);
        }
    }
    out
}

#[cfg(test)]
fn strip_vibe_rtk_entry(content: &str) -> Option<String> {
    strip_vibe_entry(content, VIBE_HOOK_NAME)
}

fn strip_vibe_entry(content: &str, name: &str) -> Option<String> {
    let needle = format!(r#"name = "{name}""#);
    if !content.contains(&needle) {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut sections: Vec<(usize, usize)> = Vec::new();
    let mut current_start: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[[hooks]]") || trimmed.starts_with('[') {
            if let Some(start) = current_start.take() {
                sections.push((start, i));
            }
            if trimmed.starts_with("[[hooks]]") {
                current_start = Some(i);
            }
        }
    }
    if let Some(start) = current_start {
        sections.push((start, lines.len()));
    }

    let target = sections
        .iter()
        .find(|(start, end)| lines[*start..*end].iter().any(|l| l.contains(&needle)))?;

    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend(&lines[..target.0]);
    kept.extend(&lines[target.1..]);

    let mut out = kept.join("\n");
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Vibe tests

    #[test]
    fn test_vibe_detects_rtk_entry_by_name_field() {
        assert!(!vibe_hooks_toml_has_rtk(""));
        assert!(!vibe_hooks_toml_has_rtk("[[hooks]]\nname = \"other\"\n"));
        assert!(vibe_hooks_toml_has_rtk(
            "[[hooks]]\nname = \"rtk-rewrite\"\n"
        ));
    }

    #[test]
    fn test_vibe_hook_entry_shape_matches_docs() {
        let entry = vibe_hook_entry("pre_tool", Some("bash"));
        assert!(entry.contains("[[hooks]]"));
        assert!(entry.contains(r#"name = "rtk-rewrite""#));
        assert!(entry.contains(r#"type = "pre_tool""#));
        assert!(entry.contains("match = 'bash'"));
        assert!(entry.contains(r#"command = "rtk hook vibe""#));
        assert!(entry.contains("strict = false"));

        let post = vibe_hook_entry("post_tool", Some(r"re:^(?:bash|file_system\.bash)$"));
        assert!(post.contains(r#"name = "rtk-posthook""#));
        assert!(post.contains(r#"type = "post_tool""#));
        assert!(post.contains(r"match = 're:^(?:bash|file_system\.bash)$'"));
        let parsed: toml::Value = toml::from_str(&post).expect("valid toml");
        assert_eq!(
            parsed["hooks"][0]["match"].as_str(),
            Some(r"re:^(?:bash|file_system\.bash)$")
        );
    }

    #[test]
    fn test_vibe_patch_widens_a_stale_match_and_adds_post_tool() {
        let temp = TempDir::new().unwrap();
        let hooks_path = temp.path().join(VIBE_HOOKS_FILE);
        let user_hook = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n";
        fs::write(
            &hooks_path,
            format!("{user_hook}\n{}", vibe_hook_entry("pre_tool", Some("bash"))),
        )
        .unwrap();

        let entries = vec![
            ("pre_tool", Some("re:^(?:bash|edit)$".to_string())),
            ("post_tool", Some("re:^(?:bash)$".to_string())),
        ];
        let outcome = patch_vibe_hooks_toml(
            &hooks_path,
            &entries,
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap();
        assert_eq!(outcome, VibeHookPatchOutcome::Installed);
        let content = fs::read_to_string(&hooks_path).unwrap();
        assert!(content.contains(r#"name = "user-audit""#));
        assert_eq!(content.matches(r#"name = "rtk-rewrite""#).count(), 1);
        assert!(content.contains("match = 're:^(?:bash|edit)$'"));
        assert!(!content.contains("match = 'bash'"));
        assert!(content.contains(r#"name = "rtk-posthook""#));
        let parsed: toml::Value = toml::from_str(&content).expect("valid toml");
        assert_eq!(parsed["hooks"].as_array().unwrap().len(), 3);

        let outcome = patch_vibe_hooks_toml(
            &hooks_path,
            &entries,
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap();
        assert_eq!(outcome, VibeHookPatchOutcome::AlreadyPresent);

        assert!(
            strip_vibe_rtk_entries(&content)
                .unwrap()
                .contains("user-audit")
        );
        assert!(
            !strip_vibe_rtk_entries(&content)
                .unwrap()
                .contains("rtk-posthook")
        );
    }

    #[test]
    fn test_vibe_strip_returns_none_when_entry_absent() {
        let content = "[[hooks]]\nname = \"other\"\ntype = \"post_tool\"\n";
        assert!(strip_vibe_rtk_entry(content).is_none());
    }

    #[test]
    fn test_vibe_strip_removes_only_rtk_entry() {
        let content = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n\n[[hooks]]\nname = \"rtk-rewrite\"\ntype = \"pre_tool\"\nmatch = \"bash\"\ncommand = \"rtk hook vibe\"\n";
        let stripped = strip_vibe_rtk_entry(content).expect("expected removal");
        assert!(stripped.contains(r#"name = "user-audit""#));
        assert!(!stripped.contains(r#"name = "rtk-rewrite""#));
        assert!(!stripped.contains("rtk hook vibe"));
    }

    #[test]
    fn test_vibe_install_creates_hook_and_prompt() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();

        let hooks_content = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert!(hooks_content.contains(r#"name = "rtk-rewrite""#));
        assert!(hooks_content.contains(r#"command = "rtk hook vibe""#));

        let prompt_path = vibe_dir.join(VIBE_PROMPTS_SUBDIR).join(VIBE_PROMPT_FILE);
        assert!(prompt_path.exists());
    }

    #[test]
    fn test_vibe_install_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();

        let hooks_content = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert_eq!(hooks_content.matches("rtk-rewrite").count(), 1);
    }

    #[test]
    fn test_vibe_install_preserves_existing_user_hook() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        fs::create_dir_all(&vibe_dir).unwrap();
        let user_hook = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n";
        fs::write(vibe_dir.join(VIBE_HOOKS_FILE), user_hook).unwrap();

        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();

        let hooks_content = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert!(hooks_content.contains(r#"name = "user-audit""#));
        assert!(hooks_content.contains(r#"name = "rtk-rewrite""#));
    }

    #[test]
    fn test_vibe_hook_only_skips_prompt_file() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, true, PatchMode::Auto, InitContext::default()).unwrap();

        assert!(vibe_dir.join(VIBE_HOOKS_FILE).exists());
        assert!(
            !vibe_dir
                .join(VIBE_PROMPTS_SUBDIR)
                .join(VIBE_PROMPT_FILE)
                .exists()
        );
    }

    #[test]
    fn test_vibe_uninstall_removes_only_rtk_entry_and_prompt() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        fs::create_dir_all(&vibe_dir).unwrap();
        let user_hook = "[[hooks]]\nname = \"user-audit\"\ntype = \"post_tool\"\nmatch = \"*\"\ncommand = \"audit.py\"\n";
        fs::write(vibe_dir.join(VIBE_HOOKS_FILE), user_hook).unwrap();

        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert!(
            vibe_dir
                .join(VIBE_PROMPTS_SUBDIR)
                .join(VIBE_PROMPT_FILE)
                .exists()
        );

        let removed_first = uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();
        let removed_second = uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(
            !vibe_dir
                .join(VIBE_PROMPTS_SUBDIR)
                .join(VIBE_PROMPT_FILE)
                .exists()
        );

        let remaining = fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap();
        assert!(remaining.contains(r#"name = "user-audit""#));
        assert!(!remaining.contains(r#"name = "rtk-rewrite""#));
    }

    #[test]
    fn test_vibe_uninstall_removes_hooks_file_when_no_other_hooks() {
        let temp = TempDir::new().unwrap();
        let vibe_dir = temp.path().join(".vibe");
        run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, InitContext::default()).unwrap();
        assert!(vibe_dir.join(VIBE_HOOKS_FILE).exists());

        uninstall_vibe_at(&vibe_dir, InitContext::default()).unwrap();

        assert!(!vibe_dir.join(VIBE_HOOKS_FILE).exists());
    }
}

#[cfg(test)]
mod proxy_provider_tests {
    use super::*;

    const RTK: &str = "http://127.0.0.1:8080/mistral/v1";

    #[test]
    fn the_mistral_api_base_is_read_from_the_provider_table() {
        assert_eq!(mistral_api_base_in(""), None);
        assert_eq!(mistral_api_base_in("active_model = \"devstral\"\n"), None);
        assert_eq!(
            mistral_api_base_in(
                "[[providers]]\nname = \"other\"\napi_base = \"http://x\"\n\n[[providers]]\nname = \"mistral\"\napi_base = \"https://api.mistral.ai/v1\"\n"
            )
            .as_deref(),
            Some("https://api.mistral.ai/v1")
        );
        assert_eq!(
            mistral_api_base_in("[[providers]]\nname = \"mistral\"\napi_base = \"  \"\n"),
            None
        );
        assert_eq!(
            mistral_api_base_in("providers = [{ name = \"mistral\" }]\n"),
            None
        );
    }

    #[test]
    fn pointing_vibe_at_the_proxy_adds_a_full_provider_entry_once() {
        let created = set_mistral_api_base("", RTK).unwrap();
        assert_eq!(
            created,
            "[[providers]]\nname = \"mistral\"\napi_base = \"http://127.0.0.1:8080/mistral/v1\"\napi_key_env_var = \"MISTRAL_API_KEY\"\nbackend = \"mistral\"\n"
        );
        let again = set_mistral_api_base(&created, RTK).unwrap();
        assert_eq!(again, created);
    }

    #[test]
    fn an_existing_mistral_provider_keeps_its_other_fields_and_comments() {
        let text = "# my vibe config\nactive_model = \"devstral\"\n\n[[providers]]\nname = \"mistral\"\napi_base = \"https://api.mistral.ai/v1\"\napi_key_env_var = \"MY_KEY\" # custom\nbackend = \"mistral\"\nextra = 1\n\n[[providers]]\nname = \"local\"\napi_base = \"http://localhost:11434/v1\"\n";
        let updated = set_mistral_api_base(text, RTK).unwrap();
        assert!(updated.starts_with("# my vibe config\nactive_model = \"devstral\"\n"));
        assert!(updated.contains("api_base = \"http://127.0.0.1:8080/mistral/v1\""));
        assert!(updated.contains("api_key_env_var = \"MY_KEY\" # custom"));
        assert!(updated.contains("extra = 1"));
        assert!(updated.contains("name = \"local\"\napi_base = \"http://localhost:11434/v1\""));
        assert_eq!(mistral_api_base_in(&updated).as_deref(), Some(RTK));
        assert!(set_mistral_api_base("providers = [{ name = \"mistral\" }]\n", RTK).is_err());
    }

    #[test]
    fn uninstall_removes_only_what_rtk_wrote_and_restores_a_customised_entry() {
        let ours = set_mistral_api_base("", RTK).unwrap();
        assert_eq!(remove_rtk_mistral_provider(&ours, RTK).as_deref(), Some(""));

        let with_other = format!(
            "[[providers]]\nname = \"local\"\napi_base = \"http://localhost:1/v1\"\n\n{ours}"
        );
        let stripped = remove_rtk_mistral_provider(&with_other, RTK).unwrap();
        assert!(stripped.contains("name = \"local\""));
        assert!(!stripped.contains("mistral"));

        let customised = "[[providers]]\nname = \"mistral\"\napi_base = \"http://127.0.0.1:8080/mistral/v1/\"\napi_key_env_var = \"MY_KEY\"\nbackend = \"mistral\"\ntemperature = 0.2\n";
        let restored = remove_rtk_mistral_provider(customised, RTK).unwrap();
        assert!(restored.contains("api_base = \"https://api.mistral.ai/v1\""));
        assert!(restored.contains("temperature = 0.2"));
        assert!(restored.contains("api_key_env_var = \"MY_KEY\""));

        let foreign = "[[providers]]\nname = \"mistral\"\napi_base = \"https://gateway.corp/v1\"\n";
        assert_eq!(remove_rtk_mistral_provider(foreign, RTK), None);
        assert_eq!(remove_rtk_mistral_provider("", RTK), None);
    }

    #[cfg(feature = "engine")]
    #[test]
    fn the_vibe_onramp_writes_and_reads_back_the_provider_entry() {
        use super::super::proxy_onramp::ProxyOnramp;
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let onramp = VibeOnramp {
            config_path: config_path.clone(),
        };
        assert_eq!(onramp.current_target().unwrap(), None);
        assert!(!onramp.autostart_present().unwrap());
        assert!(onramp.spec().autostart_option.is_none());
        assert_eq!(onramp.spec().path, "/mistral/v1");
        let lines = onramp.write(RTK, false).unwrap();
        assert!(
            lines[0].contains("providers[mistral].api_base = http://127.0.0.1:8080/mistral/v1"),
            "{lines:?}"
        );
        assert_eq!(onramp.current_target().unwrap().as_deref(), Some(RTK));
        assert!(onramp.manual_line(RTK).starts_with("[[providers]]\n"));
        assert!(
            onramp
                .manual_line(RTK)
                .contains("api_base = \"http://127.0.0.1:8080/mistral/v1\"")
        );

        let removed = uninstall_vibe_at(temp.path(), InitContext::default()).unwrap();
        assert!(
            removed.iter().any(|l| l.contains("RTK proxy")),
            "{removed:?}"
        );
        assert_eq!(onramp.current_target().unwrap(), None);
    }
}

#[cfg(test)]
mod onramp_report_tests {
    use super::*;
    use crate::hooks::hook_check::HookStatus;

    #[test]
    fn the_project_footer_reports_global_state_and_points_at_g() {
        let missing = project_scope_footer_lines(HookStatus::Missing, false, false).join("\n");
        assert!(missing.contains("NOT registered"), "{missing}");
        assert!(missing.contains("not set up"), "{missing}");
        assert_eq!(missing.matches("rtk init -g").count(), 2, "{missing}");

        let done = project_scope_footer_lines(HookStatus::Ok, true, true).join("\n");
        assert!(done.contains("registered globally"), "{done}");
        assert!(done.contains("configured and running"), "{done}");
        assert!(!done.contains("rtk init -g"), "{done}");
    }

    /// The state that leaves `rtk gain` empty: daemon up, nothing aimed at it.
    #[test]
    fn a_running_but_unused_daemon_is_called_out_in_project_scope() {
        let lines = project_scope_footer_lines(HookStatus::Ok, false, true).join("\n");
        assert!(lines.contains("nothing routed to it"), "{lines}");
    }
}
