# 10: Filesystem backend: recovery, `Pending` and cancellation

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0005](../../adr/0005-the-filesystem-journal.md)

**What to build:** A filesystem Commit is all-or-nothing however it is interrupted:
- **A crash at any step.** After a crash, opening the Store leaves the Commit either fully there
  or fully absent.
- **A rename that keeps failing** (as on Windows when another program has the File open). The
  app gets `Pending`, reads through tidings show the committed contents, and the Commit is
  finished later.
- **A Commit cancelled once started.** It still finishes.

Named failure points in test builds make every one of these cases testable on Linux.

**Blocked by:** 09

**Status:** ready-for-agent

- [ ] Named failure points exist only with the `testing` feature, using the `fail` crate or a
      small equivalent. The points are:
      - after the `prepared` journal is written;
      - after each temporary file is written;
      - after the `committed` journal is written;
      - after each rename;
      - a rename that fails;
      - a pause point for testing cancellation.
- [ ] For every point, a test stops the Commit there, drops the Store without cleanup, opens it
      again, and checks through the public API that the Commit is fully applied or fully absent,
      with no temporary files left over.
- [ ] A rename that keeps failing after brief retries gives `Pending`. Until the renames are
      finished, reads of the affected Paths return the committed contents. The next Commit or
      `open` finishes the renames.
- [ ] A Commit whose future is dropped once it has started runs to completion in the background.
      Its Changes still arrive on the feed.
- [ ] Journal recovery is logged through `tracing` at debug level.
