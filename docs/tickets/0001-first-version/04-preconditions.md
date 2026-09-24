# 04: Preconditions

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** An app can make a Commit depend on what it read. It can require that a File
is absent, or unchanged since a Revision. It can do this for Files it writes or deletes, for Files
it doesn't write, and for everything under a Prefix, via a Prefix Revision. If any Precondition
fails, the Commit writes nothing and says which Paths conflicted. Two Paths that differ only in
letter case are refused. Everything works on the memory Backend and is covered by the shared suite.

**Blocked by:** 03

**Status:** done

- [x] Staged writes and deletes take a Precondition: *any* (the default), *absent*, or *unchanged
      since* a Revision.
- [x] Writing back a File that was read carries *unchanged since* its Revision automatically.
- [x] `require(path, precondition)` adds a Precondition on a File the Staging does not write.
- [x] `stat_prefix(area, prefix)` returns a Prefix Revision. It changes when a File under the
      Prefix is added, removed or changed. The empty Prefix covers the Area.
- [x] `require_prefix(prefix, prefix_revision)` makes the Commit fail if anything under the
      Prefix differs.
- [x] A failed Precondition gives `Conflict { paths }` and nothing is written. For a Prefix, the
      paths are the ones added, removed or changed under it.
- [x] A rename (a delete and a write in one Commit) that hits a Conflict leaves both Paths as
      they were. This is the "nothing" half of ticket 03's all-or-nothing rename, which couldn't
      be tested until a Commit could fail.
- [x] A Staging with no Preconditions depends on nothing, and costs nothing extra.
- [x] A Commit that would create a Path differing only in letter case from an existing Path (or
      from another Path in the same Commit) is refused with `InvalidPath`.
- [x] Tests cover a Prefix Precondition failing when a File is *added* under the Prefix after the
      Prefix Revision was taken.

**Notes:**

- The API: `Precondition` is `Any` (the default), `Absent` or `UnchangedSince(Revision)`.
  `Staging::write_requiring(path, contents, precondition)` and `delete_requiring(path,
  precondition)` stage with one, and `write` and `delete` stage with `Any`.
  `write_back(&file, contents)` writes to the Path of a `File` that was read, requiring it
  unchanged since that File's Revision. It returns `&mut Self` rather than a `Result`, because the
  Path is already valid. `require(path, precondition)`, `require_prefix(prefix, prefix_revision)`
  and `Store::stat_prefix(area, prefix)` are as the spec names them. A failed Precondition gives
  `Error::Conflict { paths }`, with the Paths in order.
- **A Precondition is never dropped** (the question the ticket 03 review raised). A later `write`,
  `delete` or `delete_prefix` replaces the action staged for a Path, but every Precondition staged
  on that Path still has to hold. So `write_back(&draft, ..)` followed by `delete_prefix("drafts/")`
  deletes the draft only if it is unchanged since it was read. The Precondition was about what the
  app read, which a later change of plan doesn't undo. Every Precondition staged is kept in one
  list, apart from the actions, so nothing has to move it. Two Preconditions on the same Path that
  can't both hold (such as `Absent` and `UnchangedSince`) always give a Conflict. Tested by
  `a_precondition_stays_when_something_staged_later_replaces_it`.
- **How a Prefix Conflict names Paths.** A `PrefixRevision` holds the XXH3-128 hash the spec
  describes (over each Path, a NUL, and its Revision, in order of Path) and, privately, the
  sorted (Path, Revision) list it was taken over, behind an `Arc`. Equality and `Hash` use the hash
  alone, and `Debug` shows only the hash, so it stays opaque to the app. When a Commit checks it,
  the Backend gives the current list, and if the hashes differ, the two lists are compared to find
  the Paths added, removed or changed. The cost is memory in proportion to the number of Files
  under the Prefix, for as long as the app holds the Prefix Revision. The alternative, keeping only
  the hash, couldn't name any Path.
- **The checks are shared.** `Staged::check_preconditions(&impl AreaState)` holds all the
  Precondition logic. A Backend implements the crate-private `AreaState` trait (`revision(path)`,
  and `revisions_under(prefix)` in order of Path, both returning `Result` so the filesystem can
  report I/O errors), and calls the check under its lock before writing anything. Only reading the
  current state is Backend-specific. A Staging with no Preconditions has an empty list, so the
  check reads nothing. `Store::stat_prefix` goes through the same `revisions_under`.
- **Letter case.** `caseless` (unicode-rs, the same group as `unicode-normalization`; Unicode 16)
  does the folding. Two names clash if they are equal after uppercasing and then Unicode canonical
  caseless matching (NFD, full case folding, NFD). Folding is what macOS does; Windows compares in
  upper case, and the dotless `ı` matches `I` only that way. I checked every code point: equal
  under folding alone implies equal under this key, so it is the stricter of the two.
  `path::letter_case_key` is the one definition. The `.tidings` check now uses it too, which
  refuses everything the uppercase comparison did.
- The clash check covers Prefixes as well as Paths: `Themes/light.toml` clashes with an existing
  `themes/dark.toml`, because on a case-insensitive filesystem they share a directory. A Path
  deleted in the same Commit doesn't count, so renaming `Settings.toml` to `settings.toml` in one
  Commit works. `Staged::refuse_letter_case_clashes(existing_paths)` is shared too, called after
  `expand_prefix_deletes`. Preconditions are checked first, so a Commit that breaks both gives
  `Conflict`. The refused Path is in `InvalidPath { path, reason: LetterCaseClash }`.
- For later tickets:
  - Ticket 07: a unique case-folded Path column catches clashes between whole Paths, but not
    between Prefixes (`Themes/a` against `themes/b`). Use `letter_case_key` for the column and
    still call `refuse_letter_case_clashes`, or also store the folded Prefixes.
  - Ticket 09: a File and a Prefix with the same name (`a` and `a/b`) can both exist on memory,
    but not on a filesystem. That isn't a letter-case rule, so this ticket doesn't refuse it.
