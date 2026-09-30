# Code reviews: spec 0002, Working copies

Every ticket in [docs/tickets/0002-working-copies](../tickets/0002-working-copies) was reviewed after
it was implemented, using the two-axis `/mattpocock-skills:code-review`. It checks two things:
**Standards** (the repo's documented conventions plus the Fowler smell baseline) and **Spec**
(the ticket and spec 0002). The two axes are reported separately and not ranked against each
other. Each entry ends with a **Resolution** section recording what was fixed or deliberately not
fixed, and why. A re-review follows the Resolution when the fixes were significant.

---

## Ticket 01: Move the Store commands under `tidings store`

Reviewed: `git diff f650200...38dae93` (commit 38dae93).

### Standards

**(a) Documented-standard violations:** none.

- No CONTEXT.md _Avoid_ words in the added lines. New text uses Store, File, Area, Staging,
  Conflict, Change and Backend correctly.
- README examples, the Commands and Exit codes bullets and the shell section use `tidings store`,
  as the ticket asks. The full rewrite is ticket 11.
- No ADR governs the command layout; nothing contradicts ADR 0008.
- Two lines exceed the 100-column width that the repo wraps comments to by hand (rustfmt can't
  wrap comments or string literals): `cli/tests/shell.rs:1` and `cli/tests/edit.rs:23`. A
  convention, not a written rule.

**(b) Baseline smells (judgement calls):**

1. **Possible Duplicated Code, `cli/tests/one_shot.rs:255, 271, 279-281`:** the list of Store
   commands is written out three times, each a different subset. One shared constant would stop a
   command added later being missed in one test.
2. **Possible Mysterious Name, `cli/src/main.rs:44, 49, 62`:** "Store" names three nested layers:
   `Command::Store(StoreGroup::OneShot(OneShot::Store(StoreCommand)))`.
3. Not Speculative Generality: `Command` has one variant, but the ticket exists to free the top
   level for later variants. Not Middle Man: clap needs the `StoreGroup` layer.

### Spec

The ticket is fully met. The reviewer built the binary and probed it by hand; the CLI tests pass.

- **(a) Missing or partial:** none. All ten commands are under `store` and dispatch to the same
  functions; the old forms fail with "unrecognized subcommand" (exit 1); the Store flags and
  `--json` stay global and work before or after `store`; exit codes 0/1/2/3 unchanged; `commit`
  and `discard` in the shell keep their Staging meaning; both `--help` outputs are right and
  tested; the harness helpers use `tidings store`; `git grep` finds no old forms outside spec 0002.
- **(b) Scope creep:** none of substance. User-facing strings naming `tidings store shell` are
  consistent with "list the commands where they now live".
- **(c) Looks wrong:** none. Minor note: nothing asserts that top-level `commit`/`discard` are
  refused, which is right since later tickets give those names Working copy meanings.

### Summary

Standards: 0 hard violations, 2 judgement-call smells (worst: the command list duplicated three
times in `one_shot.rs`) plus 2 over-long lines. Spec: 0 findings.

### Resolution

1. **Duplicated Code, the command lists:** fixed. `one_shot.rs` has one `STORE_COMMANDS` constant
   with all ten commands, used by both tests. `the_store_commands_are_only_under_store` now checks
   every one of the ten at the top level, not just subsets, and the help test checks all ten are
   absent from `tidings --help` and present in `tidings store --help`.
2. **Mysterious Name:** partly fixed. `StoreGroup` is renamed `StoreSubcommand`, matching clap's
   terms and saying what it is. `OneShot::Store(StoreCommand)` is left: `StoreCommand` is shared
   with the shell and predates this ticket, and renaming it would ripple through `command.rs` and
   `shell.rs` for little gain.
3. **Over-long lines:** fixed. The `shell.rs` module doc is rewrapped, and the `edit.rs` script is
   built from two shorter raw strings.

No re-review: the fixes are small and mechanical. `cargo clippy` and the CLI tests pass.

---

## Ticket 02: Tracer bullet: sync a new Working copy, then commit it back

Reviewed: `git diff 1560927...d78a75b` (commit d78a75b).

### Standards

**(a) Documented-standard violations:** none.

- No added line exceeds 100 columns.
- Some _Avoid_ words appear ("apply", "save", "event", "version") at `working_copy.rs:44, 163,
  175, 215, 368` and `record.rs:31`. Each names a Working copy operation in spec 0002's own words
  ("Applying `S`", "saving the record", a `sync` event, "the Store's version"), not a glossary
  concept, as review 0001 ruled on similar cases.
- Commands are thin (`main.rs:173-203`), reports are printed by the output module
  (`output.rs:91-120, 193-203`), the memory Backend is refused, and tests go through the binary on
  fs and SQLite without reading the record's format.

**(b) Judgement calls:**

1. **Module depth, `main.rs:186-192`:** `sync` itself takes the sync lock, calls
   `reconcile_all`, then builds `SyncEvent::CaughtUp`. Ticket 03's per-batch reconcile would copy
   this into the command. A module-level entry point returning events would keep it thin.
2. **Possible Duplicated Code, `working_copy.rs:95, 222, 299, 307, 325, 343, 364`:**
   `|error| Failure::from(error).in_context(x.display())` seven times.
3. **Possible Data Clumps:** `StoreLocation` and `BackendName` travel together (`record.rs:37-38`,
   `WorkingCopy::create`, `StoreLocation::open(backend, create)`, `open_store`): a "which Store"
   type waiting to be born.
4. **Possible Repeated Switches:** `match StoreLocation` in `location.rs:99-121` and
   `record.rs:101-107`. Tolerable with two variants.
5. **Possible Feature Envy / coupling, `record.rs:23-26, 139, 168`:** the on-disk record parses
   Area and Backend names through `clap::ValueEnum` and the output module, so the format follows
   the CLI's argument spelling.
6. **Possible Primitive Obsession, `record.rs:119, 172`:** `parse` returns `Result<_, String>`.
   Minor, internal.
7. **Possible Mysterious Name:** `TIDINGS` (`working_copy.rs:20`) holds a directory name; `at(path)`
   (`:373`) is unclear; `failed`, `to_text` and `bad` lack doc comments against the repo's habit.
8. **Possible Duplicated Code, `main.rs:207-224`:** the two cfg'd `listen_for_ctrl_c`s repeat
   their doc and body. Probably inherent to cfg.

### Spec

All 9 tests pass on fs and SQLite; the reviewer also probed the binary by hand (sync into a
missing folder, `--json`, `-C`, Conflict exit 3, nothing to commit, memory refused, non-empty
folder refused).

**(a) Missing or partial**

- Nothing in the ticket is missing. Gaps owned by later tickets: writes via `tmp/` and removing
  emptied directories (03), Store flags given to `commit` ignored (04), `Error::Pending` (06), the
  ignore file (07).
- **Partial, crash safety.** "The folder and record to agree again on the next command after a
  crash": `create` makes `.tidings/` (`working_copy.rs:112`) before writing the record (`:116`). A
  crash between leaves a folder the next `sync` refuses as not empty. (A reconcile that fails
  partway leaves files but no Bases; resuming is ticket 04.)

**(b) Scope creep:** minor only. `--root` is now made absolute for every command (needed for the
record); commit `--json` adds a `change` field per Path.

**(c) Implemented but wrong**

1. **The record is read outside `.tidings/lock`.** Spec: "`lock`: held exclusively while any
   command reads the folder to act on it, or changes the folder or the record … `commit` … for
   their whole run." `open`/`find` read the record (`working_copy.rs:129`) before `commit` takes the
   lock (`:246`). Confirmed by probe: holding the lock, starting two commits of one edit, then
   releasing it, the first committed and the second exited 3 with a Conflict against its stale
   Base. Worse once ticket 03 lands: `sync` keeps the record in memory and saves it after each
   reconcile, overwriting Bases a concurrent `commit` saved.
2. Minor: the `--json` event is `"caught up"`, with a space, unlike every other event name.
3. Minor: the scan skips a root entry named `.tidings` in any letter case (`:333`). No practical
   effect, since the library refuses such Paths anyway.

### Summary

Standards: 0 hard violations, 8 judgement calls (worst: the command, not the module, driving the
sync loop). Spec: 1 partial and 3 wrong (worst: the record read outside the lock, which loses a
concurrent commit's Bases).

### Resolution

Fixed in 4f672f4:

1. **Spec (c)1, record read outside the lock:** fixed. `WorkingCopy` now keeps only the folder,
   the Store and the Area; taking `.tidings/lock` rereads the record, so `commit` and each
   `reconcile_all` work on fresh Bases and save those, and `create` checks again for a record
   under the lock. Test: `a_second_commit_waiting_for_the_first_reads_the_bases_it_saved` holds
   the lock, starts two commits, then releases it: one commits and the other has nothing to commit
   (fs and SQLite). It fails on d78a75b.
2. **Spec (a), crash before the record is written:** fixed. A `.tidings/` holding only the lock
   files or `atomic-write-file`'s `.working-copy.*` temporary files counts as empty for `sync`.
   Test: `sync_finishes_a_working_copy_a_crash_left_without_a_record`, which fails on d78a75b.
3. **Spec (c)2, `caught up` in `--json`:** fixed. The JSON event is `caught-up`, in the CLI's
   kebab-case; text still says "caught up".
4. **Standards 2, duplicated error mapping:** fixed with one helper, `failed_at(path)`.
5. **Standards 3, Data Clump:** fixed. `StoreAddress { location, backend }` in `location.rs`
   says which Store; `Record`, `create` and `open_store` take it, and it can be compared with the
   flags given, for ticket 04.
6. **Standards 7, names and docs:** fixed. `TIDINGS` became `RECORD_DIRECTORY` (with
   `RECORD_FILE`, `LOCK_FILE` and `SYNC_LOCK_FILE`), `at` became `path_in_folder`, and `failed`,
   `to_text` and `bad` are documented.

Not acted on:
- **Standards 1, the command driving the sync loop:** ticket 03 reworks the loop to reconcile
  per batch of Changes; the shape is decided there.
- **Standards 4, 5, 6, 8:** two-variant matches, the record reading names through the CLI's
  spellings (they are the same names everywhere), a `String` parse error that stays internal, and
  cfg'd duplicates. Low value.
- **Spec (c)3, `.tidings` in any letter case:** no practical effect.

### Re-review of 4f672f4

**Standards.** No hard violations beyond missing doc comments on `StoreAddress`'s fields and
`Record::area`. Items 3–6 were done cleanly. Judgement calls:

1. Possible Speculative Generality: `PartialEq` on `StoreLocation`, `Identity` and
   `StoreAddress` is unused until ticket 04.
2. Possible Duplicated Code: `address()` probes the Store with `detect()`, and `open` probes it
   again; the `Memory` check is repeated.
3. Possible Feature Envy: `StoreAddress::open` mostly calls into `self.location`.
4. Possible Data Clumps: `Opened { backend, description }` restates a `StoreAddress` (partly
   unavoidable, since a memory Store has no address).
5. A flag argument: `lock_file(name, wait: bool)`, where `false` means "don't lock".
6. Possible Mysterious Name: `is_empty` also accepts a record-less `.tidings/`.
7. `holds_no_record` hard-codes `atomic-write-file`'s undocumented temp prefix.
8. `is_empty` compares `.tidings` case-sensitively, where `scan` doesn't.

**Spec.** All 11 tests pass 5 runs of 5, and both new tests fail on d78a75b. Probes confirm the
lock: `sync` blocks in `create` while the lock is held, and three commits during a running `sync`
commit once and then find nothing to commit, with the record holding both new Bases. The crash
leftovers can't make a folder with user files count as empty. Findings:

- The concurrency test can pass vacuously on a slow machine: nothing checks that both commits
  were actually blocked before the lock is released.
- `sync` now honours `--create`, as the spec asks (lines 256-257): a gap the first review missed.
- **(c)1:** `sync` opens, and with `--create` creates, the Store before checking the folder, so a
  refused `sync` into a non-empty folder leaves a new Store behind.
- For later: once tickets 03 and 07 add `tmp/` and `ignore` to what `create` makes, the list of
  allowed leftovers must grow, or crash recovery breaks again.

### Resolution of the re-review

Fixed in dcbd2bb:

- **(c)1, a refused `sync` leaving a new Store:** fixed. `sync` calls
  `WorkingCopy::check_can_create` before it opens or makes the Store. Test:
  `sync_into_a_folder_that_isnt_empty_makes_no_store`.
- **The concurrency test passing vacuously:** fixed. It asserts that both commits are still
  running (blocked on the lock) before releasing it.
- **Standards 2, the double probe:** fixed. `StoreArgs::open_with_address()` probes the Store's
  location once and checks for `Memory` once.
- **Standards 5, the flag argument:** fixed. `lock_file(name, wait)` became `open_lock_file` and
  `lock_waiting`.
- **Standards 6 and 8:** fixed. `is_empty` became `is_missing_empty_or_unfinished` and matches
  `.tidings` in any letter case, as `scan` does. `holds_no_record` notes that its list of allowed
  leftovers must grow when `create` makes `ignore` and `tmp/`.
- **Missing doc comments:** added.

Not acted on: `PartialEq` on the address types stays for ticket 04, which is next. The temp
prefix coupling is documented rather than removed, since `atomic-write-file` offers no way to ask.

Note: one library test, `an_area_directory_removed_while_running_is_made_again_with_a_resync`,
failed once during this run and passed on rerun. The change doesn't touch the library; it may be
a timing-sensitive test worth watching.

---

## Ticket 03: `sync` follows the Store

Reviewed: `git diff 41b0992...17da6f2` (commit 17da6f2).

### Standards

**(a) Documented-standard violations:** none hard. No line over 100 columns, every new item is
documented, tests run through the binary on fs and SQLite. Soft departures from spec 0002:

1. **Interface shape, `working_copy.rs:190, 257`:** the spec names `reconcile(paths | all)` as
   the module's interface; the diff makes `reconcile` private and exposes
   `sync(store, feed, stop, report)`. That makes the module deeper and settles ticket 02's
   Standards item 1, but the spec's "Implementation Decisions" no longer describe it exactly.
2. **"Tests wait on what `sync` says rather than sleeping":** `sync_quiet_prints_only_what_needs_attention`
   polls with `wait_until` (unavoidable under `--quiet`); `ctrl_c_during_a_reconcile_lets_it_finish`
   spin-sleeps; `wait_until` sets its own 400×50 ms timeout, duplicating `EVENT_TIMEOUT` outside
   the shared harness.
3. `tests/working_copy.rs:313` reads `.tidings/tmp`: internal layout, not the record's format, so
   allowed, but coupling.

**(b) Judgement calls:**

1. **Possible Mysterious Name, `output.rs:144-146`:** `SyncOutput { pub output: Output }` is
   documented as "Whether to print JSON", which describes a field of `Output`, not this one.
2. **Possible Duplicated Code:** folder writes use `tempfile` + `sync_all` + `persist`
   (`working_copy.rs:357`), the record uses `AtomicWriteFile` (`record.rs:88`). Allowed by the
   spec, but one idea built twice.
3. **Inconsistent error seam, `working_copy.rs:340` vs `:348`:** `write` returns `io::Result`
   mapped by the caller, `remove` returns `Failure`.
4. **Possible Duplicated Code in tests:** the Resync trigger and its setup repeat at `:422` and
   `:506`.
5. **Possible Repeated Switches, `output.rs:154`:** the `--quiet` filter and the name mapping both
   switch on `SyncEvent`; ticket 05's *diverged* must edit both. Tolerable.
6. **Readability, `working_copy.rs:190-236`:** the nested `loop { select! … }` is dense.

### Spec

The reviewer probed the binary on fs and SQLite: live *created*/*updated*/*removed* lines each
followed by *caught up*; a directory holding a user file is kept; a File replacing a directory and
the reverse; a commit during `sync` gives no *updated* line; local edits are left alone. The feed
is taken at Store open, before the first reconcile; the record is reread under `lock` on every
reconcile; Ctrl-C is polled only between reconciles.

**(a) Missing or partial:** none. The Resync test runs on fs only, since SQLite sends one only
after a 10-minute lag.

**(b) Scope creep (minor):** new files get 0666 before the umask, and a replaced file keeps its
permissions (reasonable: tempfile's 0600 would be a regression). The `--quiet` help says "only
resyncs and errors", which ticket 05 must extend with *diverged*.

**(c) Implemented but wrong**

1. **Writes and deletes escape the folder through a symlinked directory.** Spec: "a symlinked
   directory on the way … the Path is Diverged instead"; story 16: "`sync` never deletes anything
   it didn't put there". Probe: with `wc/a` replaced by a symlink to `ext/a`, a Store change to
   `a/c` and delete of `a/b` made `sync` write `ext/a/c` and delete `ext/a/b`, outside the Working
   copy, then die with "Not a directory". `local()` (`:440-459`) checks only the last component;
   `write`/`remove` follow symlinks in parent directories.
2. **One blocked apply ends `sync` and skips the record save.** Spec: "If applying fails because
   of what is in the folder … `sync` carries on". Probe: an untracked local file `q`, then the
   Store writes `q/r`: `sync` exits 1, and the `?` in the apply loop (`:282-287`) returns before
   `save`, so Paths already applied aren't recorded (the self-heal covers it next run).

### Summary

Standards: 0 hard violations, 3 soft departures and 6 judgement calls (worst: the misdescribed
`SyncOutput` field). Spec: 2 wrong (worst: writes and deletes outside the folder through a
symlinked directory).

### Resolution

Fixed in 3943030:

1. **Spec (c)1, writes and deletes through a symlinked directory:** fixed. Before anything reads,
   writes, removes or cleans up for a Path, every directory from below the folder down to it must
   be a real directory or missing; a symlink or a file on the way blocks the Path. Test:
   `nothing_outside_the_folder_is_changed_through_a_symlinked_directory` (unix).
2. **Spec (c)2, a blocked apply ending `sync`:** fixed. A new `SyncEvent::Blocked` leaves the
   Path's Base as it was, the other Paths are still applied, the record is saved and `sync` keeps
   running. It prints as `error <path>: <reason>` (JSON `{"event":"error",…}`), even with
   `--quiet`. It is a stand-in for ticket 05, which makes such a Path Diverged. Tests:
   `a_path_blocked_by_a_local_file_is_reported_and_the_rest_still_applied`,
   `sync_quiet_prints_a_blocked_path`.
3. **Standards (b)1, the `SyncOutput.output` doc:** fixed.
4. **Standards (b)3, the error seam:** `write` returns `Failure`, like `remove`.
5. **Standards (a)2 and (b)4, test waits and setup:** `wait_until` moved to the harness and uses
   `EVENT_TIMEOUT`; the Resync trigger is the helper `clear_the_cache_directory`.
6. **Standards (b)6, the dense loop:** the feed-waiting part is now `wait_for_paths`.

All three new tests fail on 17da6f2.

Not acted on:
- **Standards (a)1, the interface shape:** kept. `sync(store, feed, stop, report)` is deeper than
  the spec's `reconcile(paths | all)`, as ticket 02's review asked. **For the spec's owner:** the
  "Where it lives" bullet in spec 0002 could be updated to match.
- **Standards (b)2, two atomic-write mechanisms:** the spec places folder writes in `.tidings/tmp/`,
  which `atomic-write-file` (it writes next to the target) doesn't do.
- **Standards (a)3, a test reading `.tidings/tmp`:** it checks that no temporary file is left over,
  which a person can see.

Note: one library test, `fs::blocking::a_prefix_delete_removes_what_is_under_the_prefix_when_committed`,
failed once in this run and passed on rerun, a second timing-sensitive library test.

### Re-review of 3943030

**Standards.** No hard violations; all claimed fixes are in. Judgement calls:

1. **Possible Repeated Switches, getting worse (`output.rs:157-175`):** the output match grew to a
   three-part tuple with 7 arms, two of which silently drop a message.
2. `check_directories` returns `Result<Result<(), Blocked>, Failure>` beside a private
   `struct Blocked(String)` and `SyncEvent::Blocked`; and it re-splits the Path by hand next to
   `path_in_folder`.
3. `write` became a one-line wrapper around `write_io`, while `remove` maps inline.
4. `wait_for_paths` also reports *caught up* and *resync*; documented, acceptable.
5. The blocked-path test only checks `contains('q')` on the message.

**Spec.** Probes confirm nothing outside the folder is touched: symlinked directories at depth 1
and 2, a leaf symlink to a directory or file outside, a folder named through a symlink or as `.`,
and cleanup walking only approved directories. Findings:

1. **Regression, a stale Base.** Spec row 3: "`S` becomes the Base silently". The directory check
   runs before the table's rows are decided, so when directory `q` is replaced by a local file and
   the Store deletes `q/r`, `sync` reports an error and keeps the Base although both sides are
   absent; after `rm q`, `commit` exits 3 with a Conflict. Before the fix it said nothing to
   commit.
2. A window between checking the directories and using them: a concurrent process deliberately
   swapping a directory for a symlink could still redirect a write. Not a realistic concern.
3. A leaf that isn't a regular file (`Local::Other`) where the Store changed is left alone and
   prints nothing, while a blocked directory prints `error`. Inconsistent until ticket 05.

### Resolution of the re-review

Fixed in a180fc3:

- **The stale Base regression:** fixed. `reconcile_path` decides the table's rows first and only
  refuses an actual apply. A file where a directory should be means the Path is locally absent, so
  a Store removal becomes the Base silently (row 3). Under a symlinked directory nothing is read
  and the Path is still reported. Test:
  `a_removal_under_a_local_file_needs_nothing_applied_so_takes_the_base_silently` (fs and SQLite),
  which fails on 3943030.
- **The output switch:** each event now maps to a name, a Path and a message, from which both the
  JSON object and the text line are built, so ticket 05's *diverged* adds one arm.
- **The nested Result:** the check is now `blocking_directory`, returning
  `Result<Option<Blocked>, Failure>`, with `Blocked` an enum (`Symlink`, `NotADirectory`); the
  walk uses `path_in_folder(path).ancestors()`.
- **`write`/`write_io`:** `write_io` is gone; `write` maps its errors inline, like `remove`.
- **The weak test assertion:** it checks the exact message and labels the Backend.
- **The check-then-use window:** documented at the `apply` call as accepted.

Not acted on: a leaf that isn't a regular file prints nothing while a blocked directory prints an
error (re-review Spec 3). Ticket 05 reports both as Diverged. A related change for ticket 05 to
pick up: with a local file where a directory should be, a Path with a Base that the Store changes
is now left alone silently (it is row 4, Diverged) where 3943030 reported an error.

No second re-review: these fixes are narrow and each has a test.

---

## Ticket 04: Resuming a Working copy, and refusing the wrong folder or Store

Reviewed: `git diff 0fa4b53...51c97f5` (commit 51c97f5).

### Standards

**(a) Documented-standard violations:** none. No line over 100 columns; every new item documented;
the _Avoid_ words present ("version" for the record's format, `same_directory` for a directory on
disk, "updated" as an event name) are uses earlier reviews allowed. Tests go through the binary on
fs and SQLite without reading the record's format.

**(b) Judgement calls:**

1. **Command not thin, `main.rs:192-213`:** `sync` holds the Working copy lifecycle (the memory
   refusal, `exists`, then `open` + `check_area` + `open_store`, or `check_can_create` +
   `open_with_address` + `create`). The spec's "Where it lives" gives opening and creating to the
   module; this also widens its interface with `exists` and `check_area`.
2. **Possible Duplicated Code, `location.rs:246`:** `is_memory()` exists, but the same test is
   still written inline at `:215` and `:223`.
3. **Possible Duplicated Code:** the memory refusal message is built in `main.rs:193-195`, apart
   from `location.rs`'s `memory_outside_the_shell()`.
4. **Possible Feature Envy, `location.rs:252-279`:** `StoreArgs::check_matches` mostly inspects
   the address. Tolerable, same module.
5. **Possible Duplicated Code in tests:** the shape `tidings()` + `--json sync config <folder>` +
   `Sync::spawn` repeats five times.
6. `let [first_sync, second_sync] = syncs; let mut second_sync = second_sync;` could be one
   pattern.
7. `std::path::Path` and `std::fs` written out in full at `location.rs:319-321`.

### Spec

Probed on fs and SQLite. Working as asked: resuming after Store changes while stopped (a local edit
left alone), after SIGKILL, and after moving or renaming the folder; a stale `TIDINGS_ROOT` refused
for `sync` and `commit`, naming the path; `--root` as a relative path, with a trailing slash or
through a symlink accepted; `--identity` against a Root record and a `--backend` mismatch refused,
naming the difference; the wrong Area refused before the Store is opened; memory refused; a
non-empty folder refused and untouched; a Store that has gone refused, with nothing created.

**(a) Missing or partial:** none. (A resumed Path that diverged printed nothing: ticket 05.)

**(b) Scope creep:** none of substance.

**(c) Implemented but wrong**

1. **`--create` on a Working copy whose Store has gone wipes the folder.** Story 76: "never
   corrupts my Working copy". `open_store` passes `--create` through (`working_copy.rs:225`), so
   `tidings --create sync config wc` made an empty Store, printed `removed …`, and deleted every
   unchanged file; only a locally edited file survived. The error just before invites exactly that
   command ("there is no Store at …: pass --create and --backend to make one"). The ticket's
   "never creates a new Store without `--create`" allows it to the letter, but the spec's "For
   every other case they are optional, and any given must match the record" treats the flags as a
   check, not a permission. No test covers it.
2. **A second `sync` opens, and with `--create` makes, a Store before refusing.** `open_store`
   (`main.rs:203-206`) runs before `WorkingCopy::sync` takes `sync.lock`. Probe: with a `sync`
   running and the Store moved away, `tidings --create sync config wc` made a new Store and only
   then said a `sync` was running already. The same ordering problem ticket 02's re-review fixed
   for `check_can_create`.

### Summary

Standards: 0 hard violations, 7 judgement calls (worst: the `sync` command, not the module,
deciding between resuming and creating). Spec: 2 wrong (worst: `--create` on a Working copy whose
Store has gone deletes its unchanged files).

### Resolution

Fixed in 6fe79fb:

1. **Spec (c)1, `--create` wiping a Working copy:** fixed by refusing `--create` for an existing
   Working copy. **A decision for the spec's owner:** the ticket's "never creates a new Store
   without `--create`" allowed `--create` to recreate a missing Store, but that deletes every
   unchanged file in the folder to match the new, empty Store. Refusing it follows the spec's "For
   every other case they are optional, and any given must match the record": the flags are a
   check, not a permission. `StoreAddress::open` no longer creates, and a missing Store is reported
   as "the Store of the Working copy F is missing at L (backend)", without the `--create` hint,
   which stays for `tidings store …` and a new Working copy. Test:
   `a_working_copy_whose_store_has_gone_fails_to_open` (fs and SQLite): `commit` and `sync`, plain,
   with `--create`, and with the recorded flags plus `--create`, are all refused, no Store is made
   and every file stays. It fails on 51c97f5.
2. **Spec (c)2, a second `sync` opening a Store:** fixed. The sync lock is taken before the Store
   is opened. Test: `a_second_sync_of_a_working_copy_is_refused` moves the Store away and checks
   the second `sync` says one is running and makes nothing. It fails on 51c97f5.
3. **Standards 1, the command not thin:** fixed. `WorkingCopy::sync(folder, area, &StoreArgs,
   stop, report)` is the one entry point; `exists`, `check_area`, `check_can_create` and `create`
   are private. (The spec's "Where it lives" says commands "open the Store, and call it"; `sync`
   now leaves opening to the module, as that section's intent asks.)
4. **Standards 2 and 3, the memory checks:** `is_memory()` is private and used throughout
   `location.rs`, and the refusal moved there as `check_not_memory()`.
5. **Standards 5, 6 and 7:** `Sync::start_with(flags, area, folder)` replaces the repeated spawns
   where the shape matches; the other items fixed.

Not acted on: Standards 4 (`check_matches` next to the data it inspects, in the same module).

### Re-review of 6fe79fb

**Standards.** Every claimed fix is in place. One hard violation: a doc line at
`working_copy.rs:144` is 101 columns. Judgement calls: the memory check runs twice on the create
path (`open_or_create`, then `open_with_address`); `open_or_create` returns a three-part tuple
(private, used once: tolerable); in a test, `run` holds first a `Command` then its output, and
`command` is a `&[&str]` where elsewhere it is a `Command`; the `tidings()` + args + `run` shape
still repeats three times.

**Spec.** Probed on fs and SQLite: `--create` is refused on an existing Working copy whether alone,
with matching flags or with a matching `TIDINGS_ROOT`, on `sync` and `commit` (there is no
environment variable for `--create`); the missing-Store error no longer suggests it and nothing is
made; a second `sync` says one is running with the Store present, moved away or with `--create`;
`commit` during `sync` works; Ctrl-C exits 0; everything ticket 04 did still holds; new Working
copies (missing, empty, crash-leftover `.tidings/`) still work. Both new tests fail on 51c97f5.
Findings:

1. The ticket's wording ("never creates a new Store without `--create`") now reads as though
   `--create` would make one; neither the ticket nor the spec records the decision.
2. On the new-Working-copy path the Store is opened, and possibly made, before `create` and the
   sync lock, so two `sync --create` runs racing on one empty folder can both open or make the
   Store, and the loser says "is a Working copy already" rather than that one is running. Read from
   the code; the window is too narrow to probe.

### Resolution of the re-review

Fixed in 5defbdb:

- **The race on a new Working copy:** fixed. The create path now works out the Store's address
  without opening it (`StoreArgs::address_for_working_copy`), checks the folder, makes the folder
  and `.tidings/`, takes `sync.lock`, checks the folder again under it, and only then opens or
  makes the Store and saves the record. A refusal for wrong flags, memory, a missing `--create` or
  a non-empty folder makes neither a Store nor the folder. If opening the Store itself fails,
  `.tidings/` is left holding only lock files, which `holds_no_record` counts as unfinished. No
  race test: it can't be made deterministic.
- **The double memory check:** one check, inside `address_for_working_copy`. On the resume path
  `--backend memory` now gets the "doesn't match the Working copy's Store" error rather than the
  memory error; a record can never name a memory Store, so that is accurate.
- **The 101-column line and the test names:** fixed.

Left for the spec's owner: updating ticket 04's `--create` wording (re-review Spec 1).

No second re-review: the restructure only reorders existing checks, and the existing tests pass.

---

## Ticket 05: Divergence

Reviewed: `git diff 2dd7196...d043802` (commit d043802).

### Standards

**(a) Documented-standard violations:** none hard. No line over 100 columns; every new item is
documented; the report is printed by the output module; tests go through the binary on fs and
SQLite and treat `.tidings/theirs/` only as a place the person can see.

- Glossary (judgement call): `record.rs:57-60` says "which of the Store's versions `theirs`
  holds" beside a `Revision`: "version" for a Revision is an _Avoid_ word. (Earlier reviews
  allowed "the Store's version" for contents, not for a Revision.)

**(b) Judgement calls:**

1. **Possible Mysterious Name, `theirs`:** one flow uses it for an `Option<&File>`, an
   `Option<PathBuf>`, an `Option<Revision>` and a `String`; worst at `working_copy.rs:511`,
   `let theirs = theirs.map(|_| theirs_file(path));`.
2. **Possible Primitive Obsession, `working_copy.rs:463-466`:** "`{path} is a directory`" is a
   blocking reason built ad hoc beside the `Blocked` enum, and `SyncEvent::Diverged.blocked` is an
   `Option<String>`.
3. **Possible Duplicated Code, `working_copy.rs:550-574`:** `write_theirs` and `remove_theirs`
   both build the `theirs` path and check it with `blocking_directory`.
4. **Possible Duplicated Code in tests:** two tests inline `.tidings/theirs/app.toml` beside a
   `theirs()` helper.
5. **Possible Data Clump:** `(record, path, theirs, blocked)` travels into `diverge` from three
   sites. Tolerable.
6. **Possible Repeated Switches, `working_copy.rs:445-453`:** `unchanged` and `same_as_theirs`
   both had to learn `Local::Directory`.

Noted for Spec: Diverged entries are separate record lines, not part of a Base entry as the spec's
record section describes, because a Diverged Path may have no Base.

### Spec

Probed on fs and SQLite: every row of the table live and across a stopped-then-resumed `sync`,
stories 30–34, `--quiet` and `--json`; `theirs` refreshed; *resolved* printed; blocked applies
reported as Diverged ("q is a directory; …"). A kill -9 during a 120-Path resume healed: after
resuming, all 120 were Diverged with `theirs` holding the Store's version and local files
untouched.

**(a) Missing or partial:** none. Each row has a test on both Backends.

**(b) Scope creep:** none.

**(c) Implemented but may be wrong**

1. **A `theirs` file that has gone is never written again.** "`theirs` gets `S`": `diverge`
   (`:503`) returns early when the record holds the same Revision without checking the file
   exists, so after a crash between removing `theirs` and saving the record, or a person deleting
   `theirs/`, it never comes back.
2. **One `theirs` write failure ends `sync`.** Story 34: "`sync` keeps running". If
   `.tidings/theirs` is a symlink or a file, `write_theirs` errors, `sync` exits 1 before saving
   the record, and every resume fails the same way. Nothing escapes `.tidings/`.
3. **A Path created locally stays Diverged when the Store creates and then removes it.** Row 1:
   "`S` matches `B` (both absent…) | anything | nothing". The Store writing then deleting `n` over
   a local `n` with no Base leaves `n` Diverged ("removed in the Store"), though nothing disagrees
   any more, and a full commit would be blocked until `resolve`. Ambiguous between row 1 and "A
   Path already Diverged is reconciled the same way"; should be decided.

**The implementer's judgement calls, assessed:**

- Unchanged local file and Store contents equal under a new Revision: the Base is taken silently
  rather than rewritten and reported *updated*. Accepted: nothing changes on disk.
- A local directory at a Path counts as absent there ("Directories exist only as Prefixes").
  Accepted.
- A Diverged Path the person deletes: with no Base it is row 2 (apply the Store's version); with
  a Base it is row 4 (stays Diverged). Both follow the table, and neither loses anything.
- JSON *diverged* names the `theirs` file only in `message`. Story 23 is about following a Working
  copy from another program, which would have to parse prose: a `theirs` field would help.

### Summary

Standards: 0 hard violations, 1 glossary slip and 6 judgement calls (worst: `theirs` naming four
different things). Spec: 3 possibly wrong (worst: a failure writing `theirs` stops `sync` for
good).

### Resolution

Fixed in 1f3f649:

1. **Spec (c)1, a missing `theirs` never written again:** fixed. A Diverged Path whose `theirs`
   file is missing gets it written again, with no new *diverged* line unless the Revision changed.
   Test: `a_missing_theirs_is_written_again`.
2. **Spec (c)2, a `theirs` write failure ending `sync`:** fixed. The Path is recorded Diverged all
   the same, an *error* names it (even with `--quiet`), `sync` carries on, and a later reconcile
   retries. Test: `a_theirs_that_cant_be_written_is_an_error_and_sync_carries_on`.
3. **Spec (c)3, a Divergence outliving its cause:** decided: a Diverged Path whose Store state
   matches its Base again (both absent, or the same Revision) is no longer Diverged. It is table
   row 1, "local edits stay local", and a commit would go through. `theirs` is removed and
   *resolved* printed. Test: `a_divergence_clears_once_the_store_is_back_at_the_base`, with and
   without a Base (the library gives the same Revision for the same contents).
4. **JSON `theirs` field:** added: the `theirs` file relative to the folder, or `null` when the
   Store removed the File.
5. **Standards:** the glossary slip is fixed; `theirs` is split into `store_file`, `theirs_file`
   and `theirs_revision`; `Blocked::Directory` joins the enum and the event carries
   `Option<Blocked>`; `write_theirs` and `remove_theirs` share `theirs_at`; tests use `theirs()`.

All the new or changed tests fail on d043802.

**For the spec's owner:** the fix agent also edited spec 0002 ("Reconciling a Path" and the `sync`
output bullet) to describe items 1–4. The edit is accurate and changes nothing else, but it was
not asked for; review or revert it as you see fit.

Not acted on: Standards 5 (the Data Clump into `diverge`) and 6 (two switches learning
`Local::Directory`), tolerable.

### Re-review of 1f3f649

**Standards.** All claimed fixes are clean. One small hard violation: the fields of the
`Error { path, message }` event (`working_copy.rs:88`) have no doc comments. Judgement calls: the
resolve-and-remove-`theirs` block is duplicated in `reconcile_path` (`:448-451`, `:476-482`);
`diverge` now takes five parameters; `Some(_) =>` at `:461` no longer says only a symlink reaches
that arm; `Blocked` mixes filesystem paths and a Tidings `Path`; `theirs_at` returns
`Result<Result<PathBuf, Blocked>, Failure>` and is worked out twice when writing.

**Spec.** Probed on fs and SQLite; all four fixes work, the six new or changed tests fail on
d043802, and the spec edit is accurate. Nothing was ever written or removed outside
`.tidings/theirs`. Findings:

1. **A `theirs` that can't be *removed* still ends `sync`.** With `.tidings/theirs/c` replaced by
   a directory, the Store deleting `c` (or going back to the Base) makes `sync` exit 1 without
   saving the record, on every restart. The same class as the original finding 2.
2. **Error noise:** every batch re-reconciles every Diverged Path, so while `theirs` is blocked,
   each unrelated Store change re-prints one *error* per Diverged Path, even with `--quiet`.
3. **The *diverged* line names a `theirs` file that wasn't written** when the write failed, and the
   JSON `theirs` field points there; the *error* line after it contradicts it.
4. Minor: the error repeats the path, the second time absolute.

### Resolution of the re-review

Fixed in ddfeefe:

1. **A `theirs` that can't be removed:** fixed. `theirs` is now written or removed in one place
   (`settle_theirs`), after the record is updated; a failure is an *error* for that Path, `sync`
   carries on, the record is saved, and later reconciles retry. A cleared Divergence stays cleared
   and the person's directory in `theirs/` is left alone. The spec line covers removal too. Limit:
   a leftover `theirs` file for a cleared Divergence is retried only while that `sync` runs, since
   the retry isn't recorded. Test:
   `a_theirs_that_cant_be_removed_is_an_error_and_sync_carries_on` (back to the Base, and removed
   in the Store).
2. **Error noise:** fixed. `sync` keeps an in-memory set of Paths whose `theirs` failed; they are
   retried silently each reconcile and reported again only when the Divergence changes. Test:
   `a_theirs_that_cant_be_written_is_reported_once_until_the_divergence_changes`.
3. **Untruthful *diverged* line:** fixed. When `theirs` can't be written, only the *error* is
   printed ("Diverged, but can't write .tidings/theirs/a: …").
4. **The repeated, absolute path:** fixed.
5. **Standards:** the `Error` fields are documented; one `resolve` helper replaces the duplicated
   block; an explicit `Blocked::Symlink` pattern says which case reaches that arm; `settle_theirs`
   replaces `theirs_at`, `has_theirs`, `write_theirs` and `remove_theirs` and works out the
   location once.

The new tests fail on 1f3f649. No third review: each fix is narrow and tested.

Note: a third library test, `fs::a_file_replaced_then_removed_straight_away_is_reported_removed`,
failed in the fixer's two full workspace runs and passed alone. The orchestrator's own full run
afterwards passed all 467 tests. The library is untouched by this spec; together with the two
noted under tickets 02 and 03, the fs watcher tests look timing-sensitive under load and may be
worth a look separately.

---

## Ticket 06: Commit: Conflicts, Divergence and naming paths

Reviewed: `git diff 399a2d3...bcbab60` (commit bcbab60). (The implementer was interrupted by a
usage limit and resumed; the review covers the final commit.)

### Standards

**(a) Documented-standard violations:** none hard. No line over 100 columns; every new item is
documented; tests go through the binary on fs and SQLite. Soft departures:

1. **Spec 0002, "reports and failures are added to [the output and failure modules] rather than
   printed separately":** `working_copy.rs` now imports `output::event_line` and builds the
   finished person-facing text of the Conflict and Diverged-refusal failures (`listing` at
   `:1177`, the "same {path}: …" line at `:866-879`). The failure module gets an opaque string, so
   `--json` gets prose.
2. **CONTEXT.md, Prefix:** the doc on `Chosen::Under` (`:158-159`) calls something "a Prefix
   without its last `/`", which the glossary says is not a Prefix.
3. **Testing Decisions:** a test sleeps 200 ms (`tests/working_copy.rs:207`); two reach into
   `.tidings/lock` and `.tidings/theirs` (internal layout, not the record's format: coupling).

**(b) Judgement calls:**

1. **Possible Primitive Obsession / Mysterious Name, `working_copy.rs:155-160`:**
   `Chosen::Under(Vec<(PathBuf, String)>)`, with `""` meaning the whole folder; `Chosen` doesn't
   say it is a selection of Paths.
2. **Possible Duplicated Code:** the `sync_running` scaffolding in two tests; "reconcile the
   Paths, then save" in both the `Pending` arm and `reconcile_conflict`.
3. **Possible Divergent Change / long method:** `WorkingCopy::commit` (`:756-849`) selects,
   refuses, stages, commits, handles three outcomes and builds the report.
4. **Possible Feature Envy:** `output::event_line` made public for the Working copy module's
   failure text.

### Spec

Probed on fs and SQLite. Working as asked: paths relative to the current directory from a
subdirectory, `..`, absolute paths, `-C` with a path through a symlink into the folder; a symlink
pointing out of the folder, and `.TIDINGS/x`, refused as outside (exit 1); a named directory
meaning everything under it; a named deleted file committing the delete; Diverged names refused
(exit 3) while other names go ahead; a Conflict without `sync` (exit 3, Path named, `theirs`
written, nothing committed per `store read`) and with `sync` running; story 48 (`sync` prints
only *caught up* after a commit); `commit` blocking on a held lock; `--json` Revisions equal to
`store stat`; the unnamed rest keeping its Bases after a partial commit.

**(a) Missing or partial:** none in the ticket. But:

- **`Pending`, for the spec's owner.** The spec says `Pending` "exits 0, as `store write` does",
  but `store write` exits 1 on `Pending` (`failure.rs:84` maps it to a plain error), so the spec's
  premise is false. `commit` follows the ticket and README ("`Error::Pending` means the commit
  succeeded") and exits 0. Either `store write` should change or the spec's wording should.

**(b) Scope creep:**

- **For the spec's owner:** `commit nope.toml`, naming nothing that exists or is recorded, exits
  1 "no such file in the Working copy" rather than "nothing to commit". Story 47 ("running it
  twice is harmless") still holds, since an unchanged named file gives "nothing to commit". Ticket
  07 must adjust the message for a named *ignored* file.
- Stricter than the spec but consistent with story 44: naming a directory holding a Diverged Path
  refuses the whole commit.

**(c) Possibly wrong:**

1. **`Pending` discards reconcile events** (`:825-830`): if another Commit lands in between, a
   Path becomes Diverged silently (`revision: null`, nothing says Diverged).

The implementer's other calls were judged sound: the Conflict test's "either way" assertions with
`sync` running, and two older tests now reconciling before committing, since a full commit is
refused while anything is Diverged (the record stays Diverged until a reconcile, `discard` or
`resolve`, even after the person undoes their edit, which ticket 08's `status` should make
visible).

### Summary

Standards: 0 hard violations, 3 soft departures and 4 judgement calls (worst: the module
formatting its own failure text, so `--json` gets prose). Spec: 1 possibly wrong (`Pending`
dropping a Divergence) and 2 questions for the owner (`store write`'s `Pending` exit code;
refusing a name that matches nothing).

### Resolution

Fixed in 9149597:

1. **Standards (a)1, failure text built in the module:** fixed. The Diverged refusal and the
   Conflict are structured failures (`Failure::diverged`, `Failure::conflicted`) carrying each
   Path's outcome as data; `failure.rs` renders them as the same text as before, or with `--json`
   as one JSON object on stderr, e.g.
   `{"failure":"conflict","message":"Conflict: …","paths":[{"event":"diverged","path":"app.toml","message":"…","theirs":".tidings/theirs/app.toml"},{"event":"same","path":"same.toml"}]}`.
   Exit code 3 as before.
2. **Spec (c)1, `Pending` dropping a Divergence:** fixed. `CommitReport.events` carries what the
   reconcile found, printed after the change lines and as `"events"` in JSON. No binary test can
   time another Commit landing in between, so an in-process test covers the output.
3. **Standards (a)2, the Prefix wording:** fixed.
4. **Standards (b)1:** `Chosen` became `Selection { All, Named(Vec<NamedPath>) }`, with
   `NamedPath { given, target }` and `Target { Folder, Under(String) }`, so the `""` sentinel is
   gone.
5. **Standards (b)2 and (b)3:** `commit` is split into `refuse_diverged`, `stage` and
   `record_commit`, with the record saved in one place; the tests share `with_a_diverged_path`.
6. **Standards (a)3, the sleep:** kept, with a comment: nothing `commit` does before it blocks on
   the lock can be observed, and the test's checks hold either way.

Fixed in ac2b59b, **a flaky test from ticket 05:** `a_divergence_clears_once_the_store_is_back_at_the_base`
failed about one run in four. Root cause: the test made two separate Store Commits and assumed
the first *caught up* would follow both *diverged* events; separate Commits can reach `sync` as
two batches. Reproduced 12 of 12 under load; the code's behaviour was correct. A `wait_for_count`
helper waits through each *caught up* until the expected events have been printed; the test still
checks the exact sets of Paths. 35 of 35 passes under the same load. A similar loop in
`a_theirs_that_cant_be_removed_is_an_error_and_sync_carries_on` got the same fix.

Left for the spec's owner: `store write`'s `Pending` exit code, and refusing a named path that
matches nothing (Spec (a) and (b) above).

### Re-review of 9149597 and ac2b59b

**Standards.** Every claimed fix is clean. Findings:

1. **Mixed `--json` behaviour (soft, spec "reports and failures are added to [the output and
   failure modules]"; README "Text for a person, or JSON with `--json`"):** now only the Conflict
   and the Diverged refusal print JSON under `--json`; every other failure, including a
   `store write --if-absent` Conflict, prints text. A script can't know which to expect.
2. The in-process test at `output.rs:329` is named for reconciling but only formats a hand-built
   report through the private `as_json`.
3. A JSON `"same"` event, which `sync` never prints, has no `message` unlike the others.
4. Judgement calls: `print` and `Display` in `failure.rs` switch in parallel on the structured
   failures, as do `reconciled_json`/`reconciled_line`; `reconcile_after_commit` is a thin wrapper
   (its doc justifies it); `files` and `changes` travel together through `stage` and
   `record_commit`; `Failure.paths` holds outcomes, not Paths; `Failure::conflict` is still `pub`
   with a stale doc; `TookStoresVersion` uses "version". The Conflict's text form is no longer
   tested through the binary.

**Spec.** Text for a person is unchanged from bcbab60 on both Backends (outputs diffed); only
`--json` runs differ. Every ticket 06 probe gives the same result as before the refactor. The flake
fix doesn't weaken its test (exact sets of Paths still checked); 8 loops under load passed; the
old failure couldn't be reproduced in 10 runs of bcbab60 by this reviewer, so the root cause is
plausible rather than confirmed by this probe. Minor: every commit's `--json` now has
`"events":[]`, not only a Pending one.

### Resolution of the re-review

Fixed in 81754f5:

1. **Mixed `--json` behaviour:** decided and fixed. **A decision for the spec's owner:** under
   `--json`, every failure now prints one JSON object on one line on stderr,
   `{"failure": <kind>, "message": …}`, with `"paths"` added for the Conflict and the Diverged
   refusal. The kinds are `error` (exit 1), `missing` (exit 2), `conflict` (exit 3) and
   `diverged` (exit 3). This changes what `tidings store …` commands print on failure under
   `--json` (before, `tidings: …` text), e.g. `tidings --json store write data a.txt --contents b
   --if-absent` now prints `{"failure":"conflict","message":"Conflict: a Precondition did not hold
   for a.txt"}`. Text output and exit codes are unchanged. Not covered: clap usage errors, and the
   interactive shell's `error: …` line at its prompt. README's Output bullet gained one clause.
   Test: `with_json_a_failure_is_one_json_object_on_stderr` (a Conflict, a missing File, an
   error).
2. **The Conflict's text form:** tested through the binary again, alongside its JSON.
3. **The `"same"` entry:** has a message.
4. **The parallel switches:** one `EventParts` (name, path, message, `theirs`) renders both text
   and JSON for events and failures.
5. **Names:** the internal field `paths` became `outcomes` (the JSON key stays `"paths"`), `StoppedPaths` became `Outcomes`, `Failure::conflict` is
   private with its doc fixed, `TookStoresVersion` became `TookStoresFile`, and the in-process test
   is now `a_pending_commits_events_are_printed_with_it`.

Not acted on: `"events":[]` on every commit's JSON (a stable shape is easier for scripts than a
key that appears only sometimes), the `reconcile_after_commit` wrapper (documented), and the
`files`/`changes` pair (tolerable).

---

## Ticket 07: The ignore file, and files that can't be Files

Reviewed: `git diff 03e4bfc...5ead549` (commit 5ead549).

### Standards

**(a) Documented-standard violations:** none hard. No line over 100 columns; every new item is
documented; the design follows spec 0002 (`Failure::invalid` exits 1 listing every file, JSON via
`EventParts`, the `ignore` crate's `.gitignore` syntax, a file with a Base never ignored). Tests go
through the binary on fs and SQLite; reading `.tidings/ignore` is fine, since the person edits it.
Nits: one test's assert message lacks `{backend}`; the crash-leftover allowance depends on
`atomic-write-file`'s undocumented temp-file name (accepted before for the record).

**(b) Judgement calls:**

1. **Possible Primitive Obsession / glossary tension:** `EventParts.path` and `Selection` /
   `Target::covers` went from `Path` to `&str`, so a field named `path` can hold something that
   isn't a Path, and callers add `.as_str()`.
2. **Possible Mysterious Name:** `InvalidFile`; in CONTEXT.md a File is a Store File, and this is
   exactly a file that can't be one.
3. **Possible Primitive Obsession:** `Result<Path, Option<tidings::Error>>` with `None` meaning
   "not UTF-8".
4. **Possible Duplicated Code:** the ignore file's path built twice; a repeated `map_err`; joining
   a folder-relative path's parts into a name, and the `.tidings` check, written in both scan and
   select.
5. **Possible Repeated Switches:** each `Outcomes` variant needs an arm in two matches in
   `failure.rs`'s `print`. Tolerable at three.
6. **Possible long method:** `scan_directory` recurses, filters, names, ignores and classifies in
   one loop body.
7. **Module size:** `working_copy.rs` is 1397 lines; scanning and the ignore file are
   self-contained behind `scan(&bases) -> Scan` and would sit well in `working_copy/scan.rs`.

### Spec

Probed on fs and SQLite: a new Working copy's `ignore` holds the six defaults, and each is left
out by `commit`; naming an ignored file is a clear exit 1; a default removed lets `.DS_Store` be
committed, and put back, its edit and delete are still committed (story 52); a `cfg/` pattern
still commits edits to a Base'd `cfg/a.toml` while leaving out a new `cfg/b.toml`; one full commit
names every invalid file (`CON`, `con.txt`, `a:b`, `trail `, NFD `é.txt`, non-UTF-8 contents, a
symlink, a fifo, a nested symlink), exit 1, with a `{"failure":"invalid",…}` JSON shape; each can
be ignored; an invalid file outside the named paths doesn't block the commit; a tracked Path turned
into a symlink is invalid even when a pattern matches it; `mv app.toml App.toml` commits as an add
and a delete. (A non-UTF-8 name can't be made on APFS, so it is untested here.)

**(a) Missing or partial:** none. **(b) Scope creep:** none; keeping an existing `ignore` in an
unfinished `.tidings/` is fine.

**(c) Wrong or questionable:**

1. **A symlinked ignore file is read as empty:** with `.tidings/ignore` a symlink, `commit .x.swp`
   committed the swap file, against story 49 ("so that editor and OS leftovers don't land in my
   Store"), and inconsistent with a bad pattern stopping the command.
2. **Negation differs from `.gitignore`:** with `build/` and `!build/keep`, tidings commits
   `build/keep`; git doesn't (a file can't be re-included under an excluded directory). Spec: "It
   uses `.gitignore` syntax."
3. **Malformed patterns** exit 1 naming the file but not the line. Refusing is the safer choice
   (git skips them silently); the line number would make it friendly.
4. **For the spec's owner:** an ignored local `.DS_Store` with no Base, then the Store gets a
   `.DS_Store`: `sync` makes it Diverged (the table's "added … contents differ"), and full commits
   are then refused. "Local states" says such a file "is left out entirely", which read literally
   would make it absent, so `sync` would overwrite it. Diverged keeps local data; the spec should
   say so.

### Summary

Standards: 0 hard violations, 2 nits and 7 judgement calls (worst: `&str` standing where a Path
was). Spec: 3 wrong or questionable (worst: a symlinked ignore file letting leftovers into the
Store) and 1 question for the owner.

### Resolution

Fixed in c0c2717:

1. **Spec (c)1, a symlinked ignore file:** fixed. A `.tidings/ignore` that is a symlink or isn't a
   regular file refuses the commit (exit 1), saying why. A missing ignore file still means no
   patterns (see below). Test: `an_ignore_file_that_isnt_a_regular_file_refuses_the_commit`.
2. **Spec (c)2, negation under an ignored directory:** fixed to git's rule: the scan carries "a
   parent directory is ignored" down, so a file under it stays ignored whatever re-includes it; a
   Path with a Base is still never ignored. Test:
   `a_file_in_an_ignored_directory_is_ignored_whatever_re_includes_it`.
3. **Spec (c)3, malformed patterns:** still refused, now naming the line
   (`…/.tidings/ignore:3: error parsing glob 'a{b': …`). Test:
   `a_malformed_pattern_refuses_the_commit_naming_its_line`.
4. **Standards 1–3:** a `LocalName` type holds a name in the folder that may not be a Path;
   `EventParts.path` is a `Path` again; `covers(&Path)` stays beside `covers_name`; `InvalidFile`
   was renamed; `Result<Path, Option<Error>>` became an `AsPath` enum.
5. **Standards 4, 6 and 7:** scanning and the ignore file moved to `working_copy/scan.rs` behind
   `scan(folder, bases)`, with the ignore file's path built in one place, one name-joining helper
   and `.tidings` check shared with `select`, and `scan_directory` split up.
6. The test nit is fixed.

The three new tests fail on 5ead549.

**For the spec's owner:** an ignored local file with no Base where the Store then creates the
same Path is Diverged (Spec (c)4), which keeps local data; the spec's "left out entirely" could be
read as absent, which would let `sync` overwrite it. Also, a missing ignore file silently means no
patterns (as git treats a missing `.gitignore`), and nothing recreates it.

### Re-review of c0c2717

**Standards.** Every claimed fix is clean except one: the rename landed on `InvalidEntry`, and
"entry" is an _Avoid_ word for File in CONTEXT.md (hard). Nits: `Scanner.folder` lacks a doc
comment; `ignore_file` is `pub(super)` but used only in `scan.rs`. Judgement calls: `Selection` and
`Target` each gained the same three `covers*` methods; `EventParts` holds `path` and `local_name`
side by side though never both set; `as_path` takes a path only to check UTF-8.

**Spec.** Probed on fs and SQLite; the three new tests fail on 5ead549. A symlink, dangling
symlink, directory or fifo at `.tidings/ignore` refuses `commit` (full, named and `--json`);
`sync` doesn't scan, so it is unaffected. Nested ignored directories with negations, anchored
patterns, `**`, CRLF, comments, blank lines, `\#` and trailing spaces all behave as in git; Base'd
Files under ignored directories still commit. The first review's probes give the same output.
Findings:

1. **A UTF-8 BOM breaks the first pattern:** adding lines one by one skips the BOM stripping the
   `ignore` crate's own `add` does, so `\xEF\xBB\xBFsecret.txt` lets `secret.txt` be committed.
   (Also true before this commit.)
2. A missing ignore file silently means no patterns (for the owner, noted above).
3. A non-UTF-8 ignore file refuses with a raw "stream did not contain valid UTF-8".

### Resolution of the re-review

Fixed in c1c1157:

- **The BOM:** a leading byte order mark is stripped before the patterns are parsed. Test:
  `a_byte_order_mark_doesnt_spoil_the_first_pattern`, which fails without the fix.
- **A non-UTF-8 ignore file:** refused with `.tidings/ignore:N: isn't UTF-8 text: save the ignore
  file as UTF-8`. Tested.
- **"entry":** `InvalidEntry` became `Unfit` (and its variables `unfit`). `entry` stays only where
  it names a std `DirEntry` in older directory loops.
- **The `covers*` duplication:** one `covers(&str)` each on `Selection` and `Target`.
- **`path` beside `local_name`:** one `subject: Option<Subject>` (`Subject::Path | Subject::Local`);
  the JSON is unchanged.
- The two nits are fixed.

Not acted on: `as_path` taking a path only to check UTF-8 (minor). No third review: each fix is
narrow and tested.

---

## Ticket 08: `status`

Reviewed: `git diff 7b95ac3...97ae483` (commit 97ae483).

### Standards

**(a) Documented-standard violations:** none. Every new item is documented, no line is over 100
columns, glossary terms are used correctly, and spec 0002 is followed: `status()` returns a
`StatusReport`, the command is thin, printing is in `output.rs`, `lock` is held for the whole run,
and tests go through the binary on fs and SQLite. `Scan::changes` now gives `commit` and `status`
one classification, and `EventParts::invalid` replaces an inline construction in `failure.rs`.

**(b) Judgement calls:**

1. **Possible Data Clump:** the Bases a scan was made with travel beside it everywhere
   (`scan(&folder, &bases)`, `scan.changes(&bases)`, `stage(&scan, &bases, …)`), and `stage`
   indexes `bases[path]` and `scan.files[path]` unchecked, assuming they agree.
2. **Possible Primitive Obsession / Duplicated Code:** a `LocalName` is compared with a `Path` as
   strings by linear search in two places, and `status` filters "not Diverged" two different ways
   on neighbouring lines.
3. **Possible illegal state:** `PathStatus::Diverged(SyncEvent)` can hold any `SyncEvent`, so its
   `name()` needs a fallback.
4. **Possible Mysterious Name:** `let diverged = diverged(record, selection);` shadows the function.
5. **Possible Mysterious Name:** `status --json` rows are keyed `event`, though a row is a state,
   not something that happened ("event" is also an _Avoid_ word under Change).
6. **Possible Duplicated Code in tests:** `status_in` copies `commit_in`.

### Spec

Probed on fs: `status` classifies each case exactly as `commit` acts on it: modified; `touch` or
a same-contents rewrite shows nothing; added; deleted; a new ignored file hidden; non-UTF-8
contents, a symlink or a bad name *invalid*; a tracked file matching a pattern still *modified*
(story 52); `Foo`→`foo` a delete and an add; a Diverged Path naming its `theirs` file, or "removed
in the Store". With the invalid files removed, `commit` committed exactly the 7 Paths `status`
listed. A bad ignore file, a missing Store and mismatched Store flags are refused as `commit`
refuses them; outside a Working copy the error is clear; `-C` and walking up work; with `sync`
running, `status` returned at once with "sync is running" and `sync` carried on.

**(a) Missing or partial:** none. **(b) Scope creep:** none.

**(c) Questionable:**

1. **For the spec's owner: Paths from a subdirectory.** In `sub/`, `status` prints
   `modified sub/d.txt`, while `commit d.txt` is how you'd commit it there (story 38). Area-relative
   Paths match `sync`'s output and `--json`, but can't be pasted back into `commit` from a
   subdirectory. (git prints paths relative to the current directory.)
2. `status` takes `sync.lock` for an instant to learn whether `sync` runs; a `sync` starting in that
   instant would be refused. Within the spec ("by trying it"), and not reproduced in 40 tries.

**The implementer's judgement calls, assessed:** rows keyed `event` match the spec's JSON style
for `sync` (see Standards 5 for the other view); sorted by name is fine; "nothing to commit" goes
to stdout here but stderr in `commit`, which is defensible (it is `status`'s report) but
inconsistent; a Diverged Path listed only as diverged matches `commit`, which refuses Diverged
first, though it hides that the local file was deleted or is invalid; a tracked Path turned into
a symlink is *invalid*, not *deleted*, correctly. `status` exits 0 even when it lists invalid or
Diverged Paths; the spec doesn't say otherwise.

### Summary

Standards: 0 hard violations, 6 judgement calls (worst: the scan's Bases travelling beside it,
with unchecked indexing in `stage`). Spec: nothing wrong; 1 question for the owner (Paths or
cwd-relative paths from a subdirectory).

### Resolution

Fixed in 17d5266 (no behaviour change; `--json` keys unchanged):

1. **The scan's Bases:** `Scan` borrows the Bases it was made with; `changes()` takes no arguments
   and yields a `Difference` (`Added`, `Modified`, `Deleted`) carrying the contents and Base, so
   `stage` no longer indexes unchecked.
2. **Name vs Path matching:** `LocalName::path()` gives the Path a name spells, so both checks are
   set lookups, and `status` checks "not Diverged" one way.
3. **`PathStatus::Diverged`:** holds a `DivergedPath { path, theirs_file, blocked }`, which
   `SyncEvent::Diverged` also wraps and `Failure::diverged` takes, so the fallback is gone.
4. **Shadowing:** the function is now `diverged_paths`.
5. **Tests:** one `run_in(subcommand, directory, args)` replaces `commit_in` and `status_in`.

Not acted on: rows keyed `event` (kept, matching the spec's JSON style for `sync`). The owner
accepted the open decisions listed across these reviews, including Area-relative Paths from a
subdirectory. No re-review: a refactor with no behaviour change, and the tests pass.

---

## Ticket 09: `discard` and `resolve`

Reviewed: `git diff bb505ea...f5628d2` (commit f5628d2). (The implementer stalled once and was
resumed. It reported library fs tests hanging for over 10 minutes; the orchestrator found no stray
processes, ran one of them alone in 8.6 s, and a full workspace run then passed all 505 tests:
outside load on the machine, not a regression.)

### Standards

**(a) Documented-standard violations:** none hard. Thin commands, reports through `output.rs`,
refusals through `failure.rs` (`Failure::blocked`), every item documented, tests through the
binary on fs and SQLite. Leaning soft:

- **The record format:** a `base` line may now lack its hash (`record.rs:181-189`) under the same
  `tidings working-copy 1`. An older binary won't misread it (its parser fails with "has a line it
  can't read"), but that is not the "unknown format or version is refused" story 78 asks for, and
  the spec's record section still says each entry holds a hash.

**(b) Judgement calls:**

1. **Possible Duplicated Code:** "set or drop the Base from the Store's File" is written three
   times (`settle.rs:666-673`, `working_copy.rs:751-756`, `:1004-1010`).
2. **Possible Duplicated Code:** the text and JSON arms for the discard and resolve reports repeat
   one "parts, then events" pipeline.
3. **Possible Primitive Obsession:** `Discarded { revision: Option<Revision>, removed: bool }`
   encodes three outcomes, decoded by a tuple match.
4. **Possible Primitive Obsession:** `check_each_names_something(…, command: &str)` passes the
   command's name only to build a message.
5. **Possible Feature Envy:** `settle.rs` uses a dozen of its parent's helpers: a file split rather
   than a module hiding anything. Acceptable as a child module.

### Spec

Probed on fs and SQLite: `discard` of modified, deleted, symlink (not followed; the file outside
untouched) and non-UTF-8 files; a Diverged Path removed in the Store has its local file and emptied
directories removed; a bare `discard` leaves added files alone and a named one is removed, also
from a subdirectory; `resolve` then `commit` goes through, and `resolve`, a Store change, then
`commit` is still a Conflict (story 64); `resolve` of a removed-in-the-Store Divergence leaves no
Base; `discard` waits on a held lock; all work while `sync` runs; `store shell`'s `commit` and
`discard` keep their Staging meaning (story 73). Nothing outside the folder was ever touched.

**(a) Missing or partial:**

1. **No test runs `discard` or `resolve` alongside a live `sync`**, which the ticket asks for
   (behaviour is fine by probe).
2. **A `theirs` that couldn't be removed stays for good** (story 66): after `resolve` reports the
   error, a later `sync` never removes it, since the retry lives only in `sync`'s memory.

**(b) Scope creep:** none of substance (`{"failure":"blocked"}`; refusing a named ignored file,
as `commit` does).

**(c) Looks wrong or questionable:**

1. **The record change** (as Standards (a)): sound in design, since a resumed `sync` marks such a
   merge Diverged rather than overwriting it, but should be a new format version.
2. **A side effect of the unknown hash:** with the folder holding exactly the `theirs` contents
   after a `resolve`, `status` still lists it as *modified* (story 57 "compare contents").
3. **A named file whose name can't be a Path** gives "nothing to discard", exit 0, with no hint
   why.
4. **`discard <directory>` removes every added and invalid no-Base file under it**, e.g.
   `discard .`: consistent with "as for `commit`", but it stretches story 61, "remove an added
   file only when I name it". For the owner.
5. Judged fine: `resolve` of anything not Diverged (a directory, a typo, a mix) exits 1 and
   resolves nothing; a blocked write refuses the whole `discard` up front, stricter than `sync`'s
   Divergence but safe.

### Summary

Standards: 0 hard violations, 1 near-violation (the record format) and 5 judgement calls. Spec: 2
partial (no tests with `sync` running; a stale `theirs` never cleaned up) and 4 questionable (worst:
`discard <directory>` deleting added files the person didn't name).

### Resolution

Fixed in 8a1672e:

1. **Spec (a)1, tests with `sync` running:** added
   `discard_and_resolve_while_sync_runs_leave_it_nothing_to_report` (fs and SQLite).
2. **Spec (a)2, a stale `theirs`:** a sweep, `remove_stale_theirs`, removes any file under
   `.tidings/theirs/` whose Path isn't Diverged (never following symlinks), after `discard` and
   `resolve` and whenever `sync` reconciles everything. Tests:
   `a_theirs_that_couldnt_be_removed_is_removed_once_sync_restarts` and
   `discard_resolve_and_a_resync_remove_every_stale_theirs_without_following_symlinks`.
3. **Standards (a) and Spec (c)1, the record format:** the record is now written as
   `tidings working-copy 2`; version 1 is still read; any other version is refused as a format
   this version doesn't know. Test:
   `a_record_in_version_1_is_read_and_one_in_an_unknown_version_refused`.
4. **Spec (c)2, `status` after `resolve`:** the Base's hash is taken from the Store's contents if
   it still holds the Revision, else from the `theirs` file, else left unknown. (See the
   re-review: the `theirs` fallback was wrong.)
5. **Spec (c)3:** `discard` refuses a named file whose name can't be a Path, as `commit` and
   `resolve` do.
6. **Spec (c)4, decided:** `discard` removes a file with no Base only when that file itself is
   named; a named directory, or `.`, leaves such files alone (story 61). **For the spec's owner:**
   the spec doesn't say this about directories yet.
7. **Standards (b):** `Record::set_base` and `Base::of_file` replace the three copies;
   `settled_lines`/`settled_json` share the report pipeline; a `DiscardOutcome` enum and a
   `Naming` enum replace the bool pair and the command string.

### Re-review of 8a1672e

**Standards.** Hard: `DiscardOutcome::TookStoresVersion` brings back "version" for a Revision,
which ticket 06 had renamed away; `Naming`'s variants lack doc comments. Soft: **the spec's record
section (lines 264-271) still says `tidings working-copy 1` and that every entry holds a hash; the
owner should update it.** Judgement calls: two matches on `DiscardOutcome`; a shadowed `committed`;
`everything` for "all Paths"; `Naming` names a command; `read_theirs` repeats checks; the
version-1 test writes the record format directly (stretching the one exception, reasonably).

**Spec.** Probed on fs and SQLite, with the old binary built from f5628d2 for the format checks:
the sweep keeps Diverged Paths' `theirs`, removes stale files, directories and dotfiles, removes
symlinks inside `theirs/` as links without touching their targets, leaves non-Path names alone,
and touches nothing outside when `.tidings/theirs` is itself a symlink; a version-1 record is read
and rewritten as version 2, and the old binary refuses version 2 as an unknown format;
`discard .` leaves added files while naming them removes them; everything from the first review
still holds. Findings:

1. **A person's merge can be silently overwritten (data loss).** If the person merges inside
   `.tidings/theirs/a.txt` and copies it into the folder while the Store has moved on, `resolve`
   takes the merge's hash as the Base's (from the `theirs` file), `status` says nothing to commit,
   and the next `sync` sees the file as unchanged and overwrites it with the Store's newer File;
   the sweep has already removed `theirs`, so the merge is gone. Stories 63, 64.
2. The sweep empties a directory a person put at a Diverged Path's `theirs` location, in the same
   reconcile that reports it can't write there. It stays inside `.tidings/theirs/` and heals, but
   the report no longer matches the disk.

### Resolution of the re-review

Fixed in 1a6b441:

1. **The merge overwrite (data loss):** fixed. The record's Divergence now stores the hash of the
   Store's File, computed from the File itself when the Divergence is recorded (so it is there
   even if writing `theirs` fails). `resolve` uses it, else the Store's contents if the Store still
   holds that Revision, else leaves the hash unknown; it never reads `theirs`. Version-2
   `diverged` lines carry the hash; version-1 records still load and gain it on the next
   reconcile. Test: `a_merge_made_in_theirs_after_the_store_changed_again_isnt_overwritten`.
2. **The sweep and a Diverged Path's `theirs` location:** it now skips that location without
   looking inside. Test: `the_sweep_leaves_a_directory_where_a_diverged_paths_theirs_goes`.
3. **Standards:** `TookStoresFile` again; `NamingCommand` with documented variants;
   `DiscardOutcome::revision()`; `revision` and `all_paths` renames; the test message.

Both new tests fail on 8a1672e.

### Third review, of 1a6b441

**Standards.** No hard violations; every claimed fix is clean. Judgement calls: `Divergence.theirs`
is typed `Option<Base>` though it isn't a Base until `resolve` takes it (its doc explains);
two small duplications in `record.rs` and the sweep; `was` for a previous value; a closure named
like the method it calls; a hard-to-read sentence in `base_of`'s doc. The spec's record section
drifts a little further: it doesn't mention the Divergence's hash either.

**Spec.** Probed on fs and SQLite; both new tests fail on 8a1672e. The original repro now shows
*modified* after `resolve`, `sync` Diverges instead of overwriting, and the merge survives; a commit
without `sync` is a real Conflict (story 64). Variants hold: editing only `theirs`; the Store
changing after `resolve`; `theirs` refreshed by a running `sync` (the recorded hash follows the
refresh); a failed `theirs` write; version-1 records made by the f5628d2 binary, with and without
the Store having moved on, before and after a reconcile. The sweep keeps Diverged `theirs` files
(nested too), removes stale ones and symlinks as links, and replaces a symlink at a Diverged
location with a file without following it. Findings (minor, not acted on):

1. A version-1 record whose Store has moved on, with the folder exactly equal to the old
   contents: `resolve` leaves the hash unknown, so `status` shows *modified* and `sync` makes a
   needless Divergence rather than applying the newer File. Nothing is lost; it is the accepted
   cost of never trusting `theirs`, and affects only records written before version 2.
2. A directory the person put at `theirs/a.txt` is kept only while the Path is Diverged; once it
   is resolved, the sweep removes it, as story 66 asks.

**For the spec's owner:** update the record section (docs/specs/0002-working-copies.md:264-271):
format `tidings working-copy 2`; a Base's hash may be unknown; a Diverged entry also records the
hash of the Store's File. And `discard` of a named directory leaves files with no Base alone.

---

## Ticket 10: Surviving crashes

Reviewed: `git diff 9dab660...b0086e1` (commit b0086e1).

The implementer audited every multi-step operation (`sync` apply, `commit`, `discard`, `resolve`,
create, `theirs` writes, the sweep) and changed three things: a `commit` whose earlier record save
was lost no longer fails with a Conflict (if every conflicting Path already holds the local
contents in the Store, those take their Base silently and the rest is committed, retrying once);
every full reconcile clears leftovers in `.tidings/tmp/` and temporary files from writing the
record or the ignore file; folder writes and removals force their directory to disk before the
record is saved.

### Standards

**(a) Documented-standard violations:** none. Every item documented, no line over 100 columns,
tests through the binary (reaching crash states by SIGKILL or by restoring an earlier record's
bytes, never parsing the format) on fs and SQLite. Judgement call: a test plants a
`.working-copy.x1Y2z3` file, copying `atomic-write-file`'s undocumented naming.

**(b) Judgement calls:**

1. **Possible Data Clump:** `commit`'s retry loop packs `Attempt { changes, files, outcome }` and
   then breaks with a new 4-tuple of the same fields.
2. **Possible Mysterious Name:** `all_same` (every conflicting Path took the Store's File) and
   `healed` (set before any healing; means "already retried").
3. Two `if all_paths` branches in `reconcile`; a repeated `sync_directory(…).map_err(…)`.
4. **Tests:** the sync-until-caught-up steps, "commit and expect nothing to commit", and the
   `store shell` script writing numbered files each repeat; `heals(…, next: &str)` branches on its
   label strings; `label, location, folder` travel together.

### Spec

Probed on fs: SIGKILL with 952 of 1,500 Files created, and with 19 of 1,500 updated: each resume
reported only the rest, nothing Diverged, the folder matching the Store. A lost record after
`commit` (an earlier record restored) heals: `commit` says nothing to commit, then `status` shows
nothing; a new file added first is the only thing committed; a mixed case Diverges the edited
Path, reports the matching one as `same`, exits 3 and commits nothing. A lost deletion or
`discard` heals the same way. The retry rescans against the updated Bases, so its preconditions
are "unchanged since" the new Base, and a second Conflict is reported: **no way was found to
commit over an unseen change.** Leftovers are removed, other files in `.tidings/` kept, and a
symlink in `tmp/` removed without touching its target. A record in an unknown format is refused
by all five commands and left byte for byte.

**(a) Missing or partial:** after a lost commit record, `status` lists the Paths as *modified*
until a `commit` or `sync` runs; the spec's `status` doesn't read the Store, so this is expected,
but the ticket's "healed by the next command" overstates it.

**(b) Scope creep:** none.

**(c) Looks wrong:**

1. **`sync` writes outside the folder when `.tidings/tmp` is a symlink** (confirmed: with `tmp`
   linked to a read-only outside directory, `sync` failed creating its temporary file there).
   `write` makes and uses `tmp` without checking it. Known to the implementer but unrecorded.
2. **When another process committed the same contents, `commit` exits 0, not 3.** The spec's
   Conflict bullet says "if the contents turn out equal, takes the new Base. The command exits 3,
   naming them." Code can't tell this from a lost record save; the behaviour is safe.
3. A crash between removing a file and removing the directories it emptied leaves an empty
   directory, never cleaned up.

Outside this ticket: `resolve` removes `theirs` before saving the record, so after a lost save
`status` and `commit` point to a `theirs` file that isn't there until `sync` rewrites it.

### Summary

Standards: 0 hard violations, 4 groups of judgement calls (worst: the tuple packing in `commit`).
Spec: 1 partial and 3 wrong (worst: writing outside the folder through a symlinked `tmp`).

### Resolution

Fixed in a69a291:

1. **Spec (c)1, a symlinked `tmp`:** fixed. Before writing, `make_tmp_directory` replaces a
   symlink at `.tidings/tmp` with a real directory, removing the link and never its target;
   anything else that isn't a directory there fails with a clear error (tidings never puts one
   there). Test: `sync_never_writes_through_a_symlinked_tmp_directory` (unix, fs and SQLite), which
   fails on b0086e1 with the probe's Permission denied.
2. **`theirs` removed before the record was saved:** fixed in both `resolve` and `discard`: the
   record is saved first and stale `theirs` removed after, so a crash leaves only a stale file for
   the sweep. No binary test: the order can only be seen with a crash in between.
3. **Standards:** `commit`'s loop breaks with its `Attempt`; `healed` became `retried` and
   `all_same` became `all_took_stores_file`; one `sync_parent` helper; tests share `synced()` (now
   returning its events), `commits_nothing()`, `numbered_files`/`commit_all`, and a
   `Next { Commit, Sync }` enum.

Accepted, not changed, **for the spec's owner:**

- `commit` exits 0 and takes the Store's File as the Base when another process committed the same
  contents (it can't be told from a lost record save, and it is safe); the spec's Conflict bullet
  says it exits 3 naming them.
- After a lost commit record, `status` shows *modified* until the next `commit` or `sync`.
- A crash between removing a file and removing the directories it emptied leaves an empty
  directory; removing empty directories on sight would break story 16.

No re-review: the fixes are narrow and the safety fix is tested. (One library fs test,
`an_area_directory_removed_while_running_is_made_again_with_a_resync`, flaked once again.)

---

## Ticket 11: The README for everyday use

Reviewed: `git diff b573696...ce8c331` (commit ce8c331).

### Standards

**(a) Documented-standard violations:** none. Prose is within 100 columns (only table and code
rows exceed); the spec's order is followed (walkthrough and Working copies, then `tidings store`,
then limits); limits use the *Why:* style; no _Avoid_ word names a domain concept.

**(b) Judgement calls:**

1. "version" standing for a Revision where `resolve` makes "the Store's version … the Base".
2. Undefined terms: "Divergence", "the Working copy's record", `blocked`, and the `4913` default.
3. Inconsistent terms within the new text ("Store Change" and "Store change"; "Paths you give",
   "the paths given", "other paths"); the older sections still lowercase store, area, commit.
4. Said more than once: `status` not reaching the Store; a Divergence clearing by itself; an
   ignored file with no Base; refusing to commit a Diverged Path (four times).
5. "Working copies" does many jobs, and its Output and Exit codes bullets also cover
   `tidings store`.
6. Two limits describe crash recovery rather than limits.
7. Four overlong sentences; a forward reference to the Store-selection flags; the Status line at
   the top predates Working copies.

### Spec

The reviewer ran the whole walkthrough on SQLite and its output matched README.md line for line,
Revisions included; the invalid-files example and its `--json` failure matched too. Probed claims
all held: `--create` refused on an existing Working copy; a stale `TIDINGS_ROOT` refused; `sync` of
another Area refused; the ignore defaults; Area-relative Paths from a subdirectory; bare `discard`
and `discard .` leaving a file with no Base; `--quiet`; a Divergence clearing by itself;
`store read` of a missing File (exit 2, `{"failure":"missing"}`); memory Stores refused for a
Working copy while the memory shell works.

**(a) Missing or partial:** none; every checkbox is met. **(b) Scope creep:** three extra limits,
all restating owner-accepted decisions.

**(c) Likely to trip people up:**

1. **Where `-C` goes is never shown.** It works only after the subcommand: `tidings -C cfg status`
   fails with clap's "unexpected argument", while `tidings status -C cfg` works. Git users will try
   the first.
2. `commit` of a path that neither exists nor is recorded exits 1, but the README says only that
   nothing to commit succeeds.

### Summary

Standards: 0 hard violations, 7 groups of judgement calls (worst: explanations repeated across
sections). Spec: every example sampled runs as shown; 2 omissions (worst: where `-C` goes).

### Resolution

Fixed in 8897440 (README only):

1. **`-C`:** shown going after the command (`tidings status -C cfg`, not `tidings -C cfg status`).
2. **A mistyped path:** `commit` and `discard` of a path neither in the folder nor recorded are
   said to exit 1, "no such file in the Working copy".
3. **"version" for a Revision:** the `resolve` reference now says "the Revision in `theirs`, the
   Store's File you merged against". The walkthrough keeps the program's own output line ("…the
   Store's version it was merged with"), where "version" means contents, as earlier reviews allow.
4. **Undefined terms:** "no longer Diverged" and "the Bases the Working copy recorded" replace
   them; `blocked`, `invalid` and `4913` (vim's write-test file) are explained.
5. **Consistent terms:** "a Change in the Store" and "Paths you name" throughout the new sections.
6. **Said once:** each repeated point is explained in one place, with a pointer at most.
7. **Recovery notes:** moved out of the limits into an "After a crash" note.
8. **Long sentences:** split.
9–10. **A shared section**, "Choosing a Store, output and exit codes", which the Working copies
   and `tidings store` sections both point to, replacing the forward reference.
11. **The Status line:** updated.

The changed examples were run against the binary. No re-review: prose edits only.

## Dropping the old record format

The owner asked for all reading of the record's version 1 to go, since no Working copy exists
outside development, and for the layout then written as version 2 to become version 1. Reviewed
before committing, as 30c87d9.

### Standards

No hard violations. Stale wording: a comment in `WorkingCopy::reconcile` on "only a hash was
recorded", a case that can no longer happen; and the `resolve` command's doc and spec bullet not
saying it opens the Store only to check it, as `status`'s do. Judgement calls: the Revision and
hash pair is now written out twice in `Record::to_text`, while `parse_base` reads it once; the
unknown-format test picks its expected message by the version's suffix, a pattern older than this
change.

### Spec

Probed on a scratch Store: a new record is written as `tidings working-copy 1` with a hash on every
`base` line and on each `diverged` line that has a Revision; versions `2`, `3`, `0`, `01`, `1 ` and
`1.0` are refused by all five commands with the record, folder and `theirs` untouched; a line in the
old layout (no hash) is refused as "a line it can't read". `resolve` still refuses mismatched Store
flags and a Store that has gone, and a commit after the Store changed again is a Conflict. Every
place a Base or Divergence is made computes its hash, so making it mandatory is safe. Findings: the
same stale comment; no test pins the refusal of an old-layout line (checked by hand only).

### Summary

Standards: 0 hard violations, 3 stale wordings, 2 judgement calls. Spec: nothing missing or wrong
beyond the stale comment.

### Resolution

In 30c87d9: the stale comment removed, and the `resolve` doc and spec bullet now say it opens the
Store only to check it. Not acted on: the repeated pair (simpler than the closure it replaced), the
test's suffix check (older than this change), and a test of an old-layout line (the spec keeps tests
from reading the record's format beyond the unknown-format one).
