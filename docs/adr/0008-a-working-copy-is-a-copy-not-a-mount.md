---
status: accepted, amended by ADR 0009
---

# A Working copy is a copy that never overwrites local edits, not a mount

People want to edit a Store's Files with ordinary tools, such as vim, even when the Store is in
SQLite. The `tidings` command does this with a Working copy: a folder holding one Area\*'s Files as
plain files. `tidings sync` keeps it in step with the Store, and `tidings commit` commits local
edits back as a single Commit. It is not a filesystem mount. Nothing reaches the Store until it is
committed, and syncing never overwrites a local edit. A Path changed on both sides is Diverged,
and stays as you left it until you `resolve` or `discard` it.

We chose this because it keeps the promise of [ADR 0002](0002-writes-only-through-all-or-nothing-commits.md):
writes happen only through all-or-nothing Commits that the person decides on. Each local edit
carries its Base as an *unchanged since* Precondition, so an edit made against a stale copy is a
Conflict, and is never silently lost.

## Considered options

- **A real mount (FUSE, macFUSE, NFS loopback)**: rejected. Every save would become a Commit of
  whatever the editor happened to write, so a save that writes a temporary file and then renames
  it would be two Commits, and there is no point at which a person says "these edits together".
  On macOS it also needs a kernel extension or a local NFS server.
- **Syncing that lets the Store win**: rejected. A Change from another process would overwrite
  an edit in progress.
- **Writing the Store's version beside the local file** (`app.toml.theirs`): rejected. That file
  would be a valid Path, and the next commit would pick it up. The Store's version of a Diverged
  Path goes in the Working copy's `.tidings/theirs/` instead, where no Path can be.
- **One folder for the whole Store, with `config/`, `data/` and `cache/` inside**: rejected. A
  single commit would become up to three Commits, and one could succeed while another is refused.

## Consequences

- A Working copy is for one Area\* of a filesystem or SQLite Store. A memory Store can't be reached
  from another process.
- Committing doesn't need `sync` to be running: the Working copy's record of each Path's Base, and
  of which Store and Area\* it belongs to, is kept in its `.tidings/`. `.tidings` is reserved, so
  that folder can never be a File.
- Working copies are the main way to use the command, so they have the top-level commands (`sync`,
  `commit`, `status`, `discard`, `resolve`), and the commands that act on a Store directly are under
  `tidings store`.
- A Working copy is built only on the library's public API, in the `cli` crate, and is not part of
  the library.

---

\* Areas were later removed: a Store is now one Location the app chooses. See
[ADR 0009](0009-a-store-is-one-location-the-app-chooses.md).
