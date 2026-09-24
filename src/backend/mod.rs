//! What actually holds a Store's Files. Backends are private to the crate: the Store layer does
//! everything that is the same for all of them, and calls into the Backend for the rest.

#[cfg(feature = "fs")]
pub(crate) mod fs;
pub(crate) mod memory;
#[cfg(feature = "sqlite")]
pub(crate) mod sqlite;

use std::collections::BTreeMap;

use jiff::Timestamp;

#[cfg(feature = "fs")]
use crate::Error;
use crate::staging::{Action, PlannedRevisions, Staged};
use crate::{Area, ChangeKind, File, Origin, Path, Prefix, PrefixRevision, Result, Revision, Stat};

/// The Backend a Store was opened on.
#[derive(Debug)]
pub(crate) enum Backend {
    #[cfg(feature = "fs")]
    Fs(fs::FsBackend),
    Memory(memory::MemoryBackend),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlite::SqliteBackend),
}

/// A Backend's view of one Area as it stood when a Snapshot was taken.
///
/// It holds only what the Backend needs to read that view, never the Store's shared state, so a
/// Snapshot doesn't keep the Change feed open after the last Store handle goes, and it can still
/// be read after that. Memory holds the Area's Files. SQLite holds a read transaction on a
/// connection of its own. The filesystem has none (ADR 0006).
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
    /// What to do to each Path the Commit changes, in order of Path.
    changes: Vec<(Path, Planned)>,
    /// The new Revision of each Path written, including writes left out because they would not
    /// have changed the contents.
    revisions: BTreeMap<Path, Revision>,
}

/// What a Commit does to one Path.
#[derive(Debug)]
pub(crate) enum Planned {
    /// Writes the File with these contents and this Stat.
    Write { contents: String, stat: Stat },
    /// Removes the File, which has this Revision.
    Remove {
        #[cfg_attr(
            not(feature = "fs"),
            expect(dead_code, reason = "only the filesystem journals it")
        )]
        revision: Revision,
    },
}

/// What a Commit did.
#[derive(Debug, Default)]
pub(crate) struct CommitOutcome {
    /// The new Revision of each Path written, including writes left out because they would not
    /// have changed the contents.
    pub(crate) revisions: BTreeMap<Path, Revision>,
    /// Each Path the Commit changed or removed, in order.
    pub(crate) changes: Vec<RawChange>,
    /// What other Stores did to the Area before this Commit that this Store hasn't recorded on
    /// its Change feed yet, in the order they did it. Only SQLite, which reads it from its change
    /// log, gives any.
    pub(crate) observed_before: Vec<Observed>,
    /// Whether the Commit has happened but isn't finished yet. Only the filesystem gives one, when
    /// a step after its journal is committed keeps failing. The Store records its Changes,
    /// then gives [`Error::Pending`](crate::Error::Pending).
    pub(crate) pending: bool,
}

/// Something a Backend observed in an Area, other than this Store's Commits as it makes them: in
/// SQLite's change log, which Commits by every Store on the same storage go into, or with the
/// filesystem's watcher.
#[derive(Debug)]
#[cfg_attr(
    not(any(feature = "fs", feature = "sqlite")),
    expect(dead_code, reason = "only SQLite and the filesystem observe other Stores")
)]
pub(crate) enum Observed {
    /// Changes to the Area, all with the same Origin, to go in the same batch: a Commit's, read
    /// from the change log, whose Origin is local if this Store made it, or all the watcher saw in
    /// one burst of events, which are external.
    Changes { origin: Origin, changes: Vec<RawChange> },
    /// Changes may have been missed: Commits were pruned from the log before this Store read
    /// them, or watching failed, or the Area's directory was removed.
    Missed,
}

/// A change a Backend made or observed, without its Area and Origin. The Store layer adds the Area,
/// and the Origin: local for its own Commits, and for what a Backend observed, the Origin it gives
/// with them ([`Observed::Changes`]).
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
        let PlannedRevisions { written: revisions, removed } =
            staged.leave_out_what_changes_nothing(current)?;
        staged.refuse_clashing_paths(current)?;
        let changes = staged.actions.into_iter().map(|(path, action)| {
            let planned = match action {
                Action::Write(contents) => {
                    Planned::Write { contents, stat: Stat::new(timestamp, revisions[&path]) }
                }
                Action::Delete => Planned::Remove { revision: removed[&path] },
            };
            (path, planned)
        });
        Ok(Plan { changes: changes.collect(), revisions })
    }
}

impl Plan {
    /// Makes each change with `change`, in order of Path. The Backend makes them all under its
    /// lock, all-or-nothing.
    pub(crate) fn apply(
        self,
        mut change: impl FnMut(&Path, Planned) -> Result<()>,
    ) -> Result<CommitOutcome> {
        let mut changes = Vec::with_capacity(self.changes.len());
        for (path, planned) in self.changes {
            let kind = match planned {
                Planned::Write { .. } => ChangeKind::Changed,
                Planned::Remove { .. } => ChangeKind::Removed,
            };
            change(&path, planned)?;
            changes.push(RawChange { path, kind });
        }
        Ok(CommitOutcome {
            revisions: self.revisions,
            changes,
            observed_before: Vec::new(),
            pending: false,
        })
    }
}

impl Backend {
    pub(crate) async fn read(&self, area: Area, path: &Path) -> Result<Option<File>> {
        match self {
            #[cfg(feature = "fs")]
            Backend::Fs(backend) => backend.read(area, path).await,
            Backend::Memory(backend) => Ok(backend.read(area, path)),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.read(area, path).await,
        }
    }

    pub(crate) async fn stat(&self, area: Area, path: &Path) -> Result<Option<Stat>> {
        match self {
            #[cfg(feature = "fs")]
            Backend::Fs(backend) => backend.stat(area, path).await,
            Backend::Memory(backend) => Ok(backend.stat(area, path)),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.stat(area, path).await,
        }
    }

    /// The Paths under `prefix`, in order.
    pub(crate) async fn list(&self, area: Area, prefix: &Prefix) -> Result<Vec<Path>> {
        match self {
            #[cfg(feature = "fs")]
            Backend::Fs(backend) => backend.list(area, prefix).await,
            Backend::Memory(backend) => Ok(backend.list(area, prefix)),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.list(area, prefix).await,
        }
    }

    /// The Prefix Revision of everything under `prefix`.
    pub(crate) async fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        match self {
            #[cfg(feature = "fs")]
            Backend::Fs(backend) => backend.stat_prefix(area, prefix).await,
            Backend::Memory(backend) => backend.stat_prefix(area, prefix),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => backend.stat_prefix(area, prefix).await,
        }
    }

    /// Whether [`snapshot`](Self::snapshot) can give a Snapshot. A Backend that can't gives
    /// `Unsupported` from it instead.
    pub(crate) fn supports_snapshots(&self) -> bool {
        match self {
            #[cfg(feature = "fs")]
            Backend::Fs(_) => false,
            Backend::Memory(_) => true,
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(_) => true,
        }
    }

    /// A view of `area` as it stands now, which Commits made afterwards don't change, and which
    /// doesn't hold them up.
    pub(crate) async fn snapshot(&self, area: Area) -> Result<BackendSnapshot> {
        match self {
            #[cfg(feature = "fs")]
            Backend::Fs(_) => Err(Error::Unsupported),
            Backend::Memory(backend) => Ok(BackendSnapshot::Memory(backend.snapshot(area))),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(backend) => Ok(BackendSnapshot::Sqlite(backend.snapshot(area).await?)),
        }
    }

    /// Applies every write and delete in `request`, all-or-nothing, under the Backend's lock, as
    /// [`CommitRequest::plan`] works them out.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        match self {
            #[cfg(feature = "fs")]
            Backend::Fs(backend) => backend.commit(request).await,
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

/// Runs `call` on one of tokio's blocking threads, so that it doesn't hold up the async runtime.
#[cfg(any(feature = "fs", feature = "sqlite"))]
async fn off_runtime<T: Send + 'static>(
    call: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tokio::task::spawn_blocking(call).await {
        Ok(result) => result,
        Err(error) => match error.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // The runtime is shutting down.
            Err(error) => Err(crate::Error::backend(error)),
        },
    }
}
