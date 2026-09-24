//! What actually holds a Store's Files. Backends are private to the crate: the Store layer does
//! everything that is the same for all of them, and calls into the Backend for the rest.

pub(crate) mod memory;

use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;

use crate::staging::Staged;
use crate::{Area, ChangeKind, File, Path, Prefix, Result, Revision, Stat};

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
    /// What to do to each Path. It wins over `prefix_deletes`, because it was staged after them.
    pub(crate) staged: BTreeMap<Path, Staged>,
    /// Prefixes to delete everything under, expanded by the Backend when the Commit runs.
    pub(crate) prefix_deletes: BTreeSet<Prefix>,
}

/// What a Commit did.
#[derive(Debug, Default)]
pub(crate) struct Applied {
    /// The new Revision of each Path written, including writes left out because they would not
    /// have changed the contents.
    pub(crate) revisions: BTreeMap<Path, Revision>,
    /// Each Path the Commit changed or removed, in order.
    pub(crate) changes: Vec<(Path, ChangeKind)>,
}

impl Backend {
    pub(crate) async fn read(&self, area: Area, path: &Path) -> Result<Option<File>> {
        match self {
            Backend::Memory(backend) => Ok(backend.read(area, path)),
        }
    }

    pub(crate) async fn stat(&self, area: Area, path: &Path) -> Result<Option<Stat>> {
        match self {
            Backend::Memory(backend) => Ok(backend.stat(area, path)),
        }
    }

    /// The Paths under `prefix`, in order.
    pub(crate) async fn list(&self, area: Area, prefix: &Prefix) -> Result<Vec<Path>> {
        match self {
            Backend::Memory(backend) => Ok(backend.list(area, prefix)),
        }
    }

    /// Applies every write and delete in `request`, all-or-nothing, after expanding its Prefix
    /// deletes and leaving out writes that would not change the contents.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<Applied> {
        match self {
            Backend::Memory(backend) => Ok(backend.commit(request)),
        }
    }
}
