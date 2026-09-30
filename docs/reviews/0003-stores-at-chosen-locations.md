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
