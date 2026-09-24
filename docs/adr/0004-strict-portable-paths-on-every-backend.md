---
status: accepted
---

# Paths follow the strictest platform's rules, on every Backend

A Path is relative and separated by `/`. It may not contain empty segments, `.` or `..`, and each
segment must be a name every platform accepts, so names Windows reserves such as `CON` are
refused. It must be in Unicode NFC form. These rules apply on SQLite and in memory too, even though
those Backends could store any string. Two more need the Area's contents, so they are checked when
a Commit is made:

- A Commit may not create a Path that differs only in letter case from another Path in the Area,
  or put it under a Prefix that does (`Themes/a` beside `themes/b`).
- A Commit may not create a Path that is also a Prefix of another Path (`a` beside `a/b`), because
  a filesystem can't have a file and a directory with the same name.

The reason is that a Store should behave the same whichever Backend it uses and whichever platform
it runs on. macOS and Windows filesystems treat `Config.toml` and `config.toml` as the same file,
and macOS treats the one-character and two-character forms of `é` as the same name. Without these
rules, data that works on SQLite, or on Linux, could not be moved to the filesystem Backend on
those platforms.

We check Paths with established crates instead of writing the rules ourselves: `relative-path` for
the structure, `sanitize-filename` (with its Windows rules) for each segment,
`unicode-normalization` for NFC, and `caseless` for comparing letter case.
