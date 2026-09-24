---
status: accepted
---

# How the filesystem Backend makes a Commit all-or-nothing

The filesystem Backend keeps a redo journal, `.tidings/journal`, as a plain file. We did not use
SQLite for it: the journal is the easy part, and SQLite does nothing to make the target Files
themselves safe to replace. Each Commit, while holding an exclusive `std::fs::File::lock` on
`.tidings/lock`, does this:

1. Check the Preconditions.
2. Write the journal in the `prepared` state, listing the temporary files it is about to create.
3. Write each new File to a temporary file next to its target, named
   `.<name>.tidings-<commit-id>-<n>` for the Commit's `n`th write, and force it to disk.
4. Write the journal in the `committed` state. From this point the Commit has happened.
5. Make the deletes, then rename each temporary file over its target.
6. Remove the journal.

Each journal write goes to `journal.tmp`, is forced to disk, and is renamed over `journal`, with
the directory then forced to disk. `atomic-write-file` does this, so we do not write it ourselves.
When a Store is opened, a `prepared` journal means its temporary files are deleted and the Commit
never happened. A `committed` journal means steps 5 and 6 are finished. A Commit does the same
first, under the lock, in case another process's Commit was interrupted while this Store was open.

A temporary file goes next to its target rather than in `.tidings/`, because a target can be a
symlink into another directory, or can be on another volume. Renaming onto the symlink itself would
replace a user's link with a plain file, and a rename across volumes is not all-or-nothing. That is
also why the journal needs a `prepared` state: without it, a crash could leave temporary files
scattered across the Area that nothing knows about. The watcher ignores the temporary files' names,
and no Path can have one, so that no File is ever taken for a temporary file.

Details settled while building it (ticket 09):
- The `-<n>` in a temporary file's name keeps two of a Commit's temporary files apart when they
  go in the same directory, which the next point allows.
- When a target's directory doesn't exist yet, its temporary file goes in the nearest directory
  above it that does, within the Area, and the directory is made in step 5, after the deletes.
  That is how a File moves under its own name in one Commit: the file `a` must be deleted before
  the directory `a/` can be made for `a/b`. Likewise, deleting `d/e` removes the emptied directory
  `d/` before a File is renamed onto `d`. Each directory made or changed is forced to disk before
  the journal is removed.
- A write through a symlink to a file goes to the file the link points to, and its temporary file
  next to that. Such a write never makes a directory, so it can't make one outside the Area: if
  the directory the link points into doesn't exist, the Commit is refused with `Backend` before
  step 2. A write under a symlink to a directory is different: it makes the directories it needs
  in that directory, wherever the link points, as writing there by hand would.
- Between steps 1 and 2, the Commit is also refused, with `InvalidPath`, where finishing it could
  go wrong. With `FileUnderFile`, if something that isn't a Path stands where a File it writes, or
  a directory for one, must go, and the Commit's deletes don't remove it: a directory holding
  names that aren't Paths, a symlink to nothing, or another kind of file. The checks every Backend
  shares can't see these, and meeting one in step 5 would leave a Commit that can never be
  finished. With `SameFile`, if two of its Paths are the same file on disk, as a symlink and the
  File it points to are, or two links to one File, and one of them is written. Otherwise one
  Path's delete could remove what another's write put there, and which wins would depend on the
  order they land in. Two deletes of one file are fine, as a Prefix delete over a directory link
  and the directory it points to makes: the second finds nothing. Where names fold, which file a
  Path is on disk is compared folded, so links to `foo` and `FOO` are one file; a write of `foo`
  with a delete of `Foo` is still a rename.
- A Path names only the file with exactly its name. Where the filesystem ignores letter case
  (macOS's and Windows' by default, found out when a Store opens, from whether `.tidings/LOCK`
  finds `.tidings/lock`), reads check each name on the way against the directory's entries, so
  `foo` is absent when only `Foo` is there. So a Commit that renames `Foo` to `foo` deletes `Foo`
  and writes `foo`, even with the same contents, and finishing it again doesn't delete `foo`: see
  the next point. Letter case is the only difference this detects; a filesystem that keeps letter
  case apart but ignores Unicode normalization isn't detected, and neither is Windows' letter case
  set per directory elsewhere in the Area. The check reads the whole directory for each name on
  the way, so reading every File of a flat directory of N Files one by one costs in proportion to
  N². Asking the platform for a file's name as it is on disk (`GetFinalPathNameByHandleW` or
  `FindFirstFileW` on Windows, `F_GETPATH` or `getattrlist` on macOS) would cost the same for
  every File, and is left for later.
- Finishing a Commit again, after a crash part way through or just before the journal was
  removed, leaves the Area as finishing it once did. A temporary file that is gone was renamed
  already, and is skipped. The journal records the Revision of each File a delete removes, and
  finishing again deletes a File only if it is still there under exactly its name with that
  Revision: so a File that one of the Commit's writes, or another program, has put there since is
  left alone. Only a File another program wrote there since with the very same contents would be
  deleted again.
- A Commit that only deletes has no temporary files, so its journal is written only once, as
  `committed`.
- Removing the journal isn't forced to disk. If a power cut loses that, the Commit is finished
  again, as above, and the next Commit's journal replaces a stale one before it changes anything.

## Consequences

- On the filesystem, a Revision is a hash of the File's contents. Timestamps are too coarse to tell
  whether a File has changed, and comparing contents also lets a write that changes nothing be left
  out of the Commit.
- If step 5 cannot finish (on Windows, a program holding the target open can block the rename),
  `commit` retries briefly and then returns `Error::Pending`. Until the remaining renames are
  finished, reads through tidings of those Paths come from the temporary files, and the next Commit
  or `open` tries the renames again. So the all-or-nothing guarantee holds for everything that
  reads through tidings.
- The lock only keeps out other tidings Commits. A program outside tidings that writes a File
  between step 1 and the end of step 5 is not detected by the Preconditions, and if the Commit
  writes that File, its edit is overwritten. The filesystem has no way to replace a File only if
  it is unchanged, so we document this window rather than trying to close it. Checking the
  Preconditions after step 3 instead would narrow the window only by a few writes to disk, so the
  order stays as it is.
- We rely on tests that kill the process at every step and check what `open` recovers, not on
  reasoning alone.
