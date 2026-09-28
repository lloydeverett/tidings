---
status: accepted
---

# Each Area records its Backend

An Area's directory doesn't say which Backend made it, so a Store could be opened over another
Backend's Areas. A SQLite Store opened over a filesystem Area ignored its Files and added its
database next to them. A filesystem Store opened over a SQLite Area then showed the databases as
Files. Either way, the app saw the wrong Files and could damage the other Backend's data.

So each Area records its Backend in a **Backend marker**, `.tidings/backend`, holding `fs` or
`sqlite` and a newline. Everything a Backend keeps in an Area's directory now lives in
`.tidings/`, which no Path can name, including SQLite's database, which moved from
`<area>/<area>.sqlite3` to `<area>/.tidings/<area>.sqlite3`. No Stores were in use, so there is no
migration.

- **Opening** `open_fs` and `open_sqlite` read all three Areas' markers before marking any. If one
  names the other Backend, they give `Error::WrongBackend { area, found }` without changing
  anything. They then mark each unmarked Area, in order of Area.
- **Unmarked** Areas are those whose directory, `.tidings/` or marker doesn't exist. The first
  Store to open them adopts what they hold. The filesystem shows those files as Files. SQLite
  logs a warning, since it never shows them.
- **Marking** creates the file only if it doesn't exist (`create_new`). A Store that finds one
  has appeared since it read the markers reads it, and fails if it names the other Backend. When
  Stores on both Backends open an unmarked location at once, the one that marks the config Area
  wins, and the other fails before it marks anything else, so the Areas never end up split.
- **A marker naming no Backend**, or one that can't be read, is an error and is never overwritten.
  One that stays empty is an error too, once it has been read again for a moment, in case
  another Store was writing it.
- **Clearing a Cache** removes its marker. The next Store to open marks it again, and meanwhile
  the other Areas' markers still keep out the other Backend.

`Store::detect(app, root_override)` reads the markers without creating anything. It returns the
one Backend they name, `None` if no Area is marked, and `WrongBackend` if they disagree. That lets
a tool such as the CLI open a location without being told its Backend.

The memory Backend has no directories, and so no marker.

## Considered options

- **Checking only what is in the directory, such as the database files**: rejected. An empty Area
  looks the same on both Backends, and the filesystem can hold any file.
- **One marker for the whole Store, under a Root override**: rejected. Without one, the Areas are
  in unrelated platform directories, with no shared place to put it.
- **The marker in each Area's root, beside the Files**: rejected. It would need a name no Path can
  have, and `.tidings/` already is one.
- **Writing the marker to a temporary file and linking it into place**, so it never shows empty:
  rejected. Some filesystems have no hard links. Reading an empty marker again for a moment covers
  the short time between creating it and writing it.
