# 11: Filesystem backend: watching

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0003](../../adr/0003-the-change-feed-comes-from-open.md)

**What to build:** Hot reloading on the filesystem. When a person edits or deletes a File in an
Area, or another copy of the app commits, the app gets an external Change once the editor's burst
of events has settled. Its own Commits come through as local. Symlinked Files are watched through
their links. If watching breaks or an Area directory disappears, the app gets a Resync instead of
silence.

**Blocked by:** 05, 09, 10

**Status:** done

- [x] Watching uses `notify` with `notify-debouncer-full`. The window defaults to about 150 ms
      and is set in `FsOptions`.
- [x] Editing, creating or deleting a File directly in an Area directory gives an external
      *changed* or *removed* Change.
- [x] Changes from this Store's own Commits are tagged *local*, by matching the Revisions it
      committed.
- [x] External events that turn out not to change a File's contents are dropped. (Partly: in
      Config, whose Files are read when the Store opens, and elsewhere once a File has changed.
      A File in Data or Cache untouched since `open` has no known Revision, so rewriting it with
      the same contents, or setting only its mtime, reports a Change. Reading every File there
      at `open` would mean reading the whole Cache. Documented in the README's Limitations.)
- [x] Events for `.tidings/` and tidings' temporary files are ignored.
- [x] The target of a symlinked File is watched, so edits made through the target arrive as
      Changes for the linking Path.
- [x] Commits by a second Store on the same Root override arrive as external Changes.
- [x] If an Area root is removed while running, it is recreated and watched again, and a Resync
      for that Area is sent.
- [x] If the watcher fails (tested with a failure point from ticket 10), a Resync for the
      affected Area is sent.
- [x] Dropped events and watcher errors are logged through `tracing` at debug level.

**Notes from ticket 09:**

- No Path can be named like a temporary file (`InvalidPathReason::Reserved`), nor be under
  `.tidings/`. So the watcher can ignore both by dropping events whose name isn't a valid Path,
  which listings already do for every name that isn't one.
- `two_stores_suite!` has a `seeing_each_other:` group that only SQLite runs so far. Once other
  Stores' Commits arrive on the filesystem, instantiate the whole suite for `Fs` in
  tests/behaviour/main.rs, and drop the `expect(dead_code)` at the top of two_stores.rs.
- `Store::open_fs` passes no follower to `Store::open` yet.

**Notes from ticket 10:**

- A Commit that gave `Pending` has had its Changes recorded already. When the next Commit or
  `open` finishes it, perhaps in another process, its renames and deletes land on disk later. The
  watcher must not report them again as local Changes, nor as external ones: their contents are
  what reads through tidings already showed (`AsFinished` in journal.rs).
- Until then, reads use `AsFinished`, so when the watcher asks whether an event changed a File's
  contents, it should compare against what a read through tidings gives, not the bare disk.
- `FailurePoint` is `#[non_exhaustive]`; add the watcher's failure as another variant. Note that
  `RenameFails` and `CommittedJournalFails` aren't places `fail_at` stops at but failures it
  injects, and `pause_at` takes only the places.

**Notes from building it:**

- The watcher is `src/backend/fs/watch.rs`. The Store runs it as it runs SQLite's poller: a task
  holding only the feed's sender and the Commit turn, stopped by `Inner`'s Drop, and supervised
  (`store::supervise`) so that a panic, or watching stopping, sends a Resync for every Area.
- **Local and external.** Rather than matching events against a list of Revisions this Store
  committed, each Area keeps `Reported`: every File in it, with the Revision the Change feed was
  last told of, if known. The Store's Commits update it as they are reported, and the watcher
  looks at each burst holding the Commit turn, reading each File through `AsFinished` and
  comparing. So the Store's own Commits, a `Pending` Commit's later renames, and edits that leave
  contents as they were all give nothing, and every Change the watcher gives is external. The
  race in the context notes (a Commit records, then its renames produce events) can't happen: the
  watcher reads either before the Commit starts or after it has been recorded.
- **Memory.** `Reported` holds every File, listed at open without reading them, rather than only
  outstanding local Revisions: removing a directory gives one event for the directory (the
  debouncer drops its Files' events), so the watcher needs to know what was under it. Memory is in
  proportion to the number of Files; a File's Revision is known once it changes, or for a
  Commit's Files in a journal at open. So rewriting a File untouched since open with the same
  contents reports a Change; `touch` and `chmod` never do (those events are dropped by kind).
  Documented in the README's Limitations.
- **The debouncer drops some removals.** `notify-debouncer-full` drops a name's events when its
  first was a creation and it is then removed, but a File replaced by a rename looks created. So
  a File replaced and then removed within the window gave nothing. The watcher's file ID cache
  (`RemovalHook`) notes every path the debouncer removes, and the watcher looks at each once a
  window has passed. Tested by `a_file_replaced_then_removed_straight_away_is_reported_removed`.
- **Other Stores' Commits, in one batch.** The debouncer reports each name once it is quiet, so a
  Commit's names can come a tick apart; the watcher waits half a window after each report, up to
  four windows. A Commit slower than the window (temporary files or the journal among the settled
  events) is waited for by taking and dropping each Area's lock, then a window more. A Commit left
  in the journal is looked at whole, once the journal has settled, and so is one being applied
  when one of its Files is looked at. That keeps Commits made apart whole, even under load, but
  under continuous events one can still be split. And a Store's own next Commit can reach its
  feed before another Store's Commits just before it, which SQLite avoids by reading its log
  first. So `two_stores_suite!` now has three groups: the filesystem runs `committing:` and
  `seeing_each_other:`, not `in_step:` (never split under continuous Commits, and a Store's own
  Commit after the others'). The order test in `seeing_each_other:` has a third Store commit the
  marker, and accepts that a Store told by watching says nothing of a Path a round left as it
  was. `another_stores_commits_made_apart_each_arrive_in_one_batch` is new, for both Backends.
  Recorded in the spec and README.
- **Directory symlinks.** notify is told not to follow symlinks, so each directory is watched
  under its own name only: otherwise a link to a directory in the Area would take over its events
  (inotify gives both one watch). Edits under a link to a directory elsewhere in the Area arrive
  under that directory's own Prefix; under a link to a directory outside the Areas, they aren't
  reported. File symlinks are tracked, and a target outside the Areas has its directory watched.
- **Area root removed or renamed away:** unwatched, made again with `.tidings/`, watched again,
  listed again, and a Resync. After a watcher error or lost events (inotify's overflow), the Area is
  listed again too, so that `Reported` isn't stale.
- `FailurePoint::WatchingFails` replaces the first events the watcher would pass on with an error
  naming their paths, so the real error path runs.
- Events that can't change contents (access, and metadata) are dropped as they arrive, which also
  keeps the watcher's own reads (inotify's open events) from waking it.
- Latency: an edit is reported about 1.75 windows after its last event (the window, the
  debouncer's tick of a quarter window, then half a window), about 260 ms by default.
- Flakiness: the watching tests were run many times, alone and with three copies of the
  filesystem suite running alongside; the made-apart test failed every time under that load
  before slow Commits were waited for, and never after. Linux (inotify) is the only platform
  tested.

**Changed after the review** (see its Resolution in docs/reviews/0001-first-version.md):

- The watcher no longer reads a burst holding the Commit turn. It reads without it, while
  `Reported` notes the Paths the Store's Commits change meanwhile, then takes the turn to read
  those again, compare and record.
- Every link in a chain of symlinks is watched and followed again when any of them changes.
- After a Resync for lost events or an error, the Area is watched again from its root. An Area
  that can't be watched, at `open` or later, is retried with backoff (a window, doubling, up to
  30 s) and gets a Resync once watched; `open` no longer fails for it, only if no watcher can be
  made at all. `FailurePoint::WatchingAnAreaFails { times }` tests it, and `WatchingFails` now
  also loses the Area's watches.
- Config's Files are read when the Store opens, so identical rewrites there are dropped.
- After the re-review: an Area that can't be watched at `open` gets a Resync straight away
  too; a directory in an Area is never watched apart from it for a symlink, even while the Area
  isn't watched; and a Config File that can't be read is listed with no known Revision rather
  than stopping `open`.
