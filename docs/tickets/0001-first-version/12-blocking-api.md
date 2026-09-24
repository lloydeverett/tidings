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
