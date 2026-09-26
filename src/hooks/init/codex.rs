//! Codex agent: hook install/uninstall helpers.

use super::*;
use crate::hooks::constants::{CODEX_DIR, CODEX_HOOK_COMMAND, HOOKS_JSON, PRE_TOOL_USE_KEY};
use std::path::Component;

/// The line that says an `RTK.md` is RTK's to rewrite and to remove.
///
/// `--codex` is the one mode whose `RTK.md` sits at the project root, next to the user's own
/// files and under a name RTK does not own, so it has to be able to tell the two apart. It
/// says so in the file rather than by comparing content: the payload changes between releases,
/// and a check against the running build's copy stops recognising RTK's own file the moment it
/// does -- leaving it orphaned on uninstall and backed up on every upgrade.
const RTK_MD_OWNED_MARKER: &str = "<!-- rtk-owned:";

/// The header [`RTK_MD_OWNED_MARKER`] appears in, written above the awareness payload.
const RTK_MD_OWNED_HEADER: &str =
    "<!-- rtk-owned: written by `rtk init --codex`, removed by `rtk init --codex --uninstall` -->";

/// The `RTK.md` the Codex mode writes: RTK's ownership line, then the awareness payload.
pub(super) fn codex_rtk_md_content(level: AwarenessLevel) -> String {
    format!("{RTK_MD_OWNED_HEADER}\n\n{}", awareness_content(level))
}

/// The heading every `--codex` release up to v0.48.0 wrote at the top of the `RTK.md` it
/// owned, before the ownership line existed. Nothing writes it any more, so it is frozen, and
/// it names both RTK and the mode, so a file starting with it is RTK's. The headings that
/// replaced it (`# RTK`, `# Command output`) are left out on purpose: a user's own notes may
/// legitimately open with either, and taking their file would be the worse mistake.
const RTK_MD_LEGACY_CODEX_HEADING: &str = "# RTK - Rust Token Killer (Codex CLI)";

/// SHA-256 of the three awareness payloads RTK wrote as `RTK.md` between the legacy heading
/// and the ownership line -- v0.49.0 and the builds after it, which wrote the shared text with
/// nothing on top claiming it. Their headings (`# RTK`, `# Command output`) name neither RTK
/// nor the mode and a user's notes may open with either, so only the whole file identifies
/// them. Digests rather than the payload constants: these name bytes that shipped, and
/// rewording the awareness text today must not change which files uninstall recognises.
const RTK_MD_UNMARKED_PAYLOAD_DIGESTS: [&str; 3] = [
    "dc37dc6afdf513200c2aae1931496e433d323f313877a49b0d5ba11992c33ac7",
    "d124b2926b0cd506680f785ab98ef64c211854549da4a85ec544fb803aaea812",
    "278274ef3d08c858d4247cc91419c4d74ef922b95719e987b22e896aef10e1fc",
];

/// The digest [`RTK_MD_UNMARKED_PAYLOAD_DIGESTS`] is compared against. Carriage returns are
/// dropped first: a checkout or an editor may have rewritten the line endings of a file RTK
/// wrote, which does not make it the user's.
fn rtk_md_payload_digest(content: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(content.replace('\r', "").as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Whether `content` is an RTK.md that RTK itself wrote.
///
/// The ownership line and the legacy heading are read on the first non-blank line only: RTK
/// writes its claim at the top of a file it wrote whole, and a user who pasted either -- or
/// an RTK-written block -- somewhere inside their own notes has not handed the rest of the
/// file over with it. A payload from the releases that claimed nothing is matched whole.
fn is_rtk_authored_md(content: &str) -> bool {
    // A BOM is what an editor adds, not a change of authorship, and Windows editors add one.
    let content = strip_leading_bom(content);
    let claimed = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .is_some_and(|line| {
            line.starts_with(RTK_MD_OWNED_MARKER) || line == RTK_MD_LEGACY_CODEX_HEADING
        });

    claimed || RTK_MD_UNMARKED_PAYLOAD_DIGESTS.contains(&rtk_md_payload_digest(content).as_str())
}

/// [`is_rtk_authored_md`] for a path. An unreadable file is not provably RTK's, so it is
/// treated as the user's and left alone.
fn rtk_md_is_rtk_authored(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|content| is_rtk_authored_md(&content))
}

/// Which directory a Codex `RTK.md` sits in, which is what decides who owns it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RtkMdScope {
    /// `--global`: the Codex home. RTK created the file there, nothing else claims the name,
    /// and every release before the ownership marker wrote it there unmarked -- reading the
    /// marker in this scope would strand all of those files on uninstall.
    CodexHome,
    /// Project scope: the project root, shared with the user's own files. Only the marker
    /// tells RTK's `RTK.md` from one the user wrote under the same name.
    ProjectRoot,
}

impl RtkMdScope {
    fn for_global(global: bool) -> Self {
        if global {
            Self::CodexHome
        } else {
            Self::ProjectRoot
        }
    }

    /// Whether RTK may overwrite or remove the `RTK.md` at `path` without asking.
    fn owns(self, path: &Path) -> bool {
        match self {
            Self::CodexHome => true,
            Self::ProjectRoot => rtk_md_is_rtk_authored(path),
        }
    }
}

/// How many numbered backups may sit beside a file before RTK refuses to make another.
///
/// High enough that no ordinary sequence of installs reaches it, and bounded so a file that
/// keeps producing distinct backups cannot quietly fill its directory: at the ceiling RTK
/// stops and says so rather than choosing one to overwrite.
const MAX_BACKUP_ATTEMPTS: usize = 99;

/// A slot to preserve a file's current content in, and what is known about that content.
///
/// Reading the source is only ever needed to answer "is this already preserved?", which is
/// why the unreadable case is its own arm rather than a refusal: a caller that copies cannot
/// proceed without that answer, while a caller that renames never needed it, since a rename
/// carries the content whether or not RTK can read it.
#[derive(Debug)]
enum BackupSlot {
    /// Free, and the source differs from every backup already present.
    Free(PathBuf),
    /// Identical content already sits here, so a copy would be redundant. A rename still has
    /// the original to move, and moving it here is what discards the duplicate.
    AlreadyPreserved(PathBuf),
    /// Free, but the source could not be read, so whether it is already preserved is unknown.
    SourceUnreadable(PathBuf),
}

impl BackupSlot {
    /// The slot to move the original onto, for a caller that preserves content by renaming.
    ///
    /// Every arm gives the same answer: a rename carries content RTK could not read, and
    /// renaming onto a byte-identical backup discards the duplicate instead of burning a slot.
    fn for_rename(self) -> PathBuf {
        match self {
            BackupSlot::Free(path)
            | BackupSlot::AlreadyPreserved(path)
            | BackupSlot::SourceUnreadable(path) => path,
        }
    }
}

/// `path.bak` for the first slot, `path.bak.N` after that.
///
/// Built by appending to the `OsString` rather than through `with_extension`, which replaces
/// an extension instead of extending it, and rather than through `display()`, which is lossy:
/// on a path that is not valid UTF-8 the probe and the write would disagree.
fn numbered_backup_path(path: &Path, attempt: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    if attempt > 0 {
        name.push(format!(".{attempt}"));
    }
    PathBuf::from(name)
}

/// Pick a `.bak` sibling for `path`, numbered when earlier backups are still there so a second
/// run cannot overwrite the first one's rescue copy.
fn free_backup_slot(path: &Path) -> Result<BackupSlot> {
    let source = fs::read(path).ok();
    for attempt in 0..=MAX_BACKUP_ATTEMPTS {
        let candidate = numbered_backup_path(path, attempt);
        match fs::read(&candidate) {
            // A provisioning loop that reapplies the same local change would otherwise
            // consume a slot on every run.
            Ok(existing) if source.as_deref() == Some(existing.as_slice()) => {
                return Ok(BackupSlot::AlreadyPreserved(candidate));
            }
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(match source {
                    Some(_) => BackupSlot::Free(candidate),
                    None => BackupSlot::SourceUnreadable(candidate),
                });
            }
            // Occupied by something unreadable -- a directory, or a file this user cannot
            // read. Its content cannot be compared, so treat the slot as taken rather than
            // overwriting it.
            Err(_) => continue,
        }
    }

    anyhow::bail!(
        "Cannot back up {}: {} existing backups are already present. \
         Remove or archive them first.",
        path.display(),
        MAX_BACKUP_ATTEMPTS + 1
    )
}

/// How many hops one path's symlink chain is followed before the walk gives up.
///
/// The bound is per component, not per walk: [`resolve_symlink_components_within`] spends a
/// fresh budget on the descent into each target it jumps to. It sits below the kernel's own
/// limit on purpose -- RTK has to be able to say where a write lands, and a chain this deep
/// under a path RTK joins itself is not one a user maintains.
const MAX_SYMLINK_HOPS: usize = 16;

/// What one `readlink` established.
///
/// [`Unreadable`](Self::Unreadable) is not [`NotALink`](Self::NotALink): `lstat` already said
/// this is a symlink, so reporting the read failure as "no link here" would hand a caller a
/// path nothing resolved and let it compare that as if it had.
enum SymlinkHop {
    NotALink,
    To(PathBuf),
    Unreadable,
}

/// What following one path's symlink chain established.
enum SymlinkChain {
    /// The path is not a symlink.
    Settled,
    /// Followed to a target that is not itself a symlink.
    Target(PathBuf),
    /// Still a symlink after [`MAX_SYMLINK_HOPS`]: a cycle, or a chain too deep to vouch for.
    Exhausted,
    /// A link on the chain could not be read, so where it leads is unknown.
    Unreadable,
}

/// How far a component walk got, and whether it can be trusted as an answer.
enum Resolution {
    /// Every symlink along the path was followed to a target that is not a link.
    Fully(PathBuf),
    /// The walk gave up: a cycle, a chain too deep, or a link it could not read. The path is
    /// as far as it got, which a caller that only acts may still use -- the kernel finishes
    /// the resolution -- but which says nothing about where the write lands.
    Unresolved(PathBuf),
}

/// Read one symlink hop, resolving a relative link against the link's own directory.
fn symlink_hop(path: &Path) -> SymlinkHop {
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return SymlinkHop::NotALink;
    }
    let Ok(target) = fs::read_link(path) else {
        return SymlinkHop::Unreadable;
    };
    if target.is_absolute() {
        return SymlinkHop::To(target);
    }
    match path.parent() {
        Some(parent) => SymlinkHop::To(parent.join(target)),
        None => SymlinkHop::Unreadable,
    }
}

/// Follow a chain of symlinks whose final target does not exist yet, stopping at the first
/// entry that is not a symlink. `canonicalize` reports `ELOOP` for a cycle, so the hop limit
/// is the only terminator available here.
fn follow_symlink_chain(path: &Path) -> SymlinkChain {
    let mut current = match symlink_hop(path) {
        SymlinkHop::NotALink => return SymlinkChain::Settled,
        SymlinkHop::Unreadable => return SymlinkChain::Unreadable,
        SymlinkHop::To(target) => target,
    };
    // Inclusive: the bound counts hops followed, and an exclusive range would stop one short
    // of the chain length the documentation promises.
    for _ in 1..=MAX_SYMLINK_HOPS {
        match symlink_hop(&current) {
            SymlinkHop::NotALink => return SymlinkChain::Target(current),
            SymlinkHop::Unreadable => return SymlinkChain::Unreadable,
            SymlinkHop::To(next) => current = next,
        }
    }
    SymlinkChain::Exhausted
}

/// Jumping to a link's target abandons the components walked so far, and that target may
/// itself sit behind symlinked ancestors this walk never visited, so it is resolved from the
/// top. `budget` bounds that descent: the paths involved can form a cycle that
/// [`follow_symlink_chain`]'s own hop limit does not see, because each jump hands it a
/// different path.
fn resolve_symlink_components_within(path: &Path, budget: usize) -> Resolution {
    let mut resolved = PathBuf::new();
    let mut settled = true;
    for component in path.components() {
        resolved.push(component);
        match follow_symlink_chain(&resolved) {
            SymlinkChain::Settled => {}
            SymlinkChain::Exhausted | SymlinkChain::Unreadable => settled = false,
            SymlinkChain::Target(target) => match budget.checked_sub(1) {
                Some(remaining) => match resolve_symlink_components_within(&target, remaining) {
                    Resolution::Fully(path) => resolved = path,
                    Resolution::Unresolved(path) => {
                        resolved = path;
                        settled = false;
                    }
                },
                None => {
                    resolved = target;
                    settled = false;
                }
            },
        }
    }
    if settled {
        Resolution::Fully(resolved)
    } else {
        Resolution::Unresolved(resolved)
    }
}

/// Refuse a project-scoped write whose path leaves the project.
///
/// `.codex/hooks.json` and its backup sibling are relative names RTK joins itself, so a
/// symlinked component is the only way they can resolve elsewhere -- and then `rtk init
/// --codex` would register a hook, which runs shell commands, in a directory the user never
/// named. The global mode exists for writing outside the project.
///
/// This answers for the tree as it stands when asked. A process rewriting these paths while
/// init runs can still move the write afterwards; the case it is built for is a repository
/// that ships the links, which is settled before init starts.
fn ensure_inside_project(path: &Path) -> Result<()> {
    let root = std::env::current_dir().context("Failed to resolve the current directory")?;
    ensure_inside_root(&root, path)
}

/// [`ensure_inside_project`] against an explicit root.
fn ensure_inside_root(root: &Path, path: &Path) -> Result<()> {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // Anchored to the root before resolving: these paths are relative, and
    // `canonicalize_path_for_comparison` hands a relative path straight back when none of its
    // components exist yet, which no absolute root can ever contain.
    let site = root.join(path);

    ensure_chain_sits_inside(&root, path, &site)?;

    match resolve_symlink_components_within(&site, MAX_SYMLINK_HOPS) {
        // A walk that gave up cannot say where the write lands, and a path RTK cannot place
        // is one it must not write to.
        Resolution::Unresolved(_) => anyhow::bail!(
            "{} passes through a symlink RTK cannot follow to an end, \
             so RTK cannot say where a write to it would land.\n\
             Remove the symlink, or use --global to configure Codex outside the project.",
            path.display()
        ),
        Resolution::Fully(resolved) => {
            let resolved = canonicalize_path_for_comparison(&lexically_normalized(&resolved));
            if !resolved.starts_with(&root) {
                anyhow::bail!(
                    "{} resolves to {}, outside the project at {}.\n\
                     Remove the symlink, or use --global to configure Codex outside the project.",
                    path.display(),
                    resolved.display(),
                    root.display()
                );
            }
            Ok(())
        }
    }
}

/// Every link the write site itself passes through has to *sit* inside the project, not only
/// end up pointing back into it.
///
/// `atomic_write` cannot canonicalize a chain whose end does not exist, and then writes at the
/// path as the filesystem reads it, replacing the last link rather than following it. A link
/// anywhere along that chain is therefore a place the write can land, and one the project does
/// not contain is one an attacker may own: pointing it back inside passes a check that only
/// looked at the far end, while leaving the middle free to be re-aimed afterwards.
///
/// Only the chain of the site itself is judged this way. Links on *ancestor* components are
/// traversed, never sited -- whole directory trees hang off one on macOS, so refusing those
/// would refuse every absolute target under `/var` or `/tmp`.
fn ensure_chain_sits_inside(root: &Path, named: &Path, site: &Path) -> Result<()> {
    let mut hop = site.to_path_buf();
    for _ in 0..=MAX_SYMLINK_HOPS {
        match symlink_hop(&hop) {
            SymlinkHop::NotALink => return Ok(()),
            SymlinkHop::Unreadable => anyhow::bail!(
                "{} passes through a symlink at {} that RTK cannot read, \
                 so RTK cannot say where a write to it would land.\n\
                 Remove the symlink, or use --global to configure Codex outside the project.",
                named.display(),
                hop.display()
            ),
            SymlinkHop::To(next) => {
                let at = link_site(&hop);
                if !at.starts_with(root) {
                    anyhow::bail!(
                        "{} is a symlink at {}, outside the project at {}.\n\
                         Remove the symlink, or use --global to configure Codex outside the project.",
                        named.display(),
                        at.display(),
                        root.display()
                    );
                }
                hop = next;
            }
        }
    }
    anyhow::bail!(
        "{} passes through more symlinks than RTK follows, \
         so RTK cannot say where a write to it would land.\n\
         Remove the symlink, or use --global to configure Codex outside the project.",
        named.display()
    )
}

/// Where a symlink sits, as a path free of `.` and `..`.
///
/// Resolving the parent and re-attaching the name gives the link's own location rather than
/// its target's, whether or not the parent itself holds links.
fn link_site(link: &Path) -> PathBuf {
    match (link.parent(), link.file_name()) {
        (Some(parent), Some(name)) => canonicalize_path_for_comparison(parent).join(name),
        _ => lexically_normalized(link),
    }
}

/// `path` with `.` dropped and `..` folded into the component before it.
///
/// Only sound once the path holds no symlink, which is where the containment walk uses it:
/// `link/..` is the parent of the link's target, not of the link. Comparing without it would
/// mean comparing a path nothing resolved, because `canonicalize`'s fallback gives up and
/// returns its argument untouched as soon as a missing component is followed by `..`.
fn lexically_normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            // Only a named component can be folded away: `..` after a root, or after another
            // `..` on a relative path, names somewhere else and has to survive.
            Component::ParentDir
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) =>
            {
                normalized.pop();
            }
            kept => normalized.push(kept),
        }
    }
    normalized
}

pub(super) fn uninstall_codex(global: bool, ctx: InitContext) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    let mut hook_left_in_place = None;
    let removed = if global {
        let codex_dir = resolve_codex_dir()?;
        uninstall_codex_at(&codex_dir, ctx)?
    } else {
        // Only the hook path is guarded, and only it is given up when the guard trips:
        // `AGENTS.md` and `RTK.md` are project-root names RTK joins itself, they are
        // demonstrably where uninstall left them, and refusing to clean them because some
        // other path is a symlink leaves the user with artifacts and no command to remove
        // them -- `--global` acts on `~/.codex`, which is not where these are.
        let hooks_json_path = Path::new(CODEX_DIR).join(HOOKS_JSON);
        let hooks_json_path = match ensure_inside_project(&hooks_json_path)
            .and_then(|()| ensure_inside_project(&backup_path_for(&hooks_json_path)))
        {
            Ok(()) => Some(hooks_json_path.as_path()),
            Err(error) => {
                hook_left_in_place = Some(error);
                None
            }
        };
        uninstall_codex_with_paths(
            Path::new(AGENTS_MD),
            Path::new(RTK_MD),
            RtkMdScope::ProjectRoot,
            hooks_json_path,
            &[RTK_MD_REF],
            ctx,
        )?
    };

    println!(
        "{}",
        codex_uninstall_report(&removed, hook_left_in_place.as_ref(), dry_run)
    );

    Ok(())
}

/// Everything `rtk init --codex --uninstall` says it did, as one block.
///
/// Built rather than printed line by line so a hook the guard refused to touch cannot be
/// dropped from the summary without the tests noticing: read on its own, a list of what was
/// removed reads as a complete uninstall.
fn codex_uninstall_report(
    removed: &[String],
    unchecked_hook: Option<&anyhow::Error>,
    dry_run: bool,
) -> String {
    let mut report = if removed.is_empty() {
        "RTK was not installed for Codex CLI (nothing to remove)".to_string()
    } else {
        let header = if dry_run {
            "[dry-run] would uninstall RTK for Codex CLI:"
        } else {
            "RTK uninstalled for Codex CLI:"
        };
        std::iter::once(header.to_string())
            .chain(removed.iter().map(|item| format!("  - {item}")))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Not a claim that the hook is registered: the guard refuses before anything reads the
    // file, so RTK does not know -- only that it did not look.
    if let Some(error) = unchecked_hook {
        report.push_str(&format!(
            "\n  Not checked: the Codex hook in {}, which may still be registered:",
            Path::new(CODEX_DIR).join(HOOKS_JSON).display()
        ));
        for line in error.to_string().lines() {
            report.push_str(&format!("\n    {line}"));
        }
    }
    report
}

fn uninstall_codex_at(codex_dir: &Path, ctx: InitContext) -> Result<Vec<String>> {
    let absolute_rtk_md_ref = codex_rtk_md_ref(codex_dir);
    uninstall_codex_with_paths(
        &codex_dir.join(AGENTS_MD),
        &codex_dir.join(RTK_MD),
        RtkMdScope::CodexHome,
        Some(&codex_dir.join(HOOKS_JSON)),
        &[RTK_MD_REF, absolute_rtk_md_ref.as_str()],
        ctx,
    )
}

pub(super) fn run_codex_mode(global: bool, ctx: InitContext) -> Result<()> {
    let (agents_md_path, rtk_md_path, hooks_json_path) = if global {
        let codex_dir = resolve_codex_dir()?;
        (
            codex_dir.join(AGENTS_MD),
            codex_dir.join(RTK_MD),
            codex_dir.join(HOOKS_JSON),
        )
    } else {
        let paths = (
            PathBuf::from(AGENTS_MD),
            PathBuf::from(RTK_MD),
            PathBuf::from(CODEX_DIR).join(HOOKS_JSON),
        );
        // Only the hook path. A symlinked `AGENTS.md` or `RTK.md` may be the user's own
        // arrangement, which `atomic_write` preserves deliberately, or may have come with a
        // clone -- git stores symlinks -- in which case init appends its `@RTK.md` line to
        // whatever the link names, outside the project. That is accepted: the line is inert
        // text, where `.codex/hooks.json` is a hook that runs shell commands, and RTK creates
        // `.codex` itself rather than following something the user put there.
        //
        // Its backup sibling is vouched for as well: `fs::copy` follows a symlink at the
        // destination, so a planted `hooks.json.bak` carried the existing hooks.json out of
        // the project even when `.codex` itself was a real directory.
        ensure_inside_project(&paths.2)?;
        ensure_inside_project(&backup_path_for(&paths.2))?;
        paths
    };

    run_codex_mode_with_paths(agents_md_path, rtk_md_path, hooks_json_path, global, ctx)
}

/// Move a user-authored `RTK.md` aside before init writes RTK's own over it, returning where
/// it went, or `None` when [`RtkMdScope`] says the file is RTK's to replace in place.
/// `--codex` is the one mode that writes `RTK.md` outside a directory RTK owns, so it is the
/// one mode whose existing file may be the user's.
fn back_up_foreign_rtk_md(
    path: &Path,
    scope: RtkMdScope,
    ctx: InitContext,
) -> Result<Option<PathBuf>> {
    if !path.exists() || scope.owns(path) {
        return Ok(None);
    }
    let backup = free_backup_slot(path)?.for_rename();
    if ctx.dry_run {
        println!(
            "[dry-run] would move your RTK.md aside: {} -> {}",
            path.display(),
            backup.display()
        );
        return Ok(None);
    }
    fs::rename(path, &backup).with_context(|| {
        format!(
            "Failed to move {} aside to {}",
            path.display(),
            backup.display()
        )
    })?;
    // Now, not in the closing summary: the steps after this one can fail, and the user's
    // text has already moved. Where it went is the one thing they must be told either way.
    println!("Your existing RTK.md was saved to {}", backup.display());
    Ok(Some(backup))
}

pub(super) fn run_codex_mode_with_paths(
    agents_md_path: PathBuf,
    rtk_md_path: PathBuf,
    hooks_json_path: PathBuf,
    global: bool,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if global
        && !dry_run
        && let Some(parent) = agents_md_path.parent()
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create Codex config directory: {}",
                parent.display()
            )
        })?;
    }

    // ISSUE #892: In global mode, use absolute path so @RTK.md resolves
    // from any CWD (worktrees, nested projects). Codex resolves @ references
    // relative to CWD, not the AGENTS.md file location.
    let rtk_md_ref = if global {
        codex_rtk_md_ref(
            rtk_md_path
                .parent()
                .context("RTK.md path missing parent directory")?,
        )
    } else {
        RTK_MD_REF.to_string()
    };

    back_up_foreign_rtk_md(&rtk_md_path, RtkMdScope::for_global(global), ctx)?;
    write_if_changed(
        &rtk_md_path,
        &codex_rtk_md_content(ctx.awareness),
        RTK_MD,
        ctx,
    )?;
    let added_ref = patch_agents_md(&agents_md_path, &rtk_md_ref, ctx)?;
    let hook_added = patch_codex_hooks_json(&hooks_json_path, ctx)?;

    if !dry_run {
        println!("\nRTK configured for Codex CLI.\n");
        println!("  RTK.md:    {}", rtk_md_path.display());
        println!(
            "  Hook:      {} ({})",
            hooks_json_path.display(),
            if hook_added {
                "registered"
            } else {
                "already present"
            }
        );
        if added_ref {
            println!("  AGENTS.md: {} reference added", rtk_md_ref);
        } else {
            println!("  AGENTS.md: {} reference already present", rtk_md_ref);
        }
        if global {
            println!(
                "\n  Codex global instructions path: {}",
                agents_md_path.display()
            );
        } else {
            println!(
                "\n  Codex project instructions path: {}",
                agents_md_path.display()
            );
        }
        println!(
            "\n  Restart Codex. For a project hook, approve it when Codex asks you to trust it."
        );
        match crate::core::tracking::get_db_path().and_then(|path| codex_tracking_config(&path)) {
            Ok(config) => {
                println!("\n  Optional: if history recording fails with SQLITE_CANTOPEN in the");
                println!(
                    "  workspace-write sandbox, grant write access to the tracking directory:"
                );
                println!("\n{config}");
                println!(
                    "  Merge this into Codex config.toml only if you want persistent history."
                );
                println!("  Keep existing writable_roots; do not add a duplicate table.");
                println!("  This grants access to the directory, including SQLite sidecar files.");
                println!("  No Codex permission settings have been changed.");
            }
            Err(error) => eprintln!("rtk: warning: could not prepare tracking guidance: {error:#}"),
        }
    }

    Ok(())
}

fn resolve_codex_dir() -> Result<PathBuf> {
    resolve_codex_dir_from(
        std::env::var_os("CODEX_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
}

fn resolve_codex_dir_from(
    codex_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    resolve_config_dir(
        codex_home.map(PathBuf::into_os_string),
        home_dir,
        CODEX_DIR,
        "Cannot determine Codex config directory. Set $CODEX_HOME or $HOME.",
    )
}

fn codex_rtk_md_ref(codex_dir: &Path) -> String {
    format!("@{}", codex_dir.join(RTK_MD).display())
}

pub(super) fn show_codex_config() -> Result<()> {
    let codex_dir = resolve_codex_dir()?;
    let global_agents_md = codex_dir.join(AGENTS_MD);
    let global_rtk_md = codex_dir.join(RTK_MD);
    let global_hooks_json = codex_dir.join(HOOKS_JSON);
    let global_rtk_md_ref = codex_rtk_md_ref(&codex_dir);
    let local_agents_md = PathBuf::from(AGENTS_MD);
    let local_rtk_md = PathBuf::from(RTK_MD);
    let local_hooks_json = PathBuf::from(CODEX_DIR).join(HOOKS_JSON);

    println!("rtk Configuration (Codex CLI):\n");

    if global_rtk_md.exists() {
        println!("[ok] Global RTK.md: {}", global_rtk_md.display());
    } else {
        println!("[--] Global RTK.md: not found");
    }

    print_codex_hook_status("Global", &global_hooks_json)?;

    if global_agents_md.exists() {
        let content = fs::read_to_string(&global_agents_md).with_context(|| {
            format!(
                "Failed to read global Codex instructions: {}",
                global_agents_md.display()
            )
        })?;
        if has_rtk_reference(&content, &[RTK_MD_REF, global_rtk_md_ref.as_str()]) {
            println!("[ok] Global AGENTS.md: RTK.md reference");
        } else if content.contains(RTK_BLOCK_START) {
            println!("[!!] Global AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Global AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Global AGENTS.md: not found");
    }

    if local_rtk_md.exists() {
        if rtk_md_is_rtk_authored(&local_rtk_md) {
            println!("[ok] Local RTK.md: {}", local_rtk_md.display());
        } else {
            // The project root is not a name RTK owns, and uninstall will say the same.
            println!(
                "[--] Local RTK.md: {} exists but RTK did not write it",
                local_rtk_md.display()
            );
        }
    } else {
        println!("[--] Local RTK.md: not found");
    }

    print_codex_hook_status("Local", &local_hooks_json)?;

    if local_agents_md.exists() {
        let content = fs::read_to_string(&local_agents_md).with_context(|| {
            format!(
                "Failed to read local Codex instructions: {}",
                local_agents_md.display()
            )
        })?;
        if has_rtk_reference(&content, &[RTK_MD_REF]) {
            println!("[ok] Local AGENTS.md: @RTK.md reference");
        } else if content.contains(RTK_BLOCK_START) {
            println!("[!!] Local AGENTS.md: old inline RTK block");
        } else {
            println!("[--] Local AGENTS.md: exists but rtk not configured");
        }
    } else {
        println!("[--] Local AGENTS.md: not found");
    }

    println!("\nUsage:");
    println!("  rtk init --codex              # Configure local AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init -g --codex           # Configure global AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init --codex --uninstall     # Remove local Codex RTK artifacts");
    println!("  rtk init -g --codex --uninstall  # Remove global Codex RTK artifacts");

    Ok(())
}

fn print_codex_hook_status(label: &str, path: &Path) -> Result<()> {
    match read_json_file(path) {
        Ok(Some(root)) if codex_hook_already_present(&root) => {
            println!("[ok] {label} hook: {}", path.display())
        }
        Ok(Some(_)) => println!("[--] {label} hooks.json exists but RTK hook is not configured"),
        Ok(None) => println!("[--] {label} hook: not found"),
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
            println!("[!!] {label} hooks.json is invalid JSON")
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn uninstall_codex_with_paths(
    agents_md_path: &Path,
    rtk_md_path: &Path,
    rtk_md_scope: RtkMdScope,
    hooks_json_path: Option<&Path>,
    rtk_md_refs: &[&str],
    ctx: InitContext,
) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();

    if let Some(hooks_json_path) = hooks_json_path
        && remove_codex_hook_from_file(hooks_json_path, ctx)?
    {
        removed.push(format!("hooks.json: removed {} entry", CODEX_HOOK_COMMAND));
    }

    if rtk_md_path.exists() {
        if !rtk_md_scope.owns(rtk_md_path) {
            // Uninstall removes RTK's artifacts, not a file that merely shares their name.
            println!(
                "{}Kept RTK.md: {} (RTK did not write it; remove it yourself if you want it gone)",
                if dry_run { "[dry-run] " } else { "" },
                rtk_md_path.display()
            );
        } else if dry_run {
            println!("[dry-run] would remove RTK.md: {}", rtk_md_path.display());
            removed.push(format!("RTK.md: {}", rtk_md_path.display()));
        } else {
            // nosemgrep: filesystem-deletion
            fs::remove_file(rtk_md_path)
                .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
            if verbose > 0 {
                eprintln!("Removed RTK.md: {}", rtk_md_path.display());
            }
            removed.push(format!("RTK.md: {}", rtk_md_path.display()));
        }
    }

    if agents_md_path.exists() {
        let content = fs::read_to_string(agents_md_path)
            .with_context(|| format!("Failed to read AGENTS.md: {}", agents_md_path.display()))?;

        let mut working_content = content.clone();
        let mut agents_changed = false;

        if working_content.contains(RTK_BLOCK_START) {
            let (cleaned, did_remove) = remove_rtk_block(&working_content);
            if did_remove {
                working_content = cleaned;
                agents_changed = true;
                removed.push("AGENTS.md: removed rtk-instructions block".to_string());
            }
        }

        if agents_changed {
            atomic_write(agents_md_path, &working_content).with_context(|| {
                format!("Failed to write AGENTS.md: {}", agents_md_path.display())
            })?;
        }
    }

    if remove_rtk_reference_from_agents(agents_md_path, rtk_md_refs, ctx)? {
        removed.push("AGENTS.md: removed @RTK.md reference".to_string());
    }

    Ok(removed)
}

fn codex_tracking_config(db_path: &Path) -> Result<String> {
    let absolute = std::path::absolute(db_path).context("Failed to resolve RTK database path")?;
    let parent = absolute
        .parent()
        .context("RTK database path has no parent")?;
    let directory = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    let directory = directory
        .to_str()
        .context("RTK database directory is not valid UTF-8")?;
    let quoted = toml::Value::String(directory.to_string());
    Ok(format!(
        "sandbox_mode = \"workspace-write\"\n\n[sandbox_workspace_write]\nwritable_roots = [{quoted}]\n"
    ))
}

pub(super) fn codex_hook_already_present(root: &serde_json::Value) -> bool {
    hook_present(
        root,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        |group| group_covers_tool(group, "Bash"),
        |hook| is_command_hook(hook, is_codex_hook_command),
    )
}

fn patch_codex_hooks_json(path: &Path, ctx: InitContext) -> Result<bool> {
    let InitContext { dry_run, .. } = ctx;
    let mut root = read_json_file(path)?.unwrap_or_else(|| serde_json::json!({}));

    if codex_hook_already_present(&root) {
        return Ok(false);
    }

    insert_hook_entry(&mut root, CODEX_HOOK_COMMAND)?;

    if !dry_run && let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create Codex config directory: {}",
                parent.display()
            )
        })?;
    }
    update_json_file(
        path,
        &root,
        ctx,
        "Codex hooks.json",
        &format!("[dry-run] would patch Codex hooks: {}", path.display()),
        true,
        Written::Line(format!("Patched Codex hooks: {}", path.display())),
    )?;

    Ok(true)
}

pub(super) fn remove_codex_hook_from_json(root: &mut serde_json::Value) -> bool {
    remove_hook_entries(root, PRE_TOOL_USE_KEY, HookEntries::Grouped, |hook| {
        is_command_hook(hook, is_codex_hook_command)
    })
}

fn remove_codex_hook_from_file(path: &Path, ctx: InitContext) -> Result<bool> {
    let Some(mut root) = read_json_file(path)? else {
        return Ok(false);
    };
    if !remove_codex_hook_from_json(&mut root) {
        return Ok(false);
    }

    update_json_file(
        path,
        &root,
        ctx,
        "Codex hooks.json",
        &format!(
            "[dry-run] would remove RTK hook entry from {}",
            path.display()
        ),
        true,
        Written::Line(format!("Removed Codex RTK hook: {}", path.display())),
    )?;

    Ok(true)
}

/// Matches this agent's RTK hook command: `rtk hook codex` from a bare, absolute or
/// Windows `rtk` path, and nothing else.
fn is_codex_hook_command(command: &str) -> bool {
    crate::hooks::is_rtk_hook_command(command, "codex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn codex_hook_command_matches_bare_absolute_and_windows_rtk() {
        assert!(is_codex_hook_command("rtk hook codex"));
        assert!(is_codex_hook_command("/opt/homebrew/bin/rtk hook codex"));
        assert!(is_codex_hook_command(
            "\"C:\\Program Files\\rtk.exe\" hook codex"
        ));
    }

    #[test]
    fn codex_hook_command_rejects_other_commands() {
        assert!(!is_codex_hook_command("rtk hook claude"));
        assert!(!is_codex_hook_command("echo rtk hook codex"));
        assert!(!is_codex_hook_command("\"rtk\"evil hook codex"));
    }

    #[test]
    fn test_codex_mode_rejects_auto_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --auto-patch"
        );
    }

    #[test]
    fn test_codex_mode_rejects_no_patch() {
        let err = run(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            PatchMode::Skip,
            InitContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--codex cannot be combined with --no-patch"
        );
    }

    #[test]
    fn test_run_codex_mode_global_writes_absolute_reference_to_codex_dir() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        let rtk_md = temp.path().join("RTK.md");
        let hooks_json = temp.path().join(HOOKS_JSON);

        run_codex_mode_with_paths(
            agents_md.clone(),
            rtk_md.clone(),
            hooks_json.clone(),
            true,
            InitContext::default(),
        )
        .unwrap();

        assert!(rtk_md.exists());
        assert_eq!(
            fs::read_to_string(&rtk_md).unwrap(),
            codex_rtk_md_content(AwarenessLevel::Default)
        );
        assert_eq!(
            fs::read_to_string(&agents_md).unwrap(),
            format!("{}\n", codex_rtk_md_ref(temp.path()))
        );
        let hooks: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hooks_json).unwrap()).unwrap();
        assert!(codex_hook_already_present(&hooks));
    }

    #[test]
    fn test_resolve_codex_dir_prefers_codex_home_and_ignores_empty_value() {
        let codex_home = PathBuf::from("/tmp/custom-codex-home");
        let home_dir = PathBuf::from("/tmp/home");

        let preferred =
            resolve_codex_dir_from(Some(codex_home.clone()), Some(home_dir.clone())).unwrap();
        let empty_falls_back =
            resolve_codex_dir_from(Some(PathBuf::new()), Some(home_dir.clone())).unwrap();
        let missing_falls_back = resolve_codex_dir_from(None, Some(home_dir.clone())).unwrap();

        assert_eq!(preferred, codex_home);
        assert_eq!(empty_falls_back, home_dir.join(".codex"));
        assert_eq!(missing_falls_back, home_dir.join(".codex"));
    }

    #[test]
    fn test_uninstall_codex_at_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");

        fs::write(&agents_md, "# Team rules\n\n@RTK.md\n").unwrap();
        fs::write(&rtk_md, codex_rtk_md_content(AwarenessLevel::Default)).unwrap();

        let removed_first = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();
        let removed_second = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        assert_eq!(removed_first.len(), 2);
        assert!(removed_second.is_empty());
        assert!(!rtk_md.exists());

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("@RTK.md"));
        assert!(content.contains("# Team rules"));
    }

    #[test]
    fn test_uninstall_codex_at_removes_absolute_reference() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");
        let absolute_ref = codex_rtk_md_ref(codex_dir);

        fs::write(&agents_md, format!("# Team rules\n\n{}\n", absolute_ref)).unwrap();
        fs::write(&rtk_md, codex_rtk_md_content(AwarenessLevel::Default)).unwrap();

        let removed = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        assert_eq!(removed.len(), 2);
        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains(&absolute_ref));
        assert!(content.contains("# Team rules"));
    }

    #[test]
    fn test_run_codex_mode_dry_run_writes_nothing() {
        let temp = TempDir::new().unwrap();
        let agents_md = temp.path().join("AGENTS.md");
        let rtk_md = temp.path().join("RTK.md");
        let hooks_json = temp.path().join(HOOKS_JSON);

        run_codex_mode_with_paths(
            agents_md.clone(),
            rtk_md.clone(),
            hooks_json.clone(),
            true,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            !rtk_md.exists(),
            "dry-run must not create RTK.md: {}",
            rtk_md.display()
        );
        assert!(
            !agents_md.exists(),
            "dry-run must not create AGENTS.md: {}",
            agents_md.display()
        );
        assert!(
            !hooks_json.exists(),
            "dry-run must not create hooks.json: {}",
            hooks_json.display()
        );
    }

    #[test]
    fn test_uninstall_codex_at_removes_rtk_instructions_block() {
        let temp = TempDir::new().unwrap();
        let codex_dir = temp.path();
        let agents_md = codex_dir.join("AGENTS.md");
        let rtk_md = codex_dir.join("RTK.md");

        fs::write(
            &agents_md,
            format!(
                "# Team rules\n\n{} v2 -->\nOLD RTK STUFF\n{}\n\nMore content",
                RTK_BLOCK_START, RTK_BLOCK_END
            ),
        )
        .unwrap();
        fs::write(&rtk_md, codex_rtk_md_content(AwarenessLevel::Default)).unwrap();

        let removed = uninstall_codex_at(codex_dir, InitContext::default()).unwrap();

        let content = fs::read_to_string(&agents_md).unwrap();
        assert!(!content.contains("OLD RTK STUFF"));
        assert!(content.contains("# Team rules"));
        assert!(content.contains("More content"));
        assert!(removed.iter().any(|r| r.contains("rtk-instructions block")));
    }
    #[test]
    fn test_codex_tracking_config_limits_root_to_database_parent() {
        let temp = TempDir::new().unwrap();
        let directory = temp.path().join("tracking data");
        fs::create_dir(&directory).unwrap();
        let config = codex_tracking_config(&directory.join("custom.db")).unwrap();
        let parsed: toml::Value = toml::from_str(&config).unwrap();
        assert_eq!(parsed["sandbox_mode"].as_str(), Some("workspace-write"));
        let roots = parsed["sandbox_workspace_write"]["writable_roots"]
            .as_array()
            .unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0].as_str().unwrap(),
            fs::canonicalize(&directory).unwrap().to_str().unwrap()
        );
        assert!(!directory.join("custom.db").exists());
    }

    #[test]
    fn test_codex_tracking_config_escapes_paths_without_creating_files() {
        let temp = TempDir::new().unwrap();
        let directory = temp.path().join("quoted \"directory\"");
        let config = codex_tracking_config(&directory.join("history.db")).unwrap();
        let parsed: toml::Value = toml::from_str(&config).unwrap();
        assert_eq!(
            parsed["sandbox_workspace_write"]["writable_roots"][0]
                .as_str()
                .unwrap(),
            directory.to_str().unwrap()
        );
        assert!(!directory.exists());
    }

    #[test]
    fn test_patch_codex_hooks_is_idempotent_and_preserves_existing_hooks() {
        let temp = TempDir::new().unwrap();
        let hooks_json = temp.path().join(HOOKS_JSON);
        fs::write(
            &hooks_json,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [{ "type": "command", "command": "echo existing" }]
                    }],
                    "Stop": [{
                        "hooks": [{ "type": "command", "command": "echo stop" }]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(patch_codex_hooks_json(&hooks_json, InitContext::default()).unwrap());
        assert!(!patch_codex_hooks_json(&hooks_json, InitContext::default()).unwrap());

        let root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hooks_json).unwrap()).unwrap();
        assert!(codex_hook_already_present(&root));
        assert_eq!(root["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo existing"
        );
        assert_eq!(root["hooks"]["Stop"][0]["hooks"][0]["command"], "echo stop");
        assert!(hooks_json.with_extension("json.bak").exists());
    }

    #[test]
    fn test_local_codex_install_can_be_uninstalled() {
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        fs::write(AGENTS_MD, "# Team rules\n").unwrap();
        run_codex_mode(false, InitContext::default()).unwrap();
        let uninstall_result = uninstall_codex(false, InitContext::default());

        std::env::set_current_dir(original_cwd).unwrap();
        uninstall_result.unwrap();

        assert!(!temp.path().join(RTK_MD).exists());
        assert!(!codex_hook_already_present(
            &serde_json::from_str(
                &fs::read_to_string(temp.path().join(CODEX_DIR).join(HOOKS_JSON)).unwrap()
            )
            .unwrap()
        ));
        let agents = fs::read_to_string(temp.path().join(AGENTS_MD)).unwrap();
        assert_eq!(agents.trim_end(), "# Team rules");
        assert!(!agents.contains(RTK_MD_REF));
    }

    #[test]
    fn test_remove_codex_hook_preserves_other_hooks_in_same_entry() {
        let mut root = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [
                        { "type": "command", "command": "echo user hook" },
                        { "type": "command", "command": CODEX_HOOK_COMMAND }
                    ]
                }],
                "Stop": [{
                    "hooks": [{ "type": "command", "command": "echo stop" }]
                }]
            }
        });

        assert!(remove_codex_hook_from_json(&mut root));
        assert!(!codex_hook_already_present(&root));
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo user hook"
        );
        assert_eq!(root["hooks"]["Stop"][0]["hooks"][0]["command"], "echo stop");
    }

    #[test]
    fn test_uninstall_codex_at_removes_hook_and_preserves_other_hooks() {
        let temp = TempDir::new().unwrap();
        let hooks_json = temp.path().join(HOOKS_JSON);
        fs::write(
            &hooks_json,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "echo user hook" },
                            { "type": "command", "command": CODEX_HOOK_COMMAND }
                        ]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let removed = uninstall_codex_at(temp.path(), InitContext::default()).unwrap();

        assert_eq!(removed.len(), 1);
        let root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hooks_json).unwrap()).unwrap();
        assert!(!codex_hook_already_present(&root));
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo user hook"
        );
        assert!(hooks_json.with_extension("json.bak").exists());
    }

    #[test]
    fn test_rtk_md_ownership_is_carried_by_the_file_not_by_the_build() {
        for level in [
            AwarenessLevel::Default,
            AwarenessLevel::High,
            AwarenessLevel::Full,
        ] {
            assert!(is_rtk_authored_md(&codex_rtk_md_content(level)), "{level}");
        }
        // The point of the marker: a payload from another release is still recognised, where
        // comparing against this build's copy stopped the moment the wording moved on.
        assert!(is_rtk_authored_md(&format!(
            "{RTK_MD_OWNED_HEADER}\n\n# Command output\n\nwording from some other release\n"
        )));
        // And an edit to RTK's own file does not make it the user's.
        assert!(is_rtk_authored_md(&format!(
            "{}\nmy own note\n",
            codex_rtk_md_content(AwarenessLevel::Full)
        )));
        // Leading blank lines are not content.
        assert!(is_rtk_authored_md(&format!(
            "\n\n{}",
            codex_rtk_md_content(AwarenessLevel::Default)
        )));

        // A file RTK never wrote, including one holding the awareness text without the line
        // that claims it.
        assert!(!is_rtk_authored_md("my own notes about rtk\n"));
        assert!(!is_rtk_authored_md(""));

        // The payloads v0.49.0 and the builds after it wrote with nothing claiming them.
        // Should the awareness text ever be reworded, these assertions go rather than gaining
        // a digest: every file written from then on carries the ownership line instead.
        for payload in [
            RTK_AWARENESS_DEFAULT,
            RTK_AWARENESS_HIGH,
            RTK_AWARENESS_FULL,
        ] {
            let remedy = "the awareness text was reworded: delete these two assertions rather \
                          than adding a digest for the new wording -- every file written from \
                          then on carries the ownership line, so nothing needs recognising by \
                          content";
            assert!(is_rtk_authored_md(payload), "{remedy}");
            assert!(
                is_rtk_authored_md(&payload.replace('\n', "\r\n")),
                "{remedy}"
            );
        }
        // A line of spaces is not content: the claim is on the first line that is.
        assert!(is_rtk_authored_md(&format!(
            "   \n{RTK_MD_OWNED_HEADER}\n\npayload\n"
        )));

        // An editor that reindented the file has not changed who wrote it. (Line endings
        // need no such tolerance: `str::lines` drops the carriage return itself.)
        assert!(is_rtk_authored_md(&format!(
            "   {RTK_MD_OWNED_HEADER}\n\npayload\n"
        )));
        assert!(is_rtk_authored_md(&format!(
            "  {RTK_MD_LEGACY_CODEX_HEADING}  \n\nwording\n"
        )));

        // A checkout may have rewritten the line endings of any of them.
        assert!(is_rtk_authored_md(&format!(
            "{RTK_MD_LEGACY_CODEX_HEADING}\r\n\r\nwording from that release\r\n"
        )));
        assert!(is_rtk_authored_md(
            &codex_rtk_md_content(AwarenessLevel::Default).replace('\n', "\r\n")
        ));

        // Their headings alone prove nothing -- a user's notes may open with either.
        assert!(!is_rtk_authored_md("# RTK\n\nmy own notes\n"));
        assert!(!is_rtk_authored_md(
            "# Command output\n\nhow I like it summarised\n"
        ));
        // And neither does a payload the user has since made their own.
        assert!(!is_rtk_authored_md(&format!(
            "{RTK_AWARENESS_FULL}\n\nmy own note\n"
        )));
        // The frozen heading of the payload every release up to v0.48.0 wrote, so an
        // install done before the ownership line existed is still recognised as RTK's.
        assert!(is_rtk_authored_md(&format!(
            "{RTK_MD_LEGACY_CODEX_HEADING}\n\nwording from that release\n"
        )));

        // No release ever wrote the `--claude-md` block into a file named RTK.md, so its
        // presence means a user put it in theirs, alongside notes uninstall must not take.
        assert!(!is_rtk_authored_md(RTK_INSTRUCTIONS));
        assert!(!is_rtk_authored_md(&format!(
            "# My notes\n\n{}\n",
            codex_rtk_md_content(AwarenessLevel::Default)
        )));
    }

    #[test]
    fn test_codex_uninstall_keeps_a_user_authored_rtk_md() {
        // `--codex` puts RTK.md in the project root, where the name is not RTK's to claim:
        // uninstall removes RTK's own artifacts, never a file that merely shares their name.
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        let agents_md = dir.path().join(AGENTS_MD);
        let hooks_json = dir.path().join(CODEX_DIR).join(HOOKS_JSON);
        fs::write(&rtk_md, "my own notes about rtk\n").expect("write");
        fs::write(&agents_md, "# Agents\n").expect("write");

        let removed = uninstall_codex_with_paths(
            &agents_md,
            &rtk_md,
            RtkMdScope::ProjectRoot,
            Some(&hooks_json),
            &[RTK_MD_REF],
            InitContext::default(),
        )
        .expect("uninstall");

        assert_eq!(
            fs::read_to_string(&rtk_md).expect("read"),
            "my own notes about rtk\n",
            "a user-authored RTK.md must survive uninstall"
        );
        assert!(
            !removed.iter().any(|item| item.starts_with("RTK.md")),
            "and must not be reported as removed: {removed:?}"
        );
    }

    #[test]
    fn test_codex_uninstall_removes_rtks_own_rtk_md() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        let agents_md = dir.path().join(AGENTS_MD);
        let hooks_json = dir.path().join(CODEX_DIR).join(HOOKS_JSON);
        fs::write(&rtk_md, codex_rtk_md_content(AwarenessLevel::Default)).expect("write");
        fs::write(&agents_md, "# Agents\n").expect("write");

        let removed = uninstall_codex_with_paths(
            &agents_md,
            &rtk_md,
            RtkMdScope::ProjectRoot,
            Some(&hooks_json),
            &[RTK_MD_REF],
            InitContext::default(),
        )
        .expect("uninstall");

        assert!(!rtk_md.exists(), "RTK's own RTK.md must still be removed");
        assert!(removed.iter().any(|item| item.starts_with("RTK.md")));
    }

    #[test]
    fn test_codex_install_moves_a_user_authored_rtk_md_aside() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        fs::write(&rtk_md, "my own notes\n").expect("write");

        let backup =
            back_up_foreign_rtk_md(&rtk_md, RtkMdScope::ProjectRoot, InitContext::default())
                .expect("backup")
                .expect("a foreign RTK.md must be moved aside");
        assert_eq!(fs::read_to_string(&backup).expect("read"), "my own notes\n");
        assert!(!rtk_md.exists(), "the original is moved, not copied");

        // A second run must not overwrite the first rescue copy.
        fs::write(&rtk_md, "notes again\n").expect("write");
        let second =
            back_up_foreign_rtk_md(&rtk_md, RtkMdScope::ProjectRoot, InitContext::default())
                .expect("backup")
                .expect("second backup");
        assert_ne!(second, backup);
        assert_eq!(fs::read_to_string(&backup).expect("read"), "my own notes\n");
        assert_eq!(fs::read_to_string(&second).expect("read"), "notes again\n");
    }

    #[test]
    fn test_codex_install_leaves_rtks_own_rtk_md_alone() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        assert_eq!(
            back_up_foreign_rtk_md(&rtk_md, RtkMdScope::ProjectRoot, InitContext::default())
                .expect("absent"),
            None,
            "nothing to rescue when there is no file"
        );

        fs::write(&rtk_md, codex_rtk_md_content(AwarenessLevel::Full)).expect("write");
        assert_eq!(
            back_up_foreign_rtk_md(&rtk_md, RtkMdScope::ProjectRoot, InitContext::default())
                .expect("ours"),
            None,
            "RTK's own file is overwritten in place, with no backup litter"
        );
        // Including one written by a release whose wording has since moved on: an upgrade
        // must not leave a numbered backup behind on every run.
        fs::write(
            &rtk_md,
            format!("{RTK_MD_OWNED_HEADER}\n\nwording from some other release\n"),
        )
        .expect("write");
        assert_eq!(
            back_up_foreign_rtk_md(&rtk_md, RtkMdScope::ProjectRoot, InitContext::default())
                .expect("older payload"),
            None
        );
        assert!(rtk_md.exists());
    }

    #[test]
    fn test_codex_install_dry_run_moves_nothing() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        fs::write(&rtk_md, "my own notes\n").expect("write");

        let ctx = InitContext {
            dry_run: true,
            ..InitContext::default()
        };
        assert_eq!(
            back_up_foreign_rtk_md(&rtk_md, RtkMdScope::ProjectRoot, ctx).expect("dry run"),
            None
        );
        assert_eq!(fs::read_to_string(&rtk_md).expect("read"), "my own notes\n");
        assert!(!rtk_md.with_extension("md.bak").exists());
    }

    /// The guard exists to keep a write inside the project, not to strand the files that are
    /// already inside it: refusing to clean those leaves artifacts with no command to remove
    /// them, since `--global` acts on a different directory.
    #[test]
    fn test_uninstall_cleans_the_project_even_without_the_hook_file() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        let agents_md = dir.path().join(AGENTS_MD);
        fs::write(&rtk_md, codex_rtk_md_content(AwarenessLevel::Default)).expect("write");
        fs::write(&agents_md, format!("# Team rules\n\n{RTK_MD_REF}\n")).expect("write");

        let removed = uninstall_codex_with_paths(
            &agents_md,
            &rtk_md,
            RtkMdScope::ProjectRoot,
            None,
            &[RTK_MD_REF],
            InitContext::default(),
        )
        .expect("uninstall");

        assert!(!rtk_md.exists(), "RTK.md must still be removed");
        assert!(
            !fs::read_to_string(&agents_md)
                .expect("read")
                .contains(RTK_MD_REF),
            "the AGENTS.md reference must still go"
        );
        assert!(removed.iter().any(|item| item.starts_with("RTK.md")));
        assert!(!removed.iter().any(|item| item.starts_with("hooks.json")));
    }

    #[cfg(unix)]
    #[test]
    fn test_project_scoped_write_refuses_to_leave_the_project() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        let hooks_json = Path::new(CODEX_DIR).join(HOOKS_JSON);

        // Nothing created yet: the ordinary first-install case must pass.
        ensure_inside_root(project.path(), Path::new(AGENTS_MD)).expect("plain AGENTS.md");
        ensure_inside_root(project.path(), Path::new(RTK_MD)).expect("plain RTK.md");
        ensure_inside_root(project.path(), &hooks_json).expect("plain .codex/hooks.json");

        symlink(elsewhere.path(), project.path().join(CODEX_DIR)).expect("symlink");
        let error = ensure_inside_root(project.path(), &hooks_json)
            .expect_err("a symlinked .codex must be refused");
        assert!(error.to_string().contains("outside the project"), "{error}");
        assert!(
            fs::read_dir(elsewhere.path())
                .expect("read_dir")
                .next()
                .is_none(),
            "and nothing may be written there"
        );
    }

    /// `fs::copy` follows a symlink at the destination, so the backup sibling carries the
    /// existing hooks out of the project while `.codex` itself is an ordinary directory.
    #[cfg(unix)]
    #[test]
    fn test_the_backup_destination_must_stay_inside_the_project_too() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        let hooks_json = Path::new(CODEX_DIR).join(HOOKS_JSON);
        let backup = backup_path_for(&hooks_json);

        fs::create_dir(project.path().join(CODEX_DIR)).expect("mkdir");
        // A real directory, and a backup path that does not exist yet, are both inside.
        ensure_inside_root(project.path(), &hooks_json).expect("real .codex");
        ensure_inside_root(project.path(), &backup).expect("plain backup path");

        // The link is dangling, as it would be before the first backup is taken: resolving
        // only its existing ancestors would report it as sitting right where the link does.
        symlink(
            elsewhere.path().join("stolen.json"),
            project.path().join(&backup),
        )
        .expect("symlink");
        let error = ensure_inside_root(project.path(), &backup)
            .expect_err("a symlinked backup destination must be refused");
        assert!(error.to_string().contains("outside the project"), "{error}");
    }

    /// A link chain leaves the project at whichever hop points out of it, and that need not
    /// be the first. While any hop dangles, `canonicalize` reports the whole chain as sitting
    /// where it broke, so only walking it hop by hop says where a write would land.
    #[cfg(unix)]
    #[test]
    fn test_a_symlink_chain_is_followed_past_its_first_hop() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        let backup = backup_path_for(&Path::new(CODEX_DIR).join(HOOKS_JSON));

        fs::create_dir(project.path().join(CODEX_DIR)).expect("mkdir");
        // Both hops dangle, as they do before the first backup is taken. The middle one
        // stays inside the project; the one after it does not.
        symlink("../mid", project.path().join(&backup)).expect("first hop");
        symlink(
            elsewhere.path().join("stolen.json"),
            project.path().join("mid"),
        )
        .expect("second hop");

        let error = ensure_inside_root(project.path(), &backup)
            .expect_err("a chain that leaves the project must be refused");
        let message = error.to_string();
        assert!(message.contains("outside the project"), "{message}");
        assert!(
            message.contains(
                &fs::canonicalize(elsewhere.path())
                    .expect("canonical elsewhere")
                    .join("stolen.json")
                    .display()
                    .to_string()
            ),
            "the refusal must name where the chain actually lands: {message}"
        );
    }

    /// A link in the middle of the chain is a place the write can land, so pointing it back
    /// into the project must not buy it a pass: the far end is not the only thing that moves.
    #[cfg(unix)]
    #[test]
    fn test_a_link_partway_along_the_chain_may_not_sit_outside() {
        use std::os::unix::fs::symlink;

        let enclosing = TempDir::new().expect("enclosing");
        let root = fs::canonicalize(enclosing.path()).expect("canonical");
        let project = root.join("project");
        fs::create_dir_all(project.join(CODEX_DIR)).expect("project");
        let outside = root.join("elsewhere");
        fs::create_dir(&outside).expect("elsewhere");

        // hooks.json -> elsewhere/x -> project/kept.json, whose end sits back inside.
        symlink(outside.join("x"), project.join(CODEX_DIR).join(HOOKS_JSON)).expect("first");
        symlink(project.join("kept.json"), outside.join("x")).expect("middle");

        let error = ensure_inside_root(&project, &Path::new(CODEX_DIR).join(HOOKS_JSON))
            .expect_err("a link outside the project is a write outside the project");
        let message = error.to_string();
        assert!(
            message.contains(&outside.join("x").display().to_string()),
            "the refusal must name the link that sits outside: {message}"
        );
    }

    /// The bound counts hops followed, so a chain of exactly that length still resolves.
    #[cfg(unix)]
    #[test]
    fn test_a_chain_of_exactly_the_hop_limit_still_settles() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tmp");
        let tmp = fs::canonicalize(temp.path()).expect("canonical");
        let end = tmp.join("end");
        fs::write(&end, "landed").expect("end");

        // link1 -> link2 -> ... -> linkN -> end is N hops to a target that is not a link.
        let link = |n: usize| tmp.join(format!("link{n}"));
        symlink(&end, link(MAX_SYMLINK_HOPS)).expect("last link");
        for n in (1..MAX_SYMLINK_HOPS).rev() {
            symlink(link(n + 1), link(n)).expect("link");
        }

        match follow_symlink_chain(&link(1)) {
            SymlinkChain::Target(target) => assert_eq!(target, end),
            SymlinkChain::Settled => panic!("the head of the chain is a symlink"),
            SymlinkChain::Exhausted => {
                panic!("a chain of exactly {MAX_SYMLINK_HOPS} hops is within the bound")
            }
            SymlinkChain::Unreadable => panic!("every link in the chain is readable"),
        }
    }

    /// One hop past the bound is where the walk must give up rather than keep going.
    #[cfg(unix)]
    #[test]
    fn test_a_chain_one_hop_past_the_limit_is_refused() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tmp");
        let tmp = fs::canonicalize(temp.path()).expect("canonical");
        let end = tmp.join("end");
        fs::write(&end, "landed").expect("end");

        let over = MAX_SYMLINK_HOPS + 1;
        let link = |n: usize| tmp.join(format!("link{n}"));
        symlink(&end, link(over)).expect("last link");
        for n in (1..over).rev() {
            symlink(link(n + 1), link(n)).expect("link");
        }

        assert!(
            matches!(follow_symlink_chain(&link(1)), SymlinkChain::Exhausted),
            "a chain longer than the bound cannot be vouched for"
        );
    }

    /// A jump lands on a target that may itself sit behind symlinked ancestors the walk
    /// never visited, so stopping at the jump reports two spellings of one directory.
    #[cfg(unix)]
    #[test]
    fn test_resolve_symlink_components_resolves_the_jumped_to_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tmp");
        // Anchored on the canonical form: a macOS temp dir sits under `/var`, itself a link to
        // `/private/var`, which the resolver rightly follows and `temp.path()` does not.
        let tmp = fs::canonicalize(temp.path()).expect("canonical tmp");
        fs::create_dir_all(tmp.join("real/nested")).expect("real/nested");
        symlink("real", tmp.join("via")).expect("ancestor link");
        symlink(tmp.join("via/nested"), tmp.join("alias")).expect("alias");

        // The jump lands on a target that itself sits behind `via`, so a walk that stops at
        // the jump reports two spellings of one directory as two directories.
        let resolved =
            resolve_symlink_components_within(&tmp.join("alias/extensions"), MAX_SYMLINK_HOPS);
        let Resolution::Fully(resolved) = resolved else {
            panic!("a finite chain resolves fully");
        };
        assert_eq!(resolved, tmp.join("real/nested/extensions"));
    }

    #[test]
    fn test_a_backup_slot_is_reused_when_it_already_holds_the_same_content() {
        let tmp = TempDir::new().expect("tmp");
        let source = tmp.path().join("RTK.md");
        fs::write(&source, "the user's notes").expect("source");
        fs::write(tmp.path().join("RTK.md.bak"), "the user's notes").expect("backup");

        // A provisioning loop reapplying the same change must not consume a slot per run.
        match free_backup_slot(&source).expect("slot") {
            BackupSlot::AlreadyPreserved(path) => {
                assert_eq!(path, tmp.path().join("RTK.md.bak"));
            }
            _ => panic!("an identical backup is already preserved"),
        }
    }

    #[test]
    fn test_a_differing_backup_does_not_claim_the_content_is_preserved() {
        let tmp = TempDir::new().expect("tmp");
        let source = tmp.path().join("RTK.md");
        fs::write(&source, "the user's notes").expect("source");
        fs::write(tmp.path().join("RTK.md.bak"), "something else").expect("backup");

        match free_backup_slot(&source).expect("slot") {
            BackupSlot::Free(path) => assert_eq!(path, tmp.path().join("RTK.md.bak.1")),
            _ => panic!("differing content needs a slot of its own"),
        }
    }

    #[test]
    fn test_an_unreadable_source_still_gets_a_slot_to_be_renamed_onto() {
        let tmp = TempDir::new().expect("tmp");
        // A directory reads as an error the same way an unreadable file does, without
        // depending on the test user not being root.
        let source = tmp.path().join("RTK.md");
        fs::create_dir(&source).expect("unreadable source");

        // Whether it is already preserved cannot be known, but a rename does not need to
        // read it, so refusing outright would strand content a move could have saved.
        match free_backup_slot(&source).expect("slot") {
            BackupSlot::SourceUnreadable(path) => assert_eq!(path, tmp.path().join("RTK.md.bak")),
            _ => panic!("an unreadable source cannot be compared"),
        }
    }

    /// The ceiling test proves the probe refuses once every slot is taken; this proves it does
    /// not refuse while one is still free. An off-by-one passes the first and fails this.
    #[test]
    fn test_the_final_backup_slot_is_offered_rather_than_skipped() {
        let tmp = TempDir::new().expect("tmp");
        let source = tmp.path().join("RTK.md");
        fs::write(&source, "current").expect("source");
        // Every slot but the last, each holding something different so none can be handed
        // back by the identical-content shortcut.
        for attempt in 0..MAX_BACKUP_ATTEMPTS {
            fs::write(
                numbered_backup_path(&source, attempt),
                format!("old {attempt}"),
            )
            .expect("occupied slot");
        }

        // Coupled to the constant, so raising the ceiling cannot quietly void this the way it
        // voided an earlier test that named a fixed slot.
        match free_backup_slot(&source).expect("slot") {
            BackupSlot::Free(path) => {
                assert_eq!(path, numbered_backup_path(&source, MAX_BACKUP_ATTEMPTS));
            }
            other => panic!("the last slot is free: {other:?}"),
        }
    }

    #[test]
    fn test_backup_slots_run_out_rather_than_overwriting_a_rescue_copy() {
        let tmp = TempDir::new().expect("tmp");
        let source = tmp.path().join("RTK.md");
        fs::write(&source, "current").expect("source");
        for attempt in 0..=MAX_BACKUP_ATTEMPTS {
            fs::write(
                numbered_backup_path(&source, attempt),
                format!("old {attempt}"),
            )
            .expect("occupied slot");
        }

        let error = free_backup_slot(&source).expect_err("every slot is taken");
        assert!(
            error.to_string().contains("Remove or archive them first"),
            "{error}"
        );
    }

    #[test]
    fn test_a_backup_extends_the_name_rather_than_replacing_its_extension() {
        // `with_extension` would turn `hooks.json` into `hooks.bak`, quietly backing up under
        // a name that no longer says what the file was.
        let path = Path::new("/project/.codex/hooks.json");
        assert_eq!(
            numbered_backup_path(path, 0),
            Path::new("/project/.codex/hooks.json.bak")
        );
        assert_eq!(
            numbered_backup_path(path, 3),
            Path::new("/project/.codex/hooks.json.bak.3")
        );
    }

    /// The walk must not cost the ordinary in-project symlink its write, and must terminate
    /// on a chain that never ends.
    #[cfg(unix)]
    #[test]
    fn test_the_containment_walk_accepts_in_project_chains_and_stops_on_cycles() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        fs::create_dir(project.path().join(CODEX_DIR)).expect("mkdir");
        fs::create_dir(project.path().join("shared")).expect("mkdir");

        let backup = backup_path_for(&Path::new(CODEX_DIR).join(HOOKS_JSON));
        symlink("../shared/link", project.path().join(&backup)).expect("first hop");
        symlink("hooks.json.bak", project.path().join("shared/link")).expect("second hop");
        ensure_inside_root(project.path(), &backup).expect("a chain that stays inside");

        symlink("b", project.path().join("a")).expect("a");
        symlink("a", project.path().join("b")).expect("b");
        let error = ensure_inside_root(project.path(), Path::new("a"))
            .expect_err("a cycle resolves nowhere, so RTK cannot vouch for it");
        assert!(error.to_string().contains("symlinks"), "{error}");
    }

    /// A link above the file decides where the write lands just as the file's own link does,
    /// and `canonicalize` cannot see past it either while its target is missing.
    #[cfg(unix)]
    #[test]
    fn test_a_symlinked_ancestor_is_resolved_even_while_it_dangles() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        symlink(
            elsewhere.path().join("codex"),
            project.path().join(CODEX_DIR),
        )
        .expect("dangling .codex");

        let hooks_json = Path::new(CODEX_DIR).join(HOOKS_JSON);
        let error = ensure_inside_root(project.path(), &hooks_json)
            .expect_err("a dangling .codex link out of the project must be refused");
        assert!(error.to_string().contains("outside the project"), "{error}");
    }

    /// `..` after a component that does not exist yet defeats resolution by ancestor, so the
    /// comparison has to fold it in itself rather than compare the path as written.
    #[cfg(unix)]
    #[test]
    fn test_a_parent_hop_past_a_missing_component_is_folded_before_comparing() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let backup = backup_path_for(&Path::new(CODEX_DIR).join(HOOKS_JSON));
        fs::create_dir(project.path().join(CODEX_DIR)).expect("mkdir");
        symlink("gone/../../../stolen.json", project.path().join(&backup)).expect("symlink");

        let error = ensure_inside_root(project.path(), &backup)
            .expect_err("a target that climbs out of the project must be refused");
        assert!(error.to_string().contains("outside the project"), "{error}");
    }

    /// A chain can point back into the project and still be written outside it: with its end
    /// missing, `atomic_write` cannot canonicalize either and lands on the last link itself,
    /// wherever that link happens to sit.
    #[cfg(unix)]
    #[test]
    fn test_a_link_sitting_outside_is_refused_even_when_it_points_back_in() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        symlink(elsewhere.path(), project.path().join(CODEX_DIR)).expect(".codex");
        symlink(
            project.path().join("absent").join(HOOKS_JSON),
            elsewhere.path().join(HOOKS_JSON),
        )
        .expect("dangling hop home");

        let hooks_json = Path::new(CODEX_DIR).join(HOOKS_JSON);
        let error = ensure_inside_root(project.path(), &hooks_json)
            .expect_err("the hop through the outside link must be refused");
        assert!(error.to_string().contains("outside the project"), "{error}");
    }

    /// Which scope the project-scoped command runs in is a decision of its own: `--codex`
    /// without `--global` must not treat the project root as a directory RTK owns.
    #[test]
    fn test_project_install_moves_a_user_authored_rtk_md_aside_rather_than_claiming_it() {
        let project = TempDir::new().expect("project");
        let rtk_md = project.path().join(RTK_MD);
        fs::write(&rtk_md, "my own notes\n").expect("write");

        let _cwd = CwdGuard::enter(project.path());
        run_codex_mode(false, InitContext::default()).expect("install");

        assert_eq!(
            fs::read_to_string(rtk_md.with_extension("md.bak")).expect("backup"),
            "my own notes\n",
            "the user's file must be rescued, not overwritten"
        );
        assert!(rtk_md_is_rtk_authored(&rtk_md), "and replaced by RTK's own");
    }

    /// The same decision on the way out: uninstall must not remove a project-root `RTK.md`
    /// RTK never wrote.
    #[test]
    fn test_project_uninstall_keeps_a_user_authored_rtk_md() {
        let project = TempDir::new().expect("project");
        let rtk_md = project.path().join(RTK_MD);
        fs::write(&rtk_md, "my own notes\n").expect("write");

        let _cwd = CwdGuard::enter(project.path());
        uninstall_codex(false, InitContext::default()).expect("uninstall");

        assert_eq!(
            fs::read_to_string(&rtk_md).expect("read"),
            "my own notes\n",
            "a file RTK never wrote is not RTK's to remove"
        );
    }

    /// Uninstall rewrites `hooks.json` through [`backup_and_atomic_write`], whose `fs::copy`
    /// follows a symlink at the destination, so its backup sibling needs vouching for even
    /// when `.codex` itself is an ordinary directory.
    #[cfg(unix)]
    #[test]
    fn test_uninstall_refuses_a_backup_sibling_that_leaves_the_project() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        let codex_dir = project.path().join(CODEX_DIR);
        fs::create_dir(&codex_dir).expect("mkdir");
        let hooks_json = codex_dir.join(HOOKS_JSON);
        let registered = format!(
            "{{\"hooks\":{{\"{PRE_TOOL_USE_KEY}\":[{{\"matcher\":\"Bash\",\"hooks\":[{{\"type\":\"command\",\"command\":\"{CODEX_HOOK_COMMAND}\"}}]}}]}}}}"
        );
        fs::write(&hooks_json, &registered).expect("hooks.json");
        symlink(
            elsewhere.path().join("stolen.json"),
            hooks_json.with_extension("json.bak"),
        )
        .expect("symlink");

        let _cwd = CwdGuard::enter(project.path());
        uninstall_codex(false, InitContext::default())
            .expect("uninstall still reports what it did");

        assert!(
            !elsewhere.path().join("stolen.json").exists(),
            "the project's hooks must not be copied out of it"
        );
        assert_eq!(
            fs::read_to_string(&hooks_json).expect("read"),
            registered,
            "and the hook is left registered rather than rewritten unsafely"
        );
    }

    /// A file RTK cannot read is not provably RTK's, and guessing wrong here destroys it.
    #[cfg(unix)]
    #[test]
    fn test_an_unreadable_rtk_md_is_treated_as_the_users() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        let agents_md = dir.path().join(AGENTS_MD);
        // Not valid UTF-8, so `read_to_string` fails where the bytes are perfectly readable.
        fs::write(&rtk_md, [0x23, 0x20, 0xff, 0xfe, 0x0a]).expect("write");
        fs::write(&agents_md, "# Agents\n").expect("write");

        let removed = uninstall_codex_with_paths(
            &agents_md,
            &rtk_md,
            RtkMdScope::ProjectRoot,
            None,
            &[RTK_MD_REF],
            InitContext::default(),
        )
        .expect("uninstall");

        assert!(rtk_md.exists(), "an unreadable RTK.md must survive");
        assert!(!removed.iter().any(|item| item.starts_with("RTK.md")));
    }

    /// A list of what was removed reads as a complete uninstall, so a hook the guard refused
    /// to touch has to appear in the same block -- and must not be called registered, which
    /// RTK cannot know without reading the file it just refused.
    #[test]
    fn test_the_uninstall_report_carries_a_hook_it_did_not_check() {
        let removed = vec!["RTK.md: RTK.md".to_string()];
        let error = anyhow::anyhow!("first line\nsecond line");

        let clean = codex_uninstall_report(&removed, None, false);
        assert_eq!(clean, "RTK uninstalled for Codex CLI:\n  - RTK.md: RTK.md");

        let guarded = codex_uninstall_report(&removed, Some(&error), false);
        assert!(guarded.starts_with(&clean), "{guarded}");
        assert!(guarded.contains("Not checked"), "{guarded}");
        assert!(guarded.contains("may still be registered"), "{guarded}");
        assert!(guarded.contains("first line"), "{guarded}");
        assert!(
            guarded.contains("second line"),
            "every line of the reason, not just the first: {guarded}"
        );
        for line in guarded.lines().skip(clean.lines().count() + 1) {
            assert!(line.starts_with("    "), "unindented reason: {guarded}");
        }

        // And it is still said when there was nothing else to report.
        let nothing = codex_uninstall_report(&[], Some(&error), false);
        assert!(nothing.contains("nothing to remove"), "{nothing}");
        assert!(nothing.contains("Not checked"), "{nothing}");
    }

    /// The containment walk only protects anything if `run_codex_mode` still calls it, for
    /// both paths. Driving the command rather than the helper is what says so.
    #[cfg(unix)]
    #[test]
    fn test_install_refuses_a_project_whose_codex_dir_leaves_it() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        symlink(elsewhere.path(), project.path().join(CODEX_DIR)).expect("symlink");

        let _cwd = CwdGuard::enter(project.path());
        let result = run_codex_mode(false, InitContext::default());

        let error = result.expect_err("install must refuse rather than half-configure");
        assert!(error.to_string().contains("outside the project"), "{error}");
        assert_eq!(
            fs::read_dir(elsewhere.path()).expect("read_dir").count(),
            0,
            "and write nothing there"
        );
        assert!(
            !project.path().join(RTK_MD).exists(),
            "nor anything in the project"
        );
    }

    /// The same, for the backup sibling: `fs::copy` follows a symlink at the destination, so
    /// vouching only for `hooks.json` leaves the existing hooks free to travel.
    #[cfg(unix)]
    #[test]
    fn test_install_refuses_a_backup_sibling_that_leaves_the_project() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        let codex_dir = project.path().join(CODEX_DIR);
        fs::create_dir(&codex_dir).expect("mkdir");
        fs::write(codex_dir.join(HOOKS_JSON), "{}\n").expect("hooks.json");
        symlink(
            elsewhere.path().join("stolen.json"),
            codex_dir.join(HOOKS_JSON).with_extension("json.bak"),
        )
        .expect("symlink");

        let _cwd = CwdGuard::enter(project.path());
        let result = run_codex_mode(false, InitContext::default());

        let error = result.expect_err("install must refuse");
        assert!(error.to_string().contains("outside the project"), "{error}");
        assert!(
            !elsewhere.path().join("stolen.json").exists(),
            "the project's hooks must not travel"
        );
    }

    /// Uninstall reads and rewrites `hooks.json`, so it needs the same vouching -- and must
    /// still clean what it can, saying on stdout that the hook was left behind.
    #[cfg(unix)]
    #[test]
    fn test_uninstall_leaves_a_hooks_file_that_sits_outside_the_project_alone() {
        use std::os::unix::fs::symlink;

        let project = TempDir::new().expect("project");
        let elsewhere = TempDir::new().expect("elsewhere");
        let registered = format!(
            "{{\"hooks\":{{\"{PRE_TOOL_USE_KEY}\":[{{\"matcher\":\"Bash\",\"hooks\":[{{\"type\":\"command\",\"command\":\"{CODEX_HOOK_COMMAND}\"}}]}}]}}}}"
        );
        // The guard is the only reason this survives, so the fixture has to be one uninstall
        // would otherwise rewrite.
        assert!(
            remove_codex_hook_from_json(&mut from_json_str(&registered).expect("json")),
            "the fixture must carry a hook uninstall recognises"
        );
        fs::write(elsewhere.path().join(HOOKS_JSON), &registered).expect("outside hooks.json");
        symlink(elsewhere.path(), project.path().join(CODEX_DIR)).expect("symlink");
        fs::write(
            project.path().join(RTK_MD),
            codex_rtk_md_content(AwarenessLevel::Default),
        )
        .expect("RTK.md");

        let _cwd = CwdGuard::enter(project.path());
        let result = uninstall_codex(false, InitContext::default());

        result.expect("uninstall still cleans the project");
        assert_eq!(
            fs::read_to_string(elsewhere.path().join(HOOKS_JSON)).expect("read"),
            registered,
            "a hooks.json outside the project is not RTK's to rewrite"
        );
        assert!(
            !elsewhere
                .path()
                .join(HOOKS_JSON)
                .with_extension("json.bak")
                .exists(),
            "and not RTK's to back up either"
        );
        assert!(
            !project.path().join(RTK_MD).exists(),
            "while RTK's own RTK.md still goes"
        );
    }

    /// Whole directory trees hang off a symlink on macOS, where `/var` is one: an absolute
    /// target is normal traffic through such a link, and judging a link by where it sits
    /// rather than by where it leads would refuse every project reached through one.
    #[cfg(unix)]
    #[test]
    fn test_an_absolute_target_may_travel_through_a_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let enclosing = TempDir::new().expect("enclosing");
        fs::create_dir(enclosing.path().join("real")).expect("real");
        symlink("real", enclosing.path().join("via")).expect("ancestor link");
        let project = enclosing.path().join("via").join("project");
        fs::create_dir_all(project.join(CODEX_DIR)).expect("project");

        // Written the way the outside world names it, through the link.
        let backup = backup_path_for(&Path::new(CODEX_DIR).join(HOOKS_JSON));
        symlink(project.join("kept.json"), project.join(&backup)).expect("symlink");

        ensure_inside_root(&project, &backup).expect("a target that comes back inside");
    }

    #[test]
    fn test_a_bom_does_not_cost_rtk_its_own_rtk_md() {
        assert!(is_rtk_authored_md(&format!(
            "\u{feff}{}",
            codex_rtk_md_content(AwarenessLevel::Default)
        )));
    }

    #[test]
    fn test_lexical_normalisation_keeps_a_parent_hop_it_cannot_fold() {
        assert_eq!(
            lexically_normalized(Path::new("a/./b/../c")),
            Path::new("a/c")
        );
        assert_eq!(
            lexically_normalized(Path::new("../../a")),
            Path::new("../../a")
        );
        assert_eq!(lexically_normalized(Path::new("/../a")), Path::new("/../a"));
    }

    /// `--global` writes RTK.md into the Codex home, where RTK created it and nothing else
    /// claims the name -- including every release that wrote it there before the marker
    /// existed. Reading the marker in that scope would keep those files forever.
    #[test]
    fn test_global_codex_owns_its_rtk_md_with_or_without_the_marker() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        let agents_md = dir.path().join(AGENTS_MD);
        // Nothing in this file says RTK wrote it, which is the point: in the Codex home the
        // directory does. A fixture carrying the marker, the legacy heading or a shipped
        // payload would pass whatever the scope decided.
        fs::write(&rtk_md, "# RTK\n\nwording from some release\n").expect("write");
        fs::write(&agents_md, "# Agents\n").expect("write");
        assert!(
            !rtk_md_is_rtk_authored(&rtk_md),
            "the fixture must be one the project-root scope would keep"
        );

        assert_eq!(
            back_up_foreign_rtk_md(&rtk_md, RtkMdScope::CodexHome, InitContext::default())
                .expect("install"),
            None,
            "install overwrites it in place instead of leaving a .bak beside it"
        );

        let removed = uninstall_codex_with_paths(
            &agents_md,
            &rtk_md,
            RtkMdScope::CodexHome,
            None,
            &[RTK_MD_REF],
            InitContext::default(),
        )
        .expect("uninstall");
        assert!(!rtk_md.exists(), "uninstall must still remove it");
        assert!(removed.iter().any(|item| item.starts_with("RTK.md")));
    }

    /// An RTK-written block inside someone's own notes is a quotation, not a handover: the
    /// file is theirs and uninstall leaves all of it, block included.
    #[test]
    fn test_codex_uninstall_keeps_notes_that_merely_contain_an_rtk_block() {
        let dir = TempDir::new().expect("tempdir");
        let rtk_md = dir.path().join(RTK_MD);
        let agents_md = dir.path().join(AGENTS_MD);
        let notes = format!("# My own notes\n\n{RTK_INSTRUCTIONS}\n");
        fs::write(&rtk_md, &notes).expect("write");
        fs::write(&agents_md, "# Agents\n").expect("write");

        let removed = uninstall_codex_with_paths(
            &agents_md,
            &rtk_md,
            RtkMdScope::ProjectRoot,
            None,
            &[RTK_MD_REF],
            InitContext::default(),
        )
        .expect("uninstall");

        assert_eq!(fs::read_to_string(&rtk_md).expect("read"), notes);
        assert!(!removed.iter().any(|item| item.starts_with("RTK.md")));
    }
}
