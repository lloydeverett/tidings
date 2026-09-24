//! The SQLite Backend: one database per Area, in that Area's directory, in WAL mode.
//!
//! Each database has a `files` table holding each File's Path, the Path's letter-case fold, its
//! contents, when it was last modified and its Revision. A Commit is one write transaction, begun
//! with `BEGIN IMMEDIATE` so that it holds the database's write lock from the start. Inside it,
//! the rules every Backend shares ([`CommitRequest::plan`]) work out what the Commit changes,
//! reading the Area through the same transaction, before anything is written. The fold is
//! indexed, so the letter-case check looks up only the names a Commit writes.
//!
//! Each Commit that changes something is also appended to the database's change log, in the same
//! transaction: the Store instance that made it, and each Path it changed or removed. That is how
//! a Store sees the Commits other Stores make to the same databases, which may be in other
//! processes:
//! - **Polling.** Every poll interval, the Store checks each database's `data_version`, which
//!   changes when another connection commits to it, and if it changed, reads the log since the
//!   last Commit it recorded.
//! - **Committing.** A Commit first reads the log since then too, inside its write transaction. So
//!   other Stores' Commits reach the Change feed before this one, in the order they were applied.
//!   The Store layer records both, and polling, in turn under its `commit_order` lock.
//! - **Origin.** A Store's own Commits in the log are local, and are normally recorded straight
//!   from the Commit, which moves the Store past them in the log. Every other Store's are external.
//! - **Where reading starts.** At the end of the log when the Store was opened, so Commits made
//!   before then aren't reported.
//! - **Pruning.** Each Commit removes the Commits in the log that are older than the retention
//!   (10 minutes, by their timestamps, so by the wall clock), and records how far the log was
//!   pruned. A Store that finds it was pruned past the last Commit it read has missed Changes, and
//!   sends a Resync for the Area. Only a Store that stopped for longer than the retention, while
//!   others committed, is behind by that much, so the log stays small without keeping track of
//!   which Stores are open.
//!
//! The Store reads and commits through one connection per Area. A Snapshot is a read transaction
//! on a connection of its own, which in WAL mode neither waits for Commits nor holds them up.
//! Every call into SQLite blocks, so each runs on tokio's blocking threads.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params};

use super::{AreaState, CommitOutcome, CommitRequest, Observed, Planned, RawChange};
use crate::app::AppIdentity;
use crate::area::PerArea;
use crate::path::{letter_case_fold, letter_case_fold_unicode_versions, range_under};
use crate::{
    Area, ChangeKind, Error, File, Origin, Path, Prefix, PrefixRevision, Result, Revision, Stat,
};

/// How to open a Store on SQLite, with [`Store::open_sqlite`](crate::Store::open_sqlite).
///
/// `SqliteOptions::default()` puts each Area's database in the platform's standard directory for
/// the app, and checks for other processes' Commits every 100 ms.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SqliteOptions {
    root_override: Option<PathBuf>,
    poll_interval: Duration,
    change_log_retention: Duration,
}

impl Default for SqliteOptions {
    fn default() -> SqliteOptions {
        SqliteOptions {
            root_override: None,
            poll_interval: Duration::from_millis(100),
            change_log_retention: Duration::from_secs(10 * 60),
        }
    }
}

impl SqliteOptions {
    /// A Root override: the Areas go in `config`, `data` and `cache` directories under `root`,
    /// instead of the standard directories the App identity picks. It is an override, for tests
    /// and unusual installations: apps normally leave it unset.
    pub fn root_override(mut self, root: impl Into<PathBuf>) -> SqliteOptions {
        self.root_override = Some(root.into());
        self
    }

    /// How often the Store checks for Commits that other processes made to its databases, which
    /// arrive on its Change feed as external Changes. A shorter interval reports them sooner, and
    /// wakes the Store more often.
    pub fn poll_interval(mut self, interval: Duration) -> SqliteOptions {
        self.poll_interval = interval;
        self
    }

    /// How long this Store's Commits keep other Stores' Commits in the change log, for Stores
    /// that haven't read them yet. For tidings' own tests, which can't wait the 10 minutes it is
    /// otherwise to see a Store miss Commits.
    #[cfg(feature = "testing")]
    pub fn change_log_retention(mut self, retention: Duration) -> SqliteOptions {
        self.change_log_retention = retention;
        self
    }
}

/// Each step brings a database's schema up by one version, starting from 0, a new database. The
/// version is kept in SQLite's `user_version`. Later versions of tidings add steps at the end, and
/// never change one.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE files (
        path TEXT PRIMARY KEY NOT NULL,
        -- The Path's letter-case fold, made with the Unicode data `meta` names.
        fold TEXT NOT NULL,
        contents TEXT NOT NULL,
        modified_second INTEGER NOT NULL,
        modified_nanosecond INTEGER NOT NULL,
        revision BLOB NOT NULL
    );
    CREATE INDEX files_by_fold ON files (fold, path);
    -- What tidings keeps about the database itself.
    CREATE TABLE meta (
        name TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL
    );
",
    "
    -- Every Commit that changed something, in the order they were made, so that each Store on the
    -- database sees the others' Commits. Pruned by age.
    CREATE TABLE change_log (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        -- The Store instance that made the Commit (`last_store` in `meta` when it was opened).
        store INTEGER NOT NULL,
        -- The Commit's timestamp, in microseconds since the Unix epoch.
        committed_microsecond INTEGER NOT NULL
    );
    CREATE INDEX change_log_by_time ON change_log (committed_microsecond);
    -- Each Path a Commit in `change_log` changed, and whether it removed it.
    CREATE TABLE change_log_paths (
        commit_id INTEGER NOT NULL,
        path TEXT NOT NULL,
        removed INTEGER NOT NULL,
        PRIMARY KEY (commit_id, path)
    ) WITHOUT ROWID;
    INSERT INTO meta (name, value) VALUES ('last_store', 0), ('change_log_pruned_through', 0);
",
];

/// The name in `meta` of the versions of the Unicode data the folds in `files` were made with.
const FOLD_UNICODE_VERSIONS: &str = "fold_unicode_versions";

/// How long a Commit waits for another process's Commit to the same database to finish, before it
/// fails.
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) struct SqliteBackend {
    areas: PerArea<Database>,
    /// How long Commits are kept in the change log.
    change_log_retention: Duration,
}

/// One Area's database, the connection the Store reads and commits through, and how far the
/// Store has read the database's change log.
#[derive(Debug)]
struct Database {
    path: PathBuf,
    connection: AreaConnection,
    log: Arc<Mutex<LogReader>>,
}

/// How far a Store has read an Area's change log. It is used with the Store's connection locked,
/// and moved on under the Store layer's `commit_order` lock, which the Store holds until what was
/// read is recorded on the Change feed.
#[derive(Debug)]
struct LogReader {
    /// This Store, as the log names the Commits it made: a number no other Store opened on the
    /// database has had.
    store: i64,
    /// The last Commit in the log that this Store has read.
    seen: i64,
    /// The database's `data_version` when the poller last checked it. It changes when another
    /// connection commits to the database.
    data_version: i64,
}

/// Notices the Commits other Stores make to a Store's databases, by polling them.
#[derive(Debug)]
pub(crate) struct SqlitePoller {
    areas: PerArea<(AreaConnection, Arc<Mutex<LogReader>>)>,
    interval: Duration,
}

/// A connection to one Area's database, for async code. Each call has the connection to itself,
/// on one of tokio's blocking threads.
#[derive(Debug, Clone)]
pub(crate) struct AreaConnection(Arc<Mutex<Connection>>);

/// A Snapshot is a connection of its own, in a read transaction. Dropping it closes the
/// connection, which ends the transaction.
pub(crate) type SqliteSnapshot = AreaConnection;

impl SqliteBackend {
    /// Opens each Area's database, creating it and its directory if they don't exist. Gives the
    /// Backend, and the poller that notices other Stores' Commits from the end of each change log
    /// as it is now.
    pub(crate) async fn open(
        app: &AppIdentity,
        options: SqliteOptions,
    ) -> Result<(SqliteBackend, SqlitePoller)> {
        let directories = app.area_directories(options.root_override.as_deref())?;
        let areas = off_runtime(move || {
            PerArea::try_from_fn(|area| {
                let directory = directories.get(area);
                std::fs::create_dir_all(directory).map_err(Error::backend)?;
                let path = directory.join(format!("{}.sqlite3", area.name()));
                let (connection, log) = open_database(&path)?;
                let connection = AreaConnection::new(connection);
                Ok(Database { path, connection, log: Arc::new(Mutex::new(log)) })
            })
        })
        .await?;
        let polled = PerArea::try_from_fn(|area| {
            let database = areas.get(area);
            Ok((database.connection.clone(), Arc::clone(&database.log)))
        })?;
        let poller = SqlitePoller { areas: polled, interval: options.poll_interval };
        Ok((SqliteBackend { areas, change_log_retention: options.change_log_retention }, poller))
    }

    pub(crate) async fn read(&self, area: Area, path: &Path) -> Result<Option<File>> {
        self.areas.get(area).connection.read(path).await
    }

    pub(crate) async fn stat(&self, area: Area, path: &Path) -> Result<Option<Stat>> {
        self.areas.get(area).connection.stat(path).await
    }

    pub(crate) async fn list(&self, area: Area, prefix: &Prefix) -> Result<Vec<Path>> {
        self.areas.get(area).connection.list(prefix).await
    }

    pub(crate) async fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        let prefix = prefix.clone();
        let connection = &self.areas.get(area).connection;
        connection
            .call(move |connection| {
                let files = AreaInDatabase(connection).revisions_under(&prefix)?;
                Ok(PrefixRevision::of(area, prefix, files))
            })
            .await
    }

    /// Begins a read transaction on a new connection to the Area's database.
    pub(crate) async fn snapshot(&self, area: Area) -> Result<SqliteSnapshot> {
        let path = self.areas.get(area).path.clone();
        off_runtime(move || {
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let connection = Connection::open_with_flags(&path, flags).map_err(Error::backend)?;
            // A plain `BEGIN` fixes what the transaction sees only at its first read, so this
            // reads at once: otherwise a Commit made before the first read would show.
            let begin = connection.execute_batch("BEGIN").and_then(|()| {
                connection.query_row("SELECT EXISTS (SELECT 1 FROM files)", [], |_| Ok(()))
            });
            begin.map_err(Error::backend)?;
            Ok(AreaConnection::new(connection))
        })
        .await
    }

    /// Works out what the Commit changes, then changes it, in one write transaction. The
    /// transaction also reads what other Stores committed before it, appends the Commit to the
    /// change log and prunes the log.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let database = self.areas.get(request.staged.area);
        let log = Arc::clone(&database.log);
        let retention = self.change_log_retention;
        database
            .connection
            .call(move |connection| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(Error::backend)?;
                let mut log = log.lock().unwrap();
                // Moved on only if the Commit is: otherwise the poller reads them again.
                let mut seen = log.seen;
                let before =
                    read_log(&transaction, log.store, &mut seen).map_err(Error::backend)?;
                let timestamp = request.timestamp;
                let plan = request.plan(&AreaInDatabase(&transaction))?;
                let mut outcome = plan.apply(|path, planned| {
                    apply(&transaction, path, planned).map_err(Error::backend)
                })?;
                if !outcome.changes.is_empty() {
                    seen = append_to_log(&transaction, log.store, timestamp, &outcome.changes)
                        .and_then(|appended| {
                            prune_log(&transaction, timestamp, retention).map(|()| appended)
                        })
                        .map_err(Error::backend)?;
                }
                transaction.commit().map_err(Error::backend)?;
                log.seen = seen;
                outcome.before = before;
                Ok(outcome)
            })
            .await
    }
}

impl SqlitePoller {
    /// Waits for the poll interval, then gives each Area whose database another connection may
    /// have committed to since the last time. An Area whose database can't be checked is given
    /// too, so that reading its log shows what is wrong.
    pub(crate) async fn wait(&self) -> Vec<Area> {
        tokio::time::sleep(self.interval).await;
        let mut changed = Vec::new();
        for (area, (connection, log)) in self.areas.iter() {
            let log = Arc::clone(log);
            let checked = connection
                .call(move |connection| {
                    let now = data_version(connection).map_err(Error::backend)?;
                    let mut log = log.lock().unwrap();
                    Ok(std::mem::replace(&mut log.data_version, now) != now)
                })
                .await;
            if checked.unwrap_or(true) {
                changed.push(area);
            }
        }
        changed
    }

    /// Reads what was committed to `area` since this Store last read its change log, and moves
    /// the Store past it. The Store layer calls it in turn with Commits, under `commit_order`, and
    /// records what it gives before its turn ends.
    pub(crate) async fn read(&self, area: Area) -> Result<Vec<Observed>> {
        let (connection, log) = self.areas.get(area);
        let log = Arc::clone(log);
        connection
            .call(move |connection| {
                // One read transaction, so that how far the log was pruned and what is in it are
                // seen as they stood at one moment.
                let transaction = connection.transaction().map_err(Error::backend)?;
                let mut log = log.lock().unwrap();
                let mut seen = log.seen;
                let observed =
                    read_log(&transaction, log.store, &mut seen).map_err(Error::backend)?;
                transaction.commit().map_err(Error::backend)?;
                log.seen = seen;
                Ok(observed)
            })
            .await
    }
}

impl AreaConnection {
    fn new(connection: Connection) -> AreaConnection {
        AreaConnection(Arc::new(Mutex::new(connection)))
    }

    pub(crate) async fn read(&self, path: &Path) -> Result<Option<File>> {
        let path = path.clone();
        self.call(move |connection| read(connection, &path)).await
    }

    pub(crate) async fn stat(&self, path: &Path) -> Result<Option<Stat>> {
        let path = path.clone();
        self.call(move |connection| stat(connection, &path)).await
    }

    pub(crate) async fn list(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        let prefix = prefix.clone();
        self.call(move |connection| AreaInDatabase(connection).paths_under(&prefix)).await
    }

    /// Runs `call` with the connection, on a blocking thread.
    async fn call<T: Send + 'static>(
        &self,
        call: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let connection = Arc::clone(&self.0);
        off_runtime(move || call(&mut connection.lock().unwrap())).await
    }
}

/// Runs `call` on one of tokio's blocking threads, so that it doesn't hold up the async runtime.
async fn off_runtime<T: Send + 'static>(
    call: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tokio::task::spawn_blocking(call).await {
        Ok(result) => result,
        Err(error) => match error.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // The runtime is shutting down.
            Err(error) => Err(Error::backend(error)),
        },
    }
}

/// Opens the database at `path`, creating it if it doesn't exist, in WAL mode, with its schema
/// brought up to date. If the folds in it were made with other Unicode data than
/// [`letter_case_fold`] uses now, it makes them again. Gives the connection, and a new Store
/// instance's reader of the change log, at its end.
fn open_database(path: &std::path::Path) -> Result<(Connection, LogReader)> {
    let mut connection = Connection::open(path).map_err(Error::backend)?;
    connection.busy_timeout(BUSY_TIMEOUT).map_err(Error::backend)?;
    let mode: String = connection
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(Error::backend)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::backend(format!("SQLite would not use WAL mode, only {mode:?}")));
    }
    // Before the end of the log is read, so that a Commit made after that changes it.
    let data_version = data_version(&connection).map_err(Error::backend)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(Error::backend)?;
    let version: u32 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(Error::backend)?;
    if version as usize > MIGRATIONS.len() {
        return Err(Error::backend(format!(
            "the database's schema is version {version}, from a later version of tidings",
        )));
    }
    bring_up_to_date(&transaction, version as usize).map_err(Error::backend)?;
    let log = new_log_reader(&transaction, data_version).map_err(Error::backend)?;
    transaction.commit().map_err(Error::backend)?;
    Ok((connection, log))
}

/// Brings a database with the schema `version` up to date, and makes its folds again if they were
/// made with other Unicode data.
fn bring_up_to_date(connection: &Connection, version: usize) -> rusqlite::Result<()> {
    for migration in &MIGRATIONS[version..] {
        connection.execute_batch(migration)?;
    }
    connection.pragma_update(None, "user_version", MIGRATIONS.len() as u32)?;

    let versions = letter_case_fold_unicode_versions();
    let stored: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE name = ?1", [FOLD_UNICODE_VERSIONS], |row| {
            row.get(0)
        })
        .optional()?;
    if stored.as_ref() != Some(&versions) {
        fold_again(connection)?;
        connection.execute(
            "INSERT INTO meta (name, value) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value",
            params![FOLD_UNICODE_VERSIONS, versions],
        )?;
    }
    Ok(())
}

/// Makes the fold of every Path again, after the Unicode data behind it changed. If two Paths now
/// fold the same, both stay, as the Commit that wrote them allowed, but no Commit can add a name
/// that clashes with either.
fn fold_again(connection: &Connection) -> rusqlite::Result<()> {
    let mut paths = connection.prepare("SELECT path, fold FROM files")?;
    let mut update = connection.prepare("UPDATE files SET fold = ?2 WHERE path = ?1")?;
    let mut rows = paths.query([])?;
    while let Some(row) = rows.next()? {
        let (path, fold): (String, String) = (row.get(0)?, row.get(1)?);
        let folded_again = letter_case_fold(&path);
        if folded_again != fold {
            update.execute(params![path, folded_again])?;
        }
    }
    Ok(())
}

/// The database's `data_version`, as `connection` sees it: it changes whenever another connection
/// commits to the database.
fn data_version(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("PRAGMA data_version", [], |row| row.get(0))
}

/// Registers a new Store instance on the database, reading the change log from its end. The
/// numbers in `meta` are kept as text, as everything there is.
fn new_log_reader(connection: &Connection, data_version: i64) -> rusqlite::Result<LogReader> {
    let store = connection.query_row(
        "UPDATE meta SET value = value + 1 WHERE name = 'last_store'
         RETURNING CAST(value AS INTEGER)",
        [],
        |row| row.get(0),
    )?;
    let seen = connection.query_row(
        "SELECT max(
             (SELECT CAST(value AS INTEGER) FROM meta WHERE name = 'change_log_pruned_through'),
             (SELECT coalesce(max(id), 0) FROM change_log)
         )",
        [],
        |row| row.get(0),
    )?;
    Ok(LogReader { store, seen, data_version })
}

/// Reads the change log after the Commit `seen`, and moves `seen` to the last Commit read. A Commit
/// is local if the Store instance `store` made it. If the log was pruned past `seen`, what was
/// pruned is missed, which comes first. It reads twice, so `connection` must be in a transaction.
fn read_log(
    connection: &Connection,
    store: i64,
    seen: &mut i64,
) -> rusqlite::Result<Vec<Observed>> {
    let mut observed = Vec::new();
    let pruned: i64 = connection
        .prepare_cached(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE name = 'change_log_pruned_through'",
        )?
        .query_row([], |row| row.get(0))?;
    if pruned > *seen {
        observed.push(Observed::Missed);
        *seen = pruned;
    }
    let mut statement = connection.prepare_cached(
        "SELECT change_log.id, change_log.store, paths.path, paths.removed
         FROM change_log JOIN change_log_paths AS paths ON paths.commit_id = change_log.id
         WHERE change_log.id > ?1
         ORDER BY change_log.id, paths.path",
    )?;
    let mut rows = statement.query([*seen])?;
    while let Some(row) = rows.next()? {
        let (id, by, path, removed): (i64, i64, String, bool) =
            (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
        let kind = if removed { ChangeKind::Removed } else { ChangeKind::Changed };
        let change = RawChange { path: Path::stored(path), kind };
        match observed.last_mut() {
            Some(Observed::Commit { changes, .. }) if id == *seen => changes.push(change),
            _ => {
                let origin = if by == store { Origin::Local } else { Origin::External };
                observed.push(Observed::Commit { origin, changes: vec![change] });
                *seen = id;
            }
        }
    }
    Ok(observed)
}

/// Appends a Commit that the Store instance `store` made at `timestamp` to the change log, and
/// gives its place in the log.
fn append_to_log(
    connection: &Connection,
    store: i64,
    timestamp: Timestamp,
    changes: &[RawChange],
) -> rusqlite::Result<i64> {
    connection
        .prepare_cached("INSERT INTO change_log (store, committed_microsecond) VALUES (?1, ?2)")?
        .execute(params![store, timestamp.as_microsecond()])?;
    let id = connection.last_insert_rowid();
    let mut insert = connection.prepare_cached(
        "INSERT INTO change_log_paths (commit_id, path, removed) VALUES (?1, ?2, ?3)",
    )?;
    for RawChange { path, kind } in changes {
        insert.execute(params![id, path.as_str(), *kind == ChangeKind::Removed])?;
    }
    Ok(id)
}

/// Removes the Commits in the change log that are older than `retention` at `now`, with every
/// Commit before them, and records how far the log was pruned.
fn prune_log(connection: &Connection, now: Timestamp, retention: Duration) -> rusqlite::Result<()> {
    let retention = i64::try_from(retention.as_micros()).unwrap_or(i64::MAX);
    let before = now.as_microsecond().saturating_sub(retention);
    let last: Option<i64> = connection
        .prepare_cached("SELECT max(id) FROM change_log WHERE committed_microsecond < ?1")?
        .query_row([before], |row| row.get(0))?;
    let Some(last) = last else { return Ok(()) };
    connection
        .prepare_cached("DELETE FROM change_log_paths WHERE commit_id <= ?1")?
        .execute([last])?;
    connection.prepare_cached("DELETE FROM change_log WHERE id <= ?1")?.execute([last])?;
    connection
        .prepare_cached("UPDATE meta SET value = ?1 WHERE name = 'change_log_pruned_through'")?
        .execute([last])?;
    Ok(())
}

fn read(connection: &Connection, path: &Path) -> Result<Option<File>> {
    let mut statement = connection
        .prepare_cached(
            "SELECT contents, modified_second, modified_nanosecond, revision FROM files
             WHERE path = ?1",
        )
        .map_err(Error::backend)?;
    let file = statement
        .query_row([path.as_str()], |row| {
            Ok(File::new(path.clone(), row.get(0)?, stat_at(row, 1)?))
        })
        .optional();
    file.map_err(Error::backend)
}

fn stat(connection: &Connection, path: &Path) -> Result<Option<Stat>> {
    let mut statement = connection
        .prepare_cached(
            "SELECT modified_second, modified_nanosecond, revision FROM files WHERE path = ?1",
        )
        .map_err(Error::backend)?;
    statement.query_row([path.as_str()], |row| stat_at(row, 0)).optional().map_err(Error::backend)
}

/// The Stat in the columns of `row` from `first` on: when the File was last modified, then its
/// Revision.
fn stat_at(row: &Row, first: usize) -> rusqlite::Result<Stat> {
    let (second, nanosecond) = (row.get(first)?, row.get(first + 1)?);
    let modified = Timestamp::new(second, nanosecond).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            first,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    Ok(Stat::new(modified, Revision::from_bytes(row.get(first + 2)?)))
}

/// Does what is `planned` to the File at `path`.
fn apply(connection: &Connection, path: &Path, planned: Planned) -> rusqlite::Result<()> {
    let Planned::Write { contents, stat } = planned else {
        let mut delete = connection.prepare_cached("DELETE FROM files WHERE path = ?1")?;
        delete.execute([path.as_str()])?;
        return Ok(());
    };
    let mut upsert = connection.prepare_cached(
        "INSERT INTO files (path, fold, contents, modified_second, modified_nanosecond, revision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (path) DO UPDATE SET
             contents = excluded.contents,
             modified_second = excluded.modified_second,
             modified_nanosecond = excluded.modified_nanosecond,
             revision = excluded.revision",
    )?;
    let modified = stat.modified();
    upsert.execute(params![
        path.as_str(),
        letter_case_fold(path.as_str()),
        contents,
        modified.as_second(),
        modified.subsec_nanosecond(),
        stat.revision().to_bytes(),
    ])?;
    Ok(())
}

/// An Area's database as a connection sees it, inside a transaction or not.
struct AreaInDatabase<'a>(&'a Connection);

impl AreaInDatabase<'_> {
    /// Gives what `each` makes of every row of `sql`, which takes the parameters `params`.
    fn rows<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        mut each: impl FnMut(&Row) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        let rows = || -> rusqlite::Result<Vec<T>> {
            let mut statement = self.0.prepare_cached(sql)?;
            let mut rows = statement.query(params)?;
            let mut found = Vec::new();
            while let Some(row) = rows.next()? {
                found.push(each(row)?);
            }
            Ok(found)
        };
        rows().map_err(Error::backend)
    }

    /// Selects `columns` of every File under `prefix`, in order of Path, and gives what `each`
    /// makes of each row.
    fn under<T>(
        &self,
        prefix: &Prefix,
        columns: &str,
        each: impl FnMut(&Row) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        match prefix.as_str().strip_suffix('/') {
            None => self.rows(&format!("SELECT {columns} FROM files ORDER BY path"), [], each),
            Some(without_slash) => {
                let under = range_under(without_slash);
                let sql = format!(
                    "SELECT {columns} FROM files WHERE path >= ?1 AND path < ?2 ORDER BY path"
                );
                self.rows(&sql, [&under.start, &under.end], each)
            }
        }
    }
}

impl AreaState for AreaInDatabase<'_> {
    fn revision(&self, path: &Path) -> Result<Option<Revision>> {
        let mut statement = self
            .0
            .prepare_cached("SELECT revision FROM files WHERE path = ?1")
            .map_err(Error::backend)?;
        let revision = statement.query_row([path.as_str()], |row| row.get(0)).optional();
        Ok(revision.map_err(Error::backend)?.map(Revision::from_bytes))
    }

    fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>> {
        self.under(prefix, "path, revision", |row| {
            Ok((Path::stored(row.get(0)?), Revision::from_bytes(row.get(1)?)))
        })
    }

    fn paths_under(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        self.under(prefix, "path", |row| Ok(Path::stored(row.get(0)?)))
    }

    fn paths_named_like(&self, name: &str, fold: &str) -> Result<Vec<Path>> {
        let folds_under = range_under(fold);
        // The Paths with the name `name`: `name` itself, and the Paths under it if it is a
        // Prefix. For a File's name, the range is empty.
        let under_name = match name.strip_suffix('/') {
            Some(without_slash) => range_under(without_slash),
            None => name.to_owned()..name.to_owned(),
        };
        self.rows(
            "SELECT path FROM files
             WHERE (fold = ?1 OR (fold >= ?2 AND fold < ?3))
             AND NOT (path = ?4 OR (path >= ?5 AND path < ?6))",
            [fold, &folds_under.start, &folds_under.end, name, &under_name.start, &under_name.end],
            |row| Ok(Path::stored(row.get(0)?)),
        )
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::FOLD_UNICODE_VERSIONS;
    use crate::{AppIdentity, Area, Error, InvalidPathReason, SqliteOptions, Staging, Store};

    /// New Unicode data can change how Paths fold, which leaves the folds made with the old data
    /// stale. A test can't change the Unicode data, so this stands in for it: it gives every
    /// stored fold a value no Path folds to, as if made with other data. Opening the Store again
    /// makes the folds again, so the letter-case check still refuses a clash with a stored Path.
    ///
    /// This is the one test that touches a database directly, because the public API can't
    /// make a fold stale. The spec's Testing Decisions allow it as their third exception.
    #[tokio::test]
    async fn opening_folds_again_what_was_folded_with_other_unicode_data() {
        let root = tempfile::tempdir().unwrap();
        let app = AppIdentity::new("tidings tests", "tidings", "org");
        let open = async || {
            let options = SqliteOptions::default().root_override(root.path());
            Store::open_sqlite(&app, options).await.unwrap()
        };
        let (store, _feed) = open().await;
        let mut staging = Staging::new(Area::Config);
        staging.write("themes/dark.toml", "").unwrap();
        store.commit(staging).await.unwrap();
        drop(store);

        let database = Connection::open(root.path().join("config/config.sqlite3")).unwrap();
        database.execute("UPDATE files SET fold = 'stale'", []).unwrap();
        let versions = "UPDATE meta SET value = 'other' WHERE name = ?1";
        database.execute(versions, [FOLD_UNICODE_VERSIONS]).unwrap();
        drop(database);

        let (store, _feed) = open().await;
        let mut staging = Staging::new(Area::Config);
        staging.write("Themes/light.toml", "").unwrap();
        match store.commit(staging).await {
            Err(Error::InvalidPath { reason: InvalidPathReason::LetterCaseClash, .. }) => {}
            other => panic!("expected a letter-case clash, got {other:?}"),
        }
    }
}
