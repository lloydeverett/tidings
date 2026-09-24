# 06: Snapshots

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0006](../../adr/0006-no-snapshots-on-the-filesystem.md)

**What to build:** An app can take a Snapshot of an Area and read several Files through it
without mixing the results of different Commits. It can check when it starts whether the Store
supports Snapshots. Works on the memory Backend. The filesystem refusal comes with ticket 09.

**Blocked by:** 03

**Status:** done

- [x] `supports_snapshots()` returns whether the Store's Backend provides Snapshots. It is `true`
      on memory.
- [x] `snapshot(area)` returns a Snapshot with `read`, `stat` and `list`.
- [x] A Commit made while a Snapshot is held is not visible through it, and is visible through
      the Store.
- [x] Holding a Snapshot does not block Commits.
- [x] A Backend without Snapshots returns `Unsupported` from `snapshot`. The error variant and the
      shared test for it exist, and the test is skipped on Backends that do support Snapshots.

**Notes:**

- **Public API.** `Store::supports_snapshots()`, `Store::snapshot(area) -> Result<Snapshot>`, and
  `Snapshot` with async `read`, `stat` and `list`, which validate Paths and Prefixes as the
  Store's do (`InvalidPath`). `Error::Unsupported` is a unit variant. A Snapshot has no `area()`
  and isn't `Clone`, because nothing asks for them; it is `Send + Sync` and its futures are
  `Send` (checked in tests/store_layer.rs).
- **The memory Snapshot is copy-on-write over `Arc`s, not a persistent map.** Each Area's map is
  an `Arc<BTreeMap<Path, Arc<Stored>>>`. Taking a Snapshot clones the Area's `Arc` under the lock,
  so it copies nothing. A Commit calls `Arc::make_mut` after its checks: if a Snapshot still
  shares the map, the map is copied first, which copies each Path and a pointer to each File but
  never a File's contents. The copy is then the Backend's alone, so later Commits copy nothing
  until the next Snapshot. So each Snapshot costs at most one copy of the Area's index, paid by
  the first Commit to that Area while it's held, which is in line with the letter-case check
  every Commit already makes over every Path. A persistent map (`imbl::OrdMap`) would make that
  O(log n), at the cost of a dependency; worth it only if large Caches get snapshotted while
  committing often. The Store's own reads still read under the lock, rather than through a
  Snapshot, so that plain reads never force that copy. A Snapshot never takes the lock, so
  holding or reading one can't hold up a Commit.
- **Decision: a Snapshot doesn't keep the Store's shared state alive.** It holds only what its
  Backend needs to read it (`backend::BackendSnapshot`): memory holds the Area's map, and SQLite
  will hold its own connection in a read transaction. So, by ticket 05's rule, the Change feed
  ends once every Store handle is dropped even while a Snapshot is held, and the Snapshot can
  still be read afterwards. An app may keep a Snapshot for as long as it likes, so holding
  `Arc<Inner>` would have let it keep the feed open, which is the case the rule on `Inner` warns
  against. Tested by `a_snapshot_outlives_the_store_without_keeping_the_feed_open`.
- **The Backend seam, for tickets 07 and 09.** `Backend::supports_snapshots()` and the async
  `Backend::snapshot(area) -> Result<BackendSnapshot>`; `BackendSnapshot` is a crate-private enum
  with async `read`, `stat` and `list`, one variant per Backend that has Snapshots. SQLite adds
  `BackendSnapshot::Sqlite`, opening the connection and starting the read transaction in
  `snapshot` (off the runtime). A plain `BEGIN` is deferred: SQLite fixes what the transaction
  sees only at its first read. So `snapshot` must also read something (any `SELECT` on the
  table) before it returns, or Commits made between `snapshot` and the first read would show.
  `a_snapshot_reads_the_area_as_it_was_when_taken` catches that. The filesystem adds
  no `BackendSnapshot` variant: its `supports_snapshots` arm is `false` and its `snapshot` arm
  returns `Err(Error::Unsupported)`.
- **How the suite knows.** Through the public `supports_snapshots()`, not a Fixture flag. The
  Snapshot tests return early where it is `false`, and
  `a_backend_without_snapshots_refuses_one` returns early where it is `true`, so on memory that
  test is skipped (Rust has no runtime skip, so it shows as passed). So that a Backend can't
  silently skip its Snapshot tests by wrongly answering `false`, each Backend's module in
  tests/behaviour/main.rs asserts what it should answer: memory has `memory_supports_snapshots`.
  Tickets 07 and 09 should add the same for SQLite (`true`) and the filesystem (`false`).
  I checked the refusal test by switching memory's Snapshots off for a run: it passed and only
  `memory_supports_snapshots` failed.
- **Suite tests added:** `a_snapshot_reads_the_area_as_it_was_when_taken` (read, stat, list,
  one Area only), `reads_through_a_snapshot_never_mix_commits` (a Commit between two reads, then
  Snapshots read while another task commits pairs of Files),
  `holding_a_snapshot_does_not_hold_up_commits`,
  `a_snapshot_outlives_the_store_without_keeping_the_feed_open`,
  `a_backend_without_snapshots_refuses_one`, and Snapshot `read`, `stat` and `list` in
  `an_invalid_path_is_refused_wherever_it_is_used`. With a deliberately broken Snapshot that held
  the `commit_order` lock, the hold-up test failed; with one that held a Store handle, the
  outlives test failed.
