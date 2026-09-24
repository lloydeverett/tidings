# 01: Tracer bullet: write, commit, read and see the Change, in memory

Spec: [0001](../../specs/0001-first-version.md)

**What to build:** The thinnest path through the whole crate. An app opens a memory Store and gets
back the Store and its Change feed. It stages a write to a Path in one Area, commits it, and reads
the File back: contents, last-modified time and Revision. A local Change for that Path arrives on
the feed. This ticket also sets up the Cargo project and the shared backend test suite that every
later ticket adds to and every Backend must pass.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] Cargo project: edition 2024, `rust-version` 1.94, Apache-2.0. The `fs`, `sqlite` (both on by
      default), `blocking` and `testing` features are declared, even if still empty.
- [x] `Area` is a closed enum of Config, Data and Cache.
- [x] `Path` exists as a type that can only be created by validation. Minimal checks for now:
      relative, `/`-separated, non-empty segments. The full rules are ticket 02.
- [x] `Store::open_memory()` returns the Store and its Change feed.
- [x] A Staging for one Area can hold writes, and is built without borrowing the Store.
      Committing it applies the writes. A Staging that is dropped writes nothing.
- [x] `read` returns the File with its contents, a `jiff::Timestamp` last-modified time, and a
      Revision that is a hash of the contents.
- [x] Each Commit produces one batch on the Change feed containing a *changed*, *local* Change
      for each Path it wrote.
- [x] The error type exists as `#[non_exhaustive]`, with the variants this ticket needs.
- [x] The shared backend test suite is set up so it can later run on every Backend, and runs on
      memory. It tests only through the public API.
