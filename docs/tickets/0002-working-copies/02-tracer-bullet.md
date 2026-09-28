# 02: Tracer bullet: sync a new Working copy, then commit it back

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** The thinnest path through a Working copy. `tidings sync <area> [folder]`, with
the Store chosen by the usual flags, turns an empty or missing folder into a Working copy: it
writes every File in the Area into it as a plain file, records the Store, the Area and each Path's
Base in `.tidings/`, reports *caught up*, and runs until Ctrl-C. Following later Store Changes is
ticket 03. The person edits, adds and deletes files with ordinary tools, then `tidings commit`,
run from anywhere inside the folder or with `-C <folder>`, commits all of it as one Commit. Each
change requires its File to be unchanged since its Base, or still absent for a new file. The
committed Revisions become the new Bases. This ticket also builds the Working copy module that
later tickets extend, and a test-harness helper for driving `sync`.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `tidings sync config <folder>` on a missing or empty folder creates it with `.tidings/`,
      writes each File of the Area at its Path (making directories as needed), prints a *created*
      line per File and then *caught up*, and keeps running until Ctrl-C, when it exits 0.
- [ ] With `--json`, each event is one JSON object per line with an `event` field, in the style
      of `tidings store watch`.
- [ ] The folder defaults to the current directory.
- [ ] While `sync` runs it holds `.tidings/sync.lock`. Refusing a second `sync` is ticket 04.
- [ ] The record is a versioned text file in `.tidings/` holding the Store's location (an absolute
      Root override or an App identity), the Backend, the Area, and for each Path its Base
      Revision and a hash of its contents. It is replaced whole on each save.
- [ ] `tidings commit`, with `sync` running or not, commits every modified, added and deleted file
      in one Staging: *added* requires **absent**; *modified* and *deleted* require **unchanged
      since** the Base. It prints what it committed (`--json`: each Path and its new Revision).
- [ ] After a commit, each committed Path's Base is its new Revision (a deleted Path loses its
      entry), and a second `commit` says there is nothing to commit and exits 0.
- [ ] `commit` finds the Working copy by walking up from the current directory, recognising it by
      its record, not just by a `.tidings/` directory. `-C <folder>` names it explicitly. Outside
      any Working copy, it fails saying to `sync` one first or use `-C`.
- [ ] `commit` reopens the Store from the record, with no Store flags needed.
- [ ] `commit` and `sync` both take `.tidings/lock` while they act on the folder or record.
- [ ] A Conflict at commit time commits nothing and exits 3. Marking the Paths Diverged is
      ticket 06.
- [ ] Tests, through the binary, on fs and SQLite: sync then read the folder; edit, add and
      delete then commit and read the Store back; a commit whose file changed in the Store since
      its Base is a Conflict; commit from a subdirectory; commit with `-C`.
- [ ] The test harness can spawn `sync --json`, wait (with a timeout) for an event such as
      *caught up*, and stop it with SIGINT.
