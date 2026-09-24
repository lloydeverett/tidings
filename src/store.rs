use std::sync::Arc;

use jiff::Timestamp;

use crate::backend::{Backend, CommitRequest, memory::MemoryBackend};
use crate::change::{self, FeedSender};
use crate::{Area, Change, ChangeFeed, ChangeKind, File, IntoPath, Origin, Result, Staging};

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

    /// Commits `staging`: applies all of its writes, or none of them. Every File written gets the
    /// same last-modified time, and the Changes arrive on the Change feed in one batch.
    pub async fn commit(&self, staging: Staging) -> Result<()> {
        let area = staging.area();
        let request =
            CommitRequest { area, timestamp: Timestamp::now(), writes: staging.into_writes() };
        let written = self.inner.backend.commit(request).await?;
        let changes = written
            .into_keys()
            .map(|path| Change { area, path, kind: ChangeKind::Changed, origin: Origin::Local })
            .collect();
        self.inner.feed.announce(changes);
        Ok(())
    }
}
