# 04: Resuming, and refusing the wrong folder or Store

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** A Working copy lasts across restarts. `sync` on an existing Working copy
resumes it, catching up on everything the Store did while it wasn't running. Every way of pointing
`sync` or another command at the wrong thing is refused with a clear reason.

**Blocked by:** 03

**Status:** done

- [x] `sync` on an existing Working copy, with or without Store flags, reconciles the whole Area
      and then follows it: Files changed or removed in the Store meanwhile are applied to unchanged
      local files.
- [x] A later `sync`, `commit` and every other Working copy command reopen the Store from the
      record. Store flags (or `TIDINGS_ROOT`, `TIDINGS_IDENTITY`, `TIDINGS_BACKEND`) that don't
      match the record are refused, naming the difference.
- [x] `sync` for a different Area than the record's is refused.
- [x] A folder that isn't empty and isn't a Working copy is refused, and nothing in it is changed.
- [x] A second `sync` of a Working copy that is already being synced is refused, saying one is
      running.
- [x] `sync` on a memory Store is refused, saying no other process could reach it.
- [x] A Working copy keeps working after its folder is moved or renamed.
- [x] A Working copy whose Store has gone (such as a Root override that was removed) fails to open
      with a clear error, and never creates a new Store without `--create`.
- [x] Two Working copies of the same Area both follow it, independently.
- [x] Tests, through the binary, on fs and SQLite.
