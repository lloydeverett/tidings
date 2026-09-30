# tidings

Text files for an application, in three areas (config, data, cache), stored on the filesystem, in
SQLite or in memory. Writes happen only through all-or-nothing commits, and every change is
reported on a change feed. Async on tokio; `tidings::blocking::Store` (feature `blocking`) for
synchronous code.

| Use | Filesystem | SQLite | Memory | Why, where it isn't obvious |
| --- | :---: | :---: | :---: | --- |
| Files that must agree with each other, read together | | ✅ | ✅ | The filesystem has no Snapshots, so reading several files can mix commits, and other programs can see a commit half-applied. |
| Editing the files in place, with any editor or tool | ✅ | | | Only the filesystem holds ordinary files. An edit landing mid-commit can be lost: a window of milliseconds. |
| Editing with the `tidings` command | ✅ | ✅ | | A Working copy or `tidings store` reaches a Store from another process. Memory only in `tidings store shell`, and gone when it exits. |
| Reading, searching or backing up with ordinary tools (grep, git, rsync) | ✅ | | | |
| Keeping Files after the app exits | ✅ | ✅ | | |
| Sharing a Store between processes | ✅ | ✅ | | Commits reach other processes late: on SQLite within a poll interval (100 ms), on the filesystem once events are quiet for 150 ms. |
| Commits never held up by another program | | ✅ | ✅ | On Windows, a file another program has open blocks every commit to its Area until released. |
| A change feed with only real changes | | ✅ | ✅ | The filesystem can report a rewrite with unchanged contents, or on macOS a permissions change. |
| Quick `stat` and Prefix Revisions of large Files | | ✅ | ✅ | The filesystem reads the whole File for its Revision. |
| Nothing written to disk (tests, throwaway state) | | | ✅ | |

The details are under [Consistency](#consistency) and [Limitations](#limitations).

Status: early. The library's first version is complete, and the `tidings` command now has Working
copies for everyday editing. See [CONTEXT.md](CONTEXT.md) and [docs/adr](docs/adr).

## The `tidings` command

The `cli/` crate builds a `tidings` binary (`cargo install --path cli`) for reading and editing an
app's Files with your own tools, even when they live in SQLite. It uses only the library's public
API.

Its everyday use is a **Working copy**: a folder holding one Area's Files as ordinary files. `sync`
keeps it in step with the Store, you edit it with vim, grep or anything else, and `commit` sends
your edits back as one all-or-nothing Commit. It is a copy, not a mount ([ADR
0008](docs/adr/0008-a-working-copy-is-a-copy-not-a-mount.md)): nothing reaches the Store until you
commit, and syncing never overwrites a local edit.

### A walkthrough

To follow along, make a Store for `sync` to find. Here it is in SQLite, under `./store`; an app's
own Store is found with `--identity tld.author.app` instead of `--root`.

```sh
$ echo 'theme = "dark"' | tidings --root store --backend sqlite --create store write config app.toml
ed9fe5424334710f362be5fcc2cce6fe  app.toml
$ echo 'leader = ","' | tidings --root store store write config keys/vim.toml
492bd6e6af7dc44267aad24339eddd8a  keys/vim.toml
```

Make `cfg` a Working copy of the config Area, and leave `sync` running in a terminal of its own.
It prints one line for everything it does, until Ctrl-C:

```sh
$ tidings --root store sync config cfg
created app.toml
created keys/vim.toml
caught up
```

In another terminal, edit, check what a commit would do, and commit. The Working copy remembers
its Store, so no flags are needed, and every command works from anywhere inside the folder, as in
git:

```sh
$ cd cfg
$ vim app.toml                        # theme = "light"
$ echo 'bold = true' > keys/new.toml
$ tidings status
modified app.toml
added keys/new.toml
sync is running
$ tidings commit
modified app.toml
added keys/new.toml
```

`sync` says only `caught up`: it knows those Changes are yours. Now edit `app.toml` again, while
the app (here, `tidings store write`, from another process) changes it too:

```sh
$ vim app.toml                        # theme = "solarized"
$ printf 'theme = "light"\nfont = 16\n' | tidings --root ../store store write config app.toml
a7e9f66bab69fdb7d209cb5092f14375  app.toml
```

`sync` leaves your file alone. The Path is **Diverged**, and the Store's version is put where you
can merge against it:

```sh
diverged app.toml: the Store's version is in .tidings/theirs/app.toml
caught up
```

A commit that includes it is refused:

```sh
$ tidings commit
tidings: can't commit Diverged Paths: merge each and `tidings resolve` it, or `tidings discard` it, or name only other paths to commit
  diverged app.toml: the Store's version is in .tidings/theirs/app.toml
```

Either merge it and say so with `resolve`, then commit:

```sh
$ vimdiff app.toml .tidings/theirs/app.toml
$ tidings resolve app.toml
resolved app.toml: its Base is now the Store's version it was merged with
$ tidings commit
modified app.toml
```

Or give up your side with `discard`, which takes the Store's version:

```sh
$ tidings discard app.toml
discarded app.toml: took the Store's version
```

### Choosing a Store, output and exit codes

These apply to every command, both for Working copies and under `tidings store`.

- **Choosing a Store.** Give exactly one of `--root <dir>` or `--identity tld.author.app` (the
  platform's directories for that App identity). The Backend is found from the Backend markers;
  `--backend fs|sqlite` makes sure of it. A location with no Store is refused unless you pass
  `--create` too, so a typo doesn't make a new, empty one. `TIDINGS_ROOT`, `TIDINGS_IDENTITY` and
  `TIDINGS_BACKEND` stand in for the flags.
- **Output.** Text for a person, or JSON with `--json`: one object per line for `sync` and
  `store watch`, one object for the others. Under `--json`, any failure is one JSON object on
  stderr, `{"failure": …, "message": …}`, with a `paths` list for the Paths it names.
- **Exit codes.** `0` success, `1` error, `2` no File (`store read`, `store stat`), `3` Conflict,
  or a commit refused because a Path is Diverged. The `failure` in JSON is `error`, `invalid` or
  `blocked` (exit 1), `missing` (2), or `conflict` or `diverged` (3). `invalid` is a `commit`
  refused over invalid files; `blocked` is a `discard` refused because something in the folder,
  such as a file where a directory must go, keeps the Store's File from being written there.

### Working copies

- **Choosing the Store.** The first `sync` of a folder chooses its Store with the flags
  [above](#choosing-a-store-output-and-exit-codes). After that the Working copy remembers its
  Store, Backend and Area, so the flags are optional. Any flags given, or set in the environment,
  must match, or the command is refused: a stale `TIDINGS_ROOT` in your shell can't commit into
  the wrong Store. `--create` makes a new Store only for a new Working copy. It is refused on an
  existing one, even if its Store has gone, since a new, empty Store would remove every unchanged
  file from the folder.
- **Finding the Working copy.** Every command but `sync` finds it from the current directory or a
  folder above it, or from `-C <folder>`, which goes after the command: `tidings status -C cfg`,
  not `tidings -C cfg status`. Paths you name are relative to the current directory, and a
  directory means everything under it. Paths printed are relative to the folder.

| Command | |
| --- | --- |
| `sync <area> [folder]` | Make an empty or missing folder (the current directory if left out) a Working copy of the Area, or resume one, and keep it in step with the Store until Ctrl-C. `--quiet` prints only its `diverged`, `resync` and `error` lines. |
| `commit [paths…]` | Commit every local change, or only those under the Paths you name, as one Commit. |
| `status` | List what `commit` would do, and say whether `sync` is running. |
| `discard [paths…]` | Take the Store's version of each changed or Diverged Path, or only those you name. |
| `resolve <paths…>` | Take what is in the folder as the merge of each Diverged Path. |

**`sync`** resumes where it left off, catching up on whatever changed while it wasn't running. It
refuses a folder that holds files but isn't a Working copy, a Working copy of another Area, and
a second `sync` of the same folder. It prints one line for each thing it does:

- `created`, `updated`, `removed`: a Change in the Store applied to the folder. A directory it
  empties is removed; one still holding your files is left alone.
- `diverged`: the Path changed both locally and in the Store since its Base (the Revision last
  taken from, or committed to, the Store). The Store's version is in `.tidings/theirs/<path>`, or
  the line says `removed in the Store`. A later Change in the Store refreshes it. A local file that
  is in the way (a directory where the Store now has a File, say) also makes the Path Diverged.
- `resolved`: the Path is no longer Diverged, without a `resolve`, because the local contents now
  equal the Store's, or the Store went back to the Base.
- `error`: something `sync` couldn't do for one Path, such as writing its `theirs` file. It
  carries on, and tries again later.
- `resync`: Changes may have been missed, so it checked the whole Area again.
- `caught up` (`caught-up` in JSON): it has done everything it knows of. A script can wait for it
  rather than sleeping. With `--json`, a `diverged` line also has `theirs`: the file relative to
  the folder, or `null`.

**`commit`** requires each new file's Path to be absent in the Store still, and each changed or
deleted one to be unchanged since its Base. If one isn't, nothing is committed. The command exits
3, naming them, and marks them Diverged, with `theirs` written, whether or not `sync` is running.
A commit that would include a Diverged Path is refused up front (exit 3), but one naming only
other Paths goes ahead. If another process committed exactly your contents already, that is no
Conflict: they become the Base. `commit` and a running `sync` take turns, so `sync` never writes
into the middle of a commit. With nothing to commit, it says so and succeeds. A Path you name that
isn't in the folder and has no Base, a typo say, is refused with `no such file in the Working
copy` (exit 1), by `discard` too.

**`status`** lists each Path as `modified`, `added`, `deleted`, `diverged` or `invalid`, comparing
contents, so a `touch` shows nothing. It doesn't ask the Store what changed: see the
[limits](#working-copy-limits).

**`discard`** reads the Store's version now, and writes it into the folder (or removes the file),
making it the new Base, so the Path is no longer Diverged. A file you added, with no Base, is left
alone by a bare `discard` or by naming a directory it is in; it is removed only when named itself.

**`resolve`** makes the Revision in `theirs`, the Store's File you merged against, the Base. It
removes `theirs` and leaves the folder as it is. So if the Store has changed again since, the next
commit is a Conflict all the same: resolving never hides a change you haven't seen.

**The ignore file**, `.tidings/ignore`, says in `.gitignore` syntax which local files `commit`,
`status` and `discard` leave out. It starts with the usual editor and OS leftovers: `.*.sw?`, `*~`,
`4913` (the file vim writes to test a directory), `.DS_Store`, `Thumbs.db` and `.#*`. It applies
only to files with no Base: a File the Store holds is always synced and committed, even if a
pattern matches it. As in git, a file under an ignored directory stays ignored whatever pattern
re-includes it. If the ignore file is missing, nothing is ignored. If it is a symlink or isn't a
regular file, or a pattern is bad (the error names its line), commands that read it are refused.

**Invalid files** are those that can't be Files: a name that isn't a valid Path (such as `CON` or
`a:b`), contents that aren't UTF-8, a symlink, or anything that isn't a regular file. `status`
lists them, and `commit` refuses, naming every one, rather than leave any out of an all-or-nothing
Commit. Rename or remove each, or ignore it. Empty directories are skipped, since a Store has none.

```sh
$ touch a:b; ln -s app.toml link.toml
$ tidings status
invalid a:b: isn't a valid Path: a segment is not a name every platform accepts
invalid link.toml: is a symlink
sync isn't running
```

**After a crash** in `sync` or `commit`, two things can be left behind. A `theirs` file for a Path
no longer Diverged is removed by the next `sync`, `discard` or `resolve`. And if the crash came
before the Working copy recorded a commit's new Bases, `status` lists the committed Paths as
`modified` until the next `commit` or `sync`, which finds them already in the Store.

### `tidings store`

The commands under `tidings store` work on the Store directly, bypassing any Working copy: for a
quick look or a one-off change, or from a script.

```sh
export TIDINGS_ROOT=/tmp/scratch                  # or --root, or --identity com.example.myapp
tidings --backend fs --create store list data     # a new Store needs --create and --backend
echo 'a = 1' | tidings store write config app.toml
tidings store read config app.toml                # the contents, exactly as stored
tidings store stat config app.toml                # its Revision and modified time
tidings store write data a.txt --contents x --if-revision <revision>
tidings store edit config app.toml                # in $VISUAL or $EDITOR; :cq cancels, a Conflict keeps your edit
tidings store watch data                          # every Change to data, until Ctrl-C
```

- **Choosing a Store, output and exit codes:** as
  [above](#choosing-a-store-output-and-exit-codes).
- **Commands.** `read`, `stat`, `list`, `stat-prefix`, `write`, `delete`, `delete-prefix`, `edit`
  and `watch`, each taking the Area then the Path or Prefix. `write` takes the contents from stdin,
  `--contents` or `--from <file>`. `write` and `delete` take `--if-absent` or `--if-revision`.

`tidings store shell` keeps one Store open, so a Commit can be built up over several commands,
and the memory Backend (`--backend memory`) can be used. It takes the same commands (not
`watch`), plus:

| Command | |
| --- | --- |
| `stage <area>` | Open a Staging: `write`, `delete`, `delete-prefix` and `edit` add to it instead of committing. One at a time, shown in the prompt. |
| `require <path> absent\|<revision>` | Add a Precondition to it. |
| `require-prefix <prefix>` | Require the Prefix unchanged since its last `stat-prefix` in this shell. A Prefix Revision can't be typed in, so this works only in the shell. |
| `commit`, `discard` | Commit the Staging, or drop it. Either way it is closed. |
| `feed on\|off` | Print Changes as they arrive. |
| `help`, `exit` | |

On a terminal, Changes are printed above the prompt, a failed command doesn't end the shell, and
`read` adds a newline to contents that don't end with one, so the prompt doesn't overwrite their
last line. With stdin from a file or pipe, it runs the lines as a script (`#` starts a comment),
stops at the first failure with its exit code, and prints Changes on stderr only after `feed on`.
In the shell, `--contents` turns `\n`, `\t` and `\\` into a newline, a tab and a backslash.

### Working copy limits

Ways a Working copy can surprise you, and why.

- **The Store's version in `theirs` is only as fresh as the last `sync` or `commit`.** Without
  `sync` running, the Store may have moved on; `resolve` then commit, and you get a Conflict
  rather than overwriting it. *Why:* only `sync` follows the Store.
- **`status` doesn't reach the Store.** It shows your edits against what the Working copy last
  saw, not what changed in the Store since, nor a Path `sync` hasn't yet found Diverged. *Why:* it
  compares the folder with the Bases the Working copy recorded, so it is quick; `sync` and
  `commit` are what ask the Store.
- **A Diverged Path stays Diverged until something checks it again.** Making your file equal to
  `theirs` doesn't clear it by itself: `resolve` it, or `sync` checks it again when the Store next
  changes the Path, and checks every Path when it starts or resyncs. *Why:* `sync` watches the
  Store, not the folder.
- **An ignored file is still yours.** If the Store makes a File at the Path of an ignored local
  file, the Path is Diverged, not overwritten. *Why:* syncing never overwrites a local file.
- **One Area per Working copy.** *Why:* a commit across Areas would be up to three Commits, and one
  could succeed while another is refused.
- **No memory Stores.** *Why:* no other process can reach one.
- **`sync` runs in the foreground,** until Ctrl-C, which lets the change it is applying finish
  first. There's no daemon or `stop`. *Why:* nothing keeps running that you didn't start; a service
  manager can run it.
- **Symlinks can't be committed,** and `sync` never writes through a symlinked directory in the
  folder: the Path is Diverged instead. *Why:* a Store has no symlinks, and following one could
  write or delete outside the folder.
- **A crash can leave an empty directory** that a removal emptied. *Why:* removing empty
  directories on sight would remove ones you made.

## Consistency

Ways you could lose data or see confusing behaviour, and why.

- **A cancelled commit may still happen.** Dropping the `commit` future (e.g. on a timeout) lets
  a commit that had started finish in the background. *Why:* stopping halfway would leave it
  half-applied, and Rust can't wait for cleanup on drop.
  - If the tokio runtime shuts down meanwhile, it can be applied without being reported on the
    change feed. *Why:* the feed runs on that runtime.
  - Blocking commits can't be cancelled, so don't have this problem.
- **Reading several files can mix states from different commits.** *Why:* reads are one file at
  a time.
  - A snapshot avoids this, but the filesystem has none (check `supports_snapshots()`). *Why:*
    other programs can edit files mid-read; a lock would only hold off tidings, not them.
- **`Error::Pending` means the commit succeeded.** On the filesystem, if a file can't be replaced
  (on Windows, another program has it open), the commit has still happened and is reported.
  - Until the file is released, *every* commit to that area fails, even ones not touching it.
    *Why:* this commit must be finished first to keep commits all-or-nothing.
- **An outside program's edit can be lost if it lands mid-commit.** A precondition is checked,
  then the commit's files are prepared and forced to disk, then renamed into place. An edit
  between the check and the rename is overwritten. Usually a window of milliseconds. *Why:* a
  rename replaces whatever is there, and no filesystem offers a portable "replace only if
  unchanged" or a lock other programs must respect.
  - After a `Pending` commit, or one a crash interrupted, the window lasts until it's finished.
  - Outside programs can also see a commit half-applied.
  - The reverse can happen too: if the edit swaps a directory for a symlink to one, the commit's
    files under it are dropped, though reported as written, and read until the commit finishes.
    *Why:* they're outside the area now, and finishing never writes through such a link, so a
    commit left unfinished doesn't block every later one.
- **The change feed doesn't give you every step.** It says which path changed, not the new
  contents, and merges unread changes to the same path. Re-read to see what's there.
- **Other processes' changes arrive late,** so until then your reads and the feed disagree.
  - SQLite: up to one poll interval (100 ms by default). *Why:* SQLite can't notify other
    processes of a commit, so each store checks. Each check reads a counter from memory SQLite
    shares between processes, not the disk.
  - Filesystem: once events have been quiet for 150 ms, so that a file an editor is saving isn't
    reported half-written. Longer if they keep coming.
  - On the filesystem, your own commit can be reported before another process's commit made just
    before it, and a commit can be split across batches if its files keep changing.
- **Some changes are reported that didn't happen.** On the filesystem, after the store opens,
  the first rewrite (or `touch`) of a file with unchanged contents reports a change. On macOS,
  so can the first change to its permissions, or nothing at all, if the file was made shortly
  before the store opened: FSEvents can report a recent creation again, or late. *Why:* avoiding
  it means reading every file when the store opens, and they can be large.
  - On macOS, an area's directory removed while the store runs can give a second resync a moment
    after the first. *Why:* FSEvents can report the removal, and the directory made again, late;
    taking the directory still there for proof that nothing was missed would be wrong if it had
    been moved away and back.

## Limitations

Each with why, where it isn't obvious.

### Paths

- **Paths follow the strictest platform's rules everywhere.** Even on Linux or SQLite you can't
  use `CON`, `aux.txt`, a trailing dot, or two paths differing only in case (`Themes/a` vs
  `themes/b`). *Why:* so a store works when moved to any platform.
- **On a case-insensitive filesystem (macOS, Windows), reading `foo` gives nothing when the file
  is `Foo`,** though the OS would happily open `Foo`. *Why:* so every backend agrees; on SQLite,
  memory and Linux, `foo` isn't `Foo`.
  - The cost: each read lists every directory on its path to check the exact name, so reading
    all N files in one directory is O(N²).
- **Files other programs make with invalid names are invisible** (left out of listings), yet can
  still block a commit (`FileUnderFile`) if they're where a file must go.
- **Two names differing only in case, made by another program on a case-sensitive filesystem,
  are both listed**, though tidings itself could never have made them.
- **Non-UTF-8 files are listed but give `Error::NotText` when read.**
- **Writes go through symlinks to files**, keeping the link. The target's directory must exist.
  A commit writing both a link and its target is refused (`SameFile`).
- **Symlinks to directories inside an area are ignored**, with everything under them: not
  listed, read or watched. Writing under one is refused (`DirectoryLink`). *Why:* so each file
  has one path, and every directory holding files is watched.
- **An area's directory can be a symlink, but it's resolved once, when the store opens.**
  Re-pointing it later takes effect at the next open.

### Performance

- **Stat reads the whole file, and a prefix revision reads every file under the prefix**, on the
  filesystem. *Why:* revisions come from contents, since modified times can be coarse or set by
  hand.
- **Watching uses memory per file** in each area, to tell what an event changed.
- **A long-held SQLite snapshot makes the database's log grow** until it is dropped. *Why:* that
  is how SQLite keeps the old state readable.

### Blocking store

- **It panics if called from async code** (a tokio task or `block_on`). *Why:* blocking there
  would stall the runtime. Use the async store, or `spawn_blocking`/`block_in_place`.
- **Each one starts its own tokio runtime and thread**, kept running so the feed keeps filling
  between calls. Opening many means many runtimes.

### Other

- Writing a file's current contents again is skipped: it keeps its modified time, and no change
  is reported.
- The change feed ends when every store clone is dropped, even if a snapshot is still held.
- A SQLite store more than 10 minutes behind other processes' commits (a stopped process, say)
  gets a resync. *Why:* the log of commits is pruned, so it stays small without tracking which
  stores are open.
- **Each area records which backend holds it,** in `.tidings/backend`, and a store on the other
  backend refuses to open it (`Error::WrongBackend`). `Store::detect` finds which one a location
  has. The first store to open an area marks it, taking over what is already there.
- Not supported: binary files; moving data between backends; size limits or eviction for the
  cache; your own backends; other programs writing to tidings' SQLite databases.
