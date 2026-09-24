//! What actually holds a Store's Files. Backends are private to the crate: the Store layer does
//! everything that is the same for all of them, and calls into the Backend for the rest.

pub(crate) mod memory;
#[cfg(feature = "sqlite")]
pub(crate) mod sqlite;

use std::collections::BTreeMap;

use jiff::Timestamp;

use crate::staging::{Action, Staged};
use crate::{Area, ChangeKind, File, Path, Prefix, PrefixRevision, Result, Revision, Stat};

/// The Backend a Store was opened on.
#[derive(Debug)]
pub(crate) enum Backend {
    Memory(memory::MemoryBackend),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlite::SqliteBackend),
}

/// A Backend's view of one Area as it stood when a Snapshot was taken.
///
/// It holds only what the Backend needs to read that view, never the Store's shared state, so a
/// Snapshot doesn't keep the Change feed open after the last Store handle goes, and it can still
/// be read after that. Memory holds the Area's Files. SQLite holds a read transaction on a
/// connection of its own.
#[derive(Debug)]
pub(crate) enum BackendSnapshot {
    Memory(memory::MemorySnapshot),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlite::SqliteSnapshot),
}

/// What an Area holds when a Commit runs, as the checks shared by every Backend need to see it.
/// Each Backend reads it its own way, under its lock.
pub(crate) trait AreaState {
    /// The Revision of the File at `path`, or `None` if there is none.
    fn revision(&self, path: &Path) -> Result<Option<Revision>>;
    /// The Path and Revision of every File under `prefix`, in order of Path.
    fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>>;
    /// The Path of every File under `prefix`, in order.
    fn paths_under(&self, prefix: &Prefix) -> Result<Vec<Path>>;
    /// Every Path with a name that folds to `fold` but isn't `name`, in any order. A Path's names
    /// are each Prefix it is under and the Path itself (`a/`, `a/b/` and `a/b/c` for `a/b/c`),
    /// and a Prefix folds as its name without the `/` that ends it
    /// ([`letter_case_fold`](crate::path::letter_case_fold)). Since folding keeps each `/` where
    /// it is, the Paths with a name that folds to `fold` are those whose own fold is `fold` or
    /// starts with `fold` and a `/`. [`has_name`](crate::staging::has_name) says whether a Path
    /// has the name `name`.
    ///
    /// A Backend keeps each Path's fold indexed, so that this costs what it finds, not what the
    /// Area holds.
    fn paths_named_like(&self, name: &str, fold: &str) -> Result<Vec<Path>>;
}

/// A Staging, with its Paths validated, ready for a Backend to commit.
#[derive(Debug)]
pub(crate) struct CommitRequest {
    /// The last-modified time every File written gets. The Store layer chooses it.
    pub(crate) timestamp: Timestamp,
    /// What to commit. The Backend expands its Prefix deletes under its lock.
    pub(crate) staged: Staged,
}

/// What a Commit changes, worked out by [`CommitRequest::plan`].
#[derive(Debug)]
pub(crate) struct Plan {
    /// What to do to each Path the Commit changes, in order of Path: write it, or remove it
    /// (`None`).
    changes: Vec<(Path, Option<Written>)>,
    /// The new Revision of each Path written, including writes left out because they would not
    /// have changed the contents.
    revisions: BTreeMap<Path, Revision>,
}

/// A File a Commit writes.
#[derive(Debug)]
pub(crate) struct Written {
    pub(crate) contents: String,
    pub(crate) stat: Stat,
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

impl CommitRequest {
    /// Works out what the Commit changes, given `current`, the Area as it is under the Backend's
    /// lock. These are the rules every Backend shares, in the order they apply:
    /// 1. every Precondition is checked, so a Commit that also breaks another rule is a Conflict;
    /// 2. the Prefix deletes are expanded;
    /// 3. writes that would not change the contents, and deletes of Paths with no File, are left
    ///    out;
    /// 4. names that some platform can't hold together are refused.
    pub(crate) fn plan(self, current: &impl AreaState) -> Result<Plan> {
        let CommitRequest { timestamp, mut staged } = self;
        staged.check_preconditions(current)?;
        staged.expand_prefix_deletes(current)?;
        let revisions = staged.leave_out_what_changes_nothing(current)?;
        staged.refuse_clashing_paths(current)?;
        let changes = staged.actions.into_iter().map(|(path, action)| {
            let written = match action {
                Action::Write(contents) => {
                    let stat = Stat::new(timestamp, revisions[&path]);
                    Some(Written { contents, stat })
                }
                Action::Delete => None,
            };
            (path, written)
        });
        Ok(Plan { changes: changes.collect(), revisions })
    }
}

impl Plan {
    /// Makes each change with `change`, which writes the File or removes it (`None`), in order of
    /// Path. The Backend makes them all under its lock, all-or-nothing.
    pub(crate) fn apply(
        self,
        mut change: impl FnMut(&Path, Option<Written>) -> Result<()>,
    ) -> Result<CommitOutcome> {
        let mut changes = Vec::with_capacity(self.changes.len());
        for (path, written) in self.changes {
            let kind = if written.is_some() { ChangeKind::Changed } else { ChangeKind::Removed };
            change(&path, written)?;
            changes.push(RawChange { path, kind });
        }
        Ok(CommitOutcome { revisions: self.revisions, changes })
    }
}

impl Backend {
    pub(crate) async fn read(&self, area: Area, path: &Path) -> Result<Option<File>> {
        match self {
            Backend::Memory(backend) => Ok(backend.read(area, path)),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.read(area, path).await,
        }
    }

    pub(crate) async fn stat(&self, area: Area, path: &Path) -> Result<Option<Stat>> {
        match self {
            Backend::Memory(backend) => Ok(backend.stat(area, path)),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.stat(area, path).await,
        }
    }

    /// The Paths under `prefix`, in order.
    pub(crate) async fn list(&self, area: Area, prefix: &Prefix) -> Result<Vec<Path>> {
        match self {
            Backend::Memory(backend) => Ok(backend.list(area, prefix)),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.list(area, prefix).await,
        }
    }

    /// The Prefix Revision of everything under `prefix`.
    pub(crate) async fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        match self {
            Backend::Memory(backend) => backend.stat_prefix(area, prefix),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.stat_prefix(area, prefix).await,
        }
    }

    /// Whether [`snapshot`](Self::snapshot) can give a Snapshot. A Backend that can't gives
    /// `Unsupported` from it instead.
    pub(crate) fn supports_snapshots(&self) -> bool {
        match self {
            Backend::Memory(_) => true,
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(_) => true,
        }
    }

    /// A view of `area` as it stands now, which Commits made afterwards don't change, and which
    /// doesn't hold them up.
    pub(crate) async fn snapshot(&self, area: Area) -> Result<BackendSnapshot> {
        match self {
            Backend::Memory(backend) => Ok(BackendSnapshot::Memory(backend.snapshot(area))),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => Ok(BackendSnapshot::Sqlite(backend.snapshot(area).await?)),
        }
    }

    /// Applies every write and delete in `request`, all-or-nothing, under the Backend's lock, as
    /// [`CommitRequest::plan`] works them out.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        match self {
            Backend::Memory(backend) => backend.commit(request),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.commit(request).await,
        }
    }
}

impl BackendSnapshot {
    pub(crate) async fn read(&self, path: &Path) -> Result<Option<File>> {
        match self {
            BackendSnapshot::Memory(snapshot) => Ok(snapshot.read(path)),
            #[cfg(feature = "sqlite")]
            BackendSnapshot::Sqlite(snapshot) => snapshot.read(path).await,
        }
    }

    pub(crate) async fn stat(&self, path: &Path) -> Result<Option<Stat>> {
        match self {
            BackendSnapshot::Memory(snapshot) => Ok(snapshot.stat(path)),
            #[cfg(feature = "sqlite")]
            BackendSnapshot::Sqlite(snapshot) => snapshot.stat(path).await,
        }
    }

    /// The Paths under `prefix`, in order.
    pub(crate) async fn list(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        match self {
            BackendSnapshot::Memory(snapshot) => Ok(snapshot.list(prefix)),
            #[cfg(feature = "sqlite")]
            BackendSnapshot::Sqlite(snapshot) => snapshot.list(prefix).await,
        }
    }
}
