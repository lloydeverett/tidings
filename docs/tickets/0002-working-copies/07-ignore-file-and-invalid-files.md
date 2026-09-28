# 07: The ignore file and invalid local files

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** `commit` leaves out editor and OS leftovers, and refuses anything that can't
become a File instead of silently skipping it. `.tidings/ignore`, in `.gitignore` syntax, is
created with sensible defaults and fully editable. It only ever applies to local files with no
Base: anything the Store holds is always synced and tracked.

**Blocked by:** 02

**Status:** ready-for-agent

- [ ] A new Working copy gets `.tidings/ignore` holding `.*.sw?`, `*~`, `4913`, `.DS_Store`,
      `Thumbs.db` and `.#*`.
- [ ] A file with no Base matching the ignore file is left out of `commit`.
- [ ] Removing a pattern from the ignore file lets a matching file be committed (such as a File
      named `.DS_Store`). The file is read afresh by each command.
- [ ] A Store File matching a pattern is still written into the folder by `sync`, and local edits
      to it are committed.
- [ ] A file that would be committed and whose name isn't a valid Path, whose contents aren't
      UTF-8, which is a symlink, or which isn't a regular file refuses the whole commit (exit 1),
      naming every such file. Ignoring it instead lets the commit go ahead.
- [ ] Empty directories are never committed or reported.
- [ ] A tracked Path replaced locally by something invalid (such as a symlink) is *invalid*, not
      *deleted*, and refuses a commit that includes it.
- [ ] Tests, through the binary, including a vim-style save leaving `.a.txt.swp`, `a.txt~` and
      `4913` behind.
