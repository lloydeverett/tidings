# 03: The full Staging, and reading

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0002](../../adr/0002-writes-only-through-all-or-nothing-commits.md)

**What to build:** An app can do everything with Files except Preconditions and Snapshots:
- Stage deletes, including deleting everything under a Prefix.
- Rename a File by deleting and writing in one Commit.
- Stat a File without loading it, and list Paths under a Prefix.
- Tell a missing File from an error.

Commits give every File the same timestamp and return the new Revisions. A write that changes
nothing is left out of the Commit. Everything works on the memory Backend and is covered by the
shared suite.

**Blocked by:** 01

**Status:** done

- [x] Reading a missing File returns `Ok(None)`.
- [x] `stat` returns the last-modified time and Revision without the contents.
- [x] `list(area, prefix)` returns only Paths. The empty Prefix lists the whole Area.
- [x] A Staging supports `delete(path)`, and `delete_prefix(prefix)`, which is expanded when the
      Commit runs. Deleting a Path that doesn't exist does nothing.
- [x] Deleting a Path and writing another in the same Commit acts as a rename, and happens
      all-or-nothing. The memory Backend applies a Commit under one lock, and the rename's two
      Changes arrive in one batch. A Commit can't fail after staging until ticket 04 adds
      Conflicts, so the "nothing" half gets its test there.
- [x] Every File in a Commit gets the same last-modified time.
- [x] A successful Commit returns its timestamp and the new Revision of each Path it wrote.
- [x] A write whose contents are identical to what is stored is left out: the timestamp doesn't
      change, and it produces no Change.
- [x] Deletes produce *removed* Changes. A Commit's Changes still arrive in one batch.
- [x] A Commit that touches nothing (empty, or only writes that change nothing) produces no batch.

**Notes:**

- A Staging keeps what is staged for each Path, and the later of a write or delete to the same
  Path wins. `delete_prefix` follows the same rule: it drops anything staged under the Prefix
  before it, and a write or delete under the Prefix staged after it still happens. So staging
  `delete_prefix("drafts/")` and then `write("drafts/a.md", ..)` replaces the contents of
  `drafts/`. The Backend expands the Prefix delete under its lock, into deletes of the Paths that
  exist then and aren't staged individually.
- `Store::commit` returns `Committed`, with `timestamp()` and `revisions()`, a map from each Path
  written to its Revision. A write left out because it changed nothing is still in `revisions()`
  (its Revision is the stored one), so an app can write that File again safely without reading
  it. Deleted Paths aren't in it.
- `stat` returns `Option<Stat>`, a new public type with `modified()` and `revision()`. `list`
  returns the Paths in order, as `Vec<Path>`.
- The suite's invalid-Path test now also covers `stat` and `Staging::delete` with a refused Path
  for each rule, and `list` and `Staging::delete_prefix` with a refused Prefix for each rule a
  Prefix can break.
