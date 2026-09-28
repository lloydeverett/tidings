# 05: Divergence

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** When a Path has changed both locally and in the Store since its Base, `sync`
leaves the person's file alone and marks the Path **Diverged**. It writes the Store's version to
`.tidings/theirs/<path>` for the person to merge against, and reports it. Same contents on both
sides is not a Divergence. This completes the reconcile table in the spec.

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] A Path modified locally and changed in the Store becomes Diverged. `sync` prints *diverged*
      naming the `theirs` file, and the local file is untouched.
- [ ] A Path deleted locally and changed in the Store, or created locally and in the Store with
      different contents, is Diverged the same way.
- [ ] A Path changed locally and removed in the Store is Diverged as "removed in the Store", with
      no `theirs` file.
- [ ] If the local contents equal the Store's (both sides made the same change, or both deleted
      it), the Store's version becomes the Base with no report.
- [ ] While Diverged, a further Store change refreshes `theirs` and its recorded Revision.
- [ ] If the local file comes to equal the Store's version, the Divergence clears on the next
      reconcile, with a *resolved* line, and `theirs` is removed.
- [ ] If applying a Store change is blocked by what's in the folder (a directory of other files
      where a File must go, a symlink on the way), the Path is Diverged and `sync` keeps running.
- [ ] Divergence is kept in the record, so it survives restarting `sync`. A Divergence that arose
      while `sync` was down is found when it resumes.
- [ ] Tests, through the binary, on fs and SQLite, for each row of the reconcile table.
