# 11: Filesystem backend: watching

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0003](../../adr/0003-the-change-feed-comes-from-open.md)

**What to build:** Hot reloading on the filesystem. When a person edits or deletes a File in an
Area, or another copy of the app commits, the app gets an external Change once the editor's burst
of events has settled. Its own Commits come through as local. Symlinked Files are watched through
their links. If watching breaks or an Area directory disappears, the app gets a Resync instead of
silence.

**Blocked by:** 05, 09, 10

**Status:** ready-for-agent

- [ ] Watching uses `notify` with `notify-debouncer-full`. The window defaults to about 150 ms
      and is set in `FsOptions`.
- [ ] Editing, creating or deleting a File directly in an Area directory gives an external
      *changed* or *removed* Change.
- [ ] Changes from this Store's own Commits are tagged *local*, by matching the Revisions it
      committed.
- [ ] External events that turn out not to change a File's contents are dropped.
- [ ] Events for `.tidings/` and tidings' temporary files are ignored.
- [ ] The target of a symlinked File is watched, so edits made through the target arrive as
      Changes for the linking Path.
- [ ] Commits by a second Store on the same Root override arrive as external Changes.
- [ ] If an Area root is removed while running, it is recreated and watched again, and a Resync
      for that Area is sent.
- [ ] If the watcher fails (tested with a failure point from ticket 10), a Resync for the
      affected Area is sent.
- [ ] Dropped events and watcher errors are logged through `tracing` at debug level.

**Notes from ticket 09:**

- No Path can be named like a temporary file (`InvalidPathReason::Reserved`), nor be under
  `.tidings/`. So the watcher can ignore both by dropping events whose name isn't a valid Path,
  which listings already do for every name that isn't one.
- `two_stores_suite!` has a `seeing_each_other:` group that only SQLite runs so far. Once other
  Stores' Commits arrive on the filesystem, instantiate the whole suite for `Fs` in
  tests/behaviour/main.rs, and drop the `expect(dead_code)` at the top of two_stores.rs.
- `Store::open_fs` passes no follower to `Store::open` yet.
