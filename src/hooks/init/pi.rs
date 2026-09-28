//! Pi coding agent and Oh My Pi (OMP). Both load the same Pi extension file, so its install,
//! uninstall, stock-content checks and the shared-ownership tracking between the two agents
//! live together here.
use super::*;
use crate::hooks::constants::{
    OMP_DIR, OMP_LOCAL_DIR, PI_AGENT_STATE_FILE, PI_CODING_AGENT_DIR_ENV, PI_DIR,
    PI_EXTENSIONS_SUBDIR, PI_LOCAL_DIR, PI_PLUGIN_FILE,
};

const PI_PLUGIN: &str = include_str!("../../../hooks/pi/rtk.ts");

const PI_PLUGIN_REWRITE_MARKER: &str = "exec(\"rtk\", [\"rewrite\"";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PiCompatibleAgent {
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

enum ManagedAgentState {
    Absent,
    Known(Vec<PiCompatibleAgent>),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionShareStatus {
    NotShared,
    Shared,
    Unknown,
}

const KNOWN_PI_PLUGIN_HASHES: &[&str] = &[
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

/// Resolve Pi config directory, honouring `PI_CODING_AGENT_DIR` override.
fn resolve_pi_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(PI_CODING_AGENT_DIR_ENV)
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    resolve_home_subdir(PI_DIR)
}

/// Return the path to the installed Pi extension file.
fn pi_plugin_path(pi_dir: &Path) -> PathBuf {
    pi_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE)
}

/// Return the Pi extension install path for the given scope.
/// global=true  → `$PI_CODING_AGENT_DIR/extensions/rtk.ts`
/// global=false → `./.pi/extensions/rtk.ts`
fn pi_plugin_path_for_scope(global: bool) -> Result<PathBuf> {
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
fn ensure_pi_extensions_dir(parent: &Path, name: &str, ctx: InitContext) -> Result<()> {
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
fn validate_stock_pi_plugin_path(
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

fn normalize_pi_plugin_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn is_current_pi_plugin(content: &str) -> bool {
    normalize_pi_plugin_line_endings(content).trim_end()
        == normalize_pi_plugin_line_endings(PI_PLUGIN).trim_end()
}

fn looks_like_rtk_pi_plugin(content: &str) -> bool {
    content.contains(PI_PLUGIN_REWRITE_MARKER)
}

fn is_known_stock_pi_plugin(content: &str) -> bool {
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
fn extension_paths_alias(global: bool, path: &Path, agent: PiCompatibleAgent) -> Result<bool> {
    let other_path = match agent {
        PiCompatibleAgent::Pi => omp_extension_path_for_scope(global)?,
        PiCompatibleAgent::Omp => pi_plugin_path_for_scope(global)?,
    };

    Ok(canonicalize_path_for_comparison(path) == canonicalize_path_for_comparison(&other_path))
}

fn shared_agent_state_path(path: &Path) -> PathBuf {
    canonicalize_path_for_comparison(path).with_file_name(PI_AGENT_STATE_FILE)
}

fn read_managed_agents(path: &Path) -> Result<ManagedAgentState> {
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

fn record_managed_agent(
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

fn remove_managed_agent_state(state_path: &Path, ctx: InitContext) -> Result<()> {
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
fn extension_share_status(
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

fn extension_scope_name(global: bool) -> &'static str {
    if global { "global" } else { "project" }
}

fn warn_if_extension_shared_on_install(
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

fn confirm_shared_extension_uninstall(
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

fn read_extension_for_uninstall(
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

/// Uninstall the Pi extension for the given scope.
///
/// Like `codex::uninstall_codex` and `hermes::uninstall_hermes`, this is the per-agent half
/// that `uninstall_with_patch_mode` in `mod.rs` dispatches to, so it can be tested on its own.
pub(super) fn uninstall_pi_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
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

fn print_pi_result(plugin_path: &Path, installed: bool) {
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

#[cfg(test)]
fn with_pi_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    let pi_dir = tmp.path().join("pi_agent");
    fs::create_dir_all(&pi_dir).unwrap();

    temp_env::with_var(PI_CODING_AGENT_DIR_ENV, Some(&pi_dir), || f(&pi_dir));
}

#[cfg(test)]
fn with_omp_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    // OMP reuses PI_CODING_AGENT_DIR, so share the Pi environment lock.
    let omp_dir = tmp.path().join("omp_agent");
    fs::create_dir_all(&omp_dir).unwrap();

    temp_env::with_var(PI_CODING_AGENT_DIR_ENV, Some(&omp_dir), || f(&omp_dir));
}

/// Return the OMP extension install path for the given scope.
fn omp_extension_path_for_scope(global: bool) -> Result<PathBuf> {
    if global {
        Ok(resolve_omp_dir()?
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE))
    } else {
        Ok(PathBuf::from(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE))
    }
}

/// Resolve OMP's global agent directory. OMP itself uses
/// `PI_CODING_AGENT_DIR` for this relocation, so RTK follows the same
/// override instead of introducing a second path configuration.
fn resolve_omp_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(PI_CODING_AGENT_DIR_ENV)
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    resolve_home_subdir(OMP_DIR)
}

/// Install the shared Pi extension file for OMP with an explicit
/// confirmation policy for an existing non-stock file.
pub fn run_omp_mode_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let path = omp_extension_path_for_scope(global)?;
    let extension_was_present = path.exists();

    warn_if_extension_shared_on_install(global, &path, PiCompatibleAgent::Omp)?;

    if !validate_stock_pi_plugin_path(&path, "OMP extension", patch_mode, ctx)? {
        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "OMP extension at {} was not changed; remove or back up the file manually before retrying.",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        ensure_pi_extensions_dir(
            parent,
            if global {
                "OMP extensions directory"
            } else {
                "local OMP extensions directory"
            },
            ctx,
        )?;
    }

    let installed =
        write_if_changed_allow_read_error(path.as_path(), PI_PLUGIN, "OMP extension", ctx)?;
    record_managed_agent(
        global,
        &path,
        PiCompatibleAgent::Omp,
        extension_was_present,
        ctx,
    )?;

    if dry_run {
        print_dry_run_footer();
    } else {
        print_omp_result(&path, installed);
    }

    Ok(())
}

fn print_omp_result(extension_path: &Path, installed: bool) {
    let status = if installed {
        "installed"
    } else {
        "already up to date"
    };
    println!("RTK OMP extension {}:", status);
    println!("  Extension: {}", extension_path.display());
    println!();
    println!("OMP will load the extension automatically on next start.");
}

/// Uninstall the OMP extension with an explicit confirmation policy for a
/// global path shared with Pi.
pub(super) fn uninstall_omp_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let path = omp_extension_path_for_scope(global)?;

    if !path.exists() {
        if dry_run {
            print_dry_run_footer();
        } else {
            println!("RTK OMP extension was not installed (nothing to remove)");
        }
        return Ok(());
    }

    let ownership_state_path = shared_agent_state_path(&path);
    let Some(content) = read_extension_for_uninstall(&path, "OMP extension", ctx)? else {
        return Ok(());
    };

    if is_known_stock_pi_plugin(&content) {
        if !confirm_shared_extension_uninstall(
            global,
            &path,
            PiCompatibleAgent::Omp,
            patch_mode,
            ctx,
        )? {
            if dry_run {
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "Shared Pi/OMP extension at {} was not removed; rerun with --auto-patch to approve the removal.",
                path.display()
            );
        }

        if dry_run {
            println!("[dry-run] would remove OMP extension: {}", path.display());
            remove_managed_agent_state(&ownership_state_path, ctx)?;
            print_dry_run_footer();
        } else {
            // nosemgrep: filesystem-deletion -- OMP uninstall removes only the RTK-managed extension file.
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove OMP extension: {}", path.display()))?;
            remove_managed_agent_state(&ownership_state_path, ctx)?;
            if verbose > 0 {
                eprintln!("Removed OMP extension: {}", path.display());
            }
            println!("RTK uninstalled (OMP):");
            println!("  - Extension: {}", path.display());
            println!("\nRestart OMP to apply changes.");
        }
    } else if looks_like_rtk_pi_plugin(&content) {
        if dry_run {
            println!(
                "[dry-run] would refuse to remove OMP extension: {}",
                path.display()
            );
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "OMP extension at {} contains RTK content that does not match the stock extension. Remove the file manually.",
            path.display()
        );
    } else {
        println!(
            "OMP extension at {} is not RTK content; leaving it alone.",
            path.display()
        );
        if dry_run {
            print_dry_run_footer();
        }
    }

    Ok(())
}

/// Show OMP configuration status.
pub(super) fn show_omp_config() -> Result<()> {
    let global_extension = omp_extension_path_for_scope(true)?;
    let project_extension = omp_extension_path_for_scope(false)?;

    println!("rtk Configuration (Oh My Pi):\n");
    print_omp_extension_status("Global extension", &global_extension)?;
    print_omp_extension_status("Project extension", &project_extension)?;

    println!("\nUsage:");
    println!("  rtk init --agent omp                 # Configure ./.omp/extensions/rtk.ts");
    println!(
        "  rtk init -g --agent omp              # Configure {}",
        global_extension.display()
    );
    println!("  rtk init --agent omp --uninstall     # Remove project OMP RTK extension");
    println!("  rtk init -g --agent omp --uninstall  # Remove global OMP RTK extension");

    Ok(())
}

fn print_omp_extension_status(label: &str, path: &Path) -> Result<()> {
    if path.exists() {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => {
                println!("  {}: {} (unreadable)", label, path.display());
                return Ok(());
            }
        };
        if is_current_pi_plugin(&content) {
            println!("  {}: {} (up to date)", label, path.display());
        } else if is_known_stock_pi_plugin(&content) {
            println!(
                "  {}: {} (stock version - will be replaced on next rtk init)",
                label,
                path.display()
            );
        } else if looks_like_rtk_pi_plugin(&content) {
            println!(
                "  {}: {} (modified RTK content - rtk init will ask before overwriting; use --auto-patch to replace)",
                label,
                path.display()
            );
        } else {
            println!(
                "  {}: {} (unrelated content - rtk init will ask before overwriting; use --auto-patch to replace)",
                label,
                path.display()
            );
        }
    } else {
        println!("  {}: {} (not installed)", label, path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// Install the Pi extension (hook-only; no AGENTS.md injection).
    ///
    /// global=true  → `$PI_CODING_AGENT_DIR/extensions/rtk.ts`
    /// global=false → `.pi/extensions/rtk.ts`
    fn run_pi_mode(global: bool, ctx: InitContext) -> Result<()> {
        run_pi_mode_with_patch_mode(global, PatchMode::Ask, ctx)
    }

    /// Install the shared Pi extension file for OMP (hook-only; no AGENTS.md
    /// injection). OMP loads the file through its `legacy-pi-compat` layer.
    ///
    /// global=true  -> `$HOME/.omp/agent/extensions/rtk.ts`
    /// global=false -> `.omp/extensions/rtk.ts`
    fn run_omp_mode(global: bool, ctx: InitContext) -> Result<()> {
        run_omp_mode_with_patch_mode(global, PatchMode::Ask, ctx)
    }

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
    fn test_run_pi_mode_global_does_not_create_agents_md() {
        let tmp = TempDir::new().unwrap();
        with_pi_dir_override(&tmp, |pi_dir| {
            run_pi_mode(true, InitContext::default()).unwrap();

            let agents_md = pi_dir.join(AGENTS_MD);
            assert!(!agents_md.exists(), "AGENTS.md must not be created");
        });
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
                    ..Default::default()
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
                    ..Default::default()
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
    fn test_known_pi_plugin_hashes_are_sha256() {
        assert!(
            KNOWN_PI_PLUGIN_HASHES.len() >= 8,
            "historical Pi extension hashes must not be removed"
        );
        assert!(
            KNOWN_PI_PLUGIN_HASHES
                .iter()
                .all(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        );

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

    #[test]
    fn test_omp_extension_path_for_scope_local() {
        let path = omp_extension_path_for_scope(false).unwrap();
        assert_eq!(
            path,
            PathBuf::from(OMP_LOCAL_DIR)
                .join(PI_EXTENSIONS_SUBDIR)
                .join(PI_PLUGIN_FILE)
        );
    }

    #[test]
    fn test_omp_extension_path_for_scope_global_honours_pi_dir_override() {
        let tmp = TempDir::new().unwrap();
        with_omp_dir_override(&tmp, |omp_dir| {
            let path = omp_extension_path_for_scope(true).unwrap();
            assert_eq!(
                path,
                omp_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE)
            );
        });
    }

    #[test]
    fn test_omp_global_install_and_uninstall_use_override() {
        let tmp = TempDir::new().unwrap();
        with_omp_dir_override(&tmp, |omp_dir| {
            run_omp_mode(true, InitContext::default()).unwrap();

            let plugin = omp_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            assert!(plugin.exists(), "global OMP extension must be created");
            let state_path = shared_agent_state_path(&plugin);
            assert_eq!(
                fs::read_to_string(&state_path).unwrap(),
                "omp\n",
                "OMP install must record its ownership"
            );

            uninstall_with_patch_mode(
                true,
                false,
                false,
                false,
                false,
                true,
                PatchMode::Auto,
                InitContext::default(),
            )
            .unwrap();
            assert!(!plugin.exists(), "global OMP extension must be removed");
            assert!(!state_path.exists(), "ownership state must be removed");
        });
    }

    #[test]
    fn test_omp_local_install_writes_shared_pi_extension() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(false, InitContext::default()).unwrap();
        std::env::set_current_dir(&cwd).unwrap();

        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.trim(), PI_PLUGIN.trim());
    }

    #[test]
    fn test_omp_install_refuses_modified_extension() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_omp_mode_with_patch_mode(false, PatchMode::Skip, InitContext::default());
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
    fn test_omp_install_dry_run_reports_refusal_without_error() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_omp_mode(
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
    fn test_omp_local_install_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(
            false,
            InitContext {
                verbose: 0,
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        std::env::set_current_dir(&cwd).unwrap();

        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(!path.exists());
        assert!(!tmp.path().join(OMP_LOCAL_DIR).exists());
    }

    #[test]
    fn test_omp_local_uninstall_removes_plugin() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(false, InitContext::default()).unwrap();
        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext::default(),
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(!path.exists());
    }

    #[test]
    fn test_omp_local_uninstall_dry_run_keeps_plugin() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        run_omp_mode(false, InitContext::default()).unwrap();
        let plugin = tmp
            .path()
            .join(OMP_LOCAL_DIR)
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
            false,
            true,
            InitContext {
                verbose: 0,
                dry_run: true,
                ..Default::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        assert!(
            plugin.exists(),
            "dry-run uninstall must not remove the local OMP extension"
        );
    }

    #[test]
    fn test_omp_uninstall_modified_extension_bails() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// user-modified extension\nexport default (pi) => { pi.exec(\"rtk\", [\"rewrite\", cmd]) }\n",
        )
        .unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
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
    fn test_omp_uninstall_modified_extension_dry_run_is_preview() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// user-modified extension\nexport default (pi) => { pi.exec(\"rtk\", [\"rewrite\", cmd]) }\n",
        )
        .unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
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
    fn test_omp_uninstall_unreadable_extension_is_left_alone() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
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
    fn test_omp_uninstall_unrelated_content_dry_run_left_alone() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// rtk rewrite is mentioned here\nexport default () => {}\n",
        )
        .unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();

        assert!(path.exists(), "non-RTK extension must be left in place");
    }

    #[test]
    fn test_omp_uninstall_missing_dry_run_is_noop() {
        let tmp = TempDir::new().unwrap();
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = uninstall(
            false,
            false,
            false,
            false,
            false,
            true,
            InitContext {
                dry_run: true,
                ..InitContext::default()
            },
        );
        std::env::set_current_dir(&cwd).unwrap();
        result.unwrap();
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
    fn test_run_pi_mode_global_creates_plugin_when_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let absent_dir = tmp.path().join("no_such_pi_dir");
        temp_env::with_var(PI_CODING_AGENT_DIR_ENV, Some(&absent_dir), || {
            run_pi_mode(true, InitContext::default())
        })
        .unwrap();

        let plugin = absent_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
        assert!(
            plugin.exists(),
            "plugin must be written even when dir was absent"
        );

        let agents_md = absent_dir.join(AGENTS_MD);
        assert!(!agents_md.exists(), "AGENTS.md must not be created");
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
                ..Default::default()
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
                ..Default::default()
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
}
