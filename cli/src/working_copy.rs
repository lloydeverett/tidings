//! A Working copy: a folder holding one Area's Files as ordinary files, with its record in
//! `.tidings/` (ADR 0008). It owns everything about the folder and the record: creating and
//! opening one, locking it, reading the folder, reconciling Paths against the Store, and
//! committing local changes. Each operation gives a report for the output module to print.

mod record;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, TryLockError};
use std::io::{self, ErrorKind};
use std::path::{Path as FsPath, PathBuf};

use tidings::{Area, File, Path, Precondition, Revision, Staging, Store};

use crate::failure::Failure;
use crate::location::{Opened, StoreAddress};
use record::{Base, Hash, Record};

/// The directory in a Working copy's folder that holds its record and locks, which is never a
/// File.
const RECORD_DIRECTORY: &str = ".tidings";

/// The record's file in [`RECORD_DIRECTORY`].
const RECORD_FILE: &str = "working-copy";

/// The lock file in [`RECORD_DIRECTORY`] that [`Lock`] holds.
const LOCK_FILE: &str = "lock";

/// The lock file in [`RECORD_DIRECTORY`] that [`SyncLock`] holds.
const SYNC_LOCK_FILE: &str = "sync.lock";

/// A folder that is a Working copy, and the Store and Area its record names, which never change.
/// Its Bases are read from the record only under [`Lock`], since another command may change them.
pub struct WorkingCopy {
    folder: PathBuf,
    store: StoreAddress,
    area: Area,
}

/// Held for as long as a `sync` runs: the lock on `.tidings/sync.lock`.
pub struct SyncLock {
    _file: fs::File,
}

/// Held while a command reads the folder to act on it, or changes the folder or the record: the
/// lock on `.tidings/lock`, and the record as it was read under it.
struct Lock {
    _file: fs::File,
    record: Record,
}

/// Something `sync` did, or found.
#[derive(Debug)]
pub enum SyncEvent {
    /// It wrote a File the folder didn't have.
    Created(Path),
    /// It wrote a newer version of a File over the local one.
    Updated(Path),
    /// It removed a File the Store no longer has.
    Removed(Path),
    /// It has applied everything it knows of.
    CaughtUp,
}

/// What `commit` committed: nothing, if there were no local changes.
#[derive(Debug)]
pub struct CommitReport {
    pub changes: Vec<CommittedChange>,
}

/// One local change that was committed.
#[derive(Debug)]
pub struct CommittedChange {
    pub path: Path,
    pub change: LocalChange,
    /// The File's new Revision, or `None` if it was deleted.
    pub revision: Option<Revision>,
}

/// How a Path in the folder differs from its Base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalChange {
    /// A file with no Base.
    Added,
    /// A file whose contents differ from its Base's.
    Modified,
    /// A Path with a Base, but no file.
    Deleted,
}

/// What the folder holds at a Path.
enum Local {
    Absent,
    File(String),
    /// Something that isn't a regular file holding text, such as a directory.
    Other,
}

impl WorkingCopy {
    /// Makes `folder`, which must be missing or empty, a Working copy of `area` in `store`, with
    /// no Files yet. A `.tidings/` holding no record, as a `sync` that stopped while making the
    /// Working copy leaves, counts as empty.
    pub fn create(
        folder: &FsPath,
        store: StoreAddress,
        area: Area,
    ) -> Result<WorkingCopy, Failure> {
        let refuse = |what: &str| {
            Failure::error(format!(
                "{} {what}: sync into an empty or missing folder",
                folder.display()
            ))
        };
        if !is_empty(folder)? {
            return Err(refuse(if record::is_record(&record_file(folder)) {
                "is a Working copy already"
            } else {
                "isn't empty"
            }));
        }
        fs::create_dir_all(folder.join(RECORD_DIRECTORY)).map_err(failed_at(folder))?;
        let working_copy = WorkingCopy { folder: folder.to_owned(), store, area };
        let _lock = working_copy.lock_file(LOCK_FILE, true)?;
        // Another `sync` may have made it a Working copy since the folder was found empty.
        if record_file(folder).exists() {
            return Err(refuse("is a Working copy already"));
        }
        working_copy.save(&Record::new(working_copy.store.clone(), area))?;
        Ok(working_copy)
    }

    /// Opens the Working copy that is `folder`, reading which Store and Area it belongs to.
    pub fn open(folder: &FsPath) -> Result<WorkingCopy, Failure> {
        let file = record_file(folder);
        if !record::is_record(&file) {
            return Err(Failure::error(format!(
                "{} is not a Working copy: `tidings sync` one first",
                folder.display()
            )));
        }
        let Record { store, area, .. } = Record::read(&file)?;
        Ok(WorkingCopy { folder: folder.to_owned(), store, area })
    }

    /// Opens the Working copy `start` is in: the first folder from `start` up that has a record.
    /// A `.tidings/` directory alone, as in a filesystem Area, doesn't make one.
    pub fn find(start: &FsPath) -> Result<WorkingCopy, Failure> {
        match start.ancestors().find(|folder| record::is_record(&record_file(folder))) {
            Some(folder) => WorkingCopy::open(folder),
            None => Err(Failure::error(format!(
                "{} is not in a Working copy: `tidings sync` one first, or name one with -C",
                start.display()
            ))),
        }
    }

    /// Opens the Store the record names.
    pub async fn open_store(&self) -> Result<Opened, Failure> {
        self.store.open(false).await
    }

    /// Locks `.tidings/sync.lock`, for as long as a `sync` runs.
    pub fn lock_for_sync(&self) -> Result<SyncLock, Failure> {
        let file = self.lock_file(SYNC_LOCK_FILE, false)?;
        match file.try_lock() {
            Ok(()) => Ok(SyncLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Failure::error(format!(
                "a `tidings sync` of {} is running already",
                self.folder.display()
            ))),
            Err(TryLockError::Error(error)) => Err(self.failed("lock", error)),
        }
    }

    /// Reconciles every Path in the Area and in the record with the Store: where the local file is
    /// unchanged since its Base, or absent with no Base, the Store's version is applied to the
    /// folder and becomes the Base. A local change is left alone.
    pub async fn reconcile_all(&self, store: &Store) -> Result<Vec<SyncEvent>, Failure> {
        let mut lock = self.lock()?;
        let bases = &mut lock.record.bases;
        let mut paths: BTreeSet<Path> = store.list(self.area, "").await?.into_iter().collect();
        paths.extend(bases.keys().cloned());
        let mut events = Vec::new();
        for path in paths {
            events.extend(self.reconcile(store, bases, path).await?);
        }
        // The folder is changed first and the record saved after.
        self.save(&lock.record)?;
        Ok(events)
    }

    /// Reconciles `path` with the Store, updating its Base in `bases`.
    async fn reconcile(
        &self,
        store: &Store,
        bases: &mut BTreeMap<Path, Base>,
        path: Path,
    ) -> Result<Option<SyncEvent>, Failure> {
        let theirs = store.read(self.area, &path).await?;
        let base = bases.get(&path).copied();
        if theirs.as_ref().map(File::revision) == base.map(|base| base.revision) {
            return Ok(None);
        }
        let local = self.local(&path)?;
        let unchanged = match (&local, base) {
            (Local::Absent, None) => true,
            (Local::File(contents), Some(base)) => Hash::of(contents) == base.hash,
            _ => false,
        };
        let same_as_theirs = match (&local, &theirs) {
            (Local::Absent, None) => true,
            (Local::File(contents), Some(file)) => contents == file.contents(),
            _ => false,
        };
        if !unchanged && !same_as_theirs {
            // A local change stays as it is.
            return Ok(None);
        }
        let event = if unchanged { self.apply(&path, &local, theirs.as_ref())? } else { None };
        match theirs {
            Some(file) => {
                let base = Base { revision: file.revision(), hash: Hash::of(file.contents()) };
                bases.insert(path, base);
            }
            None => {
                bases.remove(&path);
            }
        }
        Ok(event)
    }

    /// Makes the folder hold `theirs` at `path`, where it now holds `local`.
    fn apply(
        &self,
        path: &Path,
        local: &Local,
        theirs: Option<&File>,
    ) -> Result<Option<SyncEvent>, Failure> {
        let at = self.path_in_folder(path);
        let failed = failed_at(&at);
        match (local, theirs) {
            (_, Some(file)) => {
                if let Some(parent) = at.parent() {
                    fs::create_dir_all(parent).map_err(failed)?;
                }
                fs::write(&at, file.contents()).map_err(failed)?;
                Ok(Some(match local {
                    Local::Absent => SyncEvent::Created(path.clone()),
                    _ => SyncEvent::Updated(path.clone()),
                }))
            }
            (Local::Absent, None) => Ok(None),
            (_, None) => {
                fs::remove_file(&at).map_err(failed)?;
                Ok(Some(SyncEvent::Removed(path.clone())))
            }
        }
    }

    /// Commits every local change as one Commit: an added file requires the Path to be absent in
    /// the Store, and a modified or deleted one that the File is unchanged since its Base. The
    /// committed Revisions become the new Bases.
    pub async fn commit(&self, store: &Store) -> Result<CommitReport, Failure> {
        let mut lock = self.lock()?;
        let bases = &mut lock.record.bases;
        let files = self.scan()?;
        let mut staging = Staging::new(self.area);
        let mut changes = Vec::new();
        for (path, contents) in &files {
            let (change, precondition) = match bases.get(path) {
                None => (LocalChange::Added, Precondition::Absent),
                Some(base) if Hash::of(contents) != base.hash => {
                    (LocalChange::Modified, Precondition::UnchangedSince(base.revision))
                }
                Some(_) => continue,
            };
            staging.write_requiring(path, contents.as_str(), precondition)?;
            changes.push((path.clone(), change));
        }
        for (path, base) in bases.iter() {
            if !files.contains_key(path) {
                staging.delete_requiring(path, Precondition::UnchangedSince(base.revision))?;
                changes.push((path.clone(), LocalChange::Deleted));
            }
        }
        if changes.is_empty() {
            return Ok(CommitReport { changes: Vec::new() });
        }
        let committed = store.commit(staging).await?;
        let mut report = CommitReport { changes: Vec::new() };
        for (path, change) in changes {
            let revision = committed.revisions().get(&path).copied();
            match (revision, files.get(&path)) {
                (Some(revision), Some(contents)) => {
                    let base = Base { revision, hash: Hash::of(contents) };
                    bases.insert(path.clone(), base);
                }
                _ => {
                    bases.remove(&path);
                }
            }
            report.changes.push(CommittedChange { path, change, revision });
        }
        self.save(&lock.record)?;
        Ok(report)
    }

    /// What the folder holds at `path`.
    fn local(&self, path: &Path) -> Result<Local, Failure> {
        let at = self.path_in_folder(path);
        let metadata = match fs::symlink_metadata(&at) {
            Ok(metadata) => metadata,
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
            {
                return Ok(Local::Absent);
            }
            Err(error) => return Err(failed_at(&at)(error)),
        };
        if !metadata.is_file() {
            return Ok(Local::Other);
        }
        match fs::read_to_string(&at) {
            Ok(contents) => Ok(Local::File(contents)),
            Err(error) if error.kind() == ErrorKind::InvalidData => Ok(Local::Other),
            Err(error) => Err(failed_at(&at)(error)),
        }
    }

    /// Every file in the folder, outside `.tidings/`, with its contents.
    fn scan(&self) -> Result<BTreeMap<Path, String>, Failure> {
        let mut files = BTreeMap::new();
        self.scan_directory(&self.folder, "", &mut files)?;
        Ok(files)
    }

    /// Adds every file under `directory`, whose Prefix is `prefix`, to `files`.
    fn scan_directory(
        &self,
        directory: &FsPath,
        prefix: &str,
        files: &mut BTreeMap<Path, String>,
    ) -> Result<(), Failure> {
        let in_directory = failed_at(directory);
        for entry in fs::read_dir(directory).map_err(in_directory)? {
            let entry = entry.map_err(in_directory)?;
            let at = entry.path();
            let invalid = |why: &str| Failure::error(format!("{} {why}", at.display()));
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(invalid("has a name that isn't UTF-8"));
            };
            if prefix.is_empty() && name.eq_ignore_ascii_case(RECORD_DIRECTORY) {
                continue;
            }
            let file_type = entry.file_type().map_err(in_directory)?;
            if file_type.is_dir() {
                self.scan_directory(&at, &format!("{prefix}{name}/"), files)?;
            } else if file_type.is_file() {
                let path = Path::new(format!("{prefix}{name}"))?;
                let contents = fs::read_to_string(&at).map_err(|error| match error.kind() {
                    ErrorKind::InvalidData => invalid("isn't UTF-8 text"),
                    _ => failed_at(&at)(error),
                })?;
                files.insert(path, contents);
            } else {
                return Err(invalid("isn't a regular file"));
            }
        }
        Ok(())
    }

    /// Locks `.tidings/lock`, waiting for any other command holding it, and reads the record
    /// under it.
    fn lock(&self) -> Result<Lock, Failure> {
        let file = self.lock_file(LOCK_FILE, true)?;
        let record = Record::read(&record_file(&self.folder))?;
        Ok(Lock { _file: file, record })
    }

    /// Opens `.tidings/<name>`, making it if need be, and locks it if `wait`, waiting for any
    /// other command holding it.
    fn lock_file(&self, name: &str, wait: bool) -> Result<fs::File, Failure> {
        let path = self.folder.join(RECORD_DIRECTORY).join(name);
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path);
        let file = file.map_err(failed_at(&path))?;
        if wait {
            file.lock().map_err(|error| self.failed("lock", error))?;
        }
        Ok(file)
    }

    /// Saves `record`, whole.
    fn save(&self, record: &Record) -> Result<(), Failure> {
        record.write(&record_file(&self.folder))
    }

    /// The file in the folder that holds `path`.
    fn path_in_folder(&self, path: &Path) -> PathBuf {
        self.folder.join(path.as_str())
    }

    /// The failure for being unable to `what` the folder, as in "lock", because of `error`.
    fn failed(&self, what: &str, error: io::Error) -> Failure {
        Failure::error(format!("can't {what} {}: {error}", self.folder.display()))
    }
}

/// Where the Working copy that is `folder` keeps its record.
fn record_file(folder: &FsPath) -> PathBuf {
    folder.join(RECORD_DIRECTORY).join(RECORD_FILE)
}

/// Whether `folder` is missing or empty, counting a `.tidings/` that holds no record, only what
/// making a Working copy leaves before it writes one, as nothing.
fn is_empty(folder: &FsPath) -> Result<bool, Failure> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(failed_at(folder)(error)),
    };
    for entry in entries {
        let entry = entry.map_err(failed_at(folder))?;
        let is_directory = entry.file_type().map_err(failed_at(folder))?.is_dir();
        if entry.file_name() != RECORD_DIRECTORY
            || !is_directory
            || !holds_no_record(&entry.path())?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether `directory`, a `.tidings/`, holds only what making a Working copy leaves before it
/// writes the record: the lock files, and a temporary file the record was being written to.
fn holds_no_record(directory: &FsPath) -> Result<bool, Failure> {
    let temporary = format!(".{RECORD_FILE}.");
    for entry in fs::read_dir(directory).map_err(failed_at(directory))? {
        let name = entry.map_err(failed_at(directory))?.file_name();
        let name = name.to_string_lossy();
        if name != LOCK_FILE && name != SYNC_LOCK_FILE && !name.starts_with(&temporary) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Turns an I/O error about `path` into a Failure that names it.
fn failed_at(path: &FsPath) -> impl Fn(io::Error) -> Failure + Copy + '_ {
    move |error| Failure::from(error).in_context(path.display())
}
