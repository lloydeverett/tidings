# 01: Remove Areas: a Store is one Location

Spec: [0003](../../specs/0003-stores-at-chosen-locations.md)

**What to build:** An app opens a Store at a directory it chooses, on the Backend it chooses, and
as many as it wants: config on the filesystem in one directory, data in SQLite in another. Every
call after opening is the same on every Backend and names no Area. tidings no longer picks
platform directories: the App identity, the Root override and the `etcetera` dependency are gone.
The `tidings` command names a Store with `--store <dir>` and takes no Area. This ticket covers
the whole library and the CLI together, so that it ends with the workspace building and every
test green. Nesting is ticket 02.

Start with a prefactor that changes no behaviour and keeps every test green: each Backend, and
the filesystem watcher and SQLite poller, handles one Location, and only the Store layer and the
Change feed still hold one per Area. Then remove the Areas.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `Store::open_fs` and `Store::open_sqlite` take the Location and their options, and make the
      Location and its parents if missing. `Store::open_memory` is unchanged. `blocking::Store`
      matches.
- [x] `read`, `stat`, `list`, `stat_prefix` and `snapshot` take no Area. `Staging::new()` takes
      none, and has no `area()`. `Change` has no `area`, and `FeedItem::Resync` carries nothing.
- [x] `Area`, `AppIdentity`, the Root override on both options types, and `etcetera` are removed;
      neither Backend feature pulls in `etcetera`.
- [x] `Store::detect` takes the Location and reads its one Backend marker. `MixedBackends` is
      removed, and `WrongBackend` names only the Backend found. Markers otherwise follow ADR 0007
      for one Location.
- [x] SQLite keeps its database at `.tidings/store.sqlite3` in the Location.
- [x] A Prefix Revision carries the identity of the opened Store that took it. Requiring it in a
      Staging committed to another Store, or for another Prefix, makes the Commit fail with an
      error instead of panicking when it is staged.
- [x] Each Store has its own Commit ordering and its own watch or poll task; a Commit never waits
      for another Store's. A task that stops sends one Resync.
- [x] Removing a Location while its Store is open gives a Resync, and the Location is made and
      marked again, on every Store on disk.
- [x] The behaviour suite, marker, blocking and Store-layer tests use one Store at a temporary
      directory. New tests: two Stores at different Locations, on the same Backend and on fs and
      SQLite together, are independent in their Files, feeds and Resyncs, and a Prefix Revision
      from one is refused by a Commit to the other.
- [x] The CLI takes `--store <dir>` (and `TIDINGS_STORE`) in place of `--root` and `--identity`;
      `--backend` and `--create` are unchanged, and no command takes an Area, including `sync`.
      JSON output has no `area`.
- [x] The Working copy record holds `store` (the absolute Location) and `backend`, and is still
      version 1. A record in the old shape is refused as one in an unknown format.
- [x] The CLI tests use `--store`, and cover `--identity` being rejected and an old-shape record
      being refused.
- [x] Doc comments on every item changed here describe Stores and Locations, not Areas.
