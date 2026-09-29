# 06: Commit: Conflicts, Divergence and naming paths

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** `commit` can commit only some paths, and it handles Divergence properly. A
Conflict marks its Paths Diverged, exactly as `sync` would have, so the person can merge without
`sync` running. A full commit is refused up front while anything is Diverged.

**Blocked by:** 05

**Status:** done

- [x] `tidings commit <paths…>` commits only those. Paths are relative to the current directory,
      and a directory means everything under it.
- [x] A path outside the Working copy, or inside its `.tidings/`, is refused.
- [x] A full commit while any Path is Diverged is refused before committing, exits 3, lists the
      Diverged Paths, and says to `resolve` or `discard` them or name other paths.
- [x] A commit naming a Diverged Path is refused the same way. One naming only Paths that aren't
      Diverged goes ahead.
- [x] On a Conflict, nothing is committed. Each conflicting Path is reconciled: Diverged with
      `theirs` written, or given the new Base if its contents turn out equal. The command exits 3,
      naming them.
- [x] `Error::Pending` updates the Bases as a success does, says the Commit happened but isn't
      finished, and exits 0.
- [x] Tests, through the binary, on fs and SQLite, with `sync` both running and not running.
