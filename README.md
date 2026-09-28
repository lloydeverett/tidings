# tidings

Text files for an application, in three areas (config, data, cache), stored on the filesystem, in
SQLite or in memory. Writes happen only through all-or-nothing commits, and every change is
reported on a change feed. Async on tokio; `tidings::blocking::Store` (feature `blocking`) for
synchronous code.

Status: first version, complete but early. See [CONTEXT.md](CONTEXT.md) and [docs/adr](docs/adr).

## Gotchas

Each entry: what may surprise you, then why it is that way.

### Commits and reads

- **A cancelled commit may still happen.** Dropping the `commit` future (e.g. on a timeout) lets
  a commit that had started finish in the background. *Why:* stopping halfway would leave it
  half-applied, and Rust can't wait for cleanup on drop.
- **…and may go unreported** if the tokio runtime shuts down while it finishes. *Why:* the
  feed runs on that runtime.
- **Reading several files can mix commits.** Use a snapshot to avoid it, but the filesystem has
  none (check `supports_snapshots()`). *Why:* other programs can edit files mid-read; a lock
  would only hold off tidings, not them.
- **A long-held SQLite snapshot makes the database's log grow** until it is dropped. *Why:* that
  is how SQLite keeps the old state readable.
- **Writing a file's current contents again does nothing:** the file keeps its old modified
  time, and no change is reported.
- **`Error::Pending` means the commit succeeded.** On the filesystem, if a file can't be replaced
  (on Windows, another program has it open), the commit has still happened and is reported.
  Until it can be finished, *every* commit to that area fails, even ones not touching that file.
  *Why:* the next commit must wait for this one to finish to keep commits all-or-nothing.
- **Programs outside tidings aren't bound by preconditions.** One that writes a file while a
  commit is applied can have its edit overwritten, and can see the commit half-applied. *Why:*
  there's no portable way to lock other programs out.
- **Stat reads the whole file, and a prefix revision reads every file under the prefix**, on the
  filesystem. *Why:* revisions come from contents, since modified times can be coarse or set by
  hand.

### The change feed

- **It says which path changed, not the new contents, and merges unread changes to one path.**
  You re-read to find out what is there.
- **It can send a resync for an area instead of changes:** read the whole area again. Happens if
  watching fails, the area's directory is removed (cache cleared), or on SQLite, if a store falls
  over 10 minutes behind other processes. *Why:* the alternative is silently missing changes.
- **Other processes' commits arrive late:** up to a 100 ms poll on SQLite, and after 150 ms of
  quiet on the filesystem. *Why:* SQLite has no cross-process notification; on the filesystem,
  waiting avoids reporting a file an editor is halfway through saving.
- **On the filesystem, order and batching are loose for others' changes.** Your own commit can
  be reported before another process's commit made just before it, and a commit can be split
  across batches if files keep changing. *Why:* those arrive as filesystem events, not from the
  store.
- **In the data and cache areas, the first rewrite of an unchanged file reports a change.**
  *Why:* the store would otherwise have to read every file there when it opens, and they can be
  large. (Config is read at open, so doesn't do this.)
- **Watching uses memory per file** in each area, to tell what an event changed.
- **Edits under a symlink to a directory aren't reported under the link's path**: only under the
  directory's own path, if it's in the area, and not at all if it's outside. Links to files are
  followed. *Why:* watching through directory links means following arbitrary trees and cycles.
- **The feed ends when every store clone is dropped**, even if a snapshot is still held.

### Paths

- **Paths follow the strictest platform's rules everywhere.** Even on Linux or SQLite you can't
  use `CON`, `aux.txt`, a trailing dot, or two paths differing only in case (`Themes/a` vs
  `themes/b`). *Why:* so a store works when moved to any platform.
- **On a case-insensitive filesystem (macOS, Windows), reading `foo` gives nothing when the file
  is `Foo`,** though the OS would happily open `Foo`. *Why:* so every backend agrees; on SQLite,
  memory and Linux, `foo` isn't `Foo`. The cost: each read lists every directory on its path to
  check the exact name, so reading all N files in one directory is O(N²).
- **Files other programs make with invalid names are invisible** (left out of listings), yet can
  still block a commit (`FileUnderFile`) if they're where a file must go.
- **Two names differing only in case, made by another program on a case-sensitive filesystem,
  are both listed**, though tidings itself could never have made them.
- **Non-UTF-8 files are listed but give `Error::NotText` when read.**
- **Writes go through symlinks to files**, keeping the link. The target's directory must exist.
  A commit writing both a link and its target is refused (`SameFile`).

### Blocking store

- **It panics if called from async code** (a tokio task or `block_on`). *Why:* blocking there
  would stall the runtime. Use the async store, or `spawn_blocking`/`block_in_place`.
- **Each one starts its own tokio runtime and thread**, kept running so the feed keeps filling
  between calls. Opening many means many runtimes.
- **A blocking commit can't be cancelled.**

## Not supported

Binary files; moving data between backends; size limits or eviction for the cache; your own
backends; other programs writing to tidings' SQLite databases.
