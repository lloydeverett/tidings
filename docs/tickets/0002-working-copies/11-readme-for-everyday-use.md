# 11: Rewrite the README for everyday use

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** The README presents the `tidings` command as an everyday tool rather than one
for trying out and debugging Stores. Working copies come first, then the `tidings store`
commands, then the Working copy's own limits, in the same "what, and why" style as the
Consistency section.

**Blocked by:** 01, 02, 03, 04, 05, 06, 07, 08, 09, 10

**Status:** ready-for-agent

- [ ] A short walkthrough: `sync` a Working copy, edit in vim, `status`, `commit`, and dealing with
      a Diverged Path through `theirs`, `resolve` and `discard`.
- [ ] The Working copy commands and their flags (`-C`, `--quiet`, `--json`), the ignore file and its
      defaults, and which files count as invalid.
- [ ] `tidings store` and the shell, as before.
- [ ] Limits, each with why: the Store's version in `theirs` is only as fresh as the last `sync`
      or `commit`; `status` doesn't reach the Store; one Area per Working copy; no memory Stores;
      `sync` runs in the foreground; symlinks can't be committed.
- [ ] Every example in it has been run.
