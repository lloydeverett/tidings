//! The SQLite Backend: one database per Area, in that Area's directory, in WAL mode.
//!
//! Each database has a `files` table holding each File's Path, the Path's letter-case fold, its
//! contents, when it was last modified and its Revision. A Commit is one write transaction, begun
//! with `BEGIN IMMEDIATE` so that it holds the database's write lock from the start. Inside it,
//! the rules every Backend shares ([`CommitRequest::plan`]) work out what the Commit changes,
//! reading the Area through the same transaction, before anything is written. The fold is
//! indexed, so the letter-case check looks up only the names a Commit writes.
//!
//! The Store reads and commits through one connection per Area. A Snapshot is a read transaction
//! on a connection of its own, which in WAL mode neither waits for Commits nor holds them up.
//! Every call into SQLite blocks, so each runs on tokio's blocking threads.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params};

use super::{AreaState, CommitOutcome, CommitRequest, Planned};
use crate::app::AppIdentity;
use crate::area::PerArea;
use crate::path::{letter_case_fold, letter_case_fold_unicode_versions, range_under};
use crate::{Area, Error, File, Path, Prefix, PrefixRevision, Result, Revision, Stat};

/// How to open a Store on SQLite, with [`Store::open_sqlite`](crate::Store::open_sqlite).
///
/// `SqliteOptions::default()` puts each Area's database in the platform's standard directory for
/// the app.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SqliteOptions {
    root_override: Option<PathBuf>,
}

impl SqliteOptions {
    /// A Root override: the Areas go in `config`, `data` and `cache` directories under `root`,
    /// instead of the standard directories the App identity picks. It is an override, for tests
    /// and unusual installations: apps normally leave it unset.
    pub fn root_override(mut self, root: impl Into<PathBuf>) -> SqliteOptions {
        self.root_override = Some(root.into());
        self
    }
}

/// Each step brings a database's schema up by one version, starting from 0, a new database. The
/// version is kept in SQLite's `user_version`. Later versions of tidings add steps at the end, and
/// never change one.
const MIGRATIONS: &[&str] = &["
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
"];

/// The name in `meta` of the versions of the Unicode data the folds in `files` were made with.
const FOLD_UNICODE_VERSIONS: &str = "fold_unicode_versions";

#[derive(Debug)]
pub(crate) struct SqliteBackend {
    areas: PerArea<Database>,
}

/// One Area's database, and the connection the Store reads and commits through.
#[derive(Debug)]
struct Database {
    path: PathBuf,
    connection: AreaConnection,
}

/// A connection to one Area's database, for async code. Each call has the connection to itself,
/// on one of tokio's blocking threads.
#[derive(Debug)]
pub(crate) struct AreaConnection(Arc<Mutex<Connection>>);

/// A Snapshot is a connection of its own, in a read transaction. Dropping it closes the
/// connection, which ends the transaction.
pub(crate) type SqliteSnapshot = AreaConnection;

impl SqliteBackend {
    /// Opens each Area's database, creating it and its directory if they don't exist.
    pub(crate) async fn open(app: &AppIdentity, options: SqliteOptions) -> Result<SqliteBackend> {
        let directories = app.area_directories(options.root_override.as_deref())?;
        let areas = off_runtime(move || {
            PerArea::try_from_fn(|area| {
                let directory = directories.get(area);
                std::fs::create_dir_all(directory).map_err(Error::backend)?;
                let path = directory.join(format!("{}.sqlite3", area.name()));
                let connection = AreaConnection::new(open_database(&path)?);
                Ok(Database { path, connection })
            })
        })
        .await?;
        Ok(SqliteBackend { areas })
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

    /// Works out what the Commit changes, then changes it, in one write transaction.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let connection = &self.areas.get(request.staged.area).connection;
        connection
            .call(move |connection| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(Error::backend)?;
                let plan = request.plan(&AreaInDatabase(&transaction))?;
                let outcome = plan.apply(|path, planned| {
                    apply(&transaction, path, planned).map_err(Error::backend)
                })?;
                transaction.commit().map_err(Error::backend)?;
                Ok(outcome)
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
/// [`letter_case_fold`] uses now, it makes them again.
fn open_database(path: &std::path::Path) -> Result<Connection> {
    let mut connection = Connection::open(path).map_err(Error::backend)?;
    let mode: String = connection
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(Error::backend)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::backend(format!("SQLite would not use WAL mode, only {mode:?}")));
    }
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
    transaction.commit().map_err(Error::backend)?;
    Ok(connection)
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
