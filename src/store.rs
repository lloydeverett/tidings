use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use jiff::Timestamp;
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use tokio::task::AbortHandle;

#[cfg(feature = "testing")]
use crate::ChangeKind;
#[cfg(feature = "testing")]
use crate::backend::RawChange;
#[cfg(feature = "sqlite")]
use crate::backend::sqlite::{SqliteBackend, SqlitePoller};
use crate::backend::{Backend, CommitRequest, Observed, memory::MemoryBackend};
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
/// what it needs, and `Inner` stops it when dropped. A task that finishes on its own may hold one,
/// which keeps the feed open until it is done. A Commit the app stops waiting for continues in
/// such a task, holding its `commit_order` guard, so that it finishes and records its Changes.
#[derive(Debug)]
struct Inner {
    backend: Backend,
    feed: FeedSender,
    /// Held from before a Commit is applied until its Changes are recorded, so that Commits reach
    /// the Change feed in the order they were applied. Otherwise two Commits to the same Path
    /// could be recorded the other way round, and the merged Change would have the wrong kind.
    /// The task that follows other Stores' Commits holds it too, from reading them until they are
    /// recorded.
    commit_order: Arc<Mutex<()>>,
    /// The task that follows other Stores' Commits, if the Backend has one. It is stopped when
    /// the Store is dropped.
    following: Option<AbortHandle>,
}

/// A Commit that has started. It runs in place while the app waits for it. If the app stops
/// waiting, by dropping it, the rest of the Commit is handed to a task on the runtime, which
/// finishes it. So a task is spawned only for a Commit that is dropped, not for every Commit.
struct Started {
    /// The rest of the Commit, until it has finished.
    commit: Option<Pin<Box<dyn Future<Output = Result<Committed>> + Send>>>,
    runtime: Handle,
}

impl Future for Started {
    type Output = Result<Committed>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let commit = self.commit.as_mut().expect("a Commit is not polled once it has finished");
        let polled = panic::catch_unwind(AssertUnwindSafe(|| commit.as_mut().poll(cx)));
        match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(result)) => {
                self.commit = None;
                Poll::Ready(result)
            }
            // The Backend panicked. The Commit is over, so it is not handed on when dropped.
            Err(panic) => {
                self.commit = None;
                panic::resume_unwind(panic)
            }
        }
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(commit) = self.commit.take() {
            // Nobody waits for the result. The Commit records its Changes itself.
            self.runtime.spawn(async move {
                let _ = commit.await;
            });
        }
    }
}

impl Drop for Inner {
    /// The last Store handle is gone, so nothing more can be committed: the Change feed ends.
    fn drop(&mut self) {
        if let Some(following) = &self.following {
            following.abort();
        }
        self.feed.end();
    }
}

impl Store {
    /// Opens a Store that keeps its Files in memory, for tests and short-lived data. Returns the
    /// Store together with its one Change feed.
    pub fn open_memory() -> (Store, ChangeFeed) {
        Store::open(Backend::Memory(MemoryBackend::default()), |_, _| None)
    }

    /// Opens a Store that keeps each Area in a SQLite database of its own, in the Area's standard
    /// directory for `app`, or under the Root override in `options`. Opening creates the
    /// databases and their directories if they don't exist. Returns the Store together with its
    /// one Change feed.
    ///
    /// Other processes can open the same databases, and commit to them safely: Commits are
    /// applied one at a time. Every poll interval in `options`, the Store checks for their
    /// Commits, which arrive on the Change feed as external Changes, each Commit's in one batch.
    ///
    /// Gives [`Error::Backend`](crate::Error::Backend) if a database can't be opened or created.
    ///
    /// # Panics
    ///
    /// If it isn't called from within a tokio runtime.
    #[cfg(feature = "sqlite")]
    pub async fn open_sqlite(
        app: &AppIdentity,
        options: SqliteOptions,
    ) -> Result<(Store, ChangeFeed)> {
        let (backend, poller) = SqliteBackend::open(app, options).await?;
        Ok(Store::open(Backend::Sqlite(backend), |feed, commit_order| {
            Some(start_following_other_stores(poller, feed, commit_order))
        }))
    }

    /// Opens a Store on `backend`, with its Change feed. `follow` starts the task that follows
    /// other Stores' Commits, if the Backend has one, given the Store's end of the feed and its
    /// turn with Commits. The Store stops the task when it is dropped.
    fn open(
        backend: Backend,
        follow: impl FnOnce(&FeedSender, &Arc<Mutex<()>>) -> Option<AbortHandle>,
    ) -> (Store, ChangeFeed) {
        let (feed, change_feed) = change::feed();
        let commit_order = Arc::default();
        let following = follow(&feed, &commit_order);
        let inner = Inner { backend, feed, commit_order, following };
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
    ///
    /// A Commit can be cancelled, for example with a timeout, by dropping this future. If it is
    /// dropped while the Commit waits for the Commits before it to finish, the Commit never
    /// happens. Once the Commit has started, dropping this future hands the rest of it to a task
    /// on the tokio runtime, which finishes it: its Changes arrive on the Change feed as usual.
    /// If the runtime is shutting down, that task can't run: the Commit may then have been
    /// applied, or not, without its Changes being reported.
    ///
    /// # Panics
    ///
    /// If it isn't called from within a tokio runtime.
    pub async fn commit(&self, staging: Staging) -> Result<Committed> {
        let in_order = Arc::clone(&self.inner.commit_order).lock_owned().await;
        // The Commit starts here. It holds its turn and the Store, so if this future is dropped
        // from here on, the Commit still finishes in a task and its Changes are still recorded.
        // Dropped before here, while waiting its turn, it never happens.
        let inner = Arc::clone(&self.inner);
        let commit = async move {
            let _in_order = in_order;
            let area = staging.area();
            // Taken in turn too, so Commits read the clock in the order they are applied. The
            // wall clock can step backwards, so their timestamps are in that order only while it
            // doesn't.
            let timestamp = Timestamp::now();
            let request = CommitRequest { timestamp, staged: staging.into_staged() };
            let outcome = inner.backend.commit(request).await?;
            record_observed(&inner.feed, area, outcome.observed_before);
            inner.feed.record(area, outcome.changes, Origin::Local);
            Ok(Committed::new(timestamp, outcome.revisions))
        };
        Started { commit: Some(Box::pin(commit)), runtime: Handle::current() }.await
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

/// Records on `feed` what the Backend observed in `area`, in order: each Commit in one batch.
fn record_observed(feed: &FeedSender, area: Area, observed: Vec<Observed>) {
    for observed in observed {
        match observed {
            Observed::Commit { origin, changes } => feed.record(area, changes, origin),
            Observed::Missed => feed.resync(area),
        }
    }
}

/// Starts the task that follows other Stores' Commits to the SQLite databases, and gives the handle
/// that stops it.
///
/// If the task panics, their Changes stop arriving, so a second task, which waits for it, sends a
/// Resync for every Area. When the Store stops the task, it sends nothing.
#[cfg(feature = "sqlite")]
fn start_following_other_stores(
    poller: SqlitePoller,
    feed: &FeedSender,
    commit_order: &Arc<Mutex<()>>,
) -> AbortHandle {
    let following =
        tokio::spawn(follow_other_stores(poller, feed.clone(), Arc::clone(commit_order)));
    let stopping = following.abort_handle();
    let feed = feed.clone();
    tokio::spawn(async move {
        if let Err(ended) = following.await
            && ended.is_panic()
        {
            tracing::debug!("following other Stores' Commits panicked: {ended}");
            feed.resync_every_area();
        }
    });
    stopping
}

/// Records other Stores' Commits to the SQLite databases as the poller notices them, until the
/// Store is dropped and stops it. It holds only the Store's end of the Change feed and its turn
/// with Commits, not the Store, so that the feed still ends.
///
/// If an Area's change log can't be read, a Resync for the Area is sent, once until it can be
/// read again.
#[cfg(feature = "sqlite")]
async fn follow_other_stores(poller: SqlitePoller, feed: FeedSender, commit_order: Arc<Mutex<()>>) {
    let mut failing = Vec::new();
    loop {
        for area in poller.wait().await {
            let _turn = commit_order.lock().await;
            match poller.read(area).await {
                Ok(observed) => {
                    failing.retain(|failed| *failed != area);
                    record_observed(&feed, area, observed);
                }
                Err(error) => {
                    tracing::debug!("reading the change log of {area:?} failed: {error}");
                    if !failing.contains(&area) {
                        failing.push(area);
                        feed.resync(area);
                    }
                }
            }
        }
    }
}
