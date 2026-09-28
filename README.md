# tidings

Text files for an application, in three areas (config, data, cache), stored on the filesystem, in
SQLite or in memory. Writes happen only through all-or-nothing commits, and every change is
reported on a change feed. Async on tokio; `tidings::blocking::Store` (feature `blocking`) for
synchronous code.

Status: first version, complete but early. See [CONTEXT.md](CONTEXT.md) and [docs/adr](docs/adr).

## Consistency

Ways you could lose data or see confusing behaviour, and why.

- **A cancelled commit may still happen.** Dropping the `commit` future (e.g. on a timeout) lets
  a commit that had started finish in the background. *Why:* stopping halfway would leave it
  half-applied, and Rust can't wait for cleanup on drop.
  - If the tokio runtime shuts down meanwhile, it can be applied without being reported on the
    change feed. *Why:* the feed runs on that runtime.
  - Blocking commits can't be cancelled, so don't have this problem.
- **Reading several files can mix states from different commits.** *Why:* reads are one file at
  a time.
  - A snapshot avoids this, but the filesystem has none (check `supports_snapshots()`). *Why:*
    other programs can edit files mid-read; a lock would only hold off tidings, not them.
- **`Error::Pending` means the commit succeeded.** On the filesystem, if a file can't be replaced
  (on Windows, another program has it open), the commit has still happened and is reported.
  - Until the file is released, *every* commit to that area fails, even ones not touching it.
    *Why:* this commit must be finished first to keep commits all-or-nothing.
- **An outside program's edit can be lost if it lands mid-commit.** A precondition is checked,
  then the commit's files are prepared and forced to disk, then renamed into place. An edit
  between the check and the rename is overwritten. Usually a window of milliseconds. *Why:* a
  rename replaces whatever is there, and no filesystem offers a portable "replace only if
  unchanged" or a lock other programs must respect.
  - After a `Pending` commit, or one a crash interrupted, the window lasts until it's finished.
  - Outside programs can also see a commit half-applied.
  - The reverse can happen too: if the edit swaps a directory for a symlink to one, the commit's
    files under it are dropped, though reported as written, and read until the commit finishes.
    *Why:* they're outside the area now, and finishing never writes through such a link, so a
    commit left unfinished doesn't block every later one.
- **The change feed doesn't give you every step.** It says which path changed, not the new
  contents, and merges unread changes to the same path. Re-read to see what's there.
- **Other processes' changes arrive late,** so until then your reads and the feed disagree.
  - SQLite: up to one poll interval (100 ms by default). *Why:* SQLite can't notify other
    processes of a commit, so each store checks. Each check reads a counter from memory SQLite
    shares between processes, not the disk.
  - Filesystem: once events have been quiet for 150 ms, so that a file an editor is saving isn't
    reported half-written. Longer if they keep coming.
  - On the filesystem, your own commit can be reported before another process's commit made just
    before it, and a commit can be split across batches if its files keep changing.
- **Some changes are reported that didn't happen.** On the filesystem, after the store opens,
  the first rewrite (or `touch`) of a file with unchanged contents reports a change. On macOS,
  so does the first change to its permissions. *Why:* avoiding it means reading every file when
  the store opens, and they can be large.

## Limitations

Each with why, where it isn't obvious.

### Paths

- **Paths follow the strictest platform's rules everywhere.** Even on Linux or SQLite you can't
  use `CON`, `aux.txt`, a trailing dot, or two paths differing only in case (`Themes/a` vs
  `themes/b`). *Why:* so a store works when moved to any platform.
- **On a case-insensitive filesystem (macOS, Windows), reading `foo` gives nothing when the file
  is `Foo`,** though the OS would happily open `Foo`. *Why:* so every backend agrees; on SQLite,
  memory and Linux, `foo` isn't `Foo`.
  - The cost: each read lists every directory on its path to check the exact name, so reading
    all N files in one directory is O(N²).
- **Files other programs make with invalid names are invisible** (left out of listings), yet can
  still block a commit (`FileUnderFile`) if they're where a file must go.
- **Two names differing only in case, made by another program on a case-sensitive filesystem,
  are both listed**, though tidings itself could never have made them.
- **Non-UTF-8 files are listed but give `Error::NotText` when read.**
- **Writes go through symlinks to files**, keeping the link. The target's directory must exist.
  A commit writing both a link and its target is refused (`SameFile`).
- **Symlinks to directories inside an area are ignored**, with everything under them: not
  listed, read or watched. Writing under one is refused (`DirectoryLink`). *Why:* so each file
  has one path, and every directory holding files is watched.
- **An area's directory can be a symlink, but it's resolved once, when the store opens.**
  Re-pointing it later takes effect at the next open.

### Performance

- **Stat reads the whole file, and a prefix revision reads every file under the prefix**, on the
  filesystem. *Why:* revisions come from contents, since modified times can be coarse or set by
  hand.
- **Watching uses memory per file** in each area, to tell what an event changed.
- **A long-held SQLite snapshot makes the database's log grow** until it is dropped. *Why:* that
  is how SQLite keeps the old state readable.

### Blocking store

- **It panics if called from async code** (a tokio task or `block_on`). *Why:* blocking there
  would stall the runtime. Use the async store, or `spawn_blocking`/`block_in_place`.
- **Each one starts its own tokio runtime and thread**, kept running so the feed keeps filling
  between calls. Opening many means many runtimes.

### Other

- Writing a file's current contents again is skipped: it keeps its modified time, and no change
  is reported.
- The change feed ends when every store clone is dropped, even if a snapshot is still held.
- A SQLite store more than 10 minutes behind other processes' commits (a stopped process, say)
  gets a resync. *Why:* the log of commits is pruned, so it stays small without tracking which
  stores are open.
- **Each area records which backend holds it,** in `.tidings/backend`, and a store on the other
  backend refuses to open it (`Error::WrongBackend`). `Store::detect` finds which one a location
  has. The first store to open an area marks it, taking over what is already there.
- Not supported: binary files; moving data between backends; size limits or eviction for the
  cache; your own backends; other programs writing to tidings' SQLite databases.
