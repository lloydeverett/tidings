# 06: Snapshots

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0006](../../adr/0006-no-snapshots-on-the-filesystem.md)

**What to build:** An app can take a Snapshot of an Area and read several Files through it
without mixing the results of different Commits. It can check when it starts whether the Store
supports Snapshots. Works on the memory Backend. The filesystem refusal comes with ticket 09.

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] `supports_snapshots()` returns whether the Store's Backend provides Snapshots. It is `true`
      on memory.
- [ ] `snapshot(area)` returns a Snapshot with `read`, `stat` and `list`.
- [ ] A Commit made while a Snapshot is held is not visible through it, and is visible through
      the Store.
- [ ] Holding a Snapshot does not block Commits.
- [ ] A Backend without Snapshots returns `Unsupported` from `snapshot`. The error variant and the
      shared test for it exist, and the test is skipped on Backends that do support Snapshots.
