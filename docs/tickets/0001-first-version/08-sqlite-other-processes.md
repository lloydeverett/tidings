# 08: SQLite backend: other processes' Commits

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** When two copies of an app share a SQLite Store, each sees the other's Commits
as external Changes. Each Change names exactly the Paths that changed, and each Commit's Changes
arrive in one batch.

**Blocked by:** 05, 07

**Status:** done

- [x] Every Commit appends to a change log table, in the same transaction, recording the Paths,
      the kind and which Store instance wrote it.
- [x] Other processes' Commits are noticed by polling SQLite's `data_version` or by watching the
      database files, whichever works better. The poll interval is in `SqliteOptions`.
- [x] Changes from the log are tagged *local* when this Store instance wrote them, and *external*
      otherwise.
- [x] A test opens a second Store on the same Root override, which stands in for another
      process. It checks that Commits from either Store arrive on the other's Change feed as
      external, one batch per Commit.
- [x] The change log is pruned so it doesn't grow without limit.
- [x] Concurrent Commits from two Stores are applied one at a time, and Preconditions stay exact.

**Notes:**

- **Public API.** `SqliteOptions::poll_interval(Duration)` (default 100 ms). With the `testing`
  feature only, `SqliteOptions::change_log_retention(Duration)` (default 10 minutes), so that a test
  can see a Store miss Commits without waiting 10 minutes; the spec's `testing` line now names it.
  `open_sqlite` documents that it panics outside a tokio runtime, since it spawns the poller.
- **Schema step 2.** `change_log (id INTEGER PRIMARY KEY AUTOINCREMENT, store, committed_microsecond)`
  with an index on the time, and `change_log_paths (commit_id, path, removed)` (`WITHOUT ROWID`).
  `meta` gains `last_store` and `change_log_pruned_through`. `AUTOINCREMENT`, so that an id is never
  used twice even after the log is pruned empty. Each Commit that changes something appends one
  row, and one per Path, in its own transaction. A Commit that changes nothing appends nothing.
- **Store instances.** Opening a database adds one to `last_store` in the opening transaction, so
  each Store instance has a number no other has had on that database. It is per database, not per
  Store, which is all tagging needs. The instance starts reading the log at its end, read in the
  same transaction, so Commits made before `open` returns aren't reported.
- **Polling, not watching files.** Every poll interval the poller checks `PRAGMA data_version` on
  the Store's own connection for each Area. That changes only when another connection commits, so
  the Store's own Commits don't wake it, and the check needs no file-watching dependency or
  platform differences. If it changed, the poller reads the log since the last Commit the Store
  read, in a read transaction.
- **One path for every Change, in the order applied.** The Store records its own Commits' Changes
  straight from the Commit, as before (so `commit` returning still means its Changes are on the
  feed, and cancelled Commits keep working). A naive poller beside that breaks the order: if
  another Store writes `x` and then this Store deletes it before the poller runs, the local Removed
  is recorded first and the external Changed after it, so the merged kind is wrong. So:
  - A Commit reads the log since the Store's last-read position inside its `BEGIN IMMEDIATE`
    transaction, before its own changes. It returns those Commits in
    `CommitOutcome::observed_before` (`Observed::Commit { origin, changes }` or
    `Observed::Missed`), and the Store layer records them, then its own Changes, under
    `commit_order`. The position moves past them and the Commit only if
    the Commit succeeds; after a Conflict the poller reads them instead.
  - The poller reads and records under `commit_order` too, so the two never interleave.
  - A Store's own Commits in the log are tagged local. Normally the Store never reads them from the
    log, because its Commit moves its position past them. The poller would only meet one if a
    Commit was applied but reported failing, and then it is recorded, as local.
  - `commits_from_both_stores_reach_each_feed_in_the_order_they_were_applied` catches the naive
    design: with the Commit's log read removed and the poller skipping its own Commits, it failed 4
    runs out of 4 (2 of 3 at 50 rounds, so it runs 150).
- **The poller's lifetime.** `follow_other_stores` in src/store.rs holds a `FeedSender` clone
  (`FeedSender` is now `Clone`; a clone doesn't keep the feed open), the `commit_order` lock and
  the `SqlitePoller`, never `Arc<Inner>`. `Inner` keeps its `AbortHandle` and aborts it when
  dropped. The suite's feed-ending tests pass on SQLite with it running.
- **Pruning and Resync.** Each Commit, in its transaction, deletes every Commit in the log up to
  the last one older than the retention (by Commit timestamps, so by the wall clock), and records
  that id as `change_log_pruned_through`. A Store whose position is before that has missed
  Commits: reading the log gives `Observed::Missed` first, and the Store sends a Resync for the
  Area, then moves on. No registry of open Stores is needed, and a crashed process can't hold the
  log up. A Store falls that far behind if it stopped running (a stopped process, a blocked
  runtime) for 10 minutes while others committed. Because the timestamps come from the wall clock,
  a forward jump of the clock by more than the retention also prunes Commits written moments
  earlier, and Stores that haven't read them yet get a Resync. Either way the Resync is the honest
  answer, and nothing is missed silently. The log's size is bounded by the commit rate over the
  retention.
  - The Resync is built as ticket 05's notes designed it: `Unread` holds, per Area, either merged
    Changes or a Resync (`UnreadInArea`). A Resync replaces the Area's unread Changes and absorbs
    those recorded after it until it is read; `next` gives Resyncs first, one Area at a time, then
    the batch of the other Areas. `a_store_that_the_change_log_was_pruned_past_gets_a_resync` (in
    tests/behaviour/main.rs, since it needs SQLite options) checks all of that, and fails with the
    gap check removed.
  - If the poller can't read an Area's log, it sends a Resync for that Area, once until a read
    succeeds again, and logs the error through `tracing` at debug level (`tracing` is now a
    dependency). If the poller task panics, a second task waiting for it sends a Resync for every
    Area, since external Changes stop. The `LogReader` lock carries on after a panic poisoned it,
    because it only changes once a transaction commits. Nothing can make SQLite fail or the poller
    panic on demand, so none of this is tested.
- **Busy.** The busy timeout is now set explicitly to 30 s (rusqlite's default of 5 s "may
  change"). `BEGIN IMMEDIATE` takes the write lock first, so a Commit waits for the other Store's
  Commit rather than failing. `concurrent_commits_from_both_stores_keep_preconditions_exact` has
  tasks on both Stores add to a counter with write-backs; with `DEFERRED` instead it fails with
  "database is locked".
- **Tests.** tests/behaviour/two_stores.rs is a second suite, `two_stores_suite!`, for Backends
  where a second Store can be opened on the same Root override (its Fixture's `open` called twice).
  Only SQLite instantiates it for now; ticket 11 should instantiate it for the filesystem too, and
  widen the `cfg` on `mod two_stores` in tests/behaviour/main.rs. The SQLite Fixture polls every
  10 ms. The tests: external Changes in one batch in both directions (local once on the committing
  Store), Commits before `open` not reported, a large external Commit never split (with a reader on
  its own thread; recording each Change separately fails it 3 of 3), the ordering test above, and
  the Preconditions test.
- **For later tickets.** `CommitOutcome::observed_before` and `Observed` are how a Backend hands
  the Store layer other Stores' Commits it saw under its lock; memory never gives any, and they are compiled
  without `sqlite` only so the Store layer needs no `cfg`. Ticket 11 records watched events itself
  and can send `FeedSender::resync` for a failed watcher or a vanished Area root.
