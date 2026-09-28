# 09: `tidings discard` and `tidings resolve`

Spec: [0002](../../specs/0002-working-copies.md)

**What to build:** The two ways to settle a local change. `discard` throws the person's side away
and takes the Store's version. `resolve` says "what's in my folder is the merge", so the next
commit goes through, unless the Store has changed since the version the person merged against.

**Blocked by:** 06

**Status:** ready-for-agent

- [ ] `tidings discard [paths…]` reads the Store's version of each chosen modified, deleted,
      invalid or Diverged Path, applies it to the folder as `sync` would, makes it the Base,
      clears any Divergence and removes `theirs`.
- [ ] With no paths, *added* files are left alone. A named *added* file is removed.
- [ ] Paths are relative to the current directory, and a directory means everything under it, as
      for `commit`.
- [ ] `tidings resolve <path…>` requires each Path to be Diverged, refusing otherwise. It makes the
      Store version recorded for `theirs` the Base (or leaves no Base if the Store had removed
      it), clears the Divergence, removes `theirs`, and leaves the folder untouched.
- [ ] After `resolve`, a `commit` goes through. If the Store changed again after `theirs` was last
      written, the commit is a Conflict instead.
- [ ] Both print what they did, with `--json` too, and take `-C` or walk up.
- [ ] Tests, through the binary, on fs and SQLite, with `sync` running and not running.
