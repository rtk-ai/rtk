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
}

/// Whether to gate schema migrations behind `user_version` (the hot-path
/// default) or always re-run them (used by `rtk init`, which isn't a hot path
/// and wants to self-heal a dropped/corrupted table — see
/// `tracking::ensure_schema_fresh`).
pub(crate) enum MigrationMode {
    GatedByVersion,
    Always,
}

/// Open (creating if needed) the `schema` database at `path`, apply pragmas, and
/// run its steps per `mode`.
pub(crate) fn open_and_migrate<V: Version>(
    path: &Path,
    schema: &Schema<V>,
    mode: MigrationMode,
) -> Result<Connection> {
    let label = schema.label();
    // For a brand-new DB, create its directory and pre-create the file ourselves
    // via `open_private` so SQLite derives the -wal/-shm modes from an
    // already-private DB instead of the umask: that file is ours by
    // construction. An already-existing DB skips that pre-create's own redundant
    // open()/close() pair (a second file open on top of the `Connection::open`
    // right after it), and neither it nor its directory is touched until it has
    // been read and found to be ours: see `claim`. The tracking DB's open is on
    // the hot path for every `rtk <cmd>` and, since rtk-ai/rtk#3206's
    // hook_decisions ground-truth logging, every single PreToolUse hook call (not
    // just RTK-covered ones), so avoiding that redundant open on the
    // already-set-up common case matters for the <10ms budget.
    pre_create(path, label)?;
    let conn = open_ready(path, OpenFlags::default(), label)?;
    // The directory is ours to tighten once the file has been found to be ours,
    // whether this open created it or `claim` confirms it.
    claim(conn, path, schema, mode, OpenFlags::default(), true)
}

/// Opens `path` with `flags` and sets the busy timeout. Another connection can
/// hold the file's lock while the first read of it happens (a checkpoint, a
/// switch to WAL), so the timeout comes before that read.
fn open_ready(path: &Path, flags: OpenFlags, label: &str) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("Failed to open {label} DB: {}", path.display()))?;
    busy_wait(&conn, label)?;
    Ok(conn)
}

fn busy_wait(conn: &Connection, label: &str) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .with_context(|| format!("Failed to set the {label} DB busy timeout"))
}

/// Creates the file private (`0600`) when there is none, and says whether it did.
/// The creation is exclusive, so a file that appears in the meantime is not
/// ours to chmod: it is left to [`claim`], which reads it first, and so is its
/// directory. The directory is prepared only when the file's path was not found,
/// and no other failure touches a mode.
fn pre_create(path: &Path, label: &str) -> Result<bool> {
    let attempt = || match crate::core::utils::open_private(
        std::fs::OpenOptions::new().write(true).create_new(true),
        path,
    ) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
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
/// directory is left alone.
pub(crate) fn open_existing_and_migrate<V: Version>(
    path: &Path,
    schema: &Schema<V>,
    mode: MigrationMode,
) -> Result<Option<Connection>> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = match Connection::open_with_flags(path, flags) {
        Ok(conn) => conn,
        Err(e) => {
            let vanished = matches!(
                std::fs::metadata(path),
                Err(missing) if missing.kind() == std::io::ErrorKind::NotFound
            );
            if vanished {
                return Ok(None);
            }
            return Err(e).with_context(|| {
                format!("Failed to open {} DB: {}", schema.label(), path.display())
            });
        }
    };
    busy_wait(&conn, schema.label())?;
    claim(conn, path, schema, mode, flags, false).map(Some)
}

/// Reads the file's tag and stamp and refuses it unless it is this database's,
/// before rtk changes anything on it: no pragma write, no chmod, no directory
/// repair. A file whose tag is foreign when it is opened is left as it was in
/// content, journal mode, permissions and directory; reading a WAL-mode file may
/// checkpoint it and use its `-shm`, as any SQLite reader does. A file another
/// process tags between this read and a step's lock is refused under that lock,
/// after the pragmas and permissions here have been applied. For a file that is
/// ours, repairs the owner-only permissions of its directory (when `repair_dir`)
/// and of the file *before* any write to it (external tampering, a restored
/// backup), sets WAL, and migrates. `flags` are the ones `conn` was opened with.
fn claim<V: Version>(
    mut conn: Connection,
    path: &Path,
    schema: &Schema<V>,
    mode: MigrationMode,
    flags: OpenFlags,
    repair_dir: bool,
) -> Result<Connection> {
    let label = schema.label();
    let (tag, stamp) = read_identity(&conn, label)?;
    check_owner(&conn, schema, tag, stamp)?;
    if repair_dir {
        private_dir(path, label)?;
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
        conn = open_ready(path, flags, label)?;
    }
    // WAL for concurrent access (multiple Claude Code instances). It is refused
    // on some filesystems (NFS, read-only), which stays non-fatal. It is cheap on
    // every open: WAL persists in the DB file.
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");
    // SQLite creates `-wal`/`-shm` under the umask, so the permissions of the
    // sidecars are repaired once migrating is done, whether or not it worked.
    let migrated = run_steps(&conn, schema, mode, stamp);
    restrict_sidecars(path);
    migrated?;
    Ok(conn)
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

/// SQLite appends `-wal`/`-shm` to the whole filename, so these are siblings
/// rather than extension swaps. Concatenate on `OsString`, not `PathBuf::push`,
/// which would append a component and silently target `history.db/-wal`.
fn restrict_sidecars(db_path: &Path) {
    for sidecar in db_sidecars(db_path) {
        crate::core::utils::restrict_file(&sidecar);
    }
}

pub(crate) fn db_sidecars(db_path: &Path) -> Vec<PathBuf> {
    ["-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut name = db_path.as_os_str().to_os_string();
            name.push(suffix);
            PathBuf::from(name)
        })
        .collect()
}

/// Reads the tag and stamp of a connection the caller opened itself, refuses
/// it unless it is this database's, and runs the steps: what `claim` does for a
/// file it opens.
#[cfg(test)]
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
    // The hot-path gate: on a current database the one read of the tag and the
    // stamp is the only cost. A step at or below the stamp was applied before the
    // stamp reached it, and a stamp lowered afterwards (an older build's
    // `rtk init`) only makes a later open replay it, so it needs no lock.
    let seen = gated.then_some(seen);
    let last = schema.steps.last().map(|&(version, _)| version.number());
    for &(version, migrate) in schema.steps {
        let version_number = version.number();
        if seen.is_some_and(|seen| seen >= version_number) {
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
        if gated || stamped < version_number || Some(version_number) == last {
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
        use std::os::unix::fs::PermissionsExt;

        let tmp = crate::core::test_isolation::tempdir();
        let db = tmp.path().join("history.db");
        let wal = tmp.path().join("history.db-wal");
        let shm = tmp.path().join("history.db-shm");
        for p in [&db, &wal, &shm] {
            std::fs::write(p, b"x").expect("write");
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }

        restrict_sidecars(&db);

        let mode_of =
            |p: &Path| std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
        for p in [&wal, &shm] {
            assert_eq!(mode_of(p), 0o600, "expected 0600 on {}", p.display());
        }
        assert_eq!(mode_of(&db), 0o644, "the main file is claim's to restrict");
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
    fn test_an_untagged_unversioned_file_is_taken_by_any_database() {
        let conn = Connection::open_in_memory().expect("open should succeed");

        apply_migrations(&conn, &schema(OWNED, false), MigrationMode::GatedByVersion)
            .expect("an empty file is anyone's");
        assert_eq!(identity(&conn).0, TEST_ID);
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
        use std::os::unix::fs::PermissionsExt;

        let tmp = crate::core::test_isolation::tempdir();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
            .expect("chmod should succeed");
        let file = tmp.path().join("plain_file");
        std::fs::write(&file, b"").expect("file should be written");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644))
            .expect("chmod should succeed");

        let err = open_and_migrate(
            &file.join("sub").join("x.db"),
            &schema(OWNED, false),
            MigrationMode::GatedByVersion,
        )
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

    /// A directory that cannot be created is named. Skipped for a user who can
    /// write into a `0555` directory (root).
    #[test]
    #[cfg(unix)]
    fn test_a_directory_that_cannot_be_created_is_named() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = crate::core::test_isolation::tempdir();
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).expect("directory should be created");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))
            .expect("chmod should succeed");
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
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
            .expect("chmod should succeed");
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
    #[cfg(unix)]
    fn test_writable_reports_what_this_process_can_do() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = crate::core::test_isolation::tempdir();
        let file = tmp.path().join("probe.db");
        std::fs::write(&file, b"x").expect("file should be written");
        assert!(writable(&file));
        assert!(!writable(&tmp.path().join("absent.db")));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444))
            .expect("chmod should succeed");
        let still_writable = std::fs::OpenOptions::new().write(true).open(&file).is_ok();
        assert_eq!(writable(&file), still_writable);
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
        let newer = DbKind::Next.application_id();
        assert!(DbKind::is_rtk_id(newer));
        assert!(
            !DbKind::is_rtk_id(0x5254_4B00),
            "kind 0 is no rtk database's"
        );
        assert!(!DbKind::is_rtk_id(FOREIGN_ID));
        let conn = Connection::open_in_memory().expect("open should succeed");
        conn.pragma_update(None, "application_id", newer)
            .expect("tag should be set");

        let err = apply_migrations(&conn, &schema(OWNED, true), MigrationMode::GatedByVersion)
            .expect_err("a newer build's database is refused");
        assert!(
            format!("{err:#}").contains(
                "is a newer rtk database, not the tracking one; set RTK_DB_PATH or [tracking] database_path to another file"
            ),
            "{err:#}"
        );
        let bare_prefix = Connection::open_in_memory().expect("open should succeed");
        bare_prefix
            .pragma_update(None, "application_id", 0x5254_4B00)
            .expect("tag should be set");
        let err = apply_migrations(
            &bare_prefix,
            &schema(OWNED, true),
            MigrationMode::GatedByVersion,
        )
        .expect_err("the bare prefix is refused");
        assert!(
            format!("{err:#}").contains("belongs to another application"),
            "{err:#}"
        );
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

    const TWO_STEPS: &[(Versions, Migration)] = &[
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
            steps: TWO_STEPS,
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
                .expect("a new database should open");

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
                    .expect("the file exists");

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
                .expect("a legacy database should open");

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
            .expect("an older database should open");

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
            .expect("the file should open");

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
            .expect("the file exists");
        assert_eq!(
            built_by(&conn),
            (true, false),
            "an empty file takes the snapshot"
        );
        assert_eq!(identity(&conn), current_identity());

        // The database is no longer empty, so init replays the steps on it.
        let replayed = open_existing_and_migrate(&empty, &counting_schema(), MigrationMode::Always)
            .expect("the steps should run on a database with tables")
            .expect("the file exists");
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

    #[cfg(unix)]
    fn mode_bits(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .expect("path should exist")
            .permissions()
            .mode()
            & 0o777
    }

    /// A foreign file is read and refused, not changed: it keeps its journal
    /// mode, its permission bits, its bytes and its directory's mode, whichever
    /// open meets it.
    #[test]
    #[cfg(unix)]
    fn test_a_refused_file_is_left_as_it_was() {
        use std::os::unix::fs::PermissionsExt;

        fn bytes(path: &Path) -> Vec<u8> {
            std::fs::read(path).expect("file should be readable")
        }
        fn journal_mode(path: &Path) -> String {
            Connection::open(path)
                .expect("open should succeed")
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .expect("journal mode should be readable")
        }

        for creating in [true, false] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("foreign.db");
            let foreign = Connection::open(&path).expect("open should succeed");
            foreign
                .execute_batch(&format!(
                    "PRAGMA application_id = {FOREIGN_ID};
                     CREATE TABLE theirs (x INTEGER);"
                ))
                .expect("foreign file should be written");
            drop(foreign);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("chmod should succeed");
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
                .expect("chmod should succeed");
            assert_eq!(
                journal_mode(&path),
                "delete",
                "the file starts in rollback mode"
            );
            let before = bytes(&path);

            let mode = MigrationMode::GatedByVersion;
            let refused = if creating {
                open_and_migrate(&path, &schema(OWNED, true), mode).map(|_| ())
            } else {
                open_existing_and_migrate(&path, &schema(OWNED, true), mode).map(|_| ())
            };

            refused.expect_err("a foreign file is refused");
            assert_eq!(bytes(&path), before, "creating: {creating}");
            assert_eq!(mode_bits(&path), 0o644, "creating: {creating}");
            assert_eq!(mode_bits(tmp.path()), 0o755, "creating: {creating}");
            assert_eq!(journal_mode(&path), "delete", "creating: {creating}");
        }
    }

    /// A foreign WAL-mode file keeps its rows, its journal mode and its
    /// permission bits through a refused open (SQLite may checkpoint it and use
    /// its `-shm`, which changes none of them).
    #[test]
    #[cfg(unix)]
    fn test_a_refused_wal_file_keeps_its_rows_journal_mode_and_permissions() {
        use std::os::unix::fs::PermissionsExt;

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

        for creating in [true, false] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("foreign.db");
            let foreign = Connection::open(&path).expect("open should succeed");
            testing::enable_wal(&foreign);
            foreign
                .execute_batch(&format!(
                    "PRAGMA application_id = {FOREIGN_ID};
                     CREATE TABLE theirs (x INTEGER);
                     INSERT INTO theirs VALUES (1), (2);"
                ))
                .expect("foreign file should be written");
            drop(foreign);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("chmod should succeed");
            let before = rows_and_journal(&path);
            assert_eq!(before, (vec![1, 2], "wal".to_string()));

            let mode = MigrationMode::GatedByVersion;
            let refused = if creating {
                open_and_migrate(&path, &schema(OWNED, true), mode).map(|_| ())
            } else {
                open_existing_and_migrate(&path, &schema(OWNED, true), mode).map(|_| ())
            };

            refused.expect_err("a foreign file is refused");
            assert_eq!(rows_and_journal(&path), before, "creating: {creating}");
            assert_eq!(mode_bits(&path), 0o644, "creating: {creating}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_only_a_confirmed_files_directory_is_made_private() {
        use std::os::unix::fs::PermissionsExt;

        for creating in [true, false] {
            let tmp = crate::core::test_isolation::tempdir();
            let path = tmp.path().join("ours.db");
            drop(
                open_and_migrate(&path, &schema(OWNED, true), MigrationMode::GatedByVersion)
                    .expect("a new store should open"),
            );
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
                .expect("chmod should succeed");

            let mode = MigrationMode::GatedByVersion;
            if creating {
                open_and_migrate(&path, &schema(OWNED, true), mode).map(|_| ())
            } else {
                open_existing_and_migrate(&path, &schema(OWNED, true), mode).map(|_| ())
            }
            .expect("our own file opens");
            let expected = if creating { 0o700 } else { 0o755 };
            assert_eq!(mode_bits(tmp.path()), expected, "creating: {creating}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_a_new_store_in_an_existing_directory_makes_it_private() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = crate::core::test_isolation::tempdir();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
            .expect("chmod should succeed");

        open_and_migrate(
            &tmp.path().join("new.db"),
            &schema(OWNED, true),
            MigrationMode::GatedByVersion,
        )
        .expect("a new store should open");

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
        use std::os::unix::fs::PermissionsExt;

        let tmp = crate::core::test_isolation::tempdir();

        // A missing file in a missing directory is created private, directory first.
        let nested = tmp.path().join("data").join("new.db");
        assert!(pre_create(&nested, "tracking").expect("a new file is created"));
        assert_eq!(mode_bits(&nested), 0o600);
        assert_eq!(mode_bits(nested.parent().expect("has a parent")), 0o700);

        // A file that is there, whoever made it, is not ours to chmod.
        let existing = tmp.path().join("existing.db");
        std::fs::write(&existing, b"").expect("file should be written");
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o644))
            .expect("chmod should succeed");
        assert!(!pre_create(&existing, "tracking").expect("an existing file is not an error"));
        assert_eq!(mode_bits(&existing), 0o644);
    }
}
