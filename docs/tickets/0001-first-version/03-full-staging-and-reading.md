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

**Status:** ready-for-agent

- [ ] Reading a missing File returns `Ok(None)`.
- [ ] `stat` returns the last-modified time and Revision without the contents.
- [ ] `list(area, prefix)` returns only Paths. The empty Prefix lists the whole Area.
- [ ] A Staging supports `delete(path)`, and `delete_prefix(prefix)`, which is expanded when the
      Commit runs. Deleting a Path that doesn't exist does nothing.
- [ ] Deleting a Path and writing another in the same Commit acts as a rename, and happens
      all-or-nothing.
- [ ] Every File in a Commit gets the same last-modified time.
- [ ] A successful Commit returns its timestamp and the new Revision of each Path it wrote.
- [ ] A write whose contents are identical to what is stored is left out: the timestamp doesn't
      change, and it produces no Change.
- [ ] Deletes produce *removed* Changes. A Commit's Changes still arrive in one batch.
- [ ] A Commit that touches nothing (empty, or only writes that change nothing) produces no batch.
