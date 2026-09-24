# tidings

Text files for an application, in three areas (config, data, cache), stored on the filesystem, in
SQLite or in memory. Writes happen only through staged commits. Every change is reported on a
change feed.

Status: early. Only the in-memory Store, with plain writes and reads and checked paths, exists
so far. See [CONTEXT.md](CONTEXT.md) and [docs/adr](docs/adr).

## Consistency

- A commit is all-or-nothing, and only happens when you call `commit`. A staging you drop is
  discarded.
- Reads are one file at a time. Reading several files can mix states from different commits.
  Every path that changes afterwards appears on the change feed, so read it again when it does.
- For a consistent read of several files, use a snapshot. SQLite and memory support snapshots;
  the filesystem doesn't, because other programs can change files mid-read. Check
  `supports_snapshots()`.
- Every change after `open` returns is reported. Unread changes to the same path are merged into
  one. If changes may have been missed (watching failed, or an area's directory was removed), the
  feed sends a resync for that area instead: read it all again.
- Changes say which path changed, not what it now contains.
- A read always returns a whole file, never a partly written one.
- A commit can require that files, or everything under a prefix, are unchanged since you read
  them, and fails with a conflict otherwise. On the filesystem this holds against other tidings
  commits. A program outside tidings that writes a file while a commit is being applied can have
  its edit overwritten.

## Limitations

- Text only (UTF-8). No binary files.
- Paths follow the strictest platform's rules on every backend, so a path that works on one
  platform works on all of them. Names Windows reserves (such as `CON`, `aux.txt` or `COM¹`),
  control characters, a trailing dot or space, `.` and `..`, and paths not in Unicode NFC form are
  refused, as is anything under `.tidings/`.
- No moving a store's data from one backend to another.
- No size limit or eviction for the cache area.
- Backends are defined in this crate. You cannot plug in your own.
- On the filesystem, other programs can see a commit half-applied. Readers going through tidings
  can't.
