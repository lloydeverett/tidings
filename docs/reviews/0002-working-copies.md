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
