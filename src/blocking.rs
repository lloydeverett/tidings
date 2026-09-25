//! A blocking Store, for synchronous code: an app that has no async runtime of its own can use
//! tidings from plain threads.
//!
//! [`Store`] mirrors the async [`crate::Store`], with the same operations and behaviour, and
//! waits for each to finish. It runs the async Store on an internal tokio runtime of its own, as
//! `reqwest::blocking` does. That runtime keeps running between calls, so what the Store follows
//! in the background (other Stores' Commits on SQLite, edits in the Areas' directories on the
//! filesystem) keeps reaching the Change feed while the app isn't calling it. The runtime stops
//! once the Store, every Snapshot taken from it and its Change feed have all been dropped.
//!
//! Every method that waits panics if it is called where blocking would stall a tokio runtime or
//! deadlock it: in an async task, or in a runtime's own `block_on`. Use the async
//! [`crate::Store`] there. Where tokio allows blocking, it works: on a plain thread, and in a
//! runtime's `spawn_blocking` or `block_in_place`. Dropping is allowed anywhere. (Built with
//! `panic = "abort"`, the process ends with tokio's own message instead, located in tokio.)

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;

#[cfg(any(feature = "fs", feature = "sqlite"))]
use crate::AppIdentity;
#[cfg(feature = "testing")]
use crate::ChangeKind;
#[cfg(feature = "fs")]
use crate::FsOptions;
#[cfg(feature = "sqlite")]
use crate::SqliteOptions;
use crate::{
    Area, Committed, FeedItem, File, IntoPath, IntoPrefix, Path, PrefixRevision, Result, Staging,
    Stat,
};

/// What an application opens to reach its Files, for synchronous code: the async
/// [`Store`](crate::Store)'s operations, each waited for.
///
/// A cheap handle that can be shared between threads: clones share the same Files and the same
/// Change feed. The Change feed ends once every clone has been dropped.
#[derive(Debug, Clone)]
pub struct Store {
    store: crate::Store,
    /// Dropped after the Store, so that the Store's tasks are stopped before the runtime is.
    runtime: Arc<SharedRuntime>,
}

/// A view of one Area as it stood when it was taken, from [`Store::snapshot`]: the async
/// [`Snapshot`](crate::Snapshot)'s operations, each waited for.
///
/// Like the async one, it doesn't keep the Store open: the Change feed still ends once every
/// Store handle has been dropped, and the Snapshot can still be read. It holds the runtime it is
/// read on, not the Store.
#[derive(Debug)]
pub struct Snapshot {
    snapshot: crate::Snapshot,
    runtime: Arc<SharedRuntime>,
}

/// The single receiver of a Store's Changes, handed over when the Store is opened, for a plain
/// thread to wait on: the async [`ChangeFeed`](crate::ChangeFeed), with the same guarantees.
///
/// It is an [`Iterator`] of the feed's items, which waits for each and ends once every handle to
/// the Store has been dropped and everything recorded before has been read. So a thread can
/// process Changes with `for item in feed`. [`next_timeout`](Self::next_timeout) waits for a
/// while only.
#[derive(Debug)]
pub struct ChangeFeed {
    feed: crate::ChangeFeed,
    runtime: Arc<SharedRuntime>,
}

/// Nothing arrived on the Change feed within the time given to
/// [`ChangeFeed::next_timeout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("nothing arrived on the Change feed in time")]
pub struct TimedOut;

impl Store {
    /// Opens a Store that keeps its Files in memory, as
    /// [`crate::Store::open_memory`] does.
    ///
    /// # Panics
    ///
    /// If the internal runtime can't be started.
    pub fn open_memory() -> (Store, ChangeFeed) {
        let runtime = SharedRuntime::start();
        Store::opened(crate::Store::open_memory(), runtime)
    }

    /// Opens a Store that keeps each Area in a directory, as [`crate::Store::open_fs`] does,
    /// with the same options and errors. The Areas' directories are watched while the app isn't
    /// calling the Store too.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`, or the internal runtime
    /// can't be started.
    #[cfg(feature = "fs")]
    #[track_caller]
    pub fn open_fs(app: &AppIdentity, options: FsOptions) -> Result<(Store, ChangeFeed)> {
        let runtime = SharedRuntime::start();
        let opened = runtime.block_on(crate::Store::open_fs(app, options))?;
        Ok(Store::opened(opened, runtime))
    }

    /// Opens a Store that keeps each Area in a SQLite database, as
    /// [`crate::Store::open_sqlite`] does, with the same options and errors. Other Stores'
    /// Commits are checked for while the app isn't calling the Store too.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`, or the internal runtime
    /// can't be started.
    #[cfg(feature = "sqlite")]
    #[track_caller]
    pub fn open_sqlite(app: &AppIdentity, options: SqliteOptions) -> Result<(Store, ChangeFeed)> {
        let runtime = SharedRuntime::start();
        let opened = runtime.block_on(crate::Store::open_sqlite(app, options))?;
        Ok(Store::opened(opened, runtime))
    }

    /// The blocking Store and Change feed for an async Store and its feed, opened on `runtime`.
    fn opened(
        (store, feed): (crate::Store, crate::ChangeFeed),
        runtime: Arc<SharedRuntime>,
    ) -> (Store, ChangeFeed) {
        let feed = ChangeFeed { feed, runtime: Arc::clone(&runtime) };
        (Store { store, runtime }, feed)
    }

    /// Reads the File at `path` in `area`, as [`crate::Store::read`] does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn read(&self, area: Area, path: impl IntoPath) -> Result<Option<File>> {
        self.runtime.block_on(self.store.read(area, path))
    }

    /// Gives when the File at `path` in `area` was last modified and its Revision, as
    /// [`crate::Store::stat`] does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn stat(&self, area: Area, path: impl IntoPath) -> Result<Option<Stat>> {
        self.runtime.block_on(self.store.stat(area, path))
    }

    /// Lists the Paths of the Files under `prefix` in `area`, as [`crate::Store::list`] does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn list(&self, area: Area, prefix: impl IntoPrefix) -> Result<Vec<Path>> {
        self.runtime.block_on(self.store.list(area, prefix))
    }

    /// Gives the Prefix Revision of everything under `prefix` in `area`, as
    /// [`crate::Store::stat_prefix`] does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn stat_prefix(&self, area: Area, prefix: impl IntoPrefix) -> Result<PrefixRevision> {
        self.runtime.block_on(self.store.stat_prefix(area, prefix))
    }

    /// Whether this Store's Backend provides Snapshots, as
    /// [`crate::Store::supports_snapshots`] says. It doesn't wait, so it can be called anywhere.
    pub fn supports_snapshots(&self) -> bool {
        self.store.supports_snapshots()
    }

    /// Takes a [`Snapshot`] of `area`, as [`crate::Store::snapshot`] does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn snapshot(&self, area: Area) -> Result<Snapshot> {
        let snapshot = self.runtime.block_on(self.store.snapshot(area))?;
        Ok(Snapshot { snapshot, runtime: Arc::clone(&self.runtime) })
    }

    /// Commits `staging`, all-or-nothing, as [`crate::Store::commit`] does, and waits for it to
    /// finish. The Store's runtime keeps running until it has, since this call holds the Store.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn commit(&self, staging: Staging) -> Result<Committed> {
        self.runtime.block_on(self.store.commit(staging))
    }

    /// Records an external Change on the Change feed, as
    /// [`crate::Store::inject_external_change`] does. For tidings' own tests. It doesn't wait, so
    /// it can be called anywhere.
    #[cfg(feature = "testing")]
    pub fn inject_external_change(
        &self,
        area: Area,
        path: impl IntoPath,
        kind: ChangeKind,
    ) -> Result<()> {
        self.store.inject_external_change(area, path, kind)
    }
}

impl Snapshot {
    /// Reads the File at `path` as it was, as [`crate::Snapshot::read`] does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn read(&self, path: impl IntoPath) -> Result<Option<File>> {
        self.runtime.block_on(self.snapshot.read(path))
    }

    /// Gives when the File at `path` was last modified and its Revision, as they were, as
    /// [`crate::Snapshot::stat`] does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn stat(&self, path: impl IntoPath) -> Result<Option<Stat>> {
        self.runtime.block_on(self.snapshot.stat(path))
    }

    /// Lists the Paths of the Files that were under `prefix`, as [`crate::Snapshot::list`]
    /// does.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn list(&self, prefix: impl IntoPrefix) -> Result<Vec<Path>> {
        self.runtime.block_on(self.snapshot.list(prefix))
    }
}

impl ChangeFeed {
    /// Waits up to `timeout` for the next item, as [`next`](Iterator::next) does. Gives
    /// [`TimedOut`] if nothing arrived in time, and `Ok(None)` once the feed has ended.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    pub fn next_timeout(&mut self, timeout: Duration) -> Result<Option<FeedItem>, TimedOut> {
        // Made on the runtime, since a timer needs one.
        let next = async { tokio::time::timeout(timeout, self.feed.next()).await };
        self.runtime.block_on(next).map_err(|_| TimedOut)
    }
}

impl Iterator for ChangeFeed {
    type Item = FeedItem;

    /// Waits for the next item: every Change recorded since the last one, merged per Area and
    /// Path, in one batch, as [`crate::ChangeFeed::next`] gives it. Gives `None` once every
    /// handle to the Store has been dropped and everything recorded before has been read.
    ///
    /// # Panics
    ///
    /// If it is called in an async task, or a runtime's `block_on`.
    #[track_caller]
    fn next(&mut self) -> Option<FeedItem> {
        self.runtime.block_on(self.feed.next())
    }
}

/// The internal runtime a Store's handles share: the Store's clones, its Snapshots and its
/// Change feed. It is multi-threaded, with one worker, so that the Store's tasks run between
/// calls, not only while a call is waiting. The last handle to go shuts it down.
#[derive(Debug)]
struct SharedRuntime {
    /// Only taken when it is shut down.
    tokio: Option<tokio::runtime::Runtime>,
}

impl SharedRuntime {
    fn start() -> Arc<SharedRuntime> {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("tidings")
            .enable_all()
            .build()
            .expect("tidings should be able to start the runtime of a blocking Store");
        Arc::new(SharedRuntime { tokio: Some(tokio) })
    }

    /// Runs `future` on the runtime, blocking this thread until it finishes.
    ///
    /// # Panics
    ///
    /// If this thread mustn't block: see [`refuse_if_blocking_would_stall`].
    #[track_caller]
    fn block_on<F: Future>(&self, future: F) -> F::Output {
        let tokio = self.tokio.as_ref().expect("the runtime runs until it is dropped");
        refuse_if_blocking_would_stall(tokio);
        tokio.block_on(future)
    }
}

impl Drop for SharedRuntime {
    /// Shuts the runtime down, waiting for its threads, unless this is in a runtime's context,
    /// where waiting could block that runtime, and tokio would panic. There it lets them finish
    /// on their own. The Store's tasks have been stopped already, and no call is in progress,
    /// since each holds a handle.
    ///
    /// Unlike [`refuse_if_blocking_would_stall`], it doesn't ask tokio whether it may wait, since
    /// asking prints a panic where no mistake was made.
    fn drop(&mut self) {
        let tokio = self.tokio.take().expect("the runtime is shut down only once");
        if Handle::try_current().is_ok() {
            tokio.shutdown_background();
        } else {
            drop(tokio);
        }
    }
}

/// Panics, saying why, if this thread mustn't block, because it runs a tokio runtime's async
/// tasks or its `block_on`, where blocking would stall that runtime or deadlock it.
///
/// A thread with no runtime's context may block. One in a runtime's context may too, in
/// `spawn_blocking` or `block_in_place`, and only tokio can tell those apart: its public API
/// tells only by refusing to block. So this asks it, blocking on `tokio` with a future that does
/// nothing and can't panic itself. If tokio refuses, its own panic is printed first, then this
/// one, which names the call that made the mistake.
#[track_caller]
fn refuse_if_blocking_would_stall(tokio: &tokio::runtime::Runtime) {
    if Handle::try_current().is_err() {
        return;
    }
    if panic::catch_unwind(AssertUnwindSafe(|| tokio.block_on(async {}))).is_err() {
        panic!(
            "tidings' blocking API was called in an async task, or a runtime's `block_on`, where \
             blocking would stall the runtime or deadlock it: use the async `tidings::Store` \
             there, or call it through `spawn_blocking` or `block_in_place`"
        );
    }
}
