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
- [ ] A filesystem Commit whose future is dropped once it has started runs to completion in the
      background, and its Changes still arrive on the feed, tested with the pause point. The
      mechanism is the Store layer's and already exists (ticket 07's review): a Commit dropped
      once it has its turn is handed to a task that finishes it, and the shared suite's
      `a_cancelled_commit_finishes_or_never_happens_and_is_reported_if_it_finishes` covers it on
      every Backend. The pause point lets a test drop the future mid-journal for certain.
- [ ] Journal recovery is logged through `tracing` at debug level.

**Notes from ticket 09:**

- Failure points already exist in a small form: with `testing`, `FsOptions::fail_at(FailurePoint)`
  stops every Commit through that Store at the point, as if the process had died (it gives
  `Backend` and leaves the disk as it is). `FailurePoint` is `#[non_exhaustive]` and, after the
  ticket 09 review, has `AfterPreparedJournal`, `AfterTemporaryFile(n)`, `AfterCommittedJournal`,
  `AfterDeletes` and `AfterRename(n)`; add the rest as variants. They exist only with `testing`,
  and are checked with `AreaRoot::stop_at` in src/backend/fs.rs and `Journal::finish` in
  src/backend/fs/journal.rs. Finishing a Commit left behind never stops. It is per Store, so crash
  tests can run in parallel. Replace it with the `fail` crate only if that is better for some
  reason.
- Recovery already runs on `open` and at the start of every Commit, under the lock, and logs at
  debug level. Finishing a journal again leaves the Area as finishing it once did (ADR 0005). A step after `committed` that fails currently
  gives `Backend` and leaves the journal for the next Commit or `open`: that is where `Pending`
  and the retries go. Reads don't yet look at a committed journal.
- A Commit that only deletes writes its journal once, as `committed`, so `AfterPreparedJournal`
  and `AfterTemporaryFile` never stop it.
