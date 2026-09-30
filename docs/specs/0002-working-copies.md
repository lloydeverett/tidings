# Spec 0002: Working copies in the `tidings` command

Status: ready to build. Terms are used as defined in [CONTEXT.md](../../CONTEXT.md), and the
decisions in [docs/adr](../adr) apply throughout, especially
[ADR 0008](../adr/0008-a-working-copy-is-a-copy-not-a-mount.md).

## Problem Statement

A person using an app built on tidings wants to read and edit its Files with their own tools: vim,
grep, a diff viewer, a shell loop. On the filesystem Backend they can, with the risks the README
describes. But on SQLite every File is inside a database. The `tidings` command can read, write
and `edit` Files one at a time, and that's fine for a quick look, but not for spending a day
editing config:

- There is no folder to `cd` into, grep through or open in an editor as a project.
- Editing several Files that belong together means building a Staging by hand in the shell.
- If the app, or another process, changes a File, nothing in front of the person shows it.
- Nothing warns them when their edit is based on a version that has since changed, unless they
  thread Revisions through by hand.

The CLI also presents itself as a debugging tool, with the Store commands at the top level. If
people live in it, the everyday operations should come first.

## Solution

A **Working copy**: a folder holding one Area's Files as ordinary files.

- `tidings sync config ~/cfg` makes `~/cfg` a Working copy of the config Area (or resumes one) and
  keeps it in step with the Store until Ctrl-C, printing what it does.
- The person edits files in `~/cfg` however they like. Nothing reaches the Store until they run
  `tidings commit`, which commits every local change (or only the paths named) as a single,
  all-or-nothing Commit. Each change requires its File to be unchanged since its **Base**, the
  Revision the Working copy last took from, or committed to, the Store.
- Syncing never overwrites a local edit. If the Store and the folder both changed a Path, it is
  **Diverged**: the person's file is left alone, the Store's version is put in
  `.tidings/theirs/` to merge against, and `tidings resolve` or `tidings discard` settles it.
- `tidings status` shows what a commit would do.
- A Working copy remembers its Store, so every command after the first finds everything from the
  folder, including from a subdirectory, as git does.
- The commands that work on the Store directly move under `tidings store`.

## User Stories

### Starting and resuming

1. As a person using a tidings app, I want `tidings sync <area> <folder>` to make an empty or
   missing folder a Working copy of that Area, so that I can see every File as a plain file.
2. As a person, I want `sync` to default the folder to the current directory, so that
   `mkdir cfg && cd cfg && tidings sync config --identity …` just works.
3. As a person, I want to choose the Store for the first `sync` with the same flags as every other
   command (`--root` or `--identity`, `--backend`, `--create`, and their environment variables),
   so that there's nothing new to learn.
4. As a person, I want the Working copy to remember which Store and Area it belongs to, so that I
   never have to repeat those flags and can't commit into the wrong Store by mistake.
5. As a person, I want Store flags passed to a later command to be refused if they don't match
   what the Working copy remembers, so that a stale `TIDINGS_ROOT` in my shell can't mix things up.
6. As a person, I want `sync` on an existing Working copy to resume it, so that restarting my
   terminal or laptop loses nothing.
7. As a person, I want `sync` to refuse a folder that holds files but isn't a Working copy, so that
   a typo can't pour an unrelated folder into my Store.
8. As a person, I want `sync` to refuse a Working copy of a different Store or Area than the one I
   asked for, so that I notice my mistake.
9. As a person, I want a second `sync` of a folder that is already being synced to be refused,
   saying so, so that two processes never fight over my files.
10. As a person, I want `sync` to work on filesystem and SQLite Stores alike, so that I don't need
    to care which Backend the app picked.
11. As a person, I want `sync` to refuse a memory Store with a clear reason, so that I understand
    why it can't work.
12. As a person, I want the Working copy to keep working after I move or rename its folder, so that
    I can organise my folders freely.

### Following the Store

13. As a person, I want Files the Store adds or changes to appear in my folder while `sync` runs,
    so that I always see the current state.
14. As a person, I want Files the Store removes to disappear from my folder, unless I have changed
    them, so that the folder doesn't collect stale files.
15. As a person, I want a directory that `sync` emptied by its own deletes to be removed, so that
    the folder shows only the Prefixes that exist.
16. As a person, I want a directory still holding my own files to be left alone, so that `sync`
    never deletes anything it didn't put there.
17. As a person, I want files `sync` writes to appear whole, never half-written, so that my editor
    or tools never read a torn file.
18. As a person, I want `sync` to catch up on everything that changed while it wasn't running, so
    that resuming is the same as never having stopped.
19. As a person, I want `sync` to re-read the whole Area after a Resync, and say so, so that
    nothing missed stays missed.
20. As a person with a Working copy of the Cache Area, I want Files the OS cleared to disappear from
    my folder like any other removal, so that the cache behaves as it does for the app.
21. As a person, I want one line per thing `sync` does (*created*, *updated*, *removed*,
    *diverged*, *resync*), so that I can keep it in a tmux pane and see what's happening.
22. As a person, I want `sync --quiet` to print only divergences, resyncs and errors, so that it
    stays out of my way.
23. As a script author, I want `sync --json` to print one JSON object per line, so that I can
    follow a Working copy from another program, as I can with `store watch`.
24. As a script author, I want `sync` to report when it is **caught up** with everything it knows
    of, so that I can wait for it rather than guessing with a sleep.
25. As a person, I want `sync` to stop cleanly on Ctrl-C, even halfway through applying a Change, so
    that the folder and its record never disagree.

### Local edits and Divergence

26. As a person, I want my local edits never to be overwritten by `sync`, so that I can't lose work
    to another process's Commit.
27. As a person, I want a Path that changed both locally and in the Store to be marked Diverged and
    reported, so that I know I'm editing against an old version.
28. As a person, I want the Store's version of a Diverged Path written to `.tidings/theirs/<path>`,
    so that I can `vimdiff` it against mine and merge.
29. As a person, I want `theirs` kept up to date if the Store changes the Path again, so that I
    merge against the latest version.
30. As a person, I want a Path I deleted locally that the Store then changed to be Diverged too,
    so that my delete doesn't silently throw away someone's change.
31. As a person, I want a Path I created locally that the Store also created to be Diverged, so that
    neither version is lost.
32. As a person, I want a Path whose local and Store contents came out the same not to be treated
    as Diverged, so that making the same fix on both sides isn't a problem.
33. As a person, I want a Path whose Store version was removed while I had it edited to be Diverged
    as "removed in the Store", so that I can choose to keep my edit or let it go.
34. As a person, I want a local change that blocks `sync` from applying a Store change (a
    directory where the Store now has a File, say) to make the Path Diverged instead of failing, so
    that `sync` keeps running and tells me what to fix.

### Committing

35. As a person, I want `tidings commit` to commit every local change in the Working copy as one
    Commit, so that related edits land together or not at all.
36. As a person, I want `tidings commit <paths…>` to commit only those, so that I can land one fix
    while leaving an experiment uncommitted.
37. As a person, I want a directory given to `commit` to mean everything under it, so that
    `tidings commit themes/` does what I expect.
38. As a person, I want paths I give to be relative to my current directory, so that tab completion
    and vim's `%` work.
39. As a person, I want a path outside the Working copy, or inside its `.tidings/`, to be refused,
    so that I can't commit something I didn't mean to.
40. As a person, I want new files committed only if the Path is still absent in the Store, changed
    files only if unchanged since their Base, and deleted files deleted only if unchanged since
    their Base, so that I never overwrite a change I haven't seen.
41. As a person, I want `commit` to work whether or not `sync` is running, so that my edits are
    never stuck waiting for a process.
42. As a person, I want `commit` and a running `sync` never to act on the folder at the same time,
    so that `sync` can't write a file in the middle of my commit.
43. As a person, I want a Conflict at commit time to commit nothing, exit with 3, name the Paths,
    and mark them Diverged with `theirs` written, so that I can go straight to merging, even without
    `sync` running.
44. As a person, I want a full `commit` to be refused up front while any Path is Diverged, listing
    them and telling me to `resolve` or `discard` them or name other paths, so that I'm not
    surprised by a Conflict I could have seen coming.
45. As a person, I want a `commit` naming only Paths that aren't Diverged to go ahead even while
    others are, so that one Divergence doesn't block unrelated work.
46. As a person, I want `commit` to tell me what it committed, with each Path's new Revision in
    `--json`, so that I can see what happened.
47. As a person, I want `commit` with nothing to commit to say so and succeed, so that running it
    twice is harmless.
48. As a person, I want my committed edit to become the new Base, and `sync` not to report my own
    Commit back to me as a change, so that the folder stays quiet after I commit.

### What can and can't become a File

49. As a person, I want `.tidings/ignore`, in `.gitignore` syntax, to decide which local files
    `commit` leaves out, so that editor and OS leftovers don't land in my Store.
50. As a person, I want the ignore file created with sensible defaults (`.*.sw?`, `*~`, `4913`,
    `.DS_Store`, `Thumbs.db`, `.#*`), so that it works out of the box.
51. As a person, I want to be able to remove a default from the ignore file, so that if I really do
    want a File named `.DS_Store` I can commit one.
52. As a person, I want the ignore file to apply only to files with no Base, so that a File the
    Store already holds is always synced and tracked, even if a pattern matches it.
53. As a person, I want `commit` to refuse, naming every offending file, when a file I'd commit has
    a name that isn't a valid Path, isn't UTF-8, is a symlink, or isn't a regular file, so that
    nothing is silently left out of an all-or-nothing commit.
54. As a person, I want to be able to ignore such a file instead of renaming it, so that tools that
    drop files into my folder don't block me.
55. As a person, I want empty directories to be ignored by `commit` and `status`, since a Store has
    no empty directories, so that they aren't reported as changes.

### Status, discard and resolve

56. As a person, I want `tidings status` to list Paths as *modified*, *added*, *deleted*,
    *diverged* or *invalid*, so that I know what `commit` would do.
57. As a person, I want `status` to compare contents, not modified times, so that `touch` or an
    editor that rewrites the same text doesn't show a change.
58. As a person, I want `status` to say whether a `sync` is running on this Working copy, so that I
    know whether the folder is keeping up.
59. As a script author, I want `status --json`, so that I can build prompts and editor integrations.
60. As a person, I want `tidings discard [paths…]` to throw away my local changes and take the
    Store's version, which becomes the new Base, so that I can start over on a File.
61. As a person, I want `discard` with no paths to leave files I added (with no Base) alone, and
    remove an added file only when I name it, so that a bare `discard` can't delete work the Store
    never had.
62. As a person, I want `discard` on a Diverged Path to settle it by taking the Store's version, so
    that giving up my side is one command.
63. As a person, I want `tidings resolve <path>` to say "what's in my folder is the merge", taking
    the Store's version I merged against as the Base, so that my next commit goes through.
64. As a person, I want a commit after `resolve` still to be a Conflict if the Store changed again
    since the version I merged against, so that resolving never hides a change I haven't seen.
65. As a person, I want `resolve` to refuse a Path that isn't Diverged, so that I don't mark
    something resolved by mistake.
66. As a person, I want the `theirs` copy removed once a Path is no longer Diverged, so that
    `.tidings/` doesn't fill with stale files.

### Finding the Working copy

67. As a person, I want `commit`, `status`, `discard` and `resolve` to find the Working copy by
    walking up from my current directory, so that I can run them from anywhere inside it.
68. As a person, I want `-C <folder>` to name a Working copy explicitly, so that scripts and editor
    integrations don't depend on the current directory.
69. As a person, I want the walk to recognise a Working copy by its record, not just by a
    `.tidings/` directory, so that a filesystem Area's `.tidings/` is never mistaken for one.
70. As a person, I want a clear error when I'm not inside a Working copy, so that I know to
    `sync` one first or use `-C`.

### The command layout

71. As a person, I want the Working copy commands at the top level (`sync`, `commit`, `status`,
    `discard`, `resolve`), since they're what I use every day, so that they're quick to type.
72. As a person, I want the commands that act on a Store directly under `tidings store` (`read`,
    `stat`, `list`, `stat-prefix`, `write`, `delete`, `delete-prefix`, `edit`, `watch`, `shell`),
    so that it's clear they bypass any Working copy.
73. As a person, I want `commit` and `discard` inside `tidings store shell` to keep their Staging
    meaning, so that the shell works as it does now.
74. As a person, I want the exit codes to stay as they are (0 success, 1 error, 2 no File, 3
    Conflict) and to apply to Working copy commands too, with a refused Divergence counted as a
    Conflict, so that scripts can branch on them.
75. As a person, I want the README to present the CLI as a tool for everyday use, with Working
    copies first, so that I find the right commands.

### Safety

76. As a person, I want the Working copy's record always written whole, and the folder and record
    to agree again on the next command after a crash or power cut, so that an interrupted `sync` or
    `commit` never corrupts my Working copy.
77. As a person, I want two Working copies of the same Area to be allowed, each independent, with
    their commits conflicting the usual way, so that I can have one on each machine or project.
78. As a person, I want a Working copy's record from a newer, unknown version to be refused rather
    than guessed at, so that an old binary never damages it.

## Implementation Decisions

### Where it lives

- The Working copy is part of the `cli` crate only, built on the library's public API (ADR 0008).
  The library doesn't change.
- A deep module, the **Working copy**, owns everything about the folder and its record: opening
  (by explicit folder or by walking up), creating, locking, scanning the folder into local states,
  reconciling Paths against the Store, and saving the record. Its interface is small and
  command-shaped: `create`/`open`/`find`, `reconcile(paths | all)` (used by `sync` on start, per
  batch of Changes, and on Resync), `status()`, `commit(paths)`, `discard(paths)` and
  `resolve(path)`, each returning a report that the output module prints as text or JSON. The
  commands themselves are thin: they parse arguments, open the Store, and call it.
- The existing output and failure modules print the results and choose the exit code; Working copy
  reports and failures are added to them rather than printed separately.

### The command layout

- The top-level subcommands become `sync`, `commit`, `status`, `discard`, `resolve` and `store`.
  `store` holds the existing commands unchanged, plus `watch` and `shell`.
- The Store-selection flags stay global. For `sync` on a new Working copy they choose the Store as
  they do now, `--create` included. For every other case they are optional, and any given must
  match the record, or the command fails with an error naming the difference.
- `-C <folder>` is a flag on the Working copy commands other than `sync` (which takes the folder
  as its optional second positional argument).
- `--json` stays global. `--quiet` is a flag on `sync` only.

### The record (`<folder>/.tidings/`)

- `working-copy`: the record. A plain text file whose first line is a format name and version
  (`tidings working-copy 1`), in the style of the filesystem journal (ADR 0005). It holds:
  the Store's location (an absolute Root override or an App identity), its Backend, the Area, and
  one entry per Path with a Base. Each entry holds the Base Revision (written as text, ADR 0007),
  a hash of the contents last written to or read from the folder for that Base, and whether the
  Path is Diverged. A Diverged entry also records the Revision of the Store's version in `theirs`
  and a hash of its contents, or that the Store has no File there. An unknown format or version is
  refused.
- The record is always replaced whole: written to a temporary file in `.tidings/`, forced to disk,
  and renamed into place (`atomic-write-file`, as the filesystem Backend uses).
- `ignore`: the ignore file, created with the defaults when the Working copy is created, and read
  afresh by every command that scans the folder. It uses `.gitignore` syntax (the `ignore` crate).
- `theirs/`: the Store's version of each Diverged Path, at its Path.
- `tmp/`: where `sync` writes a file before renaming it into place. Being inside the folder keeps
  the rename on one volume.
- `sync.lock`: held with an exclusive `std::fs::File::lock` for as long as a `sync` runs, so a
  second `sync` is refused and `status` can tell whether one is running (by trying it).
- `lock`: held exclusively while any command reads the folder to act on it, or changes the folder
  or the record: `sync` for each reconcile, and `commit`, `discard`, `resolve` and `status` for
  their whole run. `status` holds it so that it never reports a half-applied reconcile.
- A folder is a Working copy if `.tidings/working-copy` exists and starts with the format line.
  The walk up from the current directory stops at the first one found.

### Local states

- Scanning walks the folder, skipping `.tidings/`, and gives each Path one of: *absent*, *regular
  file* (with the hash of its contents), or *invalid* (a name that isn't a valid Path, contents
  that aren't UTF-8, a symlink, or any file that isn't a regular file). Directories exist only as
  Prefixes, and empty ones are skipped. A file with no Base that matches the ignore file is left
  out entirely; the ignore file never applies to a Path with a Base.
- Compared with its record entry, a Path is *unchanged* (hash equal to the Base's), *modified*,
  *deleted* (it has a Base, but no file), *added* (a file, but no Base), *invalid*, or *diverged*
  (flagged in the record). Case differences on a case-insensitive folder are handled by Path
  rules: renaming `Foo` to `foo` is a delete of `Foo` and an add of `foo`.
- The contents hash is internal to the Working copy (xxh3, which the library already uses), and is
  never compared with a Revision, which stays opaque.

### Reconciling a Path

For a Path, with its Store state `S` (absent, or a File with its Revision), its Base `B` (none, or a
Revision) and its local state `L`:

| Store vs Base | Local | Result |
| --- | --- | --- |
| `S` matches `B` (both absent, or same Revision) | anything | nothing: local edits stay local |
| differs | unchanged, or absent with no Base | apply `S` to the folder, and `S` becomes the Base |
| differs | modified / added / deleted / invalid, and the local contents equal `S` (or both absent) | `S` becomes the Base silently |
| differs | modified / added / deleted / invalid, contents differ | Diverged: `theirs` gets `S` (or is removed if `S` is absent), the local file is untouched |

- A Path already Diverged is reconciled the same way: `theirs` is refreshed when `S` changes, or
  written again if it is missing, and the Divergence is cleared if the local contents now equal
  `S`, or if `S` matches `B` again (the first row).
- If `theirs` can't be written or removed (`.tidings/theirs/` is a symlink, or the person put a
  directory where the file goes, say), the Path is Diverged, or no longer Diverged, all the same,
  and `sync` carries on. An *error* names the Path and says why, in place of the *diverged* line if
  `theirs` wasn't written. Each later reconcile tries again, silently: the *error* is reported
  once while `sync` runs, unless the Path's Divergence changes. Nothing outside `.tidings/theirs/`
  is written or removed, and no symlink followed.
- Applying `S` writes the file in `tmp/`, forces it to disk, and renames it into place, making any
  directories it needs; or removes the file, then removes each directory the removal emptied, up to
  the folder, stopping at any that isn't empty. Within one reconcile, removals happen before
  writes, so a File can move to where a directory was.
- If applying fails because of what is in the folder (a directory holding other files where a File
  must go, a symlinked directory on the way), the Path is Diverged instead, and `sync` carries on.
- The folder is changed first and the record saved after. A crash in between leaves local contents
  equal to the Store's, which the third row turns into the new Base with no report: the Working
  copy heals itself.
- The whole Area is reconciled at `sync` start and on each Resync for the Area. Otherwise, each
  batch of Changes from the Change feed (for this Area only) reconciles just its Paths, reading
  each one's Store state again. A Change is never trusted for contents (CONTEXT.md). `sync` takes
  the Change feed from opening the Store before its first full reconcile, so nothing is missed in
  between.

### `sync`

- On a missing or empty folder: create the folder, `.tidings/`, the ignore file and an empty record
  under `lock`, then reconcile everything, which writes every File. On an existing Working copy:
  check it's for the Store and Area asked for, then reconcile everything.
- Output: one line per event, *created*, *updated*, *removed*, *diverged* (naming the `theirs`
  file, or saying the Store removed it), *resolved* (a Divergence that cleared by itself),
  *resync*, and *caught up* after each reconcile that leaves nothing pending. `--quiet` keeps only
  *diverged*, *resync* and errors. With `--json`, each is a JSON object on its own line with an
  `event` field, in the style of `store watch`; a *diverged* one also has `theirs`, the `theirs`
  file relative to the folder, or `null` if the Store removed the File.
- Ctrl-C is only acted on between reconciles, so a reconcile always finishes and saves the record.
- `sync` on a memory Backend is refused, since no other process could reach it.

### `commit`

- Under `lock`: scan, classify, and choose the Paths (all, or those under the paths given,
  resolved relative to the current directory to Paths in the Area).
- Refusals before anything is committed: any chosen Path invalid (exit 1, listing every one); a
  full commit with any Diverged Path, or a named path that is Diverged (exit 3, listing them).
- One Staging for the Area: *added* writes require **absent**, *modified* writes and *deleted*
  deletes require **unchanged since** the Base. Unchanged Paths aren't staged.
- On success, each committed Path's Base becomes its new Revision from `Committed` (a deleted Path
  loses its entry), with the hash of the committed contents, and the record is saved. `sync` then
  sees the Commit as external Changes whose Store Revision matches the new Base, and reports
  nothing.
- On `Error::Conflict`, nothing is committed. Each Path it names is reconciled against the Store,
  which marks it Diverged (writing `theirs`), or, if the contents turn out equal, takes the new
  Base. The command exits 3, naming them.
- `Error::Pending` means the Commit happened: the record is updated as on success, and the
  command says so and exits 0, as `store write` does.
- Nothing to commit: says so, exits 0.

### `discard`, `resolve`, `status`

- `discard [paths…]`, under `lock`: for each chosen Path that is modified, deleted, invalid or
  Diverged, read the Store's version now and apply it to the folder, as `sync` would, making it
  the Base and clearing any Divergence. With no paths given, *added* files are left alone; a named
  *added* file is removed. Reports what was discarded.
- `resolve <path…>`, under `lock`: each must be Diverged. The Base becomes the Store version
  recorded for `theirs`: its Revision, or no Base if the Store had removed it. The Divergence is
  cleared and `theirs` removed. The folder is untouched. A later commit therefore still conflicts
  if the Store has changed since the version merged against. Like `status`, it reads the Store
  only to open it.
- `status`: the classification above for every Path that isn't unchanged, plus whether `sync` is
  running. Text for a person, or JSON with `--json`. It reads the Store only to open it (to check
  the record's Store is still there), not to reconcile: it reports the folder against the record.
- All three accept `-C` and find the Working copy by walking up, as `commit` does.

### README

- Rewritten to present the CLI as an everyday tool, with Working copies first, the `tidings store`
  commands after, and the Working copy's own limits (such as: the Store's version of a Diverged
  Path is only as fresh as the last `sync` or `commit`) in the same "What, and why" style as the
  Consistency section.

## Testing Decisions

- **One seam: the `tidings` binary, run as a subprocess**, through the existing `cli/tests/common`
  harness: a `Location` holding a Store under a temporary Root override, with the environment
  cleared. A test sets up a Store with `tidings store write`, makes a Working copy in another
  temporary directory, edits files there with `std::fs` (standing in for vim), and checks the
  result with `tidings store read` and the Working copy commands' output and exit codes.
- `sync` is spawned as a long-running child with `--json`. The harness gains a helper to read its
  stdout line by line until an event matches (with a timeout that fails the test), especially
  *caught up*, so tests wait on what `sync` says rather than sleeping. It is stopped with SIGINT,
  checking that it exits 0.
- Good tests check only behaviour a person could see: files in the folder, what the Store holds,
  what commands print, and exit codes. They don't read the record's format, which is free to
  change; the exception is a test that a record in an unknown format is refused.
- Every behaviour runs on both fs and SQLite Stores where it could differ, as `cli/tests/edit.rs`
  alternates between them today.
- Changes from "another process" are made by running `tidings store write` against the same Root
  override while `sync` runs, as `cli/tests/edit.rs` does from its editor script.
- Crash recovery is tested through the binary by killing `sync` (SIGKILL) and resuming it, and by
  making the folder or record disagree the way an interrupted run would, such as by leaving a file
  written but the record unchanged, then checking the next command heals it.
- In-process tests of the Working copy module are added only for conditions the binary can't
  produce reliably. Candidates: a crash between the folder change and the record save at an exact
  point, and the reconcile table's rows where timing matters (a Store change landing between a
  commit's scan and its Commit). They go through the module's interface, not its internals.
- Prior art: `cli/tests/one_shot.rs` (one-shot commands, exit codes, JSON), `cli/tests/shell.rs`
  (a long-running child driven through its streams), `cli/tests/edit.rs` (another process writing
  mid-operation, fs and SQLite). The existing CLI tests are updated for the move under
  `tidings store`.

## Out of Scope

- A real filesystem mount (FUSE, macFUSE, NFS) (ADR 0008).
- A Working copy of the whole Store, or of more than one Area in a folder.
- Memory Stores.
- Daemonizing `sync`, starting it at login, or a `stop` command. `sync` runs in the foreground;
  a service manager can run it.
- Merging: tidings writes `theirs` and never merges text itself.
- History, commit messages, stashing or branches. A Store has none.
- Watching the folder for local edits. `status` and `commit` scan it when run.
- Moving the Working copy into the library for apps to embed. It can move later if an app needs it.
- Binary files, as in the library.
- Any library change.

## Further Notes

- Two refinements were made while writing this, beyond what was discussed:
  - `resolve` takes as Base the Revision of the Store's version recorded for `theirs` (the one
    the person merged against), rather than whatever the Store holds at the moment `resolve` runs.
    That's what "unless the Store changes yet again" needs when `sync` isn't running to refresh
    `theirs`: a change made since then still conflicts instead of being silently overwritten.
  - `discard` with no paths leaves *added* files alone, and removes one only when it is named,
    since a bare discard deleting files the Store never had would destroy work.
- The existing `edit` command stays, under `tidings store edit`, for a quick change without a
  Working copy.
- The Working copy's record, and so the self-healing after a crash, relies on "same contents means
  same state", which holds because Paths are text and the hash covers the whole contents.
