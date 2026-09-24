# 05: What the Change feed promises

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0003](../../adr/0003-the-change-feed-comes-from-open.md)

**What to build:** The Change feed keeps its promises however the app uses it:
- An app that reads slowly never loses a Path, and memory never grows without limit.
- An app that skips its own Changes never misses anyone else's.
- An app that shares the Store between tasks keeps the feed alive until the last handle goes.

These are properties of the Store layer, tested on the memory Backend.

**Blocked by:** 01

**Status:** done

- [x] Changes not yet read are merged per (Area, Path): the latest kind wins. Memory grows with
      the number of distinct Paths, not with the number of Commits.
- [x] A merged Change's Origin is *external* if any of the Changes merged into it was.
- [x] A Commit's Changes are never split across batches. A batch may contain several Commits'
      Changes.
- [x] `Store` is `Clone + Send + Sync`, and clones share state and the one Change feed.
- [x] Once every Store handle has been dropped, the Change feed ends (`next` returns nothing).
- [x] Dropping the Change feed leaves the Store fully working, and Changes stop being recorded.
- [x] Every Commit made after `open` returns produces a Change, including a Commit made before
      the app first polls the feed.
- [x] The rule for merged external Changes is tested on memory. Memory never produces external
      Changes itself, so the `testing` feature adds a way to inject one into the Store layer.

**Notes:**

- **How the feed holds Changes.** The mpsc channel is gone. The Store's end (`FeedSender`) and the
  `ChangeFeed` share one `Pending` behind a mutex, plus a tokio `Notify`. `Pending` holds, for
  each Area, a map from Path to the merged kind and Origin, so it has at most one entry per Area
  and Path. `next` takes everything pending as one batch. A Commit's Changes are recorded under
  one lock, so they are always taken together. A batch is in order of Area, then Path. `next` is
  cancel-safe: a `next` dropped while waiting loses nothing, which matters because apps wrap it
  in timeouts.
- **Commits reach the feed in the order they were applied** (the ordering bug from ticket 01).
  `Store::commit` used to record its Changes after the Backend's lock was released, so two
  concurrent Commits to one Path could be recorded the other way round, and the merged kind would
  be stale (Changed when the File was last Removed). Now the Store layer holds an async
  `commit_order` lock from before the Commit is applied until its Changes are recorded. This
  costs no concurrency, because every Backend's Commit takes an exclusive lock anyway. It lives in
  the Store layer, so the SQLite and filesystem Backends get it for free. The timestamp is chosen
  under it too, so a Commit applied later never has an earlier timestamp.
  `concurrent_commits_reach_the_feed_in_the_order_they_were_made` catches the bug: with the lock
  removed, it failed on every one of eight runs, within 17 to 370 of its 500 rounds.
- **The feed ends when the Store's shared state is dropped, not when the last reference to the
  feed's state goes.** `Inner`'s `Drop` calls `FeedSender::end`. What was recorded before still
  arrives, then `next` gives `None`, and keeps giving it. Anything recorded after the end is
  ignored.
- **For the tickets that add background tasks** (08's poller, 11's watcher, 10's Commits that
  finish in the background). The rule is written on `Inner`:
  - A task that runs for as long as the Store is open must **not** hold an `Arc<Inner>` (or a
    `Store`), or the feed never ends. Give it only what it needs: the Backend's parts, and a way
    to record. `FeedSender` isn't `Clone` yet, because nothing needs it. Derive `Clone` when a
    task needs one: a clone doesn't keep the feed open, because only `Inner`'s `Drop` ends it.
    `Inner` must stop such tasks when it is dropped, for example by keeping their `AbortHandle`s
    or a cancellation token, so they don't outlive the Store.
  - A task that finishes on its own, such as a cancelled Commit completing in the background
    (ticket 10), may hold an `Arc<Inner>`. It keeps the feed open only until its Changes are
    recorded, which is what ticket 10's "its Changes still arrive on the feed" needs.
- **Where raw changes enter.** `FeedSender::record(area, raw_changes, origin)` is the one
  crate-internal entry point. The Store layer decides the Origin (ticket 08 from the change log's
  Store instance, ticket 11 by matching committed Revisions) and records the batch. Record each
  observed Commit's changes in one call, so they stay in one batch.
- **The `testing` feature** adds only `Store::inject_external_change(area, path, kind)`. It
  records one external Change without touching any File. It is used by
  `tests/store_layer.rs`, which holds the Store-layer tests that don't belong in the shared
  suite: the external-Origin rule, and compile-time checks that `Store` is
  `Clone + Send + Sync`, `ChangeFeed` is `Send + Sync`, and the futures from `read`, `stat`,
  `list`, `stat_prefix`, `commit` and `next` are all `Send`. The check on `next` is not
  hypothetical: the first version of the new `next` kept a mutex guard alive across its
  `.await` and wasn't `Send`.
- **Decision: how a pending Resync meets pending Changes for its Area** (for tickets 08 and 11,
  not built here). A Resync for an Area *replaces* that Area's pending Changes and *absorbs* any
  recorded for it until it is read. The app reads the whole Area again after receiving the
  Resync, so every Change recorded before `next` hands it over is covered by that read, and
  nothing is lost. Changes recorded after it is handed over are pending as usual. A Commit or an
  external batch is always for one Area, so absorbing per Area never splits one. `next` gives
  pending Resyncs first, one Area at a time, then the batch of the other Areas' Changes. A second
  Resync for an Area already waiting adds nothing. This is why `Pending` keeps a map per Area:
  the Resync becomes per-Area state (such as a `resync: bool` beside each map, or an enum in its
  place), and replacing means clearing that Area's map. The memory stays bounded. It isn't built
  because nothing sends a Resync yet, and the `testing` feature only injects Changes.
- **The shared suite now runs on tokio's multi-threaded runtime** (4 workers), so the concurrency
  tests really run Commits at once. It gained six tests:
  - `unread_changes_are_merged_per_path_and_the_latest_kind_wins`: 1000 Commits to one Path give
    one Change.
  - `clones_of_a_store_share_its_files_and_its_change_feed`
  - `the_store_keeps_working_once_its_change_feed_is_dropped`
  - `a_commit_made_before_the_feed_is_first_read_is_reported`
  - `a_commits_changes_are_never_split_across_batches`: its reader runs on its own OS thread,
    because a task woken on tokio's pool runs in the waker's LIFO slot, only after the committing
    task yields, so it could never see a Commit half recorded. With a deliberately broken
    `record` (one lock and wake-up per Change) it failed 3 runs out of 4.
  - `concurrent_commits_reach_the_feed_in_the_order_they_were_made`

  `a_rename_that_conflicts_leaves_both_paths_as_they_were` read two batches for two unread
  Commits. They are now merged into one, so it reads one.
- "Changes stop being recorded" once the feed is dropped: the pending maps are cleared and
  recording does nothing. No test can see this through the public API, so the test checks only
  that the Store keeps working.
- **Seen in passing, for later tickets:** every Commit folds every Path in the Area for the
  letter-case check (ticket 04), so each Commit costs time in proportion to the Area's size. A
  draft of the ordering test that made 48,000 Commits into an Area growing to 3,000 Files took
  95 s. That doesn't matter for config, but a large Cache would notice.
