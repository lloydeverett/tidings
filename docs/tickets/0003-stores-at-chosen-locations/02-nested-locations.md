# 02: Nested Locations

Spec: [0003](../../specs/0003-stores-at-chosen-locations.md)

**What to build:** Now that apps choose Locations, two Stores, or a Store and a Working copy, can
end up one inside the other. They never claim the same files. Opening a Store inside another
Store's Location, or inside a Working copy, is refused. A filesystem Store whose Location already
holds another Store or a Working copy in a subdirectory treats that directory as outside itself,
as it treats a symlinked directory today. No Path can name `.tidings` at any depth. Build it
test-first.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] Opening a filesystem or SQLite Store whose Location has an ancestor holding a `.tidings/`
      entry is refused before anything is made, with an error naming that ancestor. That covers
      both another Store's Location and a Working copy folder. Symlinks in the Location are
      resolved before the check. The Location itself holding `.tidings/` is normal.
- [ ] A Path with a `.tidings` name at any depth, in any letter case, is invalid, with the same
      reason as one starting with it.
- [ ] In a filesystem Store, a directory in the Location that holds a `.tidings/` entry is outside
      the Store: `list`, `stat_prefix` and `read` don't see what's under it, and the Change feed
      reports nothing from it.
- [ ] A Commit writing or deleting a Path under such a directory, including through a Prefix
      delete, fails with an invalid Path and writes nothing. Finishing an interrupted Commit
      never writes or deletes under one.
- [ ] A `.tidings/` made inside the Location while the Store is open makes its directory drop
      out: each Path that was under it gets a Removed Change.
- [ ] Opening the same Location twice on the same Backend still works, each Store seeing the
      other's Commits as external.
- [ ] Tests go through the public API in the behaviour suite's filesystem and SQLite modules, and
      in the Path tests.
