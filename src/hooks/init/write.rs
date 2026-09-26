//! Every init write goes through here: RTK's own files, the configs RTK edits and the
//! instruction files RTK adds to. [`plan_write`] classifies a write once -- what it lands on,
//! what refuses it, what must be confirmed, what happens to the backup -- and the write, the
//! check made before a consent prompt and the `--dry-run` preview all read that one plan, so
//! they cannot disagree.

use super::*;
use std::cell::RefCell;
use std::io::IsTerminal;

/// Who owns a file RTK writes, which decides how it is written.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum WriteKind {
    /// Written whole by RTK (`RTK.md`, a hook script, an extension): replaced atomically, with
    /// no backup.
    Owned,
    /// A config RTK edits but does not own (`settings.json`, a hooks config): backed up, then
    /// replaced atomically, because agents re-read it while they run.
    Config,
    /// An instruction file RTK adds to but does not own (`CLAUDE.md`, `AGENTS.md`, a rules
    /// file): backed up, then written in place, so it keeps its owner, group, ACLs and hard
    /// links.
    Instructions,
}

/// What happens to the backup of a file RTK edits.
enum Backup {
    /// Nothing is backed up: RTK's own file, a file that does not exist yet, a device, a file
    /// already backed up in this run, or an instruction file whose earlier backup is kept.
    None,
    /// The file is copied here first.
    Take(PathBuf),
    /// The file is written without a backup, for this reason, which the write reports.
    Skip(String),
}

/// A question asked before a project write.
enum Confirmation {
    /// The write lands outside the project.
    Outside {
        landing: PathBuf,
        root: PathBuf,
        mode: PatchMode,
    },
    /// The file is written in place and has other hard links.
    HardLinks { links: u64, mode: PatchMode },
}

/// Everything decided about one write before it happens.
struct WritePlan {
    target: PathBuf,
    exists: bool,
    special: bool,
    refusal: Option<String>,
    confirmations: Vec<Confirmation>,
    backup: Backup,
}

thread_local! {
    /// Backups taken during this run. A file patched twice in one run (a block removed, then a
    /// reference) is backed up once, so the backup holds the file as it was before RTK ran.
    static BACKED_UP: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Classify a write to `path`, with no side effect.
fn plan_write(path: &Path, kind: WriteKind) -> WritePlan {
    let (target, resolved) = resolve_write_target_with(path);
    let mut plan = WritePlan {
        exists: fs::metadata(&target).is_ok(),
        special: is_special_file(&target),
        target,
        refusal: None,
        confirmations: Vec::new(),
        backup: Backup::None,
    };
    if !resolved && let Some(reason) = unfollowable_link(path) {
        plan.refusal = Some(reason);
        return plan;
    }
    // A device or a FIFO (a file linked to /dev/null) is written through as a plain write
    // would, and never probed: opening one has effects.
    if plan.special {
        return plan;
    }
    if let Some(reason) = unwritable_reason(&plan.target) {
        plan.refusal = Some(reason.to_string());
        return plan;
    }
    if !plan.exists
        && let Some(reason) = blocked_directory(&plan.target)
    {
        plan.refusal = Some(reason);
        return plan;
    }
    let in_place = kind == WriteKind::Instructions && plan.exists;
    if !in_place && directory_refuses_new_file(directory_of(&plan.target)) {
        plan.refusal = Some("its directory does not accept a new file".to_string());
        return plan;
    }
    if let Some((landing, root, mode)) = outside_project_landing(&plan.target, resolved) {
        plan.confirmations.push(Confirmation::Outside {
            landing,
            root,
            mode,
        });
    }
    // One question per write: a file already confirmed for leaving the project is not asked
    // about again for its hard links.
    if in_place
        && plan.confirmations.is_empty()
        && let Some((links, mode)) = pending_hard_link_confirmation(&plan.target)
    {
        plan.confirmations
            .push(Confirmation::HardLinks { links, mode });
    }
    match plan_backup(path, &plan, kind) {
        Ok(backup) => plan.backup = backup,
        Err(reason) => plan.refusal = Some(reason),
    }
    plan
}

/// Why a symlink at `path` cannot be written through, when the resolver left it unfollowed.
/// Such a link is the user's, and replacing it with a regular file would silently undo it --
/// a dotfiles link into a volume that is not mounted, for example -- so the write is refused.
fn unfollowable_link(path: &Path) -> Option<String> {
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
        return None;
    }
    let Ok(link) = fs::read_link(path) else {
        return Some("it is a symlink RTK cannot read".to_string());
    };
    // Follow the chain to its last link to name why it ends nowhere.
    let mut current = path.to_path_buf();
    for _ in 0..=MAX_SYMLINK_HOPS {
        let Ok(next) = fs::read_link(&current) else {
            return Some("it is a symlink RTK cannot read".to_string());
        };
        let destination = directory_of(&current).join(next);
        if fs::symlink_metadata(&destination).is_ok_and(|metadata| metadata.is_symlink()) {
            current = destination;
            continue;
        }
        let reason = match blocked_directory(&destination) {
            Some(reason) => reason,
            None if fs::metadata(directory_of(&destination)).is_err() => {
                "whose directory does not exist".to_string()
            }
            None => "that cannot be followed".to_string(),
        };
        return Some(format!("it is a symlink to {}, {reason}", link.display()));
    }
    Some(format!(
        "it is a symlink to {} that cannot be followed (a loop, or too many links)",
        link.display()
    ))
}

/// Why a file cannot be created at `path` because of the directories above it: the nearest
/// one that exists is a dangling symlink or not a directory at all, so creating the missing
/// ones fails.
fn blocked_directory(path: &Path) -> Option<String> {
    let ancestor = path.ancestors().skip(1).find(|ancestor| {
        !ancestor.as_os_str().is_empty() && fs::symlink_metadata(ancestor).is_ok()
    })?;
    match fs::metadata(ancestor) {
        Err(_) => Some(format!(
            "its directory {} is a symlink to something that does not exist",
            ancestor.display()
        )),
        Ok(metadata) if !metadata.is_dir() => {
            Some(format!("{} is not a directory", ancestor.display()))
        }
        Ok(_) => None,
    }
}

fn plan_backup(path: &Path, plan: &WritePlan, kind: WriteKind) -> Result<Backup, String> {
    if kind == WriteKind::Owned || !plan.exists {
        return Ok(Backup::None);
    }
    let backup = backup_path_for(path);
    if already_backed_up(&backup) {
        return Ok(Backup::None);
    }
    if same_file(&backup, &plan.target) {
        // The backup is the file itself: copying it there backs nothing up. An instruction
        // file is written without one, as when no backup can be taken; a config is refused.
        let reason = format!("its backup {} is the file itself", backup.display());
        return if kind == WriteKind::Instructions {
            Ok(Backup::Skip(reason))
        } else {
            Err(reason)
        };
    }
    if resolves_outside_project(&plan.target) {
        // Its backup would land next to the link, inside the project: a copy of a file that is
        // not the project's.
        return Ok(Backup::Skip("it resolves outside the project".to_string()));
    }
    let existing = fs::symlink_metadata(&backup).ok();
    if kind == WriteKind::Instructions {
        // The first backup -- the file before RTK touched it -- or the user's own copy is kept
        // rather than replaced by a later version.
        return Ok(if existing.is_some() {
            Backup::None
        } else if directory_refuses_new_file(directory_of(&backup)) {
            Backup::Skip("its directory does not accept a new file".to_string())
        } else {
            Backup::Take(backup)
        });
    }
    if existing.as_ref().is_some_and(|metadata| metadata.is_dir()) {
        return Err(format!("its backup {} is a directory", backup.display()));
    }
    let regular_backup = existing.as_ref().is_some_and(|metadata| metadata.is_file());
    if regular_backup && let Some(reason) = unwritable_reason(&backup) {
        return Err(format!(
            "its backup {} cannot be written ({reason})",
            backup.display()
        ));
    }
    // A directory that takes no new file still lets an existing backup be rewritten in place --
    // unless it is the file itself, or shares its content with another file.
    if directory_refuses_new_file(directory_of(&backup))
        && (!regular_backup || has_other_links(&backup))
    {
        return Err(format!(
            "the directory of its backup {} does not accept a new file",
            backup.display()
        ));
    }
    Ok(Backup::Take(backup))
}

/// Whether rewriting `path` in place would also change another file sharing it.
fn has_other_links(path: &Path) -> bool {
    hard_link_count(path).is_some_and(|links| links > 1)
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

fn already_backed_up(backup: &Path) -> bool {
    BACKED_UP.with(|done| done.borrow().iter().any(|taken| taken == backup))
}

/// Carry out a write to `path`: refuse it, confirm it, back the file up, and write it, as
/// [`plan_write`] decided. Returns the backup taken.
pub(super) fn write_file(path: &Path, kind: WriteKind, content: &str) -> Result<Option<PathBuf>> {
    let plan = plan_write(path, kind);
    if let Some(reason) = &plan.refusal {
        anyhow::bail!("Cannot write {}: {reason}", path.display());
    }
    for confirmation in &plan.confirmations {
        confirm(path, &plan.target, confirmation)?;
    }
    let backup = match plan.backup {
        Backup::None => None,
        Backup::Skip(reason) => {
            eprintln!(
                "[warn] Not backing up {}: {reason}. Writing it without a backup.",
                path.display()
            );
            None
        }
        Backup::Take(backup) => {
            copy_backup(&plan.target, &backup).with_context(|| {
                format!(
                    "Failed to backup {} to {}",
                    path.display(),
                    backup.display()
                )
            })?;
            BACKED_UP.with(|done| done.borrow_mut().push(backup.clone()));
            Some(backup)
        }
    };
    let failure = || match &backup {
        Some(backup) => format!(
            "Failed to write {} (previous content saved to {})",
            path.display(),
            backup.display()
        ),
        None => format!("Failed to write {}", path.display()),
    };
    if plan.special || (kind == WriteKind::Instructions && plan.exists) {
        fs::write(&plan.target, content).with_context(failure)?;
    } else {
        write_owned_target(&plan.target, content).with_context(failure)?;
    }
    if !plan.exists {
        // Created in this run: a later patch of it must not back RTK's own write up over an
        // earlier backup.
        BACKED_UP.with(|done| done.borrow_mut().push(backup_path_for(path)));
    }
    Ok(backup)
}

fn confirm(path: &Path, target: &Path, confirmation: &Confirmation) -> Result<()> {
    match confirmation {
        Confirmation::Outside {
            landing,
            root,
            mode,
        } => {
            warn_outside_project(path, landing, root);
            confirm_project_write(
                path,
                &format!("it resolves to {}, outside the project", landing.display()),
                landing,
                *mode,
            )?;
            mark_confirmed(landing.clone());
        }
        Confirmation::HardLinks { links, mode } => {
            warn_hard_links(path, *links);
            confirm_project_write(path, &format!("it has {links} hard links"), target, *mode)?;
            mark_confirmed(target.to_path_buf());
        }
    }
    Ok(())
}

/// Refuse, before RTK asks whether to patch a config, what the patch would refuse anyway.
pub(super) fn ensure_patchable(path: &Path) -> Result<()> {
    refusal_as_error(path, WriteKind::Config)
}

/// Refuse, before RTK asks whether to replace one of its own files, what the write would
/// refuse anyway.
pub(super) fn ensure_writable(path: &Path) -> Result<()> {
    refusal_as_error(path, WriteKind::Owned)
}

fn refusal_as_error(path: &Path, kind: WriteKind) -> Result<()> {
    match plan_write(path, kind).refusal {
        Some(reason) => anyhow::bail!("Cannot write {}: {reason}", path.display()),
        None => Ok(()),
    }
}

/// Under `--dry-run`, say what the real run will do about a write of this `kind` to `path`
/// that it would refuse, ask about, or make without a backup. Returns whether the real run
/// would not write it.
fn preview(path: &Path, kind: WriteKind) -> bool {
    let plan = plan_write(path, kind);
    if let Some(reason) = &plan.refusal {
        println!("[dry-run] would refuse {}: {reason}", path.display());
        return true;
    }
    for confirmation in &plan.confirmations {
        let mode = match confirmation {
            Confirmation::Outside {
                landing,
                root,
                mode,
            } => {
                warn_outside_project(path, landing, root);
                println!(
                    "[dry-run] {}: {}",
                    dry_run_outcome(*mode),
                    landing.display()
                );
                // Previewed once, like the real run asks once.
                mark_confirmed(landing.clone());
                *mode
            }
            Confirmation::HardLinks { links, mode } => {
                warn_hard_links(path, *links);
                println!("[dry-run] {}: {}", dry_run_outcome(*mode), path.display());
                mark_confirmed(plan.target.clone());
                *mode
            }
        };
        match mode {
            PatchMode::Auto => {}
            // Asked in a terminal, the write may still happen.
            PatchMode::Ask if std::io::stdin().is_terminal() => return false,
            PatchMode::Ask | PatchMode::Skip => return true,
        }
    }
    if let Backup::Skip(reason) = &plan.backup {
        println!(
            "[dry-run] would write {} without a backup: {reason}",
            path.display()
        );
    }
    false
}

/// What a write says about itself: the line a dry run prints instead of writing, what a dry
/// run shows under `-v`, and the line printed once the write is done.
pub(crate) struct Report {
    would: String,
    detail: Detail,
    done: Option<(String, Shown)>,
}

/// What a dry run shows under `-v` besides its line.
pub(crate) enum Detail {
    None,
    Content,
    Text(String),
}

/// When the line printed after a write is shown.
pub(crate) enum Shown {
    /// On stdout, every time.
    Always,
    /// On stderr, under `-v`.
    Verbose,
}

impl Report {
    /// A write that a dry run reports with the `would` line, printed as given.
    pub(super) fn new(would: impl Into<String>) -> Self {
        Self {
            would: would.into(),
            detail: Detail::None,
            done: None,
        }
    }

    /// Under `-v`, a dry run also shows the content it would write.
    pub(super) fn with_content(mut self) -> Self {
        self.detail = Detail::Content;
        self
    }

    /// Under `-v`, a dry run also shows `text`.
    pub(super) fn with_detail(mut self, text: impl Into<String>) -> Self {
        self.detail = Detail::Text(text.into());
        self
    }

    /// Once written, print `line` on stdout.
    pub(super) fn done(mut self, line: impl Into<String>) -> Self {
        self.done = Some((line.into(), Shown::Always));
        self
    }

    /// Once written, print `line` on stderr under `-v`.
    pub(super) fn done_verbose(mut self, line: impl Into<String>) -> Self {
        self.done = Some((line.into(), Shown::Verbose));
        self
    }
}

/// Write `content` to `path` as `kind` and report it -- or, under `--dry-run`, say what the
/// real run would do instead: its refusal or question if it has one, otherwise the report's
/// line. Under `-v` the backup taken is named. Returns the backup taken.
pub(super) fn write_reported(
    path: &Path,
    kind: WriteKind,
    content: &str,
    ctx: InitContext,
    report: Report,
) -> Result<Option<PathBuf>> {
    let verbose = ctx.verbose > 0;
    if ctx.dry_run {
        if !preview(path, kind) {
            println!("{}", report.would);
            match report.detail {
                Detail::Content if verbose => println!("[dry-run] content:\n{content}"),
                Detail::Text(text) if verbose => println!("{text}"),
                _ => {}
            }
        }
        return Ok(None);
    }
    let backup = write_file(path, kind, content)?;
    if verbose && let Some(backup) = &backup {
        eprintln!("Backup: {}", backup.display());
    }
    match report.done {
        Some((line, Shown::Always)) => println!("{line}"),
        Some((line, Shown::Verbose)) if verbose => eprintln!("{line}"),
        _ => {}
    }
    Ok(backup)
}

/// The write half of an owned or config write, for a target the plan already vouched for.
fn write_owned_target(target: &Path, content: &str) -> Result<()> {
    let dir = directory_of(target);
    fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    replace_through_temp_file(target, target, |file| file.write_all(content.as_bytes()))
}

/// Whether an existing path is something other than a regular file or a directory.
pub(super) fn is_special_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| !metadata.is_file() && !metadata.is_dir())
}

/// The directory a file sits in; a bare file name sits in the current directory.
pub(super) fn directory_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// The backup's directory takes no new file, though the file itself may still be writable.
#[derive(Debug)]
struct DirectoryRefusesNewFile;

impl std::fmt::Display for DirectoryRefusesNewFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the directory does not accept a new file")
    }
}

/// Replace `destination` with what `fill` writes, through a temp file in the same directory
/// renamed over it. The rename replaces whatever sits at `destination` -- a symlink, a hard
/// link, a FIFO -- instead of opening it, and an earlier file survives a write that fails.
/// The temp file takes `mode_from`'s permission bits (see [`temp_file_for`]).
fn replace_through_temp_file(
    destination: &Path,
    mode_from: &Path,
    fill: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> Result<()> {
    let dir = directory_of(destination);
    let mut temp_file = temp_file_for(mode_from, dir).map_err(|error| {
        let refused = error.kind() == std::io::ErrorKind::PermissionDenied;
        let error = anyhow::Error::new(error)
            .context(format!("Failed to create temp file in {}", dir.display()));
        if refused {
            error.context(DirectoryRefusesNewFile)
        } else {
            error
        }
    })?;
    fill(temp_file.as_file_mut())
        .with_context(|| format!("Failed to write temp file in {}", dir.display()))?;
    temp_file
        .persist(destination)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to atomically replace {}", destination.display()))?;
    Ok(())
}

/// Copy `source` to `destination` without opening what sits at `destination`, so a planted
/// `.bak` cannot redirect the copy out of the project or into another file (#4157). The copy
/// keeps the source's permission bits from the moment it is created.
fn copy_backup(source: &Path, destination: &Path) -> Result<()> {
    let mut source_file =
        fs::File::open(source).with_context(|| format!("Failed to read {}", source.display()))?;
    let replaced = replace_through_temp_file(destination, source, |file| {
        std::io::copy(&mut source_file, file).map(|_| ())
    });
    match replaced {
        // A directory that takes no new file still lets an existing backup be rewritten in
        // place; the plan made sure it is not the file itself.
        Err(error)
            if error.downcast_ref::<DirectoryRefusesNewFile>().is_some()
                && fs::symlink_metadata(destination).is_ok_and(|metadata| metadata.is_file()) =>
        {
            copy_into_existing_backup(source, destination)
        }
        result => result,
    }
}

/// Rewrite an existing regular `.bak` with `source`'s content, never following a link swapped
/// in at its path.
fn copy_into_existing_backup(source: &Path, destination: &Path) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut backup = options
        .open(destination)
        .with_context(|| format!("Failed to open {}", destination.display()))?;
    let mut source_file =
        fs::File::open(source).with_context(|| format!("Failed to read {}", source.display()))?;
    backup
        .set_len(0)
        .with_context(|| format!("Failed to truncate {}", destination.display()))?;
    std::io::copy(&mut source_file, &mut backup)
        .with_context(|| format!("Failed to write {}", destination.display()))?;
    Ok(())
}

/// Whether a new file cannot be created in `dir`: in `dir` itself when it exists, otherwise
/// in the nearest ancestor that does, where its missing directories would be created. Asked
/// without creating anything.
pub(super) fn directory_refuses_new_file(dir: &Path) -> bool {
    let existing = dir
        .ancestors()
        .find(|ancestor| !ancestor.as_os_str().is_empty() && fs::metadata(ancestor).is_ok());
    existing.is_some_and(|ancestor| {
        fs::metadata(ancestor).is_ok_and(|metadata| metadata.is_dir())
            && unwritable_directory(ancestor)
    })
}

#[cfg(unix)]
fn unwritable_directory(dir: &Path) -> bool {
    access_denied(dir, libc::W_OK | libc::X_OK)
}

/// Windows ignores a directory's read-only attribute when a file is created in it.
#[cfg(not(unix))]
fn unwritable_directory(_dir: &Path) -> bool {
    false
}

/// Whether the kernel denies `mode` access to `path`, asked without opening it, so a dry run
/// does not wake the watchers of a file it only previews.
#[cfg(unix)]
fn access_denied(path: &Path, mode: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a NUL-terminated string that outlives the call, which only reads
    // it; `faccessat` has no other preconditions. No flags: rtk is never setuid, so the real
    // ids it checks are the effective ones.
    // nosemgrep: unsafe-block
    #[allow(unsafe_code)]
    let status = unsafe { libc::faccessat(libc::AT_FDCWD, c_path.as_ptr(), mode, 0) };
    status != 0
        && matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EACCES | libc::EROFS | libc::EPERM)
        )
}

/// Why a plain write to an existing `path` would fail, if it would. A missing file is
/// writable: the write creates it.
#[cfg(unix)]
pub(super) fn unwritable_reason(path: &Path) -> Option<&'static str> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.is_dir() {
        return Some("it is a directory");
    }
    access_denied(path, libc::W_OK).then_some("the file is read-only")
}

#[cfg(not(unix))]
pub(super) fn unwritable_reason(path: &Path) -> Option<&'static str> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.is_dir() {
        Some("it is a directory")
    } else if metadata.permissions().readonly() {
        Some("the file is read-only")
    } else {
        None
    }
}

/// A temp file in `dir` that, once renamed over `target`, keeps `target`'s permission bits,
/// or gets 0o666 less the umask when `target` does not exist yet. A bare `NamedTempFile` is
/// 0o600, which would narrow every file it replaces. The file is created with the final
/// bits, so it is never readable more widely than the file it replaces, and the umask can
/// only narrow them; the fchmod restores what the umask took. Setuid, setgid and sticky
/// bits are not carried over.
#[cfg(unix)]
fn temp_file_for(target: &Path, dir: &Path) -> std::io::Result<NamedTempFile> {
    use std::os::unix::fs::PermissionsExt;
    let existing = fs::metadata(target)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o777);
    let temp_file = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(existing.unwrap_or(0o666)))
        .tempfile_in(dir)?;
    if let Some(mode) = existing {
        temp_file
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(temp_file)
}

#[cfg(not(unix))]
fn temp_file_for(_target: &Path, dir: &Path) -> std::io::Result<NamedTempFile> {
    NamedTempFile::new_in(dir)
}
