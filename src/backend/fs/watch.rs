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
//! **Local and external.** The Store's own Commits are reported by the Store, as local, and each
//! one updates [`Reported`] as it does. The watcher looks at a burst holding the Store's turn with
//! Commits, as they do, so it sees each of them done and reported, or not started: their events
//! then find the Files as reported, and give nothing. The same goes for a Commit that gave
//! `Pending`, whose renames land later. So every Change the watcher gives is external.
//!
//! **What is remembered.** [`Reported`] holds every File in the Area, so that removing a
//! directory, or a File no event names, can be told apart from nothing: memory in proportion to
//! the number of Files. It starts from a listing when the Store opens, which doesn't read the
//! Files, so it knows no Revision until a File changes. Until then, an event that rewrites a File
//! with the same contents gives a Change, and so does setting only its modification time, which
//! inotify reports as a write. (It is known for the Files of a Commit in the journal,
//! which are read, so that finishing it later gives nothing.)
//!
//! **Removals the debouncer drops.** The debouncer drops the events of a name that was created and
//! then removed within the window. But a File replaced by a rename looks created, so one replaced
//! and then removed or renamed away within the window would give nothing. So the watcher also
//! takes every name the debouncer saw removed or renamed away, through its file ID cache, and
//! looks at it once it has settled.
//!
//! **Symlinks.** A symlinked File is read through its link, but an edit to the file it points to
//! gives events for that file only. So the watcher keeps each link's target, watches the target's
//! directory if it is outside the Areas, and looks at the linking Path whenever its target has
//! events. It notices links made, changed or removed from their Paths' events. Symlinks to
//! directories aren't followed by watching: each directory is watched under its own name only,
//! so a directory reachable under two Prefixes isn't reported under both. So edits under a link to
//! a directory elsewhere in the Area arrive under that directory's own Prefix, and edits under a
//! link to a directory outside the Areas aren't reported.
//!
//! **Resyncs.** If the watcher reports an error, or that it lost track of events, the Areas it
//! concerns get a Resync, and what was reported of them is listed again. If an Area's root is
//! removed, or renamed away, it is made again, with its `.tidings/`, watched again, and gets a
//! Resync.

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
use super::{AreaRoot, failed, through_links};
use crate::area::PerArea;
use crate::backend::{CommitOutcome, Observed, RawChange, off_runtime};
use crate::path::{is_temporary_file_name, range_under};
use crate::{Area, ChangeKind, Error, Origin, Path, Prefix, Result, Revision};

/// The journal, under an Area's root.
const JOURNAL: &str = ".tidings/journal";

/// The debounce window when [`FsOptions`](super::FsOptions) doesn't set one.
pub(super) const DEFAULT_WINDOW: Duration = Duration::from_millis(150);

/// What the Change feed has been told of an Area's Files: each File there, with its Revision if it
/// is known. The Store's Commits and the watcher keep it up to date as they report Changes, both
/// holding the Store's turn with Commits.
#[derive(Debug, Default)]
pub(super) struct Reported {
    /// Each File's Path, and its Revision if it is known.
    files: BTreeMap<String, Option<Revision>>,
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
        }
    }

    /// Compares what was reported of `name`, and of the Files under it if it is or was a
    /// directory, with how reads through tidings see them now (`finished`). Adds each difference
    /// to `changes`, takes it as reported, and adds each File it found or lost to `looked_at`.
    fn catch_up(
        &mut self,
        finished: &AsFinished<'_>,
        name: &str,
        changes: &mut BTreeMap<Path, ChangeKind>,
        looked_at: &mut Vec<Path>,
    ) -> Result<()> {
        let path = Path::stored(name.to_owned());
        let now = revision_of(finished, &path)?;
        match (self.files.get(name), now) {
            (Some(Some(before)), Some(now)) if *before == now => {}
            (_, Some(now)) => {
                self.files.insert(name.to_owned(), Some(now));
                changes.insert(path.clone(), ChangeKind::Changed);
            }
            (Some(_), None) => {
                self.files.remove(name);
                changes.insert(path.clone(), ChangeKind::Removed);
            }
            (None, None) => {}
        }
        looked_at.push(path);

        let there = finished.paths_under(&Prefix::new(format!("{name}/"))?)?;
        let reported_under = self.files.range(range_under(name));
        let gone: Vec<String> = reported_under
            .map(|(under, _)| under)
            .filter(|under| there.binary_search_by(|path| path.as_str().cmp(under)).is_err())
            .cloned()
            .collect();
        for gone in gone {
            self.files.remove(&gone);
            let path = Path::stored(gone);
            changes.insert(path.clone(), ChangeKind::Removed);
            looked_at.push(path);
        }
        for path in there {
            if self.files.contains_key(path.as_str()) {
                continue;
            }
            // Removed again since it was listed, it was never there as far as the feed knows.
            if let Some(revision) = revision_of(finished, &path)? {
                self.files.insert(path.as_str().to_owned(), Some(revision));
                changes.insert(path.clone(), ChangeKind::Changed);
                looked_at.push(path);
            }
        }
        Ok(())
    }
}

/// The Revision of the File at `path`, as reads through tidings see it, or `None` if there is
/// none.
fn revision_of(finished: &AsFinished<'_>, path: &Path) -> Result<Option<Revision>> {
    Ok(finished.read(path)?.map(|(contents, _)| Revision::of_bytes(&contents)))
}

/// `reported`, locked. It holds no invariant a panic could break half way, so a poisoned lock is
/// used as it is.
pub(super) fn lock<T>(reported: &Mutex<T>) -> MutexGuard<'_, T> {
    reported.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The watcher of a Store's Areas, started when the Store opens. The Store runs it in a task of
/// its own: [`next_burst`](Self::next_burst) waits for events, and [`observe`](Self::observe)
/// works out what they changed. Dropping it stops watching.
#[derive(Debug)]
pub(crate) struct FsWatcher {
    /// What the debouncer reports.
    results: mpsc::UnboundedReceiver<DebounceEventResult>,
    removals: Arc<Removals>,
    watched: Arc<Mutex<Watched>>,
    /// Each Area's root, to wait for a Commit being applied to it.
    roots: Vec<Arc<AreaRoot>>,
    window: Duration,
}

/// Events, and errors, that settled together.
#[derive(Debug, Default)]
pub(crate) struct Burst {
    events: Vec<DebouncedEvent>,
    errors: Vec<notify::Error>,
    /// Paths the debouncer saw removed or renamed away, which have settled since.
    removed: Vec<PathBuf>,
}

impl Burst {
    fn is_empty(&self) -> bool {
        self.events.is_empty() && self.errors.is_empty() && self.removed.is_empty()
    }
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
        lock(&self.paths).values().min().map(|removed| *removed + window)
    }

    /// Takes the paths that have settled: removed a window ago or more. A path made again since
    /// has events of its own, which may not have settled. It is only read, not reported, if it
    /// is what was reported, so this can report a File half written only where its burst of
    /// events began by removing it and lasted longer than the window. Waiting for its other
    /// events too would wait for the watcher's own reads, which the debouncer passes on as
    /// events like any other.
    fn take_settled(&self, window: Duration) -> Vec<PathBuf> {
        let now = Instant::now();
        let mut paths = lock(&self.paths);
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
        lock(&self.0.paths).insert(path.to_owned(), Instant::now());
        self.0.added.notify_one();
    }
}

/// What the watcher looks after, used from the blocking threads that read the Areas.
#[derive(Debug)]
struct Watched {
    debouncer: Debouncer<RecommendedWatcher, RemovalHook>,
    areas: PerArea<WatchedArea>,
    links: Links,
}

/// One Area, as the watcher sees it.
#[derive(Debug)]
struct WatchedArea {
    root: Arc<AreaRoot>,
    reported: Arc<Mutex<Reported>>,
    /// The root as [`fs::canonicalize`] gave it when it was watched: the paths of its events
    /// start with it.
    watched: PathBuf,
}

/// What a burst holds for one Area.
#[derive(Debug, Default)]
struct Seen {
    /// The names under the root, that are Paths, that events named.
    names: BTreeSet<String>,
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
    /// returns is missed. `fails` is [`FailurePoint::WatchingFails`](super::FailurePoint).
    pub(super) fn start(
        areas: PerArea<(Arc<AreaRoot>, Arc<Mutex<Reported>>)>,
        window: Duration,
        #[cfg(feature = "testing")] fails: bool,
    ) -> Result<FsWatcher> {
        let (sender, results) = mpsc::unbounded_channel();
        let removals = Arc::new(Removals::default());
        let handler = handler(
            sender,
            #[cfg(feature = "testing")]
            fails,
        );
        // Each directory is watched under its own name only: see the module's doc.
        let config = notify::Config::default().with_follow_symlinks(false);
        let hook = RemovalHook(Arc::clone(&removals));
        let mut debouncer = new_debouncer_opt(window, None, handler, hook, config)
            .map_err(|error| Error::backend(format!("watching for changes failed: {error}")))?;
        let roots = areas.iter().map(|(_, (root, _))| Arc::clone(root)).collect();
        let areas = PerArea::try_from_fn(|area| {
            let (root, reported) = areas.get(area);
            let watched = watch_root(&mut debouncer, root)?;
            Ok(WatchedArea { root: Arc::clone(root), reported: Arc::clone(reported), watched })
        })?;
        let mut watched = Watched { debouncer, areas, links: Links::default() };
        for area in [Area::Config, Area::Data, Area::Cache] {
            watched.list(area)?;
        }
        let watched = Arc::new(Mutex::new(watched));
        Ok(FsWatcher { results, removals, watched, roots, window })
    }

    /// Waits for events, then gathers them until they settle, and gives them. Gives `None` if
    /// watching has stopped.
    pub(crate) async fn next_burst(&mut self) -> Option<Burst> {
        let window = self.window;
        // How long to wait for more once something comes: for the debouncer's next report, a
        // quarter of a window away, and for a removal to settle.
        let (settle, removal_settles) = (window / 2, window + window / 4);
        loop {
            let mut burst = Burst::default();
            let first_removal_settles = self.removals.first_settles(window);
            let mut until = tokio::select! {
                result = self.results.recv() => add(&mut burst, result?, settle),
                () = self.removals.added.notified() => Instant::now() + removal_settles,
                () = sleep_until(first_removal_settles) => Instant::now(),
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
                        until = until.max(Instant::now() + removal_settles);
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
                until = Instant::now() + removal_settles;
                latest = latest.max(until);
            }
            burst.removed = self.removals.take_settled(window);
            if !burst.is_empty() {
                return Some(burst);
            }
        }
    }

    /// Waits until no Commit is being applied to any Area, by taking the lock of each in turn.
    async fn wait_for_commits(&self) {
        let roots = self.roots.clone();
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

    /// What `burst` changed: for each Area, the Changes, all external, or that Changes to it may
    /// have been missed. It reads the Areas, so the Store calls it holding its turn with Commits.
    pub(crate) async fn observe(&self, burst: Burst) -> Vec<(Area, Observed)> {
        let watched = Arc::clone(&self.watched);
        match off_runtime(move || Ok(lock(&watched).observe(burst))).await {
            Ok(observed) => observed,
            // The runtime is shutting down, and the Store with it.
            Err(error) => {
                tracing::debug!("looking at what changed in the Areas failed: {error}");
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

/// Makes the root of `area`, and its `.tidings/`, if they don't exist, and watches it and every
/// directory under it. Gives the root as it watches it.
fn watch_root(
    debouncer: &mut Debouncer<RecommendedWatcher, RemovalHook>,
    area: &AreaRoot,
) -> Result<PathBuf> {
    drop(area.lock_file()?);
    let watched = fs::canonicalize(&area.root).map_err(|error| failed(&area.root, error))?;
    debouncer.watch(&watched, RecursiveMode::Recursive).map_err(|error| {
        Error::backend(format!("watching {} for changes failed: {error}", watched.display()))
    })?;
    Ok(watched)
}

impl Watched {
    /// What `burst` changed in each Area, as [`FsWatcher::observe`] gives it.
    fn observe(&mut self, burst: Burst) -> Vec<(Area, Observed)> {
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

        let mut observed = Vec::new();
        for (area, seen) in seen.iter_mut() {
            let seen = std::mem::take(seen);
            if seen.root_gone {
                tracing::debug!(
                    "the directory of {area:?} was removed: making and watching it again"
                );
                self.watch_again(area);
                observed.push((area, Observed::Missed));
            } else if seen.missed {
                self.list_or_forget(area);
                observed.push((area, Observed::Missed));
            } else if seen.touched {
                match self.compare(area, seen.names, seen.journal) {
                    Ok(changes) if changes.is_empty() => {}
                    Ok(changes) => {
                        observed
                            .push((area, Observed::Changes { origin: Origin::External, changes }));
                    }
                    Err(error) => {
                        tracing::debug!("looking at what changed in {area:?} failed: {error}");
                        self.list_or_forget(area);
                        observed.push((area, Observed::Missed));
                    }
                }
            }
        }
        observed
    }

    /// Adds what `path`, which an event named, is to `seen`: a name under an Area's root, or the
    /// root itself, and the Paths symlinked to it. `gone` says whether the event can mean that it
    /// is gone.
    fn locate(&self, path: &FsPath, gone: bool, seen: &mut PerArea<Seen>) {
        for (area, linking) in self.links.linking(path) {
            let seen = seen.get_mut(*area);
            seen.touched = true;
            seen.names.insert(linking.as_str().to_owned());
        }
        for (area, watched) in self.areas.iter() {
            let Ok(under) = path.strip_prefix(&watched.watched) else { continue };
            let seen = seen.get_mut(area);
            seen.touched = true;
            if under.as_os_str().is_empty() {
                seen.root_gone |= gone;
                continue;
            }
            let name: Option<Vec<&str>> = under.iter().map(|segment| segment.to_str()).collect();
            match name.map(|name| name.join("/")) {
                Some(name) if Path::new(name.as_str()).is_ok() => {
                    seen.names.insert(name);
                }
                Some(name) if name == JOURNAL => seen.journal = true,
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

    /// What changed in `area` at `names` since what was reported. So that a Commit in the
    /// journal is reported whole, its Paths are looked at too if `journal` (its events have
    /// settled), or if it is being applied and one of them is looked at. It takes the Changes as
    /// reported, and keeps the symlinks of the Files it looks at up to date.
    fn compare(
        &mut self,
        area: Area,
        names: BTreeSet<String>,
        journal: bool,
    ) -> Result<Vec<RawChange>> {
        let WatchedArea { root, reported, .. } = self.areas.get(area);
        let (root, reported) = (Arc::clone(root), Arc::clone(reported));
        let mut reported = lock(&reported);
        let finished = root.as_finished()?;
        let in_journal: BTreeSet<String> =
            finished.paths().map(|path| path.as_str().to_owned()).collect();
        let (mut changes, mut looked_at) = (BTreeMap::new(), Vec::<Path>::new());
        for name in &names {
            reported.catch_up(&finished, name, &mut changes, &mut looked_at)?;
        }
        if journal || looked_at.iter().any(|path| in_journal.contains(path.as_str())) {
            for name in in_journal.difference(&names) {
                reported.catch_up(&finished, name, &mut changes, &mut looked_at)?;
            }
        }
        let roots = self.roots();
        for path in looked_at {
            let file = root.file(path.as_str());
            self.links.relink(&mut self.debouncer, &roots, area, path, &file)?;
        }
        Ok(changes.into_iter().map(|(path, kind)| RawChange { path, kind }).collect())
    }

    /// Takes the Files in `area` now as reported, with the Revisions of those of a Commit left in
    /// its journal, which are read, and keeps track of their symlinks.
    fn list(&mut self, area: Area) -> Result<()> {
        self.links.forget(&mut self.debouncer, area);
        let WatchedArea { root, reported, .. } = self.areas.get(area);
        let (root, reported) = (Arc::clone(root), Arc::clone(reported));
        let finished = root.as_finished()?;
        let paths = finished.paths_under(&Prefix::new("")?)?;
        let mut files: BTreeMap<String, Option<Revision>> =
            paths.iter().map(|path| (path.as_str().to_owned(), None)).collect();
        for path in finished.paths() {
            if let Some(revision) = revision_of(&finished, path)? {
                files.insert(path.as_str().to_owned(), Some(revision));
            }
        }
        lock(&reported).files = files;
        let roots = self.roots();
        for path in paths {
            let file = root.file(path.as_str());
            if let Err(error) = self.links.relink(&mut self.debouncer, &roots, area, path, &file) {
                tracing::debug!("watching a symlink's target failed: {error}");
            }
        }
        Ok(())
    }

    /// [`list`](Self::list)s `area` again, after Changes to it may have been missed. If it can't be
    /// listed, nothing is taken as reported: an event for a File then gives a Change.
    fn list_or_forget(&mut self, area: Area) {
        if let Err(error) = self.list(area) {
            tracing::debug!("listing {area:?} again failed: {error}");
            lock(&self.areas.get(area).reported).files.clear();
        }
    }

    /// Makes the root of `area` again, with its `.tidings/`, once it was removed or renamed away,
    /// watches it again, and lists it.
    fn watch_again(&mut self, area: Area) {
        let watched = self.areas.get_mut(area);
        // The platform's watcher may have stopped watching it already.
        if let Err(error) = self.debouncer.unwatch(&watched.watched) {
            tracing::debug!("unwatching the old directory of {area:?} failed: {error}");
        }
        match watch_root(&mut self.debouncer, &watched.root) {
            Ok(root) => watched.watched = root,
            Err(error) => tracing::debug!("watching {area:?} again failed: {error}"),
        }
        self.list_or_forget(area);
    }

    /// Each Area's root, as watched.
    fn roots(&self) -> Vec<PathBuf> {
        self.areas.iter().map(|(_, watched)| watched.watched.clone()).collect()
    }
}

/// The symlinked Files in the Areas and the files they point to, so that the watcher can look at a
/// linking Path when its target has events.
#[derive(Debug, Default)]
struct Links {
    /// The file each symlinked File points to, with its directories as [`fs::canonicalize`]
    /// gives them, as event paths name it.
    targets: HashMap<(Area, Path), PathBuf>,
    /// The Paths linked to each target.
    linking: HashMap<PathBuf, BTreeSet<(Area, Path)>>,
    /// Each directory outside the Areas that is watched for targets in it, with how many there
    /// are.
    directories: HashMap<PathBuf, usize>,
}

impl Links {
    /// The Paths linked to `target`.
    fn linking(&self, target: &FsPath) -> impl Iterator<Item = &(Area, Path)> {
        self.linking.get(target).into_iter().flatten()
    }

    /// Notes where `path` in `area`, which is `file` on disk, points now, if it is a symlink to
    /// something other than a directory, and watches the directory that is in if it is outside the
    /// Areas, whose `roots` are watched already.
    fn relink(
        &mut self,
        debouncer: &mut Debouncer<RecommendedWatcher, RemovalHook>,
        roots: &[PathBuf],
        area: Area,
        path: Path,
        file: &FsPath,
    ) -> Result<()> {
        let now = link_target(file);
        let linked = (area, path);
        if self.targets.get(&linked) == now.as_ref() {
            return Ok(());
        }
        if let Some(before) = self.targets.remove(&linked) {
            self.unlink(debouncer, &linked, &before);
        }
        let Some(target) = now else { return Ok(()) };
        self.linking.entry(target.clone()).or_default().insert(linked.clone());
        self.targets.insert(linked, target.clone());
        let Some(directory) = target.parent() else { return Ok(()) };
        if roots.iter().any(|root| directory.starts_with(root)) {
            return Ok(());
        }
        let count = self.directories.entry(directory.to_owned()).or_default();
        *count += 1;
        if *count == 1 {
            debouncer.watch(directory, RecursiveMode::NonRecursive).map_err(|error| {
                Error::backend(format!("watching {} failed: {error}", directory.display()))
            })?;
        }
        Ok(())
    }

    /// Forgets that `linked` points to `target`, and stops watching the directory `target` is in if
    /// no other target there is watched.
    fn unlink(
        &mut self,
        debouncer: &mut Debouncer<RecommendedWatcher, RemovalHook>,
        linked: &(Area, Path),
        target: &FsPath,
    ) {
        if let Some(linking) = self.linking.get_mut(target) {
            linking.remove(linked);
            if linking.is_empty() {
                self.linking.remove(target);
            }
        }
        let Some(directory) = target.parent() else { return };
        let Some(count) = self.directories.get_mut(directory) else { return };
        *count -= 1;
        if *count == 0 {
            self.directories.remove(directory);
            if let Err(error) = debouncer.unwatch(directory) {
                tracing::debug!("unwatching {} failed: {error}", directory.display());
            }
        }
    }

    /// Forgets every symlink in `area`.
    fn forget(&mut self, debouncer: &mut Debouncer<RecommendedWatcher, RemovalHook>, area: Area) {
        let in_area: Vec<_> = self.targets.keys().filter(|(of, _)| *of == area).cloned().collect();
        for linked in in_area {
            if let Some(target) = self.targets.remove(&linked) {
                self.unlink(debouncer, &linked, &target);
            }
        }
    }
}

/// The file `file` points to, if it is a symlink to something other than a directory, with the
/// directories on the way as [`fs::canonicalize`] gives them. `None` if the directory it would be
/// in doesn't exist.
fn link_target(file: &FsPath) -> Option<PathBuf> {
    if !fs::symlink_metadata(file).ok()?.is_symlink()
        || fs::metadata(file).is_ok_and(|metadata| metadata.is_dir())
    {
        return None;
    }
    let target = through_links(file.to_owned()).ok()?;
    let directory = fs::canonicalize(target.parent()?).ok()?;
    Some(directory.join(target.file_name()?))
}
