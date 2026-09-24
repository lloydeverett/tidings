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

---

## Ticket 02: Strict Paths

Reviewed: `git diff 958df43...425090d` (commit 425090d).

### Standards

### (a) Documented-standard violations

**No hard violations found.** What was checked:
- **ADR 0004 and the spec's Implementation Decisions (Path module):** structure is checked with `relative-path` (src/path.rs:40-49), each segment with `sanitize-filename` using Windows rules (src/path.rs:63-66), and NFC with `unicode-normalization` (src/path.rs:54). Nothing is hand-rolled except the leading-`/` and `.tidings` checks, which those crates don't cover.
- **The spec's "one `#[non_exhaustive]` error type":** Prefixes reuse `Error::InvalidPath` (src/prefix.rs:21), and `InvalidPathReason` stays `#[non_exhaustive]`.
- **The spec's Testing Decisions:** tests/paths.rs uses only the public `Path::new` and `Prefix::new`, with the accepted/refused table as required. The suite test `every_allowed_path_can_be_written_and_read_back` goes only through the Store API. Nothing inspects internal state.
- **CONTEXT.md `_Avoid_` lists:** "filesystem" and "directory" appear at src/path.rs:69 and tests/paths.rs:~83 ("some filesystems ignore it"), meaning the OS filesystem rather than a Store or Prefix, so they are not breaches. "file name" at src/path.rs:61 describes a single segment, not a Path, so it is acceptable, though close to the avoided "filename"; "a name" would be safer.
- **tracing, features, README:** no tracing added. The Limitations bullet in the README matches the behaviour.
- **clippy:** clean.

### (b) Baseline smells (all judgement calls)

1. **Possible Duplicated Code**, src/prefix.rs:15-101 vs src/path.rs:19-173. Prefix copies nearly all of Path's scaffolding:
   - the same validate-then-wrap `new()`
   - identical `Debug` and `Display` impls
   - `IntoPrefix`, with the same five impls as `IntoPath` (`Self`, `&Self`, `&str`, `String`, `&String`)
   - a second `mod sealed`

   A small `macro_rules!` for the newtype and its `Into*` trait, used by both, would remove this. Minor: Rust newtypes often accept this repetition.
2. **Possible Mysterious Name**, src/prefix.rs:21 `Error::InvalidPath { path: prefix, reason }`. The field is called `path` but can hold a Prefix. The doc comment (src/error.rs:12) was changed to "Path or Prefix" instead of renaming. The ticket fixes `InvalidPath` as the variant, so renaming is optional, but the variant is public, so this is worth settling before 1.0.
3. **Possible Mysterious Name**, src/path.rs:64 `let windows = sanitize_filename::OptionsForCheck { windows: true, truncate: true };`. `windows` names the platform, not what the value holds; `strictest_rules` or `windows_rules` would. Trivial.
4. **Not a smell, but noted:** src/path.rs:45 detects empty segments by comparing `components().count()` with `split('/').count()`. That relies on how `relative-path` treats empty segments. The inline comment explains it, but it is fragile if the crate changes that behaviour. The table tests (`a//b`, `trailing/`) would catch such a change.
5. **Not Speculative Generality:** `Prefix` and `IntoPrefix` have no callers yet, but tickets 03 and 04 (list, delete-prefix) need them, and the ticket asks for them.
6. **Not Duplicated Code:** tests/behaviour/suite.rs:151-165 repeats a subset of the paths.rs table. This is deliberate and commented ("One Path for each rule. tests/paths.rs has the full table."), because the suite checks that each operation applies the rules.

### Spec

All tests pass (`cargo test`), and every checklist item is substantially met. The Path and Prefix types, the five new reasons, the table-driven test (tests/paths.rs) and the behaviour-suite coverage of `read` and `Staging::write` all match the ticket. Letter-case clashes are correctly left to ticket 04.

**(a) Missing or partial**

1. Structure is only partly checked by `relative-path`. The ticket says: *"Structure is checked with `relative-path`. Refused: absolute Paths, empty segments… and a leading `/`"* and *"Established crates do the checking; we don't write the rules ourselves."* At src/path.rs:44-49 the leading `/` is a hand-written `starts_with('/')`, and empty segments are inferred by comparing `components().count()` with `split('/').count()`. Only the detection of `.` and `..` really comes from the crate. This is minor and arguably unavoidable, because `relative-path` normalises these cases instead of rejecting them.
2. Some Windows-reserved names get through, because `sanitize-filename` misses them. A probe showed `Path::new` returns Ok for:
   - `COM¹` and `LPT³` (Windows reserves the superscript-digit forms)
   - `CONIN$` and `CONOUT$`
   - `CON .txt` and `NUL .txt` (Windows probably treats these as devices, since it trims trailing spaces before the extension)
   - `\u{7f}` (DEL), while `\u{85}` is refused, so control characters are handled inconsistently

   The ticket says: *"a Path that works on one platform works on all of them"* and *"Refused, for example: `CON`, `aux.txt`, control characters"*. The gaps are in the crate the ticket requires, so this may be an accepted limitation. If so, it should be recorded, because the README Limitations line (README.md:32-35) currently implies full coverage.

**(b) Scope creep**

3. `NoTrailingSlash` is a new user-facing rule the spec doesn't state (src/prefix.rs:37-42, src/path.rs:97). The spec only says *"A Prefix is checked the same way."* Requiring the trailing `/`, so that `themes` is refused, is a reasonable reading of the glossary (*"The leading part of a Path, up to a `/`"*), and the ticket Notes record it. But it shapes the API for tickets 03 and 04 (`list`, `delete_prefix`, `stat_prefix`, `require_prefix`), and CONTEXT.md's Prefix entry wasn't updated. This is worth confirming with the spec owner; it isn't a defect.

Nothing else goes beyond scope. The `Prefix`/`IntoPrefix` types and the README Limitations note follow from the ticket.

**(c) Implemented but looks wrong**

4. Nothing found in the checks themselves:
   - The order of the checks and the reasons they give are consistent.
   - Matching the reserved `.tidings` (src/path.rs:69-71) correctly catches `.TIDINGS`, `.tidingſ` and `.tıdings`, and allows a deeper `notes/.tidings`, as the ticket Notes say.
   - The NFC check covers the whole Path.
   - Prefixes reuse `refusal` (src/prefix.rs:41), so `.tidings/`, `CON/` and `a/./` are refused.
5. A small test gap. The ticket says *"Every operation that takes a Path or a Prefix returns `InvalidPath`"*, but no operation takes a Prefix yet, so the Prefix route through an operation is untested. Tickets 03 and 04 own that, so this is not a finding against this diff.

### Summary

Standards: 0 hard violations and 3 judgement-call smells. The most notable is possible Duplicated Code between the Path and Prefix scaffolding. Spec: 2 partial items and 1 scope question. The worst is that Windows-reserved names such as `COM¹`, `CONIN$` and `CON .txt` get past `sanitize-filename`, which breaks "a Path that works on one platform works on all of them".



### Resolution

1. **Spec (a2), Windows-reserved names that `sanitize-filename` lets through:** fixed. First I
   checked whether a newer version covers them. The only newer release, 0.7.0-beta, still lists
   only `con`…`lpt9`, and its control-character check still misses DEL. So the crate stays at 0.6
   and remains the primary check. A small, commented supplement in `src/path.rs` covers exactly the
   documented gaps:
   - `DEVICE_NAMES_SANITIZE_FILENAME_MISSES` lists `COM¹²³`, `LPT¹²³`, `CONIN$` and `CONOUT$`.
   - The crate's own check is repeated on the name as Windows reads it, with the extension and
     the spaces before it removed. That refuses `CON .txt` and `NUL .txt`.
   - `char::is_control` refuses DEL (U+007F).

   Every probed case is in the tests/paths.rs table, along with near misses that stay accepted
   (`CONIN.txt`, `not CON.txt`). The ticket notes say the crate is supplemented and why. The
   README Limitations line now names `COM¹` and control characters.
2. **Spec (b3), `NoTrailingSlash`:** the rule stays. CONTEXT.md's **Prefix** entry now says that a
   Prefix is either empty (the whole Area) or ends in `/`, and that `themes` alone is not a
   Prefix.
3. **Standards (b3), the `windows` binding:** fixed. It is now `windows_rules`, inside a new
   `is_sanitized_for_windows` helper that the supplement also uses. The doc comment says "one
   segment of a Path" instead of "file name".
4. **Standards (b1), Path and Prefix duplicate their scaffolding:** left as it is. Each is about
   40 lines of plain newtype code. A `macro_rules!` for two uses would put their docs and impls
   inside generated code that is harder to read and to document. Prefix is also likely to gain
   behaviour of its own in ticket 03 (matching Paths under it), so the two will diverge. If a
   third validated string type appears, a macro becomes worth it.
5. **Standards (b2), `InvalidPath { path }` can hold a Prefix:** left as it is, as the
   coordinator suggested. The docs already say "Path or Prefix", and `InvalidPath` is the name the
   spec and ticket use.

Not acted on, as triaged:
- **Spec (a1), structure partly hand-written:** this can't be avoided. `relative-path` has no
  validating constructor. `RelativePath::new` accepts any string, and `components()` quietly
  skips a leading `/` and empty segments instead of reporting them. So the leading `/` is checked
  with `starts_with`, and empty segments are detected by comparing the crate's segment count with
  `split('/')`. The crate still does the segmenting and classifies `.` and `..`. The table rows
  (`/etc/passwd`, `a//b`, `trailing/`) would catch any change in how the crate behaves
  (Standards b4).
- **Spec (c5), no operation takes a Prefix yet:** tickets 03 and 04 own this, through
  `impl IntoPrefix`.

---

## Ticket 03: The full Staging, and reading

Reviewed: `git diff 821421e...db1e63d` (commit db1e63d).

### Standards

#### Hard violations of documented standards

None found. What was checked:
- **Spec, Implementation Decisions:** Backends stay `pub(crate)` with no public trait, and there are no new error variants.
- **Spec, Testing Decisions:** tests use only the public API and never inspect internal state.
- **Spec, tracing and features:** no logging, so nothing above debug level, and no new features.
- **README:** the Consistency section is updated for "a write that changes nothing is left out" and matches the behaviour.
- **CONTEXT.md `_Avoid_` lists:** "key", "directory", "event", "version" and "transaction" are not used as names for domain concepts. `drafts/` appears only as test data, not as a name for Staging.

#### Judgement calls (possible documented-standard or smell issues)

1. **Uses a word from an `_Avoid_` list** (CONTEXT.md, Commit: "_Avoid_: save, flush, apply"). At `src/backend/mod.rs:33` there is `pub(crate) struct Applied`, and at `memory.rs:56` there is `let mut applied`. Both name the result of a Commit with the avoided word. This is soft, since CONTEXT's own definition uses "Applying" as a verb and the type is crate-private. `CommitOutcome` or `Done` would avoid the word.
2. **Data Clumps.** `area`, `staged` and `prefix_deletes` travel together three times:
   - in `Staging` (`staging.rs:11-17`)
   - through `into_parts() -> (BTreeMap<Path, Staged>, BTreeSet<Prefix>)` (`staging.rs:73`)
   - again in `CommitRequest` (`backend/mod.rs:21-29`)

   Each copy has its own doc comment explaining the ordering. Ticket 04 adds `require` and `require_prefix`, which will widen the clump. Consider making `CommitRequest` hold the consumed `Staging` (or a shared staged-contents type) plus the `timestamp`.
3. **Duplicated Code (incipient).** The "the later staged operation wins" rule is split in two places:
   - `Staging::delete_prefix` removes earlier operations with `retain(|path, _| !prefix.covers(path))` (`staging.rs:67`).
   - The memory Backend expands the Prefix delete with `staged.entry(path.clone()).or_insert(Staged::Delete)` (`memory.rs:51-54`).

   The SQLite and filesystem Backends (tickets 07 and 09) will each have to copy the second half. Consider a single helper on `CommitRequest`, e.g. `expand_prefix_deletes(existing_paths)`, that every Backend calls under its lock.
4. **Primitive Obsession (minor).** `Applied.changes: Vec<(Path, ChangeKind)>` (`backend/mod.rs:38`) is a bare tuple standing in for the Backend's "raw change". The spec's "What every Backend must provide" section gives a raw change a Revision too, and ticket 11 needs it. A small crate-private type now would save reshaping it later.
5. **Duplicated Code (minor).** `Stat` (`file.rs:49-68`) repeats `File`'s `modified` and `revision` fields, their accessors and their doc comments (`file.rs:35-43`). `File` could hold a `Stat`, or build one, so the two can't drift apart.

#### No issues

- `Committed` (`src/committed.rs`), `Store::stat` and `Store::list` (`store.rs:46-58`), and `Prefix::covers` (`prefix.rs:31-35`) are well named and minimal.
- Test names and helpers (`changes`, `list`, `assert_refused`, `tests/behaviour/suite.rs:470-500`) follow the glossary.
- The 5 ms sleep at `suite.rs:254` is justified and acceptable.

### Spec

`cargo test` passes: 19 suite tests and 5 path tests. Every ticked box was checked against the code and tests, and all but one are genuinely met.

**(a) Missing or partial**

1. **The rename is only half tested, and the ticket was reworded to allow it.** Ticket: "Deleting a Path and writing another in the same Commit acts as a rename, and happens all-or-nothing." The implementer added to this criterion that "the 'nothing' half gets its test there" (in ticket 04). But ticket 04's checklist has no such item. Its closest line is "A failed Precondition gives `Conflict { paths }` and nothing is written", which doesn't mention renames. So the deferral is recorded nowhere that will make it happen.
   - It is fair to say the case can't be tested today, because `src/backend/memory.rs:47-76` cannot fail.
   - Fix: add a checkbox to `04-preconditions.md` saying that a rename which hits a Conflict changes nothing.

**(b) Scope creep**

None. `Prefix::covers` (`src/prefix.rs:31-35`), `Stat` and `Committed` are what the ticket's Notes and the spec ask for. The README change follows "Keep them in sync".

**(c) Implemented but possibly wrong**

None found. On the implementer's two recorded decisions:

- **The order in which staged operations apply** (Notes: "the later of a write or delete to the same Path wins … `delete_prefix` drops anything staged under the Prefix before it"). The spec doesn't settle this, and the code does what the Notes say:
  - `src/staging.rs:66-69`: `retain` drops everything already staged under the Prefix.
  - `src/backend/memory.rs:51-54`: the Prefix delete is expanded under the lock with `or_insert`, so a write or delete staged after it wins. This fits "deletes of a Prefix, which are expanded when the Commit is made".
  - It is tested in `a_prefix_delete_and_writes_under_it_apply_in_the_order_staged` in `tests/behaviour/suite.rs`.
  - One risk for ticket 04: once writes and deletes carry Preconditions, a later `delete_prefix` will silently drop the Precondition of an earlier write under that Prefix. Ticket 04 should decide whether that is intended.
- **Revisions for writes that were left out.** The spec says "the new Revision of each Path written", and US38 says "so that I can write them again safely without reading them first". The implementation (`memory.rs:59-64`) returns the Revision even for a left-out write. That Revision equals the stored one, so it serves US38 exactly. This is a sound reading, not a deviation, and `a_write_that_changes_nothing_is_left_out` asserts it.

**Other boxes verified**

- A missing File gives `Ok(None)` from both `read` and `stat`.
- `stat` returns a `Stat` without contents (`src/file.rs:48-68`).
- `list` returns an ordered `Vec<Path>`, and the empty Prefix lists the whole Area.
- A delete of a Path that doesn't exist does nothing.
- One timestamp per Commit, chosen in the Store layer (`src/store.rs:68`), as the spec requires.
- A write identical to what's stored is left out, by comparing Revisions.
- Deletes produce *removed* Changes, and a Commit's Changes arrive in one batch.
- A Commit that changes nothing sends no batch (`announce` in `src/change.rs` returns early when there are no Changes). This is tested for an empty Commit, an unchanged write, and deletes of a missing Path and of an empty Prefix.
- The invalid-Path test now covers `stat` and `delete` with every Path rule, and `list` and `delete_prefix` with every Prefix rule, as the Notes claim.

### Summary

Standards: 0 hard violations and 5 judgement calls. The most notable is a Data Clump: `area`, `staged` and `prefix_deletes` travel together through Staging, `into_parts` and CommitRequest, and ticket 04 will widen it. Spec: 1 partial item. The worst is that the "nothing" half of an all-or-nothing rename was deferred to ticket 04 without a checkbox there to make sure it gets done.


### Resolution

1. **Spec (a1), the "nothing" half of the rename had nowhere to go:** fixed. Ticket 04 has a new
   unticked checkbox: "A rename (a delete and a write in one Commit) that hits a Conflict leaves
   both Paths as they were". The rename criterion in ticket 03 now links to it.
2. **Standards 1, `Applied` uses a word CONTEXT.md avoids:** fixed. The type is now
   `CommitOutcome` and the binding is `outcome`.
3. **Standards 2 and 3, the Data Clump and the rule split in two places:** fixed. The staged
   contents have one crate-private home, `Staged` in `src/staging.rs`: the Area, the action for
   each Path (`Action::Write` or `Action::Delete`) and the Prefix deletes.
   - A `Staging` is a wrapper around a `Staged`.
   - `CommitRequest` is now just the timestamp and the `Staged`. The tuple from `into_parts` is
     gone.
   - Ticket 04's `require` and `require_prefix` become fields of `Staged`, and nothing else has
     to widen.
   - Both halves of "a Path staged after the Prefix delete wins" now live in `staging.rs`.
     `Staging::delete_prefix` drops anything staged under the Prefix before it.
     `Staged::expand_prefix_deletes(existing_paths)` turns the Prefix deletes into deletes of
     the existing Paths under them, keeping an action already staged for a Path. Every Backend
     calls it under its lock with the Paths it has, and the memory Backend already does.
4. **Standards 4, the `(Path, ChangeKind)` tuple:** fixed. `CommitOutcome::changes` is a
   `Vec<RawChange>`, where `RawChange { path, kind }` is crate-private and lives in
   `src/backend/mod.rs`. It has no Revision field, since nothing needs one before ticket 11.
5. **Standards 5, `Stat` repeated `File`'s fields:** fixed. `File` holds a `Stat`, and its
   public `modified()` and `revision()` read from it. The memory Backend stores each File as its
   contents plus a `Stat`, so `stat` hands out the stored `Stat` directly.

Not acted on here: the risk that a later `delete_prefix` silently drops the Precondition of an
earlier write under that Prefix. The coordinator is passing it to ticket 04.

The behaviour tests are unchanged. Clippy (all targets, with default features and with
`--all-features`) is clean, and `cargo test` passes with default features, with
`--no-default-features` and with `--all-features`.

---

## Ticket 04: Preconditions

Reviewed: `git diff 90f7ae7...edca4b7` (commit edca4b7). The Spec reviewer was also asked to judge four decisions from the implementer's Notes.

### Standards

**Hard violations of documented standards: none found.**
- **Spec, Testing Decisions** ("tests use only the public API; no internal state"): met. Every new test in tests/behaviour/suite.rs uses only `Store`, `Staging`, `Precondition`, `Committed` and `Error`.
- **Spec, Implementation Decisions:** met on every point checked:
  - `Error` stays `#[non_exhaustive]` and gains `Conflict { paths }`.
  - `AreaState` is `pub(crate)` (src/backend/mod.rs:21), so Backends stay private with no public trait.
  - The memory Backend commits in the order check → expand → letter case → apply, matching the spec's Backend list.
  - No logging was added, so the `tracing` rule is untouched.
  - `caseless` fits the spec's "case-folding crate chosen during implementation".
- **CONTEXT.md `_Avoid_` lists:** no avoided word is used for a glossary concept. `digest128`, `Entry` and `next_batch` are names from std, xxhash or existing helpers.
- **ADR 0004:** the letter-case rule is enforced on memory as required. One doc-drift note, not a violation: the ADR's list of "established crates" doesn't mention `caseless`.
- **README:** the new Consistency and Limitations lines match the behaviour and the tests.

**Baseline smells (all judgement calls):**

1. **Possible Mysterious Name**, src/staging.rs:271 `fn names_in(path: &Path)`. It returns the Prefixes plus the Path itself (`a/`, `a/b/`, `a/b/c.txt`). In CONTEXT.md a Path *is* a name, so "names in a path" reads oddly. Something like `prefixes_and_path` would say what it returns.
2. **Borderline glossary friction**, src/path.rs:117 `letter_case_key`. CONTEXT.md lists "key" as an Avoid word for Path. Here it names a comparison form, not a Path, so it isn't a breach, but `letter_case_fold` or `letter_case_form` would avoid the word.
3. **Possible naming drift from the glossary**, src/staging.rs:38–40, 185. `requirements`, `prefix_requirements` and `add_requirement` hold what the glossary calls Preconditions, while the check is `check_preconditions` (:195), so two words now name one concept. `preconditions` and `prefix_preconditions` would match. The public `require` is named in the spec and is fine.
   - Related: src/precondition.rs:3 says "Something a Commit requires of a File". The glossary says a *Staging* requires it, and a Precondition can also be on a Prefix. Minor wording mismatch.
4. **Possible Data Clump:** `(Path, Revision)` pairs travel together in four places. It's weak because the tuple is small and local, so a type is only worth it if the ticket 07/09 Backends add more uses.
   - `AreaState::revisions_under` (src/backend/mod.rs:25)
   - `PrefixRevision::of` (src/revision.rs:47)
   - `files: Arc<[(Path, Revision)]>` (src/revision.rs:42)
   - src/backend/memory.rs:91
5. **Minor Duplicated Code**, src/backend/memory.rs:43 vs :91. `list` and `revisions_under` both walk the Area with `prefix.covers(path)`. It's a one-line shape, so low priority.

No Speculative Generality found. `AreaState` returning `Result`, and the `Arc`'d file list inside `PrefixRevision`, are justified by tickets 07/09 and by the requirement that a Conflict name the Paths that differ.

### Spec

All 10 checkboxes hold up against the code and the suite, and `cargo test` passes: 30 behaviour tests and 5 path tests.

**(a) Missing or partial:** none.

**(b) Scope creep:** none of substance.

**(c) Implemented, but wrong or behind the docs**

1. The wider letter-case rule was not recorded in the glossary or the spec. The code also refuses clashes between Prefixes (src/staging.rs:232–275). The docs still say "No two Paths in an Area may differ only by letter case" (CONTEXT.md:45) and "the case-folded Path (unique, which enforces the letter-case rule)" (spec:355, ticket 07:18). Only README.md was updated. CONTEXT.md, ADR 0004 and ticket 07's text should be updated now, not just the Notes.
2. `PrefixRevision` doesn't record which Area or Prefix it was taken for. Two misuses were probed:
   - `require_prefix("u/", <rev of "t/">)` gives `Conflict ["t/1","u/1"]`.
   - A revision taken in another Area gives a Conflict naming Paths that don't exist in this one.

   Both fail safe, with a misleading message. Minor.

**Points the orchestrator asked about**

1. **A Precondition is never dropped: agree.** The spec says Preconditions "stop the application from overwriting changes it hasn't seen" (Solution), and user stories 27 and 29 say the same. Letting a later `write` or `delete_prefix` silently drop one would break that. The costs are safe and documented: a Staging can never take back a Precondition, and `Absent` plus `UnchangedSince` on one Path always conflicts. The glossary still says a write "carries one", so it needs a sentence.
2. **`PrefixRevision` holding the list: agree.** It's the only way to meet "the paths are the ones under the Prefix that were added, removed or changed". It stays opaque, because `Eq`, `Hash` and `Debug` use only the hash (src/revision.rs:42, 59). The `stat_prefix` rustdoc (src/store.rs:67) should mention the memory cost, which grows with the number of Files under the Prefix. A large Cache is where it would hurt.
3. **`caseless` with uppercase then folding: agree.** It fits "established Unicode case-folding crate", and refusing more is within the spirit of ADR 0004. An exhaustive check over single code points found that whenever two fold the same, their keys match (0 violations). Covering directory-like Prefixes follows from the ADR's reason ("could not be moved to the filesystem Backend"); point (c)1 is the paperwork. For ticket 07: the key depends on the Unicode tables in the toolchain and in `caseless` (Unicode 16). A key stored in SQLite can go stale after an upgrade, so recompute it on open, or at least document that it can change.
4. **File `a` alongside File `a/b`: refuse it on every Backend.** A probe showed the memory Backend accepts `a` then `a/b`, `a/b` then `a`, both in one Commit, and `A` beside `a/b`. The last is also a case-insensitive clash, which the current check misses because it compares `a` with `a/`. Under ADR 0004 and "same guarantees on every Backend" this is the same kind of rule. Without it, a filesystem Commit fails with `Backend` partway through its journal, where memory and SQLite succeed.
   - **Error:** `InvalidPath { path, reason: <new reason, e.g. FileUnderFile> }`. The reason list is `#[non_exhaustive]`, and the spec groups Commit refusals of this kind under `InvalidPath`. It should not be `Conflict`, which is for Preconditions only.
   - **How:** extend `refuse_letter_case_clashes` to compare each File's key with the Prefix keys minus their trailing `/`. As now, a Path deleted in the same Commit doesn't count.
   - **Which ticket:** a small follow-up to 04, done now, because it's the same shared function and 07 inherits it. At the latest before 07. Also add a line to ADR 0004 and the glossary ("a Path can't also be a Prefix of another Path").

### Summary

Standards: 0 hard violations and 5 judgement calls. The most notable is naming drift: `requirements` in the code, where the glossary says Preconditions. Spec: 0 missing, plus 2 wrong-or-behind-the-docs items and 1 new rule recommended. The worst is that a File `a` and a File `a/b` can both exist on memory (and SQLite), which the filesystem cannot hold. That breaks ADR 0004's portability guarantee.



### Resolution

1. **Spec point 4, a File under a File:** fixed on every Backend through the shared check, now
   called `Staged::refuse_clashing_paths`. It folds each name, with a Prefix's trailing `/`
   removed, so the File `a` and the Prefix `a/` meet. The same name gives the new
   `InvalidPathReason::FileUnderFile`. Names that also differ in letter case (`A` beside `a/b`)
   give `LetterCaseClash`, which the old check missed. Paths deleted in the same Commit still
   don't count. The new test `a_file_cannot_be_under_another_file` covers:
   - `a/b` and `a/b/c` under an existing `a`;
   - `d` over an existing `d/e`;
   - `n` and `n/m` in one Commit;
   - `D`, `A/b`, and `N` with `n/m`, which clash by letter case;
   - moving `a` to `a/b`, and `d/e` to `d`, in one Commit, both of which succeed.

   I checked that the test fails if the fold keeps the trailing `/`.
2. **Spec (c)1, docs behind the rule:** fixed.
   - CONTEXT.md: the Path entry covers the Prefixes a Path is under, and says a Path can't also
     be a Prefix of another Path. The Precondition entry says Preconditions accumulate.
   - ADR 0004 has a line for each rule checked at Commit time, and names `caseless`.
   - The spec's Path module, its Backend list and its SQLite Tables line now say that a unique
     case-folded column alone doesn't enforce the rule, so the shared check runs in the write
     transaction.
   - Ticket 07's checkbox says the same. A new unticked checkbox there says the stored fold can go
     stale after a Unicode upgrade, and asks for it to be recomputed or documented.
   - README Limitations mentions the File-under-File rule.
3. **Spec (c)2, a Prefix Revision for another Area or Prefix:** fixed. A `PrefixRevision` now
   records its Area and Prefix. `require_prefix` panics if they don't match the Staging's Area
   and the given Prefix, documented under `# Panics`. Such a Precondition could never hold, so a
   Conflict would mislead the app, and an app that retries after a Conflict could loop forever.
   The spec's error list has no variant for it, and the mistake is always in the app's code.
   Equality, `Hash` and `Debug` include the Area and Prefix. The new test
   `a_prefix_revision_is_only_for_its_own_area_and_prefix` checks the panic for another Prefix,
   the empty Prefix and another Area, and that the right one is accepted. The reasoning is in the
   ticket Notes.
4. **Spec point 2, memory cost:** the `stat_prefix` and `PrefixRevision` rustdoc now say that a
   Prefix Revision keeps each Path and Revision, so it costs memory in proportion to the number
   of Files under the Prefix, which can be large for a Cache.
5. **Standards 1 and 3, naming:** fixed.
   - `names_in` is now `prefixes_and_path`.
   - `requirements`, `prefix_requirements` and `add_requirement` are now `preconditions`,
     `prefix_preconditions` and `add_precondition`.
   - `check_preconditions` takes `current` rather than `area`, since `Staged` has an `area` of
     its own.
   - precondition.rs now says a *Staging* requires a Precondition, and that a Staging can also
     require a Prefix.
6. **Standards 2, `letter_case_key`:** renamed to `letter_case_fold`.
7. **Standards 4 and 5:** left as they are. The `(Path, Revision)` pair is small and local, and a
   type is worth it only if tickets 07 and 09 add more uses. The walk that `list` and
   `revisions_under` share is one line.

Clippy (all targets, with default features and with `--all-features`) is clean. `cargo test`
passes with default features, `--no-default-features` and `--all-features`: 32 behaviour tests
and 5 path tests.

### Re-review (after the fix commit 614050e)

The fixes added a new rule (a File can't sit under another File), a panic when `require_prefix` gets a mismatched Prefix Revision, and changes to the spec, the ADR and the glossary. That was substantial enough for a second two-axis review of `git diff edca4b7...614050e`.

#### Standards

`cargo test --all-features` passes: 32 behaviour tests and 5 path tests.

#### Were the claimed fixes made?

All four claimed fixes were made correctly:
- **Standards 1:** `names_in` is now `prefixes_and_path` (src/staging.rs:~291).
- **Standards 3:** `requirements`, `prefix_requirements` and `add_requirement` are now `preconditions`, `prefix_preconditions` and `add_precondition` (staging.rs:38-40, 197). The precondition.rs doc now says a *Staging* requires it.
- **Standards 2:** `letter_case_key` is now `letter_case_fold` (path.rs:117). No "key" is left in src.
- **Standards 4 and 5:** leaving these was reasonable.

#### Hard violations of documented standards

None.
- **Spec Testing Decisions:** the two new tests (suite.rs:830, :882) use only the public API. The `catch_unwind` checks the documented panic and does not inspect internal state.
- **Spec "one `#[non_exhaustive]` error type":** met. `FileUnderFile` is a new variant of the non-exhaustive `InvalidPathReason` (path.rs:152). The panic in `require_prefix` (staging.rs:176) is a behaviour choice, so it belongs to the Spec axis. It is documented under `# Panics`, and the spec already accepts panics for misuse (the blocking API), so it doesn't breach a standard.
- **CONTEXT.md, ADR 0004, spec and ticket 07:** all updated in step with the code.

#### Baseline smells (all judgement calls)

1. **Possible duplicated data (new):** src/staging.rs:40 `prefix_preconditions: Vec<(Prefix, PrefixRevision)>`. The `PrefixRevision` now carries its own `prefix` (revision.rs:44-45), and `require_prefix` asserts the two are equal, so the tuple's `Prefix` is redundant. `check_preconditions` then clones it back in (staging.rs:216 `PrefixRevision::of(self.area, prefix.clone(), files)`). Fix: make it `Vec<PrefixRevision>` and ask the `PrefixRevision` for its prefix.
2. **Minor Duplicated Code / inconsistency:** the same idea, a name without the `/` that ends a Prefix, is written two ways:
   - staging.rs:260 `name.strip_suffix('/').unwrap_or(name)`
   - staging.rs:278 `other.trim_end_matches('/') == name.trim_end_matches('/')`

   They behave the same today, because no name contains `//`. One small helper would stop them drifting apart.
3. **Weak Data Clump:** `(Area, Prefix)` now travels together in several places. It's local, so low priority.
   - `PrefixRevision::of(area, prefix, …)` (revision.rs:55)
   - `is_for(area, prefix)` (:67)
   - the tuples used for equality and hashing (:89, :97)
   - `stat_prefix(area, prefix)`
4. **Glossary note, not a breach:** "directory" appears in path.rs:151 and suite.rs:838. It means the *filesystem's* directory, not a Prefix, which is consistent with CONTEXT.md's "Directories exist only as Prefixes". Leave it.
5. **Cosmetic:** after the `current` rename, the doc comment at staging.rs:204 runs past the wrap width used in the rest of the file. rustfmt doesn't wrap comments, so no tool catches this.

**Summary:** 0 hard violations. The 4 claimed naming fixes are all present and correct. The fix introduced 3 small judgement calls, the most notable being the duplicated Prefix in `prefix_preconditions` (item 1).

#### Spec

`cargo test --all-features` passes: 32 behaviour tests and 5 path tests. Every item the Resolution claims to have fixed was checked and holds. 9 throwaway probes were run, and all behaved correctly.

**(a) Missing or partial**

1. Spec line 269 says "`InvalidPath`: includes letter-case clashes." The fix updated spec lines 229, 306 and 355 but not this one, so it still doesn't mention `FileUnderFile`. Minor doc drift.
2. CONTEXT.md:61-63 (Prefix Revision) still doesn't say that a Prefix Revision belongs to one Area and one Prefix. The rustdoc says so, but the glossary doesn't.

**(b) Scope creep:** none.

**(c) Implemented but possibly wrong**

1. **The panic in `require_prefix`** (staging.rs:176-180) **is consistent with the spec:**
   - The spec's error list ("`InvalidPath`, `NotText`, `Conflict`, `Pending`, `Unsupported` and `Backend`") has nothing that fits.
   - US42's "clear InvalidPath error if one is not allowed" is about a Path or Prefix that isn't allowed. Here the Prefix is valid, so `InvalidPath` would be the wrong error.
   - The spec already panics on programmer error in the app: US62 asks for "a clear panic if I call the blocking API from inside an async runtime".
   - Taking the Prefix from the Prefix Revision instead would conflict with the spec's named `require_prefix(prefix, prefix_revision)`, and wouldn't cover a mismatched Area.
   - The panic message is clear (probe 9).
   - Gap left open, not worth fixing: a Prefix Revision taken from another Store, with the same Area and Prefix, still gives a misleading Conflict (probe 8). It can't be detected without a Store identity.
2. **The FileUnderFile rule is correct in every edge case probed:**
   - `delete_prefix("d/")`, then re-staging `write("d/e")`, then `write("d")` gives FileUnderFile, because the re-staged write keeps `d/` alive.
   - A new `d/f` staged after the Prefix delete gives FileUnderFile.
   - `delete("d")` replaced by a later `write("d")`, alongside `d/x`, gives FileUnderFile.
   - `delete_prefix("")` followed by writes to `q` and `q/r` gives FileUnderFile.
   - Writing `a/b` when `a/b/c` exists gives FileUnderFile. Writing `A/B` gives LetterCaseClash.
   - A failed Precondition together with a FileUnderFile Path gives `Conflict`, as the Notes say.
   - `delete_requiring(d/e, UnchangedSince)` plus `write(d)` succeeds when the Precondition holds.

   **Test gaps** in `a_file_cannot_be_under_another_file` (suite.rs:830-880):
   - Nothing tests the order the Notes claim: "Preconditions are checked first, so a Commit that breaks both gives `Conflict`". The letter-case test doesn't either. Tickets 07 and 09 will write their own commit paths, and the shared suite is the only thing that would pin them to this order. Add one case.
   - A Path re-staged under a `delete_prefix`, which keeps the Prefix alive (the first case above), isn't covered.
3. **Heads-up for ticket 09** (not a defect here, a later ticket owns it). The new move cases in the test (`a` to `a/b`, and `d/e` to `d`) mean the filesystem journal must remove the file `a` before creating the directory `a/`, and remove the emptied directory `d/` before renaming onto `d`. Ticket 09's checklist doesn't mention this. ADR 0004 made the rule to stop filesystem Commits failing partway, so this deserves a checkbox there.

**Verdict:** every Resolution item is fixed correctly, and the panic is the right choice under the spec. What remains is two lines of doc drift and two missing test cases.

#### Summary

Standards: 0 hard violations and 3 small new judgement calls. The worst is that the Prefix is duplicated in `prefix_preconditions`. Spec: every fix verified, plus 2 lines of doc drift and 2 missing test cases. The worst is that no test holds later Backends to checking Preconditions before refusing clashing Paths.


#### Resolution

1. **Test gaps:** fixed in `a_file_cannot_be_under_another_file`.
   - A new case has a failed `require(.., Absent)` together with a File-under-File write, and
     expects `Conflict`. I checked that it fails if the memory Backend refuses clashing Paths
     before checking Preconditions.
   - Another new case stages `delete_prefix("d/")`, then `write("d/e")` again, then `write("d")`,
     and expects `FileUnderFile`.
2. **Doc drift:** fixed. The spec's `InvalidPath` line now mentions a File under another File.
   The CONTEXT.md Prefix Revision entry says it belongs to one Area and one Prefix.
3. **Standards 1, the duplicated Prefix:** fixed. `prefix_preconditions` is now
   `Vec<PrefixRevision>`, and the check asks each one for its Prefix. `PrefixRevision::differences`
   now takes the current Files directly and hashes them with a shared `hash` helper, so nothing
   is cloned to build a second Prefix Revision.
4. **Standards 2:** fixed. One helper, `without_trailing_slash`, is used in both places.
5. **Standards 5:** fixed. The doc comment on `check_preconditions` is re-wrapped.
6. **Ticket 09:** it has a new unticked checkbox. The journal must remove the file `a` before
   creating the directory `a/`, and remove the emptied directory `d/` before renaming onto `d`.

Left as the coordinator asked: the `(Area, Prefix)` clump.

Clippy (all targets, with default features and with `--all-features`) is clean. `cargo test`
passes with default features, `--no-default-features` and `--all-features`: 32 behaviour tests
and 5 path tests.

---

## Ticket 05: What the Change feed promises

Reviewed: `git diff f852326...919e83d` (commit 919e83d). The Spec reviewer was also asked to look for races (lost wakeups, split batches, a feed that never ends), to check that the `commit_order` lock is safe to cancel, and to judge the recorded Resync design.

### Standards

**(a) Documented-standard violations**

1. **`src/store.rs:128-138`, `inject_external_change` behind `testing`: possible hard violation.** The spec's Testing Decisions say tests "never look at internal state. The one exception is starting a failure point", and they list memory as "not applicable" for simulating outside changes. This hook writes straight into the Store layer's feed and bypasses the Backend, which makes it a second exception. The ticket's own notes (05, "The `testing` feature adds only…") endorse it, but the spec was not amended. Amend the spec's Testing Decisions, or accept the inconsistency.
2. **`tests/store_layer.rs:26-27`, `mod external` gated on `#[cfg(feature = "testing")]`: judgement call.** Nothing enables `testing` (no `required-features`, no CI config), so a plain `cargo test` silently skips the only test of the rule that merged Changes are external if any part was. No documented standard covers this. It is flagged because that rule is a README promise (README.md:23-24).
3. **Glossary (CONTEXT.md):** no word from an `_Avoid_` list is used for its concept.
   - "Applied" (store.rs:40, 116) matches the Commit definition, "Applying a Staging".
   - "entry" (change.rs:63) is a map entry, not a File.
   - "area's directory" (README) is the real on-disk directory.

   No `tracing` was added, Backends stay `pub(crate)`, and tests use only the public API apart from item 1.

**(b) Smells (all judgement calls)**

- **Possible Mysterious Name, `src/change.rs:106` `struct Pending`.** The spec already uses "Pending" for a planned error variant (ticket 10: "`Pending`: decided but not fully applied"), so the crate will soon have two meanings for one word. `Unread` or `Recorded` would avoid the clash. Separately, `fn next(&mut self) -> Option<Option<FeedItem>>` (change.rs:144) can only be understood with its doc comment.
- **Possible Primitive Obsession / hidden coupling, change.rs:110, 133, 154 and area.rs:17.** `areas: [BTreeMap<Path, Merged>; 3]` is indexed by `area as usize` in one place and zipped with `Area::ALL` in another. This relies on the enum's order and `ALL` staying in step. Keeping a map per Area is justified by the planned Resync design (tickets 08 and 11), so it isn't speculative. A small `PerArea<T>` with `get_mut(Area)` and `iter()` would keep the indexing in one place.
- **Possible Duplicated Code, `tests/store_layer.rs:87-99` and `:82-83`.** These copy `next_batch` and `assert_nothing_more` from `tests/behaviour/suite.rs:1139-1175` almost line for line. A shared `tests/common` module would remove the copy.
- **Possible Duplicated Code, tests.** The shape `let seen: Vec<_> = batch.iter().map(|change| (change.area, change.path.as_str(), …)).collect();` appears four times: twice in suite.rs (new tests at about :951 and :1026) and twice in store_layer.rs (:60 and :79). A helper next to `changes()` that includes the Area would cover all four.
- **No Middle Man, Feature Envy or Speculative Generality found.** `FeedSender::record` (change.rs:180) is a thin wrapper, but it adds the lock and the wake-up, so it isn't a Middle Man.

Out of scope for this axis and not assessed: whether cancelling `commit` between `backend.commit` and `feed.record` (store.rs:119-121) can drop a Change.

### Spec

Every ticked box was checked against the code and holds. Tests pass with default features, `--all-features` and `--no-default-features`. No blocking defects.

**(a) Missing or partial**
- Nothing blocking.
- "Changes stop being recorded" after the feed is dropped is implemented (src/change.rs:89, :130) but untested. The ticket note says the public API can't observe it, which is true.
- "Memory grows with the number of distinct Paths" is only checked indirectly: 1000 Commits produce one Change. Acceptable.

**(b) Scope creep**
- Nothing of substance. `Area::ALL`, the README update and `inject_external_change` (store.rs:128-138, only with the `testing` feature) are all asked for: "the `testing` feature adds a way to inject one into the Store layer".
- The per-Area maps in `Pending` (change.rs:110) are shaped for Resync, which isn't built yet. The cost is trivial.

**(c) Implemented, possibly wrong**
- **The concurrency is sound.**
  - No lost wakeups. `next` (change.rs:75-81) checks `Pending` under the lock before it waits, and there is only one waiter, so a `notify_one` with nobody waiting is kept as a permit. That covers a record or an `end()` landing between the unlock and the `.await`.
  - A stale permit costs at most one extra loop.
  - `next` is cancel-safe, because `Pending` is only taken in the same step that returns it.
  - "A Commit's Changes are never split across batches" holds: each Commit is recorded under one lock (change.rs:180-186).
  - The feed ends: `Inner::drop` calls `end()` (store.rs:47). `Shared` holds no reference back to `Inner`, so there is no cycle. Whatever was pending still arrives, then `None` is returned for good.
- **The `commit_order` lock is sound.**
  - Locks are always taken in the order commit_order → Backend mutex → `pending`, and reads never take `commit_order`, so nothing can deadlock.
  - A cancelled Commit releases the lock cleanly: tokio's lock future is cancel-safe, and the guard is dropped both on future-drop and on the `?` error path.
  - The memory Backend's commit has no `.await` inside it, so today a Commit can't be cancelled after it is applied but before it is recorded (store.rs:120-121).
  - **Watch in ticket 10** ("once started, a Commit continues in a background task"): the guard must move into that task, or a later Commit could be recorded first. The invariant written on `Inner` doesn't mention this.
- **Minor overclaim:** "a Commit applied later never has an earlier timestamp" (store.rs:117-118). `Timestamp::now()` reads the wall clock, which can step backwards. The spec doesn't require this, so only the comment and the ticket note overclaim.

**The recorded Resync decision is consistent with ADR 0003 and the spec.**
- The spec says the feed "merges changes rather than dropping them, and says so explicitly (a Resync) if it cannot keep that promise", and CONTEXT.md's Resync entry says the app should "read everything it relies on in that Area again". Absorbing an Area's Changes into a pending Resync loses nothing, because the app's reread happens after `next` hands the Resync over, and anything recorded after that is pending as usual.
- It also keeps ADR 0003's promise that memory stays bounded.
- Two points to record for ticket 11:
  - A debounced burst across Areas must be recorded with one `record` call per Area. Only a Commit is promised a single batch, so that's allowed.
  - A pending Resync hides Origin. An app that skips its own Changes loses nothing, because the required reread covers them.

### Summary

Standards: 1 possible hard violation and 4 judgement calls. The violation is that `inject_external_change` is a second test-only door into internal state, which the spec's Testing Decisions don't allow for. Spec: nothing blocking, and the concurrency was judged sound. The most important note is for ticket 10: the `commit_order` guard must move into the background task that finishes a cancelled Commit.



### Resolution

1. **Standards (a)1, a second test-only door not in the spec:** fixed in the spec. Its Testing
   Decisions now name two exceptions, both only with the `testing` feature: starting a failure
   point, and injecting an external Change into the Store layer. The "memory: not applicable"
   line now says that nothing outside tidings can reach memory, so `Store::inject_external_change`
   records an external Change without changing any File. The Crate setup line for `testing`
   mentions it too.
2. **Standards (a)2, a plain `cargo test` skipped the external-Origin test:** fixed. tidings is
   now its own dev-dependency, with `default-features = false, features = ["testing"]`, and the
   `cfg` gate in tests/store_layer.rs is gone.
   - Why this option: `required-features` would still skip the whole test target on a plain
     `cargo test`, just more visibly, and the point was to make it run. Feature unification turns
     `testing` on only in builds of tidings' own test targets. I checked with `cargo build -v`:
     a normal build compiles tidings with only `default`, `fs` and `sqlite`. Crates that depend
     on tidings never see dev-dependencies, and `cargo publish` drops a path-only dev-dependency.
   - With `default-features = false`, `--no-default-features` test runs still leave out `fs`
     and `sqlite`.
   - Ticket 10's failure points get the same treatment, so its crash tests also run under a
     plain `cargo test`. There is a comment in Cargo.toml saying so.
3. **`Pending` and `Option<Option<FeedItem>>`:** fixed. The struct is now `Unread` and the field
   `unread`, so the word is free for ticket 10's `Error::Pending`. `Unread::next` returns a small
   private enum, `Next { Item(FeedItem), Ended, Wait }`, which `ChangeFeed::next` matches on.
4. **`area as usize` and `Area::ALL`:** fixed. The crate-private `PerArea<T>` in src/area.rs
   has `get`, `get_mut` and `iter_mut`, and is the only place that maps Areas to positions.
   `Area::ALL` is gone. The feed keeps its maps in a `PerArea`, and so does the memory Backend
   for its Files. There is no `iter`, because nothing needs it: the feed needs `iter_mut` to take
   each Area's map.
5. **Duplicated test helpers:** fixed. tests/common/mod.rs holds the feed helpers (`next_item`,
   `next_batch`, `changes`, `assert_nothing_more`, `assert_ended`) and the new
   `changes_in_full`, which gives `(area, path, kind, origin)` for each Change of a batch. The
   behaviour suite includes it with `#[path]` and tests/store_layer.rs with `mod common`. It has
   `#![allow(dead_code)]`, because each test crate uses only some of the helpers. The four
   projections, and the older one in `a_commit_announces_one_batch_of_local_changes`, now use
   `changes_in_full`. The merging test now also checks the Origin inline, instead of with a
   separate `all(..)`.
6. **Spec (c):** fixed.
   - The overclaim: the comment now says the timestamp is taken under `commit_order`, so
     Commits read the clock in the order they are applied. The wall clock can step backwards, so
     their timestamps are in that order only while it doesn't. The ticket note says the same.
   - The invariant on `Inner` and the ticket note now say that a Commit continued in the
     background (ticket 10) must carry its `commit_order` guard into that task, as an owned
     guard (`Arc<Mutex<()>>` with `lock_owned`), or a later Commit could be recorded before it.

The review's two points for ticket 11 are recorded in the ticket 05 notes, next to the Resync
decision: a debounced burst that spans Areas is recorded with one `record` call per Area, and a
pending Resync hides Origin, which loses nothing.

Clippy (all targets, with default features, `--all-features` and `--no-default-features`) is
clean. `cargo test` passes with default features, `--no-default-features` and `--all-features`:
38 behaviour tests, 5 path tests and 2 Store-layer tests. A plain `cargo test` now runs both
Store-layer tests.
