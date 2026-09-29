//! A Working copy: a folder holding one Area's Files as ordinary files, with its record in
//! `.tidings/` (ADR 0008). It owns everything about the folder and the record: creating and
//! opening one, locking it, reading the folder, reconciling Paths against the Store, and
//! committing local changes. Each operation gives a report for the output module to print.

mod record;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, TryLockError};
use std::future::{self, Future};
use std::io::{self, ErrorKind, Write};
use std::path::{Path as FsPath, PathBuf};
use std::pin::{Pin, pin};

use tidings::{Area, ChangeFeed, FeedItem, File, Path, Precondition, Revision, Staging, Store};

use crate::failure::Failure;
use crate::location::{Opened, StoreAddress, StoreArgs};
use crate::output::area_name;
use record::{Base, Divergence, Hash, Record};

/// The directory in a Working copy's folder that holds its record and locks, which is never a
/// File.
const RECORD_DIRECTORY: &str = ".tidings";

/// The record's file in [`RECORD_DIRECTORY`].
const RECORD_FILE: &str = "working-copy";

/// The lock file in [`RECORD_DIRECTORY`] that [`Lock`] holds.
const LOCK_FILE: &str = "lock";

/// The lock file in [`RECORD_DIRECTORY`] that [`SyncLock`] holds.
const SYNC_LOCK_FILE: &str = "sync.lock";

/// The directory in [`RECORD_DIRECTORY`] where a file is written before it is renamed into place.
/// Being inside the folder keeps the rename on one volume.
const TMP_DIRECTORY: &str = "tmp";

/// The directory in [`RECORD_DIRECTORY`] that holds the Store's version of each Diverged Path, at
/// its Path.
const THEIRS_DIRECTORY: &str = "theirs";

/// A folder that is a Working copy, and the Store and Area its record names, which never change.
/// Its Bases are read from the record only under [`Lock`], since another command may change them.
pub struct WorkingCopy {
    folder: PathBuf,
    store: StoreAddress,
    area: Area,
}

/// Held for as long as a `sync` runs: the lock on `.tidings/sync.lock`.
struct SyncLock {
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
    /// The Path changed both locally and in the Store since its Base, or the Store's version
    /// couldn't be applied because of what is in the folder, so it is Diverged: the local file and
    /// the Base stay as they were.
    Diverged {
        path: Path,
        /// The file holding the Store's version, relative to the folder, or `None` if the Store
        /// has no File there.
        theirs_file: Option<PathBuf>,
        /// What in the folder kept the Store's version from being applied, if anything.
        blocked: Option<Blocked>,
    },
    /// A Diverged Path's local contents came to equal the Store's, or the Store's came back to its
    /// Base, so it is no longer Diverged.
    Resolved(Path),
    /// A Path's `theirs` file couldn't be written or removed. The Path is Diverged or not all the
    /// same, and each later reconcile tries again, reporting this only once while `sync` runs,
    /// unless the Path's Divergence changes.
    Error {
        /// The Path whose `theirs` file it is.
        path: Path,
        /// What couldn't be done and why, as in "Diverged, but can't write
        /// .tidings/theirs/a: Is a directory (os error 21)".
        message: String,
    },
    /// Changes to the Area may have been missed, so it reconciles every Path.
    Resync,
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

/// Which Paths a reconcile covers.
enum Paths {
    /// Every Path in the Area and in the record.
    All,
    /// Only these.
    Only(BTreeSet<Path>),
}

/// What in the folder keeps the Store's version of a Path from being applied there: one of the
/// directories the Path is in that isn't one, given relative to the folder, or a directory at the
/// Path itself.
#[derive(Debug)]
pub enum Blocked {
    /// A symlink, through which something outside the folder would be reached.
    Symlink(PathBuf),
    /// A file, or anything else that isn't a directory.
    NotADirectory(PathBuf),
    /// A directory where the File would be written.
    Directory(Path),
}

impl Blocked {
    /// Why this blocks a Path, as in "q isn't a directory".
    pub fn reason(&self) -> String {
        match self {
            Blocked::Symlink(directory) => format!("{} is a symlink", directory.display()),
            Blocked::NotADirectory(directory) => {
                format!("{} isn't a directory", directory.display())
            }
            Blocked::Directory(path) => format!("{path} is a directory"),
        }
    }
}

/// What the folder holds at a Path.
enum Local {
    Absent,
    File(String),
    /// A directory, which holds no File at the Path, as when it is absent, but stands where the
    /// Store's version would be written.
    Directory,
    /// Something that isn't a regular file holding text: a symlink, a file that isn't UTF-8, or
    /// a special file.
    Other,
}

impl WorkingCopy {
    /// `sync`: makes `folder` a Working copy of `area` in the Store `flags` choose, or resumes the
    /// Working copy it is, which must be of `area`, and keeps it in step with its Store until
    /// `stop` finishes, giving each [`SyncEvent`] to `report` as it happens. `stop` is acted on
    /// only between reconciles, so each one finishes and saves the record.
    ///
    /// Only one `sync` of a Working copy runs at a time: a second is refused before it opens the
    /// Store.
    pub async fn sync(
        folder: &FsPath,
        area: Area,
        flags: &StoreArgs,
        stop: impl Future<Output = ()>,
        report: impl FnMut(&SyncEvent) -> io::Result<()>,
    ) -> Result<(), Failure> {
        let (working_copy, _syncing, mut opened) =
            WorkingCopy::open_or_create(folder, area, flags).await?;
        working_copy.follow(&opened.store, &mut opened.feed, stop, report).await
    }

    /// Opens the Working copy that is `folder`, or makes `folder` a new one, for `sync`, as
    /// [`WorkingCopy::sync`] says. Gives the lock `sync` holds for as long as it runs, and the
    /// Store, opened with its Change feed before anything is reconciled.
    async fn open_or_create(
        folder: &FsPath,
        area: Area,
        flags: &StoreArgs,
    ) -> Result<(WorkingCopy, SyncLock, Opened), Failure> {
        if WorkingCopy::exists(folder) {
            let working_copy = WorkingCopy::open(folder)?;
            working_copy.check_area(area)?;
            let syncing = working_copy.lock_for_sync()?;
            let opened = working_copy.open_store(flags).await?;
            return Ok((working_copy, syncing, opened));
        }
        // The flags and the folder are checked before anything is made, so that a refused `sync`
        // makes neither the Store nor the folder.
        let store = flags.address_for_working_copy().await?;
        WorkingCopy::check_can_create(folder)?;
        fs::create_dir_all(folder.join(RECORD_DIRECTORY)).map_err(failed_at(folder))?;
        let working_copy = WorkingCopy { folder: folder.to_owned(), store, area };
        let syncing = working_copy.lock_for_sync()?;
        // Checked again under the lock, since another `sync` may have made it a Working copy
        // meanwhile.
        WorkingCopy::check_can_create(folder)?;
        let opened = working_copy.store.open_or_make().await?;
        working_copy.save_new_record()?;
        Ok((working_copy, syncing, opened))
    }

    /// Fails unless `folder` can become a Working copy: it must be missing or empty. A `.tidings/`
    /// holding no record, as a `sync` that stopped while making the Working copy leaves, counts as
    /// empty.
    fn check_can_create(folder: &FsPath) -> Result<(), Failure> {
        if is_missing_empty_or_unfinished(folder)? {
            return Ok(());
        }
        Err(refuse_to_create(
            folder,
            if record::is_record(&record_file(folder)) {
                "is a Working copy already"
            } else {
                "isn't empty"
            },
        ))
    }

    /// Saves the record of a new Working copy, with no Files yet, once
    /// [`WorkingCopy::check_can_create`] has passed the folder under the [`SyncLock`].
    fn save_new_record(&self) -> Result<(), Failure> {
        let _lock = self.lock_waiting(LOCK_FILE)?;
        self.save(&Record::new(self.store.clone(), self.area))
    }

    /// Whether `folder` is a Working copy: whether it has a record, of this version or another.
    fn exists(folder: &FsPath) -> bool {
        record::is_record(&record_file(folder))
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

    /// Fails unless the Working copy is of `area`.
    fn check_area(&self, area: Area) -> Result<(), Failure> {
        if area == self.area {
            return Ok(());
        }
        Err(Failure::error(format!(
            "{} is a Working copy of the {} Area, not {}",
            self.folder.display(),
            area_name(self.area),
            area_name(area),
        )))
    }

    /// Opens the Store the record names. Any Store flags given in `flags` must match it, and
    /// `--create` is refused: a Working copy's Store is never made anew, since reconciling against
    /// an empty one would remove every unchanged file from the folder.
    pub async fn open_store(&self, flags: &StoreArgs) -> Result<Opened, Failure> {
        flags.check_matches(&self.store)?;
        match self.store.open().await {
            Ok(Some(opened)) => Ok(opened),
            Ok(None) => Err(Failure::error(format!(
                "the Store of the Working copy {} is missing at {}",
                self.folder.display(),
                self.store,
            ))),
            Err(failure) => Err(failure.in_context(format!(
                "can't open the Store of the Working copy {}",
                self.folder.display()
            ))),
        }
    }

    /// Keeps the folder in step with the Store until `stop` finishes, as [`WorkingCopy::sync`]
    /// says. It reconciles every Path first, then the Paths of each batch of Changes to the Area
    /// on `feed`, and every Path again on a Resync for the Area. `feed` must have been taken when
    /// `store` was opened, so that nothing committed since is missed.
    async fn follow(
        &self,
        store: &Store,
        feed: &mut ChangeFeed,
        stop: impl Future<Output = ()>,
        mut report: impl FnMut(&SyncEvent) -> io::Result<()>,
    ) -> Result<(), Failure> {
        let mut stop = pin!(stop);
        let mut next = Paths::All;
        // Only for as long as this `sync` runs, so that a restarted one reports each again.
        let mut theirs_failures = BTreeSet::new();
        loop {
            for event in self.reconcile(store, next, &mut theirs_failures).await? {
                report(&event)?;
            }
            match self.wait_for_paths(feed, stop.as_mut(), &mut report).await? {
                Some(paths) => next = paths,
                None => return Ok(()),
            }
        }
    }

    /// Waits on `feed` for the Paths to reconcile next: those of a batch of Changes to the Area, or
    /// every Path on a Resync for it, which is reported. Before waiting on an empty feed, reports
    /// that `sync` is caught up, so anything already waiting is reconciled first. Gives `None`
    /// once `stop` finishes.
    async fn wait_for_paths(
        &self,
        feed: &mut ChangeFeed,
        mut stop: Pin<&mut impl Future<Output = ()>>,
        report: &mut impl FnMut(&SyncEvent) -> io::Result<()>,
    ) -> Result<Option<Paths>, Failure> {
        let mut caught_up = false;
        loop {
            let item = tokio::select! {
                biased;
                () = &mut stop => return Ok(None),
                item = feed.next() => item,
                () = future::ready(()), if !caught_up => {
                    report(&SyncEvent::CaughtUp)?;
                    caught_up = true;
                    continue;
                }
            };
            // The feed ends only once the Store is dropped, which it isn't until `sync` ends.
            let Some(item) = item else { return Ok(None) };
            match item {
                FeedItem::Changes(changes) => {
                    let paths: BTreeSet<Path> = changes
                        .into_iter()
                        .filter(|change| change.area == self.area)
                        .map(|change| change.path)
                        .collect();
                    if !paths.is_empty() {
                        return Ok(Some(Paths::Only(paths)));
                    }
                }
                FeedItem::Resync(area) if area == self.area => {
                    report(&SyncEvent::Resync)?;
                    return Ok(Some(Paths::All));
                }
                FeedItem::Resync(_) => {}
            }
        }
    }

    /// Locks `.tidings/sync.lock`, for as long as a `sync` runs.
    fn lock_for_sync(&self) -> Result<SyncLock, Failure> {
        let file = self.open_lock_file(SYNC_LOCK_FILE)?;
        match file.try_lock() {
            Ok(()) => Ok(SyncLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Failure::error(format!(
                "a `tidings sync` of {} is running already",
                self.folder.display()
            ))),
            Err(TryLockError::Error(error)) => Err(self.failed("lock", error)),
        }
    }

    /// Reconciles `paths`, every Diverged Path and every Path in `theirs_failures` with the Store,
    /// reading each one's Store state afresh, as [`WorkingCopy::reconcile_path`] says. Removals
    /// are applied before writes, so that a File can take the place of a directory, and the
    /// reverse.
    ///
    /// Each Path's `theirs` file is then brought in line, as [`WorkingCopy::settle_theirs`] says:
    /// written if it is Diverged and the Store has a File there, and removed otherwise. If that
    /// fails, the Path is added to `theirs_failures`, and the failure reported, in place of a
    /// *diverged* event that would name a `theirs` file that wasn't written; but a Path already
    /// there is tried again silently, unless its Divergence changed.
    async fn reconcile(
        &self,
        store: &Store,
        paths: Paths,
        theirs_failures: &mut BTreeSet<Path>,
    ) -> Result<Vec<SyncEvent>, Failure> {
        let mut lock = self.lock()?;
        let record = &mut lock.record;
        let mut paths = match paths {
            Paths::All => {
                let mut paths: BTreeSet<Path> =
                    store.list(self.area, "").await?.into_iter().collect();
                paths.extend(record.bases.keys().cloned());
                paths
            }
            Paths::Only(paths) => paths,
        };
        // Every time, so that a Divergence clears once the local file equals the Store's, even if
        // the Store hasn't changed the Path since, and a `theirs` file is written or removed once
        // it can be.
        paths.extend(record.divergences.keys().cloned());
        paths.extend(theirs_failures.iter().cloned());
        let mut removed = Vec::new();
        let mut written = Vec::new();
        for path in paths {
            let store_file = store.read(self.area, &path).await?;
            let base = record.bases.get(&path).map(|base| base.revision);
            if store_file.as_ref().map(File::revision) != base
                || record.divergences.contains_key(&path)
                || theirs_failures.contains(&path)
            {
                match store_file {
                    None => removed.push((path, None)),
                    Some(file) => written.push((file.path().clone(), Some(file))),
                }
            }
        }
        let mut events = Vec::new();
        for (path, store_file) in removed.into_iter().chain(written) {
            let divergence = record.divergences.get(&path).copied();
            let event = self.reconcile_path(record, &path, store_file.as_ref())?;
            let changed = record.divergences.get(&path) != divergence.as_ref();
            let retrying = theirs_failures.remove(&path);
            // The Store's version for `theirs` to hold, if the Path is Diverged.
            let theirs = store_file.as_ref().filter(|_| record.divergences.contains_key(&path));
            let settled = if theirs.is_some() || changed || retrying {
                self.settle_theirs(&path, theirs.map(File::contents), changed || retrying)
            } else {
                Ok(())
            };
            let failure = match settled {
                Ok(()) => {
                    events.extend(event);
                    continue;
                }
                Err(failure) => failure,
            };
            theirs_failures.insert(path.clone());
            let report = changed || !retrying;
            let message = match (theirs, event) {
                // Only the error, since a *diverged* event would name a `theirs` file that wasn't
                // written; it is the only event a Diverged Path can have.
                (Some(_), event) => {
                    let blocked = match event {
                        Some(SyncEvent::Diverged { blocked: Some(blocked), .. }) => {
                            format!("{}; ", blocked.reason())
                        }
                        _ => String::new(),
                    };
                    format!("{blocked}Diverged, but {failure}")
                }
                (None, event) => {
                    events.extend(event);
                    failure.to_string()
                }
            };
            if report {
                events.push(SyncEvent::Error { path, message });
            }
        }
        // The folder is changed first and the record saved after.
        self.save(&lock.record)?;
        Ok(events)
    }

    /// Reconciles `path` with `store_file`, what the Store holds there, in `record` and the
    /// folder, all but its `theirs` file, giving the event reporting what it did, if anything:
    /// - if `store_file` matches the Base (both absent, or the same Revision), the Path is no
    ///   longer Diverged, if it was, and local edits stay local;
    /// - if the local contents equal `store_file` (both absent, say), it becomes the Base;
    /// - if the local file is unchanged since its Base, or absent with no Base, `store_file` is
    ///   applied to the folder and becomes the Base;
    /// - otherwise, or if what is in the folder keeps `store_file` from being applied, the Path is
    ///   Diverged, and the local file and the Base stay as they are.
    fn reconcile_path(
        &self,
        record: &mut Record,
        path: &Path,
        store_file: Option<&File>,
    ) -> Result<Option<SyncEvent>, Failure> {
        let base = record.bases.get(path).copied();
        if store_file.map(File::revision) == base.map(|base| base.revision) {
            return Ok(resolve(record, path));
        }
        let blocked = self.blocking_directory(path)?;
        // Under a symlink, what is there is outside the folder, so it is never read, nor
        // `store_file` applied.
        if let Some(Blocked::Symlink(_)) = blocked {
            return Ok(diverge(record, path, store_file, blocked));
        }
        let local = match blocked {
            // Under a file, the only other directory that blocks a Path, nothing can be there.
            Some(_) => Local::Absent,
            None => self.local(path)?,
        };
        let unchanged = match (&local, base) {
            (Local::Absent | Local::Directory, None) => true,
            (Local::File(contents), Some(base)) => Hash::of(contents) == base.hash,
            _ => false,
        };
        let same_as_store = match (&local, store_file) {
            (Local::Absent | Local::Directory, None) => true,
            (Local::File(contents), Some(file)) => contents == file.contents(),
            _ => false,
        };
        let event = if same_as_store {
            resolve(record, path)
        } else if !unchanged {
            // A local change stays as it is.
            return Ok(diverge(record, path, store_file, None));
        } else {
            // Unchanged: the folder holds the Base at `path`, or no File where there is no Base,
            // and `store_file` differs from it.
            let blocked = match (blocked, &local) {
                (None, Local::Directory) => Some(Blocked::Directory(path.clone())),
                (blocked, _) => blocked,
            };
            if blocked.is_some() {
                return Ok(diverge(record, path, store_file, blocked));
            }
            // Between checking the directories and applying through them, another process could
            // swap one for a symlink and so redirect the write or removal outside the folder. Only
            // a deliberate race could do that, so it isn't guarded against.
            let event = self.apply(path, &local, store_file)?;
            record.divergences.remove(path);
            Some(event)
        };
        match store_file {
            Some(file) => {
                let base = Base { revision: file.revision(), hash: Hash::of(file.contents()) };
                record.bases.insert(path.clone(), base);
            }
            None => {
                record.bases.remove(path);
            }
        }
        Ok(event)
    }

    /// The first of the directories `path` is in, in the folder, that isn't a real directory or
    /// missing, if any, as [`blocking_directory`] says.
    fn blocking_directory(&self, path: &Path) -> Result<Option<Blocked>, Failure> {
        blocking_directory(&self.folder, &self.path_in_folder(path))
    }

    /// Makes the folder hold `theirs` at `path`, where it now holds `local`: absent, or a file,
    /// that differs from `theirs`. None of the directories `path` is in may be a
    /// [`WorkingCopy::blocking_directory`].
    fn apply(
        &self,
        path: &Path,
        local: &Local,
        theirs: Option<&File>,
    ) -> Result<SyncEvent, Failure> {
        let at = self.path_in_folder(path);
        let failed = |at: &FsPath, error| failed_at(at)(error);
        match theirs {
            Some(file) => {
                self.write(&at, file.contents(), failed)?;
                Ok(match local {
                    Local::Absent => SyncEvent::Created(path.clone()),
                    _ => SyncEvent::Updated(path.clone()),
                })
            }
            None => {
                remove_file_and_emptied_directories(&self.folder, &at, failed)?;
                Ok(SyncEvent::Removed(path.clone()))
            }
        }
    }

    /// Makes `path`'s `theirs` file hold `theirs`, the Store's version of the Diverged Path, if it
    /// is given, and if `force` or the file is missing; or else removes the file, if there is one,
    /// then each directory in `theirs/` that the removal emptied. Fails saying what it couldn't do
    /// and why.
    fn settle_theirs(&self, path: &Path, theirs: Option<&str>, force: bool) -> Result<(), Failure> {
        let file = theirs_file(path);
        let what = if theirs.is_some() { "write" } else { "remove" };
        let settled = self.settle_theirs_at(&self.folder.join(&file), theirs, force);
        settled.map_err(|failure| failure.in_context(format!("can't {what} {}", file.display())))
    }

    /// Does what [`WorkingCopy::settle_theirs`] says to the `theirs` file `at`, failing saying
    /// why, and naming any other file or directory it failed at relative to the folder.
    ///
    /// Nothing outside `.tidings/theirs/` is ever written or removed: anything on the way there in
    /// `.tidings/` that isn't a real directory keeps the file from being written, and means there
    /// is none to remove.
    fn settle_theirs_at(
        &self,
        at: &FsPath,
        theirs: Option<&str>,
        force: bool,
    ) -> Result<(), Failure> {
        let failed = |failed_at: &FsPath, error| {
            let failure = Failure::from(error);
            if failed_at == at {
                return failure;
            }
            failure.in_context(failed_at.strip_prefix(&self.folder).unwrap_or(failed_at).display())
        };
        let blocked = blocking_directory(&self.folder.join(RECORD_DIRECTORY), at)?;
        match (theirs, blocked) {
            (Some(_), Some(blocked)) => {
                Err(Failure::error(format!("{} in {RECORD_DIRECTORY}", blocked.reason())))
            }
            (Some(_), None) if !force && fs::symlink_metadata(at).is_ok_and(|m| m.is_file()) => {
                Ok(())
            }
            (Some(contents), None) => self.write(at, contents, failed),
            (None, Some(_)) => Ok(()),
            (None, None) => match fs::symlink_metadata(at) {
                Ok(_) => {
                    let top = self.folder.join(RECORD_DIRECTORY).join(THEIRS_DIRECTORY);
                    remove_file_and_emptied_directories(&top, at, failed)
                }
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(Failure::from(error)),
            },
        }
    }

    /// Writes `contents` to the file `at`, making the directories it needs, so that it is never
    /// seen half-written: in `.tidings/tmp/` first, forced to disk, then renamed into place. A
    /// file it replaces keeps its permissions. `failed` gives the failure for an error at a file
    /// or directory.
    fn write(
        &self,
        at: &FsPath,
        contents: &str,
        failed: impl Fn(&FsPath, io::Error) -> Failure,
    ) -> Result<(), Failure> {
        let tmp = self.folder.join(RECORD_DIRECTORY).join(TMP_DIRECTORY);
        fs::create_dir_all(&tmp).map_err(|error| failed(&tmp, error))?;
        let mut builder = tempfile::Builder::new();
        // As a new file would be made, before the umask, rather than only for its owner.
        #[cfg(unix)]
        builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o666));
        let mut file = builder.tempfile_in(&tmp).map_err(|error| failed(&tmp, error))?;
        file.write_all(contents.as_bytes()).map_err(|error| failed(at, error))?;
        // Not through a symlink, which might lead outside the folder.
        if let Some(metadata) = fs::symlink_metadata(at).ok().filter(fs::Metadata::is_file) {
            let permissions = file.as_file().set_permissions(metadata.permissions());
            permissions.map_err(|error| failed(at, error))?;
        }
        file.as_file().sync_all().map_err(|error| failed(at, error))?;
        if let Some(parent) = at.parent() {
            fs::create_dir_all(parent).map_err(|error| failed(parent, error))?;
        }
        file.persist(at).map_err(|error| failed(at, error.error))?;
        Ok(())
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
        if metadata.is_dir() {
            return Ok(Local::Directory);
        }
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
        let file = self.lock_waiting(LOCK_FILE)?;
        let record = Record::read(&record_file(&self.folder))?;
        Ok(Lock { _file: file, record })
    }

    /// Opens `.tidings/<name>`, making it if need be, without locking it.
    fn open_lock_file(&self, name: &str) -> Result<fs::File, Failure> {
        let path = self.folder.join(RECORD_DIRECTORY).join(name);
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path);
        file.map_err(failed_at(&path))
    }

    /// Opens `.tidings/<name>`, making it if need be, and locks it, waiting for any other command
    /// holding it.
    fn lock_waiting(&self, name: &str) -> Result<fs::File, Failure> {
        let file = self.open_lock_file(name)?;
        file.lock().map_err(|error| self.failed("lock", error))?;
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

/// Where a Working copy keeps the Store's version of the Diverged `path`, relative to its folder.
fn theirs_file(path: &Path) -> PathBuf {
    [RECORD_DIRECTORY, THEIRS_DIRECTORY, path.as_str()].iter().collect()
}

/// The first of the directories that `at`, under `top`, is in, below `top`, that isn't a real
/// directory or missing, if any: a symlink, through which something outside `top` would be
/// reached, or a file. Each is given relative to `top`.
fn blocking_directory(top: &FsPath, at: &FsPath) -> Result<Option<Blocked>, Failure> {
    let directories: Vec<&FsPath> =
        at.ancestors().skip(1).take_while(|directory| *directory != top).collect();
    // From `top` down, since nothing under a missing directory can be there either.
    for directory in directories.into_iter().rev() {
        let metadata = match fs::symlink_metadata(directory) {
            Ok(metadata) => metadata,
            // Missing, so made as a real directory when a file is written under it.
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => return Err(failed_at(directory)(error)),
        };
        let under_top = || directory.strip_prefix(top).unwrap_or(directory).to_owned();
        if metadata.is_symlink() {
            return Ok(Some(Blocked::Symlink(under_top())));
        }
        if !metadata.is_dir() {
            return Ok(Some(Blocked::NotADirectory(under_top())));
        }
    }
    Ok(None)
}

/// Removes the file `at`, then each directory above it that the removal emptied, stopping at the
/// first that holds anything else, and at `top`, which `at` is under. `failed` gives the failure
/// for an error at a file or directory.
fn remove_file_and_emptied_directories(
    top: &FsPath,
    at: &FsPath,
    failed: impl Fn(&FsPath, io::Error) -> Failure,
) -> Result<(), Failure> {
    fs::remove_file(at).map_err(|error| failed(at, error))?;
    for directory in at.ancestors().skip(1).take_while(|directory| *directory != top) {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => break,
            Err(error) => return Err(failed(directory, error)),
        }
    }
    Ok(())
}

/// Clears `path`'s Divergence in `record`, if it has one, giving the event reporting that. Local
/// edits stay local.
fn resolve(record: &mut Record, path: &Path) -> Option<SyncEvent> {
    record.divergences.remove(path).map(|_| SyncEvent::Resolved(path.clone()))
}

/// Marks `path` Diverged in `record`, with `store_file` as the Store's version, giving the event
/// reporting it, unless it was Diverged with that Revision already. `blocked` says what in the
/// folder kept `store_file` from being applied, if anything. The Base stays as it is.
fn diverge(
    record: &mut Record,
    path: &Path,
    store_file: Option<&File>,
    blocked: Option<Blocked>,
) -> Option<SyncEvent> {
    let divergence = Divergence { theirs_revision: store_file.map(File::revision) };
    if record.divergences.insert(path.clone(), divergence) == Some(divergence) {
        return None;
    }
    let theirs_file = store_file.map(|_| theirs_file(path));
    Some(SyncEvent::Diverged { path: path.clone(), theirs_file, blocked })
}

/// Where the Working copy that is `folder` keeps its record.
fn record_file(folder: &FsPath) -> PathBuf {
    folder.join(RECORD_DIRECTORY).join(RECORD_FILE)
}

/// The failure for `folder` being unable to become a Working copy because it `what`, as in "isn't
/// empty".
fn refuse_to_create(folder: &FsPath, what: &str) -> Failure {
    Failure::error(format!("{} {what}: sync into an empty or missing folder", folder.display()))
}

/// Whether `folder` is missing, empty, or holds only an unfinished Working copy: a `.tidings/` that
/// holds no record, only what making a Working copy leaves before it writes one.
fn is_missing_empty_or_unfinished(folder: &FsPath) -> Result<bool, Failure> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(failed_at(folder)(error)),
    };
    for entry in entries {
        let entry = entry.map_err(failed_at(folder))?;
        let is_directory = entry.file_type().map_err(failed_at(folder))?.is_dir();
        // Matched as `scan` matches it, which never reads a `.tidings/` in any case as Files.
        let is_record_directory = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(RECORD_DIRECTORY));
        if !is_record_directory || !is_directory || !holds_no_record(&entry.path())? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether `directory`, a `.tidings/`, holds only what making a Working copy leaves before it
/// writes the record: the lock files, and a temporary file the record was being written to.
///
/// These are the only leftovers allowed, so once [`WorkingCopy::open_or_create`] makes more in
/// `.tidings/` before writing the record (the spec's `ignore` file and `tmp/`), they must be added
/// here.
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
