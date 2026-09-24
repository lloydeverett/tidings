---
status: accepted
---

# Snapshots are optional, and the filesystem Backend has none

Only some Backends can provide a Snapshot, so `snapshot()` returns `Error::Unsupported` on those
that cannot, and `supports_snapshots()` lets an app decide what to do when it starts. SQLite (a
read transaction in WAL mode) and memory support Snapshots. The filesystem does not.

The filesystem could have held a shared lock that keeps Commits out while the Snapshot is read.
We rejected this because it would be a lie: a person editing a File, or any other program, can
still change what the Snapshot returns while it is held. It would also make one process's reads
hold up every other process's writes.

## Considered options

- **Snapshots on every Backend**: rejected for the reasons above.
- **A generic `Store<B>` that only has `snapshot()` where the Backend supports it**: checked by
  the compiler, but it would make every type that holds a Store name its Backend.
