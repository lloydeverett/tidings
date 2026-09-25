//! The Store, Snapshot and Change feed the suite tests: the async API's, or the blocking API's,
//! called as a synchronous app calls them. Each has the async API's methods, so a test is written
//! once and runs through either.
//!
//! The blocking API panics inside an async runtime, and the tests are async, so every call into
//! it, dropping included, is made on a plain thread of its own, and waited for.

use std::panic;
use std::time::Duration;

use tidings::{
    Area, Committed, FeedItem, File, IntoPath, IntoPrefix, Path, PrefixRevision, Result, Staging,
    Stat,
};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::common::{Feed, TimedOut};

/// A Store, through the async API or the blocking one.
#[derive(Debug, Clone)]
pub enum Store {
    Async(tidings::Store),
    Blocking(OffRuntime<tidings::blocking::Store>),
}

/// A Snapshot from a [`Store`], through the same API.
#[derive(Debug)]
pub enum Snapshot {
    Async(tidings::Snapshot),
    Blocking(OffRuntime<tidings::blocking::Snapshot>),
}

/// A Store's Change feed, through the same API.
#[derive(Debug)]
pub enum ChangeFeed {
    Async(tidings::ChangeFeed),
    Blocking(OffRuntime<tidings::blocking::ChangeFeed>),
}

impl Store {
    pub async fn read(&self, area: Area, path: impl IntoPath + Send) -> Result<Option<File>> {
        match self {
            Store::Async(store) => store.read(area, path).await,
            Store::Blocking(store) => off_runtime(|| store.read(area, path)),
        }
    }

    pub async fn stat(&self, area: Area, path: impl IntoPath + Send) -> Result<Option<Stat>> {
        match self {
            Store::Async(store) => store.stat(area, path).await,
            Store::Blocking(store) => off_runtime(|| store.stat(area, path)),
        }
    }

    pub async fn list(&self, area: Area, prefix: impl IntoPrefix + Send) -> Result<Vec<Path>> {
        match self {
            Store::Async(store) => store.list(area, prefix).await,
            Store::Blocking(store) => off_runtime(|| store.list(area, prefix)),
        }
    }

    pub async fn stat_prefix(
        &self,
        area: Area,
        prefix: impl IntoPrefix + Send,
    ) -> Result<PrefixRevision> {
        match self {
            Store::Async(store) => store.stat_prefix(area, prefix).await,
            Store::Blocking(store) => off_runtime(|| store.stat_prefix(area, prefix)),
        }
    }

    pub fn supports_snapshots(&self) -> bool {
        match self {
            Store::Async(store) => store.supports_snapshots(),
            Store::Blocking(store) => off_runtime(|| store.supports_snapshots()),
        }
    }

    pub async fn snapshot(&self, area: Area) -> Result<Snapshot> {
        match self {
            Store::Async(store) => store.snapshot(area).await.map(Snapshot::Async),
            Store::Blocking(store) => {
                let snapshot = off_runtime(|| store.snapshot(area))?;
                Ok(Snapshot::Blocking(OffRuntime::new(snapshot)))
            }
        }
    }

    /// Through the blocking API, the Commit is made in full while this is first polled: it can't
    /// be cancelled part way.
    pub async fn commit(&self, staging: Staging) -> Result<Committed> {
        match self {
            Store::Async(store) => store.commit(staging).await,
            Store::Blocking(store) => off_runtime(|| store.commit(staging)),
        }
    }
}

impl Snapshot {
    pub async fn read(&self, path: impl IntoPath + Send) -> Result<Option<File>> {
        match self {
            Snapshot::Async(snapshot) => snapshot.read(path).await,
            Snapshot::Blocking(snapshot) => off_runtime(|| snapshot.read(path)),
        }
    }

    pub async fn stat(&self, path: impl IntoPath + Send) -> Result<Option<Stat>> {
        match self {
            Snapshot::Async(snapshot) => snapshot.stat(path).await,
            Snapshot::Blocking(snapshot) => off_runtime(|| snapshot.stat(path)),
        }
    }

    pub async fn list(&self, prefix: impl IntoPrefix + Send) -> Result<Vec<Path>> {
        match self {
            Snapshot::Async(snapshot) => snapshot.list(prefix).await,
            Snapshot::Blocking(snapshot) => off_runtime(|| snapshot.list(prefix)),
        }
    }
}

impl ChangeFeed {
    pub async fn next(&mut self) -> Option<FeedItem> {
        match self {
            ChangeFeed::Async(feed) => feed.next().await,
            ChangeFeed::Blocking(feed) => off_runtime(|| feed.next()),
        }
    }
}

impl Feed for ChangeFeed {
    /// Through the blocking API, with its own timeout, since a blocking wait can't be cancelled.
    async fn next_within(&mut self, timeout: Duration) -> Result<Option<FeedItem>, TimedOut> {
        match self {
            ChangeFeed::Async(feed) => feed.next_within(timeout).await,
            ChangeFeed::Blocking(feed) => {
                off_runtime(|| feed.next_timeout(timeout)).map_err(|_| TimedOut)
            }
        }
    }
}

impl From<tidings::Store> for Store {
    fn from(store: tidings::Store) -> Store {
        Store::Async(store)
    }
}

impl From<tidings::blocking::Store> for Store {
    fn from(store: tidings::blocking::Store) -> Store {
        Store::Blocking(OffRuntime::new(store))
    }
}

impl From<tidings::ChangeFeed> for ChangeFeed {
    fn from(feed: tidings::ChangeFeed) -> ChangeFeed {
        ChangeFeed::Async(feed)
    }
}

impl From<tidings::blocking::ChangeFeed> for ChangeFeed {
    fn from(feed: tidings::blocking::ChangeFeed) -> ChangeFeed {
        ChangeFeed::Blocking(OffRuntime::new(feed))
    }
}

/// A handle from the blocking API, which is dropped on a plain thread too, as a synchronous app
/// would drop it.
#[derive(Debug, Clone)]
pub struct OffRuntime<T: Send>(Option<T>);

impl<T: Send> OffRuntime<T> {
    fn new(handle: T) -> OffRuntime<T> {
        OffRuntime(Some(handle))
    }
}

impl<T: Send> std::ops::Deref for OffRuntime<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0.as_ref().expect("a handle is there until it is dropped")
    }
}

impl<T: Send> std::ops::DerefMut for OffRuntime<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_mut().expect("a handle is there until it is dropped")
    }
}

impl<T: Send> Drop for OffRuntime<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            off_runtime(move || drop(handle));
        }
    }
}

/// Makes `call` on a plain thread, outside any async runtime, and gives what it gave, or goes on
/// with its panic. On a multi-threaded runtime, the task waiting for it lets the runtime run its
/// other tasks meanwhile, so that the ones the test spawned keep going.
pub fn off_runtime<T: Send>(call: impl FnOnce() -> T + Send) -> T {
    let on_a_plain_thread = || {
        std::thread::scope(|scope| scope.spawn(call).join())
            .unwrap_or_else(|panicked| panic::resume_unwind(panicked))
    };
    match Handle::try_current().map(|runtime| runtime.runtime_flavor()) {
        Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(on_a_plain_thread),
        _ => on_a_plain_thread(),
    }
}
