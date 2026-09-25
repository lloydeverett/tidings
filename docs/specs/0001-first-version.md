# Spec 0001: the first version of tidings

Status: ready to build. Terms are used as defined in [CONTEXT.md](../../CONTEXT.md), and the
decisions in [docs/adr](../adr) apply throughout.

## Problem Statement

An application needs somewhere to keep its config, its data and its cache. Usually that means
writing files into platform directories directly, and every application runs into the same
problems:

- Writing several files that belong together is not all-or-nothing, so a crash can leave them
  inconsistent.
- If a person edits a config file while the application is running, the application doesn't
  notice. Or worse, it saves its own copy over the person's edit.
- Code written against the filesystem cannot switch to a single database file (or to memory, for
  tests) without being rewritten.
- A path that works on Linux can fail on Windows or macOS: reserved names, letter case, Unicode
  normalisation.
- Watching for changes is easy to get subtly wrong. Changes are missed between loading and
  subscribing, or dropped when the application falls behind, and nothing says so.
- Library APIs usually offer either async or sync, and the application needs both.

## Solution

tidings is a Rust crate that gives an application a Store with three Areas: Config, Data and
Cache. Each Area holds Files: text, with a last-modified timestamp. The Store hides whether they
live on the filesystem (in the platform's standard directories, so people can edit them), in one
SQLite database per Area, or in memory.

- **Writing.** Changes are staged in a Staging and committed all-or-nothing, on every Backend.
  Preconditions stop the application from overwriting changes it hasn't seen, whether to the Files
  it writes, other Files, or anything under a Prefix. On the filesystem, an edit by another program
  can still be overwritten if it lands in the moment a Commit is being applied.
- **Being told about changes.** Opening a Store also hands over its Change feed. That feed reports
  every Path that changes afterwards, whoever changed it. It merges changes rather than dropping
  them, and says so explicitly (a Resync) if it cannot keep that promise.
- **Consistent reads.** Snapshots give consistent multi-file reads where the Backend can honestly
  provide them.
- **Async and sync.** The API is async on tokio, and there is a blocking companion.

## User Stories

### Opening a Store

1. As an app developer, I want to open a Store by giving my App identity (app name, author,
   top-level domain), so that my files go in the platform's standard config, data and cache
   directories without me working out where those are.
2. As an app developer, I want to open a Store on the filesystem Backend, so that people using my
   app can see and edit its files with ordinary tools.
3. As an app developer, I want to open a Store on the SQLite Backend, so that each Area is a single
   file rather than a directory tree.
4. As an app developer, I want to open a Store in memory, so that my own tests run fast and never
   touch the disk.
5. As an app developer, I want each Backend to have its own constructor and options, so that I
   only see settings that apply to the Backend I chose.
6. As an app developer, I want a clearly labelled Root override, so that tests and unusual
   installations can put a Store somewhere other than the standard directories.
7. As an app developer, I want opening a Store to create its Area directories or databases, so
   that it is ready to watch and write straight away.
8. As an app developer, I want an interrupted Commit to be recovered while the Store opens, so that
   after a crash I never see a half-applied Commit.
9. As an app developer, I want the Change feed handed to me by the same call that opens the Store,
   so that no change between opening and listening can be missed.
10. As an app developer, I want the Store to be cheap to clone and safe to share between tasks, so
    that different parts of my app can use it without wrapping it themselves.

### Reading

11. As an app developer, I want to read a File by Area and Path and get its contents,
    last-modified time and Revision, so that I can use it and later write it back safely.
12. As an app developer, I want reading a missing File to give me "nothing there" rather than an
    error, so that "no config yet" is not an error case.
13. As an app developer, I want to stat a File without loading its contents, so that I can check
    its Revision or timestamp cheaply.
14. As an app developer, I want to list the Paths under a Prefix, so that I can discover what is
    stored, for example every file under `themes/`.
15. As an app developer, I want a File that is not valid UTF-8 to give an error naming that Path,
    but still appear in listings and Changes, so that a stray binary file is visible and not
    silently hidden.
16. As an app developer, I want every read to return a whole File, so that I never parse a
    half-written one.
17. As an app developer, I want a Snapshot of an Area on Backends that can provide one, so that I
    can read several Files that were committed together and know they belong together.
18. As an app developer, I want to ask whether the Store supports Snapshots, so that I can choose
    my approach when the app starts rather than when a read fails.
19. As an app developer, I want asking for a Snapshot on the filesystem Backend to fail as
    unsupported, so that I am never given a "snapshot" that other programs can change underneath
    me.

### Writing

20. As an app developer, I want to stage writes and deletes for one Area in a Staging and commit
    them together, so that Files that belong together change together.
21. As an app developer, I want a Staging to be an ordinary value I can build without holding on to
    the Store, so that I can build it anywhere and commit it when ready.
22. As an app developer, I want a Staging I drop without committing to write nothing, so that
    abandoning a change is always safe and nothing is ever committed on my behalf.
23. As an app developer, I want a Commit to apply all of its writes and deletes or none of them, on
    every Backend, so that my code is correct whichever Backend is underneath.
24. As an app developer, I want to delete everything under a Prefix in a Commit, so that I can
    remove a group of Files without listing them first.
25. As an app developer, I want to rename a File by deleting the old Path and writing the new one
    in the same Commit, so that a rename is all-or-nothing without a separate operation.
26. As an app developer, I want a write whose Precondition is *absent*, so that I create a File
    only if nobody else has.
27. As an app developer, I want writing back a File I read to require it to be *unchanged since*
    its Revision, so that I never overwrite an edit someone made in between.
28. As an app developer, I want a Commit that fails a Precondition to write nothing and tell me
    which Paths conflicted, so that I can re-read them and decide what to do.
29. As an app developer, I want deletes to take Preconditions too, so that I don't delete a File
    someone has just changed.
30. As an app developer, I want deleting a Path that doesn't exist to do nothing, so that cleanup
    code doesn't need to check first.
31. As an app developer, I want a Commit to require that a File it does not write is unchanged since
    I read it, so that a Commit I worked out from that File fails if the File changed in the
    meantime.
32. As an app developer, I want to get a Prefix Revision for everything under a Prefix, so that I
    can later check that nothing there has changed.
33. As an app developer, I want a Commit to require that everything under a Prefix is unchanged
    since a Prefix Revision, so that it fails if any File there was added, removed or changed,
    including Files I never saw.
34. As an app developer, I want these checks to be something I add to a Commit only when I need
    them, so that Commits that don't depend on other Files pay nothing for them.
35. As an app developer, I want getting a Prefix Revision to be separate from listing, so that
    listing stays cheap.
36. As an app developer, I want every File in a Commit to get the same last-modified time, so that
    Files that changed together are visibly from the same moment.
37. As an app developer, I want a write that doesn't change a File's contents to be left out of the
    Commit, so that re-saving identical settings neither changes the timestamp nor causes a
    Change. This means an app that writes in response to Changes can't end up in a loop.
38. As an app developer, I want a successful Commit to tell me the new Revisions of the Files it
    wrote, so that I can write them again safely without reading them first.
39. As an app developer, I want a Commit I cancel once it has started to either finish or not
    happen at all, so that cancelling (for example with a timeout) can never leave it half-done.
40. As an app developer, I want to be told when a Commit has been decided but could not yet be
    fully applied, so that I know it will be finished later. Meanwhile, reads through tidings
    show the committed contents.
41. As an app developer, I want two running copies of my app to be able to commit to the same Store
    safely, so that I don't need my own locking between processes.

### Paths

42. As an app developer, I want Paths to be checked when I use them, and a clear InvalidPath error
    if one is not allowed, so that mistakes show up immediately rather than on some other
    platform.
43. As an app developer, I want names that Windows cannot store (such as `CON`) to be refused on
    every Backend and platform, so that my data can be moved anywhere.
44. As an app developer, I want a Commit that would create two Paths differing only in letter case
    to be refused, so that the Store behaves the same on case-insensitive filesystems.
45. As an app developer, I want Paths not in Unicode NFC form to be refused, so that the Store
    behaves the same on macOS, which treats different forms of the same character as one name.
46. As an app developer, I want tidings' own `.tidings/` Prefix to be off limits, so that I cannot
    damage the Store's own bookkeeping by accident.

### The Change feed

47. As an app developer, I want a Change for every Path that changes after the Store is opened,
    naming the Area, the Path and whether it was changed or removed, so that I can reload exactly
    what I need.
48. As an app developer, I want each Change to say whether it came from this Store (local) or from
    elsewhere (external), so that I can skip reacting to my own Commits.
49. As an app developer, I want all the Changes from one Commit to arrive in the same batch, so that
    I can react to the Commit as a whole.
50. As an app developer, I want Changes I haven't read yet to be merged per Path rather than
    queued or dropped, so that falling behind never loses a Path and never uses unbounded memory.
51. As an app developer, I want a merged Change to count as external if any of the Changes merged
    into it were, so that skipping my own Changes never hides someone else's.
52. As an app developer, I want edits a person makes in a text editor to arrive as external
    Changes, so that my app can reload its config while it is running.
53. As an app developer, I want the bursts of events an editor produces when saving to be held
    briefly and merged (with a configurable window), so that I don't reload a half-saved File.
54. As an app developer, I want external events that did not change a File's contents to be
    dropped, so that a Change always means the contents really changed.
55. As an app developer, I want Files deleted individually from the Area to arrive as Removed
    Changes, so that clearing files is not an error.
56. As an app developer, I want a Resync for an Area when watching fails, so that I am told
    explicitly that I may have missed Changes and should read that Area again.
57. As an app developer, I want a Resync when an Area's directory disappears (for example the
    cache is cleared), with the directory recreated, so that my app recovers without restarting.
58. As an app developer, I want the Change feed to end once every Store handle is dropped, so that
    the task listening to it finishes on its own.
59. As an app developer, I want the Store to keep working if I drop the Change feed, so that
    ignoring Changes is my choice and not a failure.

### Sync code

60. As an app developer writing synchronous code, I want a blocking Store with the same behaviour
    as the async one, so that I can use tidings without an async runtime of my own.
61. As an app developer writing synchronous code, I want to wait for the next item on the Change
    feed in a blocking way, so that a plain thread can process Changes.
62. As an app developer, I want a clear panic if I call the blocking API from inside an async
    runtime, so that I find the mistake immediately rather than as a deadlock.

### People using an app built on tidings

63. As a person using an app built on tidings, I want to edit its config file in my editor while
    it runs and have it pick up my edit, so that I don't have to restart it.
64. As a person using an app built on tidings, I want the app never to silently overwrite an edit I
    just saved, so that my changes aren't lost. (On the filesystem there is one exception: an edit
    saved in the moment a Commit is being applied.)
65. As a person who symlinks config files in from a dotfiles repo, I want the app's writes to go
    through the link and leave it a link, so that my dotfiles setup keeps working.
66. As a person using an app built on tidings, I want clearing its cache directory to be safe while
    it runs, so that I can reclaim disk space without breaking it.

### Maintainers

67. As a maintainer, I want one suite of behaviour tests that every Backend must pass, so that
    "same guarantees on every Backend" is checked, not just promised.
68. As a maintainer, I want named failure points in the filesystem Backend in test builds, so that
    I can prove recovery works after an interruption at every step of a Commit.
69. As a maintainer, I want journal recovery, dropped events and watcher errors logged through
    `tracing` at debug level, so that problems can be diagnosed without the library printing
    anything by default.
70. As an app developer, I want each Backend and the blocking API behind Cargo features, so that I
    only compile what I use.

## Implementation Decisions

### How the crate is split into modules

- **Path**: a public type for a Path that is known to be valid. It can only be created by
  validating a string:
  - Structure is checked with `relative-path`: relative, `/`-separated, UTF-8, no empty, `.` or
    `..` segments, no leading `/`.
  - Each segment is checked with `sanitize-filename` using its Windows rules.
  - NFC form is checked with `unicode-normalization`.
  - Anything under the reserved `.tidings` Prefix is refused, and so is any segment named like the
    filesystem Backend's temporary files (`.<name>.tidings-<commit-id>-<n>`, ADR 0005).
  - A Prefix is checked the same way.
  - Refusing Paths that differ only in letter case needs the Area's contents, so it happens at
    Commit time (see Backend). The same goes for the Prefixes they are under, and for a Path that
    is also a Prefix of another Path (`a` beside `a/b`). Case folding uses an established Unicode
    case-folding crate: `caseless`, chosen during implementation (ticket 04).
- **Area**: a closed enum of Config, Data and Cache.
- **Store (async)**: the public entry point. It is a cheap, cloneable handle to shared state.
  - Constructors, one per Backend: `open_fs(app, FsOptions)`, `open_sqlite(app, SqliteOptions)`
    and `open_memory()`. The filesystem and SQLite options carry the optional Root override. Each
    returns the Store together with its Change feed.
  - Operations: `read`, `stat`, `list`, `stat_prefix`, `snapshot`, `supports_snapshots` and
    `commit`. `stat_prefix` returns the Prefix Revision for a Prefix. It is separate from `list`
    because on the filesystem it has to read every File under the Prefix.
  - The Store layer handles everything that is the same for all Backends:
    - Validating Paths.
    - Choosing each Commit's timestamp (one `jiff::Timestamp` for the whole Commit).
    - Merging Changes that haven't been read yet.
    - Tagging the Changes of its own Commits with their Origin (see below).
    - Running the task that records what a Backend observes of other Stores and programs (the
      SQLite poller, the filesystem watcher), in turn with its own Commits, and sending a Resync
      for every Area if that task panics or stops.
    - Letting a cancelled Commit finish: once started, a Commit continues in a background task.
- **Staging**: an owned value for one Area, built independently of the Store. It holds:
  - writes: a Path, the new contents, and a Precondition that defaults to *any*;
  - a convenience for writing back a File that was read, which carries its Revision as
    *unchanged since*;
  - deletes: a Path and a Precondition;
  - deletes of a Prefix, which are expanded when the Commit is made;
  - `require(path, precondition)`: a Precondition on a File that the Staging does not write;
  - `require_prefix(prefix, prefix_revision)`: a Precondition that everything under a Prefix is
    unchanged since that Prefix Revision.

  Every Precondition is optional. A Staging with none of them depends on nothing but the Paths it
  writes and deletes.

  The Staging cannot be read from. It is consumed by `commit`.
- **Commit result**: on success, the Commit's timestamp and the new Revision of each Path written.
  On failure:
  - `Conflict { paths }`: nothing was written. For a failed Prefix Precondition, the paths are the
    ones under the Prefix that were added, removed or changed.
  - `Pending`: decided but not fully applied.
  - `InvalidPath`: includes letter-case clashes, and a File under another File (`a` beside
    `a/b`). On the filesystem, also something on disk that isn't a Path standing where a File or
    its directory must go (`FileUnderFile`), and a write of a Path that another written or deleted
    Path is the same file on disk as, through a symlink (`SameFile`), both refused before anything
    is written (ADR 0005).
  - `Backend`.
- **Revision**: opaque to the app. On every Backend it is a hash of the File's contents, using a
  fast, established 128-bit hash chosen during implementation. The same hash detects writes and
  external events that don't change anything, on every Backend. *Unchanged since* therefore means
  "same contents", so a File changed and then changed back counts as unchanged. That is acceptable
  because Preconditions exist to protect contents.
- **Prefix Revision**: opaque to the app. It is a hash over the sorted list of each Path under the
  Prefix together with its Revision. Adding, removing or changing any File under the Prefix changes
  it. The empty Prefix covers the whole Area.
- **Snapshot**: covers one Area, with `read`, `stat` and `list`. SQLite and memory provide it. The
  filesystem returns `Unsupported` (ADR 0006).
- **Change feed**: a single consumer that cannot be cloned. Its async `next` returns either a batch
  of Changes or a Resync for an Area, and returns nothing once every Store handle has been
  dropped.
  - A Change is an Area, a Path, a kind (*changed* or *removed*) and an Origin.
  - Changes that haven't been read are merged per (Area, Path): the latest kind wins, and the
    Origin is external if any merged Change was external.
  - A Commit's Changes are never split across batches. (On the filesystem, another Store's
    Commit sometimes can be: see Watching, below.)
  - If the feed is dropped, the Store stops recording Changes.
- **Errors**: one `#[non_exhaustive]` error type with `InvalidPath`, `NotText { path }`,
  `Conflict { paths }`, `Pending`, `Unsupported` and `Backend` (wrapping the underlying error). A
  missing File is `Ok(None)`, not an error. The blocking Change feed's `next_timeout` gives a type
  of its own, `blocking::TimedOut`, when nothing arrived in time (ticket 12). That isn't an
  operation's error: nothing went wrong, and the app chose how long to wait, as with `std`'s
  `recv_timeout` or tokio's `timeout`. As a variant of `Error`, every `match` on the errors of
  reads and Commits would have to handle a case none of them can give.
- **Blocking**: behind the `blocking` feature, a `blocking::Store` that runs its own internal tokio
  runtime (as `reqwest::blocking` does), mirroring every operation. It comes with a blocking way to
  wait for the next item on the Change feed. It panics if it would block where that stalls an
  async runtime (see below).

  Settled while building it (ticket 12):
  - The runtime is multi-threaded with one worker, so the tasks that follow other Stores' Commits
    and watch the Areas keep running between calls, not only while a call waits.
  - The Store's clones, its blocking Snapshots and its blocking Change feed share the runtime (an
    `Arc`), and the last of them to go shuts it down. A Snapshot holds the runtime, not the Store,
    so the feed still ends once every Store handle has gone. A Commit can't be cancelled through
    the blocking API, and a call in progress holds its handle, so the runtime runs until every
    Commit has finished and been reported.
  - The blocking Change feed is an `Iterator` that waits for each item and ends as the async one
    does, and `next_timeout` waits for a while only, giving `TimedOut` if nothing arrived.
  - Every method that waits panics where blocking would stall a tokio runtime or deadlock it: in
    an async task, or in a runtime's `block_on`. Where tokio allows blocking, it works: in
    `spawn_blocking` or `block_in_place`, which is how async code calls blocking code. Only tokio
    can tell those apart, and its public API tells only by refusing to block, so the Store asks it
    first, blocking on a future that does nothing, and panics with its own message, naming the
    call, if tokio refuses. Built with `panic = "abort"`, tokio's refusal ends the process first,
    with tokio's message, located in tokio. The methods that don't wait (`open_memory`,
    `supports_snapshots`, `inject_external_change`) work anywhere. Dropping is allowed anywhere: dropped in a runtime's context, the last handle lets
    the runtime's threads finish in the background, since tokio may not allow waiting for them
    there.

### What every Backend must provide (internal)

Backends are private to the crate (not a public trait). Each Backend provides:

- reading a File with its Revision;
- stat;
- listing by Prefix;
- computing a Prefix Revision;
- whether it supports Snapshots, and opening one;
- committing a list of writes and deletes all-or-nothing. While holding its lock, it:
  - checks every Precondition: those on writes and deletes, those on Files it doesn't write, and
    those on Prefixes;
  - drops writes that would not change the contents;
  - refuses Paths that differ only in letter case from an existing Path, or are under a Prefix
    that does, and Paths that are also a Prefix of another Path. The check is shared by every
    Backend and runs under its lock;
  - applies the rest;
  - returns the Revisions;
- what it observes of other Stores and programs: Changes, each group with its Origin, or that
  Changes to an Area were missed (watching failed, an Area root vanished, or the change log was
  pruned past the Store).

A Change's Origin is decided where the knowledge is. The Store layer tags the Changes of its own
Commits *local*. A Backend that reads other Stores' Commits from a change log (SQLite) knows which
Store instance wrote each one, and gives its Origin with it. The filesystem's watcher can't tell
who made what it sees, so it matches it against what the Change feed was last told of each File,
which the Store's own Commits update as they are reported (decided in ticket 11): an event that
finds a File as reported, as the events of the Store's own Commits do, gives nothing, and every
other difference is *external*.

### Filesystem Backend

- **Where Areas live.** The Area roots are `etcetera`'s config, data and cache directories for the
  App identity, or the Root override. Opening creates each root and its `.tidings/` directory.
- **Commits.** They follow ADR 0005:
  - an exclusive `std::fs::File::lock` on `.tidings/lock`;
  - a two-state (`prepared` / `committed`) journal written with `atomic-write-file`;
  - temporary files next to each target, after resolving symlinks, named
    `.<name>.tidings-<commit-id>-<n>`, a name no Path can have;
  - renames, then removing the journal;
  - recovery during `open`.
- **When renames fail.** They are retried briefly, then `Pending` is returned, with the Commit's
  Changes recorded. While a committed journal still exists, reads of its Paths come from its
  temporary files, and the next Commit or `open` finishes it. A next Commit that can't
  finish it isn't made, and gives `Backend`; `open` still opens (ADR 0005).
- **Preconditions.** The checks read and hash contents from disk, so edits by other programs
  count as changes. A Prefix Revision means reading and hashing every File under the Prefix, both
  when `stat_prefix` is called and when the Commit checks it. That is cheap for a config directory
  and can be expensive for a large cache. The checks run first, as in ADR 0005, before the
  temporary files are written. The lock only keeps out other tidings Commits. A program outside
  tidings that writes a File after the check and before the renames finish is not detected, and
  its edit is overwritten if the Commit writes that File. Ordinary filesystems have no way to
  replace a File only if it is unchanged, so this window cannot be closed. It is documented, not
  worked around.
- **Timestamps.** A File's last-modified time is its modification time on disk. A Commit sets it to
  the Commit's timestamp using the standard library's `set_modified`. Edits made outside tidings
  report whatever modification time they left.
- **Watching.** Uses `notify` with `notify-debouncer-full`. The window defaults to about 150 ms and
  is set in `FsOptions`.
  - Symlinked Files are followed, and their targets are watched too.
  - `.tidings/` and temporary-file names are ignored.
  - If an Area root disappears, the root is recreated, watched again, and a Resync is sent.
  - If watching fails, a Resync is sent.

  Settled while building it (ticket 11; the watcher's module doc has the detail):
  - For each name that events settled for, the watcher reads the File as reads through tidings
    see it, and compares it with what the Change feed was last told of it. So edits that don't
    change the contents, and the Store's own Commits, give nothing. It reads without the Store's
    turn with Commits, then takes the turn only to read again the Files the Store's Commits
    changed meanwhile, compare, and record, so that a flood of external Files doesn't hold up
    Commits.
  - What was reported is kept for every File in each Area, listed when the Store opens:
    memory in proportion to the number of Files. Config's Files are read then; in Data and Cache,
    which can be large, a File's Revision is known once it changes. So rewriting a File there
    that hasn't changed since the Store opened with the same contents reports a Change. Events
    that can't be writes (a File read, touched, or its permissions changed) are dropped before
    that.
  - A Commit left `Pending` is reported once, by the Store that made it, and by other Stores once
    its journal has settled, since reads show it finished. Its renames landing later give
    nothing.
  - Another Store's Commit is usually one batch, and one slower than the window is waited for and
    kept whole, but one can be split when events keep coming for longer than four windows. And
    the Store's own next Commit can reach its feed before another Store's Commits made just
    before it. So on the filesystem, "a Commit's Changes are never split across batches" holds
    for the Store's own Commits, as ADR 0003 says. SQLite, which reads other Stores' Commits from
    its log, keeps both promises for them too.
  - Every link in a chain of symlinks to a File is watched, not only the file at the end.
    Directories are watched under their own names only, not through symlinks to directories.
  - After a Resync for lost events or a failure, the Area is watched again from its root, since
    the platform's watcher may have lost watches. An Area that can't be watched, when the Store
    opens or later, gets a Resync, is tried again with a growing wait, and gets another Resync
    once it is watched: the Store opens anyway.

### SQLite Backend

- **Databases.** One database per Area, in that Area's standard directory (or under the Root
  override). It uses `rusqlite` with SQLite built in (the `bundled` feature), called through
  `spawn_blocking`, in WAL mode.
- **Tables.** The Files table holds the Path, the case-folded Path, the contents, the
  last-modified time and the Revision. A unique case-folded Path column alone doesn't enforce the
  letter-case rule, which also covers the Prefixes a Path is under and a Path that is a Prefix of
  another, so the shared check runs inside the write transaction too.
- **Detecting other processes' Commits.** Each Commit also appends to a change log table, in the
  same transaction, recording the Paths, the kind and which Store instance wrote it. Commits by
  other processes are noticed by polling SQLite's `data_version` or watching the database files
  (whichever works out better in practice), then reading the log since the last entry seen. The
  poll interval is in `SqliteOptions`. The log is pruned.
- **Snapshots.** A Snapshot is a read transaction on a separate connection.
- **Preconditions.** Prefix Revisions are computed from the stored Revisions, with no need to read
  contents. The checks happen inside the write transaction, so they are exact.
- The database is written only by tidings. Other programs writing to it directly are not
  supported.

### Memory Backend

- Always compiled in. All Areas live in process memory, shared by every clone of the Store.
- Snapshots are a cheap, fixed copy of the Area.
- There are no external Changes. Every Change is local.

### Crate setup

- Cargo features:
  - `fs` and `sqlite`, both on by default;
  - `blocking`;
  - `testing`, which enables the failure points, injecting external Changes, a short retention
    for the SQLite change log (so that a test can see a Store miss Commits) and shared test
    helpers. tidings' own tests always build with it.
- Edition 2024, `rust-version` 1.94, Apache-2.0.
- Main dependencies: tokio, jiff, etcetera, relative-path, sanitize-filename,
  unicode-normalization, rusqlite, notify, notify-debouncer-full, atomic-write-file and tracing.
- Logging is `tracing` at debug level only.

## Testing Decisions

- **What makes a good test.** A test uses only the public API and checks behaviour an app could
  observe: what a read returns, what a Commit does, what arrives on the Change feed. It never
  looks at journal files, tables or internal state. There are two exceptions, both only with the
  `testing` feature: starting a failure point, and injecting an external Change into the Store
  layer (see "memory" below). A third is narrower still: one unit test inside the SQLite Backend
  makes the stored letter-case folds stale, as new Unicode data would, and then checks through
  the public API that opening the Store folds them again. No public API can make a fold stale. A
  fourth: filesystem tests may check that no tidings temporary files are left in the Area
  directory, since a person sees them there.
- **Main seam: the public Store API, run on every Backend.** One behaviour suite, written once, is
  instantiated for the filesystem, SQLite and memory Backends and for the blocking Store. This is
  the same approach as OpenDAL's behaviour tests, which run one suite against every service.
  - The suite's tests are async, and use a test-only Store, Snapshot and Change feed with the async
    API's methods, which call either API. Each Backend runs the suite through both: through the
    blocking one, each call is made in `block_in_place`, as async code calls blocking code (on a
    plain thread only on a current-thread runtime, which can't), and handles are dropped where the
    test drops them.
  - Each test opens a Store on its own temporary Root override.
  - Tests use a short debounce window, and wait on the Change feed with timeouts.
- **Simulating changes from outside tidings:**
  - filesystem: write to, delete from, or remove the Area directory directly;
  - SQLite and filesystem: open a second Store on the same Root override, which stands in for
    another process;
  - memory: nothing outside tidings can reach it. To test how the Store layer merges external
    Changes, the `testing` feature's `Store::inject_external_change` records one on the Change
    feed without changing any File.
- **Second seam: named failure points in the filesystem Backend.** These exist only with the
  `testing` feature, using the `fail` crate or a small equivalent. The points are:
  - after the `prepared` journal is written;
  - after each temporary file is written;
  - after the `committed` journal is written;
  - after each rename;
  - rename fails (standing in for Windows' "file in use");
  - a pause point, which holds a Commit until the test releases it, for testing cancellation;
  - the watcher fails.

  A crash test stops a Commit at a point, drops the Store without any cleanup, opens it again, and
  checks through the public API that the Commit is fully there or fully absent. Loss of data not
  yet written to disk (power failure) is not simulated.
- **What gets covered:**
  - Path validation, as a table of accepted and refused strings.
  - Reading, stat and listing.
  - Commits being all-or-nothing, and each Precondition. That includes Preconditions on Files a
    Commit doesn't write, and a Prefix Precondition failing when a File is added under the Prefix
    after its Prefix Revision was taken.
  - Writes that change nothing.
  - Letter-case clashes.
  - Cancelling a Commit.
  - Two Stores committing concurrently.
  - Snapshots, and that they are refused on the filesystem.
  - Every Change feed guarantee: batching, merging, Origin, Resync, and the feed ending.
  - Following symlinks (the link stays a link after a write).
  - Recovery at every failure point.
- **Platforms.** The suite should run on Linux, macOS and Windows. Letter case, Unicode
  normalisation and rename behaviour are exactly what differs between them.
- **Prior art in this repo:** none yet. This is the first code.

## Out of Scope

- Binary contents.
- Moving a Store's data from one Backend to another.
- Size limits or eviction for the Cache.
- Backends written outside this crate, and a public Backend trait.
- Snapshots on the filesystem Backend.
- Commits that span more than one Area.
- Reads that mix Areas, or Snapshots across Areas.
- A starting snapshot handed over by `open`, and Changes that carry contents.
- Other programs writing to the SQLite database directly.
- Protecting programs outside tidings from seeing a filesystem Commit half-applied.
- Closing the window in which an edit by another program can be overwritten during a filesystem
  Commit. Only Linux-specific system calls could narrow it further.
- Caching hashes (as git does) to make filesystem Prefix Revisions cheaper.
- Simulating power loss in tests.

## Further Notes

- **Decided while writing this spec**, not discussed explicitly. Please review:
  1. A successful Commit returns the new Revisions.
  2. The Revision is a content hash on every Backend, not only the filesystem.
  3. A merged Change counts as external if any part of it was.
  4. On the filesystem, a Commit sets each File's modification time to the Commit's timestamp.
  5. The SQLite Backend uses a change log table to detect other processes' Commits.
  6. A Conflict from a Prefix Precondition lists the Paths under the Prefix that differ.
- The suggested build order starts with the parts that involve no disk: Path validation and the
  memory Backend. Next come the Store layer and the Change feed, then SQLite, then the filesystem
  Backend with its journal and failure points, and finally the blocking wrapper.
- The README's Consistency and Limitations sections are the short, user-facing version of this
  spec. Keep them in sync.
