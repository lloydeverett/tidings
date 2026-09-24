---
status: accepted
---

# Our own crate, not a wrapper around OpenDAL

Apache OpenDAL is the closest existing abstraction. It is async, it has filesystem and SQLite
backends, and it has a blocking wrapper. But the two things tidings exists for are all-or-nothing
Commits across several Files and a Change feed. OpenDAL can express neither: its SQLite backend is
a plain key/value table, and it has no way to watch for changes. Wrapping it would mean working
around it rather than building on it. So tidings implements its Backends directly on `rusqlite`,
`tokio::fs` and `notify`.

## Considered options

- **OpenDAL**: rejected for the reasons above. It would only be worth it for its dozens of cloud
  backends, which tidings does not need.
- **fs-transaction** (journaled multi-file commits on the filesystem): too new to depend on (0.3.0,
  September 2026), and it cannot watch for changes. Still worth reading when building the
  filesystem Backend's journal.
- **vfs**, **any-storage**: no transactions and no watching. The async part of `vfs` is being
  retired along with async-std.
