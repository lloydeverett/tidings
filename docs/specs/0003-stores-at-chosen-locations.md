# Spec 0003: Stores at Locations the app chooses

Status: built. Terms are used as defined in [CONTEXT.md](../../CONTEXT.md), and the decisions
in [docs/adr](../adr) apply throughout, especially
[ADR 0009](../adr/0009-a-store-is-one-location-the-app-chooses.md).

## Problem Statement

An app developer opening a Store gets exactly three Areas, config, data and cache, all on one
Backend, in directories tidings picks from an App identity through the `etcetera` crate.

- An app that only keeps config still gets data and cache directories made for it, on disk, that
  it never uses.
- An app that wants more separate places than three (a Store per account, per project, per
  plugin) has to fake them with Prefixes inside one Area, and so shares one Change feed and one
  set of Resyncs between them.
- An app can't keep its config on the filesystem, where people edit it, and its data in SQLite,
  where Snapshots hold, since one Store has one Backend for every Area.
- An app can't choose where its files go. It can pick the App identity, or a Root override that
  the docs say is for tests, but not "my config in this directory". An app that already uses
  `etcetera`, `directories` or its own layout has to fit tidings' instead.
- Every call names an Area, even in an app that uses only one.

The person using the `tidings` command sees the same shape: `--root` means a directory holding
`config`, `data` and `cache`, and every command takes an Area.

## Solution

A **Store** is one **Location**, a directory the app passes in, held by one Backend, with its own
Change feed. The app opens as many as it wants, each on the Backend that suits it:

- `Store::open_fs(dir, options)`, `Store::open_sqlite(dir, options)` and `Store::open_memory()`.
- Nothing after opening depends on the Backend, as today: reading, listing, Staging, Commits,
  Preconditions and the Change feed are the same on every Store. The Area argument is gone from
  all of them.
- tidings no longer knows about platform directories. An app that wants them uses `etcetera` (or
  anything else) itself and passes what it picks.
- Any Location may vanish, as a cache directory does when the OS clears it. The Store makes it
  again and sends a Resync, as it did for every Area.
- A Location can't be opened inside another Store's Location, or inside a Working copy. A
  directory inside a Location that holds another Store, or a Working copy, is not part of it.
- The `tidings` command takes `--store <dir>`, the Location, and commands take no Area.

## User Stories

### Opening Stores

1. As an app developer, I want to open a Store at a directory I choose, so that my files go where
   my app's layout says, not where tidings decides.
2. As an app developer who only keeps config, I want to open just one Store, so that nothing else
   is made on disk for me.
3. As an app developer, I want to open as many Stores as I need, so that separate things (accounts,
   projects, plugins) each get their own Files, Change feed and Resyncs.
4. As an app developer, I want to open one Store on the filesystem and another on SQLite in the
   same app, so that config stays hand-editable while data gets Snapshots.
5. As an app developer, I want every Store to offer the same calls whatever its Backend, so that
   the rest of my app doesn't care which Backend a Store is on.
6. As an app developer, I want opening a Store to make its Location, and any missing parent
   directories, so that I can pass a directory that doesn't exist yet.
7. As an app developer, I want a SQLite Store's Location to be a directory, like a filesystem
   Store's, so that I choose Locations the same way for both and can switch Backend without
   changing where it goes.
8. As an app developer, I want an in-memory Store to need no Location, so that tests and
   throwaway state stay as easy as today.
9. As an app developer, I want to use `etcetera` myself for the platform's standard directories,
   so that tidings doesn't pull it in for apps that don't want it.
10. As an app developer, I want opening a Location marked for the other Backend to be refused
    without changing anything, as today, so that I can't damage another Backend's data.
11. As an app developer, I want to find which Backend a Location holds, or that it holds none,
    without creating anything, so that I can open an existing Store without being told its
    Backend.
12. As an app developer, I want opening the same Location twice to work, with each Store seeing
    the other's Commits as external, so that two parts of my app, or a directory helper that gives
    the same directory for config and data, don't break anything.

### Using a Store

13. As an app developer, I want to read, stat, list and take Prefix Revisions without naming an
    Area, so that the calls say only what matters.
14. As an app developer, I want to make a Staging without naming an Area, so that a Staging
    belongs to whichever Store I commit it to.
15. As an app developer, I want a Change to carry only its Path, kind and Origin, so that I don't
    match on an Area that means nothing to me.
16. As an app developer, I want a Resync to mean "read everything you rely on in this Store
    again", so that it names nothing I have to look up.
17. As an app developer with several Stores, I want each Store's Change feed to be its own, so that
    one Store's Resync or slow reader never affects another's.
18. As an app developer with several Stores, I want to wait on their feeds together with ordinary
    tokio tools such as `select!`, so that I don't need anything from tidings to do it.
19. As an app developer, I want one Store's Commits to be applied and reported in order, as today,
    so that the guarantees I rely on don't change.
20. As an app developer, I want Commits to different Stores never to wait for each other, so that
    a slow Store doesn't hold up a fast one.
21. As an app developer, I want a Snapshot to be of the whole Store, so that there's nothing to
    choose.
22. As an app developer, I want a Prefix Revision taken from one Store to be refused by a Staging
    committed to another, so that I can't pass one to the wrong Store by mistake.
23. As a developer using the blocking API, I want `blocking::Store` to change the same way, so
    that synchronous code gets the same shape.

### Vanishing Locations

24. As an app developer keeping a cache in a Store, I want the whole Location being removed to
    give a Resync and not an error, so that the OS clearing my cache is routine.
25. As an app developer, I want a vanished Location made again and marked again by the Store, so
    that the next Commit works.
26. As an app developer, I want every Store to behave this way, so that I don't have to say which
    of my Stores are caches.

### Nested Locations

27. As an app developer, I want opening a Store whose Location is inside another Store's Location
    to be refused, naming the outer Location, so that two Stores never claim the same files.
28. As an app developer, I want opening a Store inside a Working copy folder to be refused too, so
    that a Store's files never get mixed into a Working copy.
29. As an app developer, I want a directory inside my filesystem Store that holds another Store,
    made before mine, to be outside my Store, so that my Store never lists, reports or overwrites
    that Store's Files or its `.tidings/`.
30. As a person with a Working copy inside a filesystem Store's Location, I want the Store to
    leave that folder alone, so that `sync` and the app don't report each other's writes.
31. As an app developer, I want writing a Path under such a directory to be refused as an invalid
    Path, so that I can't write into another Store by accident.
32. As an app developer, I want any Path with a `.tidings` name at any depth refused, so that no
    Path can ever name tidings' own bookkeeping, nested or not.
33. As an app developer, I want a nested Store or Working copy made while my filesystem Store is
    open to drop out of it as if its Files were removed, so that what my Store reports matches
    what is in it.

### The `tidings` command

34. As a person, I want to point the command at a Store with `--store <dir>`, or `TIDINGS_STORE`,
    so that I name the Location the app uses, whatever it is.
35. As a person, I want `tidings store read app.toml`, not `tidings store read config app.toml`,
    so that I don't name an Area that no longer exists.
36. As a person, I want `tidings sync <folder>` to make a Working copy of the Store I named, so
    that one Working copy is one Store.
37. As a person, I want the Backend still found from the Location's marker, `--backend` to make
    sure of it, and `--create` needed to make a new Store, as today, so that a typo doesn't make a
    new, empty Store.
38. As a person, I want `tidings store shell --backend memory` to keep working without a
    Location, so that I can try things out.
39. As a person, I want the Working copy to remember its Store's Location and Backend, as it does
    today, so that later commands need no flags.
40. As a person, I want `--root` and `--identity` gone, not quietly kept, so that I'm not misled
    about what a Location is.

### Documentation

41. As an app developer, I want the README and crate docs to show opening Stores at my own
    directories, including with `etcetera`, so that I know how to get the platform's directories
    myself.
42. As an app developer, I want the README's table of which Backend suits which use to still hold,
    per Store, so that I can choose a Backend for each.

## Implementation Decisions

### The library's interface

- The `Area` type, `AppIdentity`, the Root override on `FsOptions` and `SqliteOptions`, and the
  `etcetera` dependency are removed. Both Backend features stop depending on `etcetera`.
- `Store::open_fs` and `Store::open_sqlite` take the Location (anything that becomes a
  `PathBuf`) and their options. `Store::open_memory` is unchanged. The same for `blocking::Store`.
- `Store::detect` takes the Location and gives the one Backend its marker names, or `None`.
  `Error::MixedBackends` is removed. `Error::WrongBackend` keeps only the Backend it found.
- `read`, `stat`, `list`, `stat_prefix` and `snapshot` lose their Area argument. `Staging::new()`
  takes none, and `Staging::area()` is removed. `Change` loses its `area` field. `FeedItem::Resync`
  carries nothing.
- A Prefix Revision belongs to one Store and one Prefix. Since a Staging no longer knows its Store
  until it is committed, the check that a Prefix Revision belongs to it moves from staging it to
  the Commit, which gives an error rather than panicking. A Prefix Revision carries an identity
  of the Store that took it, unique per opened Store, for this.
- A Commit to a Store waits only for that Store's earlier Commits: the lock that keeps Commits in
  order on the feed is per Store, which it already effectively was per Backend.
- Nothing is added for merging several feeds.

### The Store layer

- `PerArea` is removed, and everything held per Area is held once: the Change feed's unread
  Changes are one map of Paths or a Resync, and the watch and poll tasks follow one Location.
- A watch or poll task that stops sends one Resync.

### The filesystem Backend

- It holds one Location. Opening makes it and its `.tidings/` directory, reads and writes its
  Backend marker, and finishes an interrupted Commit, as each Area's did.
- The watcher watches one Location. Removing the Location gives a Resync and makes it again, as for
  an Area.
- **Nested boundaries.** A directory in the Location that holds a `.tidings/` entry is outside the
  Store, as a symlink to a directory is today: listing skips it, the watcher ignores events under
  it, and a Commit writing or deleting under it is refused with an invalid Path, found when the
  Commit checks its Paths against the Location. The journal's recovery never writes or deletes
  under one. A `.tidings/` appearing inside the Location while the Store is open makes its
  directory drop out, with a Removed Change for each Path that was under it.

### The SQLite Backend

- It holds one database, `.tidings/store.sqlite3` in the Location, and one change log and poller.

### Paths

- A Path is invalid if any of its names is `.tidings`, ignoring letter case, not only its first,
  with the same reason as today.

### Opening and markers

- Before anything is made, opening walks up the Location's ancestors and refuses one that holds a
  `.tidings/` entry, giving a new error naming that ancestor. The Location itself holding
  `.tidings/` is the normal case, not a refusal, unless that `.tidings/` holds a Working copy
  record: opening, and `detect`, refuse a Working copy's folder with an error of its own (story
  28). Symlinks are resolved before the walk, so a
  Location reached through one is checked where it really is.
- Marking follows ADR 0007 for one Location. The rule about which Store wins the config Area, when
  Stores on both Backends open at once, is replaced by the plain `create_new` race on one marker.

### The `tidings` command

- `--store <dir>`, with `TIDINGS_STORE`, replaces `--root` and `--identity`. `--backend` and
  `--create` are unchanged. The Area argument is removed from every command, including `sync`.
- The Working copy record keeps `store <absolute Location>` and `backend <name>`, and loses
  `area`, `root` and `identity`. It is still version 1: a record in the old shape is refused as
  one in an unknown format.
- JSON output drops the `area` field from Changes and Resyncs.

### Documentation

- The README, crate docs, `Cargo.toml` description, and every doc comment that mentions Areas,
  the App identity or the Root override are rewritten. The README shows opening two Stores at
  directories from `etcetera`, one per Backend.
- ADR 0009 is marked accepted when this is built.

## Testing Decisions

- **The seams are the existing ones**: the behaviour suite for the library, the `tidings` binary
  as a subprocess for the CLI, and `tests/paths.rs` for Path validation. No new seam is needed.
- **The behaviour suite** runs every behaviour through the public API against each Backend, async
  and blocking. Its fixtures open one Store at a temporary directory instead of a Root override,
  and tests that used several Areas use one Store. New tests check that two Stores at different
  Locations, on the same or different Backends, are independent: separate Files, separate feeds,
  Resyncs that don't cross, and a Prefix Revision from one refused by a Commit to the other.
- **Vanishing Locations** are tested as removed Area directories are today: remove the Location
  while the Store is open, expect a Resync, then commit again.
- **Nesting** is tested through the public API in the behaviour suite's filesystem and SQLite
  modules:
  - opening inside another Store's Location, and inside a directory holding a bare `.tidings/`
    (as a Working copy does), is refused;
  - an inner Store made first is invisible to the outer filesystem Store's `list`, `read` and
    feed, and writing under it gives an invalid Path;
  - a `.tidings/` made inside an open filesystem Store makes its directory drop out, with Removed
    Changes;
  - the same Location opened twice sees each other's Commits as external, which the
    `two_stores` tests already do.
- **Markers** (`tests/behaviour/markers.rs`) keep their cases for one Location: refusing the other
  Backend, unreadable and empty markers, `detect` on unmarked and marked Locations. The
  `MixedBackends` cases are removed.
- **Paths**: `tests/paths.rs` gains cases for `.tidings` at depth, in any letter case.
- **The CLI** tests (`cli/tests`) move from `--root` and Area arguments to `--store`, and gain a
  test that a Working copy record in the old shape is refused, and that `--identity` is no longer
  accepted.
- Good tests check only what an app or person can observe: what calls return, what the feed says,
  what the command prints and its exit code, and which files exist. Not internal layout, apart
  from the documented `.tidings/` entries.
- Prior art: `tests/behaviour/suite.rs` and `two_stores.rs` for behaviour across Backends,
  `markers.rs` for Backend markers, the filesystem module's test of an Area directory removed
  while running for vanishing Locations, `tests/paths.rs` for Path rules, and `cli/tests/one_shot.rs` and `working_copy.rs` for
  the command.

## Out of Scope

- Commits or Snapshots across several Stores.
- A single `Store::open` over an enum of Backends. An app can write one; it can be added later.
- A helper to merge several Change feeds, or `ChangeFeed` implementing `Stream`.
- Migrating Stores or Working copies laid out as Areas. Nothing is read in the old shape.
- Finding Stores from an App identity in the CLI.
- Any change to what a Commit, Precondition or Change means within one Store.

## Further Notes

- Two refinements were made while writing this, beyond what was discussed:
  - A Prefix Revision can no longer be checked against its Store when it is staged, since a
    Staging doesn't know its Store. The check moves to the Commit, as an error, and a Prefix
    Revision carries the identity of the Store that took it.
  - Nested boundaries are found by any `.tidings/` entry, not only a Backend marker, so a Working
    copy folder is a boundary too. The same rule refuses opening inside one.
- An app that gives two Stores the same directory opens the same Location twice, which works
  (story 12), but gives two Stores over the same Files, or `WrongBackend` on different Backends.
  The README says so. (This note first claimed that `etcetera`'s native strategy does this on
  macOS; it doesn't, since config and data are in different `~/Library` directories.)
