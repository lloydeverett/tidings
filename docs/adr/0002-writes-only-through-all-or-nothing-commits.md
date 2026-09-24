---
status: accepted
---

# Writes happen only through all-or-nothing Commits, on every Backend

Nothing can be written directly. An application builds a Staging for one Area and commits it, and
the Commit either applies every staged write and delete or none of them, whichever Backend is
underneath. SQLite gives us this for free. The filesystem does not, so the filesystem Backend
keeps a journal and a lock in a reserved `.tidings/` directory at the root of each Area
([ADR 0005](0005-the-filesystem-journal.md)). Recovery from an interrupted Commit happens inside
`open`.

We accepted the cost of the journal because a weaker guarantee on the filesystem would make the
abstraction leak: code that is correct on SQLite would be quietly wrong on the filesystem.

## Consequences

- A Commit covers one Area. Committing across Areas would mean coordinating separate databases or
  directories, and no use for it has come up.
- The `.tidings/` Prefix is reserved in every Area. Paths under it cannot be written, they are
  left out of listings, and changes to them are never reported.
- Other programs watching the directory while a filesystem Commit is applied may briefly see some
  Files updated and others not. The all-or-nothing guarantee covers crashes and readers that go
  through tidings, not other programs.
