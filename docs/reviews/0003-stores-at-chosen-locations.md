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

---

## Ticket 03: Docs

Reviewed: `git diff a3b50a8...efa8c85` (commit efa8c85).

### Standards

**(a) Documented-standard violations:** none of CONTEXT.md. "Area" and `--root` appear in ADR
0009 only as history, and "events" means filesystem events, as earlier reviews accepted.

- **Lines wrapped too early:** `docs/specs/0003-stores-at-chosen-locations.md:3`, and
  `README.md:96, 416, 444`.
- **Judgement call:** "a filesystem Store" (README:92, 234) uses "filesystem" as an adjective for
  the Backend, not as a name for the Store.

**(b) Baseline smells (judgement calls):**

- **Duplicated Code:** the README and the crate docs hold the same example, and only the crate
  docs' copy compiled. The explanation of nesting, opening a Location twice and a vanished
  Location is in four places: the README, the crate docs, ADR 0009 and CONTEXT.md.
- **Drift:** CONTEXT.md's Location entry left out "inside a Working copy", which the README and
  the crate docs both state.
- **Inconsistent casing:** the new section writes Store and Location, while the older README
  sections write "store" and "backend".

Factual checks all held: the error names, the example's API, the CLI flags and environment
variables, `{"resync": true}`, and etcetera's strategies.

### Spec

The reviewer built the crate and the CLI and ran the doctests. They re-ran the README walkthrough,
whose hashes match, and checked by running that:
- `--backend fs` on a SQLite Location is refused;
- a nested Location is refused, naming the outer directory;
- `sync` at the Store's own Location is refused;
- the JSON for a missing File is right.

- **(a) Missing or partial:** none. No stale mention of Areas, App identity, `--root`,
  `--identity` or `etcetera` is left outside history and deliberate refusals.
- **(b) Scope creep (minor):** ADR 0009's decisions were rewritten to add what tickets 01 and 02
  decided, not only its status changed. `etcetera` was added as a dev-dependency for the doctest.
- **(c) Wrong:** the README said that on macOS `etcetera`'s native strategy gives the same
  directory for config and data. It doesn't: in etcetera 0.11, config is in
  `~/Library/Preferences/` and data in `~/Library/Application Support/`. The mistake came from the
  spec's Further Notes and the ticket, written when this work was planned.

### Summary

Standards: 0 hard violations, 4 lines wrapped wrong, 3 judgement-call smells (worst: the README's
example was never compiled). Spec: 1 finding (the false macOS claim, which came from the spec
itself).

### Resolution

1. **The macOS claim:** fixed. The README bullet is gone. The bullet on opening one Location twice
   now says that the other Backend gives `WrongBackend`, and that an app which could give two
   Stores the same directory should join a name of its own to each. The spec's note is corrected
   and says what it first claimed. The ticket is left as written, as history.
2. **Wrapping:** fixed.
3. **The README's example:** now compiled. It is marked `rust,no_run`, and the crate includes the
   README as a `cfg(doctest)` item, so its example is built with the doctests and can't drift from
   the API.
4. **CONTEXT.md's Location entry:** fixed. It says a Location can't be inside a Working copy
   either, and that a directory holding a Working copy is not part of it.
5. **Explaining nesting in four places:** not fixed. Each is for a different reader: the glossary,
   the decision, the README and the API docs. Each is short.
6. **Casing in the older README sections:** not fixed. The README's prose has always written
   "store" and "backend" in lower case, and only the new section uses the glossary's capitals.
   Recasing the whole README is out of this ticket's scope.
7. **ADR 0009's rewritten decisions and the dev-dependency:** kept. The ADR should record the
   decision as built, and the dev-dependency doesn't reach crates that depend on tidings.

`cargo fmt`, clippy, `cargo doc` with warnings denied and the doctests (now two) pass.

---

## Follow-up: the Working copy folder refusal moves to the CLI

Ticket 02 had the library refuse a Location that is a Working copy's folder, with
`Error::LocationIsWorkingCopy`. To do that it knew the CLI's record, `.tidings/working-copy`, by
name, which leaks a CLI concept into the library. The CLI now refuses `--store` naming a Working
copy's folder, the error is gone, and the library's docs, messages and tests no longer mention
Working copies.

Reviewed: `git diff 62fc326...1ad4277` (commit 1ad4277). The spec was the request "move the check
into the CLI, and make sure the library has no knowledge about things that are CLI-layer only",
with ADR 0009 and spec 0003.

### Standards

- **Hard violations (wrapping):**
  - Four comment lines ran past 100 columns: src/backend/fs.rs:33, src/path.rs:47,
    src/path.rs:116 and tests/behaviour/main.rs:1506.
  - Two paragraphs in src/backend/fs/watch.rs were left with short lines.
- **Glossary drift (judgement call):** CONTEXT.md's Location entry still says a Location can't be
  a Working copy's folder, without saying which layer enforces it.
- **Smells (judgement calls):**
  - `StoreArgs::address`'s doc comment: "but never makes one" now hangs off the "since" clause.
  - Possible Shotgun Surgery: the two refusals that keep a Store and a Working copy apart are in
    separate places, `sync` in working_copy.rs and `StoreArgs::address` in location.rs.
  - Possible Mysterious Name: the fixtures' "bare" directory, and test names like
    `..._or_a_tidings_directory_is_refused`, which say "a directory holding `.tidings/`" twice.
- **Leftovers:** none. src/ and tests/ don't mention Working copies or the `tidings` command.

### Spec

- **(a) Missing or partial:**
  - CONTEXT.md isn't in the diff.
  - Spec story 28 still names Working copies from an app developer's view.
  - Only `store list --create` tests the new CLI check. `sync --create` and `store shell` aren't
    tested, though all three go through the same function.
- **(b) Scope creep:** none.
- **(c) Wrong:**
  - ADR 0009's third bullet still said a directory holding `.tidings/` "belongs to another Store,
    or to a Working copy", which sits badly with the new wording that the library doesn't know
    what made it.
  - Coverage is otherwise right: every CLI path from `--store` goes through the check, before
    anything is made.

### Summary

Standards: 6 wrapping violations, 1 glossary drift and 3 judgement-call smells (worst: the
wrapping). Spec: 3 partial and 1 inconsistent (worst: `sync --create` isn't tested against a
Working copy's folder).

### Resolution

1. **Wrapping:** fixed.
2. **`address`'s doc comment:** fixed, by splitting the sentence.
3. **CLI tests:** `sync --create` of a new Working copy with `--store` naming a Working copy's
   folder is now tested, and the new folder is checked not to have been made. `store shell` isn't
   tested separately: it opens the Store through the same `StoreArgs::open` as `store list`.
4. **ADR 0009's third bullet:** fixed. It now says "another Store, or … something else such as a
   Working copy".
5. **CONTEXT.md:** not changed. It is the whole project's glossary, CLI included, and has no
   implementation details. So the rule stays true there, and which layer enforces it belongs in
   the ADR, which now says so.
6. **Spec story 28:** not changed. It asks for opening a Store *inside* a Working copy to be
   refused, which the library still does through its generic rule for `.tidings/` directories. The
   spec's decisions now say that the CLI refuses a Working copy's folder itself.
7. **The two CLI refusals in separate places:** not changed. Each sits where its command's other
   checks are, and each is one line calling a shared helper (`same_directory` and
   `WorkingCopy::exists`).
8. **"bare" and the test names:** not changed. The doc comments explain them, and the names
   contrast the two cases each test loops over.
