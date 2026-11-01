//! SQLite plumbing shared by rtk's databases (tracking, recall): the private
//! open, owner-only permissions on the database and its sidecars, and the
//! versioned migration engine. Each database keeps its own versions enum, its
//! own steps and its own snapshot of the latest schema; this module applies
//! them: the snapshot to a new database, the steps to an existing one.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};

/// A database's schema versions, stored in the SQLite `user_version` pragma.
/// Implemented by each database's versions enum, whose variants are what
/// `Debug` prints in the errors naming a migration step.
pub(crate) trait Version: Copy + std::fmt::Debug {
    fn number(self) -> i64;
}

pub(crate) type Migration = fn(&Connection) -> Result<()>;

/// The rtk databases. A kind's number goes in the low byte of the file's
/// `application_id`, so the databases refuse each other's files instead of
/// reading the other's `user_version` as their own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DbKind {
    Tracking = 1,
    Recall = 2,
    // New databases go directly above this line.
    Next,
}

struct KindInfo {
    kind: DbKind,
    /// Names the database in errors.
    label: &'static str,
    /// Where the user points this database elsewhere.
    path_settings: &'static str,
}

/// One entry per `DbKind`, in order.
const KINDS: &[KindInfo] = &[
    KindInfo {
        kind: DbKind::Tracking,
        label: "tracking",
        path_settings: "RTK_DB_PATH or [tracking] database_path",
    },
    KindInfo {
        kind: DbKind::Recall,
        label: "recall",
        path_settings: "RTK_RECALL_DB or [retriever] database_path",
    },
];

const _: () = {
    let mut i = 0;
    while i < KINDS.len() {
        assert!(
            KINDS[i].kind as usize == i + 1,
            "KINDS must list every DbKind once, in order"
        );
        i += 1;
    }
    assert!(KINDS.len() == DbKind::Next as usize - 1);
    assert!(DbKind::Next as usize <= 256, "a kind is one byte");
};

/// "RTK" in the upper three bytes of an `application_id`.
const APPLICATION_ID_PREFIX: i32 = 0x5254_4B00;

impl DbKind {
    /// The `application_id` written to this database's files.
    pub(crate) const fn application_id(self) -> i32 {
        APPLICATION_ID_PREFIX | self as i32
    }

    /// The kind whose files carry `application_id`, if this build knows it.
    fn from_application_id(application_id: i32) -> Option<Self> {
        KINDS
            .iter()
            .map(|info| info.kind)
            .find(|kind| kind.application_id() == application_id)
    }

    /// Whether `application_id` is one an rtk database writes, of a kind this
    /// build may not know: a newer build's.
    fn is_rtk_id(application_id: i32) -> bool {
        application_id & !0xFF == APPLICATION_ID_PREFIX && application_id & 0xFF != 0
    }

    // `KINDS` lists every kind in order, so a kind's entry is at its number.
    fn info(self) -> &'static KindInfo {
        &KINDS[self as usize - 1]
    }
}

/// What identifies one of rtk's databases and how it migrates.
pub(crate) struct Schema<'a, V> {
    pub kind: DbKind,
    /// Whether a file with no `application_id` and a nonzero `user_version` is
    /// this database's. True for a database whose releases never tagged their
    /// files: another database's untagged file can only carry a zero stamp.
    pub owns_untagged_stamps: bool,
    /// The SQL that creates the database at the schema its last step produces:
    /// every table and index, no `IF NOT EXISTS`, no transaction control. A
    /// database with no schema object at all is created from it in one
    /// transaction instead of replaying the steps. Its columns come in the order
    /// the steps leave them, since `ALTER TABLE ... ADD COLUMN` appends.
    ///
    /// A schema change updates its step and this snapshot together. The
    /// snapshot-equivalence test of each database builds one database from the
    /// snapshot and one through every step, and fails, naming what differs, when
    /// the two disagree.
    pub snapshot: &'a str,
    /// One step per version, in order. A new schema change is a new step (and an
    /// update of [`Schema::snapshot`]), and a released step never changes the
    /// schema it produces: a gated open never runs it again on a database stamped
    /// at its version. Every step must stay idempotent: `rtk init` replays all of
    /// them, in order, on a database at any version, including one stamped by a
    /// newer build.
    pub steps: &'a [(V, Migration)],
}

impl<V> Schema<'_, V> {
    /// Names the database in errors.
    pub(crate) fn label(&self) -> &'static str {
        self.kind.info().label
    }

    /// Why this database may lack part of its schema, with the [`Opened::behind`]
    /// cause.
    pub(crate) fn not_upgraded(&self, why: &str) -> String {
        format!("{} database was not upgraded: {why}", self.label())
    }

    /// What to tell the user about `what` a database at its stamp lacks. The steps
    /// the stamp covers do not run again on a normal open, only under `rtk init`.
    pub(crate) fn repair_hint(&self, what: &str) -> String {
        format!(
            "rtk: {} database looks corrupted ({what}). Run `rtk init` to recreate it.",
            self.label()
        )
    }

    /// On a database that is `behind`, an error from a statement that needs what it
    /// lacks becomes SQLite's message with [`Self::not_upgraded`]. Any other error is
    /// returned as it is.
    pub(crate) fn explain_missing(
        &self,
        err: anyhow::Error,
        behind: Option<&str>,
    ) -> anyhow::Error {
        match (behind, missing_object(&err)) {
            (Some(why), Some(msg)) => {
                NotUpgraded(format!("{msg} ({})", self.not_upgraded(why))).into()
            }
            _ => err,
        }
    }
}

/// A statement failed on what a database that is behind lacks, and says so.
#[derive(Debug)]
struct NotUpgraded(String);

impl std::fmt::Display for NotUpgraded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotUpgraded {}

/// Whether a statement failed on what a failed step would have created.
pub(crate) fn lacks_what_was_not_upgraded(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| cause.is::<NotUpgraded>())
}

/// Whether a write failed for a reason that holds for every write on this database:
/// it lacks what a failed step would have created, or it cannot be written at all.
pub(crate) fn every_write_would_fail(err: &anyhow::Error) -> bool {
    lacks_what_was_not_upgraded(err) || sqlite_reported(err, rusqlite::ErrorCode::ReadOnly)
}

/// Whether SQLite gave `code` for a failure in `err`'s chain.
fn sqlite_reported(err: &anyhow::Error, code: rusqlite::ErrorCode) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .and_then(rusqlite::Error::sqlite_error_code)
            == Some(code)
    })
}

/// Whether to gate schema migrations behind `user_version` (the hot-path
/// default) or always re-run them (used by `rtk init`, which isn't a hot path
/// and wants to self-heal a dropped/corrupted table — see
/// `tracking::ensure_schema_fresh`).
pub(crate) enum MigrationMode {
    GatedByVersion,
    Always,
}

/// A database as [`open_and_migrate`] or [`open_existing_and_migrate`] left it.
#[derive(Debug)]
pub(crate) struct Opened {
    pub conn: Connection,
    /// Why a step failed, when one did: it was rolled back, the stamp stays below
    /// it for the next open to try again, and the database is used as it is.
    pub behind: Option<String>,
}

/// Open (creating if needed) the `schema` database at `path`, apply pragmas, and
/// run its steps per `mode`. A step that fails does not fail a gated open: see
/// [`Opened::behind`]. Refusing a file that is not this database's stays an error,
/// and so does a failed step in `Always` mode (`rtk init`).
pub(crate) fn open_and_migrate<V: Version>(
    path: &Path,
    schema: &Schema<V>,
    mode: MigrationMode,
) -> Result<Opened> {
    let label = schema.label();
    // For a brand-new DB, create its directory and pre-create the file ourselves
    // via `open_private` so SQLite derives the -wal/-shm modes from an
    // already-private DB instead of the umask: that file is ours by
    // construction. An already-existing DB skips that pre-create's own redundant
    // open()/close() pair (a second file open on top of the `Connection::open`
    // right after it), and neither it nor its directory is touched until it has
    // been read and found to be ours, unless it could not be read without that:
    // see `unlock` and `claim`. The tracking DB's open is on
    // the hot path for every `rtk <cmd>` and, since rtk-ai/rtk#3206's
    // hook_decisions ground-truth logging, every single PreToolUse hook call (not
    // just RTK-covered ones), so avoiding that redundant open on the
    // already-set-up common case matters for the <10ms budget.
    let (conn, identity) = recovering(
        path,
        schema,
        || open_and_read(path, label),
        on_read_only_filesystem,
    )?;
    claim(conn, identity, path, schema, mode, OpenFlags::default())
}

/// Creates the file when there is none, opens it and reads its tag and stamp.
fn open_and_read(path: &Path, label: &str) -> Result<(Connection, (i32, i64))> {
    pre_create(path, label)?;
    let conn = open_db(path, OpenFlags::default(), label)?;
    let identity = read_identity(&conn, label)?;
    Ok((conn, identity))
}

/// Runs `attempt`. When it fails, the file is first read without its sidecars or
/// a writable directory, when that is possible: a file that is not a database or
/// not this one is refused as it is. Otherwise the owner gets back what they cannot
/// use (see [`unlock`]) and `attempt` runs again. What still fails on a read-only
/// filesystem, which nothing writes to, is served by that read unless a `-wal` or a
/// hot `-journal` holds what the read would miss: a WAL-mode file there cannot create
/// its `-shm`.
fn recovering<T: From<(Connection, (i32, i64))>, V>(
    path: &Path,
    schema: &Schema<V>,
    attempt: impl Fn() -> Result<T>,
    read_only: impl FnOnce(&Path) -> bool,
) -> Result<T> {
    let label = schema.label();
    attempt().or_else(|failure| {
        let mut peeked = peek(path, schema)?;
        let unlocked = unlock(path, label)?;
        if peeked.is_none() && unlocked.any() {
            // Only now can the file be read: one that is refused gets its modes back.
            peeked = peek(path, schema).inspect_err(|_| unlocked.restore(path))?;
        }
        let retried = if unlocked.any() {
            attempt()
        } else {
            Err(failure)
        };
        retried.or_else(|failure| match peeked {
            Some(read) if read_only(path) && !log_holds_data(path) => Ok(read.into()),
            _ => Err(failure),
        })
    })
}

/// The file read without its sidecars, or `None` when it cannot be read that way.
/// A file that is not a database or not this one is refused.
fn peek<V>(path: &Path, schema: &Schema<V>) -> Result<Option<(Connection, (i32, i64))>> {
    match open_immutable(path, schema.label()) {
        Ok((conn, (tag, stamp))) => {
            check_owner(&conn, schema, tag, stamp)?;
            Ok(Some((conn, (tag, stamp))))
        }
        Err(e) if sqlite_reported(&e, rusqlite::ErrorCode::NotADatabase) => Err(e),
        Err(_) => Ok(None),
    }
}

/// A read-only connection that asks SQLite to trust the file not to change: it takes
/// no lock and needs no `-shm`, and reads the file without any `-wal`. The tag and the
/// stamp are read with it.
fn open_immutable(path: &Path, label: &str) -> Result<(Connection, (i32, i64))> {
    let conn = open_db(
        Path::new(&immutable_uri(path)),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        label,
    )?;
    let identity = read_identity(&conn, label)?;
    Ok((conn, identity))
}

/// The `file:` URI of `path` with `immutable=1`.
fn immutable_uri(path: &Path) -> String {
    let mut slashed = path.as_os_str().as_encoded_bytes().to_vec();
    if cfg!(windows) {
        slashed
            .iter_mut()
            .filter(|byte| **byte == b'\\')
            .for_each(|byte| *byte = b'/');
        // A drive letter is an absolute path, which a URI spells with a leading slash.
        if slashed.get(1) == Some(&b':') {
            slashed.insert(0, b'/');
        }
    }
    // A path that starts with two slashes needs the empty authority: SQLite takes what
    // follows `file://` for a host.
    let mut uri = String::from(if slashed.starts_with(b"//") {
        "file://"
    } else {
        "file:"
    });
    for byte in slashed {
        match byte {
            b'/' | b':' | b'-' | b'_' | b'.' | b'~' => uri.push(byte as char),
            _ if byte.is_ascii_alphanumeric() => uri.push(byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri.push_str("?immutable=1");
    uri
}

/// Whether `path` is on a filesystem that refuses writes to everyone. A file only this
/// process cannot write may be changing, which an immutable read must not meet.
fn on_read_only_filesystem(path: &Path) -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .is_err_and(|e| e.kind() == std::io::ErrorKind::ReadOnlyFilesystem)
}

/// Whether the `-wal` or the rollback `-journal` next to the file holds pages an
/// immutable read would miss.
fn log_holds_data(db_path: &Path) -> bool {
    let resolved = std::fs::canonicalize(db_path).unwrap_or_else(|_| db_path.to_path_buf());
    ["-wal", "-journal"]
        .iter()
        .any(|suffix| std::fs::metadata(sibling(&resolved, suffix)).is_ok_and(|m| m.len() > 0))
}

/// The modes [`unlock`] replaced, for a file that turns out not to be ours.
#[derive(Default)]
struct Unlocked {
    dir: Option<std::fs::Permissions>,
    file: Option<std::fs::Permissions>,
}

impl Unlocked {
    fn any(&self) -> bool {
        self.dir.is_some() || self.file.is_some()
    }

    /// The file goes first: its directory may not let it be reached afterwards.
    fn restore(&self, path: &Path) {
        if let Some(mode) = &self.file {
            let _ = std::fs::set_permissions(path, mode.clone());
        }
        if let (Some(mode), Some(parent)) = (&self.dir, path.parent()) {
            let _ = std::fs::set_permissions(parent, mode.clone());
        }
    }
}

/// After a failed create, open or first read, gives the owner back a directory or
/// a file they cannot use (a restored backup, a `chmod`), and says what it replaced.
/// The failure may have another cause. A directory or a file its owner can use is
/// left alone.
fn unlock(path: &Path, label: &str) -> Result<Unlocked> {
    let mut unlocked = Unlocked::default();
    if let Some(meta) = path
        .parent()
        .and_then(|parent| std::fs::metadata(parent).ok())
        && meta.is_dir()
        && !owner_can(&meta, 0o700)
    {
        private_dir(path, label)?;
        unlocked.dir = Some(meta.permissions());
    }
    if let Ok(meta) = std::fs::metadata(path)
        && meta.is_file()
        && !owner_can(&meta, 0o600)
    {
        crate::core::utils::restrict_file(path);
        unlocked.file = Some(meta.permissions());
    }
    Ok(unlocked)
}

#[cfg(unix)]
fn owner_can(meta: &std::fs::Metadata, needed: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & needed == needed
}

#[cfg(not(unix))]
fn owner_can(_meta: &std::fs::Metadata, _needed: u32) -> bool {
    true
}

/// Opens `path` with `flags`. rusqlite gives every connection a 5000 ms busy
/// timeout when it opens it, so the first read waits for a lock another
/// connection holds (a checkpoint, a switch to WAL).
fn open_db(path: &Path, flags: OpenFlags, label: &str) -> Result<Connection> {
    Connection::open_with_flags(path, flags)
        .with_context(|| format!("Failed to open {label} DB: {}", path.display()))
}

/// Creates the file private (`0600`) when there is none.
/// The creation is exclusive, so a file that appears in the meantime is not
/// ours to chmod: it is left to [`claim`], which reads it first, and so is its
/// directory. The directory is prepared only when the file's path was not found.
fn pre_create(path: &Path, label: &str) -> Result<()> {
    let attempt = || match crate::core::utils::open_private(
        std::fs::OpenOptions::new().write(true).create_new(true),
        path,
    ) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        opened => opened.map(drop),
    };
    match attempt() {
        // The directory is missing: prepare it, which says why when it cannot be,
        // and try again. Any other failure is reported as it is.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            private_dir(path, label)?;
            attempt()
        }
        created => created,
    }
    .with_context(|| {
        format!(
            "Failed to pre-create private {label} DB file: {}",
            path.display()
        )
    })
}

/// The directory of `path`, created if needed and owner-only. It is cheap when
/// the directory is already right (its chmod is a no-op past the first call, see
/// `utils::set_owner_only`).
fn private_dir(path: &Path, label: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        crate::core::utils::create_private_dir(parent).with_context(|| {
            format!(
                "Failed to create the {label} DB directory: {}",
                parent.display()
            )
        })?;
    }
    Ok(())
}

/// Like [`open_and_migrate`] for a database that is already there: `None` when
/// it is not, and nothing is created. The file is opened without
/// `SQLITE_OPEN_CREATE`, so one that vanishes first is not recreated, and its
/// directory is left alone unless its owner cannot use it.
pub(crate) fn open_existing_and_migrate<V: Version>(
    path: &Path,
    schema: &Schema<V>,
    mode: MigrationMode,
) -> Result<Option<Opened>> {
    let label = schema.label();
    let flags = OpenFlags::default() - OpenFlags::SQLITE_OPEN_CREATE;
    let Some((conn, identity)) = recovering(
        path,
        schema,
        || open_existing_and_read(path, flags, label),
        on_read_only_filesystem,
    )?
    else {
        return Ok(None);
    };
    claim(conn, identity, path, schema, mode, flags).map(Some)
}

/// Opens the file if it is there and reads its tag and stamp.
fn open_existing_and_read(
    path: &Path,
    flags: OpenFlags,
    label: &str,
) -> Result<Option<(Connection, (i32, i64))>> {
    let conn = match open_db(path, flags, label) {
        Ok(conn) => conn,
        // A file that vanished first, or that cannot be seen at all, is not there.
        Err(_) if !path.exists() => return Ok(None),
        Err(e) => return Err(e),
    };
    let identity = read_identity(&conn, label)?;
    Ok(Some((conn, identity)))
}

/// Refuses the file unless the tag and stamp read from it say it is this
/// database's, before rtk changes anything on it: no pragma write, no chmod, no
/// directory repair. A file whose tag is foreign when it is opened is left as it
/// was in content, journal mode, permissions and directory; reading a WAL-mode file may
/// checkpoint it and use its `-shm`, as any SQLite reader does. A file another
/// process tags between this read and a step's lock is refused under that lock,
/// after the pragmas and permissions here have been applied. For a file that is
/// ours, repairs the owner-only permissions of its directory (when `flags` may
/// create it) and of the file *before* any write to it (external tampering, a
/// restored backup), sets WAL, and migrates. `flags` are the ones `conn` was opened
/// with.
fn claim<V: Version>(
    mut conn: Connection,
    (tag, stamp): (i32, i64),
    path: &Path,
    schema: &Schema<V>,
    mode: MigrationMode,
    flags: OpenFlags,
) -> Result<Opened> {
    let label = schema.label();
    check_owner(&conn, schema, tag, stamp)?;
    if flags.contains(OpenFlags::SQLITE_OPEN_CREATE) {
        // The directory is ours to tighten once the file has been found to be ours,
        // whether this open created it or this read confirms it.
        private_dir(path, label)?;
    } else {
        // A rollback-journal file reads fine in a directory it cannot write to.
        unlock(path, label)?;
    }
    crate::core::utils::restrict_file(path);
    // A file without owner write (a backup restored as `0444`) opens read-only,
    // and a connection stays read-only after the file is made writable, so it
    // is opened again once the chmod has made the file ours to write. A file the
    // chmod could not make writable (another owner's, a read-only mount) keeps
    // its connection: opening it again every time would only cost. It is the
    // file this open has just read and chmodded: its tag is not read again. The
    // `-wal`/`-shm` the read-only connection created copy the file's mode, so
    // they are made writable too.
    let readonly = conn
        .is_readonly(rusqlite::DatabaseName::Main)
        .unwrap_or(false);
    if should_reopen(readonly, || writable(path)) {
        drop(conn);
        restrict_sidecars(path);
        conn = open_db(path, flags, label)?;
    }
    // WAL for concurrent access (multiple Claude Code instances). It is refused
    // on some filesystems (NFS, read-only), which stays non-fatal. It is cheap on
    // every open: WAL persists in the DB file.
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
    // A loose `-wal`/`-shm` (a restored backup) is repaired before a step writes
    // history into it. What SQLite creates from here on copies the file's mode.
    restrict_sidecars(path);
    let gated = matches!(mode, MigrationMode::GatedByVersion);
    match run_steps(&conn, schema, mode, stamp) {
        Ok(()) => Ok(Opened { conn, behind: None }),
        Err(e) if gated => {
            // The steps refuse a file tagged since it was first read: that stays fatal.
            if let Ok((tag, stamp)) = read_identity(&conn, label) {
                check_owner(&conn, schema, tag, stamp)?;
            }
            Ok(Opened {
                conn,
                behind: Some(cause(&e)),
            })
        }
        Err(e) => Err(e),
    }
}

/// What SQLite said, or the innermost cause of a failure that did not come from it.
fn cause(err: &anyhow::Error) -> String {
    err.chain()
        .find(|cause| cause.is::<rusqlite::Error>())
        .unwrap_or_else(|| err.root_cause())
        .to_string()
}

/// Whether `msg` is SQLite's wording for a table or column the database lacks:
/// "no such table/column" for a SELECT or DELETE, "has no column named" for an
/// INSERT's column list.
fn names_missing_object(msg: &str) -> bool {
    msg.contains("no such table")
        || msg.contains("no such column")
        || msg.contains("has no column named")
}

/// The message of [`missing_object`] when what is missing is a table, which `rtk init`
/// brings back. An error that [`Schema::explain_missing`] explained has no such cause.
pub(crate) fn missing_table(err: &anyhow::Error) -> Option<&str> {
    missing_object(err).filter(|msg| msg.contains("no such table"))
}

/// SQLite's message for the first cause in `err`'s chain where a statement failed
/// on a table or column the database lacks.
pub(crate) fn missing_object(err: &anyhow::Error) -> Option<&str> {
    err.chain().find_map(|cause| match cause.downcast_ref()? {
        rusqlite::Error::SqliteFailure(_, Some(msg))
        | rusqlite::Error::SqlInputError { msg, .. }
            if names_missing_object(msg) =>
        {
            Some(msg.as_str())
        }
        _ => None,
    })
}

/// A read-only connection is worth replacing only when the file can be written.
fn should_reopen(connection_readonly: bool, file_writable: impl FnOnce() -> bool) -> bool {
    connection_readonly && file_writable()
}

/// Whether this process can write to `path`, found by opening it for writing
/// (nothing is created, truncated or written). The mode bits cannot say: they do
/// not know whose file it is, a read-only mount, an ACL or a Windows read-only
/// attribute. It costs one `open`, and only for a connection that came up
/// read-only.
fn writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

/// SQLite puts the sidecars next to the file a symlink resolves to, not next to
/// the link.
fn restrict_sidecars(db_path: &Path) {
    let resolved = std::fs::canonicalize(db_path).unwrap_or_else(|_| db_path.to_path_buf());
    for sidecar in db_sidecars(&resolved) {
        crate::core::utils::restrict_file(&sidecar);
    }
}

pub(crate) fn db_sidecars(db_path: &Path) -> Vec<PathBuf> {
    ["-wal", "-shm"]
        .iter()
        .map(|suffix| sibling(db_path, suffix))
        .collect()
}

/// SQLite appends `-wal`/`-shm`/`-journal` to the whole filename, so these are
/// siblings rather than extension swaps. Concatenate on `OsString`, not
/// `PathBuf::push`, which would append a component and silently target
/// `history.db/-wal`.
fn sibling(db_path: &Path, suffix: &str) -> PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Reads the tag and stamp of a connection the caller opened itself, refuses
/// it unless it is this database's, and runs the steps: what `claim` does for a
/// file it opens.
pub(crate) fn apply_migrations<V: Version>(
    conn: &Connection,
    schema: &Schema<V>,
    mode: MigrationMode,
) -> Result<()> {
    let (tag, stamp) = read_identity(conn, schema.label())?;
    check_owner(conn, schema, tag, stamp)?;
    run_steps(conn, schema, mode, stamp)
}

/// Brings the database to the schema's last version: from its snapshot when the
/// file holds no schema object yet, otherwise by [`apply_steps`].
///
/// The snapshot is tried here, below the callers that read the file's identity,
/// so every way in (`claim` for a file this process opens, and a connection a
/// caller opens itself) gets it, and the migration rules stay in one place. It
/// is tried only when the stamp the caller read is 0: any other stamp means the
/// file has a history, which the steps continue. The decision is made again
/// under the lock, so two processes creating the same file at once create it
/// once, and the second finds it current.
///
/// `seen` is the file's `user_version`, read and checked by the caller.
fn run_steps<V: Version>(
    conn: &Connection,
    schema: &Schema<V>,
    mode: MigrationMode,
    seen: i64,
) -> Result<()> {
    if seen == 0 && create_from_snapshot(conn, schema)? {
        return Ok(());
    }
    apply_steps(conn, schema, mode, seen)
}

/// Creates the database from the schema's snapshot, stamped at the last step's
/// version and tagged, in one `IMMEDIATE` transaction, and says whether it did.
/// It does only when, under the lock, the stamp is 0 and `sqlite_master` is empty:
/// a legacy unversioned or partial database, or one another process has since
/// created, is left to the steps.
fn create_from_snapshot<V: Version>(conn: &Connection, schema: &Schema<V>) -> Result<bool> {
    let label = schema.label();
    let Some(&(latest, _)) = schema.steps.last() else {
        return Ok(false);
    };
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .with_context(|| format!("Failed to start creating the {label} DB"))?;
    let (tag, stamp) = read_identity(&tx, label)?;
    check_owner(&tx, schema, tag, stamp)?;
    let has_schema_objects: bool = tx
        .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master)", [], |row| {
            row.get(0)
        })
        .with_context(|| format!("Failed to inspect the {label} DB schema"))?;
    if stamp != 0 || has_schema_objects {
        tx.rollback()
            .with_context(|| format!("Failed to release the {label} DB lock"))?;
        return Ok(false);
    }
    tx.execute_batch(schema.snapshot)
        .with_context(|| format!("Failed to create the {label} DB from its snapshot"))?;
    // As for a step: a transaction SQLite rolled back on its own must not be stamped.
    if tx.is_autocommit() {
        anyhow::bail!("Creating the {label} DB was rolled back by SQLite");
    }
    tx.pragma_update(None, "user_version", latest.number())
        .with_context(|| format!("Failed to stamp the {label} DB at {latest:?}"))?;
    if tag != schema.kind.application_id() {
        tx.pragma_update(None, "application_id", schema.kind.application_id())
            .with_context(|| format!("Failed to tag the {label} DB"))?;
    }
    tx.commit()
        .with_context(|| format!("Failed to commit the creation of the {label} DB"))?;
    Ok(true)
}

/// Apply the schema's steps in order, each in its own `IMMEDIATE` transaction
/// that reads `user_version` and raises it to the step's version, so a failed
/// step leaves the database at the previous version and the next open resumes
/// there.
///
/// `GatedByVersion` applies only the steps above the stamp read under the lock,
/// so a process that saw a stale version before another one migrated applies
/// nothing twice, and never lowers the stamp. `Always` re-applies every step
/// and leaves the database at the last step's version: a newer build's stamp
/// is lowered, so that build replays its own steps on its next open.
///
/// A file tagged as another database's, or untagged with a stamp this schema
/// does not own, is refused by the caller before any step runs, and again here
/// under each step's lock. The tag is written by the transaction that writes a stamp, so a
/// current untagged database is tagged by its next migrating write and never by
/// a gated open that has nothing to apply.
///
/// The rules every step follows are on [`Schema::steps`].
///
/// `seen` is the file's `user_version`, read and checked by the caller.
fn apply_steps<V: Version>(
    conn: &Connection,
    schema: &Schema<V>,
    mode: MigrationMode,
    seen: i64,
) -> Result<()> {
    let label = schema.label();
    let gated = matches!(mode, MigrationMode::GatedByVersion);
    let last = schema.steps.last().map(|&(version, _)| version.number());
    for &(version, migrate) in schema.steps {
        let version_number = version.number();
        // The hot-path gate: on a current database the one read of the tag and the
        // stamp is the only cost. A step at or below the stamp was applied before the
        // stamp reached it, and a stamp lowered afterwards (an older build's
        // `rtk init`) only makes a later open replay it, so it needs no lock.
        if gated && seen >= version_number {
            continue;
        }
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
                .with_context(|| {
                    format!("Failed to start migration {version:?} of the {label} DB")
                })?;
        let (tag, stamped) = read_identity(&tx, label)?;
        check_owner(&tx, schema, tag, stamped)?;
        if gated && stamped >= version_number {
            tx.rollback()
                .with_context(|| format!("Failed to release the {label} DB lock"))?;
            continue;
        }
        migrate(&tx).with_context(|| format!("Migration {version:?} of the {label} DB failed"))?;
        // SQLite can roll back the whole transaction on its own (SQLITE_IOERR,
        // SQLITE_FULL, SQLITE_NOMEM). The steps here return that error, but a
        // step that discarded it would have run the rest in autocommit, and must
        // not be stamped.
        if tx.is_autocommit() {
            anyhow::bail!("Migration {version:?} of the {label} DB was rolled back by SQLite");
        }
        // A gated step only gets here with the stamp below it. `Always` raises
        // the stamp the same way, and on the last step also hands it back, which
        // lowers a newer build's.
        if stamped < version_number || Some(version_number) == last {
            tx.pragma_update(None, "user_version", version_number)
                .with_context(|| format!("Failed to stamp the {label} DB at {version:?}"))?;
            if tag != schema.kind.application_id() {
                tx.pragma_update(None, "application_id", schema.kind.application_id())
                    .with_context(|| format!("Failed to tag the {label} DB"))?;
            }
        }
        tx.commit()
            .with_context(|| format!("Failed to commit migration {version:?} of the {label} DB"))?;
    }

    Ok(())
}

/// The file's `application_id` and `user_version`, in one query.
fn read_identity(conn: &Connection, label: &str) -> Result<(i32, i64)> {
    conn.query_row(
        "SELECT application_id, user_version FROM pragma_application_id, pragma_user_version",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .with_context(|| format!("Failed to read the {label} DB version"))
}

/// Refuses a file that is another database's: tagged with a different
/// `application_id`, or untagged with a stamp only a database that tags nothing
/// could have written. The error says whose file it is and where to point this
/// database instead.
fn check_owner<V>(conn: &Connection, schema: &Schema<V>, tag: i32, stamp: i64) -> Result<()> {
    if tag == schema.kind.application_id()
        || (tag == 0 && (schema.owns_untagged_stamps || stamp == 0))
    {
        return Ok(());
    }
    let path = conn
        .path()
        .filter(|path| !path.is_empty())
        .unwrap_or("this file");
    let label = schema.label();
    let whose = match DbKind::from_application_id(tag) {
        Some(other) => format!(
            "is the rtk {} database, not the {label} one",
            other.info().label
        ),
        None if DbKind::is_rtk_id(tag) => format!("is a newer rtk database, not the {label} one"),
        None if tag == 0 => format!(
            "is not the {label} database (it is an older rtk database or another application's file)"
        ),
        None => "belongs to another application".to_string(),
    };
    anyhow::bail!(
        "{path} {whose}; set {} to another file",
        schema.kind.info().path_settings
    )
}

/// Add `table.column` unless it already exists, so a failed `ALTER` fails the
/// step instead of being mistaken for the column already being there.
pub(crate) fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<()> {
    if !column_exists(conn, table, column)? {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
            [],
        )
        .with_context(|| format!("Failed to add column {table}.{column}"))?;
    }
    Ok(())
}

/// Column names compare case-insensitively, as SQLite compares them.
pub(crate) fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2 COLLATE NOCASE)",
        params![table, column],
        |row| row.get(0),
    )
    .with_context(|| format!("Failed to inspect columns of {table}"))
}

/// Whether `sqlite_master` holds an object of `kind` (`table`, `index`, …)
/// named `name`.
#[cfg(test)]
pub(crate) fn schema_object_exists(conn: &Connection, kind: &str, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2)",
        [kind, name],
        |row| row.get(0),
    )
    .expect("sqlite_master should be readable")
}

#[cfg(test)]
pub(crate) fn table_exists(conn: &Connection, name: &str) -> bool {
    schema_object_exists(conn, "table", name)
}

/// Scaffolding for tests that hold a database's write lock and watch another
/// connection wait for it.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, LazyLock, Mutex};
    use std::thread::{self, JoinHandle, ThreadId};
    use std::time::{Duration, Instant};

    /// A busy handler is a bare `fn`, with nowhere to carry a test's own flag,
    /// and tests run in parallel: the flag is found by the thread that waits.
    static WAITERS: LazyLock<Mutex<HashMap<ThreadId, Arc<AtomicBool>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn signal_and_retry(attempts: i32) -> bool {
        if let Some(waited) = WAITERS
            .lock()
            .expect("waiters should not be poisoned")
            .get(&thread::current().id())
        {
            waited.store(true, Ordering::SeqCst);
        }
        thread::sleep(Duration::from_millis(5));
        attempts < 6_000
    }

    /// The file's `application_id` and `user_version`.
    pub(crate) fn identity(conn: &Connection) -> (i32, i64) {
        read_identity(conn, "test").expect("identity should be readable")
    }

    pub(crate) fn user_version(conn: &Connection) -> i64 {
        identity(conn).1
    }

    pub(crate) fn application_id(conn: &Connection) -> i32 {
        identity(conn).0
    }

    /// The permission bits of `path`.
    #[cfg(unix)]
    pub(crate) fn mode_bits(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .expect("path should exist")
            .permissions()
            .mode()
            & 0o777
    }

    #[cfg(unix)]
    pub(crate) fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("chmod should succeed");
    }

    /// What [`apply_migrations`] does for a database with a history: the steps
    /// from the file's stamp, never its snapshot. For tests that build a database
    /// as an older build left it, or that pin what a step does.
    pub(crate) fn replay_steps<V: Version>(
        conn: &Connection,
        schema: &Schema<V>,
        mode: MigrationMode,
    ) -> Result<()> {
        let (tag, stamp) = read_identity(conn, schema.label())?;
        check_owner(conn, schema, tag, stamp)?;
        apply_steps(conn, schema, mode, stamp)
    }

    /// Everything about a database's structure that the snapshot and the steps
    /// must agree on, one line each: its schema objects, the columns of each
    /// table (position, name, type, `NOT NULL`, default, primary-key position),
    /// the indexes of each table (uniqueness, origin, columns in order with their
    /// direction), and its tag and stamp. `sqlite_master.sql` is left out: `ALTER
    /// TABLE` rewrites it, so the same schema reads differently.
    fn describe(conn: &Connection) -> Vec<String> {
        fn collect<T>(
            conn: &Connection,
            sql: &str,
            params: impl rusqlite::Params,
            line: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
        ) -> Vec<T> {
            conn.prepare(sql)
                .expect("the structure query should prepare")
                .query_map(params, line)
                .expect("the structure query should run")
                .collect::<std::result::Result<_, _>>()
                .expect("the structure rows should decode")
        }

        let (tag, stamp) = identity(conn);
        let mut lines = vec![format!("application_id {tag:#x}, user_version {stamp}")];
        let objects = collect(
            conn,
            "SELECT type, name, tbl_name FROM sqlite_master
             WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY type, name",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        );
        for (kind, name, table) in &objects {
            lines.push(format!("{kind} {name} on {table}"));
            if kind != "table" {
                continue;
            }
            lines.extend(collect(
                conn,
                "SELECT cid, name, type, \"notnull\", dflt_value, pk
                 FROM pragma_table_info(?1) ORDER BY cid",
                [name],
                |row| {
                    Ok(format!(
                        "column {name}.{}: position {}, type {:?}, not null {}, default {:?}, primary key {}",
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            ));
            let indexes = collect(
                conn,
                "SELECT name, \"unique\", origin, partial FROM pragma_index_list(?1) ORDER BY name",
                [name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            );
            for (index, unique, origin, partial) in indexes {
                let columns = collect(
                    conn,
                    "SELECT name, desc FROM pragma_index_xinfo(?1) WHERE key = 1 ORDER BY seqno",
                    [&index],
                    |row| {
                        let column = row.get::<_, Option<String>>(0)?;
                        let descending = row.get::<_, i64>(1)? == 1;
                        Ok(format!(
                            "{}{}",
                            column.unwrap_or_else(|| "<expression>".to_string()),
                            if descending { " DESC" } else { "" }
                        ))
                    },
                );
                lines.push(format!(
                    "index {index} on {name}: unique {unique}, origin {origin}, partial {partial}, columns ({})",
                    columns.join(", ")
                ));
            }
        }
        lines
    }

    /// What differs between the database `schema.snapshot` creates and the one
    /// its steps build from nothing, one entry per differing line. Empty when
    /// they agree.
    pub(crate) fn snapshot_differences<V: Version>(schema: &Schema<V>) -> Vec<String> {
        let from_snapshot = Connection::open_in_memory().expect("open should succeed");
        let created = create_from_snapshot(&from_snapshot, schema)
            .expect("the snapshot should create a database");
        assert!(created, "an empty database is created from the snapshot");
        let from_steps = Connection::open_in_memory().expect("open should succeed");
        replay_steps(&from_steps, schema, MigrationMode::GatedByVersion)
            .expect("the steps should build a database from nothing");

        differences_between(&from_snapshot, &from_steps)
    }

    /// The lines on which the structure, tag and stamp of two databases differ.
    pub(crate) fn differences_between(
        from_snapshot: &Connection,
        from_steps: &Connection,
    ) -> Vec<String> {
        let snapshot_lines = describe(from_snapshot);
        let step_lines = describe(from_steps);
        let mut differences = Vec::new();
        for line in &step_lines {
            if !snapshot_lines.contains(line) {
                differences.push(format!("only the steps produce: {line}"));
            }
        }
        for line in &snapshot_lines {
            if !step_lines.contains(line) {
                differences.push(format!("only the snapshot produces: {line}"));
            }
        }
        differences
    }

    /// Fails, naming what differs, unless the snapshot and the steps of `schema`
    /// produce the same database: every database's snapshot-equivalence test.
    pub(crate) fn assert_snapshot_matches_steps<V: Version>(schema: &Schema<V>) {
        let differences = snapshot_differences(schema);
        assert!(
            differences.is_empty(),
            "the {} snapshot and its steps produce different databases; a schema change \
             updates its step and its snapshot together:\n{}",
            schema.label(),
            differences.join("\n")
        );
    }

    /// A step that always fails.
    pub(crate) fn failing_step(_conn: &Connection) -> Result<()> {
        anyhow::bail!("step failed")
    }

    /// Puts `conn` in WAL mode, where readers do not wait for the writer.
    pub(crate) fn enable_wal(conn: &Connection) {
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .expect("journal mode should be settable");
        assert_eq!(mode, "wal", "these tests need WAL's concurrent readers");
    }

    /// Work running on its own thread and connection to a database, whose busy
    /// handler records that it had to wait.
    pub(crate) struct Waiter<T> {
        handle: JoinHandle<T>,
        waited: Arc<AtomicBool>,
    }

    /// Runs `work` on a new thread, on a fresh connection to `path`.
    pub(crate) fn spawn_waiter<T: Send + 'static>(
        path: &Path,
        work: impl FnOnce(&Connection) -> T + Send + 'static,
    ) -> Waiter<T> {
        let waited = Arc::new(AtomicBool::new(false));
        let path = path.to_path_buf();
        let handle = thread::spawn({
            let waited = Arc::clone(&waited);
            move || {
                let id = thread::current().id();
                WAITERS
                    .lock()
                    .expect("waiters should not be poisoned")
                    .insert(id, waited);
                let conn = Connection::open(&path).expect("open should succeed");
                conn.busy_handler(Some(signal_and_retry))
                    .expect("busy handler should be set");
                let outcome = work(&conn);
                WAITERS
                    .lock()
                    .expect("waiters should not be poisoned")
                    .remove(&id);
                outcome
            }
        });
        Waiter { handle, waited }
    }

    impl<T: std::fmt::Debug> Waiter<T> {
        /// Blocks until the work is waiting on a lock another connection holds.
        pub(crate) fn wait_until_blocked(&self) {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !self.waited.load(Ordering::SeqCst) {
                assert!(
                    !self.handle.is_finished(),
                    "the work finished before waiting on the lock"
                );
                assert!(
                    Instant::now() < deadline,
                    "the work should wait on the lock"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }

        pub(crate) fn join(self) -> T {
            self.handle.join().expect("the work should not panic")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::identity;
    #[cfg(unix)]
    use super::testing::{chmod, mode_bits};
    use super::*;

    #[derive(Clone, Copy, Debug)]
    enum Versions {
        First = 1,
        Second = 2,
    }

    impl Version for Versions {
        fn number(self) -> i64 {
            self as i64
        }
    }

    const TEST_ID: i32 = 0x5254_4B01;
    /// The recall kind's tag, another rtk database's to the tracking schema.
    const OTHER_ID: i32 = 0x5254_4B02;
    /// A tag no rtk database writes.
    const FOREIGN_ID: i32 = 0x1234_5678;

    fn create_owned_table(conn: &Connection) -> Result<()> {
        conn.execute("CREATE TABLE owned (x INTEGER)", [])?;
        Ok(())
    }

    const OWNED: &[(Versions, Migration)] = &[(Versions::First, create_owned_table)];
    const OWNED_SNAPSHOT: &str = "CREATE TABLE owned (x INTEGER);";

    fn schema(steps: &[(Versions, Migration)], owns_untagged_stamps: bool) -> Schema<'_, Versions> {
        Schema {
            kind: DbKind::Tracking,
            owns_untagged_stamps,
            snapshot: OWNED_SNAPSHOT,
            steps,
        }
    }

    /// Opens `path` with the open that creates a missing file, or the one that
    /// only takes a file that is there.
    #[cfg(unix)]
    fn open_ours(creating: bool, path: &Path) -> Result<Connection> {
        let (schema, mode) = (schema(OWNED, true), MigrationMode::GatedByVersion);
        if creating {
            open_and_migrate(path, &schema, mode).map(|opened| opened.conn)
        } else {
            open_existing_and_migrate(path, &schema, mode)
                .map(|opened| opened.expect("the file is there").conn)
        }
    }

    #[test]
    fn test_add_column_if_missing_matches_names_like_sqlite() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        conn.execute("CREATE TABLE t (Exec_Time_Ms INTEGER)", [])
            .expect("table should be created");

        add_column_if_missing(&conn, "t", "exec_time_ms", "INTEGER DEFAULT 0")
            .expect("a column differing only in case is already there");
    }

    #[test]
    fn test_step_rolled_back_by_sqlite_is_not_stamped() {
        fn rolled_back_step(conn: &Connection) -> Result<()> {
            // What SQLite can do on its own after SQLITE_IOERR or SQLITE_FULL.
            conn.execute_batch("ROLLBACK")?;
            conn.execute("CREATE TABLE after_rollback (x INTEGER)", [])?;
            Ok(())
        }
        let conn = Connection::open_in_memory().expect("open should succeed");

        let err = testing::replay_steps(
            &conn,
            &schema(&[(Versions::First, rolled_back_step as Migration)], false),
            MigrationMode::GatedByVersion,
        )
        .expect_err("a rolled-back step should fail");
        assert_eq!(testing::user_version(&conn), 0);
        assert!(
            format!("{err:#}").contains("rolled back"),
            "error should say the step was rolled back: {err:#}"
        );
    }

    #[test]
    fn test_db_sidecars_are_siblings_not_children() {
        let got = db_sidecars(std::path::Path::new("/data/rtk/history.db"));
        assert_eq!(
            got,
            vec![
                PathBuf::from("/data/rtk/history.db-wal"),
                PathBuf::from("/data/rtk/history.db-shm"),
            ],
            "PathBuf::push would yield history.db/-wal and silently harden nothing"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_restrict_sidecars_covers_the_wal_files_only() {
        let tmp = crate::core::test_isolation::tempdir();
        let db = tmp.path().join("history.db");
        let wal = tmp.path().join("history.db-wal");
        let shm = tmp.path().join("history.db-shm");
        for p in [&db, &wal, &shm] {
            std::fs::write(p, b"x").expect("write");
            chmod(p, 0o644);
        }

        restrict_sidecars(&db);

        for p in [&wal, &shm] {
            assert_eq!(mode_bits(p), 0o600, "expected 0600 on {}", p.display());
        }
        assert_eq!(
            mode_bits(&db),
            0o644,
            "the main file is claim's to restrict"
        );
    }

    #[test]
    fn test_a_fresh_database_is_tagged_with_its_stamp() {
        for mode in [MigrationMode::GatedByVersion, MigrationMode::Always] {
            let conn = Connection::open_in_memory().expect("open should succeed");
            apply_migrations(&conn, &schema(OWNED, false), mode).expect("migration should apply");
            assert_eq!(identity(&conn), (TEST_ID, Versions::First as i64));
        }
    }

    #[test]
    fn test_application_ids_are_pinned() {
        assert_eq!(DbKind::Tracking.application_id(), 0x5254_4B01);
        assert_eq!(DbKind::Recall.application_id(), 0x5254_4B02);
        for info in KINDS {
            assert_eq!(
                DbKind::from_application_id(info.kind.application_id()),
                Some(info.kind)
            );
        }
        assert_eq!(DbKind::from_application_id(FOREIGN_ID), None);
        assert_eq!(DbKind::from_application_id(0x5254_4B00), None);
    }

    #[test]
    fn test_another_databases_tag_is_refused_before_any_step() {
        let cases = [
            (
                OTHER_ID,
                "is the rtk recall database, not the tracking one; set RTK_DB_PATH or [tracking] database_path to another file",
            ),
            (
                FOREIGN_ID,
                "belongs to another application; set RTK_DB_PATH or [tracking] database_path to another file",
            ),
            (
                DbKind::Next.application_id(),
                "is a newer rtk database, not the tracking one; set RTK_DB_PATH or [tracking] database_path to another file",
            ),
            (
                0x5254_4B00,
                "belongs to another application; set RTK_DB_PATH or [tracking] database_path to another file",
            ),
        ];
        for (tag, shape) in cases {
            for mode in [MigrationMode::GatedByVersion, MigrationMode::Always] {
                let conn = Connection::open_in_memory().expect("open should succeed");
                conn.pragma_update(None, "application_id", tag)
                    .expect("tag should be set");

                let err = apply_migrations(&conn, &schema(OWNED, true), mode)
                    .expect_err("a foreign file should be refused");
                assert!(
                    format!("{err:#}").contains(shape),
                    "error should read `{shape}`: {err:#}"
                );
                assert_eq!(identity(&conn), (tag, 0), "nothing is written");
                assert!(!table_exists(&conn, "owned"), "no step runs");
            }
        }
    }

    #[test]
    fn test_the_other_databases_setting_is_named_for_recall() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        conn.pragma_update(None, "application_id", TEST_ID)
            .expect("tag should be set");
        let recall = Schema {
            kind: DbKind::Recall,
            owns_untagged_stamps: false,
            snapshot: OWNED_SNAPSHOT,
            steps: OWNED,
        };

        let err = apply_migrations(&conn, &recall, MigrationMode::GatedByVersion)
            .expect_err("a tracking file is not a recall store");
        assert!(
            format!("{err:#}").contains(
                "is the rtk tracking database, not the recall one; set RTK_RECALL_DB or [retriever] database_path to another file"
            ),
            "{err:#}"
        );
    }

    #[test]
    fn test_an_untagged_stamp_is_accepted_only_by_the_database_that_never_tagged() {
        for (owns_untagged_stamps, accepted) in [(true, true), (false, false)] {
            for mode in [MigrationMode::GatedByVersion, MigrationMode::Always] {
                let conn = Connection::open_in_memory().expect("open should succeed");
                conn.pragma_update(None, "user_version", 7)
                    .expect("stamp should be set");

                let outcome = apply_migrations(&conn, &schema(OWNED, owns_untagged_stamps), mode);
                assert_eq!(
                    outcome.is_ok(),
                    accepted,
                    "owns untagged: {owns_untagged_stamps}"
                );
                if !accepted {
                    assert_eq!(identity(&conn), (0, 7), "nothing is written");
                    let err = outcome.expect_err("refused");
                    assert!(
                        format!("{err:#}")
                            .contains("is not the tracking database (it is an older rtk database or another application's file)"),
                        "{err:#}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_a_gated_open_with_nothing_to_apply_does_not_write_the_tag() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        conn.pragma_update(None, "user_version", Versions::First as i64)
            .expect("stamp should be set");

        apply_migrations(&conn, &schema(OWNED, true), MigrationMode::GatedByVersion)
            .expect("a current untagged file opens");
        assert_eq!(identity(&conn), (0, Versions::First as i64));
        assert!(!table_exists(&conn, "owned"));

        apply_migrations(&conn, &schema(OWNED, true), MigrationMode::Always)
            .expect("the init run migrates");
        assert_eq!(
            identity(&conn).0,
            TEST_ID,
            "a migrating write tags the file"
        );
    }

    /// The migrator reads an untagged, unversioned file without the lock, then
    /// waits for it while another process tags the file as someone else's: the
    /// tag is read again under the lock, in either mode.
    #[test]
    fn test_the_tag_is_read_again_under_the_lock() {
        // Both ways to bring an empty file up: its snapshot and its steps.
        for (mode, from_snapshot) in [
            (MigrationMode::GatedByVersion, true),
            (MigrationMode::GatedByVersion, false),
            (MigrationMode::Always, true),
            (MigrationMode::Always, false),
        ] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("shared.db");
            let tagger = Connection::open(&path).expect("open should succeed");
            testing::enable_wal(&tagger);
            tagger
                .execute_batch("BEGIN IMMEDIATE")
                .expect("lock should succeed");

            let migrator = testing::spawn_waiter(&path, move |conn| {
                if from_snapshot {
                    apply_migrations(conn, &schema(OWNED, false), mode)
                } else {
                    testing::replay_steps(conn, &schema(OWNED, false), mode)
                }
            });
            migrator.wait_until_blocked();
            tagger
                .pragma_update(None, "application_id", OTHER_ID)
                .expect("tag should be set");
            tagger
                .execute_batch("COMMIT")
                .expect("commit should succeed");

            let err = migrator
                .join()
                .expect_err("the file was tagged while the migrator waited");
            assert!(
                format!("{err:#}").contains("is the rtk recall database"),
                "{err:#}"
            );
            assert_eq!(identity(&tagger), (OTHER_ID, 0));
            assert!(!table_exists(&tagger, "owned"));
        }
    }

    /// A failure to create the file that is not "not found" is reported as it
    /// is, and no mode is touched on the way.
    #[test]
    #[cfg(unix)]
    fn test_a_pre_create_that_fails_for_another_reason_changes_no_mode() {
        let tmp = crate::core::test_isolation::tempdir();
        chmod(tmp.path(), 0o755);
        let file = tmp.path().join("plain_file");
        std::fs::write(&file, b"").expect("file should be written");
        chmod(&file, 0o644);

        for path in [file.join("sub").join("x.db"), file.join("x.db")] {
            let err = open_and_migrate(&path, &schema(OWNED, false), MigrationMode::GatedByVersion)
                .expect_err("a path under a file cannot be created");
            let message = format!("{err:#}");
            assert!(
                message.contains("Failed to pre-create private tracking DB file")
                    && message.contains("plain_file"),
                "error should name the file: {message}"
            );
            assert_eq!(mode_bits(&file), 0o644);
            assert_eq!(mode_bits(tmp.path()), 0o755);
        }
    }

    /// A directory that cannot be created is named. Skipped for a user who can
    /// write into a `0555` directory (root).
    #[test]
    #[cfg(unix)]
    fn test_a_directory_that_cannot_be_created_is_named() {
        let tmp = crate::core::test_isolation::tempdir();
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).expect("directory should be created");
        chmod(&locked, 0o555);
        if std::fs::write(locked.join("probe"), b"").is_ok() {
            eprintln!("skipped: a 0555 directory is writable for this user (root)");
            return;
        }

        let err = open_and_migrate(
            &locked.join("sub").join("x.db"),
            &schema(OWNED, false),
            MigrationMode::GatedByVersion,
        )
        .expect_err("a directory under a locked one cannot be created");
        let message = format!("{err:#}");
        assert!(
            message.contains("Failed to create the tracking DB directory")
                && message.contains("sub"),
            "error should name the directory: {message}"
        );
        chmod(&locked, 0o755);
    }

    #[test]
    fn test_a_read_only_connection_is_replaced_only_for_a_file_that_can_be_written() {
        let probed = std::cell::Cell::new(false);
        let probe = |answer: bool| {
            probed.set(true);
            answer
        };

        assert!(should_reopen(true, || probe(true)));
        assert!(!should_reopen(true, || probe(false)), "nothing to gain");
        probed.set(false);
        assert!(!should_reopen(false, || probe(true)));
        assert!(!probed.get(), "a writable connection never probes the file");
    }

    #[test]
    fn test_writable_reports_what_this_process_can_do() {
        let tmp = crate::core::test_isolation::tempdir();
        let file = tmp.path().join("probe.db");
        std::fs::write(&file, b"x").expect("file should be written");
        assert!(writable(&file));
        assert!(!writable(&tmp.path().join("absent.db")));
        #[cfg(unix)]
        {
            chmod(&file, 0o444);
            let still_writable = std::fs::OpenOptions::new().write(true).open(&file).is_ok();
            assert_eq!(writable(&file), still_writable);
        }
        assert_eq!(
            std::fs::read(&file).expect("file should be readable"),
            b"x",
            "probing writes nothing"
        );
    }

    #[test]
    fn test_a_missing_store_is_none_and_nothing_is_created() {
        let tmp = crate::core::test_isolation::tempdir();
        let missing_dir = tmp.path().join("absent");
        let path = missing_dir.join("x.db");

        let opened =
            open_existing_and_migrate(&path, &schema(OWNED, false), MigrationMode::GatedByVersion)
                .expect("a missing store is not an error");
        assert!(opened.is_none());
        assert!(!missing_dir.exists(), "no directory is created either");
    }

    #[test]
    fn test_a_store_that_cannot_be_opened_is_an_error_not_none() {
        let tmp = crate::core::test_isolation::tempdir();

        let err = open_existing_and_migrate(
            tmp.path(),
            &schema(OWNED, false),
            MigrationMode::GatedByVersion,
        )
        .expect_err("a directory is not a store");
        assert!(
            format!("{err:#}").contains("Failed to open tracking DB"),
            "{err:#}"
        );
    }

    #[test]
    fn test_an_rtk_id_of_a_kind_this_build_does_not_know_is_a_newer_database() {
        assert!(DbKind::is_rtk_id(DbKind::Next.application_id()));
        assert!(
            !DbKind::is_rtk_id(0x5254_4B00),
            "kind 0 is no rtk database's"
        );
        assert!(!DbKind::is_rtk_id(FOREIGN_ID));
    }

    /// A directory its owner cannot use stops the creation of a new file and the
    /// first read of a WAL file, which has to create its sidecars there.
    #[test]
    #[cfg(unix)]
    fn test_a_locked_directory_is_given_back_to_its_owner() {
        for dir_mode in [0o555, 0o500, 0o000] {
            for (existing, creating, journal) in [
                (false, true, "wal"),
                (true, true, "wal"),
                (true, false, "wal"),
                (true, true, "delete"),
                (true, false, "delete"),
            ] {
                // A store behind a directory that cannot be searched is not seen.
                if dir_mode == 0o000 && !creating {
                    continue;
                }
                let what = format!(
                    "{dir_mode:o}, existing {existing}, creating {creating}, journal {journal}"
                );
                let tmp = crate::core::test_isolation::tempdir();
                let dir = tmp.path().join("locked");
                std::fs::create_dir(&dir).expect("directory should be created");
                let path = dir.join("ours.db");
                if existing {
                    let conn = open_ours(true, &path).expect("a new file should open");
                    conn.execute_batch(&format!("PRAGMA journal_mode = {journal};"))
                        .expect("journal mode should be set");
                }
                chmod(&dir, dir_mode);

                let conn = open_ours(creating, &path).unwrap_or_else(|e| panic!("{what}: {e:#}"));
                conn.execute_batch("CREATE TABLE written (x INTEGER)")
                    .expect("the file should be writable");
                assert_eq!(mode_bits(&dir), 0o700, "{what}");
            }
        }
    }

    /// A store that is not there is not an error, and is no reason to touch a
    /// locked directory, including one that cannot be searched to find that out.
    #[test]
    #[cfg(unix)]
    fn test_a_missing_store_leaves_a_locked_directory_alone() {
        for dir_mode in [0o555, 0o600, 0o000] {
            let tmp = crate::core::test_isolation::tempdir();
            chmod(tmp.path(), dir_mode);
            let opened = open_existing_and_migrate(
                &tmp.path().join("missing.db"),
                &schema(OWNED, true),
                MigrationMode::GatedByVersion,
            )
            .expect("a missing store is not an error");
            assert!(opened.is_none(), "{dir_mode:o}");
            assert_eq!(mode_bits(tmp.path()), dir_mode, "{dir_mode:o}");
            chmod(tmp.path(), 0o755);
        }
    }

    /// A store under a plain file is not there either.
    #[test]
    fn test_a_store_under_a_plain_file_is_not_there() {
        let tmp = crate::core::test_isolation::tempdir();
        let file = tmp.path().join("plain_file");
        std::fs::write(&file, b"").expect("file should be written");
        let opened = open_existing_and_migrate(
            &file.join("store.db"),
            &schema(OWNED, true),
            MigrationMode::GatedByVersion,
        )
        .expect("a store under a file is not an error");
        assert!(opened.is_none());
    }

    /// A schema whose snapshot and steps leave different traces, so a test sees
    /// which one built a database: the steps create `from_steps` (first step) and
    /// give it a column (second step), the snapshot creates `from_snapshot` and a
    /// one-row `snapshot_runs`.
    fn create_from_steps(conn: &Connection) -> Result<()> {
        conn.execute("CREATE TABLE from_steps (x INTEGER)", [])?;
        Ok(())
    }

    fn extend_from_steps(conn: &Connection) -> Result<()> {
        conn.execute("ALTER TABLE from_steps ADD COLUMN y TEXT", [])?;
        Ok(())
    }

    const TRACED_STEPS: &[(Versions, Migration)] = &[
        (Versions::First, create_from_steps),
        (Versions::Second, extend_from_steps),
    ];
    const COUNTING_SNAPSHOT: &str = "
        CREATE TABLE from_snapshot (x INTEGER, y TEXT);
        CREATE TABLE snapshot_runs (n INTEGER);
        INSERT INTO snapshot_runs VALUES (1);
    ";

    fn counting_schema() -> Schema<'static, Versions> {
        Schema {
            kind: DbKind::Tracking,
            owns_untagged_stamps: true,
            snapshot: COUNTING_SNAPSHOT,
            steps: TRACED_STEPS,
        }
    }

    /// Whether the database was built by the snapshot, by the steps, or neither.
    fn built_by(conn: &Connection) -> (bool, bool) {
        (
            table_exists(conn, "from_snapshot"),
            table_exists(conn, "from_steps"),
        )
    }

    fn snapshot_runs(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM snapshot_runs", [], |row| row.get(0))
            .expect("the snapshot's table should be readable")
    }

    fn current_identity() -> (i32, i64) {
        (TEST_ID, Versions::Second as i64)
    }

    #[test]
    fn test_a_new_database_is_created_from_the_snapshot_not_the_steps() {
        for mode in [MigrationMode::GatedByVersion, MigrationMode::Always] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("new.db");

            let conn = open_and_migrate(&path, &counting_schema(), mode)
                .expect("a new database should open")
                .conn;

            assert_eq!(built_by(&conn), (true, false));
            assert_eq!(snapshot_runs(&conn), 1);
            assert_eq!(
                identity(&conn),
                current_identity(),
                "stamped at the last step's version, and tagged"
            );
        }
    }

    #[test]
    fn test_an_empty_existing_file_takes_the_snapshot() {
        let tmp = crate::core::test_isolation::tempdir();
        let zero_bytes = tmp.path().join("empty.db");
        std::fs::write(&zero_bytes, b"").expect("file should be written");
        let wal_only = tmp.path().join("wal.db");
        testing::enable_wal(&Connection::open(&wal_only).expect("open should succeed"));
        let tagged = tmp.path().join("tagged.db");
        Connection::open(&tagged)
            .expect("open should succeed")
            .pragma_update(None, "application_id", TEST_ID)
            .expect("tag should be set");

        for path in [zero_bytes, wal_only, tagged] {
            let conn =
                open_existing_and_migrate(&path, &counting_schema(), MigrationMode::GatedByVersion)
                    .expect("an empty file should open")
                    .expect("the file exists")
                    .conn;

            assert_eq!(built_by(&conn), (true, false), "{}", path.display());
            assert_eq!(identity(&conn), current_identity(), "{}", path.display());
        }
    }

    #[test]
    fn test_a_database_with_tables_and_no_stamp_takes_the_steps() {
        for mode in [MigrationMode::GatedByVersion, MigrationMode::Always] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("legacy.db");
            Connection::open(&path)
                .expect("open should succeed")
                .execute_batch("CREATE TABLE legacy (x INTEGER); INSERT INTO legacy VALUES (7);")
                .expect("legacy file should be written");

            // `CREATE TABLE from_steps` has nothing in the way: the steps run
            // from nothing, and the snapshot never does.
            let conn = open_and_migrate(&path, &counting_schema(), mode)
                .expect("a legacy database should open")
                .conn;

            assert_eq!(built_by(&conn), (false, true));
            assert!(table_exists(&conn, "legacy"));
            assert_eq!(identity(&conn), current_identity());
        }
    }

    #[test]
    fn test_a_database_at_an_intermediate_stamp_takes_the_steps_above_it() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("old.db");
        Connection::open(&path)
            .expect("open should succeed")
            .execute_batch(&format!(
                "PRAGMA application_id = {TEST_ID};
                 PRAGMA user_version = {};
                 CREATE TABLE from_steps (x INTEGER);",
                Versions::First as i64
            ))
            .expect("an older build's file should be written");

        let conn = open_and_migrate(&path, &counting_schema(), MigrationMode::GatedByVersion)
            .expect("an older database should open")
            .conn;

        assert_eq!(built_by(&conn), (false, true));
        assert!(
            column_exists(&conn, "from_steps", "y").expect("columns are readable"),
            "the second step ran"
        );
        assert_eq!(identity(&conn), current_identity());
    }

    /// A stamp above 0 means the file has a history, even where it holds no table.
    #[test]
    fn test_a_stamped_file_without_tables_does_not_take_the_snapshot() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("stamped.db");
        Connection::open(&path)
            .expect("open should succeed")
            .pragma_update(None, "user_version", Versions::Second as i64)
            .expect("stamp should be set");

        let conn = open_and_migrate(&path, &counting_schema(), MigrationMode::GatedByVersion)
            .expect("the file should open")
            .conn;

        assert_eq!(built_by(&conn), (false, false), "no snapshot, no step");
        assert_eq!(identity(&conn), (0, Versions::Second as i64));
    }

    /// A stamp another process writes while this one waits for the lock, on a
    /// file that still has no table: the file has a history now, and is not
    /// created from the snapshot.
    #[test]
    fn test_a_stamp_written_while_waiting_for_the_lock_rules_out_the_snapshot() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("shared.db");
        let other = Connection::open(&path).expect("open should succeed");
        testing::enable_wal(&other);
        other
            .execute_batch("BEGIN IMMEDIATE")
            .expect("lock should succeed");

        let opener = testing::spawn_waiter(&path, |conn| {
            apply_migrations(conn, &counting_schema(), MigrationMode::GatedByVersion)
        });
        opener.wait_until_blocked();
        other
            .execute_batch(&format!(
                "PRAGMA user_version = {}; COMMIT;",
                Versions::Second as i64
            ))
            .expect("the stamp should be written");
        opener
            .join()
            .expect("the opener should find a stamped file");

        assert_eq!(built_by(&other), (false, false));
    }

    /// The tag and the stamp are compared too, whatever the structure says.
    #[test]
    fn test_the_equivalence_check_compares_the_tag_and_the_stamp() {
        let database = |tag: i32, stamp: i64| {
            let conn = Connection::open_in_memory().expect("open should succeed");
            conn.execute_batch(&format!(
                "CREATE TABLE t (a INTEGER);
                 PRAGMA application_id = {tag};
                 PRAGMA user_version = {stamp};"
            ))
            .expect("database should be written");
            conn
        };

        assert!(
            testing::differences_between(&database(TEST_ID, 2), &database(TEST_ID, 2)).is_empty()
        );
        let stamps = testing::differences_between(&database(TEST_ID, 2), &database(TEST_ID, 1));
        assert!(
            stamps.iter().any(|line| line.contains("user_version 2")) && stamps.len() == 2,
            "{stamps:#?}"
        );
        let tags = testing::differences_between(&database(TEST_ID, 2), &database(OTHER_ID, 2));
        assert!(
            tags.iter()
                .any(|line| line.contains("application_id 0x52544b01"))
                && tags.len() == 2,
            "{tags:#?}"
        );
    }

    #[test]
    fn test_init_replays_the_steps_on_a_database_with_tables_and_not_on_an_empty_one() {
        let tmp = crate::core::test_isolation::tempdir();
        let empty = tmp.path().join("empty.db");
        std::fs::write(&empty, b"").expect("file should be written");
        let conn = open_existing_and_migrate(&empty, &counting_schema(), MigrationMode::Always)
            .expect("an empty file should open")
            .expect("the file exists")
            .conn;
        assert_eq!(
            built_by(&conn),
            (true, false),
            "an empty file takes the snapshot"
        );
        assert_eq!(identity(&conn), current_identity());

        // The database is no longer empty, so init replays the steps on it.
        let replayed = open_existing_and_migrate(&empty, &counting_schema(), MigrationMode::Always)
            .expect("the steps should run on a database with tables")
            .expect("the file exists")
            .conn;
        assert_eq!(built_by(&replayed), (true, true), "the steps ran on top");
        assert_eq!(
            snapshot_runs(&replayed),
            1,
            "the snapshot did not run again"
        );
    }

    /// The other process creates the database while this one waits for the
    /// lock: this one finds it current and creates nothing.
    #[test]
    fn test_a_second_creator_finds_the_database_the_first_created() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("shared.db");
        let first = Connection::open(&path).expect("open should succeed");
        testing::enable_wal(&first);
        first
            .execute_batch("BEGIN IMMEDIATE")
            .expect("lock should succeed");

        let second = testing::spawn_waiter(&path, |conn| {
            apply_migrations(conn, &counting_schema(), MigrationMode::GatedByVersion)
        });
        second.wait_until_blocked();
        first
            .execute_batch(&format!(
                "{COUNTING_SNAPSHOT}
                 PRAGMA application_id = {TEST_ID};
                 PRAGMA user_version = {};
                 COMMIT;",
                Versions::Second as i64
            ))
            .expect("the first creator should commit");
        second
            .join()
            .expect("the second opener should find the database current");

        assert_eq!(built_by(&first), (true, false), "no step ran in the second");
        assert_eq!(snapshot_runs(&first), 1);
        assert_eq!(identity(&first), current_identity());
    }

    /// Two real openers racing on a path nobody has created: whatever the
    /// interleaving, both succeed and the snapshot ran once.
    #[test]
    fn test_two_openers_racing_on_a_new_path_create_it_once() {
        use std::sync::{Arc, Barrier};

        for _ in 0..20 {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("raced.db");
            let start = Arc::new(Barrier::new(2));
            let openers: Vec<_> = (0..2)
                .map(|_| {
                    let (path, start) = (path.clone(), Arc::clone(&start));
                    std::thread::spawn(move || {
                        start.wait();
                        open_and_migrate(&path, &counting_schema(), MigrationMode::GatedByVersion)
                            .map(|_| ())
                    })
                })
                .collect();
            for opener in openers {
                opener
                    .join()
                    .expect("an opener should not panic")
                    .expect("both openers should succeed");
            }

            let conn = Connection::open(&path).expect("open should succeed");
            assert_eq!(built_by(&conn), (true, false));
            assert_eq!(snapshot_runs(&conn), 1);
            assert_eq!(identity(&conn), current_identity());
        }
    }

    #[test]
    fn test_a_failing_snapshot_leaves_the_database_empty_and_unstamped() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        let broken = Schema {
            snapshot: "CREATE TABLE from_snapshot (x INTEGER); CREATE TABLE (",
            ..counting_schema()
        };

        let err = apply_migrations(&conn, &broken, MigrationMode::GatedByVersion)
            .expect_err("a broken snapshot should fail");

        assert!(
            format!("{err:#}").contains("Failed to create the tracking DB from its snapshot"),
            "{err:#}"
        );
        assert_eq!(identity(&conn), (0, 0));
        assert_eq!(
            built_by(&conn),
            (false, false),
            "its first statement is rolled back"
        );
    }

    #[test]
    fn test_a_snapshot_rolled_back_by_sqlite_is_not_stamped() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        let rolled_back = Schema {
            snapshot: "CREATE TABLE from_snapshot (x INTEGER); ROLLBACK;",
            ..counting_schema()
        };

        let err = apply_migrations(&conn, &rolled_back, MigrationMode::GatedByVersion)
            .expect_err("a rolled-back snapshot should fail");

        assert!(format!("{err:#}").contains("rolled back"), "{err:#}");
        assert_eq!(identity(&conn), (0, 0));
    }

    #[test]
    fn test_a_schema_without_steps_never_uses_its_snapshot() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        let stepless = Schema {
            steps: &[],
            ..counting_schema()
        };

        apply_migrations(&conn, &stepless, MigrationMode::GatedByVersion)
            .expect("nothing to apply");

        assert_eq!(built_by(&conn), (false, false));
        assert_eq!(identity(&conn), (0, 0));
    }

    /// A snapshot and steps that agree: the equivalence check finds nothing.
    const AGREEING_STEPS: &[(Versions, Migration)] = &[
        (Versions::First, |conn| {
            conn.execute_batch(
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);",
            )?;
            Ok(())
        }),
        (Versions::Second, |conn| {
            conn.execute_batch(
                "ALTER TABLE t ADD COLUMN c INTEGER DEFAULT 0;
                 CREATE UNIQUE INDEX idx_c ON t(c);",
            )?;
            Ok(())
        }),
    ];
    const AGREEING_SNAPSHOT: &str = "
        CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', c INTEGER DEFAULT 0, PRIMARY KEY (a));
        CREATE INDEX idx_b_a ON t(b, a DESC);
        CREATE UNIQUE INDEX idx_c ON t(c);
    ";

    fn equivalence_schema(snapshot: &'static str) -> Schema<'static, Versions> {
        Schema {
            snapshot,
            steps: AGREEING_STEPS,
            ..counting_schema()
        }
    }

    #[test]
    fn test_the_equivalence_check_accepts_a_snapshot_that_matches_its_steps() {
        assert_eq!(
            testing::snapshot_differences(&equivalence_schema(AGREEING_SNAPSHOT)),
            Vec::<String>::new()
        );
        testing::assert_snapshot_matches_steps(&equivalence_schema(AGREEING_SNAPSHOT));
    }

    /// Each way a snapshot can drift from its steps is found, and named.
    #[test]
    fn test_the_equivalence_check_names_what_differs() {
        let cases = [
            (
                "a missing column",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);
                 CREATE UNIQUE INDEX idx_c ON t(a);",
                "column t.c",
            ),
            (
                "columns in another order",
                "CREATE TABLE t (a INTEGER NOT NULL, c INTEGER DEFAULT 0, b TEXT DEFAULT '', PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);
                 CREATE UNIQUE INDEX idx_c ON t(c);",
                "column t.c: position 1",
            ),
            (
                "a different type",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', c TEXT DEFAULT 0, PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);
                 CREATE UNIQUE INDEX idx_c ON t(c);",
                "type \"INTEGER\"",
            ),
            (
                "a different default",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT 'x', c INTEGER DEFAULT 0, PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);
                 CREATE UNIQUE INDEX idx_c ON t(c);",
                "default Some(\"''\")",
            ),
            (
                "a missing NOT NULL",
                "CREATE TABLE t (a INTEGER, b TEXT DEFAULT '', c INTEGER DEFAULT 0, PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);
                 CREATE UNIQUE INDEX idx_c ON t(c);",
                "not null 1",
            ),
            (
                "a missing index",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', c INTEGER DEFAULT 0, PRIMARY KEY (a));
                 CREATE UNIQUE INDEX idx_c ON t(c);",
                "index idx_b_a",
            ),
            (
                "an index that is not unique",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', c INTEGER DEFAULT 0, PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);
                 CREATE INDEX idx_c ON t(c);",
                "unique 1",
            ),
            (
                "index columns in another order",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', c INTEGER DEFAULT 0, PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(a DESC, b);
                 CREATE UNIQUE INDEX idx_c ON t(c);",
                "columns (b, a DESC)",
            ),
            (
                "an index direction",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', c INTEGER DEFAULT 0, PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a);
                 CREATE UNIQUE INDEX idx_c ON t(c);",
                "columns (b, a DESC)",
            ),
            (
                "an extra table",
                "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', c INTEGER DEFAULT 0, PRIMARY KEY (a));
                 CREATE INDEX idx_b_a ON t(b, a DESC);
                 CREATE UNIQUE INDEX idx_c ON t(c);
                 CREATE TABLE extra (x INTEGER);",
                "table extra",
            ),
        ];
        for (what, snapshot, expected) in cases {
            let differences = testing::snapshot_differences(&equivalence_schema(snapshot));
            assert!(
                differences.iter().any(|line| line.contains(expected)),
                "{what}: a difference naming `{expected}` is expected, got {differences:#?}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "a schema change updates its step and its snapshot together")]
    fn test_a_snapshot_that_drifted_fails_the_equivalence_assertion_with_the_rule() {
        testing::assert_snapshot_matches_steps(&equivalence_schema(
            "CREATE TABLE t (a INTEGER NOT NULL, b TEXT DEFAULT '', PRIMARY KEY (a));
             CREATE INDEX idx_b_a ON t(b, a DESC);
             CREATE UNIQUE INDEX idx_c ON t(a);",
        ));
    }

    /// A file its owner can neither read nor write does not open at all, and a
    /// directory locked along with it (`chmod -R 000`) hides its mode until it is
    /// repaired.
    #[test]
    #[cfg(unix)]
    fn test_a_locked_file_is_given_back_to_its_owner() {
        for (file_mode, dir_mode) in [(0o000, 0o700), (0o200, 0o700), (0o000, 0o000)] {
            let what = format!("file {file_mode:o}, directory {dir_mode:o}");
            let tmp = crate::core::test_isolation::tempdir();
            let dir = tmp.path().join("locked");
            std::fs::create_dir(&dir).expect("directory should be created");
            let path = dir.join("ours.db");
            open_ours(true, &path).expect("a new file should open");
            chmod(&path, file_mode);
            chmod(&dir, dir_mode);

            let conn = open_ours(true, &path).unwrap_or_else(|e| panic!("{what}: {e:#}"));
            conn.execute_batch("CREATE TABLE written (x INTEGER)")
                .expect("the file should be writable");
            assert_eq!(mode_bits(&dir), 0o700, "{what}");
            assert_eq!(mode_bits(&path), 0o600, "{what}");
        }
    }

    /// A file that can be read is refused with its locked directory as it was, and
    /// a failure with a directory and a file their owner can use changes no mode.
    #[test]
    #[cfg(unix)]
    fn test_a_failure_permissions_do_not_explain_changes_no_mode() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("foreign.db");
        let foreign = Connection::open(&path).expect("open should succeed");
        foreign
            .execute_batch(&format!("PRAGMA application_id = {FOREIGN_ID};"))
            .expect("foreign file should be written");
        drop(foreign);
        chmod(tmp.path(), 0o555);
        open_ours(true, &path).expect_err("a foreign file is refused");
        assert_eq!(mode_bits(tmp.path()), 0o555);
        chmod(tmp.path(), 0o755);

        let garbage = tmp.path().join("garbage.db");
        std::fs::write(&garbage, vec![b'x'; 4096]).expect("file should be written");
        chmod(&garbage, 0o644);
        open_ours(true, &garbage).expect_err("a file that is not a database does not open");
        assert_eq!(mode_bits(&garbage), 0o644);
        assert_eq!(mode_bits(tmp.path()), 0o755);
    }

    /// A read-only file in a read-only directory, as a restored backup leaves it.
    #[cfg(unix)]
    fn read_only_in_a_read_only_dir(dir: &Path, path: &Path) {
        chmod(path, 0o444);
        chmod(dir, 0o555);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_file_that_is_not_a_database_keeps_its_modes_and_its_directory_too() {
        let tmp = crate::core::test_isolation::tempdir();
        let dir = tmp.path().join("configs");
        std::fs::create_dir(&dir).expect("mkdir");
        let path = dir.join("settings.conf");
        std::fs::write(&path, vec![b'x'; 4096]).expect("file should be written");
        read_only_in_a_read_only_dir(&dir, &path);

        for creating in [true, false] {
            open_ours(creating, &path).expect_err("a file that is not a database does not open");
            assert_eq!(mode_bits(&path), 0o444);
            assert_eq!(mode_bits(&dir), 0o555);
        }
        chmod(&dir, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_foreign_wal_file_that_needs_a_sidecar_is_refused_with_its_modes_kept() {
        let tmp = crate::core::test_isolation::tempdir();
        let dir = tmp.path().join("other");
        std::fs::create_dir(&dir).expect("mkdir");
        let path = dir.join("other.db");
        {
            let conn = Connection::open(&path).expect("open should succeed");
            testing::enable_wal(&conn);
            conn.execute_batch(&format!(
                "PRAGMA application_id = {FOREIGN_ID}; CREATE TABLE theirs (x INTEGER);"
            ))
            .expect("foreign file should be written");
        }
        read_only_in_a_read_only_dir(&dir, &path);

        for creating in [true, false] {
            let err = open_ours(creating, &path).expect_err("a foreign file is refused");
            assert!(
                format!("{err:#}").contains("another application"),
                "{err:#}"
            );
            assert_eq!(mode_bits(&path), 0o444);
            assert_eq!(mode_bits(&dir), 0o555);
        }
        chmod(&dir, 0o755);
    }

    /// What a WAL-mode file on a read-only mount does to a normal open.
    fn cannot_open() -> Result<Option<(Connection, (i32, i64))>> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
            None,
        )
        .into())
    }

    fn recovered(path: &Path, read_only: bool) -> Result<Option<(Connection, (i32, i64))>> {
        recovering(path, &schema(OWNED, false), cannot_open, |_| read_only)
    }

    /// A WAL-mode database with no `-wal` left, holding one row.
    fn clean_wal_db(path: &Path) {
        let conn = Connection::open(path).expect("open should succeed");
        testing::enable_wal(&conn);
        conn.execute_batch("CREATE TABLE kept (x INTEGER); INSERT INTO kept VALUES (7);")
            .expect("the row should be written");
    }

    #[test]
    fn test_a_clean_wal_file_on_a_read_only_filesystem_is_read_without_its_sidecars() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("it's a #100% file.db");
        clean_wal_db(&path);

        let (conn, identity) = recovered(&path, true)
            .expect("the file should be read as it is")
            .expect("the file exists");

        assert_eq!(identity, (0, 0));
        let row: i64 = conn
            .query_row("SELECT x FROM kept", [], |row| row.get(0))
            .expect("the row should be readable");
        assert_eq!(row, 7);
        assert!(
            conn.is_readonly(rusqlite::DatabaseName::Main)
                .expect("is_readonly")
        );
        assert!(
            db_sidecars(&path).iter().all(|sidecar| !sidecar.exists()),
            "the read needs no sidecar"
        );
    }

    #[test]
    fn test_a_file_elsewhere_keeps_its_failure() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("history.db");
        clean_wal_db(&path);
        recovered(&path, false).expect_err("another process may be writing it");
        assert!(!on_read_only_filesystem(&path));
    }

    /// An immutable read would miss the `-wal` and read half a write past a hot
    /// `-journal`. An empty one holds nothing.
    #[test]
    fn test_a_log_holding_data_keeps_the_failure_rather_than_miss_it() {
        for suffix in ["-wal", "-journal"] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("history.db");
            clean_wal_db(&path);
            let log = sibling(&path, suffix);
            std::fs::write(&log, b"pages").expect("file should be written");
            recovered(&path, true).expect_err(suffix);

            std::fs::write(&log, b"").expect("file should be written");
            recovered(&path, true).expect(suffix);
        }
    }

    #[test]
    fn test_a_read_only_file_of_another_database_is_refused_on_recovery() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("other.db");
        Connection::open(&path)
            .expect("open should succeed")
            .pragma_update(None, "application_id", OTHER_ID)
            .expect("tag should be writable");
        let err = recovered(&path, true).expect_err("a foreign file is refused");
        assert!(
            format!("{err:#}").contains("not the tracking one"),
            "{err:#}"
        );
    }

    #[test]
    fn test_a_path_that_needs_escaping_makes_an_immutable_uri() {
        let uri = immutable_uri(Path::new("a b/c#d?e%f.db"));
        assert_eq!(uri, "file:a%20b/c%23d%3Fe%25f.db?immutable=1");
    }

    /// A file name that is not UTF-8, which macOS filesystems refuse.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_path_that_is_not_utf8_is_read_too() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp
            .path()
            .join(std::ffi::OsStr::from_bytes(b"history-\xff.db"));
        clean_wal_db(&path);

        open_immutable(&path, "test").expect("the file should be read");
    }

    /// A file that only `unlock` makes readable, and the refusal that follows.
    #[cfg(unix)]
    fn assert_refused_with_its_modes_back(
        content: impl FnOnce(&Path),
        file_mode: u32,
        dir_mode: u32,
    ) {
        let tmp = crate::core::test_isolation::tempdir();
        let dir = tmp.path().join("kept");
        std::fs::create_dir(&dir).expect("mkdir");
        let path = dir.join("kept.db");
        content(&path);
        // The open that takes only a file that is there cannot see one in a directory
        // it cannot search.
        let opens: &[bool] = if dir_mode & 0o100 == 0 {
            &[true]
        } else {
            &[true, false]
        };
        for &creating in opens {
            chmod(&dir, 0o700);
            chmod(&path, file_mode);
            chmod(&dir, dir_mode);

            open_ours(creating, &path).expect_err("the file should be refused");

            assert_eq!(mode_bits(&dir), dir_mode);
            chmod(&dir, 0o700);
            assert_eq!(mode_bits(&path), file_mode);
        }
    }

    #[cfg(unix)]
    fn foreign_database(path: &Path) {
        Connection::open(path)
            .expect("open should succeed")
            .execute_batch(&format!(
                "PRAGMA application_id = {FOREIGN_ID}; CREATE TABLE theirs (x INTEGER);"
            ))
            .expect("foreign file should be written");
    }

    #[cfg(unix)]
    fn not_a_database(path: &Path) {
        std::fs::write(path, vec![b'x'; 4096]).expect("file should be written");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_refused_file_its_owner_could_not_read_gets_its_modes_back() {
        for file_mode in [0o000, 0o200] {
            assert_refused_with_its_modes_back(foreign_database, file_mode, 0o700);
            assert_refused_with_its_modes_back(not_a_database, file_mode, 0o700);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_a_refused_file_in_a_directory_its_owner_could_not_enter_gets_both_back() {
        for dir_mode in [0o000, 0o300] {
            assert_refused_with_its_modes_back(foreign_database, 0o000, dir_mode);
            assert_refused_with_its_modes_back(not_a_database, 0o000, dir_mode);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_a_path_with_two_leading_slashes_is_not_read_as_a_host() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("history.db");
        clean_wal_db(&path);
        let doubled = format!("/{}", path.display());

        open_immutable(Path::new(&doubled), "test").expect("the file should be read");
    }

    #[test]
    fn test_a_write_that_could_never_work_is_told_from_one_that_failed() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        conn.execute_batch("CREATE TABLE kept (x INTEGER)")
            .expect("the table should be created");
        let schema = schema(OWNED, false);

        let missing = conn
            .execute("DELETE FROM absent", [])
            .expect_err("no table");
        assert!(every_write_would_fail(
            &schema.explain_missing(missing.into(), Some("disk full"))
        ));

        let missing = conn
            .execute("DELETE FROM absent", [])
            .expect_err("no table");
        assert!(
            !every_write_would_fail(&schema.explain_missing(missing.into(), None)),
            "a database at its stamp should have the table"
        );

        conn.execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON kept
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .expect("the trigger should be created");
        let refused = conn
            .execute("INSERT INTO kept VALUES (1)", [])
            .expect_err("the trigger refuses it");
        assert!(!every_write_would_fail(&refused.into()));

        conn.pragma_update(None, "query_only", true)
            .expect("query_only should be settable");
        let read_only = conn
            .execute("DELETE FROM kept", [])
            .expect_err("the connection is read-only");
        assert!(every_write_would_fail(
            &anyhow::Error::from(read_only).context("Failed to log")
        ));
    }

    #[test]
    fn test_only_a_missing_table_is_a_missing_table() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        conn.execute_batch("CREATE TABLE present (x INTEGER)")
            .expect("create should succeed");
        let error = |sql| anyhow::Error::from(conn.execute(sql, []).expect_err(sql));
        assert_eq!(
            missing_table(&error("SELECT x FROM absent")),
            Some("no such table: absent")
        );
        assert_eq!(missing_table(&error("SELECT y FROM present")), None);
        assert_eq!(
            missing_table(&error("INSERT INTO present (y) VALUES (1)")),
            None
        );

        let explained =
            schema(OWNED, false).explain_missing(error("SELECT x FROM absent"), Some("disk full"));
        assert_eq!(
            missing_table(&explained),
            None,
            "that note says why already"
        );
    }

    /// What a step finds next to the file when it starts writing.
    #[cfg(unix)]
    fn fail_on_a_loose_sidecar(conn: &Connection) -> Result<()> {
        let path = conn.path().expect("the database has a file");
        for sidecar in db_sidecars(Path::new(path)) {
            anyhow::ensure!(mode_bits(&sidecar) == 0o600, "{}", sidecar.display());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_a_step_never_writes_to_a_loose_sidecar() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("history.db");
        // The holder stays open, so the sidecars exist, as a restored backup's do.
        let holder = Connection::open(&path).expect("open should succeed");
        testing::enable_wal(&holder);
        holder
            .execute_batch("CREATE TABLE kept (x INTEGER)")
            .expect("the table should be created");
        for sidecar in db_sidecars(&path) {
            chmod(&sidecar, 0o644);
        }
        let steps: &[(Versions, Migration)] = &[(Versions::First, fail_on_a_loose_sidecar)];

        let opened = open_and_migrate(&path, &schema(steps, false), MigrationMode::GatedByVersion)
            .expect("the file should open");

        assert_eq!(opened.behind, None);
        drop(holder);
    }

    /// A rollback-journal file whose lock another connection holds cannot be read
    /// either, once the step that wanted the lock has given up.
    #[test]
    fn test_a_failed_step_on_a_file_that_cannot_be_read_again_is_kept_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());
        let holder = Connection::open(&path).expect("open should succeed");
        let conn = open_db(&path, OpenFlags::default(), "test").expect("open");
        conn.busy_timeout(std::time::Duration::ZERO)
            .expect("busy timeout should be set");
        let identity = read_identity(&conn, "test").expect("identity should be readable");
        holder
            .execute_batch("BEGIN EXCLUSIVE")
            .expect("the lock should be taken");

        let opened = claim(
            conn,
            identity,
            &path,
            &schema(TWO_STEPS, false),
            MigrationMode::GatedByVersion,
            OpenFlags::default(),
        )
        .expect("the database is used as it is");

        assert_eq!(opened.behind.as_deref(), Some("database is locked"));
        drop(holder);
    }

    /// SQLite puts the sidecars next to the file a symlink resolves to.
    #[test]
    #[cfg(unix)]
    fn test_the_sidecars_of_a_symlinked_file_are_made_private() {
        let tmp = crate::core::test_isolation::tempdir();
        let real = tmp.path().join("ours.db.real");
        let link = tmp.path().join("ours.db");
        std::os::unix::fs::symlink(&real, &link).expect("symlink should be created");
        let conn = open_ours(true, &link).expect("a new file should open");
        conn.execute_batch("CREATE TABLE written (x INTEGER)")
            .expect("the file should be writable");
        let sidecars = db_sidecars(&real);
        for sidecar in &sidecars {
            chmod(sidecar, 0o644);
        }

        open_ours(true, &link).expect("the file should open again");

        for sidecar in &sidecars {
            assert_eq!(mode_bits(sidecar), 0o600, "{}", sidecar.display());
        }
    }

    /// A foreign file is read and refused, not changed: it keeps its rows, its
    /// journal mode, its permission bits and its directory's mode, whichever open
    /// meets it. Outside WAL it keeps its bytes too: SQLite may checkpoint a WAL
    /// file and use its `-shm`, which changes none of the rest.
    #[test]
    #[cfg(unix)]
    fn test_a_refused_file_is_left_as_it_was() {
        fn bytes(path: &Path) -> Vec<u8> {
            std::fs::read(path).expect("file should be readable")
        }
        fn rows_and_journal(path: &Path) -> (Vec<i64>, String) {
            let conn = Connection::open(path).expect("open should succeed");
            let mut stmt = conn
                .prepare("SELECT x FROM theirs ORDER BY x")
                .expect("their table should be readable");
            let rows = stmt
                .query_map([], |row| row.get(0))
                .expect("rows should be readable")
                .collect::<std::result::Result<Vec<i64>, _>>()
                .expect("rows should decode");
            drop(stmt);
            let journal = conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .expect("journal mode should be readable");
            (rows, journal)
        }

        for (journal, creating) in [
            ("delete", true),
            ("delete", false),
            ("wal", true),
            ("wal", false),
        ] {
            let what = format!("{journal}, creating: {creating}");
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("foreign.db");
            let foreign = Connection::open(&path).expect("open should succeed");
            foreign
                .execute_batch(&format!(
                    "PRAGMA journal_mode = {journal};
                     PRAGMA application_id = {FOREIGN_ID};
                     CREATE TABLE theirs (x INTEGER);
                     INSERT INTO theirs VALUES (1), (2);"
                ))
                .expect("foreign file should be written");
            drop(foreign);
            chmod(&path, 0o644);
            chmod(tmp.path(), 0o755);
            let before = rows_and_journal(&path);
            assert_eq!(before, (vec![1, 2], journal.to_string()), "{what}");
            let bytes_before = bytes(&path);

            open_ours(creating, &path).expect_err("a foreign file is refused");

            if journal != "wal" {
                assert_eq!(bytes(&path), bytes_before, "{what}");
            }
            assert_eq!(rows_and_journal(&path), before, "{what}");
            assert_eq!(mode_bits(&path), 0o644, "{what}");
            assert_eq!(mode_bits(tmp.path()), 0o755, "{what}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_only_a_confirmed_files_directory_is_made_private() {
        for creating in [true, false] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("ours.db");
            open_ours(true, &path).expect("a new store should open");
            chmod(tmp.path(), 0o755);

            open_ours(creating, &path).expect("our own file opens");
            let expected = if creating { 0o700 } else { 0o755 };
            assert_eq!(mode_bits(tmp.path()), expected, "creating: {creating}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_a_new_store_in_an_existing_directory_makes_it_private() {
        let tmp = crate::core::test_isolation::tempdir();
        chmod(tmp.path(), 0o755);

        open_ours(true, &tmp.path().join("new.db")).expect("a new store should open");

        assert_eq!(mode_bits(tmp.path()), 0o700);
    }

    /// Another connection holds the file's lock for a moment: the open waits
    /// for it, even for the first read.
    #[test]
    fn test_an_open_waits_for_a_lock_held_on_the_file() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("ours.db");
        let holder = Connection::open(&path).expect("open should succeed");
        holder
            .execute_batch(&format!(
                "PRAGMA application_id = {TEST_ID};
                 PRAGMA user_version = {};
                 CREATE TABLE owned (x INTEGER);",
                Versions::First as i64
            ))
            .expect("our file should be written");
        holder
            .execute_batch("BEGIN EXCLUSIVE")
            .expect("lock should succeed");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            holder
                .execute_batch("COMMIT")
                .expect("commit should succeed");
        });

        open_existing_and_migrate(&path, &schema(OWNED, true), MigrationMode::GatedByVersion)
            .expect("the open waits for the lock instead of failing");
        release.join().expect("the holder should not panic");
    }

    #[test]
    #[cfg(unix)]
    fn test_pre_create_makes_a_missing_file_private_and_leaves_an_existing_one_alone() {
        let tmp = crate::core::test_isolation::tempdir();

        // A missing file in a missing directory is created private, directory first.
        let nested = tmp.path().join("data").join("new.db");
        pre_create(&nested, "tracking").expect("a new file is created");
        assert_eq!(mode_bits(&nested), 0o600);
        assert_eq!(mode_bits(nested.parent().expect("has a parent")), 0o700);

        // A file that is there, whoever made it, is not ours to chmod.
        let existing = tmp.path().join("existing.db");
        std::fs::write(&existing, b"").expect("file should be written");
        chmod(&existing, 0o644);
        pre_create(&existing, "tracking").expect("an existing file is not an error");
        assert_eq!(mode_bits(&existing), 0o644);
    }

    fn create_first_table(conn: &Connection) -> Result<()> {
        conn.execute("CREATE TABLE IF NOT EXISTS first (x INTEGER)", [])?;
        Ok(())
    }

    fn create_second_table(conn: &Connection) -> Result<()> {
        conn.execute("CREATE TABLE IF NOT EXISTS second (x INTEGER)", [])?;
        Ok(())
    }

    const FIRST_STEP: &[(Versions, Migration)] = &[(Versions::First, create_first_table)];

    const TWO_STEPS: &[(Versions, Migration)] = &[
        (Versions::First, create_first_table),
        (Versions::Second, create_second_table),
    ];

    fn read_only_flags() -> OpenFlags {
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    }

    /// `claim` on a connection that SQLite opened read-only.
    fn claim_read_only(
        path: &Path,
        schema: &Schema<Versions>,
        mode: MigrationMode,
    ) -> Result<Opened> {
        let conn = open_db(path, read_only_flags(), "test")?;
        let identity = read_identity(&conn, "test")?;
        claim(conn, identity, path, schema, mode, read_only_flags())
    }

    /// A database file stamped at the first step only, by a build that knew just it.
    fn stale_db(dir: &Path) -> PathBuf {
        let path = dir.join("history.db");
        let conn = Connection::open(&path).expect("open should succeed");
        testing::replay_steps(&conn, &schema(FIRST_STEP, false), MigrationMode::Always)
            .expect("seeding should succeed");
        path
    }

    #[test]
    fn test_a_stale_read_only_database_is_kept_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());

        let opened = claim_read_only(
            &path,
            &schema(TWO_STEPS, false),
            MigrationMode::GatedByVersion,
        )
        .expect("the database is used as it is");

        assert_eq!(
            opened.behind.as_deref(),
            Some("attempt to write a readonly database")
        );
        assert_eq!(testing::user_version(&opened.conn), Versions::First as i64);
    }

    #[test]
    fn test_a_writable_stale_database_is_migrated_not_kept_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());
        let opened = open_and_migrate(
            &path,
            &schema(TWO_STEPS, false),
            MigrationMode::GatedByVersion,
        )
        .expect("a writable stale database migrates");
        assert_eq!(opened.behind, None);
        assert_eq!(testing::user_version(&opened.conn), Versions::Second as i64);
        assert!(table_exists(&opened.conn, "second"));
    }

    #[test]
    fn test_a_current_or_newer_read_only_database_is_not_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());
        let writer = Connection::open(&path).expect("open should succeed");
        apply_migrations(&writer, &schema(TWO_STEPS, false), MigrationMode::Always)
            .expect("seeding should succeed");
        let behind = |path: &Path| {
            claim_read_only(
                path,
                &schema(TWO_STEPS, false),
                MigrationMode::GatedByVersion,
            )
            .expect("a read-only database opens")
            .behind
        };
        assert_eq!(behind(&path), None, "stamped at the last step");

        writer
            .pragma_update(None, "user_version", Versions::Second as i64 + 1)
            .expect("stamp should be writable");
        assert_eq!(behind(&path), None, "stamped by a newer build");
    }

    #[test]
    fn test_rtk_init_still_fails_on_a_read_only_database() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());
        let err = claim_read_only(&path, &schema(TWO_STEPS, false), MigrationMode::Always)
            .expect_err("an open that must migrate reports the refusal");
        assert_eq!(cause(&err), "attempt to write a readonly database");
    }

    #[test]
    fn test_a_read_only_file_is_refused_before_it_is_kept_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let open = |path: &Path, owns_untagged_stamps: bool| {
            claim_read_only(
                path,
                &schema(TWO_STEPS, owns_untagged_stamps),
                MigrationMode::GatedByVersion,
            )
        };

        // Another database's file, tagged.
        let tagged = tmp.path().join("tagged.db");
        let conn = Connection::open(&tagged).expect("open should succeed");
        conn.pragma_update(None, "application_id", OTHER_ID)
            .expect("tag should be writable");
        conn.pragma_update(None, "user_version", 1)
            .expect("stamp should be writable");
        let err = open(&tagged, true).expect_err("a foreign file is refused");
        assert!(
            format!("{err:#}").contains("not the tracking one"),
            "{err:#}"
        );

        // An untagged file with a stamp only a database that tags nothing writes.
        let untagged = tmp.path().join("untagged.db");
        Connection::open(&untagged)
            .expect("open should succeed")
            .pragma_update(None, "user_version", 1)
            .expect("stamp should be writable");
        let err = open(&untagged, false)
            .expect_err("an untagged stamp is refused by a database that tags");
        assert!(
            format!("{err:#}").contains("is not the tracking database"),
            "{err:#}"
        );
        assert!(
            open(&untagged, true)
                .expect("the owner of untagged stamps takes it")
                .behind
                .is_some(),
            "and keeps it behind"
        );

        // An untagged, unversioned file is anyone's.
        let empty = tmp.path().join("empty.db");
        Connection::open(&empty).expect("open should succeed");
        assert!(
            open(&empty, false)
                .expect("an unversioned file is taken")
                .behind
                .is_some()
        );
    }

    fn write_then_fail(conn: &Connection) -> Result<()> {
        create_second_table(conn)?;
        testing::failing_step(conn)
    }

    const FAILS_SECOND: &[(Versions, Migration)] = &[
        (Versions::First, create_first_table),
        (Versions::Second, write_then_fail),
    ];

    #[test]
    fn test_a_failed_step_rolls_back_and_the_database_is_used_as_it_is() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());

        let opened = open_and_migrate(
            &path,
            &schema(FAILS_SECOND, false),
            MigrationMode::GatedByVersion,
        )
        .expect("a failed step does not fail the open");

        assert_eq!(opened.behind.as_deref(), Some("step failed"));
        assert_eq!(testing::user_version(&opened.conn), Versions::First as i64);
        assert!(!table_exists(&opened.conn, "second"), "rolled back");
        assert!(table_exists(&opened.conn, "first"), "and the rest is there");
        drop(opened);

        // The next open tries again.
        let opened = open_and_migrate(
            &path,
            &schema(TWO_STEPS, false),
            MigrationMode::GatedByVersion,
        )
        .expect("the step works now");
        assert_eq!(opened.behind, None);
        assert_eq!(testing::user_version(&opened.conn), Versions::Second as i64);
    }

    #[test]
    fn test_rtk_init_still_fails_on_a_failed_step() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());
        let err = open_and_migrate(&path, &schema(FAILS_SECOND, false), MigrationMode::Always)
            .expect_err("rtk init reports it");
        assert!(format!("{err:#}").contains("step failed"), "{err:#}");
    }

    #[test]
    fn test_a_file_tagged_after_the_first_read_is_refused_not_kept_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = tmp.path().join("history.db");
        let conn = open_db(&path, OpenFlags::default(), "test").expect("open");
        conn.pragma_update(None, "application_id", OTHER_ID)
            .expect("tag should be writable");

        // What the first read saw: an untagged, unversioned file.
        let err = claim(
            conn,
            (0, 0),
            &path,
            &schema(TWO_STEPS, false),
            MigrationMode::GatedByVersion,
            OpenFlags::default(),
        )
        .expect_err("the step's lock sees the tag");
        assert!(
            format!("{err:#}").contains("not the tracking one"),
            "{err:#}"
        );
    }

    #[test]
    fn test_the_cause_of_a_failed_step_is_what_sqlite_said() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        let sqlite = conn.execute("NOT SQL", []).expect_err("not sql");
        let said = sqlite.to_string();
        let err = anyhow::Error::from(sqlite)
            .context("Failed to backfill")
            .context("Migration failed");
        assert_eq!(cause(&err), said);

        let other = anyhow::anyhow!("rolled back").context("Migration failed");
        assert_eq!(cause(&other), "rolled back");
    }

    #[test]
    fn test_a_refusal_that_only_shows_on_write_keeps_the_connection_behind() {
        // `query_only` makes SQLite refuse every write with SQLITE_READONLY on a
        // connection it does not report as read-only.
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());
        let open = |mode| {
            let conn = open_db(&path, OpenFlags::default(), "test").expect("open");
            conn.pragma_update(None, "query_only", true)
                .expect("query_only should be settable");
            assert!(
                !conn
                    .is_readonly(rusqlite::DatabaseName::Main)
                    .expect("is_readonly")
            );
            let identity = read_identity(&conn, "test").expect("identity should be readable");
            claim(
                conn,
                identity,
                &path,
                &schema(TWO_STEPS, false),
                mode,
                OpenFlags::default(),
            )
        };

        let opened = open(MigrationMode::GatedByVersion).expect("the connection is kept");
        assert_eq!(
            opened.behind.as_deref(),
            Some("attempt to write a readonly database")
        );
        assert_eq!(testing::user_version(&opened.conn), Versions::First as i64);

        let err = open(MigrationMode::Always).expect_err("an open that must migrate fails");
        assert_eq!(cause(&err), "attempt to write a readonly database");
    }

    /// A directory that stops the first read is not a read-only database: it is
    /// given back to its owner and the steps run.
    #[cfg(unix)]
    #[test]
    fn test_a_locked_directory_does_not_keep_a_database_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let dir = tmp.path().join("locked");
        std::fs::create_dir(&dir).expect("mkdir");
        let path = dir.join("history.db");
        {
            let conn = Connection::open(&path).expect("open should succeed");
            testing::enable_wal(&conn);
            apply_migrations(&conn, &schema(OWNED, false), MigrationMode::Always)
                .expect("seeding should succeed");
        }
        chmod(&dir, 0o555);

        let opened = open_and_migrate(
            &path,
            &schema(TWO_STEPS, false),
            MigrationMode::GatedByVersion,
        )
        .expect("the directory should be given back");

        assert_eq!(opened.behind, None);
        assert_eq!(mode_bits(&dir), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_file_that_can_be_made_writable_is_migrated_not_kept_behind() {
        let tmp = crate::core::test_isolation::tempdir();
        let path = stale_db(tmp.path());
        chmod(&path, 0o444);
        let opened = open_and_migrate(
            &path,
            &schema(TWO_STEPS, false),
            MigrationMode::GatedByVersion,
        )
        .expect("a restored backup is made writable and migrated");
        assert_eq!(opened.behind, None);
        assert_eq!(testing::user_version(&opened.conn), Versions::Second as i64);
    }

    #[test]
    fn test_only_a_missing_table_or_column_is_a_missing_object() {
        let conn = Connection::open_in_memory().expect("open should succeed");
        conn.execute_batch("CREATE TABLE present (x INTEGER)")
            .expect("create should succeed");
        for (sql, message) in [
            ("SELECT x FROM absent", "no such table: absent"),
            ("SELECT y FROM present", "no such column: y"),
            (
                "INSERT INTO present (y) VALUES (1)",
                "table present has no column named y",
            ),
        ] {
            let err = anyhow::Error::from(conn.execute(sql, []).expect_err(sql)).context("query");
            assert_eq!(missing_object(&err), Some(message), "{sql}");
        }

        let syntax = anyhow::Error::from(conn.execute("NOT SQL", []).expect_err("not sql"));
        assert_eq!(missing_object(&syntax), None);
        let other = anyhow::anyhow!("no such table: not from sqlite");
        assert_eq!(missing_object(&other), None);
    }
}
