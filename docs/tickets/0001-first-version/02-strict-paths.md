# 02: Strict Paths

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0004](../../adr/0004-strict-portable-paths-on-every-backend.md)

**What to build:** Every Path and Prefix an app gives tidings is checked against the rules of the
strictest platform, the same way on every Backend. A Path that isn't allowed gives `InvalidPath`
straight away, wherever it is used, so a Path that works on one platform works on all of them.
Established crates do the checking; we don't write the rules ourselves.

**Blocked by:** 01

**Status:** done

- [x] Structure is checked with `relative-path`. Refused: absolute Paths, empty segments, `.`,
      `..`, and a leading `/`.
- [x] Each segment is checked with `sanitize-filename` using its Windows rules. Refused, for
      example: `CON`, `aux.txt`, control characters, a trailing dot or space.
- [x] A Path not in Unicode NFC form is refused, checked with `unicode-normalization`.
- [x] Paths under the reserved `.tidings` Prefix are refused.
- [x] Prefixes are checked by the same rules.
- [x] Every operation that takes a Path or a Prefix returns `InvalidPath`: read, stat, list,
      Staging operations. Only `read` and `Staging::write` exist so far, and both are covered.
      The operations tickets 03 and 04 add get the same checks by taking `impl IntoPath` or
      `impl IntoPrefix`.
- [x] A table-driven test covers accepted and refused strings, including Unicode and Windows
      cases.

**Notes:**

- A Prefix is written and kept as either the empty string (the whole Area) or a valid Path
  followed by `/`, such as `themes/`. That is the glossary's "the leading part of a Path, up to a
  `/`", taken literally. The trailing `/` means a Prefix can't be mistaken for the Path of a
  File, and `path.starts_with(prefix)` then matches whole segments only (`themes/` doesn't cover
  `themes2/a`). `themes` without the `/` is refused with `NoTrailingSlash` rather than silently
  treated as `themes/`.
- `sanitize-filename` is supplemented. Neither 0.6 nor 0.7.0-beta refuses `COM¹`/`LPT¹` and the
  other superscript-digit ports, `CONIN$`, `CONOUT$`, a reserved name with spaces before its
  extension (`CON .txt`, which Windows reads as `CON`), or DEL (U+007F). ADR 0004 wants every
  name every platform accepts, so a small, commented supplement in `src/path.rs` covers exactly
  those gaps: a list of the missed device names, the crate's own check repeated on the name as
  Windows reads it, and `char::is_control`. Everything else is still the crate's.
- `.tidings` is refused as the first segment in any letter case, compared in upper case as NTFS
  does, so `.Tidings` and `.tidingſ` are refused too. Deeper segments named `.tidings` are
  allowed.
