# 09: Filesystem backend: storage and journaled Commits

Spec: [0001](../../specs/0001-first-version.md) · ADRs: [0002](../../adr/0002-writes-only-through-all-or-nothing-commits.md), [0005](../../adr/0005-the-filesystem-journal.md), [0006](../../adr/0006-no-snapshots-on-the-filesystem.md)

**What to build:** An app can open a Store on the filesystem. Its Areas are real directories that
people can see and edit, in the standard places for the App identity or under a Root override.
Commits are all-or-nothing, using the journal described in ADR 0005. Writes to symlinked Files go
through the link. Snapshots are refused. The whole shared suite passes, except the Snapshot tests.

**Blocked by:** 04, 06

**Status:** done

- [x] `Store::open_fs(app, FsOptions)` uses `etcetera`'s config, data and cache directories. The
      Root override in `FsOptions` replaces them and is documented as an override. Opening
      creates each Area root and its `.tidings/` directory.
- [x] Reads, stat and list work on the directory tree. `.tidings/` and tidings' temporary files
      are left out of listings. A File that isn't valid UTF-8 gives `NotText { path }` when read,
      but is still listed.
- [x] A Revision is a hash of the File's contents. A Prefix Revision reads and hashes every File
      under the Prefix.
- [x] Commits follow ADR 0005:
      - an exclusive lock on `.tidings/lock`;
      - Preconditions checked first;
      - the journal written as `prepared`, then `committed`, each time with `atomic-write-file`;
      - temporary files next to their targets, forced to disk;
      - renames, then the journal removed.
- [x] The journal applies moves between a File and a Prefix of the same name, which the shared
      suite makes (`a_file_cannot_be_under_another_file`): the file `a` is removed before the
      directory `a/` is created for `a/b`, and the emptied directory `d/` is removed before a
      File is renamed onto `d`.
- [x] Opening a Store finishes a `committed` journal left behind, and discards a `prepared` one.
      This ticket covers the basic case; ticket 10 covers every interruption point.
- [x] Each File's modification time on disk is set to the Commit's timestamp.
- [x] Writing to a Path that is a symlink writes the File it points to, and leaves the link in
      place.
- [x] `supports_snapshots()` is `false`, and `snapshot` returns `Unsupported`.
- [x] Two Stores on the same Root override can commit concurrently. Commits are applied one at a
      time, and Preconditions hold between them.
- [x] The shared suite passes on the filesystem.
- [x] The Backend is compiled only with the `fs` feature.

**Notes:**

- **Public API.** `Store::open_fs(&AppIdentity, FsOptions) -> Result<(Store, ChangeFeed)>` is
  async, and panics outside a tokio runtime, like `open_sqlite`. `FsOptions` is `#[non_exhaustive]`
  with `Default` and `root_override(root)`; ticket 11 adds its debounce window. `Error::NotText {
  path }` is new. With `testing` only, `FsOptions::fail_at(FailurePoint)` and the
  `#[non_exhaustive]` `FailurePoint` enum (see below). `AppIdentity`, `Area::name`,
  `PerArea::try_from_fn`/`iter` and `Error::backend` are now compiled with `fs` or `sqlite`, and
  `fs` enables `etcetera` and `atomic-write-file` (0.3.1). `off_runtime` moved from sqlite.rs to
  backend/mod.rs, shared by both.
- **Layout.** src/backend/fs.rs holds the Backend, reading, the on-disk `AreaState` and the
  Commit's steps; src/backend/fs/journal.rs holds the journal: its text format, writing it with
  `atomic-write-file`, discarding, finishing and `recover`. Areas are under the Root override as
  `config/`, `data/` and `cache/`, as on SQLite.
- **Commits share the Commit rules.** Under the lock, a Commit recovers any journal left behind,
  calls `CommitRequest::plan` with the Area read from disk, and gathers the `Plan`'s changes
  through `Plan::apply` into the journal before touching anything. Only then does it write the
  journal and temporary files and finish. `paths_named_like` lists only the directories along the
  name, one segment at a time, keeping the entries whose fold matches, so the letter-case and
  File-under-File check costs what the Commit writes.
- **The journal records** each temporary file with the Path it replaces, or, through a symlink,
  the absolute file the link points to, and each Path deleted with its Revision (added after the
  review). The directories to make and remove follow from those, so they aren't recorded. A crash
  while finishing is finished again: a delete is made only if the File is still there under
  exactly its name with that Revision, a rename skips a temporary file that is gone, and emptied
  directories are removed as far up as they are empty. See the review's Resolution for the
  same-file cases that made this necessary.
  Paths on disk must be valid Unicode to be journaled; a symlink into a directory whose name isn't
  gives `Backend` rather than being written.
- **File-under-File moves (the new checkbox).** Finishing makes the deletes first, removing each
  directory they leave empty, then makes each write's directory and renames. A temporary file
  whose directory doesn't exist yet goes in the nearest existing directory above it, so `a` to
  `a/b` works. Two temporary files can then share a directory, so the name has the write's number:
  `.<name>.tidings-<commit-id>-<n>`. ADR 0005 and the spec are updated.
- **What is in the way on disk.** Something that isn't a Path can have the name a Commit needs:
  an empty directory, one holding only names that aren't Paths, or a symlink to nothing where a
  directory must go. The shared check can't see these, and hitting one after the journal is
  committed would leave a Commit nobody can finish. So the Commit checks each write before
  anything is written: a directory holding nothing but Files the Commit deletes (or nothing) makes
  way and is removed; anything else refuses the Commit with `InvalidPath`/`FileUnderFile`.
- **Reserved names (the ticket 02 note).** Temporary-file names are now reserved in Path
  validation, on every Backend: any segment shaped `.<name>.tidings-<32 hex digits>-<digits>` gives
  `InvalidPathReason::Reserved`. So a File can never be taken for a temporary file, and listings
  skip temporary files simply by skipping names that aren't Paths. tests/paths.rs has the rows,
  with near misses that stay accepted.
- **Names on disk that aren't Paths** (non-NFC, non-UTF-8, Windows-reserved, `.tidings/`, temporary
  files, and whole directories with such names) are left out of listings, Prefix Revisions and
  Prefix deletes, so Preconditions don't see them either. Two names differing only in letter case,
  made by another program on a case-sensitive filesystem, are both listed, as SQLite keeps two
  Paths whose folds became equal: a Commit can't add a third. A File that isn't UTF-8 is listed,
  hashed for Revisions from its bytes (`Revision::of_bytes`), and gives `NotText` from `read` but
  not from `stat`. Reading checks the file's metadata before opening it, so a FIFO is never opened.
- **Symlinks.** A write follows the chain of links on the final segment (up to 40) and writes the
  temporary file next to the final target, so the links stay. The target's directory must exist
  (after the review): no directory is ever made outside the Area. A delete removes the link itself.
  Reads and listings follow links, to Files and directories; listing skips a link back to a
  directory it is already in.
- **Timestamps.** Each temporary file gets the Commit's timestamp with `File::set_modified` before
  it is forced to disk, so the rename carries it. A replaced File's permissions are copied too.
  Reads report the on-disk mtime; on Linux (nanoseconds) the shared suite's exact comparison holds.
- **Durability.** Temporary files and both journal states are forced to disk, as the ADR says.
  After finishing, each directory changed is forced to disk too (Unix), so the Commit is on disk
  before the journal goes. Removing the journal isn't: finishing again is harmless, and the next
  journal replaces a stale one first. A Commit that only deletes writes its journal once, as
  `committed`, since it has no temporary files. On this WSL disk an fsync costs about 1.7 ms, so a
  Commit costs about 5–10 ms: the suite's 500-round `concurrent_commits_reach_the_feed...` takes
  about 60 s on the filesystem, against 17 s on SQLite.
- **Recovery.** `open` locks each Area and recovers its journal; so does every Commit, first, since
  another process's Commit may have been interrupted while this Store was open. Recovery is
  logged at debug level. If a step after `committed` fails, the Commit gives `Backend` and leaves
  the journal for the next Commit or `open` (ticket 10 turns this into `Pending`). If writing a
  temporary file fails, the Commit is discarded at once.
- **Failure points.** Recovery needs a crash to test, so a small per-Store equivalent of the `fail`
  crate is here, with the points recovery needs: `AfterPreparedJournal`, `AfterTemporaryFile(n)`
  and `AfterCommittedJournal`, and after the review `AfterDeletes` and `AfterRename(n)`, all only
  with `testing`. `FsOptions::fail_at(point)` makes every
  Commit through that Store stop there, as if the process had died: it gives `Backend` and leaves
  everything on disk. Being per Store, it works with tests running in parallel, which the `fail`
  crate's process-wide configuration wouldn't. Ticket 10 adds the rest (after each rename, a
  failing rename, the pause point) as more variants.
- **Tests.** tests/behaviour/main.rs runs the whole shared suite on the filesystem, plus
  `fs_does_not_support_snapshots` and filesystem tests of: the directories people see, Files
  other programs make, names that aren't Paths, a non-UTF-8 File, the mtime on disk, writing
  through a symlink (Unix), what is in the way on disk, and recovery (a Commit stopped before
  `committed` is discarded with no temporary files left; one stopped after is finished by `open`,
  including both moves, and by another Store's next Commit). `two_stores_suite!` is split into
  `committing:` and `seeing_each_other:`; the filesystem runs `committing:` (concurrent Commits
  with exact Preconditions), and ticket 11 enables the rest. Each new test was checked to fail with
  its behaviour removed (link following, `set_modified`, discard, finish, recovery,
  name validation in listings, the emptied-directory removal, the nearest-directory fallback, the
  on-disk fold lookup, the in-the-way check, removing an empty directory).
