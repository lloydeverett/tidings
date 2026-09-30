//! The Change feed: how a Store tells the application what changed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::Path;
use crate::backend::RawChange;

/// A notice that one Path in a Store was changed or removed, and by whom.
///
/// It never carries the contents: read the File again to see them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Change {
    /// The Path that changed.
    pub path: Path,
    /// Whether the File was changed or removed.
    pub kind: ChangeKind,
    /// Who made the Change.
    pub origin: Origin,
}

/// What happened to a File.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    /// The File was created or its contents changed.
    Changed,
    /// The File was removed.
    Removed,
}

/// Who made a Change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A Commit through this Store.
    Local,
    /// Anything else: another process, or a person editing the File.
    External,
}

/// One item on the Change feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedItem {
    /// Every Change recorded since the last item, merged per Path. The Changes of a
    /// Commit through this Store always arrive in the same batch, and so do another Store's on
    /// SQLite, and usually on the filesystem. A batch may hold several Commits' Changes.
    Changes(Vec<Change>),
    /// Changes to the Store may have been missed: read everything you rely on in it again.
    Resync,
}

/// The single receiver of a Store's Changes, handed over when the Store is opened.
///
/// There is exactly one, and no way to get another later, so every Change after the Store is
/// opened is reported. If it is dropped, the Store keeps working and stops recording Changes.
///
/// Changes wait here until they are read, merged per Path: a Path changed many times
/// before [`next`](Self::next) is called gives one Change, so falling behind never loses a Path
/// and the memory held grows only with the number of Paths changed.
#[derive(Debug)]
pub struct ChangeFeed {
    shared: Arc<Shared>,
}

impl ChangeFeed {
    /// Waits for the next item: every Change recorded since the last one, merged per Path, in one
    /// batch. The Changes of a Commit through this Store are never split across
    /// batches, and neither are another Store's on SQLite (on the filesystem, see
    /// `Store::open_fs`, with the `fs` feature). A batch may hold several Commits' Changes.
    ///
    /// Gives `None` once every handle to the Store has been dropped and everything recorded
    /// before has been read.
    pub async fn next(&mut self) -> Option<FeedItem> {
        loop {
            let next = self.shared.unread.lock().unwrap().next();
            match next {
                Next::Item(item) => return Some(item),
                Next::Ended => return None,
                Next::Wait => self.shared.recorded.notified().await,
            }
        }
    }
}

impl Drop for ChangeFeed {
    /// Nobody will read the Changes, so what is unread is dropped and nothing more is recorded.
    fn drop(&mut self) {
        let mut unread = self.shared.unread.lock().unwrap();
        unread.feed_dropped = true;
        unread.pending = Pending::default();
    }
}

/// What the Store's end and the Change feed share.
#[derive(Debug, Default)]
struct Shared {
    unread: Mutex<Unread>,
    /// Woken whenever something is recorded. There is only one Change feed to wake, and a
    /// wake-up with nobody waiting is kept for the next wait, so none is missed.
    recorded: Notify,
}

/// Every Change recorded and not yet read, merged. It holds at most one entry per Path, however
/// many Changes were recorded.
#[derive(Debug, Default)]
struct Unread {
    /// What is unread.
    pending: Pending,
    /// Every Store handle has been dropped: once what is unread has been read, the Change feed
    /// ends.
    store_dropped: bool,
    /// The Change feed has been dropped, so nothing is recorded.
    feed_dropped: bool,
}

/// What is unread.
#[derive(Debug)]
enum Pending {
    /// The merged Changes to each Path.
    Changes(BTreeMap<Path, Merged>),
    /// A Resync. It took the place of the unread Changes, and Changes recorded until it is read
    /// add nothing to it: reading the Store again after it covers them.
    Resync,
}

impl Default for Pending {
    fn default() -> Pending {
        Pending::Changes(BTreeMap::new())
    }
}

/// What is left of every unread Change to one Path, merged.
#[derive(Debug)]
struct Merged {
    /// The latest kind.
    kind: ChangeKind,
    /// External if any of the Changes merged was, so an app that skips its own Changes never
    /// misses someone else's.
    origin: Origin,
}

/// What [`ChangeFeed::next`] does now.
enum Next {
    /// Gives this item.
    Item(FeedItem),
    /// Gives `None`: the Store is gone and everything recorded has been read.
    Ended,
    /// Waits for something to be recorded.
    Wait,
}

impl Unread {
    fn record(&mut self, changes: Vec<RawChange>, origin: Origin) {
        if self.store_dropped || self.feed_dropped {
            return;
        }
        let Pending::Changes(merged) = &mut self.pending else {
            return;
        };
        for RawChange { path, kind } in changes {
            let merged = merged.entry(path).or_insert(Merged { kind, origin });
            merged.kind = kind;
            if origin == Origin::External {
                merged.origin = Origin::External;
            }
        }
    }

    fn resync(&mut self) {
        if self.store_dropped || self.feed_dropped {
            return;
        }
        self.pending = Pending::Resync;
    }

    /// A Resync, or else a batch of every unread Change.
    fn next(&mut self) -> Next {
        match std::mem::take(&mut self.pending) {
            Pending::Resync => Next::Item(FeedItem::Resync),
            Pending::Changes(merged) if !merged.is_empty() => {
                let batch = merged.into_iter().map(|(path, Merged { kind, origin })| Change {
                    path,
                    kind,
                    origin,
                });
                Next::Item(FeedItem::Changes(batch.collect()))
            }
            Pending::Changes(_) if self.store_dropped => Next::Ended,
            Pending::Changes(_) => Next::Wait,
        }
    }
}

/// The Store's end of its Change feed. A clone doesn't keep the feed open: it ends when the Store
/// calls [`end`](Self::end).
#[derive(Debug, Clone)]
pub(crate) struct FeedSender {
    shared: Arc<Shared>,
}

impl FeedSender {
    /// Records `changes`, all made by `origin`, merging them into what is unread. They are
    /// recorded all at once, so that a Commit's Changes reach the Change feed in the same batch.
    ///
    /// This is where every Change enters the feed, local or external: the Store layer tags each
    /// raw change with its Origin and records it here.
    pub(crate) fn record(&self, changes: Vec<RawChange>, origin: Origin) {
        if changes.is_empty() {
            return;
        }
        self.shared.unread.lock().unwrap().record(changes, origin);
        self.shared.recorded.notify_one();
    }

    /// Records that Changes may have been missed. The Resync takes the place of the unread
    /// Changes, and those recorded until the Resync is read.
    pub(crate) fn resync(&self) {
        self.shared.unread.lock().unwrap().resync();
        self.shared.recorded.notify_one();
    }

    /// Ends the Change feed, once what was recorded before has been read. The Store calls it
    /// when its last handle is dropped. Nothing recorded after it reaches the feed.
    pub(crate) fn end(&self) {
        self.shared.unread.lock().unwrap().store_dropped = true;
        self.shared.recorded.notify_one();
    }
}

/// A new Change feed and the Store's end of it.
pub(crate) fn feed() -> (FeedSender, ChangeFeed) {
    let shared = Arc::new(Shared::default());
    (FeedSender { shared: shared.clone() }, ChangeFeed { shared })
}
