# 08: `tidings status`

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** `tidings status` shows what `commit` would do. It lists each Path that isn't
unchanged as *modified*, *added*, *deleted*, *diverged* or *invalid*, by comparing contents
against the record, and says whether a `sync` is running on the Working copy.

**Blocked by:** 05, 07

**Status:** done

- [x] Each classification appears for the Paths it applies to, and unchanged and ignored files
      don't appear.
- [x] Contents are compared, not modified times: `touch`ing a file, or rewriting the same text,
      shows nothing.
- [x] A Diverged Path names its `theirs` file, or says the Store removed it.
- [x] It says whether a `sync` is running, found by whether `.tidings/sync.lock` is held.
- [x] It holds `.tidings/lock`, so it never reports a half-applied reconcile.
- [x] `--json` gives the same as one JSON object.
- [x] It finds the Working copy by walking up, or takes `-C`.
- [x] Tests, through the binary.
