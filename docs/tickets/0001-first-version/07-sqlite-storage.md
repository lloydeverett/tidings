# 07: SQLite backend: storage

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** An app can open a Store on SQLite, with one database per Area in that Area's
standard directory for its App identity, or under a Root override. Everything the memory Backend
does, the SQLite Backend does too, and the whole shared suite passes on it, including Snapshots.

**Blocked by:** 04, 06

**Status:** done

- [x] `Store::open_sqlite(app, SqliteOptions)` places each Area's database in `etcetera`'s config,
      data and cache directories for the App identity. The Root override in `SqliteOptions`
      replaces those locations and is documented as an override.
- [x] Opening creates the databases. Uses `rusqlite` with SQLite built in (the `bundled`
      feature), in WAL mode, with blocking calls kept off the async runtime.
- [x] Letter-case clashes, and a Path that is a Prefix of another, are refused by the shared
      check (`Staged::refuse_clashing_paths`) inside the write transaction. A unique case-folded
      Path column alone misses clashes between Prefixes (`Themes/a` beside `themes/b`) and a
      File under a File (`a` beside `a/b`).
- [x] A stored case-folded Path depends on the Unicode tables in `caseless` and the standard
      library, so it can go stale after an upgrade. Recompute it (for example on open, when the
      Unicode version changed), or document that it can change.
- [x] Preconditions, including Prefix Revisions computed from stored Revisions, are checked
      inside the write transaction.
- [x] A Snapshot is a read transaction on a separate connection, and doesn't block Commits.
      `supports_snapshots()` is `true`.
- [x] The whole shared suite passes on SQLite.
- [x] The Backend is compiled only with the `sqlite` feature.

**Notes:**

- **Public API.** `Store::open_sqlite(&AppIdentity, SqliteOptions) -> Result<(Store, ChangeFeed)>`
  is async, since opening creates directories and databases, off the runtime.
  `AppIdentity::new(app_name, author, top_level_domain)` lives in `src/app.rs`, which belongs to no
  Backend, so the filesystem (ticket 09) can share it. `SqliteOptions` is `#[non_exhaustive]` with
  `Default` and a `root_override(root)` builder method, so ticket 08 can add its poll interval.
  `Error::Backend(Box<dyn Error + Send + Sync>)` is new, as the spec lists it. `app.rs` is compiled
  only with `sqlite` for now, since nothing else uses it: ticket 09 should widen its `cfg` (and
  those on `PerArea::try_from_fn` and `Area::name`) to `any(feature = "fs", feature = "sqlite")`,
  and make `fs` enable `etcetera` too.
- **Where the databases are.** `choose_app_strategy` from `etcetera` 0.11 gives the config, data and
  cache directories (XDG on Linux and macOS, `AppData` on Windows). Each Area's database is
  `<area>.sqlite3` in its directory, named by Area so that two Areas never share one even where two
  directories are the same. The Root override puts the Areas in `config/`, `data/` and `cache/`
  under the root, the layout ticket 09 should use too. Only the Root override is tested: the
  standard directories would put test files in the real home directory.
- **The shared Commit rules are one function, `CommitRequest::plan`** (src/backend/mod.rs). It runs,
  in order: `check_preconditions`, `expand_prefix_deletes`, `leave_out_what_changes_nothing` (writes
  with the stored Revision, and deletes of missing Paths), then `refuse_clashing_paths`. It reads
  the Area only through `AreaState`, and gives a `Plan`, which the Backend applies with one callback
  per changed Path (`Plan::apply` builds the `CommitOutcome`). So a Backend can't get the order
  wrong or leave a step out: memory and SQLite both just call `plan`, then `apply`, under their
  lock. Ticket 09 should call `plan` too, and write its journal from the `Plan`.
- **The clash check costs what a Commit writes, not what the Area holds** (the ticket 05 finding).
  `refuse_clashing_paths` now folds only the names the Commit writes, and asks the Area, through the
  new `AreaState::paths_named_like(name, fold)`, for the Paths with another name that folds the
  same. Folding keeps each `/` where it is (upper case and folding map single characters, and `/`
  starts a new NFD run), so those are the Paths whose own fold is `fold` or starts with `fold/`: one
  range of a fold index. SQLite indexes a `fold` column; memory keeps a `BTreeMap` from fold to Path
  beside each Area's Files, outside the `Arc` a Snapshot shares, so it is never copied.
  `expand_prefix_deletes` asks for the Paths under each Prefix (`AreaState::paths_under`) rather
  than walking the whole Area. The ticket 05 probe (48,000 Commits into an Area growing to 3,000
  Files) now takes 2.6 s on memory (debug build), down from 95 s. On SQLite (release build) it takes
  34 s, 0.7 ms a Commit, flat as the Area grows; nearly all of that is SQLite syncing each Commit to
  disk.
- **The stored fold.** The `meta` table records the Unicode versions the folds were made with: the
  standard library's (upper case), `caseless`'s and `unicode-normalization`'s, which all export
  their `UNICODE_VERSION`. Opening a database whose versions differ makes every fold again in the
  opening transaction. If two stored Paths now fold the same, both stay, as the Commit that wrote
  them allowed, but no Commit can add a name that clashes with either. The unit test
  `opening_folds_again_what_was_folded_with_other_unicode_data` in src/backend/sqlite.rs is the one
  test that edits a database directly: it makes every fold stale and changes the recorded versions,
  since the public API can't. It fails if opening doesn't fold again.
- **Schema.** `files (path TEXT PRIMARY KEY, fold, contents, modified_second, modified_nanosecond,
  revision BLOB)` with an index on `(fold, path)`, and `meta (name, value)`. An ordinary rowid
  table, since contents can be large. Migrations are a list of steps, applied by `PRAGMA
  user_version`; ticket 08 adds its change log as a new step at the end. A database from a later
  schema version is refused with `Backend`. `synchronous` is SQLite's default (`FULL`), so a Commit
  that returned survives a power cut, as the filesystem journal's does.
- **Connections.** Each Area has one connection for the Store's reads and Commits, behind a mutex,
  used on tokio's blocking threads through `spawn_blocking` (the `sqlite` feature turns on tokio's
  `rt`). A Commit is a `BEGIN IMMEDIATE` transaction, so its checks and writes happen under SQLite's
  write lock, which also keeps out other processes (ticket 08).
- **Snapshots.** A Snapshot opens a read-only connection, begins a transaction and reads at once (a
  plain `BEGIN` is deferred). With the read removed,
  `a_snapshot_reads_the_area_as_it_was_when_taken` fails on SQLite. It holds only its connection,
  not the Store's shared state, so it outlives the Store. In WAL mode it doesn't hold up Commits,
  but while it is held the write-ahead log can't be emptied past it: the README's Limitations say
  so.
- **Tests.** tests/behaviour/main.rs runs the whole shared suite on SQLite. The `Sqlite` Fixture
  holds a `tempfile` directory as the Root override; the macro makes a Fixture per test, so each
  test has its own. `sqlite_supports_snapshots` pins `supports_snapshots()`. tests/store_layer.rs
  also checks that the `open_sqlite` future is `Send`. Breaking the fold lookup's SQL fails
  `a_path_differing_only_in_letter_case_is_refused` and `a_file_cannot_be_under_another_file` on
  SQLite, and doing the same to memory's fails them on memory.
- **For ticket 10: cancelling a SQLite Commit.** A SQLite Commit runs on a blocking thread, so if
  its future is dropped once that has started, the Commit still happens but the Store layer never
  records its Changes. Memory couldn't be cancelled mid-Commit, so this is new. Ticket 10's "a
  Commit whose future is dropped once it has started runs to completion in the background. Its
  Changes still arrive on the feed" fixes it in the Store layer for every Backend. Until then the
  README's Status says so.
