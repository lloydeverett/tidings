//! What actually holds a Store's Files. Backends are private to the crate: the Store layer does
//! everything that is the same for all of them, and calls into the Backend for the rest.

pub(crate) mod memory;

use std::collections::BTreeMap;

use jiff::Timestamp;

use crate::{Area, File, Path, Result, Revision};

/// The Backend a Store was opened on.
#[derive(Debug)]
pub(crate) enum Backend {
    Memory(memory::MemoryBackend),
}

/// A Staging, with its Paths validated, ready for a Backend to apply.
#[derive(Debug)]
pub(crate) struct CommitRequest {
    pub(crate) area: Area,
    /// The last-modified time every File written gets. The Store layer chooses it.
    pub(crate) timestamp: Timestamp,
    pub(crate) writes: BTreeMap<Path, String>,
}

/// The Paths a Commit wrote, with their new Revisions.
pub(crate) type Written = BTreeMap<Path, Revision>;

impl Backend {
    pub(crate) async fn read(&self, area: Area, path: &Path) -> Result<Option<File>> {
        match self {
            Backend::Memory(backend) => Ok(backend.read(area, path)),
        }
    }

    /// Applies every write in `request`, all-or-nothing. Gives the new Revision of each Path
    /// written.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<Written> {
        match self {
            Backend::Memory(backend) => Ok(backend.commit(request)),
        }
    }
}
