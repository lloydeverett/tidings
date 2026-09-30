//! Watching the Location, so that what changes in it outside this Store, such as a person's edit
//! or another Store's Commit, arrives on the Change feed as external Changes.
//!
//! **Events.** `notify` watches the Location and every directory under it, and
//! `notify-debouncer-full` holds each name's events back until none has come for the debounce
//! window ([`FsOptions::debounce_window`](super::FsOptions::debounce_window)), so that an editor's
//! burst of events for one save becomes one. The watcher then waits until no more have come for
//! half a window, up to four windows in all, so that the names of one Commit, which settle a
//! moment apart, are looked at together. A Commit slower than the window has its temporary files'
//! or journal's events settle before its Files are renamed: for one of those, the watcher takes
//! the Location's lock, which waits for the Commit to be applied, then waits a window more,
//! even past the four. Events that can't change a File's contents (a File opened, read or closed,
//! or its permissions or times changed) are dropped as they arrive.
//!
//! **What changed.** Events say which names something happened to, not what. So for each name
//! that is a Path, the watcher reads the File as reads through tidings see it ([`AsFinished`]),
//! and compares it with what the Change feed was last told of it ([`Reported`]). A File whose
//! Revision is what was reported, or that is absent as reported, gives no Change, so an edit that
//! leaves the contents as they were is dropped. A name that is a directory, or was one, is
//! compared with the Files reported under it, since the directory's own events are all there may
//! be: one made or moved in with Files in it before its watch was added, or one removed, which the
//! debouncer reports without the Files in it. Names that aren't Paths are dropped, which drops
//! `.tidings/` and tidings' temporary files too. The Files of a Commit left in the journal, which
//! gave `Pending` or was interrupted, are looked at once the journal's own events have settled,
//! since reads show them finished before any event for them may come. (An ordinary Commit's
//! journal is made and removed within the window, so the debouncer drops its events. Looking at a
//! Commit's Files any sooner could report it before the changes made just before it, whose
//! events haven't settled yet.) So are those of a Commit being applied when one of its Files is
//! looked at, so that it is reported whole: whatever came before that File's events has settled
//! too.
//!
//! **Local and external, without holding up Commits.** The Store's own Commits are reported by the
//! Store, as local, and each one updates [`Reported`] while it holds the Store's turn with Commits.
//! The watcher looks at a burst in two steps. First, without the turn, it reads and hashes what
//! the events name ([`ReadFiles`]), while [`Reported`] notes each Path the Store's Commits change
//! meanwhile. Then, holding the turn, it reads those Paths again, compares everything with
//! [`Reported`], and records the difference. So a Commit waits only for that second step, which
//! reads just the Files the Store's own Commits changed during the first. What the watcher
//! compares is then as if it had read it all holding the turn: a Commit that updated [`Reported`]
//! before the first step began had changed the disk before it too, and one that updated it after
//! noted its Paths, which are read again. So the Store's own Commits, and a Commit that gave
//! `Pending`, whose renames land later, find the Files as reported, and give nothing, and every
//! Change the watcher gives is external.
//!
//! **What is remembered.** [`Reported`] holds every File in the Store, so that removing a
//! directory, or a File no event names, can be told apart from nothing: memory in proportion to
//! the number of Files. It starts from a listing when the Store opens. The Files aren't read then,
//! since they can be large, so it knows no Revision until a File changes. Until then, an event
//! that rewrites a File with the same contents gives a Change, and so does setting only its
//! modification time, which inotify reports as a write. (The Revisions of the Files of a Commit in
//! the journal are known, so that finishing it later gives nothing.)
//!
//! **Removals the debouncer drops.** The debouncer drops the events of a name that was created and
//! then removed within the window. But a File replaced by a rename looks created, so one replaced
//! and then removed or renamed away within the window would give nothing. So the watcher also
//! takes every name the debouncer saw removed or renamed away, through its file ID cache, and
//! looks at it once it has settled.
//!
//! **Symlinks.** A symlinked File is read through its links, but an edit to the file at the end
//! gives events for that file only, and so does retargeting a link on the way. So the watcher keeps
//! each chain of links, watches the directory of each link and of the file at the end if it is
//! outside the Location, and looks at the linking Path whenever any of them has events, following
//! the chain again. It notices links made, changed or removed from their Paths' events. Symlinks to
//! directories aren't followed by watching: they are left out of the Store, with everything under
//! them, so every directory that holds Files is watched under its own name. A directory replaced
//! by a link to one is compared with the Files reported under it, as any directory removed is.
//!
//! **Resyncs, and watching again.** If the watcher reports an error, or that it lost track of
//! events, the Store gets a Resync, and the Location is watched again from the top, since the
//! platform's watcher may have lost watches, or missed directories made meanwhile. If the Location
//! is removed, or renamed away, it is made again, with its `.tidings/` and Backend marker, watched
//! again, and the Store gets a Resync. Either way, the Location is listed again. If watching the
//! Location fails, when the Store opens or again later, as it does once the platform's limit on
//! watches is reached, the Store gets a Resync then, the Location is tried again after a window,
//! then after twice as long each time up to half a minute, and the Store gets another Resync once
//! it is watched. Its directories are never watched apart from it meanwhile, as a symlink to a
//! File in it would otherwise have them watched: unwatching that later would stop the Location's
//! own watch.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::file_id::FileId;
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, FileIdCache, new_debouncer_opt,
};
use tokio::sync::{Notify, mpsc};

#[cfg(feature = "testing")]
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use super::Location;
use super::journal::AsFinished;
use crate::backend::{CommitOutcome, Observed, RawChange, off_runtime};
use crate::path::{is_temporary_file_name, range_under};
use crate::{ChangeKind, Error, Origin, Path, Prefix, Result, Revision};

/// The journal, in the Location.
const JOURNAL: &str = ".tidings/journal";

/// The debounce window when [`FsOptions`](super::FsOptions) doesn't set one.
pub(super) const DEFAULT_WINDOW: Duration = Duration::from_millis(150);

/// The longest the watcher waits before trying again to watch a Location that it couldn't.
const LONGEST_RETRY: Duration = Duration::from_secs(30);

/// What the Change feed has been told of the Store's Files: each File there, with its Revision if
/// it is known. The Store's Commits and the watcher keep it up to date as they report Changes, both
/// holding the Store's turn with Commits.
#[derive(Debug, Default)]
pub(super) struct Reported {
    /// Each File's Path, and its Revision if it is known. Keyed by the Path's string, so that the
    /// Files under a Prefix are a range of keys ([`range_under`]).
    files: BTreeMap<String, Option<Revision>>,
    /// While the watcher looks at a burst without the turn, each Path the Store's Commits changed
    /// since it began.
    changed_while_reading: Option<BTreeSet<Path>>,
}

impl Reported {
    /// Takes what a Commit through this Store changed as reported, since the Store reports it.
    pub(super) fn committed(&mut self, outcome: &CommitOutcome) {
        for RawChange { path, kind } in &outcome.changes {
            match kind {
                ChangeKind::Changed => {
                    let revision = outcome.revisions.get(path).copied();
                    self.files.insert(path.as_str().to_owned(), revision);
                }
                ChangeKind::Removed => {
                    self.files.remove(path.as_str());
                }
            }
            if let Some(changed) = &mut self.changed_while_reading {
                changed.insert(path.clone());
            }
        }
    }

    fn is_reported(&self, path: &Path) -> bool {
        self.files.contains_key(path.as_str())
    }

    /// Starts noting the Paths the Store's Commits change, as the watcher starts looking.
    fn start_noting(&mut self) {
        self.changed_while_reading = Some(BTreeSet::new());
    }

    /// Stops noting the Paths the Store's Commits change, and gives them.
    fn stop_noting(&mut self) -> BTreeSet<Path> {
        self.changed_while_reading.take().unwrap_or_default()
    }

    /// Compares what the watcher looked at with what was reported, takes the difference as
    /// reported, and gives it.
    fn catch_up(&mut self, read_files: ReadFiles) -> Vec<RawChange> {
        let mut changes = BTreeMap::new();
        for (path, state) in &read_files.files {
            match (self.files.get(path.as_str()), *state) {
                (Some(Some(before)), FileState::There(Some(revision))) if *before == revision => {}
                (_, FileState::There(None)) | (None, FileState::Absent) => {}
                (_, FileState::There(Some(revision))) => {
                    self.files.insert(path.as_str().to_owned(), Some(revision));
                    changes.insert(path.clone(), ChangeKind::Changed);
                }
                (Some(_), FileState::Absent) => {
                    self.files.remove(path.as_str());
                    changes.insert(path.clone(), ChangeKind::Removed);
                }
            }
        }
        let read_names: BTreeSet<&str> = read_files.files.keys().map(Path::as_str).collect();
        for name in &read_files.listed_under {
            let reported_under = self.files.range(range_under(name.as_str()));
            let gone: Vec<String> = reported_under
                .map(|(under, _)| under)
                .filter(|under| !read_names.contains(under.as_str()))
                .cloned()
                .collect();
            for gone in gone {
                self.files.remove(&gone);
                changes.insert(Path::stored(gone), ChangeKind::Removed);
            }
        }
        changes.into_iter().map(|(path, kind)| RawChange { path, kind }).collect()
    }

    /// Takes the Files `listed` as reported, in place of what was.
    fn replace(&mut self, listed: ReadFiles) {
        let there = listed.files.into_iter().filter_map(|(path, state)| match state {
            FileState::There(revision) => Some((path.as_str().to_owned(), revision)),
            FileState::Absent => None,
        });
        self.files = there.collect();
    }
}

/// What the watcher saw of some Files, looking without the Store's turn with Commits.
#[derive(Debug, Default)]
struct ReadFiles {
    /// Each File looked at, and how reads through tidings saw it.
    files: BTreeMap<Path, FileState>,
    /// Each name every File under which was looked at: a File reported under one of them that
    /// isn't in `files` is gone.
    listed_under: Vec<Path>,
    /// Whether every File in the Store was looked at, listing it.
    whole_location: bool,
}

/// How reads through tidings saw a File.
#[derive(Debug, Clone, Copy)]
enum FileState {
    Absent,
    /// There, with its Revision. When looking at the Files under a name, one reported already
    /// isn't read, and has none: its own events say if it changed. In a listing, a File not
    /// read has none.
    There(Option<Revision>),
}

impl ReadFiles {
    /// Whether `path` was among what was looked at.
    fn covers(&self, path: &Path) -> bool {
        self.whole_location
            || self.files.contains_key(path)
            || self.listed_under.iter().any(|name| {
                let rest = path.as_str().strip_prefix(name.as_str());
                rest.is_some_and(|rest| rest.starts_with('/'))
            })
    }

    /// Reads again each of `changed`, which the Store's Commits changed while the watcher looked,
    /// that was among what was looked at.
    fn read_again(&mut self, location: &Location, changed: &BTreeSet<Path>) -> Result<()> {
        let changed: Vec<&Path> = changed.iter().filter(|path| self.covers(path)).collect();
        if changed.is_empty() {
            return Ok(());
        }
        let finished = location.as_finished()?;
        for path in changed {
            self.files.insert(path.clone(), state_of(&finished, path)?);
        }
        Ok(())
    }
}

/// How reads through tidings see the File at `path` now.
fn state_of(finished: &AsFinished<'_>, path: &Path) -> Result<FileState> {
    let read = finished.read(path)?;
    Ok(read.map_or(FileState::Absent, |(contents, _)| {
        FileState::There(Some(Revision::of_bytes(&contents)))
    }))
}

/// `mutex`, locked. Nothing locked with it holds an invariant a panic could break half way, so a
/// poisoned lock is used as it is.
pub(super) fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The watcher of a Store's Location, started when the Store opens. The Store runs it in a task of
/// its own: [`next_burst`](Self::next_burst) waits for events, [`read`](Self::read) reads what
/// they name, and [`conclude`](Self::conclude), called holding the Store's turn with Commits, works
/// out what changed. Dropping it stops watching.
#[derive(Debug)]
pub(crate) struct FsWatcher {
    /// What the debouncer reports.
    results: mpsc::UnboundedReceiver<DebounceEventResult>,
    removals: Arc<Removals>,
    watched: Arc<Mutex<Watched>>,
    /// The Location, to wait for a Commit being applied to it.
    location: Arc<Location>,
    window: Duration,
    /// Whether the Location couldn't be watched when the Store opened.
    unwatched: bool,
}

/// Events, and errors, that settled together.
#[derive(Debug, Default)]
pub(crate) struct Burst {
    events: Vec<DebouncedEvent>,
    errors: Vec<notify::Error>,
    /// Paths the debouncer saw removed or renamed away, which have settled since.
    removed: Vec<PathBuf>,
    /// Whether it is time to try again to watch the Location, if it couldn't be.
    retrying: bool,
}

impl Burst {
    fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.errors.is_empty()
            && self.removed.is_empty()
            && !self.retrying
    }
}

/// What the watcher saw in a burst, before comparing it with what was reported, if the burst
/// touched the Location at all.
#[derive(Debug)]
pub(crate) struct Readings(Option<Reading>);

/// What the watcher saw in a burst.
#[derive(Debug)]
enum Reading {
    Changes(ReadFiles),
    /// Changes may have been missed. The Store as listed again, if it could be.
    Missed(Option<ReadFiles>),
}

/// How [`FailurePoint`](super::FailurePoint)s make watching fail.
#[cfg(feature = "testing")]
#[derive(Debug)]
pub(super) struct WatchFailures {
    /// [`WatchingFails`](super::FailurePoint::WatchingFails).
    pub(super) first_events: bool,
    /// How many more times watching fails, with
    /// [`WatchingTheLocationFails`](super::FailurePoint::WatchingTheLocationFails), shared by every
    /// watcher opened with the same options.
    pub(super) watching: Arc<AtomicUsize>,
}

/// The paths the debouncer saw removed or renamed away, which it may drop the events of. Its file
/// ID cache, [`RemovalHook`], adds them.
#[derive(Debug, Default)]
struct Removals {
    /// Each such path the watcher hasn't looked at yet, and when it was last removed.
    paths: Mutex<HashMap<PathBuf, Instant>>,
    /// Woken when one is added.
    added: Notify,
}

impl Removals {
    /// When the first of the paths settles, if there are any.
    fn first_settles(&self, window: Duration) -> Option<Instant> {
        lock_ignoring_poison(&self.paths).values().min().map(|removed| *removed + window)
    }

    /// Takes the paths that have settled: removed a window ago or more. A path made again since
    /// has events of its own, which may not have settled. It is only read, not reported, if it
    /// is what was reported, so this can report a File half written only where its burst of
    /// events began by removing it and lasted longer than the window. Waiting for its other
    /// events too would wait for the watcher's own reads, which the debouncer passes on as
    /// events like any other.
    fn take_settled(&self, window: Duration) -> Vec<PathBuf> {
        let time = Instant::now();
        let mut paths = lock_ignoring_poison(&self.paths);
        paths.extract_if(|_, removed| *removed + window <= time).map(|(path, _)| path).collect()
    }
}

/// The debouncer's file ID cache, which caches no IDs, as on Linux, but adds each path removed or
/// renamed away to [`Removals`].
#[derive(Debug)]
struct RemovalHook(Arc<Removals>);

impl FileIdCache for RemovalHook {
    fn cached_file_id(&self, _path: &FsPath) -> Option<impl AsRef<FileId>> {
        None::<&FileId>
    }

    fn add_path(&mut self, _path: &FsPath, _recursive_mode: RecursiveMode) {}

    fn remove_path(&mut self, path: &FsPath) {
        lock_ignoring_poison(&self.0.paths).insert(path.to_owned(), Instant::now());
        self.0.added.notify_one();
    }
}

/// What the watcher looks after, used from the blocking threads that read the Location.
#[derive(Debug)]
struct Watched {
    watches: Watches,
    location: Arc<Location>,
    reported: Arc<Mutex<Reported>>,
    links: Links,
    /// Whether the first error loses the watch of the Location, as a watcher that fails can,
    /// with [`FailurePoint::WatchingFails`](super::FailurePoint::WatchingFails).
    #[cfg(feature = "testing")]
    first_error_loses_watches: bool,
}

/// What `notify` watches: the Location, and the directories outside it that symlinks lead to.
#[derive(Debug)]
struct Watches {
    debouncer: Debouncer<RecommendedWatcher, RemovalHook>,
    /// What the debouncer's file ID cache adds to, which unwatching adds to as well.
    removals: Arc<Removals>,
    /// The Location, which the paths of its events start with, since it was resolved through its
    /// symlinks when the Store opened. It is known while the Location isn't watched too, so that
    /// no directory in it is watched apart from it.
    location_path: PathBuf,
    /// Whether the Location is watched.
    watching: bool,
    /// If the Location couldn't be watched, when to try again, and how long it waited last.
    retrying: Option<(Instant, Duration)>,
    /// Each directory outside the Location that is watched for the links and files in it that
    /// symlinks lead to, with how many there are.
    outside: HashMap<PathBuf, usize>,
    window: Duration,
    /// How many more times watching fails, with
    /// [`FailurePoint::WatchingTheLocationFails`](super::FailurePoint::WatchingTheLocationFails).
    #[cfg(feature = "testing")]
    failures_left: Arc<AtomicUsize>,
}

/// What a burst holds.
#[derive(Debug, Default)]
struct Seen {
    /// The names in the Location, that are Paths, that events named.
    names: BTreeSet<Path>,
    /// Whether any event was in the Location, even for names that aren't Paths.
    touched: bool,
    /// Whether the journal had events, and has settled since.
    journal: bool,
    /// Whether the Location was removed or renamed away.
    location_gone: bool,
    /// Whether the watcher reported an error, or that it lost track of events.
    missed: bool,
}

impl FsWatcher {
    /// Starts watching `location`, given what was reported of it, which the watcher lists now.
    /// Events come in from here on, so nothing that happens after this returns is missed, unless
    /// the Location couldn't be watched: see [`unwatched`](Self::unwatched). Fails only if no
    /// watcher can be made at all.
    pub(super) fn start(
        (location, reported): (Arc<Location>, Arc<Mutex<Reported>>),
        window: Duration,
        #[cfg(feature = "testing")] failures: WatchFailures,
    ) -> Result<FsWatcher> {
        let (sender, results) = mpsc::unbounded_channel();
        let removals = Arc::new(Removals::default());
        let handler = handler(
            sender,
            #[cfg(feature = "testing")]
            failures.first_events,
        );
        // Each directory is watched under its own name only: see the module's doc.
        let config = notify::Config::default().with_follow_symlinks(false);
        let hook = RemovalHook(Arc::clone(&removals));
        let debouncer = new_debouncer_opt(window, None, handler, hook, config)
            .map_err(|error| Error::backend(format!("watching for changes failed: {error}")))?;
        let mut watches = Watches {
            debouncer,
            removals: Arc::clone(&removals),
            location_path: location.directory.clone(),
            watching: false,
            retrying: None,
            outside: HashMap::new(),
            window,
            #[cfg(feature = "testing")]
            failures_left: failures.watching,
        };
        let unwatched = !watches.watch_location(&location);
        let mut watched = Watched {
            watches,
            location: Arc::clone(&location),
            reported: Arc::clone(&reported),
            links: Links::default(),
            #[cfg(feature = "testing")]
            first_error_loses_watches: failures.first_events,
        };
        let listed = watched.list()?;
        lock_ignoring_poison(&reported).replace(listed);
        let watched = Arc::new(Mutex::new(watched));
        Ok(FsWatcher { results, removals, watched, location, window, unwatched })
    }

    /// Whether the Location couldn't be watched when the Store opened. Its Changes aren't reported
    /// until it is, so the Store sends a Resync straight away. It is tried again, and gets
    /// another Resync once it is watched, since its Changes were missed until then.
    pub(crate) fn unwatched(&self) -> bool {
        self.unwatched
    }

    /// Waits for events, then gathers them until they settle, and gives them. Gives `None` if
    /// watching has stopped.
    pub(crate) async fn next_burst(&mut self) -> Option<Burst> {
        let window = self.window;
        // How long to wait for more once something comes: for the debouncer's next report, a
        // quarter of a window away; for a removal's events to settle; and for the renames of a
        // slow Commit, once it is applied, to settle.
        let settle = window / 2;
        let (until_removal_settles, until_renames_settle) =
            (window + window / 4, window + window / 4);
        loop {
            let mut burst = Burst::default();
            let wake = earliest(self.removals.first_settles(window), self.first_retry());
            let mut until = tokio::select! {
                result = self.results.recv() => add(&mut burst, result?, settle),
                () = self.removals.added.notified() => Instant::now() + until_removal_settles,
                () = sleep_until(wake) => Instant::now(),
            };
            let mut latest = Instant::now() + 4 * window;
            let (mut looked_for_commits, mut waited_late) = (0, false);
            loop {
                tokio::select! {
                    result = self.results.recv() => match result {
                        Some(result) => until = until.max(add(&mut burst, result, settle)),
                        None => break,
                    },
                    () = self.removals.added.notified() => {
                        until = until.max(Instant::now() + until_removal_settles);
                        continue;
                    }
                    () = tokio::time::sleep_until(until.min(latest).into()) => {}
                }
                if Instant::now() < until.min(latest) {
                    continue;
                }
                // A Commit slower than the window has events that settled before its Files'
                // renames, which settle later: wait for it to be applied, and for them. Once, if
                // it is late already, and events keep coming.
                let new = &burst.events[looked_for_commits..];
                looked_for_commits = burst.events.len();
                if !new.iter().any(is_of_a_slow_commit) || waited_late {
                    break;
                }
                waited_late = Instant::now() >= latest;
                self.wait_for_commits().await;
                until = Instant::now() + until_renames_settle;
                latest = latest.max(until);
            }
            burst.removed = self.removals.take_settled(window);
            burst.retrying = self.first_retry().is_some_and(|at| at <= Instant::now());
            if !burst.is_empty() {
                return Some(burst);
            }
        }
    }

    /// When to try again to watch the Location, if it couldn't be.
    fn first_retry(&self) -> Option<Instant> {
        lock_ignoring_poison(&self.watched).watches.retrying.map(|(at, _)| at)
    }

    /// Waits until no Commit is being applied, by taking the lock.
    async fn wait_for_commits(&self) {
        let location = Arc::clone(&self.location);
        let waited = off_runtime(move || {
            drop(location.lock()?);
            Ok(())
        });
        if let Err(error) = waited.await {
            tracing::debug!("waiting for a Commit to be applied failed: {error}");
        }
    }

    /// Reads what `burst` names, without the Store's turn with Commits, for
    /// [`conclude`](Self::conclude) to compare.
    pub(crate) async fn read(&self, burst: Burst) -> Readings {
        let watched = Arc::clone(&self.watched);
        match off_runtime(move || Ok(lock_ignoring_poison(&watched).read(burst))).await {
            Ok(readings) => readings,
            // The runtime is shutting down, and the Store with it.
            Err(error) => {
                tracing::debug!("looking at what changed failed: {error}");
                Readings(None)
            }
        }
    }

    /// What changed, given what [`read`](Self::read) saw: the Changes, all external, or that
    /// Changes may have been missed, if either. The Store calls it holding its turn with Commits,
    /// and records what it gives before letting go.
    pub(crate) async fn conclude(&self, readings: Readings) -> Option<Observed> {
        let watched = Arc::clone(&self.watched);
        match off_runtime(move || Ok(lock_ignoring_poison(&watched).conclude(readings))).await {
            Ok(observed) => observed,
            Err(error) => {
                tracing::debug!("working out what changed failed: {error}");
                None
            }
        }
    }
}

/// Adds `result` to `burst`, and gives when the burst can end, if nothing more comes.
fn add(burst: &mut Burst, result: DebounceEventResult, settle: Duration) -> Instant {
    match result {
        Ok(events) => burst.events.extend(events),
        Err(errors) => burst.errors.extend(errors),
    }
    Instant::now() + settle
}

/// Whether `event` is for one of tidings' temporary files, or its journal, which have events that
/// settle before a Commit is applied only if the Commit took longer than the window.
fn is_of_a_slow_commit(event: &DebouncedEvent) -> bool {
    event.paths.iter().any(|path| {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        is_temporary_file_name(name) || path.ends_with(JOURNAL)
    })
}

/// The earlier of `a` and `b`, of those there are.
fn earliest(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    a.into_iter().chain(b).min()
}

/// Waits until `at`, or forever if it is `None`.
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// What the debouncer calls with what settled: it drops the events that can't change a File's
/// contents, and sends the rest on to `sender`. With `first_events_fail`, the first events it
/// would send are replaced by an error naming their paths.
fn handler(
    sender: mpsc::UnboundedSender<DebounceEventResult>,
    #[cfg(feature = "testing")] mut first_events_fail: bool,
) -> impl FnMut(DebounceEventResult) + Send + 'static {
    move |result| {
        let result = match result {
            Ok(mut events) => {
                let before = events.len();
                events.retain(|event| may_change_contents(&event.kind));
                if events.len() < before {
                    let dropped = before - events.len();
                    tracing::debug!("dropped {dropped} events that can't change a File's contents");
                }
                if events.is_empty() {
                    return;
                }
                #[cfg(feature = "testing")]
                if std::mem::take(&mut first_events_fail) {
                    let paths = events.iter().flat_map(|event| event.paths.clone()).collect();
                    let error = notify::Error::generic("failed at FailurePoint::WatchingFails");
                    let _ = sender.send(Err(vec![error.set_paths(paths)]));
                    return;
                }
                Ok(events)
            }
            Err(errors) => Err(errors),
        };
        // The watcher has stopped, and the Store with it.
        let _ = sender.send(result);
    }
}

/// Whether an event of `kind` can mean that a File's contents changed. Opening, reading or closing
/// a File can't, nor changing its permissions or times: writing to it or truncating it gives a
/// modify event of its own.
fn may_change_contents(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_)))
}

/// Whether an event of `kind` can mean that what it names is gone: removed or renamed.
fn may_be_gone(kind: &EventKind) -> bool {
    matches!(kind, EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)))
}

impl Watches {
    /// Watches `location` from the top down, making it, its `.tidings/` and its Backend marker
    /// first if they don't exist.
    /// Anything watched of it before is unwatched first, so that every directory there now is
    /// watched once. Gives whether it is watched. If it isn't, it is tried again later, once
    /// [`retry_due`](Self::retry_due).
    fn watch_location(&mut self, location: &Location) -> bool {
        if std::mem::take(&mut self.watching) {
            self.unwatch(&self.location_path.clone());
        }
        match self.watch_directory(location) {
            Ok(path) => {
                self.location_path = path;
                self.watching = true;
                self.retrying = None;
                true
            }
            Err(error) => {
                let waited = self.retrying.map(|(_, waited)| waited);
                let wait = waited.map_or(self.window, |waited| (waited * 2).min(LONGEST_RETRY));
                tracing::debug!(
                    "watching {} failed, trying again in {wait:?}: {error}",
                    self.location_path.display(),
                );
                self.retrying = Some((Instant::now() + wait, wait));
                false
            }
        }
    }

    /// Makes `location`, its `.tidings/` and its Backend marker if they don't exist, and watches
    /// it and every directory under it. Gives the Location as it is watched.
    fn watch_directory(&mut self, location: &Location) -> Result<PathBuf> {
        #[cfg(feature = "testing")]
        if self.failures_left.fetch_update(SeqCst, SeqCst, |left| left.checked_sub(1)).is_ok() {
            return Err(Error::backend("failed at FailurePoint::WatchingTheLocationFails"));
        }
        drop(location.lock_file()?);
        let path = location.directory.clone();
        self.debouncer.watch(&path, RecursiveMode::Recursive).map_err(|error| {
            Error::backend(format!("watching {} for changes failed: {error}", path.display()))
        })?;
        Ok(path)
    }

    /// Whether it is time to try again to watch the Location.
    fn retry_due(&self) -> bool {
        self.retrying.is_some_and(|(at, _)| at <= Instant::now())
    }

    /// Watches `directory`, which holds a link or file a symlink leads to, if it is outside the
    /// Location, and counts the links and files there that it is watched for.
    fn watch_outside(&mut self, directory: &FsPath) -> Result<()> {
        if self.in_the_location(directory) {
            return Ok(());
        }
        let count = self.outside.entry(directory.to_owned()).or_default();
        *count += 1;
        if *count == 1
            && let Err(error) = self.debouncer.watch(directory, RecursiveMode::NonRecursive)
        {
            self.outside.remove(directory);
            return Err(Error::backend(format!(
                "watching {} failed: {error}",
                directory.display()
            )));
        }
        Ok(())
    }

    /// Undoes one [`watch_outside`](Self::watch_outside) of `directory`: once no link or file
    /// there is watched for, it is unwatched.
    fn unwatch_outside(&mut self, directory: &FsPath) {
        let Some(count) = self.outside.get_mut(directory) else { return };
        *count -= 1;
        if *count == 0 {
            self.outside.remove(directory);
            self.unwatch(directory);
        }
    }

    /// Stops watching `path`, which the platform's watcher may have stopped watching already.
    /// The debouncer tells its file ID cache that `path` was removed then, which it wasn't.
    fn unwatch(&mut self, path: &FsPath) {
        if let Err(error) = self.debouncer.unwatch(path) {
            tracing::debug!("unwatching {} failed: {error}", path.display());
        }
        lock_ignoring_poison(&self.removals.paths).remove(path);
    }

    /// Whether `directory` is in the Location, and so watched with it, or will be once the
    /// Location is.
    fn in_the_location(&self, directory: &FsPath) -> bool {
        directory.starts_with(&self.location_path)
    }
}

impl Watched {
    /// Reads what `burst` names, as [`FsWatcher::read`] gives it.
    fn read(&mut self, burst: Burst) -> Readings {
        lock_ignoring_poison(&self.reported).start_noting();
        let mut seen = Seen::default();
        for error in &burst.errors {
            tracing::debug!("watching for changes failed: {error}");
            seen.missed = true;
        }
        for event in &burst.events {
            if event.need_rescan() {
                tracing::debug!("watching lost track of events: {:?}", event.event);
                seen.missed = true;
                continue;
            }
            for path in &event.paths {
                self.locate(path, may_be_gone(&event.kind), &mut seen);
            }
        }
        for path in &burst.removed {
            self.locate(path, true, &mut seen);
        }
        #[cfg(feature = "testing")]
        self.lose_watches(&seen);

        let location = Arc::clone(&self.location);
        let lost = seen.location_gone || seen.missed;
        if lost || self.watches.retry_due() {
            if seen.location_gone {
                tracing::debug!("{} was removed or renamed away", location.directory.display());
            }
            // Once it failed, the Store had a Resync, and gets another once it is watched.
            if self.watches.watch_location(&location) || lost {
                return Readings(Some(Reading::Missed(self.list_or_log())));
            }
        } else if seen.touched {
            return match self.read_names(&seen.names, seen.journal) {
                Ok(read_files) => Readings(Some(Reading::Changes(read_files))),
                Err(error) => {
                    tracing::debug!("looking at what changed failed: {error}");
                    Readings(Some(Reading::Missed(self.list_or_log())))
                }
            };
        }
        Readings(None)
    }

    /// Loses the watch of the Location if the first error came, with
    /// [`FailurePoint::WatchingFails`](super::FailurePoint::WatchingFails).
    #[cfg(feature = "testing")]
    fn lose_watches(&mut self, seen: &Seen) {
        if self.first_error_loses_watches && seen.missed && self.watches.watching {
            self.first_error_loses_watches = false;
            self.watches.unwatch(&self.watches.location_path.clone());
        }
    }

    /// Compares what `readings` saw with what was reported, reading again what the Store's Commits
    /// changed meanwhile, and takes the difference as reported, as [`FsWatcher::conclude`] gives
    /// it.
    fn conclude(&mut self, Readings(reading): Readings) -> Option<Observed> {
        let Watched { location, reported, .. } = self;
        let mut reported = lock_ignoring_poison(reported);
        let changed = reported.stop_noting();
        match reading? {
            Reading::Changes(mut read_files) => match read_files.read_again(location, &changed) {
                Ok(()) => {
                    let changes = reported.catch_up(read_files);
                    (!changes.is_empty())
                        .then_some(Observed::Changes { origin: Origin::External, changes })
                }
                Err(error) => {
                    tracing::debug!("looking at what changed failed: {error}");
                    // So that every event for a File from here on is reported.
                    reported.files.clear();
                    Some(Observed::Missed)
                }
            },
            Reading::Missed(listed) => {
                let listed = listed.and_then(|mut listed| {
                    listed.read_again(location, &changed).ok()?;
                    Some(listed)
                });
                match listed {
                    Some(listed) => reported.replace(listed),
                    None => reported.files.clear(),
                }
                Some(Observed::Missed)
            }
        }
    }

    /// Adds what `path`, which an event named, is to `seen`: a name in the Location, or the
    /// Location itself, and the Paths symlinked through it. `gone` says whether the event can mean
    /// that it is gone.
    fn locate(&self, path: &FsPath, gone: bool, seen: &mut Seen) {
        for linking in self.links.linking(path) {
            seen.touched = true;
            seen.names.insert(linking.clone());
        }
        let Ok(under) = path.strip_prefix(&self.watches.location_path) else { return };
        seen.touched = true;
        if under.as_os_str().is_empty() {
            seen.location_gone |= gone;
            return;
        }
        let name: Option<Vec<&str>> = under.iter().map(|segment| segment.to_str()).collect();
        let name = name.map(|name| name.join("/"));
        match name.as_deref().map(Path::new) {
            Some(Ok(path)) => {
                seen.names.insert(path);
            }
            _ if name.as_deref() == Some(JOURNAL) => seen.journal = true,
            _ => tracing::debug!("dropped an event for {}, which is no Path", path.display()),
        }
    }

    /// Reads `names`, and the Files under each. So that a Commit in the journal is reported
    /// whole, its Paths are read too if `journal` (its events have settled), or if it is being
    /// applied and one of them is looked at. Keeps the symlinks of the Files it reads up to date.
    fn read_names(&mut self, names: &BTreeSet<Path>, journal: bool) -> Result<ReadFiles> {
        let (location, reported) = (&self.location, &self.reported);
        let (location, reported) = (Arc::clone(location), Arc::clone(reported));
        let finished = location.as_finished()?;
        let in_journal: BTreeSet<Path> = finished.paths().cloned().collect();
        let mut read_files = ReadFiles::default();
        for name in names {
            read_name(&finished, &reported, name, &mut read_files)?;
        }
        if journal || read_files.files.keys().any(|path| in_journal.contains(path)) {
            for name in in_journal.difference(names) {
                read_name(&finished, &reported, name, &mut read_files)?;
            }
        }
        self.relink(&location, read_files.files.keys())?;
        Ok(read_files)
    }

    /// Lists every File, reading only those of a Commit in the journal. Keeps track of their
    /// symlinks.
    fn list(&mut self) -> Result<ReadFiles> {
        self.links.forget(&mut self.watches);
        let location = Arc::clone(&self.location);
        let finished = location.as_finished()?;
        let in_journal: BTreeSet<&Path> = finished.paths().collect();
        let mut listed = ReadFiles { whole_location: true, ..ReadFiles::default() };
        for path in finished.paths_under(&Prefix::new("")?)? {
            let state = if in_journal.contains(&path) {
                // One that can't be read is listed with no known Revision, rather than stop
                // the Store opening.
                state_of(&finished, &path).unwrap_or_else(|error| {
                    tracing::debug!("reading {path} failed: {error}");
                    FileState::There(None)
                })
            } else {
                FileState::There(None)
            };
            listed.files.insert(path, state);
        }
        if let Err(error) = self.relink(&location, listed.files.keys()) {
            tracing::debug!(
                "following the symlinks in {} failed: {error}",
                location.directory.display()
            );
        }
        Ok(listed)
    }

    /// [`list`](Self::list)s the Files, or logs why they can't be.
    fn list_or_log(&mut self) -> Option<ReadFiles> {
        self.list().map_err(|error| tracing::debug!("listing the Files failed: {error}")).ok()
    }

    /// Follows the symlinks of `paths` in `location` again. Gives the first error, having
    /// followed the rest.
    fn relink<'a>(
        &mut self,
        location: &Location,
        paths: impl Iterator<Item = &'a Path>,
    ) -> Result<()> {
        let mut first_error = Ok(());
        for path in paths {
            let file = location.file(path.as_str());
            let relinked = self.links.relink(&mut self.watches, path.clone(), &file);
            if first_error.is_ok() {
                first_error = relinked;
            }
        }
        first_error
    }
}

/// Reads the File at `name`, and the Files under it if it is a directory, into `read_files`. A File
/// under it that was reported isn't read: its own events say if it changed.
fn read_name(
    finished: &AsFinished<'_>,
    reported: &Mutex<Reported>,
    name: &Path,
    read_files: &mut ReadFiles,
) -> Result<()> {
    read_files.files.insert(name.clone(), state_of(finished, name)?);
    read_files.listed_under.push(name.clone());
    for path in finished.paths_under(&Prefix::new(format!("{name}/"))?)? {
        if read_files.files.contains_key(&path) {
            continue;
        }
        let state = if lock_ignoring_poison(reported).is_reported(&path) {
            FileState::There(None)
        } else {
            state_of(finished, &path)?
        };
        // One removed again since it was listed was never there, as far as the feed knows.
        if let FileState::There(_) = state {
            read_files.files.insert(path, state);
        }
    }
    Ok(())
}

/// The symlinked Files in the Location and the links and files they lead through, so that the
/// watcher can look at a linking Path when any of those has events.
#[derive(Debug, Default)]
struct Links {
    /// Each link after a symlinked File's own, and the file at the end, with their directories as
    /// [`fs::canonicalize`] gives them, as event paths name them.
    chains: HashMap<Path, Vec<PathBuf>>,
    /// The Paths whose chains lead through each link or file.
    linking: HashMap<PathBuf, BTreeSet<Path>>,
}

impl Links {
    /// The Paths whose chains lead through `path`.
    fn linking(&self, path: &FsPath) -> impl Iterator<Item = &Path> {
        self.linking.get(path).into_iter().flatten()
    }

    /// Notes where `path`, which is `file` on disk, leads now, if it is a symlink to something
    /// other than a directory, and watches the directories outside the Location on the way.
    fn relink(&mut self, watches: &mut Watches, path: Path, file: &FsPath) -> Result<()> {
        let chain_now = link_chain(file);
        if self.chains.get(&path) == chain_now.as_ref() {
            return Ok(());
        }
        if let Some(before) = self.chains.remove(&path) {
            self.unlink(watches, &path, &before);
        }
        let Some(chain) = chain_now else { return Ok(()) };
        let mut watched = Ok(());
        for hop in &chain {
            self.linking.entry(hop.clone()).or_default().insert(path.clone());
            if let Some(directory) = hop.parent()
                && let Err(error) = watches.watch_outside(directory)
                && watched.is_ok()
            {
                watched = Err(error);
            }
        }
        self.chains.insert(path, chain);
        watched
    }

    /// Forgets that `linked` leads through `chain`, and stops watching the directories on the way
    /// that nothing else is watched for.
    fn unlink(&mut self, watches: &mut Watches, linked: &Path, chain: &[PathBuf]) {
        for hop in chain {
            if let Some(linking) = self.linking.get_mut(hop) {
                linking.remove(linked);
                if linking.is_empty() {
                    self.linking.remove(hop);
                }
            }
            if let Some(directory) = hop.parent() {
                watches.unwatch_outside(directory);
            }
        }
    }

    /// Forgets every symlink.
    fn forget(&mut self, watches: &mut Watches) {
        for (linked, chain) in std::mem::take(&mut self.chains) {
            self.unlink(watches, &linked, &chain);
        }
    }
}

/// Where `file` leads, if it is a symlink to something other than a directory: each link after
/// it, and the file at the end, with the directories on the way as [`fs::canonicalize`] gives
/// them. It stops before a link or file whose directory doesn't exist.
fn link_chain(file: &FsPath) -> Option<Vec<PathBuf>> {
    if !fs::symlink_metadata(file).ok()?.is_symlink()
        || fs::metadata(file).is_ok_and(|metadata| metadata.is_dir())
    {
        return None;
    }
    let mut chain = Vec::new();
    let mut at = file.to_owned();
    // As many links as Linux follows before it gives up.
    for _ in 0..40 {
        if !fs::symlink_metadata(&at).is_ok_and(|metadata| metadata.is_symlink()) {
            break;
        }
        let Ok(link) = fs::read_link(&at) else { break };
        // A relative link is relative to the directory it is in.
        let next = at.parent().map_or_else(|| link.clone(), |parent| parent.join(&link));
        let (Some(directory), Some(name)) = (next.parent(), next.file_name()) else { break };
        let Ok(directory) = fs::canonicalize(directory) else { break };
        at = directory.join(name);
        chain.push(at.clone());
    }
    (!chain.is_empty()).then_some(chain)
}
