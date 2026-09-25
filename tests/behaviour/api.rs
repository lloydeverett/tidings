//! The Store, Snapshot and Change feed the suite tests: the async API's, or the blocking API's.
//! Each has the async API's methods, so a test is written once and runs through either.
//!
//! The blocking API panics in an async task, and the tests are async, so every call into it is
//! made where tokio allows blocking, as async code calls blocking code: see [`call_blocking`].

use std::panic;
use std::time::Duration;

use tidings::blocking::TimedOut;
use tidings::{
    Area, Committed, FeedItem, File, IntoPath, IntoPrefix, Path, PrefixRevision, Result, Staging,
    Stat,
};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::common::Feed;

/// A Store, through the async API or the blocking one.
#[derive(Debug, Clone)]
pub enum Store {
    Async(tidings::Store),
    Blocking(tidings::blocking::Store),
}

/// A Snapshot from a [`Store`], through the same API.
#[derive(Debug)]
pub enum Snapshot {
    Async(tidings::Snapshot),
    Blocking(tidings::blocking::Snapshot),
}

/// A Store's Change feed, through the same API.
#[derive(Debug)]
pub enum ChangeFeed {
    Async(tidings::ChangeFeed),
    Blocking(tidings::blocking::ChangeFeed),
}

impl Store {
    pub async fn read(&self, area: Area, path: impl IntoPath + Send) -> Result<Option<File>> {
        match self {
            Store::Async(store) => store.read(area, path).await,
            Store::Blocking(store) => call_blocking(|| store.read(area, path)),
        }
    }

    pub async fn stat(&self, area: Area, path: impl IntoPath + Send) -> Result<Option<Stat>> {
        match self {
            Store::Async(store) => store.stat(area, path).await,
            Store::Blocking(store) => call_blocking(|| store.stat(area, path)),
        }
    }

    pub async fn list(&self, area: Area, prefix: impl IntoPrefix + Send) -> Result<Vec<Path>> {
        match self {
            Store::Async(store) => store.list(area, prefix).await,
            Store::Blocking(store) => call_blocking(|| store.list(area, prefix)),
        }
    }

    pub async fn stat_prefix(
        &self,
        area: Area,
        prefix: impl IntoPrefix + Send,
    ) -> Result<PrefixRevision> {
        match self {
            Store::Async(store) => store.stat_prefix(area, prefix).await,
            Store::Blocking(store) => call_blocking(|| store.stat_prefix(area, prefix)),
        }
    }

    pub fn supports_snapshots(&self) -> bool {
        match self {
            Store::Async(store) => store.supports_snapshots(),
            Store::Blocking(store) => call_blocking(|| store.supports_snapshots()),
        }
    }

    pub async fn snapshot(&self, area: Area) -> Result<Snapshot> {
        match self {
            Store::Async(store) => store.snapshot(area).await.map(Snapshot::Async),
            Store::Blocking(store) => {
                let snapshot = call_blocking(|| store.snapshot(area))?;
                Ok(Snapshot::Blocking(snapshot))
            }
        }
    }

    /// Through the blocking API, the Commit is made in full while this is first polled: it can't
    /// be cancelled part way.
    pub async fn commit(&self, staging: Staging) -> Result<Committed> {
        match self {
            Store::Async(store) => store.commit(staging).await,
            Store::Blocking(store) => call_blocking(|| store.commit(staging)),
        }
    }
}

impl Snapshot {
    pub async fn read(&self, path: impl IntoPath + Send) -> Result<Option<File>> {
        match self {
            Snapshot::Async(snapshot) => snapshot.read(path).await,
            Snapshot::Blocking(snapshot) => call_blocking(|| snapshot.read(path)),
        }
    }

    pub async fn stat(&self, path: impl IntoPath + Send) -> Result<Option<Stat>> {
        match self {
            Snapshot::Async(snapshot) => snapshot.stat(path).await,
            Snapshot::Blocking(snapshot) => call_blocking(|| snapshot.stat(path)),
        }
    }

    pub async fn list(&self, prefix: impl IntoPrefix + Send) -> Result<Vec<Path>> {
        match self {
            Snapshot::Async(snapshot) => snapshot.list(prefix).await,
            Snapshot::Blocking(snapshot) => call_blocking(|| snapshot.list(prefix)),
        }
    }
}

impl ChangeFeed {
    pub async fn next(&mut self) -> Option<FeedItem> {
        match self {
            ChangeFeed::Async(feed) => feed.next().await,
            ChangeFeed::Blocking(feed) => call_blocking(|| feed.next()),
        }
    }
}

impl Feed for ChangeFeed {
    /// Through the blocking API, with its own timeout, since a blocking wait can't be cancelled.
    async fn next_within(&mut self, timeout: Duration) -> Result<Option<FeedItem>, TimedOut> {
        match self {
            ChangeFeed::Async(feed) => feed.next_within(timeout).await,
            ChangeFeed::Blocking(feed) => call_blocking(|| feed.next_timeout(timeout)),
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
        Store::Blocking(store)
    }
}

impl From<tidings::ChangeFeed> for ChangeFeed {
    fn from(feed: tidings::ChangeFeed) -> ChangeFeed {
        ChangeFeed::Async(feed)
    }
}

impl From<tidings::blocking::ChangeFeed> for ChangeFeed {
    fn from(feed: tidings::blocking::ChangeFeed) -> ChangeFeed {
        ChangeFeed::Blocking(feed)
    }
}

/// Makes `call`, which blocks, from async test code, and gives what it gave. On a
/// multi-threaded runtime, it is made in `block_in_place`, which lets the runtime run its other
/// tasks meanwhile, so the ones the test spawned keep going. A current-thread runtime can't do
/// that, and only runs a thread reading a Change feed, so there it is made on a plain thread of
/// its own, and waited for.
pub fn call_blocking<T: Send>(call: impl FnOnce() -> T + Send) -> T {
    match Handle::current().runtime_flavor() {
        RuntimeFlavor::MultiThread => tokio::task::block_in_place(call),
        _ => std::thread::scope(|scope| scope.spawn(call).join())
            .unwrap_or_else(|panicked| panic::resume_unwind(panicked)),
    }
}
