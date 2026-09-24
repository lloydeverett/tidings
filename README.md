# tidings

Text files for an application, in three areas (config, data, cache), stored on the filesystem, in
SQLite or in memory. Writes happen only through staged commits. Every change is reported on a
change feed.

Status: early. Stores in memory, on SQLite and on the filesystem exist so far: reading, stat and
listing, commits of writes and deletes with preconditions, checked paths, the change feed,
snapshots (not on the filesystem), and seeing other processes' commits to the same store. On the
filesystem, commits are journaled, recovery is tested at every step, a commit whose rename keeps
failing gives `Pending`, and the area directories are watched, so that people's edits and other
processes' commits reach the change feed. Still to come: the blocking API. See
[CONTEXT.md](CONTEXT.md) and [docs/adr](docs/adr).

## Consistency

- A commit is all-or-nothing, and only happens when you call `commit`. A staging you drop is
  discarded.
- A commit you cancel, by dropping its future (with a timeout, say), either never happens, if it
  was still waiting for earlier commits, or finishes in the background and is reported on the
  change feed as usual. `commit` must be called from within a tokio runtime. If that runtime shuts
  down while a cancelled commit is finishing, the commit can be applied without being reported.
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
  changes may have been missed (watching failed, an area's directory was removed, or on SQLite,
  the store fell too far behind other processes' commits), the feed sends a resync for that area
  instead: read it all again.
- Several processes can open the same store on SQLite or the filesystem, and commit to it. Their
  commits are applied one at a time, and preconditions hold exactly between them. On SQLite, each
  store checks for the others' commits every poll interval (100 ms by default, set in
  `SqliteOptions`) and reports them as external changes, each commit's in one batch, in the order
  they were applied. They are kept for 10 minutes for stores that haven't seen them yet: a store
  that falls further behind than that, because its process was stopped, say, gets a resync
  instead.
- On the filesystem, the area directories are watched. A file another program changes, makes or
  deletes there, and another process's commit, is reported as an external change once its burst
  of events has settled: none for the debounce window (150 ms by default, set in `FsOptions`),
  so that a file an editor is saving isn't reported half written. That is usually within twice
  the window, and later if the events keep coming or a commit takes longer. An event that leaves
  a file's contents as they were (it was read, touched or had its permissions changed, or was
  written again the same) reports nothing, and neither do the events of the store's own commits,
  which it reports itself, as local, when it makes them. Another process's commit is usually
  reported in one batch, and whole if it took longer than the window, but can be split if files
  keep changing for longer than four windows. Unlike on SQLite, a store's own commit can reach
  its feed before another process's commits made just before it, which arrive once they settle.
  If watching fails or loses track of events, or an area's directory is removed or renamed away
  (the cache cleared, say), that area gets a resync, and a removed directory is made again, and
  watched again.
- The store can be cloned and shared between tasks. The change feed ends once every clone has
  been dropped, even if a snapshot is still held. Dropping the change feed leaves the store
  working.
- Changes say which path changed, not what it now contains.
- A read always returns a whole file, never a partly written one.
- On the filesystem, a commit that a crash interrupts is finished when a store next opens the area
  or commits to it, if it had happened, and discarded if it hadn't. Until then, reads through
  tidings show it finished if it had happened.
- On the filesystem, a commit can give `Error::Pending`, when a file can't be replaced even after
  trying again for a moment (on Windows, while another program has it open). The commit has
  happened: its changes are reported, and reads through tidings show it. The next commit to the
  area, or opening a store, finishes it. If that still fails, the next commit gives
  `Error::Backend` and isn't made, and opening a store still works. So while a program holds a
  file open, every commit to that area fails, even one that doesn't touch the file, until the
  file is released: the error says so, and to try again later.
- A commit can require that files, or everything under a prefix, are unchanged since you read
  them, and fails with a conflict otherwise, writing nothing and naming the paths that differ. A
  precondition you stage always has to hold, even if something staged later replaces the write or
  delete it came with. On the filesystem this holds against other tidings commits. A program
  outside tidings that writes a file while a commit is being applied can have its edit
  overwritten.

## Limitations

- Text only (UTF-8). No binary files. On the filesystem, a file another program wrote that isn't
  UTF-8 is still listed, but reading it gives `Error::NotText`.
- Paths follow the strictest platform's rules on every backend, so a path that works on one
  platform works on all of them. Names Windows reserves (such as `CON`, `aux.txt` or `COM¹`),
  control characters, a trailing dot or space, `.` and `..`, and paths not in Unicode NFC form are
  refused, as is anything under `.tidings/` and any name like those of tidings' temporary files
  (`.<name>.tidings-<commit-id>-<n>`). A commit can't create a path that differs only in letter
  case from another in the same area (`Themes/a.toml` against `themes/b.toml` counts too), because
  some platforms treat them as the same name. Nor can it put a file under another file: `a` and
  `a/b` can't both exist, as on a filesystem.
- No moving a store's data from one backend to another.
- No size limit or eviction for the cache area.
- Backends are defined in this crate. You cannot plug in your own.
- On the filesystem, other programs can see a commit half-applied. Readers going through tidings
  can't, with one exception: until a commit that gave `Pending`, or that a crash interrupted, is
  finished, another path that is the same file as one it writes or deletes, through a symlink, is
  read as it is on disk.
- On the filesystem, files other programs make are read and listed like any other, but names that
  aren't valid paths (not in NFC form, not UTF-8, or reserved on Windows) are left out of listings,
  prefix revisions and prefix deletes. Two names that differ only in letter case, made by another
  program on a case-sensitive filesystem, are both listed.
- On the filesystem, a write to a symlinked file goes through the link to the file it points to,
  and the link stays. The directory the link points into must exist: a write through a link to a
  file never makes a directory. (Under a link to a directory, a write makes the directories it
  needs in the linked directory, wherever that is.) Reads and listings follow symlinks. A commit
  that writes a path and also writes or deletes another path that is the same file on disk, such
  as a link and the file it points to, is refused (`InvalidPath` with `SameFile`).
- On the filesystem, a commit is refused (`InvalidPath` with `FileUnderFile`) when something that
  isn't a file tidings can list stands where a file it writes, or a directory for one, must go: a
  directory holding only names that aren't valid paths, for example, even though listing that
  prefix shows nothing.
- On filesystems that ignore letter case (macOS's and Windows' by default), a path names only the
  file with exactly that name: reading `foo` gives nothing when only `Foo` is there. To check,
  each read looks through every directory on the way, so reading each of N files in one
  directory there costs in proportion to N, and reading them all to N².
- On the filesystem, stat reads the whole file, to hash it, and a prefix revision reads every file
  under the prefix.
- On the filesystem, watching remembers every file in each area, to tell what an event changed:
  memory in proportion to the number of files. It learns a file's contents only when it changes,
  so until then, writing a file that was there when the store opened again with the same
  contents, or setting only its modification time, reports a change.
- On the filesystem, an edit to the file a symlinked file points to is reported for the link,
  wherever that file is, and links made, changed or removed while the store runs are followed.
  If that file is outside the areas and its directory is removed or replaced, edits there are no
  longer seen until the link changes or a store opens again. Symlinks to directories aren't watched through: edits
  under a link to a directory elsewhere in the area are reported under that directory's own
  prefix only, and edits under a link to a directory outside the areas aren't reported. Where a
  commit writes a file through a symlink, another path that is the same file gets an external
  change.
- On SQLite, each area is a database (`config.sqlite3`, `data.sqlite3` or `cache.sqlite3`) in the
  area's directory, written only by tidings. Other programs writing to it directly aren't
  supported.
- On SQLite, a snapshot held for a long time stops the database's write-ahead log from being
  emptied, so the log grows until the snapshot is dropped.
