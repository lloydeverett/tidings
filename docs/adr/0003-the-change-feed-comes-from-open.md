---
status: accepted
---

# The Change feed is returned by `open`, and there is only one

`Store::open` returns `(Store, ChangeFeed)`. There is no `subscribe()` to call later, and the feed
cannot be cloned. With a separate subscribe call, Changes made between opening and subscribing
would be lost. Handing the feed over together with the Store guarantees that every change after
`open` returns is reported. An application that wants several listeners passes the Changes on
itself. If the feed is dropped, the Store keeps working and stops recording Changes.

Changes that have not been read yet are merged per Path rather than queued or dropped, so an
application that is slow to read never misses a Path and memory stays bounded. All the Changes
from one local Commit arrive in the same batch.

## Considered options

- **A callback required by `open`**: this would force the application to handle Changes, but it
  would run the application's code on the Store's own watcher task. The application would also
  have to put its state into a `'static` closure. The returned feed is the usual tokio pattern (as
  `mpsc::channel()` returns both ends at once) and prevents the same race.
- **A `broadcast` channel**: it drops messages when a receiver falls behind, which is exactly the
  case we are trying to rule out.
