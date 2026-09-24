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
