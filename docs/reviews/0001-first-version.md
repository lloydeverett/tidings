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

---

## Ticket 06: Snapshots

Reviewed: `git diff be7d894...249ec8e` (commit 249ec8e). The Spec reviewer was also asked whether the copy-on-write Snapshot is truly consistent, and to judge the decision that a Snapshot doesn't keep the Store alive.

### Standards

**(a) Documented-standard violations**

No hard violations. Checked against the spec's Implementation and Testing Decisions and ADR 0006:
- Backends stay private and are dispatched through an enum: `BackendSnapshot` is `pub(crate)`, and there is no public trait.
- `Unsupported` is added to the single `#[non_exhaustive]` `Error` (src/error.rs:25-28).
- The public operations are `snapshot` and `supports_snapshots`, and a Snapshot has `read`, `stat` and `list`, as the spec lists.
- The tests use only the public API.
- No `tracing` was added, which is consistent because the crate has none yet.
- README Consistency was updated with the code (spec Further Notes: "Keep them in sync").

One borderline glossary point (judgement call):
- README.md:19, "For a consistent read of several files, use a snapshot of an area." CONTEXT.md lists "consistent read" under Snapshot's _Avoid_ list. The phrase predates this diff, but the diff rewrites the line and keeps it. It describes the purpose rather than naming the concept, so this is a soft breach. Suggested fix: "To read several files without mixing commits, use a snapshot…".
- src/backend/mod.rs:23 "read transaction" describes SQLite's mechanism, as the spec itself does, not the Snapshot concept. That's fine.

**(b) Baseline smells (all judgement calls)**

- **Duplicated Code, well handled:** memory.rs:112-123 extracts the free functions `read`, `stat` and `list`, which both `MemoryBackend` and `MemorySnapshot` call. No action.
- **Duplicated Code (minor):** in tests/behaviour/suite.rs the `snapshot_list` helper at :1324 has the same shape as `list` at :1318 (list, then map with `as_str().to_owned()`). A shared `fn as_strings(&[Path]) -> Vec<String>` would remove the copy.
- **Duplicated Code (minor):** the guard `if !store.supports_snapshots() { return; }` appears at suite.rs:1144, 1188, 1237 and 1275, and inverted at :1294. It could be folded into the suite macro, or into a `fixture.open_with_snapshots()` that returns `Option`. Tolerable as it is, and tests/behaviour/main.rs:26 (`memory_supports_snapshots`) rightly guards against the suite silently skipping everything.
- **Repeated Switches, suppressed by a repo standard:** `Backend::supports_snapshots` (mod.rs:94) and `Backend::snapshot` (mod.rs:102) each match on the Backend, and `impl BackendSnapshot` (mod.rs:118+) repeats the match for read, stat and list. The spec mandates enum dispatch without a trait, so this is endorsed. One risk remains: `supports_snapshots` and `snapshot` state the same fact twice, and the two could disagree once fs or SQLite arrive. The behaviour tests at :1294 and main.rs:26 mitigate this.
- **Mysterious Name (very minor):** src/snapshot.rs:12, `Snapshot { snapshot: BackendSnapshot }`. The field has the same name as its owner. `backend` or `view` would read better at `self.snapshot.read(...)`.
- **Speculative Generality: none.** `Backend::snapshot` being `async` and returning `Result`, `BackendSnapshot` being an enum, and `a_backend_without_snapshots_refuses_one` (a no-op on memory today) are all seams that the SQLite and fs tickets in the known series need.

### Spec

All five ticked boxes were verified as genuinely met. `cargo test` passes: 44 behaviour tests on a 4-worker multi_thread runtime.

**(a) Missing or partial.** None. Two items are deferred as planned:
- "and that they are refused on the filesystem" (spec, Testing Decisions) belongs to ticket 09. The refusal test exists and returns early on memory, as the ticket allows (tests/behaviour/suite.rs:1291).
- "Each Backend's module … asserts what it should answer" is so far done only for memory (tests/behaviour/main.rs:23). Tickets 07 and 09 add the other Backends.

**(b) Scope creep.** None. The README changes (README.md:19-30) only describe what was built.

**(c) Implemented, possibly wrong.** Nothing that breaks the spec. One nit:
- Ticket: "each Snapshot costs at most one copy … paid by the first Commit to that Area while it's held". `Arc::make_mut` at src/backend/memory.rs:72 runs even when the Commit changes nothing (every write is unchanged, or it deletes a missing File), so such a Commit still pays for the copy. The claim is still literally true and this only costs performance, so fixing it is optional.

**Is the copy-on-write Snapshot really consistent?** Yes. A Snapshot cannot see part of a Commit.
- `MemoryBackend::commit` takes the `std::sync::Mutex` at memory.rs:66. It holds the lock across the checks (68-70), `make_mut` (72) and the whole apply loop, and it never awaits.
- `snapshot()` clones the Area's `Arc` under the same lock (memory.rs:58-59), so a Snapshot is either wholly before or wholly after any Commit. It can never land between a Commit's checks and its apply.
- After the copy, the old map is never touched. `Stored` is never changed in place: there is no `get_mut` or `make_mut` on it, and writes insert a new `Arc<Stored>`.
- A Snapshot can see a Commit before that Commit's Changes reach the feed (store.rs `commit` records them after the Backend returns). That's harmless, because the spec ties Snapshots only to Commits, not to the feed.
- `reads_through_a_snapshot_never_mix_commits` is a real test on the multi_thread runtime. Its deterministic first half, a Commit between two reads, would catch a Snapshot that reads the live map.

**The decision that a Snapshot doesn't keep the Store's shared state alive.** Agreed.
- It follows the spec: the Change feed "returns nothing once every Store handle has been dropped", and story 58 says "so that the task listening to it finishes on its own".
- A Snapshot isn't a Store handle and can be held as long as the app likes, so holding `Arc<Inner>` would break the rule stated on `Inner` (store.rs:26-33).
- It fits SQLite's "a read transaction on a separate connection", and it is tested (suite.rs:1273).
- It is recorded in the ticket notes, the `BackendSnapshot` docs (backend/mod.rs:19-23) and the README. No ADR seems needed.
- Warning for ticket 12: `blocking::Store` "runs its own internal tokio runtime". A blocking Snapshot must hold that runtime itself, not the blocking Store. Otherwise it either keeps the feed alive or stops working once the Store is dropped. Ticket 12 should say so.

### Summary

Standards: 0 hard violations, 1 soft glossary point ("consistent read" in the README) and 3 minor smells. The most notable is the Snapshot-support guard repeated across the suite. Spec: 0 findings against this ticket. The Snapshot was verified consistent, and the lifetime decision was endorsed. The most important note is for ticket 12: a blocking Snapshot must hold its own runtime, not the blocking Store.



### Resolution

1. **Standards (a), "consistent read" in the README:** fixed. The line now reads "To read several
   files without mixing commits, use a snapshot of an area."
2. **`Snapshot { snapshot }`:** fixed. The field is `view`.
3. **Duplicated test code:**
   - `snapshot_list` is gone. A new `as_strings(&[Path])` turns Paths into strings. `list` uses
     it, and the Snapshot tests call it on `snapshot.list(..)` directly.
   - The repeated `if !store.supports_snapshots() { return; }` guard is kept as it is. A helper
     such as `open_with_snapshots` would still need a `let ... else { return }` at each call site,
     so it would save little, and the plain guard shows at a glance why each test can do nothing.
4. **Spec (c), a Commit that changes nothing still paid for the copy:** fixed. The memory Backend
   now calls `Arc::make_mut` just before each insert or remove, so the map is copied only when a
   Commit actually changes it (after the first call the map is unique, so later calls cost only
   the check). The module doc and the ticket note say so.
5. **Ticket 12, a blocking Snapshot's lifetime:** added as an unticked checkbox in
   docs/tickets/0001-first-version/12-blocking-api.md. A blocking Snapshot holds the internal
   runtime itself (for example an `Arc` of it), not the blocking Store.

Clippy (all targets, with default features, `--all-features` and `--no-default-features`) is
clean. `cargo test` passes with each of them: 44 behaviour tests, 5 path tests and 3 Store-layer
tests.

---

## Ticket 07: SQLite backend, storage

Reviewed: `git diff dd23089...a5c740e` (commit a5c740e). This ticket's implementer was restarted twice after accidental interruptions and built on the partial files left behind. The Spec reviewer was also asked three questions: whether cancelling a Commit belongs to this ticket or ticket 10, whether the checks inside the write transaction are exact, and whether Snapshots are pinned.

### Standards

**Hard violations**

- **src/backend/sqlite.rs:449-455 (a unit test edits the database directly).** This breaks docs/specs/0001-first-version.md:394-398: tests "never look at … tables or internal state", and the spec allows only two exceptions, both under `testing` (failure points and `inject_external_change`).
  - *Justified:* yes. A stale fold only comes from a Unicode-data upgrade, which no public-API test can cause. The test also checks the result publicly (a Commit gets `LetterCaseClash`), and the ticket notes say it fails when the re-folding is removed.
  - *Recorded:* only partly. It is noted in the test's doc comment (sqlite.rs:433) and in the ticket 07 Notes, but the spec still says "two exceptions". A one-line third exception under Testing Decisions would close the gap.
  - *Alternative:* a `testing`-feature hook that overrides the stored `letter_case_fold_unicode_versions`, under the spec's own exception pattern, would test the same thing without touching tables.
  - *Nit:* the test hard-codes `'fold_unicode_versions'` (sqlite.rs:453) instead of using `super::FOLD_UNICODE_VERSIONS`, so the two can drift apart silently.

No other documented standard is breached:
- Backends stay `pub(crate)`, with no public trait.
- The single `#[non_exhaustive]` `Error` gains `Backend`, as the spec lists.
- Feature gating matches the spec.
- The behaviour suite's Fixture uses a temporary Root override for each test.
- "transaction", "version" and "directory" are used only for SQLite transactions, schema and Unicode versions, and OS directories. None of them names a concept that CONTEXT.md's `_Avoid_` lists cover.
- No `tracing` calls were added, so the debug-only rule is not engaged.

**Baseline smells (judgement calls)**

- **Duplicated Code:** the "`x/` up to `x0`" range trick appears four times: memory.rs:145, sqlite.rs:381, sqlite.rs:411 and the SQL at sqlite.rs:414. Each copy re-explains it in a comment. One helper, e.g. `Prefix::range()` or a `fold_range(fold)` in path.rs, would keep the rule in one place.
- **Duplicated Code:** `SqliteBackend` and `SqliteSnapshot` each have `read`/`stat`/`list` plus their own `with_connection` (sqlite.rs:164 and 191). Minor, and partly forced by the Backend enum shape.
- **Primitive Obsession:** `Option<Written>` means "a write, or `None` for a removal" (mod.rs:72, 135; sqlite.rs:315), and three doc comments have to explain the `None`. An enum such as `Planned::Write(Written) | Remove` would say it in the type.
- **Mysterious Name:**
  - `blocking()` (sqlite.rs:201) collides with the spec's `blocking` feature and `blocking::Store`. Something like `off_runtime` would avoid that.
  - `Held` (memory.rs:31) and `Current` (sqlite.rs:343) don't say what they hold. `AreaInMemory` and `AreaInDatabase` are possible names.
- **Mysterious Name / misused type:** `unexpected()` (sqlite.rs:272-273) wraps a non-SQLite failure in `rusqlite::Error::ToSqlConversionFailure`, which misdescribes it. It would be more honest for `open_database` to return `crate::Result` and use `Error::backend` directly.
- **Data Clumps (mild):** `paths_named_like(name, fold)` (mod.rs:374) takes a `fold` that is always `letter_case_fold(name)`, so every caller has to keep the two consistent by hand.
- **Not Speculative Generality:** the one-step `MIGRATIONS`, the `#[non_exhaustive] SqliteOptions` and `app.rs` are all needed by tickets 08 and 09.

### Spec

`cargo test` passes: 88 behaviour tests across memory and SQLite, plus `--no-default-features`.

**(a) Missing or partial**

1. **A cancelled SQLite Commit is applied but never reaches the feed.** This breaks two spec lines:
   - ticket 05 (ticked): "Every Commit made after `open` returns produces a Change"
   - ticket 07: "Everything the memory Backend does, the SQLite Backend does too."

   A scratch probe confirmed it. `commit` was wrapped in a 50 µs timeout and run 300 times: all 300 Commits were applied, and none reached the feed.
   - **Cause:** `src/store.rs:156-160` drops `_in_order` and never calls `record`, while `spawn_blocking` (`src/backend/sqlite.rs:150-163, 181-184`) runs on and commits.
   - Story 39's "finish or not happen at all" still holds, because the SQLite transaction is atomic. Only the feed promise is broken.
   - **Is ticket 10 the right owner? Only partly.**
     - For: ticket 05's notes do hand the background-Commit mechanism to ticket 10, and its checklist item is worded for any Backend.
     - Against: ticket 10's "What to build" is about the filesystem ("A filesystem Commit is all-or-nothing…"), and it is blocked by 09, so SQLite would ship breaking a ticked guarantee for two more tickets.
     - The spec puts the fix in the Store layer ("Letting a cancelled Commit finish: once started, a Commit continues in a background task"), independent of the Backend. The fix is small: make `commit_order` an `Arc<Mutex<()>>` locked with `lock_owned`, and `tokio::spawn` a task that holds `Arc<Inner>` and the guard and records the Changes. `sqlite` already enables `tokio/rt`.
   - **Recommendation:** close it now, with a shared-suite test (Testing Decisions lists "Cancelling a Commit"). Leave ticket 10 only the filesystem pause-point test. The README Status note (`README.md:7-11`) is honest, but it documents a workaround.

**(b) Scope creep**

- The memory fold index and the rewrites of `refuse_clashing_paths` and `expand_prefix_deletes` (`src/staging.rs:226-316`, `src/backend/memory.rs`) come from the ticket 05 review finding. The shared check needs them to run inside SQLite's transaction without scanning the whole Area, and the ticket's Notes cover them. Not a finding.

**(c) Implemented but possibly wrong**

- **Q2, are the checks exact? No problem found.** The spec says "The checks happen inside the write transaction, so they are exact."
  - `sqlite.rs:152-160` opens the transaction with `TransactionBehavior::Immediate` (`BEGIN IMMEDIATE`). `plan` then reads everything it needs through `Current(&transaction)`: the Preconditions, Prefix Revisions from the stored Revisions, the no-op filter, and the clash lookup through `paths_named_like` (`sqlite.rs:414-426`). `apply` writes through the same transaction, and then it commits.
  - The connection's mutex is held for the whole closure, and an `Err` from `plan` drops the transaction, which rolls it back. There is no window between the checks and the write.
- **Q3, are Snapshots pinned? No problem found.** The spec says "A Snapshot is a read transaction on a separate connection."
  - `sqlite.rs:131-145` runs `BEGIN` and then a read on a new read-only connection before `snapshot()` returns, which pins the view at the moment of the call.
  - `SqliteSnapshot` holds only its `Arc<Mutex<Connection>>`, not `Inner`. The probe confirmed that the feed ended while a Snapshot was still held, and that the Snapshot kept returning the old contents after the Store was dropped. `a_snapshot_outlives_the_store_without_keeping_the_feed_open` (`tests/behaviour/suite.rs:1276`) covers this.

The other checklist items were verified:
- `etcetera` directories, with the Root override documented (`src/app.rs`)
- `bundled`, WAL and `spawn_blocking`
- folds redone when the recorded Unicode versions differ (`sqlite.rs:218-231`)
- `supports_snapshots()` returns true
- the `sqlite` feature gate (`Cargo.toml:15`)

### Summary

Standards: 1 hard violation, a justified but only partly recorded one: a unit test edits the database, against the spec's testing rule. There are also 6 judgement calls, the most notable being the Prefix-range trick copied four times. Spec: 1 missing item, and it is serious. A cancelled Commit is applied on SQLite but its Changes never reach the feed, which breaks ticket 05's ticked guarantee. The reviewer recommends fixing it in the Store layer now, not in ticket 10.



### Resolution

1. **Spec (a)1, a cancelled Commit never reached the feed:** fixed in the Store layer, for every
   Backend.
   - `commit_order` is now an `Arc<Mutex<()>>`. `Store::commit` waits for its turn with
     `lock_owned`. From then on the Commit is an async block that owns the guard and an
     `Arc<Inner>`, commits, and records its Changes.
   - It runs inside a small `Started` future. While the app waits, `Started` polls it in place.
     If `Started` is dropped before it finishes, it spawns the rest onto the runtime's `Handle`.
     So a Commit dropped while waiting its turn never happens, and one dropped after it started
     always finishes and is reported.
   - The suggested shape spawned a task for every Commit. I tried it first, and it made ticket
     05's probe on memory 14 times slower (0.26 s to 3.7 s in release). Spawning only for a
     Commit that is actually dropped keeps the cost where it was.
   - tokio's `rt` feature is now always on, including memory-only `--no-default-features`,
     because `Handle` needs it on every Backend. Memory's Commit finishes in a single poll, so on
     memory the task is never needed, but one code path is simpler.
   - `commit` documents that a dropped Commit either never happens or finishes, and that it panics
     outside a tokio runtime.
   - The new suite test `a_cancelled_commit_finishes_or_never_happens_and_is_reported_if_it_finishes`
     polls each of 100 Commits once and then drops it. It checks that the Changes reported are
     exactly the Files written. On SQLite it failed before the fix, with Files written and nothing
     reported. It fails again if `Started` drops the rest of the Commit instead of spawning it.
   - The README Status workaround is gone. Consistency now says what cancelling does. Ticket 10's
     checkbox asks only for the filesystem test with the pause point, and ticket 07's Notes record
     the change.
2. **Standards (hard), the unit test that edits the database:** kept, and recorded in the spec.
   Its Testing Decisions now name it as a third, narrow exception. A `testing` hook that only
   overrides the recorded versions couldn't show that the folds were really made again. That
   would take a hook that corrupts folds, which is a stranger public surface than one internal
   test. The test now uses `FOLD_UNICODE_VERSIONS`.
3. **The "`x/` up to `x0`" range:** fixed. `path::range_under(name)` gives the range, with the
   explanation in one place. Memory's fold lookup and SQLite's Prefix listing, fold lookup and
   name exclusion all bind or use it. The SQL no longer builds the range with `|| '/'`.
4. **`Option<Written>`:** fixed. It is now `Planned::Write { contents, stat } | Planned::Remove`,
   and `Written` is gone.
5. **Names:** fixed.
   - `blocking()` is now `off_runtime()`.
   - `Held` is now `AreaInMemory`, and `Current` is now `AreaInDatabase`.
6. **`unexpected()`:** removed. `open_database` returns `crate::Result` and gives
   `Error::backend(..)` for a journal mode that isn't WAL, or for a schema from a later version.
   The pure-SQL part, `bring_up_to_date`, still returns `rusqlite::Result`.
7. **`paths_named_like(name, fold)`:** left as it is. The caller already has the fold as the key
   of its map of written names. Having the Backend fold again would double the folding.
   A pairing type would mean cloning that key, and a doc comment already states the contract.
8. **SQLite's duplicated read/stat/list:** fixed. An `AreaConnection` (a shared connection whose
   calls run off the runtime) holds `read`, `stat`, `list` and `call`. Each Area's `Database` has
   one, and `SqliteSnapshot` is now simply an `AreaConnection` in a read transaction. The
   `JoinError` handling that `off_runtime` needs is one `error::joined` helper.

Clippy (all targets, with default features, `--no-default-features` and `--all-features`) is
clean. `cargo test` passes with each of them: 45 behaviour tests per Backend, 5 path tests,
3 Store-layer tests, and the SQLite unit test where `sqlite` is on.

### Re-review (after the fix commit f44bcf7)

The fix added a Store-layer mechanism that lets a started Commit finish after its caller drops it, plus several refactors. That is substantial, so `git diff a5c740e...f44bcf7` was reviewed again on both axes.

#### Standards

**Hard violations: none.**

**Verification of the claimed fixes**

1. **Spec (a)1, the cancelled Commit (src/store.rs:39-80, 180-210):** conforms to the standards.
   - The fix lives in the Store layer, as spec :249 says: "once started, a Commit continues in a background task".
   - There are no `tracing` calls and no public trait.
   - The new test (tests/behaviour/suite.rs:1145-1180) uses only the public API: it polls the future by hand, then checks `list` and the feed.
   - The new tokio-runtime requirement is recorded in the `# Panics` section and in a README Consistency bullet.
   - Making tokio `rt` unconditional (Cargo.toml:29) doesn't conflict with the spec's list of features (:383-387).
2. **The direct database edit in a unit test:** now recorded as a third exception in spec :398-400. The hard-coded `'fold_unicode_versions'` is replaced with `FOLD_UNICODE_VERSIONS` (sqlite.rs:458, 582-583). Closed.
3. **The range trick:** properly fixed. A single `path::range_under` (path.rs:130-136) carries the explanation, and all four sites use it: memory.rs:142 and sqlite.rs:510, 536 and 540. The SQL no longer builds the range with `|| '/'`.
4. **`Option<Written>`:** replaced by `Planned::Write | Remove` (mod.rs:126-131). No doc comment has to explain `None` any more. Fixed.
5. **Names:** `off_runtime`, `AreaInMemory` and `AreaInDatabase` are all in place. Fixed.
6. **`unexpected()`:** removed. `open_database` returns `crate::Result` and uses `Error::backend` (sqlite.rs:399-427). Fixed.
7. **`paths_named_like`:** the reasoning for leaving it is sound. Not re-raised.
8. **Duplicated read/stat/list:** now one `AreaConnection` (sqlite.rs:216-224, 337-375). Fixed.

**New smells (judgement calls)**

- **Possible Middle Man / Divergent Change: `error::joined`** (error.rs:595-606). It has one caller, `off_runtime` (sqlite.rs:390), is gated on `sqlite`, and moves tokio task-join handling into the error module. Before the fix this code was inline in `off_runtime`. The Resolution calls it "one helper", but nothing else shares it. Inline it back, or give it a second caller, such as the `Started` spawn.
- **Mysterious Name: `joined(joined: …)`** (error.rs:597). The parameter shadows the function's own name, and the doc comment "What a tokio task gave, as it `joined`" reads awkwardly. `join_result` or `task_result` would be clearer.
- **Mysterious Name (mild): `AreaConnection::call`** (sqlite.rs:365) is a generic name. `SqliteSnapshot` is now just a type alias of `AreaConnection` (sqlite.rs:224), so `Backend::Snapshot::Sqlite` holds a type whose name doesn't say it is in a read transaction. Acceptable, because the alias's doc comment says so.
- **Comment refers to history:** `Started`'s doc comment says Commits nobody cancels stay "as cheap as before" (store.rs:673). "Before" means nothing to a later reader. Say what it avoids instead: a task for every Commit.
- **Minor:** `paths_named_like` uses `name.to_owned()..name.to_owned()` as a deliberately empty range when the name is a File's (sqlite.rs:541). A comment explains it, but an `Option` range, with the `path` condition added only when it is `Some`, would be plainer. Low priority.

No word from CONTEXT.md's `_Avoid_` lists is newly used for a domain concept. `Planned::Remove` and `AreaConnection` stay clear of them.

#### Spec

The Resolution's one Spec finding, (a)1, is fixed correctly, with no blocking problems. The testing was done in a scratch copy made with `git archive`; the repo was not touched.

**Resolution (a)1: verified fixed.**
- **Started at the right moment.** At `src/store.rs:193-210` there is no await point between `lock_owned()` resolving and the first poll of `Started`. The Backend's work, including `spawn_blocking`, lives only inside the async block that `Started` owns. So a Commit can't be dropped after the Backend has begun applying it but before `Started` owns it. If `Handle::current()` panics, the guard is released before anything is applied.
- **Handoff on drop.** The boxed future is `'static + Send`. The guard moves into the async block when the block is created (`:199`), so it goes into the spawned task too. tokio's `Mutex` is FIFO, and the guard is released only after `record`, so the feed sees Commits in the order they were applied.
- **The test catches the bug.** With the pre-fix `commit` body restored, it failed on SQLite 3 runs out of 3: 100 Files written, 0 reported. It also fails if `Started::drop` doesn't spawn, as the Resolution claims. With the fix it passed 40 runs out of 40, plus a full suite run. It is deterministic, because the first poll always parks on `spawn_blocking`.
- **Always-on `rt` is justified.** The spec says "The API is async on tokio" (spec:40), and it places "once started, a Commit continues in a background task" in the Store layer for every Backend (spec:249). The features list (spec:382-387) says nothing about tokio's features.

**(a) Missing or partial.** None.

**(b) Scope creep.** None.

**(c) New issues from the fix (all minor)**
1. **A Commit can be applied but never reported when the runtime shuts down.** The spec says "once started, a Commit continues in a background task". `Started::drop` (`store.rs:72-80`) calls `Handle::spawn`, and after runtime shutdown that call silently drops the rest of the Commit. Probe: a SQLite Commit was polled once, then the runtime was dropped, then the future. `y.txt` was written, and nothing reached the still-open feed.
   - The drop never panicked: not on current_thread, not on multi_thread with `shutdown_background`, and not outside any runtime. It uses the saved `Handle`, not `Handle::current()`.
   - This is acceptable for an async Store that outlives its runtime. The `commit` rustdoc (`:184-187`) and the README Consistency section should say so.
   - It matters for ticket 12's `blocking::Store`, which owns its runtime and must not drop it before its Commits finish.
2. **The test checks only half its name.** The test is `a_cancelled_commit_finishes_or_never_happens_and_is_reported_if_it_finishes` (`tests/behaviour/suite.rs:1145`). With `lock_owned` moved inside the handed-off block, a Commit dropped while waiting its turn would still happen, and the test still passed. It checks that what was reported equals what was written, not "never happens". So the `commit` rustdoc's promise ("dropped while … waits … the Commit never happens") has no test. The fix is to check that every Commit that was `Poll::Pending`, after the first, is missing from the listing.
3. **Memory `commit` now panics outside tokio** (probe: "no reactor running", `store.rs:210`). Before, it worked on any executor. This is documented under `# Panics` and matches spec:40, so it's a note, not a defect.
4. **A panicking Backend is polled again in a task.** If the Backend panics (resumed through `error::joined`), `Started` is dropped during unwinding with `commit` still `Some`. It then spawns the poisoned future, and the task panics a second time ("resumed after panicking"). That's harmless but noisy. Setting `commit` to `None` before re-raising would avoid it.

`--no-default-features --features testing` also passes: 45 + 5 + 3 tests.

#### Summary

Standards: every claimed fix is verified, with 0 hard violations and 5 small new judgement calls. The most notable is `error::joined`, a helper with only one caller. Spec: the cancelled-Commit fix is verified correct and deterministic, with 4 minor new issues. The worst is that a Commit started just before the runtime shuts down can be applied but never reported. That is acceptable, but it has to be documented, and ticket 12's blocking Store must allow for it.


#### Resolution

1. **Spec c1, the runtime shutting down:** documented, not changed.
   - The `commit` rustdoc now says that if the runtime is shutting down, a cancelled Commit may
     have been applied, or not, without its Changes being reported.
   - The README Consistency bullet on cancelling says the same.
   - Ticket 12 has a new unticked checkbox: `blocking::Store` must not shut its runtime down
     while a Commit is in flight.
2. **Spec c2, the "never happens" half:** now tested, in the same suite test. A Commit
   `holding.txt` is polled once and kept. If it is still unfinished, it holds its turn. A second
   Commit, `waiting.txt`, is then polled once, must be pending, and is dropped. Once `holding`
   finishes, `waiting.txt` must not exist.
   - On memory, a Commit finishes in one poll, so this part has nothing to wait for and checks
     nothing more there.
   - With `lock_owned` moved inside the handed-off block, the test failed on SQLite in 5 of 5
     runs ("a Commit dropped waiting happened"). With the fix it passed 20 of 20.
3. **Spec c4, a panicking Backend polled again:** fixed. `Started::poll` catches a panic from
   the Commit, sets `commit` to `None`, and raises it again. So `Started::drop` has nothing to
   hand on.
4. **`error::joined`:** inlined back into `off_runtime`, its only caller.
5. **The `Started` doc comment:** now says what it avoids: a task is spawned only for a Commit
   that is dropped, not for every Commit.
6. **Left as they are:**
   - `AreaConnection::call`. It is private, and "call this with the connection" is what it does.
     The `SqliteSnapshot` alias's doc says the connection is in a read transaction.
   - The empty range in `paths_named_like`. An `Option` would mean two SQL statements, or a
     condition built at runtime, for what one commented line does now.

Clippy (all targets, with default features, `--no-default-features` and `--all-features`) is
clean. `cargo test` passes with each of them.

---

## Ticket 08: SQLite backend, other processes' Commits

Reviewed: `git diff 1872057...2182836` (commit 2182836). The Spec reviewer was also asked to check that each Commit is reported exactly once and in order, the order of the locks, how pruning detects missed Commits, that the feed still ends, the Resync design, and `data_version` polling.

### Standards

**Hard violations of documented standards**

None found. Tests use only the public API. The `change_log_retention` option is behind `testing`, and the spec's `testing` line now names it. No public Backend trait was added. "transaction" and "version" refer to SQLite's own transactions and to `data_version`/`user_version`, not to the domain concepts CONTEXT.md defines.

**Soft points against documented standards (judgement calls)**

1. **src/store.rs:298-302:** errors from the poller's reads are dropped (`Err(_)`) without being logged. docs/specs/0001-first-version.md:391 says to use `tracing` at debug level, and story 69 wants watcher errors to be diagnosable. `tracing` isn't a dependency yet, so this isn't a breach today, but it is the obvious place for a `debug!` once it is.
2. **src/backend/mod.rs:96-97 and :111:** the spec (lines 312-316) says "The Store tags a raw change as *local* when it matches a Commit this Store made". Now the Backend supplies the Origin in `Observed::Commit { origin, .. }`, while the `RawChange` doc comment at :111 still says the Store layer tags it. The ticket notes chose this on purpose, but the spec text and the doc comment have drifted from the code.
3. **CONTEXT.md:113:** the edited Resync line is 156 characters long and wasn't rewrapped. The rest of the file wraps at about 100.

**Baseline smells (all judgement calls)**

- **Duplicated Code, tests/behaviour/two_stores.rs:270:** `next_item_of` is an exact copy of `common::next_item` (tests/common/mod.rs:10), and `crate::common` is already imported. Delete it and use `next_item`.
- **Duplicated Code, src/backend/sqlite.rs:279/295 and :338/342:** `commit` and `SqlitePoller::read` repeat the same sequence: lock `log`, copy `seen`, `read_log`, commit the transaction, write `seen` back. The `change_log_pruned_through` SELECT also appears twice, at :484 and :504.
- **Primitive Obsession, src/backend/sqlite.rs:138/477/484/504/570:** the `meta` names `'last_store'` and `'change_log_pruned_through'` are written inline as string literals. The file's own convention is a named const, such as `FOLD_UNICODE_VERSIONS` at :143, bound as `?1`.
- **Data Clumps, src/backend/sqlite.rs:183 vs :161-162:** `PerArea<(AreaConnection, Arc<Mutex<LogReader>>)>` is the same connection-plus-log pair that `Database` holds. One small struct shared by both would give it a name.
- **Duplicated Code, src/store.rs:114-121 and :144-154:** with `Store::open(backend)` removed, the sequence `change::feed()` → `Inner {..}` → `Store { inner: Arc::new(..) }` now appears twice, and ticket 09's filesystem open would make three. Consider one helper that takes `backend` and `following`.
- **Mysterious Name, src/backend/mod.rs:97:** `CommitOutcome::before` only makes sense with its doc comment. `observed_before` would say it without one.

**Not flagged**

- `Observed` is compiled without `sqlite` behind `expect(dead_code)`, and `FeedSender` is now `Clone`. Both are seams that tickets 09 and 11 need, so they are not Speculative Generality.

### Spec

Verified by reading the code, by running `cargo test` (default features and `--no-default-features --features "sqlite testing"` pass, plus a `--no-default-features` check), and with a probe. The probe had two Stores make 400 Commits each at a 1 ms poll interval, with write-back Conflicts and cancelled Commits mixed in. Across 6 runs every Path arrived exactly once, with the correct Origin.

**(a) Missing or partial:** none. All six checklist items are met.

**(b) Scope creep:** none. The Resync code in `src/change.rs` is what ticket 05 set aside "for tickets 08 and 11".

**(c) Implemented, but wrong or overclaimed:**

1. **Pruning by wall-clock time contradicts what the docs claim** (low severity). The spec says "The log is pruned." `sqlite.rs:27-29` and ticket line 71 add: "A Store is only that far behind if it stopped running … for 10 minutes." That is false after a forward jump of the wall clock by more than the retention period. The next Commit that changes something (`prune_log`, `sqlite.rs:558`) then deletes entries written moments earlier, and every other Store that hasn't polled yet gets a Resync. Missed Commits are still detected exactly and the Resync is truthful, so this is a wording problem, not lost Changes.
2. **A poller that dies does so silently** (low severity, an edge case). The spec says the feed "says so explicitly (a Resync) if it cannot keep that promise". If `follow_other_stores` (`store.rs:288`) panics, external Changes simply stop, with no Resync. One path to this: a Backend panic inside the Commit closure while it holds the `LogReader` lock (`sqlite.rs:277`) poisons that lock, and the poller's `unwrap` then panics.

**Answers to the concurrency questions (no defects found):**

1. **Exactly once, in order.**
   - The last-read position (`seen`) moves only inside the one closure that holds both the connection lock and the `LogReader` lock (`sqlite.rs:277-295` for a Commit, `:337-342` for the poller), and only after `transaction.commit()` succeeds.
   - Changes are recorded on the feed under `commit_order`, which both Commits and the poller hold (`store.rs:248`, `:292`).
   - After a Conflict, `seen` doesn't move and the poller reads those Commits instead.
   - A cancelled Commit carries its owned `commit_order` guard into the task that finishes it, so it still records in turn.
2. **Lock order and starvation.**
   - Every path takes the locks in the same order (`commit_order` → connection → `LogReader`), so nothing can deadlock.
   - The poller holds `commit_order` only across a short read transaction, never across its sleep or the `data_version` check.
   - The lock is FIFO, and the poller only takes it when `data_version` has changed, so Commits aren't starved.
3. **Missed Commits are detected exactly.**
   - `AUTOINCREMENT` ids have no gaps (a rollback also rolls back the counter), and a Store's own Commits advance its `seen`. So `pruned > seen` holds exactly when entries it never recorded were deleted.
   - The prune point is read in the same transaction as the log, and only ever grows.
   - Each Commit's log rows, data changes and pruning happen in one `BEGIN IMMEDIATE` transaction.
4. **The feed still ends.** `Inner`'s Drop aborts the poller (`store.rs:104`). The poller holds only a `FeedSender` clone, and the suite's tests for the feed ending pass on SQLite.
5. **Resync matches ticket 05's design.**
   - A Resync replaces the Area's unread Changes, absorbs any recorded before it is read (`change.rs:162`), and a second Resync for the same Area adds nothing.
   - `next` hands out Resyncs first, one Area at a time (`change.rs:182`).
6. **`data_version` is checked on the right connection.**
   - Only the Store's own connection for each Area commits. Snapshots open read-only connections (`sqlite.rs:251`), and the poller shares the Store's connection (`sqlite.rs:218`).
   - Another Store opening the database changes `data_version`, which only causes one harmless empty read.
   - After a poller read fails, the Area is read again only once `data_version` changes again. The Resync sent on the failure covers the gap, and any leftover entries arrive later as harmless extra Changes.

### Summary

Standards: 0 hard violations, 3 soft points against documented standards and 6 smells. The most notable soft point is drift between the spec and the code over who sets a Change's Origin. Spec: 0 missing, and every concurrency question came back sound. There are 2 low-severity overclaims. The worse is that a poller that dies stops external Changes silently, without the Resync the spec promises.



### Resolution

1. **Spec c1, wall-clock pruning:** fixed the wording, and kept the pruning as it is. The
   sqlite.rs module doc and the ticket's Notes now say that a forward jump of the clock by more
   than the retention also prunes Commits written moments earlier. Stores that haven't read those
   Commits then get a Resync, which is true, and nothing is missed silently. Keeping the newest N
   Commits as well would need a second `testing` knob so that the Resync test could still prune past
   a Store, and it would only trade a rare spurious Resync for more machinery.
2. **Spec c2, a poller that dies silently:** fixed in two ways.
   - `LogReader::lock` recovers a poisoned lock. `LogReader` changes only once a transaction
     commits, so a panic leaves it consistent.
   - `start_following_other_stores` spawns the poller, plus a small task that awaits its
     `JoinHandle`. If the poller panicked, that task logs it and sends a Resync for every Area
     (`FeedSender::resync_every_area`). When the Store drops and aborts the poller, the join is
     cancelled rather than panicked, so nothing is sent. A guard inside the poller couldn't tell a
     panic from an abort: tokio drops the future only after it has caught the panic, so
     `thread::panicking()` is false by then.
   - Nothing can make SQLite fail or the poller panic on demand, so neither is tested. The ticket
     Notes say so.
3. **Standards 2, who tags Origin:** the spec now says the Origin is decided where the knowledge is.
   - The Store layer tags its own Commits' Changes as local.
   - A Backend that reads a change log gives each Commit's Origin with it.
   - For raw changes a watcher observes, the Store matches them against its own Commits.

   The Store-layer bullet points to this. The `RawChange` doc comment says the same.
4. **Standards 3:** CONTEXT.md's Resync entry is rewrapped.
5. **Standards 1, silent read errors:** `tracing` (0.1.44) is now a dependency. A failed log read and
   a panicked poller are both logged at debug level.
6. **Smells:** all fixed.
   - `next_item_of` is gone. The split test uses `common::next_item`.
   - `StoreConnection::read_log_and(behavior, and)` is now the one sequence: lock, copy `seen`,
     `read_log`, `and`, commit, write `seen` back. Commits use it with `IMMEDIATE`, and the poller
     with `DEFERRED` and an empty `and`. `pruned_through()` holds the only SELECT of
     `change_log_pruned_through`.
   - `LAST_STORE` and `CHANGE_LOG_PRUNED_THROUGH` are consts, bound as parameters. Only the
     migration's fixed SQL still spells them out.
   - `StoreConnection { connection, log }` is the pair that `Database` holds and the poller
     shares.
   - `Store::open(backend, follow)` builds the feed, `Inner` and `Store` for every opener.
     `follow` starts the task that follows other Stores, if the Backend has one.
   - `CommitOutcome::before` is renamed to `observed_before`.

Clippy (all targets, with default features, `--all-features` and `--no-default-features`) is
clean, and so is rustdoc. `cargo test` passes with each: 96 behaviour tests (45 without `sqlite`),
5 path tests, 3 Store-layer tests and the SQLite unit test. The SQLite tests pass on repeated
runs. Removing the gap check still fails the Resync test.

---

## Ticket 09: Filesystem backend, storage and journaled Commits

Reviewed: `git diff f8a57c7...57b0085` (commit 57b0085). The Spec reviewer was also asked to check the journal against ADR 0005: whether finishing a Commit a second time is safe (idempotent), where the fsyncs go, symlinks, locking between two Stores, names on disk that aren't valid Paths, and the failure points added early. The checks used crash-simulation probes.

### Standards

**(a) Breaches of documented standards**

1. **Tests inspect internal state.** This is the one likely hard violation. The spec's Testing Decisions say a test "never looks at journal files, tables or internal state", and the only exceptions are failure points and injecting external Changes.
   - `tests/behaviour/main.rs:489` `temporary_files()` walks the Root override looking for `.tidings-` temporary files. Tests assert on it at :331, :369, :401, :427 and :456.
   - `tests/behaviour/main.rs:205` asserts that `.tidings/` exists.
   - Defensible only because leftover temporary files are visible to a user in their config directory. If that is the intended reading, write it into the spec. Otherwise, check the outcome through `list`/`read` only.
   - Direct on-disk reads and writes of Files (`write_directly`, `on_disk`, the mtime check at :298) are permitted by "filesystem: write to, delete from … the Area directory directly".
2. **Failure points exist outside `testing`.** Soft breach. The spec says failure points "exist only with the `testing` feature". But `FailurePoint` and `stop_at` (`src/backend/fs.rs:106-118`, `:203-208`) are always compiled, and the `stop_at` calls stay in `commit` (:329, :341, :348). Only the `pub use` in `src/lib.rs` and `FsOptions::fail_at` are gated. Without the feature they do nothing, but they are still present.

Checked and conforming:
- `tracing` is used at debug level only (`fs.rs:337`, `journal.rs:62,66`).
- `NotText` was added to the single `#[non_exhaustive]` `Error`.
- The Backend stays `pub(crate)`, and the features keep their shape.
- ADR 0005's step order holds: Preconditions are checked before the journal is written.
- CONTEXT's `_Avoid_` words: "directory" and "entry" are used only for real on-disk directories and `DirEntry`, never for a Prefix or a File.

**(b) Baseline smells (all judgement calls)**

- **Duplicated Code: turning a Path into a place on disk.** `AreaRoot::file` (`fs.rs:228`) splits on `/` and extends, while `Journal::finish` (`journal.rs:120`) uses `root.join(path.as_str())`. Two answers to one question can drift apart on Windows. Share one function.
- **Duplicated Code: handling "not found".** `match … { Ok, Err(e) if is_absent(&e) => …, Err(e) => failed(..) }` appears 11 times: in `fs.rs` (:267, :272, :380, :431, :481) and in `journal.rs` (:102, :128, :162, :224, :270). A small `absent_ok(result, path)` helper would collapse them.
- **Data Clumps / parallel vectors.** In `fs.rs:300-331`, `writes: Vec<(Path, String)>` and `journal.replaces` are built separately, then re-paired by index (`journal.replaces.iter().zip(&writes)`). The contents belong with each `Replace`, or in a local struct holding both.
- **Mysterious Name.** The `Kind` enum at `fs.rs:417` is too generic next to `ChangeKind`. `OnDisk` or `EntryKind` would say what it describes.
- **Primitive Obsession (minor).** Test helpers take the Area as a `&str` (`on_disk("config", …)`, `main.rs:170`) when the `Area` enum exists. Forgivable, because `Area::name` is `pub(crate)`.

Not flagged:
- Enum dispatch in `backend/mod.rs`, because the spec mandates no public trait.
- `temporary_file_name` living in `path.rs` beside the check that recognises such names, which keeps the name format in one place.
- `FsBackend` delegating to `AreaRoot`. That is the boundary between async and blocking code, not a Middle Man.

### Spec

`cargo test --features testing` passes (152 behaviour tests), and `--no-default-features --features fs` builds. The probes were a scratch crate, `scratchpad/probe09/`. A crash at the very end of finishing was simulated by putting a saved `.tidings/journal` back after the Commit had finished, then reopening the Store.

**What checks out**
- **The lock** is held across recovery, the Preconditions and finishing (fs.rs:242-256, 295). `open` also recovers under the lock.
- **The journal** records every temporary file, each resolved target and each delete.
- **Finishing again** was tried after the whole Commit, after only the deletes, and after some of the renames (probes P1, P1b, P1c). All three were idempotent for ordinary moves.
- **Discarding a `prepared` journal** removes every temporary file.
- **Durability:** the temporary files are forced to disk. So is the journal, and atomic-write-file forces its directory.
- **Failure points:** the public API exists only with `testing` (lib.rs). Building three of them here, ahead of ticket 10, is a small step.

**(a) Missing or partial**
- The spec says: "`InvalidPath`: includes letter-case clashes, and a File under another File (`a` beside `a/b`)" (spec:270). This change adds a new `FileUnderFile` refusal for obstacles on disk (fs.rs:524-546), but only the ticket and the module doc record it.
  - In probe P5, after `delete_prefix("p/")` the Prefix `p/` lists as empty, yet writing `p` is refused, because `p/` still holds `cafe\u{301}.txt`.
  - This belongs in ADR 0004 or 0005, the spec and the README Limitations.
- The spec's Path rules (spec:229) still name only `.tidings`. They should also name the newly reserved temporary-file name.

**(c) Implemented but wrong**
1. **Finishing again is not idempotent when two Paths are the same file.** ADR 0005:43 says "Finishing a Commit again changes nothing", which fails in two cases.
   - **A symlink (P2).** `a` is a link to `b`, and one Commit writes `a` and deletes `b`. Finishing leaves `a` and `b` both reading "new a". Finishing again deletes `b` first (journal.rs:119-131), then skips the rename. The write is lost and `a` is left dangling.
   - **A case-insensitive filesystem** (found by reading the code; it can't be tested on Linux). The shared check accepts "delete `Foo`, write `foo`" (P4 on the memory Backend). Finishing again would delete the new `foo`.
   - Worse, in the case-insensitive situation with identical contents, `leave_out_what_changes_nothing` (staging.rs:250) sees `foo` as already present and drops the write. The delete then removes the File on the first finish, with no crash at all.
   - Not forcing the journal's removal to disk is only as safe as this idempotency. The comment at journal.rs:217 also admits that finishing again can re-apply a delete to a File another program has created since, which contradicts the ADR's wording.
2. **A dangling symlink escapes the Area (P3).**
   - Writing to a link that points at `<outside>/deep/er/y.txt` creates `deep/er` outside the root.
   - For a link to `/nonexistent/q`, the Commit tries to put its temporary file in `/`.
   - The cause: neither `nearest_directory` (fs.rs:604) nor the walk that refuses obstacles (fs.rs:538) stops at the Area root or at the link's own directory.
   - This behaviour is neither specified nor documented.
3. **A minor gap in durability.** When finishing creates nested directories (`a/b/c`), only the temporary file's directory and the target's directory are forced to disk (journal.rs:143-144). The directory in between (`a`) is not.

**Names on disk that aren't valid Paths** are left out of `list`, `stat_prefix` and Prefix deletes, as the README documents. Names that differ only in letter case are both listed.

**(b) Scope creep:** nothing substantial.

### Summary

Standards: 1 likely hard violation and 1 soft breach, plus 5 smells. The hard violation is that tests inspect temporary files and `.tidings/` on disk. The soft breach is failure-point code compiled outside `testing`. Spec: 2 documentation gaps and 3 wrong behaviours. The worst is that finishing a Commit again is not idempotent when two Paths are the same file (through a symlink, or letter case on a case-insensitive filesystem). On a case-insensitive filesystem, renaming `Foo` to `foo` with the same contents deletes the File even without a crash.


### Resolution

1. **Spec c1, two Paths that are the same file:** fixed in three parts, recorded in ADR 0005.
   - **(a) Exact names.** A Path now names only the file with exactly its name. `open` finds out
     whether the Area's filesystem ignores letter case, by checking whether `.tidings/LOCK` finds
     `.tidings/lock` while no entry has that name. Where it does, `read`, `stat`, the Commit's
     `revision` and `list` of a Prefix check each name on the way against its directory's entries.
     So `read("foo")` gives nothing and `list` shows `Foo` when only `Foo` is there, which keeps
     them consistent. `revisions_under` skips the check for Paths it has just listed.
     - The case-only rename (delete `Foo`, write `foo`) now keeps its write even when the contents
       are the same, because `foo` is absent.
     - When a write's File or directory is found under another case, that match must be removed by
       the Commit's deletes. Its temporary file then goes in the nearest directory that exists
       under its own name. While finishing, a directory emptied under the old case is removed
       before the directory is made in the new case. That is how the suite's `Themes/` rename
       would work on macOS.
     - I can't test this on Linux, so it is reasoned through step by step against the suite's
       case test, and documented. Only letter case is detected. Case-sensitive APFS, which
       ignores Unicode normalization, and HFS+, which stores names in NFD, aren't handled. ADR
       0005 says so.
   - **(b) The same file on disk is refused.** Before the journal is written, each written or
     deleted Path's file on disk is worked out: its directory as `canonicalize` gives it, plus
     the rest of the name. A second Path that comes out the same is refused with the new
     `InvalidPathReason::SameFile`. That covers the P2 shape (write `a`, delete `b`), writes to
     both `a` and `b`, two links to one File, and a symlinked directory. Deleting the link itself
     and writing the File are two files, so that Commit still goes through. The case-only rename
     isn't refused, because its two names differ. I went with refusing, as suggested, rather than
     defining what such a Commit means.
   - **(c) Deletes that are safe to repeat.** `Planned::Remove` now carries the Revision of the
     File it removes. That Revision comes from `leave_out_what_changes_nothing`, which reads it
     anyway. The journal records it: `remove <Path> <revision>`. Finishing again deletes a File
     only if it is still there under exactly its name with that Revision. So a File that one of the
     Commit's writes, or another program, has put there since is left alone. The only exception is
     a File another program wrote there since with the very same contents. ADR 0005 and the
     journal's module doc now say this, and the old comment in `remove` is replaced.
   - **Tests.**
     - `a_commit_interrupted_while_finishing_is_finished_when_a_store_opens` stops the moves Commit
       at `AfterDeletes` and at `AfterRename(0)`, `(1)` and `(2)`, the last of which leaves only
       the journal's removal undone.
     - `finishing_a_commit_again_keeps_a_file_written_since_where_it_deleted_one`.
     - `a_commit_to_two_paths_that_are_the_same_file_is_refused` (Unix).
     - Each fails with its behaviour removed. The two new failure points exist only with `testing`,
       and finishing a Commit left behind never stops at one.
   - Reasoning through the case-insensitive path turned up a bug in my first version of (b). A
     write whose directory doesn't exist yet got an identity from its last segment only, so
     `x.txt` beside `new/x.txt` was refused. The identity now carries the whole rest of the name.
     `files_named_alike_in_directories_still_to_be_made_are_different_files` covers it.
2. **Spec c2, a dangling symlink escapes the Area:** fixed.
   - A write through a symlink puts its temporary file next to the file the link points to. If
     that directory doesn't exist, the Commit is refused with `Backend` ("… in a directory that
     doesn't exist …") before the journal is written.
   - The journal now tells a `write` to a Path, whose directories finishing may make (only under
     the root), apart from a `replace` through a link, which never makes a directory.
   - The nearest existing directory is found from the root down, so it can't leave the Area.
   - `a_write_through_a_symlink_never_makes_a_directory` (Unix) covers a link to
     `<outside>/deep/er/y.toml`, one to `/nonexistent/q.toml`, and the dotfiles case, a link to a
     missing File in a directory that exists, which still works.
3. **Spec c3:** fixed. Finishing now makes missing directories one at a time from the root down
   (`AreaRoot::make_directories`). It forces each one it made, and the directory it was made in,
   to disk.
4. **Spec a, documentation:** fixed.
   - The on-disk `FileUnderFile` refusal, including P5, and `SameFile` are in ADR 0005, the spec's
     `InvalidPath` line, the `InvalidPathReason` docs and the README Limitations.
   - The spec's Path rules now name the reserved temporary-file name.
5. **Standards a1, tests that look at the disk:** the spec's Testing Decisions now have a fourth
   exception. Filesystem tests may check that no tidings temporary files are left in the Area
   directory, since a person sees them there. The `.tidings/` existence assertion is gone.
6. **Standards a2:** fixed. `FailurePoint`, `stop_at` (now `AreaRoot::stop_at`) and every call
   to it exist only with `testing`. A plain `cargo clippy` without the feature is clean too. The
   loops whose index names a failure point `expect` clippy's unused-index lint without it.
7. **Smells:** all fixed.
   - `on_disk(root, path)` is the one Path-to-disk function. `AreaRoot::file` and the journal
     both use it.
   - `present` and `present_at` replace the not-found matches.
   - Each write's contents travel with its `Replace` in a `Writing`.
   - `Kind` is now `OnDisk`.
   - The test helpers take an `Area`.

Clippy is clean with default features, `--all-features`, `--no-default-features`, `fs` only and
`sqlite` only, both with `--all-targets` and without it, so also without `testing`. So are
rustfmt and rustdoc. `cargo test` passes with default features, `--no-default-features` and
`--all-features`: 157 behaviour tests (45 without fs or sqlite), 5 path tests, 3 Store-layer
tests and the SQLite unit test.
