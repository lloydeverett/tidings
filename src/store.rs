use std::sync::Arc;

use jiff::Timestamp;
use tokio::sync::Mutex;

#[cfg(feature = "testing")]
use crate::ChangeKind;
#[cfg(feature = "testing")]
use crate::backend::RawChange;
#[cfg(feature = "sqlite")]
use crate::backend::sqlite::SqliteBackend;
use crate::backend::{Backend, CommitRequest, memory::MemoryBackend};
use crate::change::{self, FeedSender};
#[cfg(feature = "sqlite")]
use crate::{AppIdentity, SqliteOptions};
use crate::{
    Area, ChangeFeed, Committed, File, IntoPath, IntoPrefix, Origin, Path, PrefixRevision, Result,
    Snapshot, Staging, Stat,
};

/// What an application opens to reach its Files: all three Areas, held by one Backend.
///
/// A Store is a cheap handle that can be shared between tasks and threads: clones share the same
/// Files and the same Change feed. The Change feed ends once every clone has been dropped.
#[derive(Debug, Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

/// What every clone of a Store shares. The Store layer here does everything that is the same for
/// every Backend, and calls into the Backend for the rest.
///
/// The Change feed ends when this is dropped, which is when the last Store handle goes. So a
/// task that runs for as long as the Store is open (watching for changes, polling another
/// process's Commits) must not hold an `Arc<Inner>`, or the feed would never end: it holds only
/// what it needs, and `Inner` stops it when dropped. A task that finishes on its own, such as a
/// Commit left to complete in the background, may hold one, which keeps the feed open until its
/// Changes are recorded. Such a Commit must also carry its `commit_order` guard into the task (an
/// owned guard, from an `Arc<Mutex<()>>`), or a later Commit could be recorded before it.
#[derive(Debug)]
struct Inner {
    backend: Backend,
    feed: FeedSender,
    /// Held from before a Commit is applied until its Changes are recorded, so that Commits reach
    /// the Change feed in the order they were applied. Otherwise two Commits to the same Path
    /// could be recorded the other way round, and the merged Change would have the wrong kind.
    commit_order: Mutex<()>,
}

impl Drop for Inner {
    /// The last Store handle is gone, so nothing more can be committed: the Change feed ends.
    fn drop(&mut self) {
        self.feed.end();
    }
}

impl Store {
    /// Opens a Store that keeps its Files in memory, for tests and short-lived data. Returns the
    /// Store together with its one Change feed.
    pub fn open_memory() -> (Store, ChangeFeed) {
        Store::open(Backend::Memory(MemoryBackend::default()))
    }

    /// Opens a Store that keeps each Area in a SQLite database of its own, in the Area's standard
    /// directory for `app`, or under the Root override in `options`. Opening creates the
    /// databases and their directories if they don't exist. Returns the Store together with its
    /// one Change feed.
    ///
    /// Gives [`Error::Backend`](crate::Error::Backend) if a database can't be opened or created.
    #[cfg(feature = "sqlite")]
    pub async fn open_sqlite(
        app: &AppIdentity,
        options: SqliteOptions,
    ) -> Result<(Store, ChangeFeed)> {
        let backend = SqliteBackend::open(app, options).await?;
        Ok(Store::open(Backend::Sqlite(backend)))
    }

    fn open(backend: Backend) -> (Store, ChangeFeed) {
        let (feed, change_feed) = change::feed();
        let inner = Inner { backend, feed, commit_order: Mutex::new(()) };
        (Store { inner: Arc::new(inner) }, change_feed)
    }

    /// Reads the File at `path` in `area`, or gives `Ok(None)` if there is none.
    pub async fn read(&self, area: Area, path: impl IntoPath) -> Result<Option<File>> {
        let path = path.into_path()?;
        self.inner.backend.read(area, &path).await
    }

    /// Gives when the File at `path` in `area` was last modified and its Revision, without loading
    /// its contents, or `Ok(None)` if there is no File there.
    pub async fn stat(&self, area: Area, path: impl IntoPath) -> Result<Option<Stat>> {
        let path = path.into_path()?;
        self.inner.backend.stat(area, &path).await
    }

    /// Lists the Paths of the Files under `prefix` in `area`, in order. The empty Prefix lists the
    /// whole Area. Only the Paths are loaded, not the Files.
    pub async fn list(&self, area: Area, prefix: impl IntoPrefix) -> Result<Vec<Path>> {
        let prefix = prefix.into_prefix()?;
        self.inner.backend.list(area, &prefix).await
    }

    /// Gives the Prefix Revision of everything under `prefix` in `area`: which Files are there,
    /// and the Revision of each. Pass it to
    /// [`Staging::require_prefix`](crate::Staging::require_prefix) to make a Commit fail if any of
    /// them was added, removed or changed since. The empty Prefix covers the whole Area.
    ///
    /// Unlike [`list`](Self::list), it needs the Revision of every File under `prefix`. On the
    /// filesystem that means reading them all. The Prefix Revision keeps each Path and its
    /// Revision, so that a Conflict can name the Files that changed: holding one costs memory in
    /// proportion to the number of Files under `prefix`, which can be large for a Cache.
    pub async fn stat_prefix(&self, area: Area, prefix: impl IntoPrefix) -> Result<PrefixRevision> {
        let prefix = prefix.into_prefix()?;
        self.inner.backend.stat_prefix(area, &prefix).await
    }

    /// Whether this Store's Backend provides Snapshots. Memory and SQLite do. The filesystem
    /// doesn't, because other programs can change its Files while they are being read.
    pub fn supports_snapshots(&self) -> bool {
        self.inner.backend.supports_snapshots()
    }

    /// Takes a [`Snapshot`] of `area`: a view of it as it stands now, for reading several Files
    /// that belong together. Commits made afterwards don't show in it, and holding it doesn't hold
    /// them up.
    ///
    /// Gives [`Error::Unsupported`](crate::Error::Unsupported) if the Backend has no Snapshots.
    /// Check [`supports_snapshots`](Self::supports_snapshots) when the app starts.
    pub async fn snapshot(&self, area: Area) -> Result<Snapshot> {
        Ok(Snapshot::new(self.inner.backend.snapshot(area).await?))
    }

    /// Commits `staging`: applies all of its writes and deletes, or none of them. Prefix deletes
    /// cover the Files under the Prefix at this moment.
    ///
    /// Every File written gets the Commit's timestamp as its last-modified time. A write that
    /// wouldn't change the File's contents is left out, so the File keeps its time and no Change
    /// is sent for it. The Changes arrive on the Change feed in the same batch, and a Commit
    /// that changes nothing sends none. Commits reach the feed in the order they were applied.
    /// On success, gives the timestamp and the new Revisions.
    ///
    /// Nothing is written if the Commit fails:
    /// - [`Error::Conflict`](crate::Error::Conflict) if any of the Staging's Preconditions doesn't
    ///   hold, naming every Path where one fails;
    /// - [`Error::InvalidPath`](crate::Error::InvalidPath) with
    ///   [`LetterCaseClash`](crate::InvalidPathReason::LetterCaseClash) if a write would create a
    ///   Path that differs only in letter case from another Path in the Area.
    pub async fn commit(&self, staging: Staging) -> Result<Committed> {
        let area = staging.area();
        let _in_order = self.inner.commit_order.lock().await;
        // Taken in turn too, so Commits read the clock in the order they are applied. The wall
        // clock can step backwards, so their timestamps are in that order only while it doesn't.
        let timestamp = Timestamp::now();
        let request = CommitRequest { timestamp, staged: staging.into_staged() };
        let outcome = self.inner.backend.commit(request).await?;
        self.inner.feed.record(area, outcome.changes, Origin::Local);
        Ok(Committed::new(timestamp, outcome.revisions))
    }

    /// Records an external Change to `path` in `area` on the Change feed, as if another process
    /// had made it, without changing any File. For tidings' own tests of how the Store layer
    /// merges external Changes, since the memory Backend never observes any.
    #[cfg(feature = "testing")]
    pub fn inject_external_change(
        &self,
        area: Area,
        path: impl IntoPath,
        kind: ChangeKind,
    ) -> Result<()> {
        let change = RawChange { path: path.into_path()?, kind };
        self.inner.feed.record(area, vec![change], Origin::External);
        Ok(())
    }
}
