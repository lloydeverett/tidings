//! What actually holds a Store's Files. Backends are private to the crate: the Store layer does
//! everything that is the same for all of them, and calls into the Backend for the rest.

pub(crate) mod memory;

use std::collections::BTreeMap;

use jiff::Timestamp;

use crate::staging::Staged;
use crate::{Area, ChangeKind, File, Path, Prefix, PrefixRevision, Result, Revision, Stat};

/// The Backend a Store was opened on.
#[derive(Debug)]
pub(crate) enum Backend {
    Memory(memory::MemoryBackend),
}

/// A Backend's view of one Area as it stood when a Snapshot was taken.
///
/// It holds only what the Backend needs to read that view, never the Store's shared state, so a
/// Snapshot doesn't keep the Change feed open after the last Store handle goes, and it can still
/// be read after that. Memory holds the Area's Files. SQLite will hold a read transaction on a
/// connection of its own.
#[derive(Debug)]
pub(crate) enum BackendSnapshot {
    Memory(memory::MemorySnapshot),
}

/// What an Area holds when a Commit runs, as the checks shared by every Backend need to see it.
/// Each Backend reads it its own way, under its lock.
pub(crate) trait AreaState {
    /// The Revision of the File at `path`, or `None` if there is none.
    fn revision(&self, path: &Path) -> Result<Option<Revision>>;
    /// The Path and Revision of every File under `prefix`, in order of Path.
    fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>>;
}

/// A Staging, with its Paths validated, ready for a Backend to commit.
#[derive(Debug)]
pub(crate) struct CommitRequest {
    /// The last-modified time every File written gets. The Store layer chooses it.
    pub(crate) timestamp: Timestamp,
    /// What to commit. The Backend expands its Prefix deletes under its lock.
    pub(crate) staged: Staged,
}

/// What a Commit did.
#[derive(Debug, Default)]
pub(crate) struct CommitOutcome {
    /// The new Revision of each Path written, including writes left out because they would not
    /// have changed the contents.
    pub(crate) revisions: BTreeMap<Path, Revision>,
    /// Each Path the Commit changed or removed, in order.
    pub(crate) changes: Vec<RawChange>,
}

/// A change a Backend made or observed, before the Store layer tags it with its Area and Origin.
#[derive(Debug)]
pub(crate) struct RawChange {
    pub(crate) path: Path,
    pub(crate) kind: ChangeKind,
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

    /// The Prefix Revision of everything under `prefix`.
    pub(crate) async fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        match self {
            Backend::Memory(backend) => backend.stat_prefix(area, prefix),
        }
    }

    /// Whether [`snapshot`](Self::snapshot) can give a Snapshot. A Backend that can't gives
    /// `Unsupported` from it instead.
    pub(crate) fn supports_snapshots(&self) -> bool {
        match self {
            Backend::Memory(_) => true,
        }
    }

    /// A view of `area` as it stands now, which Commits made afterwards don't change, and which
    /// doesn't hold them up.
    pub(crate) async fn snapshot(&self, area: Area) -> Result<BackendSnapshot> {
        match self {
            Backend::Memory(backend) => Ok(BackendSnapshot::Memory(backend.snapshot(area))),
        }
    }

    /// Applies every write and delete in `request`, all-or-nothing, after expanding its Prefix
    /// deletes, checking its Preconditions and leaving out writes that would not change the
    /// contents.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        match self {
            Backend::Memory(backend) => backend.commit(request),
        }
    }
}

impl BackendSnapshot {
    pub(crate) async fn read(&self, path: &Path) -> Result<Option<File>> {
        match self {
            BackendSnapshot::Memory(snapshot) => Ok(snapshot.read(path)),
        }
    }

    pub(crate) async fn stat(&self, path: &Path) -> Result<Option<Stat>> {
        match self {
            BackendSnapshot::Memory(snapshot) => Ok(snapshot.stat(path)),
        }
    }

    /// The Paths under `prefix`, in order.
    pub(crate) async fn list(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        match self {
            BackendSnapshot::Memory(snapshot) => Ok(snapshot.list(prefix)),
        }
    }
}
