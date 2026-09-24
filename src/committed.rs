use std::collections::BTreeMap;

use jiff::Timestamp;

use crate::{Path, Revision};

/// What a successful Commit gives back: its timestamp, and the new Revision of each Path it
/// wrote, so those Files can be written again safely without reading them first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    timestamp: Timestamp,
    revisions: BTreeMap<Path, Revision>,
}

impl Committed {
    pub(crate) fn new(timestamp: Timestamp, revisions: BTreeMap<Path, Revision>) -> Committed {
        Committed { timestamp, revisions }
    }

    /// The Commit's timestamp: the last-modified time of every File it changed.
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// The Revision of each Path the Staging wrote, in order. That includes writes left out
    /// because they wouldn't have changed the File. Deleted Paths aren't included.
    pub fn revisions(&self) -> &BTreeMap<Path, Revision> {
        &self.revisions
    }
}
