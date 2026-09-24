# 12: Blocking API

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** An app written as synchronous code can use tidings without its own async
runtime. It gets a blocking Store with the same operations and behaviour as the async one, and it
can wait for the Change feed on a plain thread.

**Blocked by:** 04, 05, 06

**Status:** ready-for-agent

- [ ] Behind the `blocking` feature, `blocking::Store` runs its own internal tokio runtime (as
      `reqwest::blocking` does). It has a constructor for each Backend, and every Store and
      Snapshot operation.
- [ ] The Change feed can be waited on in a blocking way, and it ends the same way as the async
      one.
- [ ] Calling the blocking API from inside an async runtime panics with a clear message.
- [ ] The shared suite also runs through the blocking Store, on every Backend compiled in.
- [ ] A blocking Snapshot holds the internal runtime itself (for example an `Arc` of it), not the
      blocking Store. Holding the Store would keep the Change feed open while the Snapshot is
      held, and holding nothing would leave it unable to read once the Store is dropped (ticket
      06 decided a Snapshot outlives its Store without keeping the feed open).
- [ ] `blocking::Store` doesn't shut its runtime down while a Commit is in flight. A Commit whose
      future is dropped once it has started finishes in a task on the runtime (ticket 07's
      review). If the runtime is gone by then, the Commit can be applied without its Changes
      being reported.

**Notes from ticket 11:**

- The filesystem watcher, like SQLite's poller, is a task on the Store's runtime that needs tokio's
  timers and blocking threads, and runs between calls into the Store. So the internal runtime must
  keep running while no blocking call is in progress: a multi-threaded runtime, or one driven by a
  thread of its own, not a current-thread runtime driven only inside `block_on`. Otherwise edits
  and other processes' Commits would reach the Change feed only while the app is inside a call.
