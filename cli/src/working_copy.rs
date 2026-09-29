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
use crate::location::{BackendName, Opened, StoreLocation};
use record::{Base, Hash, Record};

/// The directory in a Working copy's folder that holds its record, which is never a File.
const TIDINGS: &str = ".tidings";

/// A folder that is a Working copy, and its record.
pub struct WorkingCopy {
    folder: PathBuf,
    record: Record,
}

/// Held for as long as a `sync` runs: the lock on `.tidings/sync.lock`.
pub struct SyncLock {
    _file: fs::File,
}

/// Held while a command reads the folder to act on it, or changes the folder or the record: the
/// lock on `.tidings/lock`.
struct Lock {
    _file: fs::File,
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
    /// Makes `folder`, which must be missing or empty, a Working copy of `area` in the Store at
    /// `location`, on `backend`, with no Files yet.
    pub fn create(
        folder: &FsPath,
        location: StoreLocation,
        backend: BackendName,
        area: Area,
    ) -> Result<WorkingCopy, Failure> {
        let in_folder = |error: io::Error| Failure::from(error).in_context(folder.display());
        match fs::read_dir(folder).map(|mut entries| entries.next().is_none()) {
            Ok(false) => {
                let what = if record::is_record(&record_file(folder)) {
                    "is a Working copy already"
                } else {
                    "isn't empty"
                };
                return Err(Failure::error(format!(
                    "{} {what}: sync into an empty or missing folder",
                    folder.display()
                )));
            }
            Ok(true) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(in_folder(error)),
        }
        fs::create_dir_all(folder.join(TIDINGS)).map_err(in_folder)?;
        let working_copy =
            WorkingCopy { folder: folder.to_owned(), record: Record::new(location, backend, area) };
        let _lock = working_copy.lock()?;
        working_copy.save()?;
        Ok(working_copy)
    }

    /// Opens the Working copy that is `folder`.
    pub fn open(folder: &FsPath) -> Result<WorkingCopy, Failure> {
        let file = record_file(folder);
        if !record::is_record(&file) {
            return Err(Failure::error(format!(
                "{} is not a Working copy: `tidings sync` one first",
                folder.display()
            )));
        }
        Ok(WorkingCopy { folder: folder.to_owned(), record: Record::read(&file)? })
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
        self.record.location.open(Some(self.record.backend), false).await
    }

    /// Locks `.tidings/sync.lock`, for as long as a `sync` runs.
    pub fn lock_for_sync(&self) -> Result<SyncLock, Failure> {
        let file = self.lock_file("sync.lock")?;
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
    pub async fn reconcile_all(&mut self, store: &Store) -> Result<Vec<SyncEvent>, Failure> {
        let _lock = self.lock()?;
        let mut paths: BTreeSet<Path> =
            store.list(self.record.area, "").await?.into_iter().collect();
        paths.extend(self.record.bases.keys().cloned());
        let mut events = Vec::new();
        for path in paths {
            events.extend(self.reconcile(store, path).await?);
        }
        // The folder is changed first and the record saved after.
        self.save()?;
        Ok(events)
    }

    /// Reconciles `path` with the Store, without saving the record.
    async fn reconcile(&mut self, store: &Store, path: Path) -> Result<Option<SyncEvent>, Failure> {
        let theirs = store.read(self.record.area, &path).await?;
        let base = self.record.bases.get(&path).copied();
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
                self.record.bases.insert(path, base);
            }
            None => {
                self.record.bases.remove(&path);
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
        let at = self.at(path);
        let failed = |error| Failure::from(error).in_context(at.display());
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
    pub async fn commit(&mut self, store: &Store) -> Result<CommitReport, Failure> {
        let _lock = self.lock()?;
        let files = self.scan()?;
        let mut staging = Staging::new(self.record.area);
        let mut changes = Vec::new();
        for (path, contents) in &files {
            let (change, precondition) = match self.record.bases.get(path) {
                None => (LocalChange::Added, Precondition::Absent),
                Some(base) if Hash::of(contents) != base.hash => {
                    (LocalChange::Modified, Precondition::UnchangedSince(base.revision))
                }
                Some(_) => continue,
            };
            staging.write_requiring(path, contents.as_str(), precondition)?;
            changes.push((path.clone(), change));
        }
        for (path, base) in &self.record.bases {
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
                    self.record.bases.insert(path.clone(), base);
                }
                _ => {
                    self.record.bases.remove(&path);
                }
            }
            report.changes.push(CommittedChange { path, change, revision });
        }
        self.save()?;
        Ok(report)
    }

    /// What the folder holds at `path`.
    fn local(&self, path: &Path) -> Result<Local, Failure> {
        let at = self.at(path);
        let metadata = match fs::symlink_metadata(&at) {
            Ok(metadata) => metadata,
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
            {
                return Ok(Local::Absent);
            }
            Err(error) => return Err(Failure::from(error).in_context(at.display())),
        };
        if !metadata.is_file() {
            return Ok(Local::Other);
        }
        match fs::read_to_string(&at) {
            Ok(contents) => Ok(Local::File(contents)),
            Err(error) if error.kind() == ErrorKind::InvalidData => Ok(Local::Other),
            Err(error) => Err(Failure::from(error).in_context(at.display())),
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
        let in_directory = |error: io::Error| Failure::from(error).in_context(directory.display());
        for entry in fs::read_dir(directory).map_err(in_directory)? {
            let entry = entry.map_err(in_directory)?;
            let at = entry.path();
            let invalid = |why: &str| Failure::error(format!("{} {why}", at.display()));
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(invalid("has a name that isn't UTF-8"));
            };
            if prefix.is_empty() && name.eq_ignore_ascii_case(TIDINGS) {
                continue;
            }
            let file_type = entry.file_type().map_err(in_directory)?;
            if file_type.is_dir() {
                self.scan_directory(&at, &format!("{prefix}{name}/"), files)?;
            } else if file_type.is_file() {
                let path = Path::new(format!("{prefix}{name}"))?;
                let contents = fs::read_to_string(&at).map_err(|error| match error.kind() {
                    ErrorKind::InvalidData => invalid("isn't UTF-8 text"),
                    _ => Failure::from(error).in_context(at.display()),
                })?;
                files.insert(path, contents);
            } else {
                return Err(invalid("isn't a regular file"));
            }
        }
        Ok(())
    }

    /// Locks `.tidings/lock`, waiting for any other command holding it.
    fn lock(&self) -> Result<Lock, Failure> {
        let file = self.lock_file("lock")?;
        file.lock().map_err(|error| self.failed("lock", error))?;
        Ok(Lock { _file: file })
    }

    /// Opens `.tidings/<name>`, to lock, making it if need be.
    fn lock_file(&self, name: &str) -> Result<fs::File, Failure> {
        let path = self.folder.join(TIDINGS).join(name);
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path);
        file.map_err(|error| Failure::from(error).in_context(path.display()))
    }

    /// Saves the record, whole.
    fn save(&self) -> Result<(), Failure> {
        self.record.write(&record_file(&self.folder))
    }

    /// Where the folder holds `path`.
    fn at(&self, path: &Path) -> PathBuf {
        self.folder.join(path.as_str())
    }

    fn failed(&self, what: &str, error: io::Error) -> Failure {
        Failure::error(format!("can't {what} {}: {error}", self.folder.display()))
    }
}

/// Where the Working copy that is `folder` keeps its record.
fn record_file(folder: &FsPath) -> PathBuf {
    folder.join(TIDINGS).join("working-copy")
}
