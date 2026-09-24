# tidings

Text files for an application, in three areas (config, data, cache), stored on the filesystem, in
SQLite or in memory. Writes happen only through staged commits. Every change is reported on a
change feed.

Status: early. Stores in memory and on SQLite exist so far: reading, stat and listing, commits of
writes and deletes with preconditions, checked paths, the change feed, and snapshots. Still to
come: the filesystem backend, resyncs, the blocking API, seeing another process's commits to the
same SQLite databases, and commits that finish after being cancelled. Until then, on SQLite, a
commit whose future is dropped once it has started can be applied without being reported on the
change feed. See [CONTEXT.md](CONTEXT.md) and [docs/adr](docs/adr).

## Consistency

- A commit is all-or-nothing, and only happens when you call `commit`. A staging you drop is
  discarded.
- Every file a commit writes gets the same last-modified time. A write that wouldn't change a
  file's contents is left out: the file keeps its time, and no change is reported.
- Reads are one file at a time. Reading several files can mix states from different commits.
  Every path that changes afterwards appears on the change feed, so read it again when it does.
- To read several files without mixing commits, use a snapshot of an area. Commits made while you hold
  it don't show in it, and aren't held up by it. SQLite and memory support snapshots; the
  filesystem doesn't, because other programs can change files mid-read. Check
  `supports_snapshots()`.
- Every change after `open` returns is reported. Unread changes to the same path are merged into
  one: the latest kind wins, and it counts as external if any of them was, so skipping your own
  changes never hides anyone else's. A commit's changes always arrive in the same batch. If
  changes may have been missed (watching failed, or an area's directory was removed), the feed
  sends a resync for that area instead: read it all again.
- The store can be cloned and shared between tasks. The change feed ends once every clone has
  been dropped, even if a snapshot is still held. Dropping the change feed leaves the store
  working.
- Changes say which path changed, not what it now contains.
- A read always returns a whole file, never a partly written one.
- A commit can require that files, or everything under a prefix, are unchanged since you read
  them, and fails with a conflict otherwise, writing nothing and naming the paths that differ. A
  precondition you stage always has to hold, even if something staged later replaces the write or
  delete it came with. On the filesystem this holds against other tidings commits. A program
  outside tidings that writes a file while a commit is being applied can have its edit
  overwritten.

## Limitations

- Text only (UTF-8). No binary files.
- Paths follow the strictest platform's rules on every backend, so a path that works on one
  platform works on all of them. Names Windows reserves (such as `CON`, `aux.txt` or `COM¹`),
  control characters, a trailing dot or space, `.` and `..`, and paths not in Unicode NFC form are
  refused, as is anything under `.tidings/`. A commit can't create a path that differs only in
  letter case from another in the same area (`Themes/a.toml` against `themes/b.toml` counts too),
  because some platforms treat them as the same name. Nor can it put a file under another file:
  `a` and `a/b` can't both exist, as on a filesystem.
- No moving a store's data from one backend to another.
- No size limit or eviction for the cache area.
- Backends are defined in this crate. You cannot plug in your own.
- On the filesystem, other programs can see a commit half-applied. Readers going through tidings
  can't.
- On SQLite, each area is a database (`config.sqlite3`, `data.sqlite3` or `cache.sqlite3`) in the
  area's directory, written only by tidings. Other programs writing to it directly aren't
  supported.
- On SQLite, a snapshot held for a long time stops the database's write-ahead log from being
  emptied, so the log grows until the snapshot is dropped.
