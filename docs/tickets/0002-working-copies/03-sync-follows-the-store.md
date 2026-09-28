# 03: Sync follows the Store live

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** While `sync` runs, Store Changes to the Area appear in the folder. For each
batch of Changes on the Change feed, `sync` reads each Path's Store state again and reconciles it.
Where the local file is unchanged since its Base (or absent with no Base), the Store's version is
applied and becomes the Base. A Resync re-reads the whole Area. Local edits are never touched:
with a local change, `sync` leaves the Path alone for now, and Divergence is ticket 05.

**Blocked by:** 02

**Status:** ready-for-agent

- [ ] A File another process writes appears in the folder (*created* or *updated*), and one it
      deletes disappears (*removed*). Each is followed by *caught up*.
- [ ] Files are written to `.tidings/tmp/`, forced to disk, and renamed into place, so they are
      never seen half-written.
- [ ] Removing a File also removes each directory the removal emptied, stopping at any holding
      other files (tracked, untracked or ignored). Within one reconcile, removals happen before
      writes, so a File can take the place of a directory, and the reverse.
- [ ] Only Changes to the Working copy's Area are acted on.
- [ ] The Change feed is taken before the first full reconcile, so nothing committed during it is
      missed.
- [ ] A Resync for the Area prints *resync* and reconciles every Path.
- [ ] The person's own `tidings commit` produces no *updated* line in `sync`, since the Store's
      Revision already matches the new Base.
- [ ] A local change to a Path is left untouched when the Store changes it. (Marking it Diverged
      is ticket 05.)
- [ ] `sync --quiet` prints only *diverged*, *resync* and errors.
- [ ] Ctrl-C is acted on between reconciles, never during one.
- [ ] Clearing a Cache Area (its Files removed) removes the unchanged local files.
- [ ] Tests, through the binary, on fs and SQLite, with changes made by `tidings store write` and
      `tidings store delete` against the same Root override while `sync` runs.
