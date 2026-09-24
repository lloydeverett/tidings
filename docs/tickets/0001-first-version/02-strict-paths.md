# 02: Strict Paths

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0004](../../adr/0004-strict-portable-paths-on-every-backend.md)

**What to build:** Every Path and Prefix an app gives tidings is checked against the rules of the
strictest platform, the same way on every Backend. A Path that isn't allowed gives `InvalidPath`
straight away, wherever it is used, so a Path that works on one platform works on all of them.
Established crates do the checking; we don't write the rules ourselves.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] Structure is checked with `relative-path`. Refused: absolute Paths, empty segments, `.`,
      `..`, and a leading `/`.
- [ ] Each segment is checked with `sanitize-filename` using its Windows rules. Refused, for
      example: `CON`, `aux.txt`, control characters, a trailing dot or space.
- [ ] A Path not in Unicode NFC form is refused, checked with `unicode-normalization`.
- [ ] Paths under the reserved `.tidings` Prefix are refused.
- [ ] Prefixes are checked by the same rules.
- [ ] Every operation that takes a Path or a Prefix returns `InvalidPath`: read, stat, list,
      Staging operations.
- [ ] A table-driven test covers accepted and refused strings, including Unicode and Windows
      cases.
