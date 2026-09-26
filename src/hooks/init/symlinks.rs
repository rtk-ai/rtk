//! Symlink resolution shared by every init write: where a write lands, and which project
//! writes must stay inside the project.

use super::*;
use std::cell::RefCell;
use std::io::IsTerminal;
use std::path::Component;

/// How many hops one path's symlink chain is followed before the walk gives up.
///
/// The bound is per component, not per walk: [`resolve_symlink_components_within`] spends a
/// fresh budget on the descent into each target it jumps to. It sits below the kernel's own
/// limit on purpose -- RTK has to be able to say where a write lands, and a chain this deep
/// under a path RTK joins itself is not one a user maintains.
pub(super) const MAX_SYMLINK_HOPS: usize = 16;

/// What one `readlink` established.
///
/// [`Unreadable`](Self::Unreadable) is not [`NotALink`](Self::NotALink): `lstat` already said
/// this is a symlink, so reporting the read failure as "no link here" would hand a caller a
/// path nothing resolved and let it compare that as if it had.
pub(super) enum SymlinkHop {
    NotALink,
    To(PathBuf),
    Unreadable,
}

/// What following one path's symlink chain established.
pub(super) enum SymlinkChain {
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
pub(super) enum Resolution {
    /// Every symlink along the path was followed to a target that is not a link.
    Fully(PathBuf),
    /// The walk gave up: a cycle, a chain too deep, or a link it could not read. The path is
    /// as far as it got, which a caller that only acts may still use -- the kernel finishes
    /// the resolution -- but which says nothing about where the write lands.
    Unresolved(PathBuf),
}

/// Read one symlink hop, resolving a relative link against the link's own directory.
pub(super) fn symlink_hop(path: &Path) -> SymlinkHop {
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
pub(super) fn follow_symlink_chain(path: &Path) -> SymlinkChain {
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
pub(super) fn resolve_symlink_components_within(path: &Path, budget: usize) -> Resolution {
    let mut resolved = PathBuf::new();
    let mut settled = true;
    for component in path.components() {
        resolved.push(component);
        // A drive prefix or the root is never a link; `C:` on its own would even be read as
        // the drive's current directory.
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
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

/// `path` with `.` dropped and `..` folded into the component before it.
///
/// Only sound once the path holds no symlink, which is where the containment walk uses it:
/// `link/..` is the parent of the link's target, not of the link. Comparing without it would
/// mean comparing a path nothing resolved, because `canonicalize`'s fallback gives up and
/// returns its argument untouched as soon as a missing component is followed by `..`.
pub(super) fn lexically_normalized(path: &Path) -> PathBuf {
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

thread_local! {
    /// The project a [`ProjectScope`] alive on this thread writes to.
    static PROJECT: RefCell<Option<Project>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct Project {
    /// The canonical project root, `None` when the current directory cannot be resolved.
    root: Option<PathBuf>,
    mode: PatchMode,
    /// Landings outside the project already confirmed, so a file written twice (a block
    /// removed, then a reference) is asked about once.
    confirmed: Vec<PathBuf>,
}

/// Marks the writes made while it lives as project-scoped: the hook configs and extensions
/// they write must stay inside the project ([`ensure_project_file_inside`]), and any other
/// write that resolves outside it is confirmed first (`plan_write`).
#[must_use = "the scope ends when this value is dropped; bind it with `let _scope = ...`"]
pub(super) struct ProjectScope {
    previous: Option<Project>,
}

impl ProjectScope {
    /// Scope the writes to the current directory, which is the project in project mode, and
    /// confirm writes outside it according to `ctx.patch_mode`.
    pub(super) fn enter(ctx: InitContext) -> Self {
        let root = std::env::current_dir()
            .ok()
            .map(|dir| fs::canonicalize(&dir).unwrap_or(dir));
        PROJECT.with(|scope| {
            let mut scope = scope.borrow_mut();
            // A nested entry point (`rtk init` running Codex's mode) keeps what the outer one
            // already confirmed for the same project.
            let confirmed = scope
                .as_ref()
                .filter(|outer| outer.root == root)
                .map(|outer| outer.confirmed.clone())
                .unwrap_or_default();
            let project = Project {
                root,
                mode: ctx.patch_mode,
                confirmed,
            };
            Self {
                previous: scope.replace(project),
            }
        })
    }
}

impl Drop for ProjectScope {
    fn drop(&mut self) {
        PROJECT.with(|scope| {
            let mut scope = scope.borrow_mut();
            let mut previous = self.previous.take();
            // What this scope confirmed stays confirmed for an outer scope on the same project.
            if let (Some(outer), Some(inner)) = (previous.as_mut(), scope.as_ref())
                && outer.root == inner.root
            {
                outer.confirmed.clone_from(&inner.confirmed);
            }
            *scope = previous;
        });
    }
}

/// Where a write to `path` lands, and whether every symlink on the way was resolved -- the
/// condition for folding `..` lexically in [`write_landing`].
///
/// An existing file is reached through its symlinks, so a link survives the write. A dangling
/// link is followed exactly when a plain write through it would succeed: when the directory
/// its target names exists. Directories are never created through a dangling link -- that is
/// how a cloned repository would plant files where its links point, and how a link into an
/// unmounted or read-only volume would fail halfway through an install. A live symlinked
/// ancestor is a directory like any other, and is written through. Otherwise the write
/// replaces the link, and its own missing parent directories are created.
pub(super) fn resolve_write_target_with(path: &Path) -> (PathBuf, bool) {
    if let Ok(canonical) = fs::canonicalize(path) {
        return (canonical, true);
    }
    match resolve_symlink_components_within(path, MAX_SYMLINK_HOPS) {
        // A bare name has an empty parent: the current directory, which exists.
        Resolution::Fully(resolved)
            if resolved
                .parent()
                .is_some_and(|parent| parent.as_os_str().is_empty() || parent.is_dir()) =>
        {
            (resolved, true)
        }
        _ => (path.to_path_buf(), false),
    }
}

/// Whether a [`ProjectScope`] is alive on this thread.
fn in_project_scope() -> bool {
    PROJECT.with(|scope| scope.borrow().is_some())
}

/// The hard-link count and mode for an in-place project write to `target` that still needs
/// confirming; `None` outside a [`ProjectScope`], for a single link, or once confirmed.
pub(super) fn pending_hard_link_confirmation(target: &Path) -> Option<(u64, PatchMode)> {
    let links = hard_link_count(target).filter(|links| *links > 1)?;
    PROJECT.with(|scope| {
        let scope = scope.borrow();
        let project = scope.as_ref()?;
        (!project.confirmed.iter().any(|done| done == target)).then_some((links, project.mode))
    })
}

pub(super) fn warn_hard_links(path: &Path, links: u64) {
    eprintln!(
        "[warn] {} has {links} hard links; writing it changes every one of them.",
        path.display()
    );
}

#[cfg(unix)]
pub(super) fn hard_link_count(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|metadata| metadata.nlink())
}

#[cfg(not(unix))]
pub(super) fn hard_link_count(_path: &Path) -> Option<u64> {
    None
}

pub(super) fn mark_confirmed(landing: PathBuf) {
    PROJECT.with(|scope| {
        if let Some(project) = scope.borrow_mut().as_mut() {
            project.confirmed.push(landing);
        }
    });
}

pub(super) fn dry_run_outcome(mode: PatchMode) -> &'static str {
    match mode {
        PatchMode::Auto => "would write it (--auto-patch)",
        PatchMode::Ask if std::io::stdin().is_terminal() => "would ask before writing it",
        PatchMode::Ask => "would refuse it without a terminal to ask",
        PatchMode::Skip => "would refuse it (--no-patch)",
    }
}

/// Whether `target`, inside a [`ProjectScope`], lies outside the project.
pub(super) fn resolves_outside_project(target: &Path) -> bool {
    PROJECT.with(|scope| {
        let scope = scope.borrow();
        let Some(root) = scope.as_ref().and_then(|project| project.root.as_ref()) else {
            return false;
        };
        !landing_of(target, true).starts_with(root)
    })
}

pub(super) fn warn_outside_project(path: &Path, landing: &Path, root: &Path) {
    eprintln!(
        "[warn] {} resolves to {}, outside the project at {}.",
        path.display(),
        landing.display(),
        root.display()
    );
}

/// The landing, project root and mode for a write that resolves outside the current project
/// and has not been confirmed yet; `None` outside a [`ProjectScope`], and for a device or a
/// FIFO (such as a file linked to `/dev/null`), which is written through, not replaced.
pub(super) fn outside_project_landing(
    target: &Path,
    resolved: bool,
) -> Option<(PathBuf, PathBuf, PatchMode)> {
    if is_special_file(target) {
        return None;
    }
    PROJECT.with(|scope| {
        let scope = scope.borrow();
        let project = scope.as_ref()?;
        let root = project.root.as_ref()?;
        let landing = landing_of(target, resolved);
        (!landing.starts_with(root) && !project.confirmed.contains(&landing))
            .then(|| (landing, root.clone(), project.mode))
    })
}

/// Ask, or decide by `mode`, whether to go ahead with a project write that `why` makes worth
/// a question; refuse it when the answer is no.
pub(super) fn confirm_project_write(
    path: &Path,
    why: &str,
    target: &Path,
    mode: PatchMode,
) -> Result<()> {
    let confirmed = match mode {
        PatchMode::Auto => true,
        PatchMode::Ask => prompt_user_confirmation(&format!("Write {} anyway?", target.display()))?,
        PatchMode::Skip => false,
    };
    if !confirmed {
        anyhow::bail!(
            "Not writing {}: {why}.\n\
             Answer yes in a terminal, or re-run with --auto-patch, which also answers \
             init's other prompts without asking.",
            path.display()
        );
    }
    Ok(())
}

/// The canonical location a write to `path` lands on, for telling whether two paths name the
/// same file. It follows exactly what a write follows.
pub(super) fn write_landing(path: &Path) -> PathBuf {
    let (target, resolved) = resolve_write_target_with(path);
    landing_of(&target, resolved)
}

fn landing_of(target: &Path, resolved: bool) -> PathBuf {
    let target = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let target = if resolved {
        lexically_normalized(&target)
    } else {
        target
    };
    canonicalize_path_for_comparison(&target)
}

/// Refuse a project-scoped write whose path leaves the project.
///
/// A hook config or an extension, and the backup RTK takes next to it, are relative names RTK
/// joins itself, so a symlinked component is the only way they can resolve elsewhere -- and
/// then `rtk init` would register something that runs commands in a directory the user never
/// named. The global mode exists for writing outside the project.
///
/// This answers for the tree as it stands when asked. A process rewriting these paths while
/// init runs can still move the write afterwards; the case it is built for is a repository
/// that ships the links, which is settled before init starts.
pub(super) fn ensure_inside_project(path: &Path, agent: &str) -> Result<()> {
    let root = std::env::current_dir().context("Failed to resolve the current directory")?;
    ensure_inside_root(&root, path, agent)
}

/// [`ensure_inside_project`] against an explicit root.
pub(super) fn ensure_inside_root(root: &Path, path: &Path, agent: &str) -> Result<()> {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // Anchored to the root before resolving: these paths are relative, and
    // `canonicalize_path_for_comparison` hands a relative path straight back when none of its
    // components exist yet, which no absolute root can ever contain.
    let site = root.join(path);

    ensure_chain_sits_inside(&root, path, &site, agent)?;

    match resolve_symlink_components_within(&site, MAX_SYMLINK_HOPS) {
        // A walk that gave up cannot say where the write lands, and a path RTK cannot place
        // is one it must not write to.
        Resolution::Unresolved(_) => anyhow::bail!(
            "{} passes through a symlink RTK cannot follow to an end, \
             so RTK cannot say where a write to it would land.\n\
             Remove the symlink, or use --global to configure {agent} outside the project.",
            path.display()
        ),
        Resolution::Fully(resolved) => {
            let resolved = canonicalize_path_for_comparison(&lexically_normalized(&resolved));
            if !resolved.starts_with(&root) {
                anyhow::bail!(
                    "{} resolves to {}, outside the project at {}.\n\
                     Remove the symlink, or use --global to configure {agent} outside the project.",
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
/// The write follows the chain again when it happens, so every link along it decides where the
/// write lands. One the project does not contain is one an attacker may own: pointing it back
/// inside passes a check that only looked at the far end, while leaving the middle free to be
/// re-aimed afterwards.
///
/// Only the chain of the site itself is judged this way. Links on *ancestor* components are
/// traversed, never sited -- whole directory trees hang off one on macOS, so refusing those
/// would refuse every absolute target under `/var` or `/tmp`.
fn ensure_chain_sits_inside(root: &Path, named: &Path, site: &Path, agent: &str) -> Result<()> {
    let mut hop = site.to_path_buf();
    for _ in 0..=MAX_SYMLINK_HOPS {
        match symlink_hop(&hop) {
            SymlinkHop::NotALink => return Ok(()),
            SymlinkHop::Unreadable => anyhow::bail!(
                "{} passes through a symlink at {} that RTK cannot read, \
                 so RTK cannot say where a write to it would land.\n\
                 Remove the symlink, or use --global to configure {agent} outside the project.",
                named.display(),
                hop.display()
            ),
            SymlinkHop::To(next) => {
                let at = link_site(&hop);
                if !at.starts_with(root) {
                    anyhow::bail!(
                        "{} is a symlink at {}, outside the project at {}.\n\
                         Remove the symlink, or use --global to configure {agent} outside the project.",
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
         Remove the symlink, or use --global to configure {agent} outside the project.",
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

/// In project mode, refuse to write `path` -- a file that makes `agent` run something, such
/// as a hook config, an extension or its backup -- unless it resolves inside the project.
/// Instruction files are not guarded: a project `CLAUDE.md` or `AGENTS.md` linked to a shared
/// copy outside the repository is a setup users choose. Outside a [`ProjectScope`] this does
/// nothing; global mode writes under the agent's own config directory, wherever the user's
/// links lead.
pub(super) fn ensure_project_file_inside(path: &Path, agent: &str) -> Result<()> {
    if !in_project_scope() {
        return Ok(());
    }
    ensure_inside_project(path, agent)
}

/// [`ensure_project_file_inside`] for a JSON hook config and the backup RTK takes next to it.
pub(super) fn ensure_project_json_inside(path: &Path, agent: &str) -> Result<()> {
    ensure_project_file_inside(path, agent)?;
    ensure_project_file_inside(&backup_path_for(path), agent)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn test_write_refuses_a_dangling_link_into_a_missing_directory() {
        // RTK never creates directories through a link, and never replaces the user's link.
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let link = project.join(".clinerules");
        symlink("../outside/newdir/rules", &link).unwrap();

        let error = write_file(&link, WriteKind::Owned, "rules").unwrap_err();

        assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert!(!temp.path().join("outside").exists());
    }

    #[test]
    fn test_write_through_a_live_directory_link_lands_in_its_target() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        let shared = temp.path().join("shared-github");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir(&shared).unwrap();
        symlink(&shared, project.join(".github")).unwrap();

        write_file(
            &project.join(".github").join("hooks.json"),
            WriteKind::Owned,
            "{}",
        )
        .unwrap();

        assert_eq!(fs::read_to_string(shared.join("hooks.json")).unwrap(), "{}");
    }

    #[test]
    fn test_write_follows_a_dangling_link_into_an_existing_directory() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::create_dir_all(temp.path().join("dotfiles/claude")).unwrap();
        let link = home.join("settings.json");
        symlink("../dotfiles/claude/settings.json", &link).unwrap();

        write_file(&link, WriteKind::Owned, "{}").unwrap();

        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(temp.path().join("dotfiles/claude/settings.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn test_write_follows_a_dangling_link_to_a_bare_name() {
        let temp = TempDir::new().unwrap();
        let _cwd = CwdGuard::enter(temp.path());
        symlink("AGENTS.md", "CLAUDE.md").unwrap();

        write_file(Path::new("CLAUDE.md"), WriteKind::Owned, "notes").unwrap();

        assert!(fs::symlink_metadata("CLAUDE.md").unwrap().is_symlink());
        assert_eq!(fs::read_to_string("AGENTS.md").unwrap(), "notes");
    }

    #[test]
    fn test_project_codex_agents_md_outside_is_refused_before_the_hook() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir(temp.path().join("shared")).unwrap();
        fs::write(temp.path().join("shared/AGENTS.md"), "shared\n").unwrap();
        symlink("../shared/AGENTS.md", project.join("AGENTS.md")).unwrap();
        fs::write(project.join("RTK.md"), "my own notes\n").unwrap();
        let _cwd = CwdGuard::enter(&project);

        let error = super::super::codex::run_codex_mode(
            false,
            InitContext {
                patch_mode: PatchMode::Skip,
                ..InitContext::default()
            },
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("outside the project"),
            "{error:#}"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("shared/AGENTS.md")).unwrap(),
            "shared\n"
        );
        // The user's RTK.md is kept aside, and no hook is registered.
        assert_eq!(
            fs::read_to_string(project.join("RTK.md.bak")).unwrap(),
            "my own notes\n"
        );
        assert!(!project.join(".codex").exists());
    }

    #[test]
    fn test_project_codex_agents_md_outside_is_written_with_auto_patch() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir(temp.path().join("shared")).unwrap();
        symlink("../shared/AGENTS.md", project.join("AGENTS.md")).unwrap();
        let _cwd = CwdGuard::enter(&project);

        super::super::codex::run_codex_mode(
            false,
            InitContext {
                patch_mode: PatchMode::Auto,
                ..InitContext::default()
            },
        )
        .unwrap();

        assert!(
            fs::read_to_string(temp.path().join("shared/AGENTS.md"))
                .unwrap()
                .contains("@RTK.md")
        );
    }

    #[test]
    fn test_write_does_not_climb_out_of_a_missing_directory() {
        // A plain write through this link fails: the kernel does not resolve `..` after a
        // directory that does not exist. Following it lexically would create `missing/`.
        let temp = TempDir::new().unwrap();
        let link = temp.path().join("rules.md");
        symlink("missing/../elsewhere.md", &link).unwrap();

        assert!(write_file(&link, WriteKind::Owned, "rules").is_err());

        assert!(!temp.path().join("missing").exists());
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    }

    /// A project whose `rel` is a live link to a file outside it. Returns the tempdir, the
    /// project and the outside file.
    fn project_with_outside_link(rel: &str) -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        let outside = temp.path().join("outside").join("victim");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, "VICTIM").unwrap();
        let link = project.join(rel);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&outside, &link).unwrap();
        (temp, project, outside)
    }

    fn assert_refused(result: Result<()>, outside: &Path) {
        let error = result.expect_err("a hook file linked out of the project must be refused");
        assert!(
            format!("{error:#}").contains("outside the project"),
            "{error:#}"
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), "VICTIM");
    }

    #[test]
    fn test_project_trae_hooks_linked_out_of_the_project_are_refused() {
        let (_temp, project, outside) = project_with_outside_link(".trae/hooks.json");
        let _cwd = CwdGuard::enter(&project);
        assert_refused(
            super::super::trae::run_trae_mode(false, InitContext::default()),
            &outside,
        );
    }

    #[test]
    fn test_project_droid_hooks_linked_out_of_the_project_are_refused() {
        let (_temp, project, outside) = project_with_outside_link(".factory/hooks.json");
        let _cwd = CwdGuard::enter(&project);
        assert_refused(
            super::super::droid::run_droid_mode(false, InitContext::default()),
            &outside,
        );
    }

    #[test]
    fn test_project_copilot_hook_linked_out_of_the_project_is_refused_before_any_write() {
        let (_temp, project, outside) = project_with_outside_link(".github/hooks/rtk-rewrite.json");
        let _cwd = CwdGuard::enter(&project);
        assert_refused(
            super::super::copilot::run_copilot(InitContext::default()),
            &outside,
        );
        assert!(!project.join(".github/copilot-instructions.md").exists());
    }

    #[test]
    fn test_project_pi_extension_linked_out_of_the_project_is_refused() {
        let (_temp, project, outside) = project_with_outside_link(".pi/extensions/rtk.ts");
        let _cwd = CwdGuard::enter(&project);
        assert_refused(
            super::super::pi::run_pi_mode_with_patch_mode(
                false,
                PatchMode::Auto,
                InitContext {
                    patch_mode: PatchMode::Auto,
                    ..InitContext::default()
                },
            ),
            &outside,
        );
    }

    /// A project whose `.clinerules` links to a file in a shared directory outside it.
    fn project_with_shared_rules() -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir(temp.path().join("shared")).unwrap();
        symlink("../shared/clinerules", project.join(".clinerules")).unwrap();
        let shared = temp.path().join("shared/clinerules");
        (temp, project, shared)
    }

    #[test]
    fn test_project_rules_outside_the_project_are_written_with_auto_patch() {
        let (_temp, project, shared) = project_with_shared_rules();
        let _cwd = CwdGuard::enter(&project);

        super::super::instructions_agents::run_cline_mode(InitContext {
            patch_mode: PatchMode::Auto,
            ..InitContext::default()
        })
        .unwrap();

        assert!(
            fs::symlink_metadata(project.join(".clinerules"))
                .unwrap()
                .is_symlink()
        );
        assert!(fs::read_to_string(shared).unwrap().contains("rtk"));
    }

    #[test]
    fn test_project_rules_outside_the_project_are_refused_with_no_patch() {
        let (_temp, project, shared) = project_with_shared_rules();
        let _cwd = CwdGuard::enter(&project);

        let error = super::super::instructions_agents::run_cline_mode(InitContext {
            patch_mode: PatchMode::Skip,
            ..InitContext::default()
        })
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("outside the project"),
            "{error:#}"
        );
        assert!(!shared.exists());
        assert!(
            fs::symlink_metadata(project.join(".clinerules"))
                .unwrap()
                .is_symlink()
        );
    }

    #[test]
    fn test_refused_write_outside_the_project_keeps_the_earlier_backup() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir(temp.path().join("outside")).unwrap();
        fs::write(temp.path().join("outside/settings.json"), "{}").unwrap();
        let path = project.join("settings.json");
        symlink("../outside/settings.json", &path).unwrap();
        fs::write(backup_path_for(&path), "earlier").unwrap();
        let _cwd = CwdGuard::enter(&project);
        let _scope = ProjectScope::enter(InitContext {
            patch_mode: PatchMode::Skip,
            ..InitContext::default()
        });

        assert!(write_file(&path, WriteKind::Config, "{\"new\":1}").is_err());

        assert_eq!(
            fs::read_to_string(backup_path_for(&path)).unwrap(),
            "earlier"
        );
    }

    fn skip() -> InitContext {
        InitContext {
            patch_mode: PatchMode::Skip,
            ..InitContext::default()
        }
    }

    #[test]
    fn test_project_file_linked_to_dev_null_is_written_through() {
        let temp = TempDir::new().unwrap();
        symlink("/dev/null", temp.path().join(".clinerules")).unwrap();
        let _cwd = CwdGuard::enter(temp.path());

        super::super::instructions_agents::run_cline_mode(skip()).unwrap();

        assert_eq!(
            fs::read_link(temp.path().join(".clinerules")).unwrap(),
            Path::new("/dev/null")
        );
    }

    #[test]
    fn test_read_only_file_outside_the_project_is_reported_as_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, project, shared) = project_with_shared_rules();
        fs::write(&shared, "team rules\n").unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o444)).unwrap();
        if !read_only_is_enforced(&shared) {
            return;
        }
        let _cwd = CwdGuard::enter(&project);

        let error = super::super::instructions_agents::run_cline_mode(skip()).unwrap_err();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(format!("{error:#}").contains("read-only"), "{error:#}");
    }

    #[test]
    fn test_nested_project_scope_keeps_the_outer_confirmations() {
        let temp = TempDir::new().unwrap();
        let _cwd = CwdGuard::enter(temp.path());
        let _outer = ProjectScope::enter(InitContext::default());
        PROJECT.with(|scope| {
            scope
                .borrow_mut()
                .as_mut()
                .unwrap()
                .confirmed
                .push(PathBuf::from("/shared/AGENTS.md"))
        });

        let _inner = ProjectScope::enter(InitContext::default());

        let confirmed = PROJECT.with(|scope| scope.borrow().as_ref().unwrap().confirmed.clone());
        assert_eq!(confirmed, vec![PathBuf::from("/shared/AGENTS.md")]);
    }

    #[test]
    fn test_project_patch_of_a_hard_linked_file_is_confirmed() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(temp.path().join("shared-rules"), "team rules\n").unwrap();
        fs::hard_link(
            temp.path().join("shared-rules"),
            project.join(".clinerules"),
        )
        .unwrap();
        let _cwd = CwdGuard::enter(&project);

        let error = super::super::instructions_agents::run_cline_mode(skip()).unwrap_err();

        assert!(format!("{error:#}").contains("hard links"), "{error:#}");
        assert_eq!(
            fs::read_to_string(temp.path().join("shared-rules")).unwrap(),
            "team rules\n"
        );
    }

    #[test]
    fn test_nested_project_scope_hands_its_confirmations_back() {
        let temp = TempDir::new().unwrap();
        let _cwd = CwdGuard::enter(temp.path());
        let _outer = ProjectScope::enter(InitContext::default());
        {
            let _inner = ProjectScope::enter(InitContext::default());
            mark_confirmed(PathBuf::from("/shared/AGENTS.md"));
        }

        let confirmed = PROJECT.with(|scope| scope.borrow().as_ref().unwrap().confirmed.clone());
        assert_eq!(confirmed, vec![PathBuf::from("/shared/AGENTS.md")]);
    }

    #[test]
    fn test_project_file_outside_the_project_is_not_backed_up_into_it() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir(temp.path().join("private")).unwrap();
        fs::write(temp.path().join("private/notes.md"), "SECRET NOTES\n").unwrap();
        symlink("../private/notes.md", project.join("CLAUDE.md")).unwrap();
        let _cwd = CwdGuard::enter(&project);
        let _scope = ProjectScope::enter(InitContext {
            patch_mode: PatchMode::Auto,
            ..InitContext::default()
        });

        write_file(
            Path::new("CLAUDE.md"),
            WriteKind::Instructions,
            "SECRET NOTES\n@RTK.md\n",
        )
        .unwrap();

        assert!(!project.join("CLAUDE.md.bak").exists());
    }

    #[test]
    fn test_project_copilot_uninstall_cleans_instructions_when_the_hook_is_refused() {
        let (_temp, project, outside) = project_with_outside_link(".github/hooks/rtk-rewrite.json");
        let instructions = project.join(".github/copilot-instructions.md");
        fs::write(
            &instructions,
            format!("mine\n\n{RTK_BLOCK_START} -->\nrtk\n{RTK_BLOCK_END}\n"),
        )
        .unwrap();
        let _cwd = CwdGuard::enter(&project);

        let error = super::super::copilot::uninstall_copilot(skip()).unwrap_err();

        assert!(format!("{error:#}").contains("left in place"), "{error:#}");
        assert!(
            !fs::read_to_string(&instructions)
                .unwrap()
                .contains(RTK_BLOCK_START)
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), "VICTIM");
    }

    #[test]
    fn test_project_copilot_uninstall_dry_run_does_not_fail_on_a_refused_hook() {
        let (_temp, project, outside) = project_with_outside_link(".github/hooks/rtk-rewrite.json");
        let _cwd = CwdGuard::enter(&project);

        super::super::copilot::uninstall_copilot(InitContext {
            dry_run: true,
            ..skip()
        })
        .unwrap();

        assert_eq!(fs::read_to_string(outside).unwrap(), "VICTIM");
    }

    #[test]
    fn test_project_copilot_uninstall_with_no_hook_left_does_not_fail() {
        let (_temp, project, outside) = project_with_outside_link(".github/hooks/rtk-rewrite.json");
        fs::remove_file(&outside).unwrap();
        let _cwd = CwdGuard::enter(&project);

        super::super::copilot::uninstall_copilot(skip()).unwrap();
    }

    #[test]
    fn test_project_file_outside_that_needs_no_change_is_not_refused() {
        let (_temp, project, shared) = project_with_shared_rules();
        fs::write(&shared, "rtk rules already here\n").unwrap();
        let _cwd = CwdGuard::enter(&project);

        super::super::instructions_agents::run_cline_mode(skip()).unwrap();

        assert_eq!(
            fs::read_to_string(shared).unwrap(),
            "rtk rules already here\n"
        );
    }

    #[test]
    fn test_project_copilot_uninstall_is_not_blocked_by_linked_instructions_without_a_block() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(project.join(".github/hooks")).unwrap();
        fs::write(
            project.join(".github/hooks/rtk-rewrite.json"),
            super::super::COPILOT_HOOK_JSON,
        )
        .unwrap();
        fs::create_dir(temp.path().join("shared")).unwrap();
        fs::write(temp.path().join("shared/ci.md"), "team instructions\n").unwrap();
        symlink(
            "../../shared/ci.md",
            project.join(".github/copilot-instructions.md"),
        )
        .unwrap();
        let _cwd = CwdGuard::enter(&project);

        super::super::copilot::uninstall_copilot(skip()).unwrap();

        assert!(!project.join(".github/hooks/rtk-rewrite.json").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("shared/ci.md")).unwrap(),
            "team instructions\n"
        );
    }

    #[test]
    fn test_project_codex_rtk_md_linked_to_a_user_file_is_set_aside_not_refused() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir(temp.path().join("shared")).unwrap();
        fs::write(temp.path().join("shared/RTK.md"), "my notes\n").unwrap();
        symlink("../shared/RTK.md", project.join("RTK.md")).unwrap();
        let _cwd = CwdGuard::enter(&project);

        super::super::codex::run_codex_mode(false, skip()).unwrap();

        assert_eq!(
            fs::read_to_string(temp.path().join("shared/RTK.md")).unwrap(),
            "my notes\n"
        );
        assert!(
            !fs::symlink_metadata(project.join("RTK.md"))
                .unwrap()
                .is_symlink()
        );
    }

    #[test]
    fn test_project_hook_dangling_into_an_existing_outside_directory_is_refused() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(project.join(".trae")).unwrap();
        fs::create_dir(temp.path().join("outside")).unwrap();
        symlink("../../outside/hooks.json", project.join(".trae/hooks.json")).unwrap();
        let _cwd = CwdGuard::enter(&project);

        let error = super::super::trae::run_trae_mode(false, InitContext::default()).unwrap_err();

        assert!(
            format!("{error:#}").contains("outside the project"),
            "{error:#}"
        );
        assert!(!temp.path().join("outside/hooks.json").exists());
    }

    #[test]
    fn test_project_file_guard_does_nothing_outside_a_project_scope() {
        let (_temp, project, _outside) = project_with_outside_link(".trae/hooks.json");
        ensure_project_file_inside(&project.join(".trae/hooks.json"), "Trae")
            .expect("global mode follows the user's own links");
    }

    #[test]
    fn test_project_scope_is_restored_when_dropped() {
        let outer = ProjectScope::enter(InitContext::default());
        drop(ProjectScope::enter(InitContext {
            patch_mode: PatchMode::Auto,
            ..InitContext::default()
        }));
        assert_eq!(
            PROJECT.with(|scope| scope.borrow().as_ref().map(|p| p.mode)),
            Some(PatchMode::Ask)
        );
        drop(outer);
        assert!(!in_project_scope());
    }
}
