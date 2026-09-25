# 12: Blocking API

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** An app written as synchronous code can use tidings without its own async
runtime. It gets a blocking Store with the same operations and behaviour as the async one, and it
can wait for the Change feed on a plain thread.

**Blocked by:** 04, 05, 06

**Status:** done

- [x] Behind the `blocking` feature, `blocking::Store` runs its own internal tokio runtime (as
      `reqwest::blocking` does). It has a constructor for each Backend, and every Store and
      Snapshot operation.
- [x] The Change feed can be waited on in a blocking way, and it ends the same way as the async
      one.
- [x] Calling the blocking API from inside an async runtime panics with a clear message.
- [x] The shared suite also runs through the blocking Store, on every Backend compiled in.
- [x] A blocking Snapshot holds the internal runtime itself (for example an `Arc` of it), not the
      blocking Store. Holding the Store would keep the Change feed open while the Snapshot is
      held, and holding nothing would leave it unable to read once the Store is dropped (ticket
      06 decided a Snapshot outlives its Store without keeping the feed open).
- [x] `blocking::Store` doesn't shut its runtime down while a Commit is in flight. A Commit whose
      future is dropped once it has started finishes in a task on the runtime (ticket 07's
      review). If the runtime is gone by then, the Commit can be applied without its Changes
      being reported.

**Notes from ticket 11:**

- The filesystem watcher, like SQLite's poller, is a task on the Store's runtime that needs tokio's
  timers and blocking threads, and runs between calls into the Store. So the internal runtime must
  keep running while no blocking call is in progress: a multi-threaded runtime, or one driven by a
  thread of its own, not a current-thread runtime driven only inside `block_on`. Otherwise edits
  and other processes' Commits would reach the Change feed only while the app is inside a call.

**Notes from building it:**

- `blocking::Store` holds the async Store and an `Arc` of the internal runtime: multi-threaded,
  one worker, so the SQLite poller and the filesystem watcher run between calls
  (`tests/blocking.rs` checks both give their Changes with no call in progress, and fails with a
  current-thread runtime). Clones, blocking Snapshots and the blocking Change feed share the
  `Arc`, and the last to go shuts the runtime down, waiting for its threads, or, dropped inside an
  async runtime, where tokio forbids waiting, in the background.
- In flight Commits: the blocking API can't cancel a Commit, since `commit` waits in `block_on`,
  and the call holds its handle, so the runtime can't stop before the Commit is finished and
  reported. `a_commit_in_progress_finishes_and_is_reported_when_every_other_handle_is_dropped`
  pauses one on the filesystem, drops every other handle, and checks it.
- The Change feed is an `Iterator` (waits for each item, ends as the async one does) with
  `next_timeout(Duration) -> Result<Option<FeedItem>, TimedOut>` for waiting a while only.
- Every method panics inside a tokio runtime with one message, `#[track_caller]`, including
  `supports_snapshots` and `inject_external_change`, which don't wait: one simple rule.
- **Running the shared suite through it.** The suite's tests were written against
  `tidings::Store`. They now use `tests/behaviour/api.rs`'s `Store`, `Snapshot` and `ChangeFeed`,
  enums with the async API's methods that call either the async API or the blocking one. Through
  the blocking one, each call, and dropping, is made on a plain scoped thread (under
  `block_in_place` on a multi-threaded test runtime), since the blocking API panics inside a
  runtime. The feed helpers in `tests/common` wait through a `Feed::next_within(timeout)`, which
  the blocking feed does with `next_timeout`: a blocking wait can't be cancelled, so wrapping it in
  `tokio::time::timeout` would leave a thread that takes the next item and loses it. Each
  Backend's module in `tests/behaviour/main.rs` has a `blocking` module that opens through the
  blocking constructors and instantiates the suite (and, on SQLite and the filesystem, the
  two-Store groups it runs async). The dev-dependency now enables `blocking`, as it does
  `testing`, so a plain `cargo test` runs it. Backend-specific tests in `main.rs` (failure points,
  watching, pruning) stay async-only.
- No test is skipped for the blocking fixtures. One is weaker there:
  `a_cancelled_commit_finishes_or_never_happens_and_is_reported_if_it_finishes` finishes each
  Commit in its first poll, since a blocking Commit can't be cancelled, so it only checks that each
  is reported, as on memory.
- Seen once, not reproduced: in one full `cargo test` run,
  `fs::blocking::commits_from_both_stores_reach_each_feed_in_the_order_they_were_applied` failed
  its assertion on what a feed last said of the raced Path. It then passed 5 runs out of 5 alone,
  12 with six copies of it and its async twin running at once, and 8 full runs of the suite. That
  test relies on the watcher's timing (another Store's Commits can be split, as the README says),
  and the blocking Store's watcher runs on a one-worker runtime, so heavy load may reach that
  limit there more easily.
