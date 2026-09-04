//! Opencode agent: hook install/uninstall helpers.

use super::*;

// Embedded OpenCode plugin (auto-rewrite)
pub(crate) const OPENCODE_PLUGIN: &str = include_str!("../../../hooks/opencode/rtk.ts");

// Embedded Pi extension (auto-rewrite)
pub(crate) const PI_PLUGIN: &str = include_str!("../../../hooks/pi/rtk.ts");

// Stable code marker used to recognize a modified RTK extension without
// relying on explanatory comments that users may remove. The marker matches
// both the current `pi.exec` call and older stock revisions that imported
// `exec` locally before invoking it.
pub(crate) const PI_PLUGIN_REWRITE_MARKER: &str = "exec(\"rtk\", [\"rewrite\"";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PiCompatibleAgent {
    Pi,
    Omp,
}

impl PiCompatibleAgent {
    fn name(self) -> &'static str {
        match self {
            Self::Pi => "Pi",
            Self::Omp => "OMP",
        }
    }

    fn state_name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Omp => "omp",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Pi => Self::Omp,
            Self::Omp => Self::Pi,
        }
    }

    fn from_state_name(name: &str) -> Option<Self> {
        match name {
            "pi" => Some(Self::Pi),
            "omp" => Some(Self::Omp),
            _ => None,
        }
    }
}

pub(crate) enum ManagedAgentState {
    Absent,
    Known(Vec<PiCompatibleAgent>),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExtensionShareStatus {
    NotShared,
    Shared,
    Unknown,
}

// SHA-256 hashes of stock Pi extension revisions that may exist on user
// machines, including the current embedded file. Keep historical entries
// when changing the extension so an untouched older install can still be
// removed safely. Hashes are computed after normalizing CRLF to LF and
// trimming trailing whitespace, matching the comparisons below.
// The history-based test below verifies that this list remains append-only
// for every revision in the current checkout's ancestor history.
pub(crate) const KNOWN_PI_PLUGIN_HASHES: &[&str] = &[
    "5e80e811e689adc9d5ae5a59d1d5702060ca0c10320fea7cffd83c659026f1c5",
    "2cbb2a7a9081275d6eda140d9e375f6772b5c354e7fe931c554c371ad8836c6e",
    "94e80d1a5c159ea38ba8913f7c5b9d9b5c89bf7c204f1e583bfac2ed7fc40ab9",
    "b63e3f6eeaeec23837df5a7c4024fe16dca1f8a49fb1743f8a877cc136ebc2d9",
    "c30d4f4774c59bf25b50b70ab8a7dcb1b8287074592af1598dc09962fa1c7137",
    "5ad230679294dc8dce09546fa25101fd3d0949f454cc8b72e04664fa1bd45ed7",
    "be251e44747e6d09e5ca56ecaeddd8f4861c35a57500cd8b2bf9c39afe5795e8",
    "eb56dd08b8d5f4704906d037d70b357d84d827abe1063135cc7c998efe6cf7f2",
    "628308173ae41c488b76bcf90eafbd4c0c72435927645d81cdbec652eac4b107",
    "3eb16108f51a29c2a62a453d5c97a6ea2da8aea1061da34c50fdcfaa32dc0ff7",
];

pub(crate) fn resolve_opencode_dir() -> Result<PathBuf> {
    resolve_home_subdir(CONFIG_DIR).map(|p| p.join(OPENCODE_SUBDIR))
}

// ─── Pi coding agent support ──────────────────────────────────────────

/// Resolve Pi config directory, honouring `PI_CODING_AGENT_DIR` override.
pub(crate) fn resolve_pi_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(PI_CODING_AGENT_DIR_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    resolve_home_subdir(PI_DIR)
}

/// Return the path to the installed Pi extension file.
pub(crate) fn pi_plugin_path(pi_dir: &Path) -> PathBuf {
    pi_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE)
}

/// Return the Pi extension install path for the given scope.
/// global=true  → `$PI_CODING_AGENT_DIR/extensions/rtk.ts`
/// global=false → `./.pi/extensions/rtk.ts`
pub(crate) fn pi_plugin_path_for_scope(global: bool) -> Result<PathBuf> {
    if global {
        Ok(pi_plugin_path(&resolve_pi_dir()?))
    } else {
        Ok(PathBuf::from(PI_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE))
    }
}

/// Create the Pi extensions directory, or in dry-run mode, print a message only if
/// the directory does not yet exist (avoids reporting no-op changes).
pub(crate) fn ensure_pi_extensions_dir(parent: &Path, name: &str, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if dry_run {
        if !parent.exists() {
            println!("[dry-run] would create {}: {}", name, parent.display());
        }
    } else {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}: {}", name, parent.display()))?;
    }
    Ok(())
}

/// Check whether a managed Pi-compatible extension can be installed.
///
/// Returns `false` when the selected policy declines or previews a skipped
/// action; `--auto-patch --dry-run` returns `true` so the caller can preview
/// the same directory and write actions as a real auto-patch. Validation runs
/// before parent directory creation.
pub(crate) fn validate_stock_pi_plugin_path(
    path: &Path,
    name: &str,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    if path.exists() {
        let is_known_stock = match fs::read_to_string(path) {
            Ok(existing) => is_known_stock_pi_plugin(&existing),
            Err(error) => {
                eprintln!(
                    "[warn] {} at {} could not be read; treating it as non-stock: {}",
                    name,
                    path.display(),
                    error
                );
                false
            }
        };
        if !is_known_stock {
            if ctx.dry_run {
                return match patch_mode {
                    PatchMode::Ask => {
                        println!(
                            "[dry-run] would prompt before overwriting {}: {}",
                            name,
                            path.display()
                        );
                        Ok(false)
                    }
                    PatchMode::Auto => {
                        println!(
                            "[dry-run] would overwrite non-stock {}: {}",
                            name,
                            path.display()
                        );
                        Ok(true)
                    }
                    PatchMode::Skip => {
                        println!(
                            "[dry-run] would leave {} unchanged: {}",
                            name,
                            path.display()
                        );
                        Ok(false)
                    }
                };
            }

            let should_overwrite = match patch_mode {
                PatchMode::Auto => true,
                PatchMode::Skip => false,
                PatchMode::Ask => {
                    let prompt = format!("Overwrite the non-stock {} at {}?", name, path.display());
                    prompt_user_confirmation(&prompt)?
                }
            };

            return Ok(should_overwrite);
        }
    }

    Ok(true)
}

pub(crate) fn normalize_pi_plugin_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n")
}

pub(crate) fn is_current_pi_plugin(content: &str) -> bool {
    normalize_pi_plugin_line_endings(content).trim_end()
        == normalize_pi_plugin_line_endings(PI_PLUGIN).trim_end()
}

pub(crate) fn looks_like_rtk_pi_plugin(content: &str) -> bool {
    content.contains(PI_PLUGIN_REWRITE_MARKER)
}

pub(crate) fn is_known_stock_pi_plugin(content: &str) -> bool {
    if is_current_pi_plugin(content) {
        return true;
    }

    let normalized = normalize_pi_plugin_line_endings(content);
    let hash = integrity::compute_hash_bytes(normalized.trim_end().as_bytes());
    KNOWN_PI_PLUGIN_HASHES
        .iter()
        .any(|expected| *expected == hash)
}

/// Check whether the Pi and OMP extension paths for the selected scope resolve
/// to the same target.
pub(crate) fn extension_paths_alias(
    global: bool,
    path: &Path,
    agent: PiCompatibleAgent,
) -> Result<bool> {
    let other_path = match agent {
        PiCompatibleAgent::Pi => omp_extension_path_for_scope(global)?,
        PiCompatibleAgent::Omp => pi_plugin_path_for_scope(global)?,
    };

    Ok(canonicalize_path_for_comparison(path) == canonicalize_path_for_comparison(&other_path))
}

/// Canonicalize an extension path even when its final file has not been
/// created yet. This detects agent directories connected by symlinks while
/// retaining a literal-path fallback for genuinely unresolved paths.
pub(crate) fn canonicalize_path_for_comparison(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }

    let mut missing_components = Vec::new();
    let mut candidate = path;
    loop {
        if let Ok(mut canonical) = fs::canonicalize(candidate) {
            for component in missing_components.iter().rev() {
                canonical.push(component);
            }
            return canonical;
        }

        let Some(file_name) = candidate.file_name() else {
            return path.to_path_buf();
        };
        missing_components.push(file_name.to_os_string());

        let Some(parent) = candidate.parent() else {
            return path.to_path_buf();
        };
        if parent == candidate {
            return path.to_path_buf();
        }
        candidate = parent;
    }
}

pub(crate) fn shared_agent_state_path(path: &Path) -> PathBuf {
    canonicalize_path_for_comparison(path).with_file_name(PI_AGENT_STATE_FILE)
}

pub(crate) fn read_managed_agents(path: &Path) -> Result<ManagedAgentState> {
    let state_path = shared_agent_state_path(path);
    let content = match fs::read_to_string(&state_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedAgentState::Absent);
        }
        Err(error) => {
            eprintln!(
                "[warn] RTK extension ownership state at {} could not be read; treating ownership as unknown: {}",
                state_path.display(),
                error
            );
            return Ok(ManagedAgentState::Unknown);
        }
    };
    let mut agents = Vec::new();
    let mut has_invalid_entry = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match PiCompatibleAgent::from_state_name(line) {
            Some(agent) => {
                if !agents.contains(&agent) {
                    agents.push(agent);
                }
            }
            None => has_invalid_entry = true,
        }
    }

    if has_invalid_entry {
        eprintln!(
            "[warn] RTK extension ownership state at {} contains unknown entries; treating ownership as unknown",
            state_path.display()
        );
        return Ok(ManagedAgentState::Unknown);
    }

    if agents.is_empty() {
        eprintln!(
            "[warn] RTK extension ownership state at {} is empty; treating ownership as unknown",
            state_path.display()
        );
        return Ok(ManagedAgentState::Unknown);
    }

    Ok(ManagedAgentState::Known(agents))
}

pub(crate) fn record_managed_agent(
    global: bool,
    path: &Path,
    agent: PiCompatibleAgent,
    extension_was_present: bool,
    ctx: InitContext,
) -> Result<()> {
    if !extension_paths_alias(global, path, agent)? {
        return Ok(());
    }

    let state_path = shared_agent_state_path(path);
    let mut agents = if !extension_was_present {
        // If the extension was absent before this install, any remaining
        // sidecar describes a file that no longer exists and must not be
        // carried into the new installation.
        Vec::new()
    } else {
        match read_managed_agents(path)? {
            ManagedAgentState::Absent => {
                eprintln!(
                    "[warn] RTK extension ownership state at {} could not be established because this pre-existing extension has no ownership record; preserving the absent state and proceeding without recording {}",
                    state_path.display(),
                    agent.state_name()
                );
                return Ok(());
            }
            ManagedAgentState::Known(agents) => agents,
            ManagedAgentState::Unknown => {
                // Do not turn unknown ownership into a current-agent-only
                // record. The extension was installed successfully, but the
                // existing state must remain intact for the fallback path.
                eprintln!(
                    "[warn] RTK extension ownership state at {} could not be updated because ownership is unknown; preserving it and proceeding without recording {}",
                    state_path.display(),
                    agent.state_name()
                );
                return Ok(());
            }
        }
    };
    if !agents.contains(&agent) {
        agents.push(agent);
    }
    agents.sort_by_key(|agent| agent.state_name());
    let content = format!(
        "{}\n",
        agents
            .iter()
            .map(|agent| agent.state_name())
            .collect::<Vec<_>>()
            .join("\n")
    );
    write_if_changed_allow_read_error(&state_path, &content, "RTK extension ownership state", ctx)?;
    Ok(())
}

pub(crate) fn remove_managed_agent_state(state_path: &Path, ctx: InitContext) -> Result<()> {
    if !state_path.exists() {
        return Ok(());
    }

    if ctx.dry_run {
        println!(
            "[dry-run] would remove RTK extension ownership state: {}",
            state_path.display()
        );
    } else {
        // nosemgrep: filesystem-deletion -- state belongs exclusively to the RTK-managed extension.
        fs::remove_file(state_path).with_context(|| {
            format!(
                "Failed to remove RTK extension ownership state: {}",
                state_path.display()
            )
        })?;
    }
    Ok(())
}

/// Determine whether a Pi-compatible extension path is shared by both agents,
/// distinguishing definitive sidecar ownership from unavailable information.
///
/// The ownership sidecar records which agents RTK installed for a relocated
/// shared path. A missing sidecar is treated as uncertain because the
/// extension may predate RTK's ownership tracking.
pub(crate) fn extension_share_status(
    global: bool,
    path: &Path,
    agent: PiCompatibleAgent,
) -> Result<ExtensionShareStatus> {
    if !extension_paths_alias(global, path, agent)? {
        return Ok(ExtensionShareStatus::NotShared);
    }

    let other_agent = agent.other();
    match read_managed_agents(path)? {
        ManagedAgentState::Known(agents) => {
            if agents.contains(&other_agent) {
                Ok(ExtensionShareStatus::Shared)
            } else {
                Ok(ExtensionShareStatus::NotShared)
            }
        }
        ManagedAgentState::Absent | ManagedAgentState::Unknown => Ok(ExtensionShareStatus::Unknown),
    }
}

pub(crate) fn extension_scope_name(global: bool) -> &'static str {
    if global {
        "global"
    } else {
        "project"
    }
}

pub(crate) fn warn_if_extension_shared_on_install(
    global: bool,
    path: &Path,
    agent: PiCompatibleAgent,
) -> Result<()> {
    let scope = extension_scope_name(global);
    match extension_share_status(global, path, agent)? {
        ExtensionShareStatus::NotShared => {}
        ExtensionShareStatus::Shared => eprintln!(
            "[warn] Pi and OMP share the {} extension path at {}; installing {} here enables the shared integration for both agents.",
            scope,
            path.display(),
            agent.name()
        ),
        ExtensionShareStatus::Unknown => eprintln!(
            "[warn] Pi and OMP resolve to the same {} extension path at {}, but RTK could not confirm both agents' ownership; installing {} without a definitive ownership record.",
            scope,
            path.display(),
            agent.name()
        ),
    }

    Ok(())
}

pub(crate) fn confirm_shared_extension_uninstall(
    global: bool,
    path: &Path,
    agent: PiCompatibleAgent,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    let scope = extension_scope_name(global);
    match extension_share_status(global, path, agent)? {
        ExtensionShareStatus::NotShared => return Ok(true),
        ExtensionShareStatus::Unknown => {
            eprintln!(
                "[warn] Pi and OMP resolve to the same {} extension path at {}, but RTK could not confirm both agents' ownership; proceeding with {} uninstall without shared-path protection.",
                scope,
                path.display(),
                agent.name()
            );
            return Ok(true);
        }
        ExtensionShareStatus::Shared => eprintln!(
            "[warn] Pi and OMP share the {} extension path at {}; uninstalling {} changes a path used by the other agent's shared integration.",
            scope,
            path.display(),
            agent.name()
        ),
    }

    match patch_mode {
        PatchMode::Auto => Ok(true),
        PatchMode::Skip => {
            if ctx.dry_run {
                println!(
                    "[dry-run] would leave shared Pi/OMP extension unchanged: {}",
                    path.display()
                );
            }
            Ok(false)
        }
        PatchMode::Ask => {
            if ctx.dry_run {
                println!(
                    "[dry-run] would prompt before removing shared Pi/OMP extension: {}",
                    path.display()
                );
                return Ok(false);
            }

            let prompt = format!("Remove the shared Pi/OMP extension at {}?", path.display());
            if prompt_user_confirmation(&prompt)? {
                Ok(true)
            } else {
                println!("Skipped removal of shared Pi/OMP extension.");
                Ok(false)
            }
        }
    }
}

pub(crate) fn read_extension_for_uninstall(
    path: &Path,
    name: &str,
    ctx: InitContext,
) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) => {
            eprintln!(
                "[warn] {} at {} could not be read; leaving it alone: {}",
                name,
                path.display(),
                error
            );
            if ctx.dry_run {
                println!(
                    "[dry-run] would leave unreadable {} unchanged: {}",
                    name,
                    path.display()
                );
                print_dry_run_footer();
                Ok(None)
            } else {
                anyhow::bail!(
                    "{} at {} could not be read; leaving it alone.",
                    name,
                    path.display()
                );
            }
        }
    }
}

/// Uninstall Pi extension for the given scope.
/// Mirrors `uninstall_codex` / `uninstall_hermes`: extracted from the dispatcher
/// so it can be tested and reasoned about independently.
pub(crate) fn uninstall_pi_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { verbose, dry_run } = ctx;
    let plugin_path = pi_plugin_path_for_scope(global)?;

    if !plugin_path.exists() {
        if dry_run {
            print_dry_run_footer();
        } else {
            println!("RTK Pi extension was not installed (nothing to remove)");
        }
        return Ok(());
    }

    let ownership_state_path = shared_agent_state_path(&plugin_path);
    let Some(content) = read_extension_for_uninstall(&plugin_path, "Pi extension", ctx)? else {
        return Ok(());
    };

    if !is_known_stock_pi_plugin(&content) {
        if looks_like_rtk_pi_plugin(&content) {
            if dry_run {
                println!(
                    "[dry-run] would refuse to remove Pi extension: {}",
                    plugin_path.display()
                );
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "Pi extension at {} contains RTK content that does not match the stock extension. Remove the file manually.",
                plugin_path.display()
            );
        }
        println!(
            "Pi extension at {} is not RTK content; leaving it alone.",
            plugin_path.display()
        );
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(());
    }

    if !confirm_shared_extension_uninstall(
        global,
        &plugin_path,
        PiCompatibleAgent::Pi,
        patch_mode,
        ctx,
    )? {
        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "Shared Pi/OMP extension at {} was not removed; rerun with --auto-patch to approve the removal.",
            plugin_path.display()
        );
    }

    if dry_run {
        println!(
            "[dry-run] would remove Pi extension: {}",
            plugin_path.display()
        );
        remove_managed_agent_state(&ownership_state_path, ctx)?;
        print_dry_run_footer();
    } else {
        // nosemgrep: filesystem-deletion -- Pi uninstall removes only a known RTK stock extension.
        fs::remove_file(&plugin_path)
            .with_context(|| format!("Failed to remove Pi extension: {}", plugin_path.display()))?;
        remove_managed_agent_state(&ownership_state_path, ctx)?;
        if verbose > 0 {
            eprintln!("Removed Pi extension: {}", plugin_path.display());
        }
        println!("RTK uninstalled (Pi):");
        println!("  - Pi extension: {}", plugin_path.display());
        println!("\nRestart pi to apply changes.");
    }
    Ok(())
}

/// Install the Pi extension with an explicit confirmation policy for an
/// existing non-stock file.
pub fn run_pi_mode_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let plugin_path = pi_plugin_path_for_scope(global)?;
    let extension_was_present = plugin_path.exists();

    warn_if_extension_shared_on_install(global, &plugin_path, PiCompatibleAgent::Pi)?;

    if !validate_stock_pi_plugin_path(&plugin_path, "Pi extension", patch_mode, ctx)? {
        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "Pi extension at {} was not changed; remove or back up the file manually before retrying.",
            plugin_path.display()
        );
    }

    if let Some(parent) = plugin_path.parent() {
        ensure_pi_extensions_dir(
            parent,
            if global {
                "Pi extensions directory"
            } else {
                "local Pi extensions directory"
            },
            ctx,
        )?;
    }

    let installed =
        write_if_changed_allow_read_error(&plugin_path, PI_PLUGIN, "Pi extension", ctx)?;
    record_managed_agent(
        global,
        &plugin_path,
        PiCompatibleAgent::Pi,
        extension_was_present,
        ctx,
    )?;

    if dry_run {
        print_dry_run_footer();
    } else {
        print_pi_result(&plugin_path, installed);
    }

    Ok(())
}

/// Install the Pi extension (hook-only; no AGENTS.md injection).
///
/// global=true  → `$PI_CODING_AGENT_DIR/extensions/rtk.ts`
/// global=false → `.pi/extensions/rtk.ts`
#[allow(dead_code)] // Kept as the default-policy API for in-crate callers and tests.
pub fn run_pi_mode(global: bool, ctx: InitContext) -> Result<()> {
    run_pi_mode_with_patch_mode(global, PatchMode::Ask, ctx)
}

pub(crate) fn print_pi_result(plugin_path: &Path, installed: bool) {
    let status = if installed {
        "installed"
    } else {
        "already up to date"
    };
    println!("RTK Pi extension {}:", status);
    println!("  Extension: {}", plugin_path.display());
    println!();
    println!("Pi will load the extension automatically on next start.");
    println!("Verify: pi -e {} --no-session", plugin_path.display());
}

/// Return OpenCode plugin path: ~/.config/opencode/plugins/rtk.ts
pub(crate) fn opencode_plugin_path(opencode_dir: &Path) -> PathBuf {
    opencode_dir.join(PLUGIN_SUBDIR).join(OPENCODE_PLUGIN_FILE)
}

/// Prepare OpenCode plugin directory and return install path
pub(crate) fn prepare_opencode_plugin_path() -> Result<PathBuf> {
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    // Directory creation is deferred to install time (caller guards on dry_run).
    Ok(path)
}

/// Write OpenCode plugin file if missing or outdated
pub(crate) fn ensure_opencode_plugin_installed(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { dry_run, .. } = ctx;
    // Ensure parent dir exists (skip in dry-run)
    if !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create OpenCode plugin directory: {}",
                    parent.display()
                )
            })?;
        }
    }
    write_if_changed(path, OPENCODE_PLUGIN, "OpenCode plugin", ctx)
}

/// Remove OpenCode plugin file
pub(crate) fn remove_opencode_plugin(ctx: InitContext) -> Result<Vec<PathBuf>> {
    let InitContext { verbose, dry_run } = ctx;
    let opencode_dir = resolve_opencode_dir()?;
    let path = opencode_plugin_path(&opencode_dir);
    let mut removed = Vec::new();

    if path.exists() {
        if dry_run {
            println!("[dry-run] would remove OpenCode plugin: {}", path.display());
        } else {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove OpenCode plugin: {}", path.display()))?;
            if verbose > 0 {
                eprintln!("Removed OpenCode plugin: {}", path.display());
            }
        }
        removed.push(path);
    }

    Ok(removed)
}

pub(crate) fn run_opencode_only_mode(ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let opencode_plugin_path = prepare_opencode_plugin_path()?;
    ensure_opencode_plugin_installed(&opencode_plugin_path, ctx)?;
    if !dry_run {
        println!("\nOpenCode plugin installed (global).\n");
        println!("  OpenCode: {}", opencode_plugin_path.display());
        println!("  Restart OpenCode. Test with: git status\n");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn test_opencode_plugin_install_and_update() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);

        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        assert!(!plugin_path.exists());

        let changed =
            ensure_opencode_plugin_installed(&plugin_path, InitContext::default()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content, OPENCODE_PLUGIN);

        fs::write(&plugin_path, "// old").unwrap();
        let changed_again =
            ensure_opencode_plugin_installed(&plugin_path, InitContext::default()).unwrap();
        assert!(changed_again);
        let content_updated = fs::read_to_string(&plugin_path).unwrap();
        assert_eq!(content_updated, OPENCODE_PLUGIN);
    }

    #[test]
    fn test_opencode_plugin_remove() {
        let temp = TempDir::new().unwrap();
        let opencode_dir = temp.path().join("opencode");
        let plugin_path = opencode_plugin_path(&opencode_dir);
        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        fs::write(&plugin_path, OPENCODE_PLUGIN).unwrap();

        assert!(plugin_path.exists());
        fs::remove_file(&plugin_path).unwrap();
        assert!(!plugin_path.exists());
    }

    // ─── Pi integration tests ───────────────────────────────────────────

    #[test]
    fn test_run_pi_mode_global_installs_plugin() {
        let tmp = TempDir::new().unwrap();
        with_pi_dir_override(&tmp, |pi_dir| {
            run_pi_mode(true, InitContext::default()).unwrap();

            let plugin = pi_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            assert!(plugin.exists(), "global Pi extension must be created");

            let content = fs::read_to_string(&plugin).unwrap();
            assert!(
                content.contains("rtk rewrite"),
                "extension must delegate to rtk rewrite"
            );
            // Regression guard for #2753: a value import (e.g. `import { isToolCallEventType }`)
            // pulls in the whole @earendil-works/pi-coding-agent barrel at extension load,
            // adding ~250ms of startup latency. Only `import type { ... }` is allowed.
            assert!(
                !content.contains("import {"),
                "extension must not load the Pi package at runtime"
            );
        });
    }

    #[test]
    fn test_run_pi_mode_local_installs_plugin() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = run_pi_mode(false, InitContext::default());
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        let plugin = tmp
            .path()
            .join(".pi")
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(plugin.exists(), "local Pi extension must be created");
    }

    #[test]
    fn test_run_pi_mode_global_does_not_create_agents_md() {
        let tmp = TempDir::new().unwrap();
        with_pi_dir_override(&tmp, |pi_dir| {
            run_pi_mode(true, InitContext::default()).unwrap();

            let agents_md = pi_dir.join(AGENTS_MD);
            assert!(!agents_md.exists(), "AGENTS.md must not be created");
        });
    }

    #[test]
    fn test_run_pi_mode_global_creates_plugin_when_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let absent_dir = tmp.path().join("no_such_pi_dir");
        let _guard = PI_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let orig = std::env::var_os(PI_CODING_AGENT_DIR_ENV);
        std::env::set_var(PI_CODING_AGENT_DIR_ENV, &absent_dir);

        let result = run_pi_mode(true, InitContext::default());

        match orig {
            Some(v) => std::env::set_var(PI_CODING_AGENT_DIR_ENV, v),
            None => std::env::remove_var(PI_CODING_AGENT_DIR_ENV),
        }

        result.unwrap();

        let plugin = absent_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
        assert!(
            plugin.exists(),
            "plugin must be written even when dir was absent"
        );

        let agents_md = absent_dir.join(AGENTS_MD);
        assert!(!agents_md.exists(), "AGENTS.md must not be created");
    }

    #[test]
    fn test_pi_global_uninstall_removes_plugin() {
        let tmp = TempDir::new().unwrap();
        with_pi_dir_override(&tmp, |pi_dir| {
            run_pi_mode(true, InitContext::default()).unwrap();

            let plugin = pi_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            assert!(plugin.exists());

            uninstall_with_patch_mode(
                true,
                false,
                false,
                false,
                true,
                false,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();

            assert!(!plugin.exists(), "plugin must be removed");
        });
    }

    #[test]
    fn test_pi_local_uninstall_removes_plugin() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_pi_mode(false, InitContext::default()).unwrap();
        let result = uninstall(
            false,
            false,
            false,
            false,
            true,
            false,
            InitContext::default(),
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        let plugin = tmp
            .path()
            .join(".pi")
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(!plugin.exists(), "local plugin must be removed");
    }

    #[test]
    fn test_pi_plugin_path_for_scope_global() {
        let tmp = TempDir::new().unwrap();
        with_pi_dir_override(&tmp, |pi_dir| {
            let path = pi_plugin_path_for_scope(true).unwrap();
            assert_eq!(path, pi_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE));
        });
    }

    #[test]
    fn test_pi_plugin_path_for_scope_local() {
        let path = pi_plugin_path_for_scope(false).unwrap();
        assert_eq!(
            path,
            PathBuf::from(PI_LOCAL_DIR)
                .join(PI_EXTENSIONS_SUBDIR)
                .join(PI_PLUGIN_FILE)
        );
    }

    #[test]
    fn test_run_pi_mode_global_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        with_pi_dir_override(&tmp, |pi_dir| {
            run_pi_mode(
                true,
                InitContext {
                    verbose: 0,
                    dry_run: true,
                },
            )
            .unwrap();

            assert!(
                !pi_dir.join(PI_EXTENSIONS_SUBDIR).exists(),
                "dry-run must not create the Pi extensions directory"
            );
            assert!(
                !pi_dir
                    .join(PI_EXTENSIONS_SUBDIR)
                    .join(PI_PLUGIN_FILE)
                    .exists(),
                "dry-run must not create the Pi extension file"
            );
        });
    }

    #[test]
    fn test_run_pi_mode_local_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = run_pi_mode(
            false,
            InitContext {
                verbose: 0,
                dry_run: true,
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        assert!(
            !tmp.path().join(".pi").join(PI_EXTENSIONS_SUBDIR).exists(),
            "dry-run must not create .pi/extensions/"
        );
    }

    #[test]
    fn test_pi_global_uninstall_dry_run_keeps_plugin() {
        let tmp = TempDir::new().unwrap();
        with_pi_dir_override(&tmp, |pi_dir| {
            run_pi_mode(true, InitContext::default()).unwrap();
            let plugin = pi_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            assert!(
                plugin.exists(),
                "plugin must exist before uninstall dry-run"
            );

            uninstall(
                true,
                false,
                false,
                false,
                true,
                false,
                InitContext {
                    verbose: 0,
                    dry_run: true,
                },
            )
            .unwrap();

            assert!(
                plugin.exists(),
                "dry-run uninstall must not remove the Pi extension"
            );
        });
    }

    #[test]
    fn test_pi_local_uninstall_dry_run_keeps_plugin() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_pi_mode(false, InitContext::default()).unwrap();
        let plugin = tmp
            .path()
            .join(".pi")
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(
            plugin.exists(),
            "plugin must exist before uninstall dry-run"
        );

        let result = uninstall(
            false,
            false,
            false,
            false,
            true,
            false,
            InitContext {
                verbose: 0,
                dry_run: true,
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        assert!(
            plugin.exists(),
            "dry-run uninstall must not remove the local Pi extension"
        );
    }

    #[test]
    fn test_pi_install_refuses_modified_extension() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_pi_mode_with_patch_mode(false, PatchMode::Skip, InitContext::default());
        std::env::set_current_dir(&cwd).unwrap();

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("was not changed"),
            "unexpected error: {}",
            err
        );
        assert_eq!(fs::read_to_string(path).unwrap(), modified);
    }

    #[test]
    fn test_pi_install_dry_run_reports_refusal_without_error() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_pi_mode(
            false,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();

        result.unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), modified);
    }

    #[test]
    fn test_pi_uninstall_modified_extension_bails() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(&path, format!("{}\n// user modification\n", PI_PLUGIN)).unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            true,
            false,
            InitContext::default(),
        );
        std::env::set_current_dir(&cwd).unwrap();

        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("does not match the stock extension"),
            "unexpected error: {}",
            err
        );
        assert!(path.exists(), "modified extension must not be removed");
    }

    #[test]
    fn test_pi_uninstall_modified_extension_dry_run_is_preview() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(&path, format!("{}\n// user modification\n", PI_PLUGIN)).unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            true,
            false,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();

        result.unwrap();
        assert!(path.exists(), "dry-run must preserve modified extension");
    }

    #[test]
    fn test_pi_uninstall_unreadable_extension_is_left_alone() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            true,
            false,
            InitContext::default(),
        );
        std::env::set_current_dir(&cwd).unwrap();

        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("could not be read; leaving it alone"),
            "unreadable extension uninstall should fail clearly: {err}"
        );
        assert!(path.exists(), "unreadable extension must be left alone");
    }

    #[test]
    fn test_pi_uninstall_unrelated_content_left_alone() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// rtk rewrite is mentioned here\nexport default () => {}\n",
        )
        .unwrap();

        uninstall(
            false,
            false,
            false,
            false,
            true,
            false,
            InitContext::default(),
        )
        .unwrap();
        std::env::set_current_dir(&cwd).unwrap();

        assert!(path.exists(), "non-RTK extension must be left in place");
    }

    #[test]
    fn test_known_pi_plugin_hashes_are_sha256() {
        assert!(
            KNOWN_PI_PLUGIN_HASHES.len() >= 8,
            "historical Pi extension hashes must not be removed"
        );
        assert!(KNOWN_PI_PLUGIN_HASHES
            .iter()
            .all(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())));

        let current_hash = integrity::compute_hash_bytes(
            normalize_pi_plugin_line_endings(PI_PLUGIN)
                .trim_end()
                .as_bytes(),
        );
        assert!(
            KNOWN_PI_PLUGIN_HASHES.contains(&current_hash.as_str()),
            "current Pi extension hash {current_hash} is missing from KNOWN_PI_PLUGIN_HASHES"
        );
        assert!(is_known_stock_pi_plugin(PI_PLUGIN));

        let crlf = PI_PLUGIN.replace("\r\n", "\n").replace('\n', "\r\n");
        assert!(is_current_pi_plugin(&crlf));
        assert!(is_known_stock_pi_plugin(&crlf));

        let modified = format!("{}\n// user modification\n", PI_PLUGIN);
        assert!(!is_known_stock_pi_plugin(&modified));
    }

    #[test]
    fn test_all_git_pi_plugin_revisions_are_allowlisted() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        if !manifest_dir.join(".git").exists() {
            // Source archives do not contain Git history. CI checks out the
            // repository with full history so this guard remains active there.
            return;
        }

        let revisions = Command::new("git")
            .current_dir(manifest_dir)
            .args([
                "rev-list",
                "HEAD",
                "--full-history",
                "--",
                "hooks/pi/rtk.ts",
            ])
            .output()
            .expect("git must be available to verify Pi extension history");
        assert!(
            revisions.status.success(),
            "git rev-list failed: {}",
            String::from_utf8_lossy(&revisions.stderr)
        );

        let mut commits: Vec<String> = String::from_utf8(revisions.stdout)
            .expect("git revision list must be UTF-8")
            .lines()
            .map(str::to_owned)
            .collect();
        commits.push("HEAD".to_owned());
        commits.sort();
        commits.dedup();

        for commit in commits {
            let object = format!("{commit}:hooks/pi/rtk.ts");
            let file = Command::new("git")
                .current_dir(manifest_dir)
                .args(["show", object.as_str()])
                .output()
                .expect("git must be available to inspect Pi extension history");
            if !file.status.success() {
                // A revision that deletes the file is not an installable stock
                // extension revision.
                continue;
            }

            let content = String::from_utf8(file.stdout)
                .expect("Pi extension history must contain UTF-8 source");
            let hash = integrity::compute_hash_bytes(
                normalize_pi_plugin_line_endings(&content)
                    .trim_end()
                    .as_bytes(),
            );
            assert!(
                KNOWN_PI_PLUGIN_HASHES.contains(&hash.as_str()),
                "Pi extension revision {commit} has unallowlisted hash {hash}"
            );
        }
    }

    #[test]
    fn test_rtk_pi_plugin_marker_tracks_code_not_comments() {
        let code_without_comments: String = PI_PLUGIN
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(looks_like_rtk_pi_plugin(&code_without_comments));
        assert!(looks_like_rtk_pi_plugin(
            "import { exec } from 'pi';\nexec(\"rtk\", [\"rewrite\", cmd]);\n"
        ));
        assert!(!looks_like_rtk_pi_plugin("const note = 'rtk rewrite';\n"));
    }

    #[test]
    fn test_global_uninstall_detects_shared_pi_omp_extension() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        with_omp_dir_override(&tmp, |omp_dir| {
            let omp_path = omp_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            let pi_path = pi_plugin_path_for_scope(true).unwrap();
            assert_eq!(pi_path, omp_path);
            fs::create_dir_all(omp_path.parent().unwrap()).unwrap();
            fs::write(&omp_path, PI_PLUGIN).unwrap();
            record_managed_agent(
                true,
                &omp_path,
                PiCompatibleAgent::Pi,
                false,
                InitContext::default(),
            )
            .unwrap();
            assert_eq!(
                extension_share_status(true, &omp_path, PiCompatibleAgent::Omp).unwrap(),
                ExtensionShareStatus::Shared
            );

            fs::create_dir_all(OMP_LOCAL_DIR).unwrap();
            assert!(
                extension_share_status(true, &pi_path, PiCompatibleAgent::Pi).unwrap()
                    == ExtensionShareStatus::NotShared,
                "a definitive Pi-only sidecar must override an unrelated project-local OMP directory"
            );

            record_managed_agent(
                true,
                &omp_path,
                PiCompatibleAgent::Omp,
                true,
                InitContext::default(),
            )
            .unwrap();
            assert_eq!(
                extension_share_status(true, &pi_path, PiCompatibleAgent::Pi).unwrap(),
                ExtensionShareStatus::Shared
            );
        });
        std::env::set_current_dir(&cwd).unwrap();
    }
}
