# tidings

tidings gives an application three independent places to keep text files: config, data and
cache. It hides whether they live in real directories, a SQLite database or memory. The only way
to write is to stage changes and commit them, and every change is announced to the application.

## Language

### Where files live

**Store**:
What an application opens to reach its files: all three Areas, held by one Backend, for one App
identity.
_Avoid_: filesystem, vfs, repository

**Area**:
One of three independent parts of a Store: Config, Data or Cache. Each is separate from the others
in the same way the platform's config, data and cache directories are.
_Avoid_: storage area, namespace, bucket, volume

**Cache** (the Area):
The Area whose contents may disappear at any time, because the user or the OS cleared it. When
that happens, the app receives Removed Changes, or a Resync if the whole Area vanished. Neither is
an error.

**Backend**:
What actually holds a Store's Files: the filesystem, SQLite or memory.
_Avoid_: provider, driver, adapter

**App identity**:
The app name, author and top-level domain that decide where a Store's Areas are on this platform.

**Root override**:
An explicit location that replaces what the App identity would have picked. It exists mainly for
tests.

### Files

**File**:
A Path, its text contents, and when it was last modified, in one Area.
_Avoid_: entry, blob, object, document

**Path**:
A File's name within its Area: relative, `/`-separated, UTF-8 in NFC form, made only of names that
are safe on every platform. No two Paths in an Area may differ only by letter case, and neither
may the Prefixes they are under (`Themes/a` and `themes/b` clash). A Path can't also be a Prefix of
another Path: `a` and `a/b` can't both be Files.
_Avoid_: key, filename

**Prefix**:
The leading part of a Path, up to a `/`, used to list or delete a group of Files. A Prefix is
either empty, meaning the whole Area, or ends in `/`, as in `themes/`: `themes` alone is not a
Prefix. Directories exist only as Prefixes: there are no empty directories.
_Avoid_: directory, folder

**Revision**:
An opaque value identifying one state of a File, as returned when the File is read. It is used to
tell whether the File has changed since then.
_Avoid_: version, etag, last_modified (that is a timestamp, not a Revision)

**Prefix Revision**:
An opaque value identifying the state of everything under a Prefix: which Paths exist there and the
Revision of each. It changes if a File under the Prefix is added, removed or changed.
_Avoid_: token, digest, listing hash

**Snapshot**:
A view of one Area as it stood at a single moment, so that reading several Files through it never
mixes the results of different Commits. Only some Backends can provide one.
_Avoid_: transaction, consistent read

### Writing

**Staging**:
An owned set of staged writes and deletes for one Area. Nothing happens until it is committed.
_Avoid_: transaction, changeset, batch, draft

**Precondition**:
Something a Staging requires to hold when it is committed. A staged write or delete carries one:
*any*, *absent*, or *unchanged since* a given Revision. A Staging can also carry Preconditions on
Files it does not write, and on a whole Prefix (*unchanged since* a given Prefix Revision).
Preconditions accumulate: a later write or delete of the same Path replaces the earlier one, but
never drops the Precondition it carried.
_Avoid_: guard, assertion

**Commit**:
Applying a Staging all-or-nothing: either every staged write and delete happens, or none do. Every
File in a Commit gets the same last-modified timestamp.
_Avoid_: save, flush, apply

**Conflict**:
A Commit that was refused because at least one Precondition did not hold. Nothing was written.

### Being told about changes

**Change**:
A notice that one Path in one Area was changed or removed, and by whom. It never carries the
contents; the application reads the File again if it cares.
_Avoid_: event, update, notification

**Origin**:
Who made a Change: *local*, meaning a Commit through this Store, or *external*, meaning anything
else (another process, or a person editing the file).

**Change feed**:
The single receiver of a Store's Changes, handed over when the Store is opened. There is exactly
one, and no way to get another later. A Change that has not been read yet is merged with later
Changes to the same Path rather than dropped.
_Avoid_: subscription, listener, watcher

**Resync**:
An item on the Change feed saying that Changes to one Area may have been missed, because watching
failed or the Area itself disappeared. The app should read everything it relies on in that Area
again.
_Avoid_: lost, overflow, reset
