//! The filesystem Backend: each Area is a directory, and each File a file in it, so that people
//! can see and edit them with ordinary tools.
//!
//! **Where Areas are.** In the platform's standard config, data and cache directories for the App
//! identity, or under the Root override. Each Area's root has a `.tidings/` directory of tidings'
//! own, which holds the lock and the journal.
//!
//! **Reading.** Reads, stat and listing go straight to the directory tree, so they see Files other
//! programs made as well. A File's Revision is a hash of its contents, so stat reads the whole
//! File, and so does working out a Prefix Revision, for every File under the Prefix. A File that
//! isn't valid UTF-8 is listed and has a Revision, but reading it gives [`Error::NotText`]. Reads
//! follow symlinks, to Files and to directories.
//!
//! **Names on disk that aren't Paths.** Another program can make names no Path has: names Windows
//! reserves, names not in NFC form or not valid UTF-8, `.tidings/`, and tidings' temporary files.
//! Those Files, and everything under such directories, are left out: they aren't listed, aren't
//! in Prefix Revisions, and aren't deleted by a Prefix delete. Two names that differ only in
//! letter case can both be on disk on a case-sensitive filesystem, if another program made them:
//! both are listed, as the Files they are, but a Commit can't add another name that clashes with
//! them.
//!
//! **Commits.** A Commit follows ADR 0005, holding an exclusive lock on `.tidings/lock`, which
//! keeps out other tidings Commits to the Area, from this process or any other:
//! 1. finish or discard any Commit a crash left in the journal;
//! 2. work out what the Commit changes with the rules every Backend shares
//!    ([`CommitRequest::plan`]), reading the Area from disk, and refuse a write that something on
//!    disk that isn't a File would stop from finishing, such as a directory holding names that
//!    aren't Paths where the File would go;
//! 3. write the journal as `prepared`, listing each temporary file and what it replaces, unless
//!    the Commit only deletes, and so has none;
//! 4. write each temporary file, with the Commit's timestamp as its modification time, and force it
//!    to disk;
//! 5. write the journal as `committed`: from here on, the Commit has happened;
//! 6. make the deletes, remove the directories they empty, make the directories the writes need,
//!    and rename each temporary file over its target;
//! 7. remove the journal.
//!
//! [`journal`] does steps 3 and 5 to 7, and finishes or discards a journal left behind. If any
//! step before 5 fails, the Commit is discarded, and nothing was written. If a step after it fails,
//! the Commit is finished by the next Commit, or when a Store is next opened.
//!
//! **Temporary files.** Each goes next to the File it replaces, named
//! `.<name>.tidings-<commit-id>-<n>` for the Commit's `n`th write, so that the rename can't cross a
//! volume. No Path can have such a name. A write to a Path that is a symlink goes to the File the
//! link points to, and the link stays. Where a File's directory doesn't exist yet, its temporary
//! file goes in the nearest directory above it that does, and the directory is made in step 6. That
//! is also how a File can move under its own name in one Commit (`a` to `a/b`): the directory `a/`
//! can only be made once the file `a` is gone.
//!
//! **What other programs see.** A program outside tidings can see a Commit half applied, during
//! step 6. And one that writes a File after step 2 and before step 6 has its edit overwritten if
//! the Commit writes that File: the filesystem can't replace a File only if it is unchanged.
//!
//! Every call to the filesystem blocks, so each runs on tokio's blocking threads.

mod journal;

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use jiff::Timestamp;
use xxhash_rust::xxh3::xxh3_128;

use self::journal::{Journal, Replace};
use super::{AreaState, CommitOutcome, CommitRequest, Planned, off_runtime};
use crate::app::AppIdentity;
use crate::area::PerArea;
use crate::path::{letter_case_fold, temporary_file_name};
use crate::staging::has_name;
use crate::{Area, Error, File, Path, Prefix, PrefixRevision, Result, Revision, Stat};

/// How to open a Store on the filesystem, with [`Store::open_fs`](crate::Store::open_fs).
///
/// `FsOptions::default()` puts each Area in the platform's standard directory for the app.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct FsOptions {
    root_override: Option<PathBuf>,
    #[cfg(feature = "testing")]
    fail_at: Option<FailurePoint>,
}

impl FsOptions {
    /// A Root override: the Areas go in `config`, `data` and `cache` directories under `root`,
    /// instead of the standard directories the App identity picks. It is an override, for tests
    /// and unusual installations: apps normally leave it unset.
    pub fn root_override(mut self, root: impl Into<PathBuf>) -> FsOptions {
        self.root_override = Some(root.into());
        self
    }

    /// Makes every Commit through the Store stop at `point`, as if the process had died there: the
    /// Commit gives [`Error::Backend`], and leaves everything on disk as it
    /// is. For tidings' own tests of how an interrupted Commit is recovered.
    #[cfg(feature = "testing")]
    pub fn fail_at(mut self, point: FailurePoint) -> FsOptions {
        self.fail_at = Some(point);
        self
    }
}

/// A named point in a filesystem Commit, where [`FsOptions::fail_at`] stops it. For tidings' own
/// tests: it is public only with the `testing` feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailurePoint {
    /// Once the journal is written as `prepared`, before any temporary file is.
    AfterPreparedJournal,
    /// Once the temporary file for the Commit's `n`th write is written and forced to disk,
    /// counting from 0 in order of Path.
    AfterTemporaryFile(usize),
    /// Once the journal is written as `committed`, before anything is renamed or deleted.
    AfterCommittedJournal,
}

#[derive(Debug)]
pub(crate) struct FsBackend {
    roots: PerArea<PathBuf>,
    #[cfg(feature = "testing")]
    fail_at: Option<FailurePoint>,
}

impl FsBackend {
    /// Makes each Area's root and its `.tidings/` directory if they don't exist, and finishes or
    /// discards any Commit a crash left in its journal.
    pub(crate) async fn open(app: &AppIdentity, options: FsOptions) -> Result<FsBackend> {
        let roots = app.area_directories(options.root_override.as_deref())?;
        let roots = off_runtime(move || {
            for (_, root) in roots.iter() {
                AreaRoot::new(root.clone()).lock()?;
            }
            Ok(roots)
        })
        .await?;
        Ok(FsBackend {
            roots,
            #[cfg(feature = "testing")]
            fail_at: options.fail_at,
        })
    }

    pub(crate) async fn read(&self, area: Area, path: &Path) -> Result<Option<File>> {
        let path = path.clone();
        self.off_runtime(area, move |root| {
            let Some((contents, modified)) = root.read(&path)? else { return Ok(None) };
            let revision = Revision::of_bytes(&contents);
            let contents =
                String::from_utf8(contents).map_err(|_| Error::NotText { path: path.clone() })?;
            Ok(Some(File::new(path, contents, Stat::new(modified, revision))))
        })
        .await
    }

    pub(crate) async fn stat(&self, area: Area, path: &Path) -> Result<Option<Stat>> {
        let path = path.clone();
        self.off_runtime(area, move |root| {
            let read = root.read(&path)?;
            Ok(read.map(|(contents, modified)| Stat::new(modified, Revision::of_bytes(&contents))))
        })
        .await
    }

    pub(crate) async fn list(&self, area: Area, prefix: &Prefix) -> Result<Vec<Path>> {
        let prefix = prefix.clone();
        self.off_runtime(area, move |root| root.paths_under(&prefix)).await
    }

    pub(crate) async fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        let prefix = prefix.clone();
        self.off_runtime(area, move |root| {
            let files = root.revisions_under(&prefix)?;
            Ok(PrefixRevision::of(area, prefix, files))
        })
        .await
    }

    /// Commits `request` to its Area's directory, as the module's doc describes.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        #[cfg(feature = "testing")]
        let fail_at = self.fail_at;
        #[cfg(not(feature = "testing"))]
        let fail_at = None;
        self.off_runtime(request.staged.area, move |root| root.commit(request, fail_at)).await
    }

    /// Runs `call` with `area`'s root, on a blocking thread.
    async fn off_runtime<T: Send + 'static>(
        &self,
        area: Area,
        call: impl FnOnce(AreaRoot) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let root = AreaRoot::new(self.roots.get(area).clone());
        off_runtime(move || call(root)).await
    }
}

/// Gives the error that stops a Commit at `point`, if [`FsOptions::fail_at`] chose it. Without the
/// `testing` feature, nothing can choose one.
fn stop_at(point: FailurePoint, fail_at: Option<FailurePoint>) -> Result<()> {
    if fail_at == Some(point) {
        return Err(Error::backend(format!("the Commit stopped at the failure point {point:?}")));
    }
    Ok(())
}

/// One Area's root directory.
#[derive(Debug)]
struct AreaRoot {
    root: PathBuf,
}

/// The lock on an Area, which keeps out every other tidings Commit to it. Dropping it unlocks.
struct Locked {
    _lock: fs::File,
}

impl AreaRoot {
    fn new(root: PathBuf) -> AreaRoot {
        AreaRoot { root }
    }

    /// Where the File at `path`, or the directory of a Prefix, is on disk, or would be. If it is
    /// a symlink, it is the link.
    fn file(&self, path: &str) -> PathBuf {
        let mut file = self.root.clone();
        file.extend(path.split('/'));
        file
    }

    /// tidings' own directory in the Area.
    fn tidings(&self) -> PathBuf {
        self.root.join(".tidings")
    }

    /// Takes the lock on the Area, making its root and `.tidings/` first if they don't exist, as
    /// they don't once someone clears a Cache. Then finishes or discards any Commit a crash left
    /// in the journal, so that the Area is as its last Commit left it.
    fn lock(&self) -> Result<Locked> {
        let tidings = self.tidings();
        fs::create_dir_all(&tidings).map_err(|error| failed(&tidings, error))?;
        let lock_file = tidings.join("lock");
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_file)
            .and_then(|lock| lock.lock().map(|()| lock))
            .map_err(|error| failed(&lock_file, error))?;
        journal::recover(&self.root, &tidings)?;
        Ok(Locked { _lock: lock })
    }

    /// The contents of the File at `path` and when it was last modified, or `None` if there is no
    /// File there. It follows a symlink.
    fn read(&self, path: &Path) -> Result<Option<(Vec<u8>, Timestamp)>> {
        let file = self.file(path.as_str());
        let read = || -> io::Result<Option<(Vec<u8>, SystemTime)>> {
            // Only a file is opened: opening a FIFO, say, could wait for a writer forever.
            match fs::metadata(&file) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => return Ok(None),
                Err(error) if is_absent(&error) => return Ok(None),
                Err(error) => return Err(error),
            }
            let mut opened = match fs::File::open(&file) {
                Ok(opened) => opened,
                Err(error) if is_absent(&error) => return Ok(None),
                Err(error) => return Err(error),
            };
            let metadata = opened.metadata()?;
            if !metadata.is_file() {
                return Ok(None);
            }
            let mut contents = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
            opened.read_to_end(&mut contents)?;
            Ok(Some((contents, metadata.modified()?)))
        };
        let Some((contents, modified)) = read().map_err(|error| failed(&file, error))? else {
            return Ok(None);
        };
        Ok(Some((contents, Timestamp::try_from(modified).map_err(Error::backend)?)))
    }

    /// Commits `request`, as the module's doc describes, unless it stops at `fail_at`.
    fn commit(
        &self,
        request: CommitRequest,
        fail_at: Option<FailurePoint>,
    ) -> Result<CommitOutcome> {
        let _locked = self.lock()?;
        let timestamp = request.timestamp;
        let plan = request.plan(self)?;
        // Nothing is changed here yet: the journal is made from what the Plan changes, and then
        // applied.
        let (mut writes, mut removes) = (Vec::new(), Vec::new());
        let outcome = plan.apply(|path, planned| {
            match planned {
                Planned::Write { contents, .. } => writes.push((path.clone(), contents)),
                Planned::Remove => removes.push(path.clone()),
            }
            Ok(())
        })?;
        if outcome.changes.is_empty() {
            return Ok(outcome);
        }

        let commit_id = new_commit_id(timestamp);
        let removed: HashSet<PathBuf> =
            removes.iter().map(|path: &Path| self.file(path.as_str())).collect();
        let replaces = writes.iter().enumerate().map(|(n, (path, _))| {
            let target = through_links(self.file(path.as_str()))?;
            refuse_what_is_in_the_way(path, &target, &removed)?;
            let name = path.as_str().rsplit('/').next().unwrap_or(path.as_str());
            let temporary =
                nearest_directory(&target).join(temporary_file_name(name, commit_id, n));
            Ok(Replace { temporary, target })
        });
        let mut journal = Journal::prepared(replaces.collect::<Result<_>>()?, removes);
        let tidings = self.tidings();
        // A Commit that only deletes has no temporary files to keep track of, so its journal is
        // written only once, as `committed`.
        if !journal.replaces.is_empty() {
            journal.write(&tidings)?;
            stop_at(FailurePoint::AfterPreparedJournal, fail_at)?;
        }
        for (n, (replace, (_, contents))) in journal.replaces.iter().zip(&writes).enumerate() {
            let written = write_temporary_file(replace, contents, timestamp)
                .map_err(|error| failed(&replace.temporary, error));
            if let Err(error) = written {
                // If it can't be discarded now, the next Commit, or the next `open`, discards it.
                if let Err(discarding) = journal.discard(&tidings) {
                    tracing::debug!("discarding a failed Commit failed: {discarding}");
                }
                return Err(error);
            }
            stop_at(FailurePoint::AfterTemporaryFile(n), fail_at)?;
        }

        // If this fails, the journal is either still as it was or already `committed`. The
        // temporary files are all there, so the next Commit, or the next `open`, can do whichever
        // it is: discard it or finish it.
        journal.commit(&tidings)?;
        stop_at(FailurePoint::AfterCommittedJournal, fail_at)?;
        journal.finish(&self.root, &tidings)?;
        Ok(outcome)
    }

    /// The Path of every File under `prefix`, in order, with the directory tree walked from the
    /// Prefix down. Names that aren't Paths are left out, and so is everything under them.
    fn paths_under(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        let mut found = Vec::new();
        let start = self.file(prefix.as_str());
        let mut ancestors = Vec::new();
        if let Ok(canonical) = fs::canonicalize(&start) {
            ancestors.push(canonical);
        }
        self.walk(&start, prefix.as_str(), &mut ancestors, &mut found)?;
        found.sort();
        Ok(found)
    }

    /// Adds every File in the directory `directory`, whose Prefix is `prefix`, and in the
    /// directories under it, to `found`. `ancestors` are the directories it is in, as
    /// [`fs::canonicalize`] gives them, so that a symlink to one of them isn't followed round and
    /// round.
    fn walk(
        &self,
        directory: &std::path::Path,
        prefix: &str,
        ancestors: &mut Vec<PathBuf>,
        found: &mut Vec<Path>,
    ) -> Result<()> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if is_absent(&error) => return Ok(()),
            Err(error) => return Err(failed(directory, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| failed(directory, error))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            let Some(kind) = Kind::of(&entry)? else { continue };
            match kind {
                Kind::File => {
                    if let Ok(path) = Path::new(format!("{prefix}{name}")) {
                        found.push(path);
                    }
                }
                Kind::Directory { symlink } => {
                    let Ok(under) = Prefix::new(format!("{prefix}{name}/")) else { continue };
                    let entry = entry.path();
                    let canonical = if symlink {
                        let Ok(canonical) = fs::canonicalize(&entry) else { continue };
                        if ancestors.contains(&canonical) {
                            continue;
                        }
                        canonical
                    } else {
                        ancestors.last().map_or_else(|| entry.clone(), |last| last.join(&name))
                    };
                    ancestors.push(canonical);
                    let walked = self.walk(&entry, under.as_str(), ancestors, found);
                    ancestors.pop();
                    walked?;
                }
            }
        }
        Ok(())
    }
}

/// What a directory entry is, following a symlink.
enum Kind {
    File,
    Directory { symlink: bool },
}

impl Kind {
    /// What `entry` is, or `None` if it is neither a File nor a directory, as a symlink to nothing
    /// isn't.
    fn of(entry: &fs::DirEntry) -> Result<Option<Kind>> {
        let file_type = entry.file_type().map_err(|error| failed(&entry.path(), error))?;
        let symlink = file_type.is_symlink();
        let file_type = if symlink {
            match fs::metadata(entry.path()) {
                Ok(metadata) => metadata.file_type(),
                Err(error) if is_absent(&error) => return Ok(None),
                Err(error) => return Err(failed(&entry.path(), error)),
            }
        } else {
            file_type
        };
        Ok(if file_type.is_dir() {
            Some(Kind::Directory { symlink })
        } else if file_type.is_file() {
            Some(Kind::File)
        } else {
            None
        })
    }
}

impl AreaState for AreaRoot {
    fn revision(&self, path: &Path) -> Result<Option<Revision>> {
        Ok(self.read(path)?.map(|(contents, _)| Revision::of_bytes(&contents)))
    }

    fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>> {
        let mut files = Vec::new();
        for path in self.paths_under(prefix)? {
            // A File removed since it was listed is no longer under the Prefix.
            if let Some(revision) = self.revision(&path)? {
                files.push((path, revision));
            }
        }
        Ok(files)
    }

    fn paths_under(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        AreaRoot::paths_under(self, prefix)
    }

    /// Lists only the directories along the way to `name`: for each of its segments, the entries
    /// that fold like it, in each directory the segment before led to.
    fn paths_named_like(&self, name: &str, fold: &str) -> Result<Vec<Path>> {
        let segments: Vec<&str> = fold.split('/').collect();
        let mut found = Vec::new();
        // The Prefixes of the directories the segments so far lead to.
        let mut directories = vec![String::new()];
        for (level, segment) in segments.iter().enumerate() {
            let last = level + 1 == segments.len();
            let mut next = Vec::new();
            for prefix in &directories {
                let directory = self.file(prefix);
                let entries = match fs::read_dir(&directory) {
                    Ok(entries) => entries,
                    Err(error) if is_absent(&error) => continue,
                    Err(error) => return Err(failed(&directory, error)),
                };
                for entry in entries {
                    let entry = entry.map_err(|error| failed(&directory, error))?;
                    let Some(entry_name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    if letter_case_fold(&entry_name) != *segment {
                        continue;
                    }
                    let Some(kind) = Kind::of(&entry)? else { continue };
                    let named = format!("{prefix}{entry_name}");
                    match kind {
                        Kind::Directory { .. } if !last => next.push(format!("{named}/")),
                        // Every File under `name` has the name `name`, so none is looked at.
                        Kind::Directory { .. } if format!("{named}/") == name => {}
                        Kind::Directory { .. } => {
                            if let Ok(under) = Prefix::new(format!("{named}/")) {
                                found.extend(AreaRoot::paths_under(self, &under)?);
                            }
                        }
                        Kind::File if last => found.extend(Path::new(named)),
                        Kind::File => {}
                    }
                }
            }
            directories = next;
        }
        found.retain(|path| !has_name(path, name));
        Ok(found)
    }
}

/// Refuses a write of `path`, to `target` on disk, that something on disk that isn't a Path
/// would stop the Commit from finishing, so that it is refused before the Commit happens rather
/// than stuck once it has. Only the Files the Commit deletes, which are `removed`, may be in the
/// way: under `target` if it is a directory, and where a directory must be made for it. Otherwise
/// it gives [`Error::InvalidPath`] with [`FileUnderFile`](crate::InvalidPathReason::FileUnderFile).
///
/// Things in the way that are Paths are refused before this, by the checks every Backend shares.
/// This catches the rest: a directory with nothing in it, or only names that aren't Paths, and a
/// symlink to nothing or another kind of file where a directory must go.
fn refuse_what_is_in_the_way(
    path: &Path,
    target: &std::path::Path,
    removed: &HashSet<PathBuf>,
) -> Result<()> {
    let in_the_way = || Error::InvalidPath {
        path: path.as_str().to_owned(),
        reason: crate::InvalidPathReason::FileUnderFile,
    };
    if fs::symlink_metadata(target).is_ok_and(|metadata| metadata.is_dir())
        && !holds_only(target, removed).map_err(|error| failed(target, error))?
    {
        return Err(in_the_way());
    }
    let mut directory = target.parent();
    while let Some(missing) = directory.filter(|directory| !directory.is_dir()) {
        if fs::symlink_metadata(missing).is_ok() && !removed.contains(missing) {
            return Err(in_the_way());
        }
        directory = missing.parent();
    }
    Ok(())
}

/// Whether every file under `directory`, at any depth, is one of `removed`, without following
/// symlinks.
fn holds_only(directory: &std::path::Path, removed: &HashSet<PathBuf>) -> io::Result<bool> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let held = if entry.file_type()?.is_dir() {
            holds_only(&entry.path(), removed)?
        } else {
            removed.contains(&entry.path())
        };
        if !held {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Writes `contents` to `replace`'s temporary file, which must not exist yet, with the permissions
/// of the File it replaces, if there is one, and `modified` as its modification time, and forces
/// it to disk.
fn write_temporary_file(replace: &Replace, contents: &str, modified: Timestamp) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&replace.temporary)?;
    file.write_all(contents.as_bytes())?;
    if let Ok(replaced) = fs::metadata(&replace.target) {
        file.set_permissions(replaced.permissions())?;
    }
    file.set_modified(SystemTime::from(modified))?;
    file.sync_all()
}

/// A number for a new Commit, for its temporary files' names, which no other Commit has had.
fn new_commit_id(timestamp: Timestamp) -> u128 {
    static COMMITS: AtomicU64 = AtomicU64::new(0);
    let this_process = COMMITS.fetch_add(1, Ordering::Relaxed);
    let unique = format!("{} {} {this_process}", timestamp.as_nanosecond(), std::process::id());
    xxh3_128(unique.as_bytes())
}

/// Where a write to `file` goes: `file` itself, or if it is a symlink, the File it points to,
/// through as many links as there are.
fn through_links(mut file: PathBuf) -> Result<PathBuf> {
    // As many links as Linux follows before it gives up.
    for _ in 0..40 {
        match fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link = fs::read_link(&file).map_err(|error| failed(&file, error))?;
                // A relative link is relative to the directory it is in.
                file = file.parent().map_or_else(|| link.clone(), |parent| parent.join(&link));
            }
            _ => return Ok(file),
        }
    }
    Err(failed(&file, io::Error::other("too many levels of symlinks")))
}

/// The directory `file` is in if it exists, or else the nearest directory above it that does.
fn nearest_directory(file: &std::path::Path) -> PathBuf {
    let mut directory = file.parent();
    while let Some(candidate) = directory {
        if candidate.is_dir() {
            return candidate.to_path_buf();
        }
        directory = candidate.parent();
    }
    PathBuf::new()
}

/// Whether `error` means that there is nothing there: no such file, or a file where a directory
/// on the way would have to be.
fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

/// The Backend failed with `error`, doing something to `path`.
fn failed(path: &std::path::Path, error: io::Error) -> Error {
    Error::backend(io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}
