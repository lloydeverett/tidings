# 10: Surviving crashes

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** An interrupted `sync` or `commit` never corrupts a Working copy. The folder is
always changed before the record is saved, and the record is always replaced whole. So after a
crash, a Path whose local contents already equal the Store's simply takes the Store's version as
its Base on the next command, with nothing reported. A record this version doesn't understand is
refused rather than guessed at.

**Blocked by:** 04, 05

**Status:** ready-for-agent

- [ ] Killing `sync` (SIGKILL) while it applies Store changes, then resuming it, leaves the folder
      matching the Store with no *diverged* or spurious lines.
- [ ] A commit that happened but whose record update was lost (the Store holds the committed
      contents, the record still has the old Base) is healed by the next command, with no Conflict
      and no Divergence.
- [ ] A file written into the folder but not yet in the record is healed the same way.
- [ ] Leftovers in `.tidings/tmp/` from an interrupted run are removed on the next `sync`.
- [ ] A record whose first line names an unknown format or version is refused, and left as it is.
- [ ] Tests go through the binary where it can reach the state (by killing it, or by making the
      folder and record disagree the way a crash would). In-process tests of the Working copy
      module are added only for exact points the binary can't hit, such as between changing the
      folder and saving the record.
