# 04: Preconditions

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** An app can make a Commit depend on what it read. It can require that a File
is absent, or unchanged since a Revision. It can do this for Files it writes or deletes, for Files
it doesn't write, and for everything under a Prefix, via a Prefix Revision. If any Precondition
fails, the Commit writes nothing and says which Paths conflicted. Two Paths that differ only in
letter case are refused. Everything works on the memory Backend and is covered by the shared suite.

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] Staged writes and deletes take a Precondition: *any* (the default), *absent*, or *unchanged
      since* a Revision.
- [ ] Writing back a File that was read carries *unchanged since* its Revision automatically.
- [ ] `require(path, precondition)` adds a Precondition on a File the Staging does not write.
- [ ] `stat_prefix(area, prefix)` returns a Prefix Revision. It changes when a File under the
      Prefix is added, removed or changed. The empty Prefix covers the Area.
- [ ] `require_prefix(prefix, prefix_revision)` makes the Commit fail if anything under the
      Prefix differs.
- [ ] A failed Precondition gives `Conflict { paths }` and nothing is written. For a Prefix, the
      paths are the ones added, removed or changed under it.
- [ ] A rename (a delete and a write in one Commit) that hits a Conflict leaves both Paths as
      they were. This is the "nothing" half of ticket 03's all-or-nothing rename, which couldn't
      be tested until a Commit could fail.
- [ ] A Staging with no Preconditions depends on nothing, and costs nothing extra.
- [ ] A Commit that would create a Path differing only in letter case from an existing Path (or
      from another Path in the same Commit) is refused with `InvalidPath`.
- [ ] Tests cover a Prefix Precondition failing when a File is *added* under the Prefix after the
      Prefix Revision was taken.
