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
3. Write each new File to a temporary file next to its target, named `.<name>.tidings-<commit-id>`,
   and force it to disk.
4. Write the journal in the `committed` state. From this point the Commit has happened.
5. Rename each temporary file over its target, and make the deletes.
6. Remove the journal.

Each journal write goes to `journal.tmp`, is forced to disk, and is renamed over `journal`, with
the directory then forced to disk. `atomic-write-file` does this, so we do not write it ourselves.
When a Store is opened, a `prepared` journal means its temporary files are deleted and the Commit
never happened. A `committed` journal means steps 5 and 6 are finished.

A temporary file goes next to its target rather than in `.tidings/`, because a target can be a
symlink into another directory, or can be on another volume. Renaming onto the symlink itself would
replace a user's link with a plain file, and a rename across volumes is not all-or-nothing. That is
also why the journal needs a `prepared` state: without it, a crash could leave temporary files
scattered across the Area that nothing knows about. The watcher ignores the temporary files' names.

## Consequences

- On the filesystem, a Revision is a hash of the File's contents. Timestamps are too coarse to tell
  whether a File has changed, and comparing contents also lets a write that changes nothing be left
  out of the Commit.
- If step 5 cannot finish (on Windows, a program holding the target open can block the rename),
  `commit` retries briefly and then returns `Error::Pending`. Until the remaining renames are
  finished, reads through tidings of those Paths come from the temporary files, and the next Commit
  or `open` tries the renames again. So the all-or-nothing guarantee holds for everything that
  reads through tidings.
- We rely on tests that kill the process at every step and check what `open` recovers, not on
  reasoning alone.
