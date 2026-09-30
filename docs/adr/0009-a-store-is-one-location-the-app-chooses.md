---
status: proposed
---

# A Store is one Location the app chooses

A Store used to hold three fixed Areas, config, data and cache, all on one Backend, in directories
the `etcetera` crate picked from an App identity. That didn't fit two kinds of app: one that only
wants config, which got a data and a cache directory it never used, and one that wants ten
separate places, or config on the filesystem and data in SQLite.

So a Store is now one **Location**, a directory the app passes in, held by one Backend, with its
own Change feed. An app opens as many Stores as it needs, each on the Backend it chooses. Areas, the
App identity, the Root override and the `etcetera` dependency are gone: an app that wants the
platform's standard directories depends on `etcetera` itself and passes what it picks.

Almost nothing spanned Areas already: a Staging, a Commit, a Snapshot, a Prefix Revision and a
Resync each belonged to one. Only the Change feed, the order of Commits on it, and `detect` covered
all three, and the feed's guarantees (merged per Path, one batch per Commit, in order) were per
Area anyway. So splitting them loses no guarantee. What changes:

- `Staging::new()` takes no Area, a Change has no `area`, and a Resync names nothing.
- `Store::open_fs(dir, options)`, `Store::open_sqlite(dir, options)` and `Store::open_memory()`.
  A SQLite Location is a directory too, holding `.tidings/backend` and `.tidings/store.sqlite3`,
  so a Location means the same on every Backend and ADR 0007's Backend marker works unchanged, one
  per Location. `detect(dir)` reads that one marker, and `MixedBackends` is gone.
- A Store's Location can't be inside another Store's, or a Working copy: opening refuses one
  whose ancestor holds a `.tidings/` directory. A directory in a Location that holds `.tidings/`
  belongs to another Store, or to a Working copy, and is outside this one, like a symlinked
  directory: not listed or watched, and a write under it is an invalid Path. So `.tidings` is reserved at any depth of a Path, not only as
  its first name. The same Location opened twice, on the same Backend, is allowed: the two Stores
  see each other's Commits as external, as two processes do.
- Cache is no longer a kind of place. Any Location may vanish; the Store makes it again and sends a
  Resync, as it already did for every Area.
- The CLI takes `--store <dir>` for the Location, commands take no Area, and `--identity` is gone.

There was no migration: the formats on disk, including the Working copy record, change in place and
are still called version 1.

## Considered options

- **A Store holding any number of named Areas**, each on its own Backend, sharing one Change feed
  whose items name the Area: rejected. The Store would own nothing on disk, only group Stores
  together, and every call would still take an Area. Its one feed would add no guarantee across
  Areas, since the Backends commit separately; an app that wants one stream merges the feeds.
- **A SQLite Location that is the database file**: rejected. It would need no marker, but a
  Location would then be a file on one Backend and a directory on the other, and `detect` and the
  CLI would have to handle both.
- **Keeping `--identity` in the CLI, with etcetera there**: rejected. Apps now put Stores wherever
  they like, so an App identity would find only some of them.
