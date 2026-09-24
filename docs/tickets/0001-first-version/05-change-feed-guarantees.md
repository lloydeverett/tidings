# 05: What the Change feed promises

Spec: [0001](../../specs/0001-first-version.md) · ADR: [0003](../../adr/0003-the-change-feed-comes-from-open.md)

**What to build:** The Change feed keeps its promises however the app uses it:
- An app that reads slowly never loses a Path, and memory never grows without limit.
- An app that skips its own Changes never misses anyone else's.
- An app that shares the Store between tasks keeps the feed alive until the last handle goes.

These are properties of the Store layer, tested on the memory Backend.

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] Changes not yet read are merged per (Area, Path): the latest kind wins. Memory grows with
      the number of distinct Paths, not with the number of Commits.
- [ ] A merged Change's Origin is *external* if any of the Changes merged into it was.
- [ ] A Commit's Changes are never split across batches. A batch may contain several Commits'
      Changes.
- [ ] `Store` is `Clone + Send + Sync`, and clones share state and the one Change feed.
- [ ] Once every Store handle has been dropped, the Change feed ends (`next` returns nothing).
- [ ] Dropping the Change feed leaves the Store fully working, and Changes stop being recorded.
- [ ] Every Commit made after `open` returns produces a Change, including a Commit made before
      the app first polls the feed.
- [ ] The rule for merged external Changes is tested on memory. Memory never produces external
      Changes itself, so the `testing` feature adds a way to inject one into the Store layer.
