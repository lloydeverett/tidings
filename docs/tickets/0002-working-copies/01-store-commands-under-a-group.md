# 01: Move the Store commands under `tidings store`

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** A prefactor that frees the top level for Working copies. Every command that acts
on a Store directly moves, unchanged, under `tidings store`: `read`, `stat`, `list`,
`stat-prefix`, `write`, `delete`, `delete-prefix`, `edit`, `watch` and `shell`. Inside
`tidings store shell`, `commit` and `discard` keep their Staging meaning. The Store-selection flags
and `--json` stay global, and the exit codes stay as they are.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `tidings store <command>` behaves exactly as `tidings <command>` did, for every command
      listed above, including stdin handling, `--json` output and exit codes.
- [x] The old top-level forms are gone, not kept as aliases. Nobody uses the CLI yet.
- [x] `tidings --help` and `tidings store --help` list the commands where they now live.
- [x] The existing CLI tests are updated to the new form and pass. The shared test harness's
      helpers (`with_store`, `write`, `read`) use `tidings store`.
- [x] The README's command examples and the shell section use `tidings store`. The full README
      rewrite is ticket 11.
