# 08: SQLite backend: other processes' Commits

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** When two copies of an app share a SQLite Store, each sees the other's Commits
as external Changes. Each Change names exactly the Paths that changed, and each Commit's Changes
arrive in one batch.

**Blocked by:** 05, 07

**Status:** ready-for-agent

- [ ] Every Commit appends to a change log table, in the same transaction, recording the Paths,
      the kind and which Store instance wrote it.
- [ ] Other processes' Commits are noticed by polling SQLite's `data_version` or by watching the
      database files, whichever works better. The poll interval is in `SqliteOptions`.
- [ ] Changes from the log are tagged *local* when this Store instance wrote them, and *external*
      otherwise.
- [ ] A test opens a second Store on the same Root override, which stands in for another
      process. It checks that Commits from either Store arrive on the other's Change feed as
      external, one batch per Commit.
- [ ] The change log is pruned so it doesn't grow without limit.
- [ ] Concurrent Commits from two Stores are applied one at a time, and Preconditions stay exact.
