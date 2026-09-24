//! The Change feed: how a Store tells the application what changed.

use tokio::sync::mpsc;

use crate::{Area, Path};

/// A notice that one Path in one Area was changed or removed, and by whom.
///
/// It never carries the contents: read the File again to see them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Change {
    /// The Area the Path is in.
    pub area: Area,
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
    /// Changes that happened together. A Commit's Changes always arrive in the same batch.
    Changes(Vec<Change>),
    /// Changes to this Area may have been missed: read everything you rely on in it again.
    Resync(Area),
}

/// The single receiver of a Store's Changes, handed over when the Store is opened.
///
/// There is exactly one, and no way to get another later, so every Change after the Store is
/// opened is reported. If it is dropped, the Store keeps working.
#[derive(Debug)]
pub struct ChangeFeed {
    receiver: mpsc::UnboundedReceiver<FeedItem>,
}

impl ChangeFeed {
    /// Waits for the next item. Gives `None` once every handle to the Store has been dropped.
    pub async fn next(&mut self) -> Option<FeedItem> {
        self.receiver.recv().await
    }
}

/// The Store's end of its Change feed.
#[derive(Debug)]
pub(crate) struct FeedSender {
    sender: mpsc::UnboundedSender<FeedItem>,
}

impl FeedSender {
    /// Sends `changes` as one batch. Once the Change feed has been dropped, nothing is sent.
    pub(crate) fn announce(&self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        // An error only means the application dropped its Change feed.
        let _ = self.sender.send(FeedItem::Changes(changes));
    }
}

/// A new Change feed and the Store's end of it.
pub(crate) fn feed() -> (FeedSender, ChangeFeed) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (FeedSender { sender }, ChangeFeed { receiver })
}
