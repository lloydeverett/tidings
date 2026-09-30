# Code reviews: spec 0003, Stores at chosen Locations

Every ticket in [docs/tickets/0003-stores-at-chosen-locations](../tickets/0003-stores-at-chosen-locations)
was reviewed after it was implemented, using the two-axis `/mattpocock-skills:code-review`. It checks
two things: **Standards** (the repo's documented conventions plus the Fowler smell baseline) and
**Spec** (the ticket, spec 0003 and ADR 0009). The two axes are reported separately and not ranked
against each other. Each entry ends with a **Resolution** section recording what was fixed or
deliberately not fixed, and why.

---

## Ticket 01: Remove Areas: a Store is one Location

Reviewed: `git diff 63d8150...0f56103` (commits 490eb30, the prefactor, and 0f56103).

### Standards

**(a) Documented-standard violations:** none hard. No _Avoid_ words are used as domain terms, and
the code follows ADR 0009. The remaining matches for "identity" and "root" are deliberate: the fs
file identity, and refusing an old-shape record.

- **Comments wrapped too early:** comments shortened by the removal were not reflowed, at
  `src/change.rs:46, 59, 69`, `src/staging.rs:16`, `src/blocking.rs:8, 279`,
  `src/backend/memory.rs:4`, `cli/src/main.rs:47` and `cli/src/working_copy.rs:506, 531`.
- **A line over 100 columns:** a 122-column string literal at `cli/tests/shell.rs:137`.

Leftovers:

1. `src/store.rs:283` says "…which can be large for a cache", but Cache is no longer a kind of
   place.
2. The test `clearing_the_cache_removes_the_unchanged_local_files` now deletes every File in the
   Store, not a cache Area.
3. `src/prefix.rs:45` calls `/` "the root", and "root" is an _Avoid_ word for Location.

**(b) Baseline smells (judgement calls):**

- **Mysterious Name:**
  - `cli/src/command.rs:145`: `staging()` returns only a count.
  - `cli/src/command.rs:223, 238, 251`: `self.open()?` is called only to check that a Staging is
    open, and its result is thrown away.
  - `cli/tests/common/mod.rs:39, 57`: `location.store()` gives a directory.
  - `src/change.rs:109, 118`: the field `pending: Pending` and the enum `Pending` both have the doc
    "What is unread.".
- **Primitive Obsession**, `cli/src/location.rs:506`: a Location is a bare `PathBuf`, "always
  absolute" only by its doc comment.
- **Middle Man**, `cli/src/location.rs:478`: the free `detect()` only wraps `Store::detect`.
- **Duplicated Code:** "move the Location away, then remove it" is written out three times in the
  behaviour tests.

### Spec

Every ticket checkbox is implemented, with tests where the ticket asks for them: two separate
Stores on fs+fs, SQLite+SQLite and fs+SQLite; `--identity` and `--root` refused; an old-shape
record refused.

- **(a) Missing or partial:** none.
- **(b) Scope creep:** none. The reviewer judged each of the implementer's decisions not in the
  spec to be acceptable:
  - `Error::WrongPrefixRevision`, checked before the Commit takes its turn;
  - Prefix Revision equality including the Store;
  - SQLite noticing a removed Location by device and inode, and owing its Resync until one is
    delivered;
  - fs marking the Location again when `.tidings/` is missing;
  - a Resync in JSON written `{"resync": true}`;
  - an old-shape record refused.
- **(c) Looks wrong:** SQLite checked for a removed Location only on poll and Commit. Until the
  next poll, reads served the old, unlinked database, while `snapshot`, which opens the path
  again, failed with `Error::Backend`.

### Summary

Standards: 0 hard violations, 11 lines wrapped wrong, 3 leftovers, 7 judgement-call smells (worst:
the stale mentions of the cache). Spec: 1 finding (a SQLite Snapshot failing after its Location is
removed).

### Resolution

Fixed in 8d06632:

1. **Wrapping:** fixed. The comments are reflowed and the long string literal is split.
2. **Leftovers:** fixed.
   - The comment in `store.rs` no longer mentions a cache.
   - The test is renamed for what it does, deleting every File.
   - `prefix.rs` no longer calls `/` a root.
3. **Mysterious Names:**
   - `staging()` is now `staged_count()`.
   - The thrown-away `open()` calls are now `require_open()`.
   - The `Pending` docs say what the variants distinguish.
   - The test helper's `location.store()` is left: it reads as "the Store's directory" at its call
     sites, and renaming it would touch every CLI test for little gain.
4. **Duplicated Code:** fixed. One `remove_the_location` helper is in the behaviour tests, like the
   CLI tests' helper.
5. **SQLite and a removed Location:** fixed.
   - Reads (`read`, `stat`, `list`, `revisions_under`) and `snapshot` now check for a removed
     Location first, as polls and Commits do. They share one path, and the check is a single
     `stat` of the database file.
   - A new test fails without the fix. In it, a Snapshot and a read after removal both work, and
     the Resync still comes.
   - A Snapshot taken before the removal still reads the old database, as the module doc says.
6. **Primitive Obsession and Middle Man:** not fixed.
   - The Location is a directory the app passes in, and a bare path is what ADR 0009 asks for.
   - The CLI's `detect()` wrapper only maps a type, so cutting it would move that mapping to each
     caller.

`cargo fmt` and clippy are clean. `cargo test -p tidings --all-features` passes, and so do the
CLI's shell and working-copy tests. No re-review: the fixes are small, except the SQLite check,
which is covered by its new test.

---

## Ticket 02: Nested Locations

Reviewed: `git diff 9441707...e3a14b3` (commits 4e6f08f to e3a14b3).

### Standards

**(a) Documented-standard violations:**

- **Comments wrapped too early:** `src/backend/fs/watch.rs:27` (a module doc) and
  `src/backend/fs/journal.rs:174`.
- **An earlier fix undone:** `src/backend/fs.rs:725` wrote out
  `given.strip_suffix('/').unwrap_or(given)` again. That is what `without_trailing_slash`, added
  after review 0001, was meant to stop.

No _Avoid_ words are used as domain terms. "Event" in `watch.rs` means a filesystem event, as it
did before.

**(b) Baseline smells (judgement calls):**

1. **Duplicated Code: the check on disk for "does this directory hold `.tidings/`?"** It was
   written twice, in `fs.rs` (`holds_tidings`, which passes I/O errors up) and in `marker.rs`
   (`is_ok_and`, which hides every error). So an ancestor that couldn't be read was not refused.
2. **Duplicated Code:** `watch.rs` copied `path::is_reserved`.
3. **Duplicated Code:** `left_out(..)? == Some(LeftOut::Nested)` was written out three times.
4. **Mysterious Name:** `holds_tidings` (a check on disk) and `holding_tidings` (which parses an
   event's path) were named almost alike.
5. **Mysterious Name:** `written_outside` covered only nested directories, while the `outside`
   closure beside it covered directory links too.
6. **Primitive Obsession (minor):** `Staged::changed_names` gave Paths and Prefixes as one kind of
   `&str`.

### Spec

The reviewer ran the 18 nested-location, symlink and two-Store behaviour tests; all pass. The
edge cases checked out:
- the ancestor walk works on the resolved path, including for a Location that doesn't exist yet
  and one that is a symlink;
- SQLite runs the ancestor check, and has no boundaries below its Location;
- every Backend refuses `.tidings` at any depth of a Path;
- the same Location can still be opened twice;
- Prefix deletes and finishing a Commit leave nested directories alone.

- **(a) Missing or partial:**
  1. No test that an inner Store's Commits give the outer Store no Changes: the test bound the
     outer feed as `_feed` and never checked it.
  2. No test that Files come back when `.tidings/` is removed from a subdirectory, as the
     implementation claims.
- **(b) Scope creep:** none. The reviewer judged each decision not in the spec to be acceptable:
  - `Error::NestedLocation { outer }`;
  - only a directory named `.tidings` counts as one;
  - `InvalidPathReason::Nested`, checked before Preconditions;
  - pending writes under a new boundary hidden from reads;
  - Files under a new boundary reported Removed.
- **(c) Looks wrong:**
  1. A Store could be opened at a Working copy's own folder. That follows the letter of the spec
     ("the Location itself holding `.tidings/` is the normal case"), but it then writes a Backend
     marker into the Working copy's `.tidings/`, against story 28's intent.
  2. The ancestor check treated an I/O error as "not nested", the same as smell 1.

### Summary

Standards: 2 lines wrapped wrong, 1 earlier fix undone, 6 judgement-call smells (worst: the check
for `.tidings/` written twice, with different error handling). Spec: 2 missing tests, 2 findings
(worst: a Store could be opened at a Working copy's folder).

### Resolution

1. **Wrapping:** fixed in 6bac34e. The `journal.rs` comment was reworded, since its old wrap was
   right for its words.
2. **Slash stripping:** fixed in 6bac34e, using the shared helper again.
3. **The check for `.tidings/`:** fixed in 6bac34e (smell 1 and spec (c) 2). There is one
   `marker::holds_tidings`, used both when opening and by the filesystem Store. It gives `false`
   only when nothing is there, and any other I/O error makes opening fail. The duplicated
   `is_absent` moved to the Backend module.
4. **`is_reserved`:** fixed in 6bac34e. It is shared, not copied.
5. **The repeated `Nested` test:** partly fixed in 6bac34e. Two uses now call `is_nested`. The
   `outside` closure in `Journal::finish` still matches on `left_out`, because it handles directory
   links too, and `is_nested` would walk the directories on disk a second time.
6. **Names:** fixed in 6bac34e. The event parser and `written_outside` are renamed for what they
   do.
7. **Primitive Obsession:** not fixed. Using `without_trailing_slash` again covers the one caller
   that suffered, and a new type for one iterator isn't worth it.
8. **Missing tests:** fixed in f5c60cf. The outer Store's feed is checked to stay quiet while the
   inner Store commits. A new test checks that Files come back, as Changes, when a
   subdirectory's `.tidings/` is removed.
9. **A Store at a Working copy's folder:** fixed in ff47178.
   - `open_fs`, `open_sqlite` and `detect` refuse a Location whose `.tidings/` holds a Working copy
     record, with a new `Error::LocationIsWorkingCopy`. The library knows the record only by its
     file name, and says why in a comment. The check runs before anything is made.
   - In the other direction, a folder that is already a Store's Location was refused as not empty.
     But `sync <dir> --store <dir> --create` got through, making the Store and then the Working
     copy in one `.tidings/`. `sync` now refuses it.
   - Both directions are tested, and CONTEXT.md's Location entry and spec 0003 are updated.

Also fixed, from ticket 01, in c6bf0fc: the intermittently failing test
`removing_one_stores_location_resyncs_only_that_store`. The code was right and the test was wrong.
Sometimes the watcher reads `kept.txt`'s events from the Commit only after the Location has been
moved away, and then rightly reports `kept.txt` Removed before the Resync. The single-Store test
already allows for this. The test now accepts exactly that Change first. It passed 12 runs under
load and four full-suite runs.

`cargo fmt` and clippy are clean, and the full `cargo test --workspace --all-features` passes. No
re-review: item 9 is small and tested in both directions, and the rest are mechanical.
