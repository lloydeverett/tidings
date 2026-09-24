---
status: accepted
---

# Paths follow the strictest platform's rules, on every Backend

A Path is relative and separated by `/`. It may not contain empty segments, `.` or `..`, and each
segment must be a name every platform accepts, so names Windows reserves such as `CON` are
refused. It must be in Unicode NFC form. A Commit may not create a Path that differs only in letter
case from a Path already in the Area. These rules apply on SQLite and in memory too, even though
those Backends could store any string.

The reason is that a Store should behave the same whichever Backend it uses and whichever platform
it runs on. macOS and Windows filesystems treat `Config.toml` and `config.toml` as the same file,
and macOS treats the one-character and two-character forms of `é` as the same name. Without these
rules, data that works on SQLite, or on Linux, could not be moved to the filesystem Backend on
those platforms.

We check Paths with established crates instead of writing the rules ourselves: `relative-path` for
the structure, `sanitize-filename` (with its Windows rules) for each segment, and
`unicode-normalization` for NFC.
