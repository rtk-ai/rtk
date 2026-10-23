//! Pi coding agent and Oh My Pi (OMP). Both load the same Pi extension file, so its install,
//! uninstall, stock-content checks and the shared-ownership tracking between the two agents
//! live together here.
use super::*;
#[cfg(test)]
use crate::core::test_isolation;
use crate::core::user_dirs;
use crate::core::user_env;
use crate::hooks::constants::{
    OMP_DIR, OMP_LOCAL_DIR, PI_AGENT_STATE_FILE, PI_AGENT_STATE_UNKNOWN_PRIOR,
    PI_CODING_AGENT_DIR_ENV, PI_DIR, PI_EXTENSIONS_SUBDIR, PI_LOCAL_DIR, PI_PLUGIN_FILE,
};

const PI_PLUGIN: &str = include_str!("../../../hooks/pi/rtk.ts");

const PI_PLUGIN_REWRITE_MARKER: &str = "exec(\"rtk\", [\"rewrite\"";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiCompatibleAgent {
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

    fn extension_path_for_scope(self, global: bool) -> Result<PathBuf> {
        match self {
            Self::Pi => pi_plugin_path_for_scope(global),
            Self::Omp => omp_extension_path_for_scope(global),
        }
    }

    /// The program the uninstall message asks the user to restart.
    fn program_name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Omp => "OMP",
        }
    }
}

enum ManagedAgentState {
    Absent,
    /// Ownership was recorded. `prior_unknown` is the persisted marker: the record was
    /// started over an extension that already existed, so it cannot be assumed exhaustive.
    Known {
        agents: Vec<PiCompatibleAgent>,
        prior_unknown: bool,
    },
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
    "def594058835ce23d82abd6872a8359144a4486ed0905ff12e59fedb2260b472",
];

/// Resolve Pi config directory, honouring `PI_CODING_AGENT_DIR` override.
fn resolve_pi_dir() -> Result<PathBuf> {
    if let Some(dir) = user_env::var(PI_CODING_AGENT_DIR_ENV)
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
        Ok(user_dirs::in_working_dir(PI_LOCAL_DIR)
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

/// One agent's extension in one scope, and whether the other agent's path for the
/// same scope resolves to the same file. Resolved once when a command starts, so its
/// reads of the ownership record, its warnings and its decisions act on one answer.
/// The one exception is `record_managed_agent`, which resolves a site that was not
/// shared at the start again, since the install itself can make the paths meet.
struct ExtensionSite {
    agent: PiCompatibleAgent,
    global: bool,
    path: PathBuf,
    /// The ownership record, beside the file the path resolves to.
    state_path: PathBuf,
    /// The other agent's extension path for this scope resolves to the same file.
    shared_path: bool,
}

impl ExtensionSite {
    fn resolve(agent: PiCompatibleAgent, global: bool) -> Result<Self> {
        let path = agent.extension_path_for_scope(global)?;
        let canonical = canonicalize_path_for_comparison(&path);
        let other_path = agent.other().extension_path_for_scope(global)?;
        Ok(Self {
            agent,
            global,
            state_path: canonical.with_file_name(PI_AGENT_STATE_FILE),
            shared_path: canonical == canonicalize_path_for_comparison(&other_path),
            path,
        })
    }

    /// Reads the sidecar only when both agents resolve to this path. Elsewhere the
    /// record plays no part in the command, so reporting on its contents would
    /// announce an ownership decision that is never made.
    fn read_ownership(&self) -> Result<ManagedAgentState> {
        self.read_ownership_reporting(true)
    }

    fn read_ownership_reporting(&self, warn: bool) -> Result<ManagedAgentState> {
        if self.shared_path {
            read_managed_agents_reporting(&self.state_path, warn)
        } else {
            Ok(ManagedAgentState::Absent)
        }
    }
}

/// Reads the ownership sidecar. `warn` is false for a re-read after a question or
/// before a write, where a warning would repeat what the command's first read
/// reported, or describe a record the write is about to replace.
fn read_managed_agents_reporting(state_path: &Path, warn: bool) -> Result<ManagedAgentState> {
    macro_rules! report {
        ($($arg:tt)*) => {
            if warn {
                eprintln!($($arg)*);
            }
        };
    }
    let content = match fs::read_to_string(state_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedAgentState::Absent);
        }
        Err(error) => {
            report!(
                "[warn] RTK extension ownership state at {} could not be read; treating ownership as unknown: {}",
                state_path.display(),
                error
            );
            return Ok(ManagedAgentState::Unknown);
        }
    };
    let mut agents = Vec::new();
    let mut prior_unknown = false;
    let mut has_invalid_entry = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == PI_AGENT_STATE_UNKNOWN_PRIOR {
            prior_unknown = true;
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
        report!(
            "[warn] RTK extension ownership state at {} contains unknown entries; treating ownership as unknown",
            state_path.display()
        );
        return Ok(ManagedAgentState::Unknown);
    }

    // A record carrying only the marker still states that ownership is partial, and must
    // stay writable, or the next install can never repair it.
    if agents.is_empty() && !prior_unknown {
        report!(
            "[warn] RTK extension ownership state at {} is empty; treating ownership as unknown",
            state_path.display()
        );
        return Ok(ManagedAgentState::Unknown);
    }

    Ok(ManagedAgentState::Known {
        agents,
        prior_unknown,
    })
}

fn record_managed_agent(
    site: &ExtensionSite,
    extension_was_present: bool,
    ctx: InitContext,
) -> Result<()> {
    // The install can itself make both agents' paths resolve to one file, by
    // creating the directory a link points to or writing the file a link names,
    // so a site that was not shared when the command started is resolved again.
    let resolved_now;
    let site = if site.shared_path {
        site
    } else {
        resolved_now = ExtensionSite::resolve(site.agent, site.global)?;
        &resolved_now
    };
    if !site.shared_path {
        return Ok(());
    }

    let (state_path, agent) = (&site.state_path, site.agent);
    // Re-read rather than merging into the caller's snapshot: an `Ask` prompt
    // sits between that read and this write, so another process may have
    // recorded itself in the meantime and must not be dropped. Quietly: the
    // caller's read already reported on a record that bears on an existing
    // extension, and a record RTK cannot read beside no extension is replaced
    // below; one it keeps is reported by the arm that keeps it.
    let state = &read_managed_agents_reporting(state_path, false)?;
    // Recorded agents are those known to use this path. Deleting the extension
    // does not unconfigure an agent that points here, so a record RTK can read is
    // merged into rather than replaced.
    let (mut agents, mut prior_unknown) = match state {
        // No record yet. If the extension already existed, somebody installed
        // it and RTK cannot know who, so the new record is explicitly partial.
        ManagedAgentState::Absent => (Vec::new(), extension_was_present),
        ManagedAgentState::Known {
            agents,
            prior_unknown,
        } => (agents.clone(), *prior_unknown),
        // With no extension beside it, a record RTK cannot read describes nothing that
        // exists, so this install starts a new one.
        ManagedAgentState::Unknown if !extension_was_present => (Vec::new(), false),
        ManagedAgentState::Unknown => {
            // Do not overwrite state RTK failed to parse: that would turn an
            // unreadable record into a confident one.
            eprintln!(
                "[warn] RTK extension ownership state at {} could not be updated because ownership is unknown; preserving it and proceeding without recording {}",
                state_path.display(),
                agent.state_name()
            );
            return Ok(());
        }
    };
    if !agents.contains(&agent) {
        agents.push(agent);
    }
    agents.sort_by_key(|agent| agent.state_name());
    // A record listing both agents is exhaustive on its own.
    if agents.len() == 2 {
        prior_unknown = false;
    }
    let content = serialize_managed_agents(&agents, prior_unknown);
    write_if_changed_allow_read_error(state_path, &content, "RTK extension ownership state", ctx)?;
    Ok(())
}

fn serialize_managed_agents(agents: &[PiCompatibleAgent], prior_unknown: bool) -> String {
    let mut entries: Vec<&str> = agents.iter().map(|agent| agent.state_name()).collect();
    if prior_unknown {
        entries.push(PI_AGENT_STATE_UNKNOWN_PRIOR);
    }
    format!("{}\n", entries.join("\n"))
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
///
/// Takes the ownership state rather than reading it, so the caller decides when
/// the record is read and whether a re-read reports on it again.
fn extension_share_status(site: &ExtensionSite, state: &ManagedAgentState) -> ExtensionShareStatus {
    if !site.shared_path {
        return ExtensionShareStatus::NotShared;
    }

    let other_agent = site.agent.other();
    match state {
        ManagedAgentState::Known {
            agents,
            prior_unknown,
        } => {
            if agents.contains(&other_agent) {
                ExtensionShareStatus::Shared
            } else if *prior_unknown {
                // A partial record lists agents known to use the file, not every one
                // that does, so the other agent's absence from it proves nothing.
                ExtensionShareStatus::Unknown
            } else {
                ExtensionShareStatus::NotShared
            }
        }
        // No probe of `~/.pi`/`~/.omp` here: whether those directories exist says
        // nothing reliable about which agents use an aliased path, and reading their
        // absence as an uninstall would claim sole ownership over a shared file.
        ManagedAgentState::Absent | ManagedAgentState::Unknown => ExtensionShareStatus::Unknown,
    }
}

fn extension_scope_name(global: bool) -> &'static str {
    if global { "global" } else { "project" }
}

fn warn_if_extension_shared_on_install(
    site: &ExtensionSite,
    extension_was_present: bool,
    state: &ManagedAgentState,
) {
    let (scope, path, agent) = (extension_scope_name(site.global), &site.path, site.agent);
    match extension_share_status(site, state) {
        ExtensionShareStatus::NotShared => {}
        ExtensionShareStatus::Shared => eprintln!(
            "[warn] Pi and OMP share the {} extension path at {}; installing {} here enables the shared integration for both agents.",
            scope,
            path.display(),
            agent.name()
        ),
        // Nothing was there to have an owner, and this install records one, so
        // an uncertainty warning would only be noise on a first-ever install.
        ExtensionShareStatus::Unknown if !extension_was_present => {}
        ExtensionShareStatus::Unknown => eprintln!(
            "[warn] Pi and OMP resolve to the same {} extension path at {}, but RTK could not confirm both agents' ownership; installing {} without a definitive ownership record.",
            scope,
            path.display(),
            agent.name()
        ),
    }
}

fn confirm_shared_extension_uninstall(
    site: &ExtensionSite,
    share: ExtensionShareStatus,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    let (scope, path, agent) = (extension_scope_name(site.global), &site.path, site.agent);
    match share {
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
            // The caller reports the declined removal on stderr and exits
            // non-zero; announcing it here too would state the outcome twice,
            // once on a stream that reads as success.
            prompt_user_confirmation(&prompt)
        }
    }
}

/// Decide whether RTK content that no longer matches a known stock revision may
/// be removed. Mirrors the install side's Ask/Auto/Skip policy so `--auto-patch`
/// approves a removal the same way it approves an overwrite, instead of leaving
/// a hand-edited or fork-built extension permanently un-uninstallable.
fn confirm_modified_extension_removal(
    path: &Path,
    name: &str,
    reach: &str,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    if ctx.dry_run {
        let action = match patch_mode {
            PatchMode::Ask => "prompt before removing modified",
            PatchMode::Auto => "remove modified",
            PatchMode::Skip => "refuse to remove",
        };
        println!("[dry-run] would {action} {name}: {}", path.display());
        if !reach.is_empty() {
            println!("[dry-run] {}", reach.trim_start());
        }
        return Ok(matches!(patch_mode, PatchMode::Auto));
    }

    match patch_mode {
        PatchMode::Auto => Ok(true),
        PatchMode::Skip => Ok(false),
        PatchMode::Ask => prompt_user_confirmation(&format!(
            "{} at {} has local changes, which will be lost.{} Remove it?",
            name,
            path.display(),
            reach
        )),
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

/// Uninstall one agent's extension for the given scope.
///
/// Like `codex::uninstall_codex` and `hermes::uninstall_hermes`, this is the per-agent half
/// that `uninstall_with_patch_mode` in `mod.rs` dispatches to, so it can be tested on its own.
pub(super) fn uninstall_pi_compatible_with_patch_mode(
    agent: PiCompatibleAgent,
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let site = ExtensionSite::resolve(agent, global)?;
    let path = &site.path;
    let name = format!("{} extension", agent.name());

    if !path.exists() {
        if dry_run {
            print_dry_run_footer();
        } else {
            println!("RTK {name} was not installed (nothing to remove)");
        }
        return Ok(());
    }

    let Some(content) = read_extension_for_uninstall(path, &name, ctx)? else {
        return Ok(());
    };

    let stock = is_known_stock_pi_plugin(&content);
    if !stock && !looks_like_rtk_pi_plugin(&content) {
        println!(
            "{name} at {} is not RTK content; leaving it alone.",
            path.display()
        );
        // The ownership record is deliberately left in place: "not RTK
        // content" rests on a substring match, so a merely reformatted
        // extension lands here, and discarding the record would silently
        // remove the shared-file protection for both agents.
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(());
    }

    let mut share = extension_share_status(&site, &site.read_ownership()?);

    if !stock {
        // Removing the file removes it for the other agent too when it loads the
        // same file, so the question and the refusal about local changes say so.
        let other = agent.other().name();
        let reach = match share {
            ExtensionShareStatus::NotShared => String::new(),
            ExtensionShareStatus::Shared => {
                format!(" {other} loads the same file, so removing it removes it for {other} too.")
            }
            ExtensionShareStatus::Unknown => {
                format!(" {other} may load the same file, and would lose it too.")
            }
        };
        if !confirm_modified_extension_removal(path, &name, &reach, patch_mode, ctx)? {
            if dry_run {
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "{name} at {} contains RTK content that does not match the stock extension.{reach} Remove the file manually, or rerun with --auto-patch to remove it.",
                path.display()
            );
        }
        if matches!(patch_mode, PatchMode::Ask) && !dry_run {
            // The question may have waited on the user while the other agent
            // recorded itself, so the decision re-reads the record. Quietly: the
            // read above already reported anything wrong with it.
            share = extension_share_status(&site, &site.read_ownership_reporting(false)?);
        }
    }

    if !confirm_shared_extension_uninstall(&site, share, patch_mode, ctx)? {
        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "Shared Pi/OMP extension at {} was not removed; rerun with --auto-patch to remove it for both agents.",
            path.display()
        );
    }

    if dry_run {
        println!("[dry-run] would remove {name}: {}", path.display());
        remove_managed_agent_state(&site.state_path, ctx)?;
        print_dry_run_footer();
    } else {
        // nosemgrep: filesystem-deletion -- uninstall removes only RTK content, stock or approved.
        fs::remove_file(path)
            .with_context(|| format!("Failed to remove {name}: {}", path.display()))?;
        remove_managed_agent_state(&site.state_path, ctx)?;
        if verbose > 0 {
            eprintln!("Removed {name}: {}", path.display());
        }
        println!("RTK uninstalled ({}):", agent.name());
        println!("  - {name}: {}", path.display());
        println!("\nRestart {} to apply changes.", agent.program_name());
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
    install_pi_compatible(PiCompatibleAgent::Pi, global, patch_mode, ctx)
}

/// Install the shared Pi extension file for OMP with an explicit
/// confirmation policy for an existing non-stock file.
pub fn run_omp_mode_with_patch_mode(
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    install_pi_compatible(PiCompatibleAgent::Omp, global, patch_mode, ctx)
}

fn install_pi_compatible(
    agent: PiCompatibleAgent,
    global: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let site = ExtensionSite::resolve(agent, global)?;
    let path = &site.path;
    let name = format!("{} extension", agent.name());
    let extension_was_present = path.exists();

    let ownership_state = site.read_ownership()?;

    if !validate_stock_pi_plugin_path(path, &name, patch_mode, ctx)? {
        if dry_run {
            print_dry_run_footer();
            return Ok(());
        }
        anyhow::bail!(
            "{name} at {} was not changed; remove or back up the file manually before retrying.",
            path.display()
        );
    }

    // Only once the write is going ahead: announcing a shared install before the
    // overwrite is approved would assert something that may never happen.
    warn_if_extension_shared_on_install(&site, extension_was_present, &ownership_state);

    if let Some(parent) = path.parent() {
        let scope = if global { "" } else { "local " };
        ensure_pi_extensions_dir(
            parent,
            &format!("{scope}{} extensions directory", agent.name()),
            ctx,
        )?;
    }

    let installed = write_if_changed_allow_read_error(path, PI_PLUGIN, &name, ctx)?;
    record_managed_agent(&site, extension_was_present, ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else {
        match agent {
            PiCompatibleAgent::Pi => print_pi_result(path, installed),
            PiCompatibleAgent::Omp => print_omp_result(path, installed),
        }
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

    test_isolation::with_agent_dir(tmp.path(), PI_CODING_AGENT_DIR_ENV, &pi_dir, || f(&pi_dir));
}

#[cfg(test)]
fn with_omp_dir_override<F: FnOnce(&Path)>(tmp: &TempDir, f: F) {
    let omp_dir = tmp.path().join("omp_agent");
    fs::create_dir_all(&omp_dir).unwrap();

    test_isolation::with_agent_dir(tmp.path(), PI_CODING_AGENT_DIR_ENV, &omp_dir, || {
        f(&omp_dir)
    });
}

/// Return the OMP extension install path for the given scope.
fn omp_extension_path_for_scope(global: bool) -> Result<PathBuf> {
    if global {
        Ok(resolve_omp_dir()?
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE))
    } else {
        Ok(user_dirs::in_working_dir(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE))
    }
}

/// Resolve OMP's global agent directory. OMP itself uses
/// `PI_CODING_AGENT_DIR` for this relocation, so RTK follows the same
/// override instead of introducing a second path configuration.
fn resolve_omp_dir() -> Result<PathBuf> {
    if let Some(dir) = user_env::var(PI_CODING_AGENT_DIR_ENV)
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    resolve_home_subdir(OMP_DIR)
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

/// Show the extension status for a Pi-compatible agent. Pi and OMP install the
/// same file, so they share every state the report distinguishes.
pub(super) fn show_pi_compatible_config(agent: PiCompatibleAgent) -> Result<()> {
    let global_extension = agent.extension_path_for_scope(true)?;
    let project_extension = agent.extension_path_for_scope(false)?;
    let (flag, title) = match agent {
        PiCompatibleAgent::Pi => ("pi", "Pi"),
        PiCompatibleAgent::Omp => ("omp", "Oh My Pi"),
    };

    println!("rtk Configuration ({title}):\n");
    print_extension_status("Global extension", &global_extension)?;
    print_extension_status("Project extension", &project_extension)?;

    println!("\nUsage:");
    println!(
        "  rtk init --agent {flag}                 # Configure {}",
        project_extension.display()
    );
    println!(
        "  rtk init -g --agent {flag}              # Configure {}",
        global_extension.display()
    );
    println!("  rtk init --agent {flag} --uninstall     # Remove project {title} RTK extension");
    println!("  rtk init -g --agent {flag} --uninstall  # Remove global {title} RTK extension");

    Ok(())
}

fn print_extension_status(label: &str, path: &Path) -> Result<()> {
    if path.exists() {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => {
                println!("  {}: {} (unreadable)", label, path.display());
                return Ok(());
            }
        };
        // "Up to date" has to mean the next install writes nothing, so it takes
        // the byte comparison `write_if_changed` uses. A file that only matches
        // after CRLF normalisation is still stock, but it will be rewritten.
        if content == PI_PLUGIN {
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
    /// global=true  -> `$PI_CODING_AGENT_DIR/extensions/rtk.ts`, else
    ///                 `$HOME/.omp/agent/extensions/rtk.ts`
    /// global=false -> `.omp/extensions/rtk.ts`
    fn run_omp_mode(global: bool, ctx: InitContext) -> Result<()> {
        run_omp_mode_with_patch_mode(global, PatchMode::Ask, ctx)
    }

    /// Uninstall the OMP extension with the default `Ask` policy for modified
    /// content.
    fn uninstall_omp(global: bool, ctx: InitContext) -> Result<()> {
        uninstall_pi_compatible_with_patch_mode(PiCompatibleAgent::Omp, global, PatchMode::Ask, ctx)
    }

    #[test]
    fn test_run_pi_mode_global_installs_plugin() {
        let tmp = test_isolation::tempdir();
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
        let tmp = test_isolation::tempdir();
        with_pi_dir_override(&tmp, |pi_dir| {
            run_pi_mode(true, InitContext::default()).unwrap();

            let agents_md = pi_dir.join(AGENTS_MD);
            assert!(!agents_md.exists(), "AGENTS.md must not be created");
        });
    }

    #[test]
    fn test_pi_global_uninstall_removes_plugin() {
        let tmp = test_isolation::tempdir();
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
        let tmp = test_isolation::tempdir();
        with_pi_dir_override(&tmp, |pi_dir| {
            let path = pi_plugin_path_for_scope(true).unwrap();
            assert_eq!(path, pi_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE));
        });
    }

    #[test]
    fn test_pi_plugin_path_for_scope_local() {
        let path = pi_plugin_path_for_scope(false).unwrap();
        assert!(
            path.ends_with(
                PathBuf::from(PI_LOCAL_DIR)
                    .join(PI_EXTENSIONS_SUBDIR)
                    .join(PI_PLUGIN_FILE)
            ),
            "the project's own extension path, got {}",
            path.display()
        );
        let project = user_dirs::current_dir().expect("a test build has a project");
        assert!(
            path.starts_with(&project),
            "in the project, not the home: {}",
            path.display()
        );
    }

    #[test]
    fn test_run_pi_mode_global_dry_run_writes_nothing() {
        let tmp = test_isolation::tempdir();
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
        let tmp = test_isolation::tempdir();
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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_pi_mode_with_patch_mode(false, PatchMode::Skip, InitContext::default());

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        let dir = tmp.path().join(PI_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(&path, format!("{}\n// user modification\n", PI_PLUGIN)).unwrap();

        // Pin the patch mode rather than relying on the `Ask` default: under
        // `cargo test` stdin is the developer's terminal, so `Ask` would block
        // on the confirmation prompt instead of taking the refusal path.
        let result = uninstall_with_patch_mode(
            false,
            false,
            false,
            false,
            true,
            false,
            PatchMode::Skip,
            InitContext::default(),
        );

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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

        // Normalise first: a CRLF checkout would otherwise yield \r\r\n.
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

        let git = |args: &[&str]| {
            let mut cmd = Command::new("git");
            cmd.current_dir(manifest_dir).args(args);
            test_isolation::isolate_git(&mut cmd);
            cmd.output()
        };
        let revisions = git(&[
            "rev-list",
            "HEAD",
            "--full-history",
            "--",
            "hooks/pi/rtk.ts",
        ])
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
            let file = git(&["show", object.as_str()])
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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());
        with_omp_dir_override(&tmp, |omp_dir| {
            let omp_path = omp_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            let pi_path = pi_plugin_path_for_scope(true).unwrap();
            assert_eq!(pi_path, omp_path);
            fs::create_dir_all(omp_path.parent().unwrap()).unwrap();
            fs::write(&omp_path, PI_PLUGIN).unwrap();
            let pi_site = ExtensionSite::resolve(PiCompatibleAgent::Pi, true).unwrap();
            let omp_site = ExtensionSite::resolve(PiCompatibleAgent::Omp, true).unwrap();
            assert!(pi_site.shared_path && omp_site.shared_path);
            record_managed_agent(&pi_site, false, InitContext::default()).unwrap();
            assert_eq!(
                extension_share_status(&omp_site, &omp_site.read_ownership().unwrap()),
                ExtensionShareStatus::Shared
            );

            assert_eq!(
                extension_share_status(&pi_site, &pi_site.read_ownership().unwrap()),
                ExtensionShareStatus::NotShared,
                "a sidecar naming only Pi, with no unknown prior owner, must report the aliased file as not shared"
            );

            record_managed_agent(&omp_site, true, InitContext::default()).unwrap();
            assert_eq!(
                extension_share_status(&pi_site, &pi_site.read_ownership().unwrap()),
                ExtensionShareStatus::Shared
            );
        });
    }

    #[test]
    fn test_omp_extension_path_for_scope_local() {
        let path = omp_extension_path_for_scope(false).unwrap();
        assert!(
            path.ends_with(
                PathBuf::from(OMP_LOCAL_DIR)
                    .join(PI_EXTENSIONS_SUBDIR)
                    .join(PI_PLUGIN_FILE)
            ),
            "the project's own extension path, got {}",
            path.display()
        );
        let project = user_dirs::current_dir().expect("a test build has a project");
        assert!(
            path.starts_with(&project),
            "in the project, not the home: {}",
            path.display()
        );
    }

    #[test]
    fn test_omp_extension_path_for_scope_global_honours_pi_dir_override() {
        let tmp = test_isolation::tempdir();
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
        let tmp = test_isolation::tempdir();
        with_omp_dir_override(&tmp, |omp_dir| {
            run_omp_mode(true, InitContext::default()).unwrap();

            let plugin = omp_dir.join(PI_EXTENSIONS_SUBDIR).join(PI_PLUGIN_FILE);
            assert!(plugin.exists(), "global OMP extension must be created");
            let state_path = ExtensionSite::resolve(PiCompatibleAgent::Omp, true)
                .unwrap()
                .state_path;
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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        run_omp_mode(false, InitContext::default()).unwrap();

        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.trim(), PI_PLUGIN.trim());
    }

    #[test]
    fn test_omp_default_policy_uninstall_removes_stock_extension() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        run_omp_mode(false, InitContext::default()).unwrap();
        let path = tmp
            .path()
            .join(OMP_LOCAL_DIR)
            .join(PI_EXTENSIONS_SUBDIR)
            .join(PI_PLUGIN_FILE);
        assert!(path.exists());

        uninstall_omp(false, InitContext::default()).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn test_omp_install_refuses_modified_extension() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        let modified = "// user-modified extension\nexport default () => {}\n";
        fs::write(&path, modified).unwrap();

        let result = run_omp_mode_with_patch_mode(false, PatchMode::Skip, InitContext::default());

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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

        result.unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), modified);
    }

    #[test]
    fn test_omp_local_install_dry_run_writes_nothing() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        run_omp_mode(
            false,
            InitContext {
                verbose: 0,
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        result.unwrap();

        assert!(
            plugin.exists(),
            "dry-run uninstall must not remove the local OMP extension"
        );
    }

    #[test]
    fn test_omp_uninstall_modified_extension_bails() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        let dir = tmp.path().join(OMP_LOCAL_DIR).join(PI_EXTENSIONS_SUBDIR);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PI_PLUGIN_FILE);
        fs::write(
            &path,
            "// user-modified extension\nexport default (pi) => { pi.exec(\"rtk\", [\"rewrite\", cmd]) }\n",
        )
        .unwrap();

        // Pin the patch mode rather than relying on the `Ask` default: under
        // `cargo test` stdin is the developer's terminal, so `Ask` would block
        // on the confirmation prompt instead of taking the refusal path.
        let result = uninstall_with_patch_mode(
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Skip,
            InitContext::default(),
        );

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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

        result.unwrap();
        assert!(path.exists(), "dry-run must preserve modified extension");
    }

    #[test]
    fn test_omp_uninstall_unreadable_extension_is_left_alone() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        result.unwrap();

        assert!(path.exists(), "non-RTK extension must be left in place");
    }

    #[test]
    fn test_omp_uninstall_missing_dry_run_is_noop() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        result.unwrap();
    }

    #[test]
    fn test_run_pi_mode_local_installs_plugin() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        let result = run_pi_mode(false, InitContext::default());
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
        let tmp = test_isolation::tempdir();
        let absent_dir = tmp.path().join("no_such_pi_dir");
        user_env::with_path(PI_CODING_AGENT_DIR_ENV, Some(&absent_dir), || {
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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

        let result = run_pi_mode(
            false,
            InitContext {
                verbose: 0,
                dry_run: true,
                ..Default::default()
            },
        );
        result.unwrap();

        assert!(
            !tmp.path().join(".pi").join(PI_EXTENSIONS_SUBDIR).exists(),
            "dry-run must not create .pi/extensions/"
        );
    }

    #[test]
    fn test_pi_local_uninstall_dry_run_keeps_plugin() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        result.unwrap();

        assert!(
            plugin.exists(),
            "dry-run uninstall must not remove the local Pi extension"
        );
    }

    #[test]
    fn test_pi_install_dry_run_reports_refusal_without_error() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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

        result.unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), modified);
    }

    #[test]
    fn test_pi_uninstall_unreadable_extension_is_left_alone() {
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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
        let tmp = test_isolation::tempdir();
        let _entered = test_isolation::enter(tmp.path());

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

        assert!(path.exists(), "non-RTK extension must be left in place");
    }
}
