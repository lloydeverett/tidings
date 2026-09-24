//! Watching the Areas' directories, so that what changes in them outside this Store, such as a
//! person's edit or another Store's Commit, arrives on the Change feed as external Changes.
//!
//! **Events.** `notify` watches each Area's root and every directory under it, and
//! `notify-debouncer-full` holds each name's events back until none has come for the debounce
//! window ([`FsOptions::debounce_window`](super::FsOptions::debounce_window)), so that an editor's
//! burst of events for one save becomes one. The watcher then waits until no more have come for
//! half a window, up to four windows in all, so that the names of one Commit, which settle a
//! moment apart, are looked at together. A Commit slower than the window has its temporary files'
//! or journal's events settle before its Files are renamed: for one of those, the watcher takes
//! each Area's lock in turn, which waits for the Commit to be applied, then waits a window more,
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
//! the events name ([`Looked`]), while [`Reported`] notes each Path the Store's Commits change
//! meanwhile. Then, holding the turn, it reads those Paths again, compares everything with
//! [`Reported`], and records the difference. So a Commit waits only for that second step, which
//! reads just the Files the Store's own Commits changed during the first. What the watcher
//! compares is then as if it had read it all holding the turn: a Commit that updated [`Reported`]
//! before the first step began had changed the disk before it too, and one that updated it after
//! noted its Paths, which are read again. So the Store's own Commits, and a Commit that gave
//! `Pending`, whose renames land later, find the Files as reported, and give nothing, and every
//! Change the watcher gives is external.
//!
//! **What is remembered.** [`Reported`] holds every File in the Area, so that removing a
//! directory, or a File no event names, can be told apart from nothing: memory in proportion to
//! the number of Files. It starts from a listing when the Store opens. Config's Files are read
//! then, so their Revisions are known; the other Areas' aren't, since a Cache can be large, so it
//! knows no Revision until a File changes. Until then, an event that rewrites a File there with
//! the same contents gives a Change, and so does setting only its modification time, which
//! inotify reports as a write. (The Revisions of the Files of a Commit in the journal are known
//! in every Area, so that finishing it later gives nothing.)
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
//! outside the Areas, and looks at the linking Path whenever any of them has events, following the
//! chain again. It notices links made, changed or removed from their Paths' events. Symlinks to
//! directories aren't followed by watching, and neither are those on the way to a link: each
//! directory is watched under its own name only, so a directory reachable under two Prefixes
//! isn't reported under both. So edits under a link to a directory elsewhere in the Area arrive
//! under that directory's own Prefix, and edits under a link to a directory outside the Areas
//! aren't reported.
//!
//! **Resyncs, and watching again.** If the watcher reports an error, or that it lost track of
//! events, the Areas it concerns get a Resync, and each is watched again from its root, since the
//! platform's watcher may have lost watches, or missed directories made meanwhile. If an Area's
//! root is removed, or renamed away, it is made again, with its `.tidings/`, watched again, and
//! gets a Resync. Either way, the Area is listed again. If watching an Area fails, when the Store
//! opens or again later, as it does once the platform's limit on watches is reached, it is tried
//! again after a window, then after twice as long each time up to half a minute, and the Area gets
//! a Resync once it is watched.

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

use super::journal::AsFinished;
use super::{AreaRoot, failed};
use crate::area::PerArea;
use crate::backend::{CommitOutcome, Observed, RawChange, off_runtime};
use crate::path::{is_temporary_file_name, range_under};
use crate::{Area, ChangeKind, Error, Origin, Path, Prefix, Result, Revision};

/// The journal, under an Area's root.
const JOURNAL: &str = ".tidings/journal";

/// The debounce window when [`FsOptions`](super::FsOptions) doesn't set one.
pub(super) const DEFAULT_WINDOW: Duration = Duration::from_millis(150);

/// The longest the watcher waits before trying again to watch an Area that it couldn't.
const LONGEST_RETRY: Duration = Duration::from_secs(30);

/// What the Change feed has been told of an Area's Files: each File there, with its Revision if it
/// is known. The Store's Commits and the watcher keep it up to date as they report Changes, both
/// holding the Store's turn with Commits.
#[derive(Debug, Default)]
pub(super) struct Reported {
    /// Each File's Path, and its Revision if it is known. Keyed by the Path's string, so that the
    /// Files under a Prefix are a range of keys ([`range_under`]).
    files: BTreeMap<String, Option<Revision>>,
    /// While the watcher looks at a burst without the turn, each Path the Store's Commits changed
    /// since it began.
    changed_while_looking: Option<BTreeSet<Path>>,
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
            if let Some(changed) = &mut self.changed_while_looking {
                changed.insert(path.clone());
            }
        }
    }

    fn is_reported(&self, path: &Path) -> bool {
        self.files.contains_key(path.as_str())
    }

    /// Starts noting the Paths the Store's Commits change, as the watcher starts looking.
    fn start_looking(&mut self) {
        self.changed_while_looking = Some(BTreeSet::new());
    }

    /// Stops noting the Paths the Store's Commits change, and gives them.
    fn stop_looking(&mut self) -> BTreeSet<Path> {
        self.changed_while_looking.take().unwrap_or_default()
    }

    /// Compares what the watcher `looked` at with what was reported, takes the difference as
    /// reported, and gives it.
    fn catch_up(&mut self, looked: Looked) -> Vec<RawChange> {
        let mut changes = BTreeMap::new();
        for (path, now) in &looked.files {
            match (self.files.get(path.as_str()), *now) {
                (Some(Some(before)), Now::There(Some(now))) if *before == now => {}
                (_, Now::There(None)) | (None, Now::Absent) => {}
                (_, Now::There(Some(now))) => {
                    self.files.insert(path.as_str().to_owned(), Some(now));
                    changes.insert(path.clone(), ChangeKind::Changed);
                }
                (Some(_), Now::Absent) => {
                    self.files.remove(path.as_str());
                    changes.insert(path.clone(), ChangeKind::Removed);
                }
            }
        }
        let looked_at: BTreeSet<&str> = looked.files.keys().map(Path::as_str).collect();
        for name in &looked.under {
            let reported_under = self.files.range(range_under(name.as_str()));
            let gone: Vec<String> = reported_under
                .map(|(under, _)| under)
                .filter(|under| !looked_at.contains(under.as_str()))
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
    fn replace(&mut self, listed: Looked) {
        let there = listed.files.into_iter().filter_map(|(path, now)| match now {
            Now::There(revision) => Some((path.as_str().to_owned(), revision)),
            Now::Absent => None,
        });
        self.files = there.collect();
    }
}

/// What the watcher saw of some Files of an Area, looking without the Store's turn with Commits.
#[derive(Debug, Default)]
struct Looked {
    /// Each File looked at, and how reads through tidings saw it.
    files: BTreeMap<Path, Now>,
    /// Each name every File under which was looked at: a File reported under one of them that
    /// isn't in `files` is gone.
    under: Vec<Path>,
    /// Whether every File in the Area was looked at, listing it.
    everything: bool,
}

/// How reads through tidings saw a File.
#[derive(Debug, Clone, Copy)]
enum Now {
    Absent,
    /// There, with its Revision. When looking at the Files under a name, one reported already
    /// isn't read, and has none: its own events say if it changed. In a listing, a File not
    /// read has none.
    There(Option<Revision>),
}

impl Looked {
    /// Whether `path` was among what was looked at.
    fn covers(&self, path: &Path) -> bool {
        self.everything
            || self.files.contains_key(path)
            || self.under.iter().any(|name| {
                let rest = path.as_str().strip_prefix(name.as_str());
                rest.is_some_and(|rest| rest.starts_with('/'))
            })
    }

    /// Reads again each of `changed`, which the Store's Commits changed while the watcher looked,
    /// that was among what was looked at.
    fn look_again(&mut self, root: &AreaRoot, changed: &BTreeSet<Path>) -> Result<()> {
        let changed: Vec<&Path> = changed.iter().filter(|path| self.covers(path)).collect();
        if changed.is_empty() {
            return Ok(());
        }
        let finished = root.as_finished()?;
        for path in changed {
            self.files.insert(path.clone(), now(&finished, path)?);
        }
        Ok(())
    }
}

/// How reads through tidings see the File at `path` now.
fn now(finished: &AsFinished<'_>, path: &Path) -> Result<Now> {
    let read = finished.read(path)?;
    Ok(read.map_or(Now::Absent, |(contents, _)| Now::There(Some(Revision::of_bytes(&contents)))))
}

/// `mutex`, locked. Nothing locked with it holds an invariant a panic could break half way, so a
/// poisoned lock is used as it is.
pub(super) fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The watcher of a Store's Areas, started when the Store opens. The Store runs it in a task of
/// its own: [`next_burst`](Self::next_burst) waits for events, [`look`](Self::look) reads what
/// they name, and [`conclude`](Self::conclude), called holding the Store's turn with Commits, works
/// out what changed. Dropping it stops watching.
#[derive(Debug)]
pub(crate) struct FsWatcher {
    /// What the debouncer reports.
    results: mpsc::UnboundedReceiver<DebounceEventResult>,
    removals: Arc<Removals>,
    watched: Arc<Mutex<Watched>>,
    /// Each Area's root, to wait for a Commit being applied to it.
    area_roots: Vec<Arc<AreaRoot>>,
    window: Duration,
}

/// Events, and errors, that settled together.
#[derive(Debug, Default)]
pub(crate) struct Burst {
    events: Vec<DebouncedEvent>,
    errors: Vec<notify::Error>,
    /// Paths the debouncer saw removed or renamed away, which have settled since.
    removed: Vec<PathBuf>,
    /// Whether it is time to try again to watch an Area that couldn't be.
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

/// What the watcher saw of each Area in a burst, before comparing it with what was reported.
#[derive(Debug)]
pub(crate) struct Looks(Vec<(Area, Look)>);

/// What the watcher saw of one Area in a burst.
#[derive(Debug)]
enum Look {
    Changes(Looked),
    /// Changes may have been missed. The Area as listed again, if it could be.
    Missed(Option<Looked>),
}

/// How [`FailurePoint`](super::FailurePoint)s make watching fail.
#[cfg(feature = "testing")]
#[derive(Debug, Default)]
pub(super) struct WatchFailures {
    /// [`WatchingFails`](super::FailurePoint::WatchingFails).
    pub(super) first_events: bool,
    /// [`WatchingAnAreaFails`](super::FailurePoint::WatchingAnAreaFails).
    pub(super) watching_an_area: usize,
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
        let now = Instant::now();
        let mut paths = lock_ignoring_poison(&self.paths);
        paths.extract_if(|_, removed| *removed + window <= now).map(|(path, _)| path).collect()
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

/// What the watcher looks after, used from the blocking threads that read the Areas.
#[derive(Debug)]
struct Watched {
    watches: Watches,
    areas: PerArea<WatchedArea>,
    links: Links,
    /// Whether the first error loses the watches of the Areas it names, as a watcher that fails
    /// can, with [`FailurePoint::WatchingFails`](super::FailurePoint::WatchingFails).
    #[cfg(feature = "testing")]
    loses_watches: bool,
}

/// One Area, as the watcher sees it.
#[derive(Debug)]
struct WatchedArea {
    root: Arc<AreaRoot>,
    reported: Arc<Mutex<Reported>>,
}

/// What `notify` watches: the Areas, and the directories outside them that symlinks lead to.
#[derive(Debug)]
struct Watches {
    debouncer: Debouncer<RecommendedWatcher, RemovalHook>,
    /// What the debouncer's file ID cache adds to, which unwatching adds to as well.
    removals: Arc<Removals>,
    /// Each Area's root as [`fs::canonicalize`] gave it when it was watched, which the paths of
    /// its events start with, or `None` while it isn't watched.
    area_paths: PerArea<Option<PathBuf>>,
    /// For each Area that couldn't be watched, when to try again, and how long it waited last.
    retrying: PerArea<Option<(Instant, Duration)>>,
    /// Each directory outside the Areas that is watched for the links and files in it that
    /// symlinks lead to, with how many there are.
    outside: HashMap<PathBuf, usize>,
    window: Duration,
    /// How many more times watching an Area fails, with
    /// [`FailurePoint::WatchingAnAreaFails`](super::FailurePoint::WatchingAnAreaFails).
    #[cfg(feature = "testing")]
    fails: usize,
}

/// What a burst holds for one Area.
#[derive(Debug, Default)]
struct Seen {
    /// The names under the root, that are Paths, that events named.
    names: BTreeSet<Path>,
    /// Whether any event was in the Area, even for names that aren't Paths.
    touched: bool,
    /// Whether the journal had events, and has settled since.
    journal: bool,
    /// Whether the root was removed or renamed away.
    root_gone: bool,
    /// Whether the watcher reported an error, or that it lost track of events, for the Area.
    missed: bool,
}

impl FsWatcher {
    /// Starts watching each Area in `areas`, given its root and what was reported of it, which
    /// the watcher lists now. Events come in from here on, so nothing that happens after this
    /// returns is missed, except in an Area that couldn't be watched: it is tried again, and gets
    /// a Resync once it is watched. Fails only if no watcher can be made at all.
    pub(super) fn start(
        areas: PerArea<(Arc<AreaRoot>, Arc<Mutex<Reported>>)>,
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
            area_paths: PerArea::default(),
            retrying: PerArea::default(),
            outside: HashMap::new(),
            window,
            #[cfg(feature = "testing")]
            fails: failures.watching_an_area,
        };
        let area_roots = areas.iter().map(|(_, (root, _))| Arc::clone(root)).collect();
        let areas = PerArea::try_from_fn(|area| {
            let (root, reported) = areas.get(area);
            watches.watch_area(area, root);
            Ok(WatchedArea { root: Arc::clone(root), reported: Arc::clone(reported) })
        })?;
        let mut watched = Watched {
            watches,
            areas,
            links: Links::default(),
            #[cfg(feature = "testing")]
            loses_watches: failures.first_events,
        };
        for area in [Area::Config, Area::Data, Area::Cache] {
            let listed = watched.list(area)?;
            lock_ignoring_poison(&watched.areas.get(area).reported).replace(listed);
        }
        let watched = Arc::new(Mutex::new(watched));
        Ok(FsWatcher { results, removals, watched, area_roots, window })
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

    /// When to try again to watch an Area that couldn't be, if there is one.
    fn first_retry(&self) -> Option<Instant> {
        let watched = lock_ignoring_poison(&self.watched);
        watched.watches.retrying.iter().filter_map(|(_, retry)| retry.map(|(at, _)| at)).min()
    }

    /// Waits until no Commit is being applied to any Area, by taking the lock of each in turn.
    async fn wait_for_commits(&self) {
        let roots = self.area_roots.clone();
        let waited = off_runtime(move || {
            for root in &roots {
                drop(root.lock()?);
            }
            Ok(())
        });
        if let Err(error) = waited.await {
            tracing::debug!("waiting for a Commit to be applied failed: {error}");
        }
    }

    /// Reads what `burst` names, without the Store's turn with Commits, for
    /// [`conclude`](Self::conclude) to compare.
    pub(crate) async fn look(&self, burst: Burst) -> Looks {
        let watched = Arc::clone(&self.watched);
        match off_runtime(move || Ok(lock_ignoring_poison(&watched).look(burst))).await {
            Ok(looks) => looks,
            // The runtime is shutting down, and the Store with it.
            Err(error) => {
                tracing::debug!("looking at what changed in the Areas failed: {error}");
                Looks(Vec::new())
            }
        }
    }

    /// What changed, given what [`look`](Self::look) saw: for each Area, the Changes, all
    /// external, or that Changes to it may have been missed. The Store calls it holding its turn
    /// with Commits, and records what it gives before letting go.
    pub(crate) async fn conclude(&self, looks: Looks) -> Vec<(Area, Observed)> {
        let watched = Arc::clone(&self.watched);
        match off_runtime(move || Ok(lock_ignoring_poison(&watched).conclude(looks))).await {
            Ok(observed) => observed,
            Err(error) => {
                tracing::debug!("working out what changed in the Areas failed: {error}");
                Vec::new()
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
/// contents, and sends the rest on to `sender`. With `fails`, the first events it would send are
/// replaced by an error naming their paths.
fn handler(
    sender: mpsc::UnboundedSender<DebounceEventResult>,
    #[cfg(feature = "testing")] mut fails: bool,
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
                if std::mem::take(&mut fails) {
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

/// Whether an event of `kind` can mean that a File's contents changed. Opening, reading or closing a
/// File can't, nor changing its permissions or times: writing to it or truncating it gives a
/// modify event of its own.
fn may_change_contents(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_)))
}

/// Whether an event of `kind` can mean that what it names is gone: removed or renamed.
fn may_be_gone(kind: &EventKind) -> bool {
    matches!(kind, EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)))
}

impl Watches {
    /// Watches `area`, whose root is `root`, from its root down, making the root and its
    /// `.tidings/` first if they don't exist. Anything watched of it before is unwatched first, so
    /// that every directory there now is watched once. Gives whether it is watched. If it isn't,
    /// it is tried again later, once [`retry_due`](Self::retry_due).
    fn watch_area(&mut self, area: Area, root: &AreaRoot) -> bool {
        if let Some(path) = self.area_paths.get_mut(area).take() {
            self.unwatch(&path);
        }
        match self.watch_root(root) {
            Ok(path) => {
                *self.area_paths.get_mut(area) = Some(path);
                *self.retrying.get_mut(area) = None;
                true
            }
            Err(error) => {
                let waited = self.retrying.get(area).map(|(_, waited)| waited);
                let wait = waited.map_or(self.window, |waited| (waited * 2).min(LONGEST_RETRY));
                tracing::debug!("watching {area:?} failed, trying again in {wait:?}: {error}");
                *self.retrying.get_mut(area) = Some((Instant::now() + wait, wait));
                false
            }
        }
    }

    /// Makes `root` and its `.tidings/` if they don't exist, and watches it and every directory
    /// under it. Gives the root as it is watched.
    fn watch_root(&mut self, root: &AreaRoot) -> Result<PathBuf> {
        #[cfg(feature = "testing")]
        if self.fails > 0 {
            self.fails -= 1;
            return Err(Error::backend("failed at FailurePoint::WatchingAnAreaFails"));
        }
        drop(root.lock_file()?);
        let path = fs::canonicalize(&root.root).map_err(|error| failed(&root.root, error))?;
        self.debouncer.watch(&path, RecursiveMode::Recursive).map_err(|error| {
            Error::backend(format!("watching {} for changes failed: {error}", path.display()))
        })?;
        Ok(path)
    }

    /// Whether it is time to try again to watch `area`.
    fn retry_due(&self, area: Area) -> bool {
        self.retrying.get(area).is_some_and(|(at, _)| at <= Instant::now())
    }

    /// Watches `directory`, which holds a link or file a symlink leads to, if it is outside the
    /// Areas, and counts the links and files there that it is watched for.
    fn watch_outside(&mut self, directory: &FsPath) -> Result<()> {
        if self.in_an_area(directory) {
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

    /// Whether `directory` is in an Area, and so watched with it.
    fn in_an_area(&self, directory: &FsPath) -> bool {
        self.area_paths
            .iter()
            .any(|(_, path)| path.as_ref().is_some_and(|path| directory.starts_with(path)))
    }
}

impl Watched {
    /// Reads what `burst` names in each Area, as [`FsWatcher::look`] gives it.
    fn look(&mut self, burst: Burst) -> Looks {
        for (_, area) in self.areas.iter() {
            lock_ignoring_poison(&area.reported).start_looking();
        }
        let mut seen = PerArea::<Seen>::default();
        for error in &burst.errors {
            tracing::debug!("watching for changes failed: {error}");
            self.missed(&error.paths, &mut seen);
        }
        for event in &burst.events {
            if event.need_rescan() {
                tracing::debug!("watching lost track of events: {:?}", event.event);
                self.missed(&event.paths, &mut seen);
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

        let mut looks = Vec::new();
        for area in [Area::Config, Area::Data, Area::Cache] {
            let seen = std::mem::take(seen.get_mut(area));
            let root = Arc::clone(&self.areas.get(area).root);
            let lost = seen.root_gone || seen.missed;
            if lost || self.watches.retry_due(area) {
                if seen.root_gone {
                    tracing::debug!("the directory of {area:?} was removed or renamed away");
                }
                // Once it failed, the Area had a Resync, and gets another once it is watched.
                if self.watches.watch_area(area, &root) || lost {
                    looks.push((area, Look::Missed(self.list_or_log(area))));
                }
            } else if seen.touched {
                match self.look_at(area, &seen.names, seen.journal) {
                    Ok(looked) => looks.push((area, Look::Changes(looked))),
                    Err(error) => {
                        tracing::debug!("looking at what changed in {area:?} failed: {error}");
                        looks.push((area, Look::Missed(self.list_or_log(area))));
                    }
                }
            }
        }
        Looks(looks)
    }

    /// Loses the watches of the Areas the first error names, with
    /// [`FailurePoint::WatchingFails`](super::FailurePoint::WatchingFails).
    #[cfg(feature = "testing")]
    fn lose_watches(&mut self, seen: &PerArea<Seen>) {
        if !self.loses_watches {
            return;
        }
        for (area, seen) in seen.iter() {
            if seen.missed
                && let Some(path) = self.watches.area_paths.get(area).clone()
            {
                self.loses_watches = false;
                self.watches.unwatch(&path);
            }
        }
    }

    /// Compares what `looks` saw with what was reported, reading again what the Store's Commits
    /// changed meanwhile, and takes the difference as reported, as [`FsWatcher::conclude`] gives
    /// it.
    fn conclude(&mut self, Looks(looks): Looks) -> Vec<(Area, Observed)> {
        let mut changed = PerArea::<BTreeSet<Path>>::default();
        for (area, watched) in self.areas.iter() {
            *changed.get_mut(area) = lock_ignoring_poison(&watched.reported).stop_looking();
        }
        let mut observed = Vec::new();
        for (area, look) in looks {
            let WatchedArea { root, reported } = self.areas.get(area);
            let mut reported = lock_ignoring_poison(reported);
            match look {
                Look::Changes(mut looked) => match looked.look_again(root, changed.get(area)) {
                    Ok(()) => {
                        let changes = reported.catch_up(looked);
                        if !changes.is_empty() {
                            let origin = Origin::External;
                            observed.push((area, Observed::Changes { origin, changes }));
                        }
                    }
                    Err(error) => {
                        tracing::debug!("looking at what changed in {area:?} failed: {error}");
                        // So that every event for a File from here on is reported.
                        reported.files.clear();
                        observed.push((area, Observed::Missed));
                    }
                },
                Look::Missed(listed) => {
                    let listed = listed.and_then(|mut listed| {
                        listed.look_again(root, changed.get(area)).ok()?;
                        Some(listed)
                    });
                    match listed {
                        Some(listed) => reported.replace(listed),
                        None => reported.files.clear(),
                    }
                    observed.push((area, Observed::Missed));
                }
            }
        }
        observed
    }

    /// Adds what `path`, which an event named, is to `seen`: a name under an Area's root, or the
    /// root itself, and the Paths symlinked through it. `gone` says whether the event can mean
    /// that it is gone.
    fn locate(&self, path: &FsPath, gone: bool, seen: &mut PerArea<Seen>) {
        for (area, linking) in self.links.linking(path) {
            let seen = seen.get_mut(*area);
            seen.touched = true;
            seen.names.insert(linking.clone());
        }
        for (area, area_path) in self.watches.area_paths.iter() {
            let Some(under) = area_path.as_ref().and_then(|root| path.strip_prefix(root).ok())
            else {
                continue;
            };
            let seen = seen.get_mut(area);
            seen.touched = true;
            if under.as_os_str().is_empty() {
                seen.root_gone |= gone;
                continue;
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
    }

    /// Marks each Area that one of `paths` is in, or every Area if none is, as having missed
    /// events.
    fn missed(&self, paths: &[PathBuf], seen: &mut PerArea<Seen>) {
        let mut located = PerArea::<Seen>::default();
        for path in paths {
            self.locate(path, false, &mut located);
        }
        let any = located.iter().any(|(_, located)| located.touched);
        for (area, located) in located.iter() {
            if located.touched || !any {
                seen.get_mut(area).missed = true;
            }
        }
    }

    /// Reads `names` in `area`, and the Files under each. So that a Commit in the journal is
    /// reported whole, its Paths are read too if `journal` (its events have settled), or if it is
    /// being applied and one of them is looked at. Keeps the symlinks of the Files it reads up to
    /// date.
    fn look_at(&mut self, area: Area, names: &BTreeSet<Path>, journal: bool) -> Result<Looked> {
        let WatchedArea { root, reported } = self.areas.get(area);
        let (root, reported) = (Arc::clone(root), Arc::clone(reported));
        let finished = root.as_finished()?;
        let in_journal: BTreeSet<Path> = finished.paths().cloned().collect();
        let mut looked = Looked::default();
        for name in names {
            look_under(&finished, &reported, name, &mut looked)?;
        }
        if journal || looked.files.keys().any(|path| in_journal.contains(path)) {
            for name in in_journal.difference(names) {
                look_under(&finished, &reported, name, &mut looked)?;
            }
        }
        self.relink(area, &root, looked.files.keys())?;
        Ok(looked)
    }

    /// Lists every File in `area`. Config's Files are read, and so are those of a Commit in the
    /// journal. Keeps track of their symlinks.
    fn list(&mut self, area: Area) -> Result<Looked> {
        self.links.forget(&mut self.watches, area);
        let root = Arc::clone(&self.areas.get(area).root);
        let finished = root.as_finished()?;
        let in_journal: BTreeSet<&Path> = finished.paths().collect();
        let mut listed = Looked { everything: true, ..Looked::default() };
        for path in finished.paths_under(&Prefix::new("")?)? {
            let now = if area == Area::Config || in_journal.contains(&path) {
                now(&finished, &path)?
            } else {
                Now::There(None)
            };
            listed.files.insert(path, now);
        }
        if let Err(error) = self.relink(area, &root, listed.files.keys()) {
            tracing::debug!("following the symlinks in {area:?} failed: {error}");
        }
        Ok(listed)
    }

    /// [`list`](Self::list)s `area`, or logs why it can't be.
    fn list_or_log(&mut self, area: Area) -> Option<Looked> {
        self.list(area).map_err(|error| tracing::debug!("listing {area:?} failed: {error}")).ok()
    }

    /// Follows the symlinks of `paths` in `area`, whose root is `root`, again. Gives the first
    /// error, having followed the rest.
    fn relink<'a>(
        &mut self,
        area: Area,
        root: &AreaRoot,
        paths: impl Iterator<Item = &'a Path>,
    ) -> Result<()> {
        let mut first_error = Ok(());
        for path in paths {
            let file = root.file(path.as_str());
            let relinked = self.links.relink(&mut self.watches, area, path.clone(), &file);
            if first_error.is_ok() {
                first_error = relinked;
            }
        }
        first_error
    }
}

/// Reads the File at `name`, and the Files under it if it is a directory, into `looked`. A File
/// under it that was reported isn't read: its own events say if it changed.
fn look_under(
    finished: &AsFinished<'_>,
    reported: &Mutex<Reported>,
    name: &Path,
    looked: &mut Looked,
) -> Result<()> {
    looked.files.insert(name.clone(), now(finished, name)?);
    looked.under.push(name.clone());
    for path in finished.paths_under(&Prefix::new(format!("{name}/"))?)? {
        if looked.files.contains_key(&path) {
            continue;
        }
        let now = if lock_ignoring_poison(reported).is_reported(&path) {
            Now::There(None)
        } else {
            now(finished, &path)?
        };
        // One removed again since it was listed was never there, as far as the feed knows.
        if let Now::There(_) = now {
            looked.files.insert(path, now);
        }
    }
    Ok(())
}

/// The symlinked Files in the Areas and the links and files they lead through, so that the
/// watcher can look at a linking Path when any of those has events.
#[derive(Debug, Default)]
struct Links {
    /// Each link after a symlinked File's own, and the file at the end, with their directories as
    /// [`fs::canonicalize`] gives them, as event paths name them.
    chains: HashMap<(Area, Path), Vec<PathBuf>>,
    /// The Paths whose chains lead through each link or file.
    linking: HashMap<PathBuf, BTreeSet<(Area, Path)>>,
}

impl Links {
    /// The Paths whose chains lead through `path`.
    fn linking(&self, path: &FsPath) -> impl Iterator<Item = &(Area, Path)> {
        self.linking.get(path).into_iter().flatten()
    }

    /// Notes where `path` in `area`, which is `file` on disk, leads now, if it is a symlink to
    /// something other than a directory, and watches the directories outside the Areas on the
    /// way.
    fn relink(
        &mut self,
        watches: &mut Watches,
        area: Area,
        path: Path,
        file: &FsPath,
    ) -> Result<()> {
        let now = link_chain(file);
        let linked = (area, path);
        if self.chains.get(&linked) == now.as_ref() {
            return Ok(());
        }
        if let Some(before) = self.chains.remove(&linked) {
            self.unlink(watches, &linked, &before);
        }
        let Some(chain) = now else { return Ok(()) };
        let mut watched = Ok(());
        for hop in &chain {
            self.linking.entry(hop.clone()).or_default().insert(linked.clone());
            if let Some(directory) = hop.parent()
                && let Err(error) = watches.watch_outside(directory)
                && watched.is_ok()
            {
                watched = Err(error);
            }
        }
        self.chains.insert(linked, chain);
        watched
    }

    /// Forgets that `linked` leads through `chain`, and stops watching the directories on the way
    /// that nothing else is watched for.
    fn unlink(&mut self, watches: &mut Watches, linked: &(Area, Path), chain: &[PathBuf]) {
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

    /// Forgets every symlink in `area`.
    fn forget(&mut self, watches: &mut Watches, area: Area) {
        let in_area: Vec<_> = self.chains.keys().filter(|(of, _)| *of == area).cloned().collect();
        for linked in in_area {
            if let Some(chain) = self.chains.remove(&linked) {
                self.unlink(watches, &linked, &chain);
            }
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
