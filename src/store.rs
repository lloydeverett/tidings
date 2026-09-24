use std::sync::Arc;

use jiff::Timestamp;

use crate::backend::{Backend, CommitRequest, memory::MemoryBackend};
use crate::change::{self, FeedSender};
use crate::{
    Area, Change, ChangeFeed, Committed, File, IntoPath, IntoPrefix, Origin, Path, Result, Staging,
    Stat,
};

/// What an application opens to reach its Files: all three Areas, held by one Backend.
///
/// A Store is a cheap handle: clones share the same Files and the same Change feed.
#[derive(Debug, Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

/// What every clone of a Store shares. The Store layer here does everything that is the same for
/// every Backend, and calls into the Backend for the rest.
#[derive(Debug)]
struct Inner {
    backend: Backend,
    feed: FeedSender,
}

impl Store {
    /// Opens a Store that keeps its Files in memory, for tests and short-lived data. Returns the
    /// Store together with its one Change feed.
    pub fn open_memory() -> (Store, ChangeFeed) {
        Store::open(Backend::Memory(MemoryBackend::default()))
    }

    fn open(backend: Backend) -> (Store, ChangeFeed) {
        let (feed, change_feed) = change::feed();
        (Store { inner: Arc::new(Inner { backend, feed }) }, change_feed)
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

    /// Commits `staging`: applies all of its writes and deletes, or none of them. Prefix deletes
    /// cover the Files under the Prefix at this moment.
    ///
    /// Every File written gets the Commit's timestamp as its last-modified time. A write that
    /// wouldn't change the File's contents is left out, so the File keeps its time and no Change
    /// is sent for it. The Changes arrive on the Change feed in one batch, and a Commit that
    /// changes nothing sends none. On success, gives the timestamp and the new Revisions.
    pub async fn commit(&self, staging: Staging) -> Result<Committed> {
        let area = staging.area();
        let timestamp = Timestamp::now();
        let (staged, prefix_deletes) = staging.into_parts();
        let request = CommitRequest { area, timestamp, staged, prefix_deletes };
        let applied = self.inner.backend.commit(request).await?;
        let changes = applied
            .changes
            .into_iter()
            .map(|(path, kind)| Change { area, path, kind, origin: Origin::Local })
            .collect();
        self.inner.feed.announce(changes);
        Ok(Committed::new(timestamp, applied.revisions))
    }
}
