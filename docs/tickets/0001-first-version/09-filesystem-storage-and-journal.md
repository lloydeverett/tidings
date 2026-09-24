# 09: Filesystem backend: storage and journaled Commits

Spec: [0001](../../specs/0001-first-version.md) · ADRs: [0002](../../adr/0002-writes-only-through-all-or-nothing-commits.md), [0005](../../adr/0005-the-filesystem-journal.md), [0006](../../adr/0006-no-snapshots-on-the-filesystem.md)

**What to build:** An app can open a Store on the filesystem. Its Areas are real directories that
people can see and edit, in the standard places for the App identity or under a Root override.
Commits are all-or-nothing, using the journal described in ADR 0005. Writes to symlinked Files go
through the link. Snapshots are refused. The whole shared suite passes, except the Snapshot tests.

**Blocked by:** 04, 06

**Status:** ready-for-agent

- [ ] `Store::open_fs(app, FsOptions)` uses `etcetera`'s config, data and cache directories. The
      Root override in `FsOptions` replaces them and is documented as an override. Opening
      creates each Area root and its `.tidings/` directory.
- [ ] Reads, stat and list work on the directory tree. `.tidings/` and tidings' temporary files
      are left out of listings. A File that isn't valid UTF-8 gives `NotText { path }` when read,
      but is still listed.
- [ ] A Revision is a hash of the File's contents. A Prefix Revision reads and hashes every File
      under the Prefix.
- [ ] Commits follow ADR 0005:
      - an exclusive lock on `.tidings/lock`;
      - Preconditions checked first;
      - the journal written as `prepared`, then `committed`, each time with `atomic-write-file`;
      - temporary files next to their targets, forced to disk;
      - renames, then the journal removed.
- [ ] Opening a Store finishes a `committed` journal left behind, and discards a `prepared` one.
      This ticket covers the basic case; ticket 10 covers every interruption point.
- [ ] Each File's modification time on disk is set to the Commit's timestamp.
- [ ] Writing to a Path that is a symlink writes the File it points to, and leaves the link in
      place.
- [ ] `supports_snapshots()` is `false`, and `snapshot` returns `Unsupported`.
- [ ] Two Stores on the same Root override can commit concurrently. Commits are applied one at a
      time, and Preconditions hold between them.
- [ ] The shared suite passes on the filesystem.
- [ ] The Backend is compiled only with the `fs` feature.
