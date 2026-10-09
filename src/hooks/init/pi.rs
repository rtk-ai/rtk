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
}

enum ManagedAgentState {
    Absent,
    /// Ownership was recorded. `prior_unknown` marks a record that cannot be
    /// assumed exhaustive — started over an extension RTK did not install, or
    /// carrying `unparsed` entries written by a version that knows more agents
    /// than this one. Unparsed entries are preserved on every rewrite.
    Known {
        agents: Vec<PiCompatibleAgent>,
        prior_unknown: bool,
        unparsed: Vec<String>,
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
    // `create_dir_all` fails with EEXIST on a symlinked directory whose target
    // is missing: the link exists, its target does not. Create the target.
    let resolved = resolve_symlink_components(parent);
    let parent = resolved.as_path();

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
    // `exists()` follows the link, so a symlink whose chain cannot be resolved
    // — a cycle, or one longer than MAX_SYMLINK_HOPS — reads as absent and
    // would be replaced with no prompt and no backup, unlike every other path
    // that overwrites something the user put there. A *dangling* link is not in
    // that category: it resolves to a target the install is meant to create, so
    // it stays on the ordinary path and is written through.
    let unresolvable_link = fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && resolve_symlink_target(path).is_none();
    if path.exists() || unresolvable_link {
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
                        if let Some(backup) = plan_copy_backup(path, name)? {
                            println!("[dry-run] would back up {} to {}", name, backup.display());
                        }
                        println!(
                            "[dry-run] would prompt before overwriting {}: {}",
                            name,
                            path.display()
                        );
                        Ok(false)
                    }
                    PatchMode::Auto => {
                        if let Some(backup) = plan_copy_backup(path, name)? {
                            println!("[dry-run] would back up {} to {}", name, backup.display());
                        }
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
                    // Disclose the backup, as the uninstall prompt does: a user
                    // protecting local edits would otherwise decline a question
                    // whose answer preserves them.
                    let fate = match plan_copy_backup(path, name)? {
                        Some(backup) => format!("back it up to {} and overwrite", backup.display()),
                        None => "overwrite".to_owned(),
                    };
                    let prompt = format!(
                        "{} at {} is not stock content; {} it?",
                        name,
                        path.display(),
                        fate
                    );
                    prompt_user_confirmation(&prompt)?
                }
            };

            if should_overwrite {
                // The overwrite is about to discard whatever the user had here,
                // and unlike the stock content replacing it, that is not
                // reproducible from anywhere else.
                back_up_modified_extension(path, name)?;
            }

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
    read_managed_agents_reporting(path, true)
}

/// Reads the sidecar only when both agents resolve to this path. Elsewhere the
/// record plays no part in the command, so reporting on its contents would
/// announce an ownership decision that is never made.
fn read_ownership_if_aliased(
    global: bool,
    path: &Path,
    agent: PiCompatibleAgent,
) -> Result<ManagedAgentState> {
    if extension_paths_alias(global, path, agent)? {
        read_managed_agents(path)
    } else {
        Ok(ManagedAgentState::Absent)
    }
}

/// Reads the ownership sidecar. `warn` is false for the pre-write re-read,
/// where any problem has already been reported by the earlier read and a second
/// identical warning would only be noise.
fn read_managed_agents_reporting(path: &Path, warn: bool) -> Result<ManagedAgentState> {
    macro_rules! report {
        ($($arg:tt)*) => {
            if warn {
                eprintln!($($arg)*);
            }
        };
    }
    let state_path = shared_agent_state_path(path);
    let content = match fs::read_to_string(&state_path) {
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
    let mut unparsed = Vec::new();
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
            None => unparsed.push(line.to_owned()),
        }
    }

    if !unparsed.is_empty() {
        // An entry RTK cannot parse says nothing about the ones it can. Keep the
        // recognised owners so they still protect the file, and mark the record
        // partial so a missing agent is not read as proof of sole ownership.
        prior_unknown = true;
    }

    if agents.is_empty() {
        // A record carrying only the marker, or only entries written by a newer
        // RTK, still states a usable fact — ownership is partial — and must stay
        // writable, or the next install can never repair it.
        if prior_unknown || !unparsed.is_empty() {
            return Ok(ManagedAgentState::Known {
                agents,
                prior_unknown,
                unparsed,
            });
        }
        report!(
            "[warn] RTK extension ownership state at {} records no agent; treating ownership as unknown",
            state_path.display()
        );
        return Ok(ManagedAgentState::Unknown);
    }

    if !unparsed.is_empty() {
        report!(
            "[warn] RTK extension ownership state at {} contains unrecognised entries; treating the recorded agents as a partial record",
            state_path.display()
        );
    }

    Ok(ManagedAgentState::Known {
        agents,
        prior_unknown,
        unparsed,
    })
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
    // Re-read rather than merging into the caller's snapshot: an `Ask` prompt
    // sits between that read and this write, so another process may have
    // recorded itself in the meantime and must not be dropped. Quietly, since
    // the caller's read already reported anything wrong with the file.
    let state = &read_managed_agents_reporting(path, false)?;
    // Recorded agents are those known to use this path. Deleting the extension
    // does not unconfigure an agent that points here, so an existing record is
    // always merged into rather than replaced.
    let (mut agents, mut prior_unknown, unparsed) = match state {
        // No record yet. If the extension already existed, somebody installed
        // it and RTK cannot know who, so the new record is explicitly partial.
        ManagedAgentState::Absent => (Vec::new(), extension_was_present, Vec::new()),
        ManagedAgentState::Known {
            agents,
            prior_unknown,
            unparsed,
        } => (agents.clone(), *prior_unknown, unparsed.clone()),
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
    // A record listing both agents is exhaustive on its own, unless it also
    // carries entries this version cannot account for.
    if agents.len() == 2 && unparsed.is_empty() {
        prior_unknown = false;
    }
    let content = serialize_managed_agents(&agents, prior_unknown, &unparsed);
    write_if_changed_allow_read_error(&state_path, &content, "RTK extension ownership state", ctx)?;
    Ok(())
}

/// Give up this agent's claim after an uninstall. When only a symlink to the
/// extension was removed the file itself survives and the other agent still
/// owns it, so the record is pruned rather than deleted — deleting it would
/// silently drop the surviving owner's shared-file protection.
fn release_managed_agent(
    state_path: &Path,
    canonical_extension: &Path,
    agent: PiCompatibleAgent,
    extension_survives_removal: bool,
    ctx: InitContext,
) -> Result<()> {
    if !state_path.exists() {
        return Ok(());
    }

    // Decided from what the removal will do, not from what is on disk right
    // now: under `--dry-run` nothing has been removed yet, so probing the file
    // would preview the pruning branch and then really take the deleting one.
    if !extension_survives_removal {
        return remove_managed_agent_state(state_path, ctx);
    }

    let ManagedAgentState::Known {
        mut agents,
        prior_unknown,
        unparsed,
    } = read_managed_agents_reporting(canonical_extension, false)?
    else {
        return Ok(());
    };

    agents.retain(|recorded| *recorded != agent);
    if agents.is_empty() && !prior_unknown && unparsed.is_empty() {
        return remove_managed_agent_state(state_path, ctx);
    }

    let content = serialize_managed_agents(&agents, prior_unknown, &unparsed);
    write_if_changed_allow_read_error(state_path, &content, "RTK extension ownership state", ctx)?;
    Ok(())
}

fn serialize_managed_agents(
    agents: &[PiCompatibleAgent],
    prior_unknown: bool,
    unparsed: &[String],
) -> String {
    let mut entries: Vec<&str> = agents.iter().map(|agent| agent.state_name()).collect();
    // Entries written by a version that knows more agents than this one are
    // carried through untouched rather than dropped.
    entries.extend(unparsed.iter().map(String::as_str));
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
/// Takes the ownership state rather than reading it, so one command reads the
/// sidecar once: reading it twice also warns twice about the same bad file.
fn extension_share_status(
    global: bool,
    path: &Path,
    agent: PiCompatibleAgent,
    state: &ManagedAgentState,
) -> Result<ExtensionShareStatus> {
    if !extension_paths_alias(global, path, agent)? {
        return Ok(ExtensionShareStatus::NotShared);
    }

    let other_agent = agent.other();
    match state {
        ManagedAgentState::Known {
            agents,
            prior_unknown,
            ..
        } => {
            if agents.contains(&other_agent) {
                Ok(ExtensionShareStatus::Shared)
            } else if *prior_unknown {
                // The record was started over an extension RTK did not
                // install, so the absence of the other agent proves nothing.
                Ok(ExtensionShareStatus::Unknown)
            } else {
                Ok(ExtensionShareStatus::NotShared)
            }
        }
        // No probe of `~/.pi`/`~/.omp` here: the alias arises when
        // `PI_CODING_AGENT_DIR` relocates both agents away from those defaults,
        // so their absence is not evidence the other agent is uninstalled, and
        // treating it as such would claim sole ownership over a shared file.
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
    extension_was_present: bool,
    state: &ManagedAgentState,
) -> Result<()> {
    let scope = extension_scope_name(global);
    match extension_share_status(global, path, agent, state)? {
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
    let state = read_ownership_if_aliased(global, path, agent)?;
    match extension_share_status(global, path, agent, &state)? {
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

/// Where a copy of `path` would go, or `None` when no copy will be made. Shared by the copy
/// itself, the install prompt and the dry-run previews, so none of them can name a
/// destination the copy will not use.
///
/// The two reasons for `None` differ, and only one is worth saying out loud: content already
/// preserved is the quiet case, while a source RTK cannot read means the copy silently will
/// not happen. A caller whose wording turns on which it is matches [`free_backup_slot`]
/// directly, as the uninstall prompt does.
fn plan_copy_backup(path: &Path, name: &str) -> Result<Option<PathBuf>> {
    match free_backup_slot(path)? {
        BackupSlot::Free(backup) => Ok(Some(backup)),
        BackupSlot::AlreadyPreserved(_) => Ok(None),
        // Blocking here would make an unreadable extension unrecoverable, which is the
        // situation `--auto-patch` is documented to resolve, so report and carry on.
        BackupSlot::SourceUnreadable(_) => {
            eprintln!(
                "[warn] {} at {} could not be read; proceeding without a backup",
                name,
                path.display()
            );
            Ok(None)
        }
    }
}

/// Copy a non-stock extension aside before it is replaced or removed, never overwriting an
/// earlier backup. Stock content is reproducible from the embedded copy, but a user's edits
/// are not.
fn back_up_modified_extension(path: &Path, name: &str) -> Result<()> {
    let Some(backup_path) = plan_copy_backup(path, name)? else {
        return Ok(());
    };

    fs::copy(path, &backup_path)
        .with_context(|| format!("Failed to back up {} to {}", name, backup_path.display()))?;
    println!("Backed up {} to {}", name, backup_path.display());
    Ok(())
}

/// Decide whether RTK content that no longer matches a known stock revision may
/// be removed. Mirrors the install side's Ask/Auto/Skip policy so `--auto-patch`
/// approves a removal the same way it approves an overwrite, instead of leaving
/// a hand-edited or fork-built extension permanently un-uninstallable.
fn confirm_modified_extension_removal(
    path: &Path,
    name: &str,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    if ctx.dry_run {
        match patch_mode {
            PatchMode::Ask => println!(
                "[dry-run] would prompt before removing modified {}: {}",
                name,
                path.display()
            ),
            PatchMode::Auto => println!(
                "[dry-run] would remove modified {}: {}",
                name,
                path.display()
            ),
            PatchMode::Skip => println!(
                "[dry-run] would refuse to remove {}: {}",
                name,
                path.display()
            ),
        }
        return Ok(matches!(patch_mode, PatchMode::Auto));
    }

    match patch_mode {
        PatchMode::Auto => Ok(true),
        PatchMode::Skip => Ok(false),
        // Names the backup so this reads distinctly from the shared-path
        // question that may follow it for the very same file.
        PatchMode::Ask => {
            let destination = match free_backup_slot(path)? {
                BackupSlot::Free(backup) => format!("back them up to {} and", backup.display()),
                BackupSlot::AlreadyPreserved(_) => "they are already backed up;".to_owned(),
                // Claiming they are backed up here would be a guess: the copy will not happen,
                // and an unreadable source is one RTK cannot compare against what it already
                // holds.
                BackupSlot::SourceUnreadable(_) => {
                    "they cannot be read and will not be backed up;".to_owned()
                }
            };
            prompt_user_confirmation(&format!(
                "{} at {} has local changes; {} remove it?",
                name,
                path.display(),
                destination
            ))
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
    let canonical_extension = canonicalize_path_for_comparison(&plugin_path);
    // Removing a symlink leaves its target — and the target's other owner —
    // in place; removing the file itself does not.
    let extension_survives_removal =
        fs::symlink_metadata(&plugin_path).is_ok_and(|metadata| metadata.file_type().is_symlink());
    let Some(content) = read_extension_for_uninstall(&plugin_path, "Pi extension", ctx)? else {
        return Ok(());
    };

    let mut removal_needs_backup = false;
    if !is_known_stock_pi_plugin(&content) {
        if !looks_like_rtk_pi_plugin(&content) {
            println!(
                "Pi extension at {} is not RTK content; leaving it alone.",
                plugin_path.display()
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

        if !confirm_modified_extension_removal(&plugin_path, "Pi extension", patch_mode, ctx)? {
            if dry_run {
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "Pi extension at {} contains RTK content that does not match the stock extension. Remove the file manually, or rerun with --auto-patch to remove it.",
                plugin_path.display()
            );
        }

        removal_needs_backup = true;
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
        if removal_needs_backup
            && let Some(backup) = plan_copy_backup(&plugin_path, "Pi extension")?
        {
            println!(
                "[dry-run] would back up Pi extension to {}",
                backup.display()
            );
        }
        println!(
            "[dry-run] would remove Pi extension: {}",
            plugin_path.display()
        );
        release_managed_agent(
            &ownership_state_path,
            &canonical_extension,
            PiCompatibleAgent::Pi,
            extension_survives_removal,
            ctx,
        )?;
        print_dry_run_footer();
    } else {
        if removal_needs_backup {
            back_up_modified_extension(&plugin_path, "Pi extension")?;
        }
        // nosemgrep: filesystem-deletion -- Pi uninstall removes only a known RTK stock extension.
        fs::remove_file(&plugin_path)
            .with_context(|| format!("Failed to remove Pi extension: {}", plugin_path.display()))?;
        release_managed_agent(
            &ownership_state_path,
            &canonical_extension,
            PiCompatibleAgent::Pi,
            extension_survives_removal,
            ctx,
        )?;
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

    let ownership_state = read_ownership_if_aliased(global, &plugin_path, PiCompatibleAgent::Pi)?;

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

    // Only once the write is going ahead: announcing a shared install before the
    // overwrite is approved would assert something that may never happen.
    warn_if_extension_shared_on_install(
        global,
        &plugin_path,
        PiCompatibleAgent::Pi,
        extension_was_present,
        &ownership_state,
    )?;

    // Through a symlink alias the write lands in the link's target directory,
    // not the link's own parent.
    let pi_write_target =
        resolve_symlink_target(&plugin_path).unwrap_or_else(|| plugin_path.clone());
    if let Some(parent) = pi_write_target.parent() {
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

    let ownership_state = read_ownership_if_aliased(global, &path, PiCompatibleAgent::Omp)?;

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

    // Only once the write is going ahead: announcing a shared install before the
    // overwrite is approved would assert something that may never happen.
    warn_if_extension_shared_on_install(
        global,
        &path,
        PiCompatibleAgent::Omp,
        extension_was_present,
        &ownership_state,
    )?;

    // Through a symlink alias the write lands in the link's target directory,
    // not the link's own parent.
    let omp_write_target = resolve_symlink_target(&path).unwrap_or_else(|| path.clone());
    if let Some(parent) = omp_write_target.parent() {
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
    let canonical_extension = canonicalize_path_for_comparison(&path);
    // Removing a symlink leaves its target — and the target's other owner —
    // in place; removing the file itself does not.
    let extension_survives_removal =
        fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink());
    let Some(content) = read_extension_for_uninstall(&path, "OMP extension", ctx)? else {
        return Ok(());
    };

    let mut removal_needs_backup = false;
    if !is_known_stock_pi_plugin(&content) {
        if !looks_like_rtk_pi_plugin(&content) {
            println!(
                "OMP extension at {} is not RTK content; leaving it alone.",
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

        if !confirm_modified_extension_removal(&path, "OMP extension", patch_mode, ctx)? {
            if dry_run {
                print_dry_run_footer();
                return Ok(());
            }
            anyhow::bail!(
                "OMP extension at {} contains RTK content that does not match the stock extension. Remove the file manually, or rerun with --auto-patch to remove it.",
                path.display()
            );
        }

        removal_needs_backup = true;
    }

    if !confirm_shared_extension_uninstall(global, &path, PiCompatibleAgent::Omp, patch_mode, ctx)?
    {
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
        if removal_needs_backup && let Some(backup) = plan_copy_backup(&path, "OMP extension")? {
            println!(
                "[dry-run] would back up OMP extension to {}",
                backup.display()
            );
        }
        println!("[dry-run] would remove OMP extension: {}", path.display());
        release_managed_agent(
            &ownership_state_path,
            &canonical_extension,
            PiCompatibleAgent::Omp,
            extension_survives_removal,
            ctx,
        )?;
        print_dry_run_footer();
    } else {
        if removal_needs_backup {
            back_up_modified_extension(&path, "OMP extension")?;
        }
        // nosemgrep: filesystem-deletion -- OMP uninstall removes only the RTK-managed extension file.
        fs::remove_file(&path)
            .with_context(|| format!("Failed to remove OMP extension: {}", path.display()))?;
        release_managed_agent(
            &ownership_state_path,
            &canonical_extension,
            PiCompatibleAgent::Omp,
            extension_survives_removal,
            ctx,
        )?;
        if verbose > 0 {
            eprintln!("Removed OMP extension: {}", path.display());
        }
        println!("RTK uninstalled (OMP):");
        println!("  - Extension: {}", path.display());
        println!("\nRestart OMP to apply changes.");
    }

    Ok(())
}

/// Show the extension status for a Pi-compatible agent. Pi and OMP install the
/// same file, so they share every state the report distinguishes.
pub(super) fn show_pi_compatible_config(agent: PiCompatibleAgent) -> Result<()> {
    let (global_extension, project_extension, flag, title) = match agent {
        PiCompatibleAgent::Pi => (
            pi_plugin_path_for_scope(true)?,
            pi_plugin_path_for_scope(false)?,
            "pi",
            "Pi",
        ),
        PiCompatibleAgent::Omp => (
            omp_extension_path_for_scope(true)?,
            omp_extension_path_for_scope(false)?,
            "omp",
            "Oh My Pi",
        ),
    };

    println!("rtk Configuration ({title}):\n");
    print_omp_extension_status("Global extension", &global_extension)?;
    print_omp_extension_status("Project extension", &project_extension)?;

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

fn print_omp_extension_status(label: &str, path: &Path) -> Result<()> {
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
        uninstall_omp_with_patch_mode(global, PatchMode::Ask, ctx)
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
            record_managed_agent(
                true,
                &omp_path,
                PiCompatibleAgent::Pi,
                false,
                InitContext::default(),
            )
            .unwrap();
            assert_eq!(
                extension_share_status(
                    true,
                    &omp_path,
                    PiCompatibleAgent::Omp,
                    &read_managed_agents(&omp_path).unwrap()
                )
                .unwrap(),
                ExtensionShareStatus::Shared
            );

            assert_eq!(
                extension_share_status(
                    true,
                    &pi_path,
                    PiCompatibleAgent::Pi,
                    &read_managed_agents(&pi_path).unwrap()
                )
                .unwrap(),
                ExtensionShareStatus::NotShared,
                "a sidecar naming only Pi, with no unknown prior owner, must report the aliased file as not shared"
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
                extension_share_status(
                    true,
                    &pi_path,
                    PiCompatibleAgent::Pi,
                    &read_managed_agents(&pi_path).unwrap()
                )
                .unwrap(),
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
