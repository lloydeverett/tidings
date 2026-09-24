# 07: SQLite backend: storage

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** An app can open a Store on SQLite, with one database per Area in that Area's
standard directory for its App identity, or under a Root override. Everything the memory Backend
does, the SQLite Backend does too, and the whole shared suite passes on it, including Snapshots.

**Blocked by:** 04, 06

**Status:** ready-for-agent

- [ ] `Store::open_sqlite(app, SqliteOptions)` places each Area's database in `etcetera`'s config,
      data and cache directories for the App identity. The Root override in `SqliteOptions`
      replaces those locations and is documented as an override.
- [ ] Opening creates the databases. Uses `rusqlite` with SQLite built in (the `bundled`
      feature), in WAL mode, with blocking calls kept off the async runtime.
- [ ] Letter-case clashes, and a Path that is a Prefix of another, are refused by the shared
      check (`Staged::refuse_clashing_paths`) inside the write transaction. A unique case-folded
      Path column alone misses clashes between Prefixes (`Themes/a` beside `themes/b`) and a
      File under a File (`a` beside `a/b`).
- [ ] A stored case-folded Path depends on the Unicode tables in `caseless` and the standard
      library, so it can go stale after an upgrade. Recompute it (for example on open, when the
      Unicode version changed), or document that it can change.
- [ ] Preconditions, including Prefix Revisions computed from stored Revisions, are checked
      inside the write transaction.
- [ ] A Snapshot is a read transaction on a separate connection, and doesn't block Commits.
      `supports_snapshots()` is `true`.
- [ ] The whole shared suite passes on SQLite.
- [ ] The Backend is compiled only with the `sqlite` feature.
