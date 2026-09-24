//! The filesystem Backend: each Area is a directory, and each File a file in it, so that people
//! can see and edit them with ordinary tools.
//!
//! **Where Areas are.** In the platform's standard config, data and cache directories for the App
//! identity, or under the Root override. Each Area's root has a `.tidings/` directory of tidings'
//! own, which holds the lock and the journal.
//!
//! **Reading.** Reads, stat and listing go straight to the directory tree, so they see Files other
//! programs made as well. Where the journal holds a Commit that has happened but isn't finished,
//! they see the Area as finishing it will leave it (see **Commits**). A File's Revision is a hash
//! of its contents, so stat reads the whole File, and so does working out a Prefix Revision, for
//! every File under the Prefix. A File that isn't valid UTF-8 is listed and has a Revision, but
//! reading it gives [`Error::NotText`]. Reads follow symlinks, to Files and to directories.
//!
//! **Names on disk that aren't Paths.** Another program can make names no Path has: names Windows
//! reserves, names not in NFC form or not valid UTF-8, `.tidings/`, and tidings' temporary files.
//! Those Files, and everything under such directories, are left out: they aren't listed, aren't
//! in Prefix Revisions, and aren't deleted by a Prefix delete. Two names that differ only in
//! letter case can both be on disk on a case-sensitive filesystem, if another program made them:
//! both are listed, as the Files they are, but a Commit can't add another name that clashes with
//! them.
//!
//! **Exact names.** A Path names only the file on disk with exactly its name. Some filesystems
//! (macOS's and Windows' by default) also find `Foo` when asked for `foo`. Opening a Store finds
//! out whether the Area's does, from whether `.tidings/LOCK` finds `.tidings/lock`. Where it does,
//! each read checks that every name on the way is there exactly, by looking in its directory, so
//! that reading `foo` gives nothing when only `Foo` is there, as listing shows. That costs a look
//! through each directory on the way, on those filesystems only. It also makes a Commit that
//! renames `Foo` to `foo` write `foo`, rather than find it already there.
//!
//! **Commits.** A Commit follows ADR 0005, holding an exclusive lock on `.tidings/lock`, which
//! keeps out other tidings Commits to the Area, from this process or any other:
//! 1. finish or discard any Commit a crash left in the journal;
//! 2. work out what the Commit changes with the rules every Backend shares
//!    ([`CommitRequest::plan`]), reading the Area from disk. Then refuse what the filesystem
//!    couldn't finish, or would get wrong: two Paths that are the same file on disk (through a
//!    symlink), something that isn't a File where a File or its directory must go, and a
//!    symlink into a directory that doesn't exist;
//! 3. write the journal as `prepared`, listing each temporary file and what it replaces, unless
//!    the Commit only deletes, and so has none;
//! 4. write each temporary file, with the Commit's timestamp as its modification time, and force it
//!    to disk;
//! 5. write the journal as `committed`: from here on, the Commit has happened;
//! 6. make the deletes, remove the directories they empty, make the directories the writes need,
//!    rename each temporary file over its target, and force each directory changed to disk;
//! 7. remove the journal.
//!
//! [`journal`] does steps 3 and 5 to 7, and finishes or discards a journal left behind. If any
//! step before 5 fails, the Commit is discarded, and nothing was written. If step 6 fails, as a
//! rename does on Windows while another program has the File open, it is tried again after each
//! of the [`RETRY_DELAYS`], and if it still fails, the Commit gives [`Error::Pending`]: it has
//! happened, and its Changes are recorded, but the journal stays. Until it is finished, reads see
//! each Path it writes as its temporary file, and each Path it deletes as absent. Step 1 of the
//! next Commit finishes it, or so does opening a Store. If that fails too, the next Commit isn't
//! made, and gives [`Error::Backend`]: giving it `Pending` would say that it had happened.
//! Opening a Store still works, since reads show the Commit. Another Path that is the same file
//! as one the Commit writes or deletes, through a symlink, is read as it is on disk until then.
//!
//! **Temporary files.** Each goes next to the File it replaces, named
//! `.<name>.tidings-<commit-id>-<n>` for the Commit's `n`th write, so that the rename can't cross a
//! volume. No Path can have such a name. Where a File's directory doesn't exist yet, its temporary
//! file goes in the nearest directory above it that does, within the Area, and the directory is
//! made in step 6. That is also how a File can move under its own name in one Commit (`a` to
//! `a/b`): the directory `a/` can only be made once the file `a` is gone.
//!
//! **Symlinks.** A write to a Path that is a symlink goes to the File the link points to, wherever
//! that is, and the link stays. The directory it points into must exist: a write through a link to
//! a File never makes a directory, so it can't make one outside the Area. (A write under a
//! symlink to a directory makes any directories it needs in that directory, wherever it is.) A
//! delete removes the link itself.
//!
//! **What other programs see.** A program outside tidings can see a Commit half applied, during
//! step 6. And one that writes a File after step 2 and before step 6 has its edit overwritten if
//! the Commit writes that File: the filesystem can't replace a File only if it is unchanged.
//!
//! **Watching.** Each Area's directory is watched, so that what other programs and other Stores
//! change there reaches the Change feed: see [`watch`].
//!
//! Every call to the filesystem blocks, so each runs on tokio's blocking threads.

mod journal;
mod watch;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "testing")]
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
#[cfg(feature = "testing")]
use std::sync::{Condvar, PoisonError};
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use xxhash_rust::xxh3::xxh3_128;

use self::journal::{AsFinished, Journal, Recovery, Remove, Replace, Target};
pub(crate) use self::watch::FsWatcher;
use self::watch::Reported;
use super::{AreaState, CommitOutcome, CommitRequest, Planned, off_runtime};
use crate::app::AppIdentity;
use crate::area::PerArea;
use crate::path::{letter_case_fold, temporary_file_name};
use crate::staging::has_name;
use crate::{
    Area, Error, File, InvalidPathReason, Path, Prefix, PrefixRevision, Result, Revision, Stat,
};

/// How to open a Store on the filesystem, with [`Store::open_fs`](crate::Store::open_fs).
///
/// `FsOptions::default()` puts each Area in the platform's standard directory for the app.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct FsOptions {
    root_override: Option<PathBuf>,
    debounce_window: Option<Duration>,
    #[cfg(feature = "testing")]
    fail_at: Option<FailurePoint>,
    #[cfg(feature = "testing")]
    pause_at: Option<(FailurePoint, Pause)>,
}

impl FsOptions {
    /// A Root override: the Areas go in `config`, `data` and `cache` directories under `root`,
    /// instead of the standard directories the App identity picks. It is an override, for tests
    /// and unusual installations: apps normally leave it unset.
    pub fn root_override(mut self, root: impl Into<PathBuf>) -> FsOptions {
        self.root_override = Some(root.into());
        self
    }

    /// How long the events of an edit made outside this Store are held back, once the last of
    /// them, before the edit is reported. Editors save a File with a burst of events, so that a
    /// File can be half written in between: the burst has to settle first. 150 ms by default. A
    /// longer window reports edits later, and a shorter one risks reporting a File half saved.
    pub fn debounce_window(mut self, window: Duration) -> FsOptions {
        self.debounce_window = Some(window);
        self
    }

    /// Makes every Commit through the Store stop at `point`, as if the process had died there: the
    /// Commit gives [`Error::Backend`], and leaves everything on disk as it is. Finishing a Commit
    /// left behind, when a Store opens or commits, never stops. For tidings' own tests of how an
    /// interrupted Commit is recovered.
    ///
    /// [`FailurePoint::RenameFails`] and [`FailurePoint::CommittedJournalFails`] are different:
    /// each makes a step fail, as described there. [`FailurePoint::WatchingFails`] and
    /// [`FailurePoint::WatchingAnAreaFails`] make watching fail.
    #[cfg(feature = "testing")]
    pub fn fail_at(mut self, point: FailurePoint) -> FsOptions {
        self.fail_at = Some(point);
        self
    }

    /// Makes every Commit through the Store wait at `point` until `pause` is released, where
    /// [`fail_at`](Self::fail_at) would stop it. For tidings' own tests of cancelling a Commit part
    /// way through. `point` must be a place [`fail_at`](Self::fail_at) stops a Commit at.
    #[cfg(feature = "testing")]
    pub fn pause_at(mut self, point: FailurePoint, pause: &Pause) -> FsOptions {
        self.pause_at = Some((point, pause.clone()));
        self
    }
}

/// Holds filesystem Commits at a point, for tidings' own tests of cancelling one part way
/// through: [`FsOptions::pause_at`] says where. Its clones are the same Pause.
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Default)]
pub struct Pause {
    shared: Arc<PauseShared>,
}

#[cfg(feature = "testing")]
#[derive(Debug, Default)]
struct PauseShared {
    released: Mutex<bool>,
    release: Condvar,
    reached: tokio::sync::Notify,
}

#[cfg(feature = "testing")]
impl Pause {
    /// A Pause that holds Commits until it is released.
    pub fn new() -> Pause {
        Pause::default()
    }

    /// Waits until a Commit is held at the point.
    pub async fn reached(&self) {
        self.shared.reached.notified().await;
    }

    /// Lets each Commit held at the point go on, and every Commit that reaches it later.
    pub fn release(&self) {
        *self.shared.released.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.shared.release.notify_all();
    }

    /// Holds the Commit calling it at `point`, on a blocking thread, until the Pause is released.
    ///
    /// # Panics
    ///
    /// If it isn't released within 30 seconds, so that a test that never releases it fails
    /// rather than hangs.
    fn hold(&self, point: FailurePoint) {
        self.shared.reached.notify_one();
        let released = self.shared.released.lock().unwrap_or_else(PoisonError::into_inner);
        let held =
            self.shared.release.wait_timeout_while(released, Duration::from_secs(30), |r| !*r);
        let (_released, waited) = held.unwrap_or_else(PoisonError::into_inner);
        assert!(!waited.timed_out(), "a Commit held at {point:?} was never released");
    }
}

/// A named point in a filesystem Commit, where [`FsOptions::fail_at`] stops it and
/// [`FsOptions::pause_at`] holds it, or with [`RenameFails`](Self::RenameFails) and
/// [`CommittedJournalFails`](Self::CommittedJournalFails), a step that fails, or with
/// [`WatchingFails`](Self::WatchingFails) and [`WatchingAnAreaFails`](Self::WatchingAnAreaFails),
/// watching. For tidings' own tests.
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailurePoint {
    /// Once the journal is written as `prepared`, before any temporary file is.
    AfterPreparedJournal,
    /// Once the temporary file for the Commit's `n`th write is written and forced to disk,
    /// counting from 0 in order of Path.
    AfterTemporaryFile(usize),
    /// Once the journal is written as `committed`, before anything is deleted or renamed.
    AfterCommittedJournal,
    /// Once the deletes are made, before anything is renamed.
    AfterDeletes,
    /// Once the temporary file for the Commit's `n`th write is renamed over its File.
    AfterRename(usize),
    /// Renaming the temporary file for the Commit's `n`th write over its File fails, as it does on
    /// Windows while another program has the File open. It fails the first `times` times it is
    /// tried through the Store, counting the Commit's own tries and those of the Commits and
    /// opening that finish it later, and then works. With `usize::MAX`, it keeps failing.
    RenameFails {
        /// Which write's rename fails, counting from 0 in order of Path.
        n: usize,
        /// How many times it fails.
        times: usize,
    },
    /// Writing the journal as `committed` gives an error once the journal is written, as it would
    /// if forcing its directory to disk failed.
    CommittedJournalFails,
    /// Watching the Areas' directories fails: the first events that could change a File are
    /// replaced by an error naming them, as the platform's watcher gives when it fails, and the
    /// watches of the Areas they are in are lost, as they can be then. Not a point in a Commit.
    WatchingFails,
    /// Watching an Area's directory, when the Store opens or again later, fails the first `times`
    /// times, as it does once the platform's limit on watches is reached. The Areas are watched
    /// in order of Area. Not a point in a Commit.
    WatchingAnAreaFails {
        /// How many times it fails.
        times: usize,
    },
}

/// What [`FsOptions::fail_at`] and [`FsOptions::pause_at`] set up, for each Area of a Store.
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Default)]
struct FailureSetup {
    fail_at: Option<FailurePoint>,
    pause_at: Option<(FailurePoint, Pause)>,
    /// How many more times the rename [`FailurePoint::RenameFails`] names fails, shared by every
    /// Area of the Store.
    renames_to_fail: Arc<AtomicUsize>,
}

/// The error [`AreaRoot::stop_at`] stops a Commit with, which is never tried again.
#[cfg(feature = "testing")]
#[derive(Debug, thiserror::Error)]
#[error("the Commit stopped at the failure point {0:?}")]
struct Stopped(FailurePoint);

/// Why a Commit wasn't made: the Commit left in the journal before it, which gave
/// [`Error::Pending`] or was interrupted, still can't be finished or discarded, and must be first.
#[derive(Debug, thiserror::Error)]
#[error(
    "this Commit wasn't made, because an earlier Commit to {} that gave `Pending` or was \
     interrupted still can't be finished. Try again later, once nothing holds its Files open, as \
     another program can on Windows",
    area.display()
)]
struct EarlierCommitLeft {
    area: PathBuf,
    #[source]
    error: Error,
}

/// How long a Commit waits before trying again to finish, each time finishing fails, before it
/// gives [`Error::Pending`]. A program that has a File open on Windows often closes it again
/// quickly.
const RETRY_DELAYS: [Duration; 3] =
    [Duration::from_millis(10), Duration::from_millis(50), Duration::from_millis(200)];

#[derive(Debug)]
pub(crate) struct FsBackend {
    areas: PerArea<Arc<AreaRoot>>,
    /// What the Change feed has been told of each Area's Files, which the watcher compares events
    /// with, and each Commit updates.
    reported: PerArea<Arc<Mutex<Reported>>>,
}

impl FsBackend {
    /// Makes each Area's root and its `.tidings/` directory if they don't exist, finishes or
    /// discards any Commit a crash left in its journal, and starts watching the Areas, giving the
    /// watcher for the Store to run.
    pub(crate) async fn open(
        app: &AppIdentity,
        options: FsOptions,
    ) -> Result<(FsBackend, FsWatcher)> {
        let roots = app.area_directories(options.root_override.as_deref())?;
        let window = options.debounce_window.unwrap_or(watch::DEFAULT_WINDOW);
        #[cfg(feature = "testing")]
        let watch_failures = watch::WatchFailures {
            first_events: options.fail_at == Some(FailurePoint::WatchingFails),
            watching_an_area: match options.fail_at {
                Some(FailurePoint::WatchingAnAreaFails { times }) => times,
                _ => 0,
            },
        };
        #[cfg(feature = "testing")]
        let failures = FailureSetup {
            fail_at: options.fail_at,
            pause_at: options.pause_at,
            renames_to_fail: Arc::new(AtomicUsize::new(match options.fail_at {
                Some(FailurePoint::RenameFails { times, .. }) => times,
                _ => 0,
            })),
        };
        let areas = off_runtime(move || {
            PerArea::try_from_fn(|area| {
                #[cfg_attr(not(feature = "testing"), expect(unused_mut))]
                let mut root = AreaRoot::open(roots.get(area).clone())?;
                #[cfg(feature = "testing")]
                {
                    root.failures = failures.clone();
                }
                let _locked = root.lock()?;
                // Reads show a Commit that can't be finished yet, so the Store can open.
                if let Recovery::Left(error) = journal::recover(&root)? {
                    tracing::debug!(
                        "a Commit to {} is left unfinished for now: {error}",
                        root.root.display(),
                    );
                }
                Ok(Arc::new(root))
            })
        })
        .await?;
        let reported = PerArea::try_from_fn(|_| Ok(Arc::default()))?;
        let watching = PerArea::try_from_fn(|area| {
            Ok((Arc::clone(areas.get(area)), Arc::clone(reported.get(area))))
        })?;
        let watcher = off_runtime(move || {
            FsWatcher::start(
                watching,
                window,
                #[cfg(feature = "testing")]
                watch_failures,
            )
        })
        .await?;
        Ok((FsBackend { areas, reported }, watcher))
    }

    pub(crate) async fn read(&self, area: Area, path: &Path) -> Result<Option<File>> {
        let path = path.clone();
        self.off_runtime(area, move |root| {
            let Some((contents, modified)) = root.as_finished()?.read(&path)? else {
                return Ok(None);
            };
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
            let read = root.as_finished()?.read(&path)?;
            Ok(read.map(|(contents, modified)| Stat::new(modified, Revision::of_bytes(&contents))))
        })
        .await
    }

    pub(crate) async fn list(&self, area: Area, prefix: &Prefix) -> Result<Vec<Path>> {
        let prefix = prefix.clone();
        self.off_runtime(area, move |root| root.as_finished()?.paths_under(&prefix)).await
    }

    pub(crate) async fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        let prefix = prefix.clone();
        self.off_runtime(area, move |root| {
            let files = root.as_finished()?.revisions_under(&prefix)?;
            Ok(PrefixRevision::of(area, prefix, files))
        })
        .await
    }

    /// Commits `request` to its Area's directory, as the module's doc describes, and takes what
    /// it changed as reported, since the Store reports it.
    pub(crate) async fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let area = request.staged.area;
        let outcome = self.off_runtime(area, move |root| root.commit(request)).await?;
        watch::lock_ignoring_poison(self.reported.get(area)).committed(&outcome);
        Ok(outcome)
    }

    /// Runs `call` with `area`'s root, on a blocking thread.
    async fn off_runtime<T: Send + 'static>(
        &self,
        area: Area,
        call: impl FnOnce(&AreaRoot) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let root = Arc::clone(self.areas.get(area));
        off_runtime(move || call(&root)).await
    }
}

/// One Area's root directory, and how its filesystem treats names.
#[derive(Debug)]
struct AreaRoot {
    root: PathBuf,
    /// Whether the filesystem finds a file under names that differ from its own, such as `FOO`
    /// for `foo`, so that a Path must be checked against the names really there.
    names_fold: bool,
    #[cfg(feature = "testing")]
    failures: FailureSetup,
}

/// The lock on an Area, which keeps out every other tidings Commit to it. Dropping it unlocks.
struct Locked {
    _lock: fs::File,
}

/// A write a Commit makes: its temporary file and what it replaces, and the contents.
struct Writing {
    replace: Replace,
    contents: String,
}

impl AreaRoot {
    /// The Area whose root is `root`, which it makes, with its `.tidings/` directory and lock
    /// file, if they don't exist. It finds out whether the filesystem there treats names that
    /// differ in letter case as the same: whether `.tidings/LOCK` finds the lock file, though no
    /// entry has that name.
    fn open(root: PathBuf) -> Result<AreaRoot> {
        let mut area = AreaRoot {
            root,
            names_fold: false,
            #[cfg(feature = "testing")]
            failures: FailureSetup::default(),
        };
        drop(area.lock_file()?);
        let tidings = area.tidings();
        let upper_case = tidings.join("LOCK");
        area.names_fold = present_at(fs::symlink_metadata(&upper_case), &upper_case)?.is_some()
            && !has_entry(&tidings, "LOCK".as_ref())?;
        Ok(area)
    }

    /// Where the File at `path`, or the directory of a Prefix, is on disk, or would be. If it is
    /// a symlink, it is the link.
    fn file(&self, path: &str) -> PathBuf {
        on_disk(&self.root, path)
    }

    /// tidings' own directory in the Area.
    fn tidings(&self) -> PathBuf {
        self.root.join(".tidings")
    }

    /// Opens the lock file, making the Area's root and `.tidings/` first if they don't exist, as
    /// they don't once someone clears a Cache.
    fn lock_file(&self) -> Result<fs::File> {
        let tidings = self.tidings();
        fs::create_dir_all(&tidings).map_err(|error| failed(&tidings, error))?;
        let lock_file = tidings.join("lock");
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_file)
            .map_err(|error| failed(&lock_file, error))
    }

    /// Takes the lock on the Area.
    fn lock(&self) -> Result<Locked> {
        let lock = self.lock_file()?;
        lock.lock().map_err(|error| failed(&self.tidings().join("lock"), error))?;
        Ok(Locked { _lock: lock })
    }

    /// Finishes `journal`, a committed Commit's, in the Area, which must be locked. `again` says
    /// whether the Commit may be partly finished already ([`Journal::finish`]). If finishing
    /// fails, it is tried again after each of the [`RETRY_DELAYS`], as finishing again, and the
    /// last error is given if it still fails. A stop at a failure point isn't tried again.
    fn finish(&self, journal: &Journal, again: bool) -> Result<()> {
        let mut finished = journal.finish(self, again);
        for delay in RETRY_DELAYS {
            match &finished {
                Ok(()) => break,
                Err(error) if stopped(error) => break,
                Err(error) => {
                    tracing::debug!(
                        "finishing a Commit to {} failed, trying again in {delay:?}: {error}",
                        self.root.display(),
                    );
                }
            }
            std::thread::sleep(delay);
            finished = journal.finish(self, true);
        }
        finished
    }

    /// Holds a Commit at `point`, if [`FsOptions::pause_at`] chose it, then gives the error that
    /// stops it there, if [`FsOptions::fail_at`] chose it.
    #[cfg(feature = "testing")]
    fn stop_at(&self, point: FailurePoint) -> Result<()> {
        if let Some((at, pause)) = &self.failures.pause_at
            && *at == point
        {
            pause.hold(point);
        }
        if self.failures.fail_at == Some(point) {
            return Err(Error::backend(Stopped(point)));
        }
        Ok(())
    }

    /// Gives an error for `point`, a step that fails rather than a place a Commit stops, if
    /// [`FsOptions::fail_at`] chose it.
    #[cfg(feature = "testing")]
    fn fail_at(&self, point: FailurePoint) -> Result<()> {
        if self.failures.fail_at == Some(point) {
            return Err(Error::backend(format!("failed at the failure point {point:?}")));
        }
        Ok(())
    }

    /// Gives an error for renaming the temporary file of the Commit's `n`th write, if
    /// [`FailurePoint::RenameFails`] says that rename fails this time.
    #[cfg(feature = "testing")]
    fn rename_fails(&self, n: usize) -> io::Result<()> {
        let Some(FailurePoint::RenameFails { n: failing, .. }) = self.failures.fail_at else {
            return Ok(());
        };
        let fails = |left: usize| left.checked_sub(1);
        if failing == n && self.failures.renames_to_fail.fetch_update(SeqCst, SeqCst, fails).is_ok()
        {
            return Err(io::Error::other("the rename failed at FailurePoint::RenameFails"));
        }
        Ok(())
    }

    /// Whether `file`, which is there, is there under exactly its own name, rather than found
    /// under another that the filesystem treats as the same.
    fn named_exactly(&self, file: &FsPath) -> Result<bool> {
        match (self.names_fold, file.parent(), file.file_name()) {
            (true, Some(directory), Some(name)) => has_entry(directory, name),
            _ => Ok(true),
        }
    }

    /// Follows `segments` down from the root while each is a directory there under exactly its
    /// own name. The one walk from the root that reads, writes and finishing share.
    fn own_directories(&self, segments: &[&str]) -> Result<OwnDirectories> {
        let mut directory = self.root.clone();
        for (count, segment) in segments.iter().enumerate() {
            let next = directory.join(segment);
            if next.is_dir() && self.named_exactly(&next)? {
                directory = next;
                continue;
            }
            let there = present_at(fs::symlink_metadata(&next), &next)?.is_some();
            let next_under_other_name = there && !self.named_exactly(&next)?;
            return Ok(OwnDirectories { directory, count, next_under_other_name });
        }
        Ok(OwnDirectories { directory, count: segments.len(), next_under_other_name: false })
    }

    /// Whether nothing at `name`, a Path or a Prefix without its `/`, or on the way to it, is
    /// found only under another name that the filesystem treats as the same.
    fn found_under_its_own_name(&self, name: &str) -> Result<bool> {
        if !self.names_fold {
            return Ok(true);
        }
        let segments: Vec<&str> = name.split('/').collect();
        Ok(!self.own_directories(&segments)?.next_under_other_name)
    }

    /// The contents of the File at `path` and when it was last modified, or `None` if there is no
    /// File there under exactly that name. It follows a symlink.
    fn read(&self, path: &Path) -> Result<Option<(Vec<u8>, Timestamp)>> {
        if !self.found_under_its_own_name(path.as_str())? {
            return Ok(None);
        }
        read_file(&self.file(path.as_str()))
    }

    /// The Area as reads through tidings see it, with the Commit in its journal finished.
    fn as_finished(&self) -> Result<AsFinished<'_>> {
        AsFinished::of(self)
    }

    /// Commits `request`, as the module's doc describes.
    fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let _locked = self.lock()?;
        if let Recovery::Left(error) = journal::recover(self)? {
            return Err(Error::backend(EarlierCommitLeft { area: self.root.clone(), error }));
        }
        let timestamp = request.timestamp;
        let plan = request.plan(self)?;
        // Nothing is changed here yet: the journal is made from what the Plan changes, and then
        // applied.
        let (mut written, mut removes) = (Vec::new(), Vec::new());
        let outcome = plan.apply(|path, planned| {
            match planned {
                Planned::Write { contents, .. } => written.push((path.clone(), contents)),
                Planned::Remove { revision } => {
                    removes.push(Remove { path: path.clone(), revision });
                }
            }
            Ok(())
        })?;
        if outcome.changes.is_empty() {
            return Ok(outcome);
        }

        let mut same_file = SameFile::default();
        for Remove { path, .. } in &removes {
            let file = self.file(path.as_str());
            let directory = file.parent().unwrap_or(&self.root);
            let identity = self.identity(directory, file.file_name().unwrap_or_default())?;
            same_file.deleted(path, identity);
        }
        let removed: HashSet<String> =
            removes.iter().map(|remove| self.deleted_form(remove.path.as_str())).collect();
        let commit_id = new_commit_id(timestamp);
        let mut writes = Vec::with_capacity(written.len());
        for (n, (path, contents)) in written.into_iter().enumerate() {
            let Destination { directory, target, identity } =
                self.where_to_write(&path, &removed)?;
            // Where names fold, a write under a Path deleted in another letter case is a rename.
            let renames_case = self.names_fold && matches!(target, Target::AtPath);
            same_file.written(&path, identity, renames_case)?;
            let name = path.as_str().rsplit('/').next().unwrap_or(path.as_str());
            let temporary = directory.join(temporary_file_name(name, commit_id, n));
            let replace = Replace { path, temporary, target };
            writes.push(Writing { replace, contents });
        }

        let replaces = writes.iter().map(|write| write.replace.clone()).collect();
        let mut journal = Journal::prepared(replaces, removes);
        let tidings = self.tidings();
        // A Commit that only deletes has no temporary files to keep track of, so its journal is
        // written only once, as `committed`.
        if !writes.is_empty() {
            journal.write(&tidings)?;
            #[cfg(feature = "testing")]
            self.stop_at(FailurePoint::AfterPreparedJournal)?;
        }
        #[cfg_attr(
            not(feature = "testing"),
            expect(clippy::unused_enumerate_index, reason = "the number names a failure point")
        )]
        for (_n, Writing { replace, contents }) in writes.iter().enumerate() {
            let written = write_temporary_file(self, replace, contents, timestamp)
                .map_err(|error| failed(&replace.temporary, error));
            if let Err(error) = written {
                // If it can't be discarded now, the next Commit, or the next `open`, discards it.
                if let Err(discarding) = journal.discard(&tidings) {
                    tracing::debug!("discarding a failed Commit failed: {discarding}");
                }
                return Err(error);
            }
            #[cfg(feature = "testing")]
            self.stop_at(FailurePoint::AfterTemporaryFile(_n))?;
        }

        self.commit_journal(&mut journal)?;
        #[cfg(feature = "testing")]
        self.stop_at(FailurePoint::AfterCommittedJournal)?;
        match self.finish(&journal, false) {
            Ok(()) => Ok(outcome),
            Err(error) if stopped(&error) => Err(error),
            // The Commit has happened, and reads show it. The next Commit, or the next `open`,
            // finishes it.
            Err(error) => {
                tracing::debug!(
                    "a Commit to {} is pending, since it can't be finished yet: {error}",
                    self.root.display(),
                );
                Ok(CommitOutcome { pending: true, ..outcome })
            }
        }
    }

    /// Writes `journal` as `committed`. If that fails, the journal on disk is either as it was or
    /// `committed` already: writing it can fail once it is renamed into place, as when forcing its
    /// directory to disk fails. So it is read back. If it is `committed`, the Commit has happened,
    /// so this gives `Ok`, and the Commit goes on to be finished and reported. Otherwise the Commit
    /// is discarded, and the error given. If it can't be discarded now, or the journal can't be
    /// read, the next Commit or `open` does whichever the journal says.
    fn commit_journal(&self, journal: &mut Journal) -> Result<()> {
        let tidings = self.tidings();
        let committing = journal.commit(&tidings);
        #[cfg(feature = "testing")]
        let committing =
            committing.and_then(|()| self.fail_at(FailurePoint::CommittedJournalFails));
        let Err(error) = committing else { return Ok(()) };
        match journal::is_committed(&tidings) {
            Ok(true) => {
                tracing::debug!(
                    "writing a Commit's journal in {} as committed failed, but it is: {error}",
                    self.root.display(),
                );
                return Ok(());
            }
            Ok(false) => {
                if let Err(discarding) = journal.discard(&tidings) {
                    tracing::debug!("discarding a failed Commit failed: {discarding}");
                }
            }
            Err(reading) => tracing::debug!("reading back a Commit's journal failed: {reading}"),
        }
        Err(error)
    }

    /// Where a write of `path` goes. `removed` holds the [`deleted_form`](Self::deleted_form) of
    /// each Path the Commit deletes.
    ///
    /// A symlink is followed to the File it points to, whose directory must exist. Otherwise the
    /// File goes at `path`, and its temporary file in the nearest directory on the way that exists
    /// under its own name. Whatever stands where the File or its first missing directory must go
    /// has to be removed by the Commit's deletes, or the write is refused with
    /// [`FileUnderFile`](InvalidPathReason::FileUnderFile) before the Commit happens, since
    /// otherwise it couldn't be finished. Paths in the way were refused already, by the checks
    /// every Backend shares. This catches the rest: a directory with nothing in it, or only names
    /// that aren't Paths, a symlink to nothing or another kind of file where a directory must
    /// go, and on a filesystem that ignores letter case, a name that differs only in case.
    fn where_to_write(&self, path: &Path, removed: &HashSet<String>) -> Result<Destination> {
        let file = self.file(path.as_str());
        let in_the_way = || Error::InvalidPath {
            path: path.as_str().to_owned(),
            reason: InvalidPathReason::FileUnderFile,
        };
        let there = present_at(fs::symlink_metadata(&file), &file)?;
        let own_name = self.found_under_its_own_name(path.as_str())?;
        if own_name && there.as_ref().is_some_and(fs::Metadata::is_symlink) {
            let target = through_links(file)?;
            let directory = target.parent().map(FsPath::to_path_buf).unwrap_or_default();
            if !directory.is_dir() {
                return Err(Error::backend(format!(
                    "{path} is a symlink to {}, in a directory that doesn't exist, so it can't be \
                     written",
                    target.display(),
                )));
            }
            if target.is_dir() {
                return Err(in_the_way());
            }
            let identity = self.identity(&directory, target.file_name().unwrap_or_default())?;
            return Ok(Destination { directory, target: Target::Linked(target), identity });
        }

        let segments: Vec<&str> = path.as_str().split('/').collect();
        let (directories, name) = segments.split_at(segments.len() - 1);
        let OwnDirectories { directory, count: existing, .. } =
            self.own_directories(directories)?;
        let obstacle = if existing < directories.len() {
            // The first directory missing under its own name.
            Some(directory.join(directories[existing]))
        } else if own_name && there.as_ref().is_none_or(|there| !there.is_dir()) {
            // The File this write replaces, there under its own name, makes way.
            None
        } else {
            Some(directory.join(name[0]))
        };
        if let Some(obstacle) = obstacle
            && !self.removed_by_commit(&obstacle, removed)?
        {
            return Err(in_the_way());
        }
        let identity = self.identity(&directory, segments[existing..].join("/").as_ref())?;
        Ok(Destination { directory, target: Target::AtPath, identity })
    }

    /// Whether whatever is at `file`, if anything, is gone once the Commit's deletes are made:
    /// it is a File they delete, or a directory holding nothing else, at any depth. `removed`
    /// holds the [`deleted_form`](Self::deleted_form) of each Path deleted.
    fn removed_by_commit(&self, file: &FsPath, removed: &HashSet<String>) -> Result<bool> {
        let Some(there) = present_at(fs::symlink_metadata(file), file)? else { return Ok(true) };
        if !there.is_dir() {
            let deleted = self.deleted_form_on_disk(file);
            return Ok(deleted.is_some_and(|deleted| removed.contains(&deleted)));
        }
        let entries = fs::read_dir(file).map_err(|error| failed(file, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| failed(file, error))?;
            if !self.removed_by_commit(&entry.path(), removed)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The form in which [`removed_by_commit`](Self::removed_by_commit) compares the Path `path`
    /// with what is on disk: the Path itself, or where names fold, its letter-case fold, since a
    /// name found on disk may then differ in case from the Path deleted.
    fn deleted_form(&self, path: &str) -> String {
        if self.names_fold { letter_case_fold(path) } else { path.to_owned() }
    }

    /// The [`deleted_form`](Self::deleted_form) of `file`, a place on disk under the root, if its
    /// name is Unicode.
    fn deleted_form_on_disk(&self, file: &FsPath) -> Option<String> {
        let relative = file.strip_prefix(&self.root).ok()?;
        let segments: Option<Vec<&str>> = relative.iter().map(|segment| segment.to_str()).collect();
        Some(self.deleted_form(&segments?.join("/")))
    }

    /// Which file on disk `name` in `directory` is, whichever symlinks to directories led there:
    /// the directory as [`fs::canonicalize`] gives it, and `name`, which may go on through
    /// directories that don't exist yet. Where names fold, it is folded as a whole, since two
    /// names that differ only in letter case are then one file. (If a symlink leads from there to
    /// a filesystem that doesn't fold, that could take two files for one, which refuses a Commit
    /// that would have been fine, never the other way round.)
    fn identity(&self, directory: &FsPath, name: &std::ffi::OsStr) -> Result<PathBuf> {
        let canonical = fs::canonicalize(directory).map_err(|error| failed(directory, error))?;
        let identity = canonical.join(name);
        match identity.to_str() {
            Some(text) if self.names_fold => Ok(PathBuf::from(letter_case_fold(text))),
            _ => Ok(identity),
        }
    }

    /// Makes the directories the File at `path` goes in that don't exist under their own names,
    /// from the root down, adding each directory changed to `changed`. Something else with one of
    /// their names is removed first: a directory the Commit's deletes emptied, which on a
    /// filesystem that ignores letter case can differ from it in case.
    fn make_directories(&self, path: &Path, changed: &mut BTreeSet<PathBuf>) -> Result<()> {
        let segments: Vec<&str> = path.as_str().split('/').collect();
        let directories = &segments[..segments.len() - 1];
        let OwnDirectories { mut directory, count, .. } = self.own_directories(directories)?;
        for segment in &directories[count..] {
            let parent = directory.clone();
            directory.push(segment);
            if present_at(fs::symlink_metadata(&directory), &directory)?.is_some() {
                remove_empty_directories(&directory).map_err(|error| failed(&directory, error))?;
            }
            fs::create_dir(&directory).map_err(|error| failed(&directory, error))?;
            changed.insert(parent);
            changed.insert(directory.clone());
        }
        Ok(())
    }

    /// The Path of every File under `prefix`, in order, with the directory tree walked from the
    /// Prefix down. Names that aren't Paths are left out, and so is everything under them.
    fn paths_under(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        let mut found = Vec::new();
        if let Some(name) = prefix.as_str().strip_suffix('/')
            && !self.found_under_its_own_name(name)?
        {
            return Ok(found);
        }
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
        directory: &FsPath,
        prefix: &str,
        ancestors: &mut Vec<PathBuf>,
        found: &mut Vec<Path>,
    ) -> Result<()> {
        let Some(entries) = present_at(fs::read_dir(directory), directory)? else { return Ok(()) };
        for entry in entries {
            let entry = entry.map_err(|error| failed(directory, error))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            let Some(on_disk) = OnDisk::of(&entry)? else { continue };
            match on_disk {
                OnDisk::File => {
                    if let Ok(path) = Path::new(format!("{prefix}{name}")) {
                        found.push(path);
                    }
                }
                OnDisk::Directory { symlink } => {
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
enum OnDisk {
    File,
    Directory { symlink: bool },
}

impl OnDisk {
    /// What `entry` is, or `None` if it is neither a File nor a directory, as a symlink to nothing
    /// isn't.
    fn of(entry: &fs::DirEntry) -> Result<Option<OnDisk>> {
        let file_type = entry.file_type().map_err(|error| failed(&entry.path(), error))?;
        let symlink = file_type.is_symlink();
        let file_type = if symlink {
            match present_at(fs::metadata(entry.path()), &entry.path())? {
                Some(metadata) => metadata.file_type(),
                None => return Ok(None),
            }
        } else {
            file_type
        };
        Ok(if file_type.is_dir() {
            Some(OnDisk::Directory { symlink })
        } else if file_type.is_file() {
            Some(OnDisk::File)
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
        // Listed Paths have their own names, so they aren't checked again.
        revisions(self.paths_under(prefix)?, |path| read_file(&self.file(path.as_str())))
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
                let Some(entries) = present_at(fs::read_dir(&directory), &directory)? else {
                    continue;
                };
                for entry in entries {
                    let entry = entry.map_err(|error| failed(&directory, error))?;
                    let Some(entry_name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    if letter_case_fold(&entry_name) != *segment {
                        continue;
                    }
                    let Some(on_disk) = OnDisk::of(&entry)? else { continue };
                    let named = format!("{prefix}{entry_name}");
                    match on_disk {
                        OnDisk::Directory { .. } if !last => next.push(format!("{named}/")),
                        // Every File under `name` has the name `name`, so none is looked at.
                        OnDisk::Directory { .. } if format!("{named}/") == name => {}
                        OnDisk::Directory { .. } => {
                            if let Ok(under) = Prefix::new(format!("{named}/")) {
                                found.extend(AreaRoot::paths_under(self, &under)?);
                            }
                        }
                        OnDisk::File if last => found.extend(Path::new(named)),
                        OnDisk::File => {}
                    }
                }
            }
            directories = next;
        }
        found.retain(|path| !has_name(path, name));
        Ok(found)
    }
}

/// Where a write goes, as [`AreaRoot::where_to_write`] works it out.
struct Destination {
    /// The directory its temporary file goes in.
    directory: PathBuf,
    /// What it replaces.
    target: Target,
    /// Which file on disk that is, as [`AreaRoot::identity`] gives it.
    identity: PathBuf,
}

/// How far a Path's segments lead down from the root through directories there under exactly
/// their own names, as [`AreaRoot::own_directories`] gives it.
struct OwnDirectories {
    /// The last of those directories, or the root.
    directory: PathBuf,
    /// How many segments they are.
    count: usize,
    /// Whether the segment after them is there, but only under another name that the filesystem
    /// treats as the same.
    next_under_other_name: bool,
}

/// Refuses a write to a file on disk that another Path of the Commit writes or deletes too, as
/// two Paths are when one is a symlink to the other. Which of them wins would depend on the order
/// they land in, and finishing the Commit again after a crash could lose the write. Two deletes of
/// one file are fine, as through a directory link: the second finds nothing.
#[derive(Default)]
struct SameFile {
    /// Each file on disk a Path the Commit writes is, as [`AreaRoot::identity`] gives it.
    written: HashSet<PathBuf>,
    /// Each file on disk a Path the Commit deletes is, with the letter-case fold of each such
    /// Path.
    deleted: HashMap<PathBuf, Vec<String>>,
}

impl SameFile {
    /// Notes that the Commit deletes `path`, the file `identity`.
    fn deleted(&mut self, path: &Path, identity: PathBuf) {
        self.deleted.entry(identity).or_default().push(letter_case_fold(path.as_str()));
    }

    /// Gives [`SameFile`](InvalidPathReason::SameFile) for a write of `path`, the file `identity`,
    /// if the Commit writes that file under another Path too, or deletes it. If `renames_case`, a
    /// delete of a Path that differs only in letter case is a rename rather than a clash: where
    /// names fold, deleting `Foo` and writing `foo` is the same file on disk, and finishing deletes
    /// first.
    fn written(&mut self, path: &Path, identity: PathBuf, renames_case: bool) -> Result<()> {
        let fold = letter_case_fold(path.as_str());
        let deleted = self
            .deleted
            .get(&identity)
            .is_some_and(|deleted| deleted.iter().any(|deleted| !renames_case || *deleted != fold));
        if deleted || !self.written.insert(identity) {
            return Err(Error::InvalidPath {
                path: path.as_str().to_owned(),
                reason: InvalidPathReason::SameFile,
            });
        }
        Ok(())
    }
}

/// Whether `error` is from [`AreaRoot::stop_at`], which is never tried again.
fn stopped(error: &Error) -> bool {
    #[cfg(feature = "testing")]
    if let Error::Backend(error) = error {
        return error.is::<Stopped>();
    }
    #[cfg(not(feature = "testing"))]
    let _ = error;
    false
}

/// The Revision of each of `paths` that `read` finds a File at, with its Path, in the same order.
/// A File removed since its Path was listed is left out.
fn revisions(
    paths: Vec<Path>,
    mut read: impl FnMut(&Path) -> Result<Option<(Vec<u8>, Timestamp)>>,
) -> Result<Vec<(Path, Revision)>> {
    let mut files = Vec::new();
    for path in paths {
        if let Some((contents, _)) = read(&path)? {
            files.push((path, Revision::of_bytes(&contents)));
        }
    }
    Ok(files)
}

/// Whether `directory` has an entry named exactly `name`.
fn has_entry(directory: &FsPath, name: &std::ffi::OsStr) -> Result<bool> {
    let entries = fs::read_dir(directory).map_err(|error| failed(directory, error))?;
    for entry in entries {
        if entry.map_err(|error| failed(directory, error))?.file_name() == name {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Where the File at `path`, or the directory of a Prefix, is on disk in the Area whose root is
/// `root`, or would be, one segment at a time.
fn on_disk(root: &FsPath, path: &str) -> PathBuf {
    let mut file = root.to_path_buf();
    file.extend(path.split('/'));
    file
}

/// The contents of the file at `file` and when it was last modified, or `None` if there is no
/// file there. It follows a symlink.
fn read_file(file: &FsPath) -> Result<Option<(Vec<u8>, Timestamp)>> {
    let read = || -> io::Result<Option<(Vec<u8>, SystemTime)>> {
        // Only a file is opened: opening a FIFO, say, could wait for a writer forever.
        if !present(fs::metadata(file))?.is_some_and(|metadata| metadata.is_file()) {
            return Ok(None);
        }
        let Some(mut opened) = present(fs::File::open(file))? else { return Ok(None) };
        let metadata = opened.metadata()?;
        if !metadata.is_file() {
            return Ok(None);
        }
        let mut contents = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        opened.read_to_end(&mut contents)?;
        Ok(Some((contents, metadata.modified()?)))
    };
    let Some((contents, modified)) = read().map_err(|error| failed(file, error))? else {
        return Ok(None);
    };
    Ok(Some((contents, Timestamp::try_from(modified).map_err(Error::backend)?)))
}

/// Writes `contents` to `replace`'s temporary file, which must not exist yet, with the permissions
/// of the File it replaces, if there is one, and `modified` as its modification time, and forces
/// it to disk.
fn write_temporary_file(
    area: &AreaRoot,
    replace: &Replace,
    contents: &str,
    modified: Timestamp,
) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&replace.temporary)?;
    file.write_all(contents.as_bytes())?;
    if let Some(replaced) = present(fs::metadata(replace.on_disk(&area.root)))? {
        file.set_permissions(replaced.permissions())?;
    }
    file.set_modified(SystemTime::from(modified))?;
    file.sync_all()
}

/// Removes `directory`, which holds nothing but directories that hold nothing. It fails if there
/// is anything else in it.
fn remove_empty_directories(directory: &FsPath) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_directories(&entry.path())?;
        }
    }
    fs::remove_dir(directory)
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

/// What `result` gave, or `None` if it failed because there is nothing there: no such file, or a
/// file where a directory on the way would have to be.
fn present<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if is_absent(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// What `result`, from doing something to `path`, gave, as [`present`] does, with any other
/// failure as tidings' error.
fn present_at<T>(result: io::Result<T>, path: &FsPath) -> Result<Option<T>> {
    present(result).map_err(|error| failed(path, error))
}

/// Whether `error` means that there is nothing there: no such file, or a file where a directory
/// on the way would have to be.
fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

/// The Backend failed with `error`, doing something to `path`.
fn failed(path: &FsPath, error: io::Error) -> Error {
    Error::backend(io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}
