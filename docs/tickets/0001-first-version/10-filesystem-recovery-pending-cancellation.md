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

**Status:** done

- [x] Named failure points exist only with the `testing` feature, using the `fail` crate or a
      small equivalent. The points are:
      - after the `prepared` journal is written;
      - after each temporary file is written;
      - after the `committed` journal is written;
      - after each rename;
      - a rename that fails;
      - a pause point for testing cancellation.
- [x] For every point, a test stops the Commit there, drops the Store without cleanup, opens it
      again, and checks through the public API that the Commit is fully applied or fully absent,
      with no temporary files left over.
- [x] A rename that keeps failing after brief retries gives `Pending`. Until the renames are
      finished, reads of the affected Paths return the committed contents. The next Commit or
      `open` finishes the renames.
- [x] A filesystem Commit whose future is dropped once it has started runs to completion in the
      background, and its Changes still arrive on the feed, tested with the pause point. The
      mechanism is the Store layer's and already exists (ticket 07's review): a Commit dropped
      once it has its turn is handed to a task that finishes it, and the shared suite's
      `a_cancelled_commit_finishes_or_never_happens_and_is_reported_if_it_finishes` covers it on
      every Backend. The pause point lets a test drop the future mid-journal for certain.
- [x] Journal recovery is logged through `tracing` at debug level.

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

**Notes from ticket 10:**

- **Failure points.** Still the small per-Store equivalent of the `fail` crate from ticket 09,
  with `testing` only. `FailurePoint::RenameFails { n, times }` makes the rename of the Commit's
  `n`th write fail the first `times` times it is tried through the Store, counting retries and
  the Commits and `open` that finish it later (`usize::MAX` keeps failing). The pause point is
  `FsOptions::pause_at(point, &Pause)`: each Commit through the Store waits at `point` (any place
  `fail_at` can stop) until `Pause::release`, and `Pause::reached().await` says one is there. A
  Commit held for 30 seconds panics, so a test that never releases it fails rather than hangs.
  After the review, `FailurePoint::CommittedJournalFails` makes writing the `committed` journal
  report an error once it is written.
- **The crash matrix** replaces ticket 09's three piecemeal crash tests. `stop_everywhere(shape)`
  runs one shape of Commit through every point (`AfterPreparedJournal`, each
  `AfterTemporaryFile(n)`, `CommittedJournalFails`, `AfterCommittedJournal`, `AfterDeletes`,
  each `AfterRename(n)` and each `RenameFails { n, usize::MAX }`), each on a fresh Area. Each
  point's result is exact: the failure point's own stop, `Pending`, or success. What "all there"
  looks like comes from committing the shape without stopping; "not there" is what the Area held
  before. Both are compared through `list`, `read`, `stat` and `stat_prefix`. It checks
  a Store that was open already (as in another process) before anything finishes the Commit, then
  a Store opened afterwards, no temporary files, and that every symlink is still a symlink. The
  shapes: a write (one File replaced, one in a new directory), a write and a delete, a Prefix
  delete (journal written once), the moves `a` → `a/b` and `d/e` → `d`, and a write through a
  symlink (Unix). The pause point isn't in the matrix: a held Commit can't be dropped without its
  thread going on, so it is tested by the cancellation test instead, and stopping at the same
  places is what the matrix does.
- **`Pending`.** Finishing is retried after 10, 50 and 200 ms, as finishing again (Revision-guarded
  deletes, renamed temporary files skipped). A stop at a failure point is never retried (it
  models the process dying). Then the Backend returns its `CommitOutcome` with `pending: true`,
  the Store layer records the Changes and gives `Error::Pending`, so the Changes arrive once,
  including from a dropped Commit's task. Finishing it later reports nothing.
- **Reads while a `committed` journal is there** (`AsFinished` in journal.rs): a Path the Commit
  writes is read from its temporary file until that is renamed, and a Path it deletes is absent if
  the File there has the journaled Revision. `list` and `stat_prefix` apply the same. This also
  covers a Commit that crashed after `committed` and a Commit being finished right then. The
  journal's `replace` line now records the Path too, so that a write through a symlink is found by
  its Path. Preconditions need none of it, since a Commit finishes the journal first.
- **Choices recorded in ADR 0005:** only the Commit's own Paths are looked up, so another Path that
  is the same file through a symlink reads as on disk until it is finished (README Limitations).
  A next Commit that still can't finish the journal gives `Backend` and isn't made, rather than
  `Pending`, which would say it had happened. `open` logs that and opens anyway; only a journal
  that can't be read stops it.
- **Logging.** Discarding or finishing a journal left behind, each retry, a `Pending` Commit and a
  journal `open` leaves unfinished are logged at debug level. There is no test of the logging.
- **Writing the `committed` journal can report a failure after its rename landed** (the
  directory fsync failing, say). After the review, the journal is read back then: if it is
  `committed`, the Commit goes on to be finished and reported; otherwise it is discarded and gives
  the error. Only if the journal can't be read back either are the Changes of a Commit that
  happened lost.
- **The journal format** is `tidings journal 2` after the review, since the `replace` line changed.
