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
