# 03: Docs for Stores at chosen Locations

Spec: [0003](../../specs/0003-stores-at-chosen-locations.md)

**What to build:** Someone reading the README or the crate docs learns that they open a Store at a
directory of their choosing, as many as they need, each on the Backend that suits it, and how to
get the platform's standard directories with `etcetera` themselves. Nothing mentions Areas, the
App identity or a Root override any more.

**Blocked by:** 01, 02

**Status:** done

- [x] The README's opening, and its walkthrough and command reference, use `--store` and no Areas.
- [x] An example opens a config Store on the filesystem and a data Store in SQLite, at directories
      from `etcetera`, and waits on both Change feeds with `select!`.
- [x] The table of which Backend suits which use is framed as a choice per Store.
- [x] The README says what nesting does: opening inside another Store or a Working copy is
      refused, and a nested one is outside a filesystem Store. It notes that on macOS
      `etcetera`'s native strategy gives the same directory for config and data, which opens one
      Location twice.
- [x] The crate-level docs and the `Cargo.toml` description match.
- [x] ADR 0009 is marked accepted, and the spec's status says it is built.
