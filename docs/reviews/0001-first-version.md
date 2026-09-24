# Code reviews: spec 0001, first version

Every ticket in [docs/tickets/0001-first-version](../tickets/0001-first-version) was reviewed after
it was implemented, using the two-axis `/mattpocock-skills:code-review`. It checks two things:
**Standards** (the repo's documented conventions plus the Fowler smell baseline) and **Spec**
(the ticket and spec 0001). The two axes are reported separately and not ranked against each
other. Each entry ends with a **Resolution** section recording what the implementer fixed or
chose not to fix, and why.

---

## Ticket 01: Tracer bullet

Reviewed: `git diff 9fd4f71...3891a73` (commit 3891a73).

### Standards

### (a) Documented-standard violations

No hard violations.

- **CONTEXT.md terms:** every `_Avoid_` word was grepped across src/ and tests/. The only hits are "batch" (Avoid for Staging) and "applies" (Avoid for Commit). "batch" (change.rs:44,73, suite.rs:103-169) means a batch of Changes on the feed, which is the spec's own term for it. "Applies" (store.rs:43) matches the glossary's own definition of Commit ("Applying a Staging"). Neither is a breach.
- **Spec, Implementation Decisions:**
  - Backends are private: `mod backend` is private, and `Backend` is a `pub(crate)` enum with no public trait.
  - There is one `#[non_exhaustive]` `Error` (error.rs:4-15).
  - The Store chooses the Commit timestamp (store.rs:48).
  - All four features are declared, with `fs` and `sqlite` on by default (Cargo.toml:23-32).
  - The memory Backend is always compiled in.
- **Spec, Testing Decisions:** the suite uses only the public API (suite.rs) and is instantiable per Backend through a Fixture.
- **tracing:** not yet a dependency, but nothing logs, so not a breach.
- **README:** the Status line was updated (README.md:7-8) and is accurate.

### (b) Baseline smells (all judgement calls)

- **Possible Speculative Generality, path.rs:59-118:** there are five ways to do one conversion: `TryFrom<&str>`, `TryFrom<String>`, `FromStr`, `AsRef<str>`, and a sealed `IntoPath` with 5 impls, all delegating to `Path::new`. No spec line or ticket asks for `IntoPath`, `FromStr` or `TryFrom`, and `File::into_contents` (file.rs:36) is also unrequested. Suggestion: keep `IntoPath` because it makes call sites more ergonomic, and drop the unused `TryFrom`/`FromStr` until something needs them.
- **Possible Primitive Obsession, error.rs:12-14:** `reason: String`. path.rs:29 returns a fixed set of `&'static str` reasons, which get turned into `String`. A small reason enum would let callers and tests match on the rule, which the Path-validation table in ticket 02 will want.
- **Possible Repeated Switches / Middle Man, backend/mod.rs:31-43:** each method is a pure delegation, `match self { Backend::Memory(b) => Ok(b.x(..)) }`. The seam is justified because tickets 07 and 09 add Backends, but the match will repeat in every method as variants arrive. A private trait (the spec forbids only a public one) or a single dispatch point would help later. Not actionable yet.
- **Minor Duplicated Code, backend/memory.rs:32-46:** `Areas::get` and `Areas::get_mut` repeat the same match. A `[BTreeMap; 3]` indexed by Area would remove it. Low value.
- **Minor Duplicated Code, tests/behaviour/suite.rs:53:** repeats `store.read(..).await.unwrap().unwrap()` inline even though a `read` helper exists (suite.rs:115). The helpers `next_item`, `next_batch` and `read` (suite.rs:95-121) sit between tests rather than grouped together.
- **Not speculative:** `Written` carries Revisions that `Store::commit` discards (store.rs:51, `into_keys`). Ticket 03 needs them ("A successful Commit returns ... the new Revision of each Path"), so this is a legitimate seam.

Naming otherwise follows the glossary consistently. Stored, CommitRequest, FeedItem and announce don't collide with any Avoid list.

### Spec

All nine ticked boxes in `docs/tickets/0001-first-version/01-tracer-bullet.md` hold up. `cargo test` passes all 7 behaviour tests with default features, with `--no-default-features` and with `--all-features`.

What was checked:
- **Cargo.toml:4-19:** edition 2024, `rust-version` 1.94, Apache-2.0. The four features are declared and empty, with `default = ["fs","sqlite"]`.
- **Area:** a closed enum of Config, Data and Cache (`src/area.rs:6`).
- **Path:** the inner field is private, and the only way to build one is `Path::new` with its validation (`src/path.rs:14-38`). The minimal checks are in place: relative, `/`-separated, no empty segments. `.` and `..` are still accepted, which is left to ticket 02.
- **Store:** `Store::open_memory() -> (Store, ChangeFeed)` (`src/store.rs:29`).
- **Staging:** an owned value that doesn't borrow the Store and is consumed by `commit`. A dropped Staging writes nothing. Tested at `tests/behaviour/suite.rs:147,156`.
- **Reading:** `read` gives contents, a `jiff::Timestamp` and a Revision. The Revision is the XXH3-128 hash of the contents (`src/revision.rs:13-15`), which matches "a fast, established 128-bit hash". The Store layer picks one timestamp per Commit (`src/store.rs:48`).
- **Change feed:** one batch of *changed*/*local* Changes per Commit (`src/store.rs:49-54`).
- **Errors:** a `#[non_exhaustive]` error type with `InvalidPath` (`src/error.rs:6-15`).
- **Test suite:** one `Fixture`-based suite that uses only the public API and runs on memory (`tests/behaviour/main.rs`).

**(a) Missing or partial**
- "Each Commit produces one batch on the Change feed…" is only partly tested. `a_commit_announces_one_batch_of_local_changes` (`suite.rs:123-143`) checks the first item only. It never shows that no second batch follows, so a Commit split across two batches would still pass. The code is correct (it calls `announce` once); this is a gap in the test, not a bug.

**(b) Scope creep** (none of it substantial)
- `IntoPath`, a public sealed trait, and `TryFrom`/`FromStr` for Path (`src/path.rs:60-127`) weren't asked for. They are harmless and support "checked when I use them".
- `Staging` derives `Clone` (`src/staging.rs:10`). The spec says it "is consumed by `commit`", and a clone lets the same staged writes be committed twice. Probably fine, but no spec line asks for it.
- `Staging::area()` is public. The spec says "The Staging cannot be read from". It exposes only the Area, not the staged contents, so this is borderline at most.

**(c) Implemented but looks wrong**
- Nothing contradicts ticket 01. Three stand-ins are knowingly temporary and owned by later tickets or spec lines:
  - `commit` returns `Result<()>` (`src/store.rs:45`), where the spec's "Commit result: … the new Revision of each Path written" is ticket 03's.
  - The feed is an unbounded mpsc (`src/change.rs:85`), where the spec's "merged per Path … never uses unbounded memory" is ticket 05's.
  - `commit` runs inline rather than as a background task. The spec says "once started, a Commit continues in a background task", and cancellation is covered elsewhere in the spec.

**Verdict:** ticket 01 matches its spec. The one-batch test in (a) is the only thing to act on.

### Summary

Standards: 0 hard violations and 5 judgement-call smells. The most notable is possible Speculative Generality in Path's conversions. Spec: 1 partial item (the one-batch test doesn't rule out a second batch) and 3 minor scope-creep notes. The worst is the untested "never split across batches" guarantee.


### Resolution

1. **Spec (a), one-batch test only looked at the first item:** fixed.
   `a_commit_announces_one_batch_of_local_changes` now also checks, with a new
   `assert_nothing_more` helper, that nothing else arrives on the feed within 200 ms. I checked the
   test by temporarily announcing each Change as its own batch, and it failed.
2. **Spec (b), `Staging` derived `Clone`:** fixed. The derive is gone, since nothing needed it. A
   Staging can only be consumed by `commit`.
3. **Speculative Generality, Path conversions:** fixed. `TryFrom<&str>`, `TryFrom<String>`,
   `FromStr` and `AsRef<str>` are removed. `Path::new`, `Path::as_str` and the sealed `IntoPath`
   stay. `File::into_contents` is removed too, because nothing asked for it.
4. **Duplicated Code in suite.rs:** fixed. `a_committed_write_can_be_read_back` uses the `read`
   helper, and all the helpers (`next_item`, `next_batch`, `read`, `assert_nothing_more`) are now
   grouped at the end of the file.
5. **Primitive Obsession, `InvalidPath { reason: String }`:** fixed now rather than left to
   ticket 02. The error is now `InvalidPath { path, reason: InvalidPathReason }`, where
   `InvalidPathReason` is a public `#[non_exhaustive]` enum (`Empty`, `NotRelative`,
   `EmptySegment`) with a `Display`. Fixing the error's shape now means ticket 02 only adds
   variants, and its table test can match on the rule. The suite's invalid-Path test already
   asserts the reason for each refused string.
6. **Duplicated Code, `Areas::get`/`get_mut`:** fixed. The memory Backend keeps
   `[BTreeMap<Path, Stored>; 3]` indexed by `area as usize`, so the two matches are gone.

Not acted on, as triaged:
- The Backend dispatch match (Repeated Switches / Middle Man) waits until a second Backend
  exists (tickets 07 and 09).
- `Staging::area()` stays public. It exposes the Area, not the staged contents.
- The temporary stand-ins stay: `commit` returning `()` (ticket 03), the unbounded mpsc feed
  (ticket 05), and running `commit` inline rather than as a background task (ticket 10).
