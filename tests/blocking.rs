//! What only the blocking API does: it panics in an async task, its runtime runs between
//! calls, and it lives as long as its handles. The behaviour suite in tests/behaviour runs through
//! it too, on every Backend, for everything else.

mod common;

use std::thread;
use std::time::Duration;

use common::paths_and_origins;
use tidings::blocking::{ChangeFeed, Snapshot, Store};
use tidings::{Change, ChangeKind, FeedItem, Origin, Staging};

#[test]
fn a_blocking_store_and_what_it_gives_can_be_shared_between_threads() {
    fn clone_send_sync<T: Clone + Send + Sync>() {}
    fn send_sync<T: Send + Sync>() {}
    clone_send_sync::<Store>();
    send_sync::<ChangeFeed>();
    send_sync::<Snapshot>();
}

/// Every method that waits, called in an async task on a worker thread, panics and says why,
/// rather than stalling the runtime or deadlocking.
#[test]
fn waiting_in_an_async_task_panics_on_a_multi_threaded_runtime() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let opened = Handles::new();
    runtime.block_on(runtime.spawn(async move { every_wait_panics(opened) })).unwrap();
}

/// And in a current-thread runtime's `block_on`, which runs its async tasks.
#[test]
fn waiting_in_an_async_task_panics_on_a_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let opened = Handles::new();
    runtime.block_on(async move { every_wait_panics(opened) });
}

/// `spawn_blocking` is how async code calls blocking code, and tokio allows blocking there.
#[test]
fn the_blocking_api_works_in_spawn_blocking() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(runtime.spawn_blocking(every_wait_works)).unwrap();
}

/// So does `block_in_place`, which moves the runtime's other tasks off the thread first.
#[test]
fn the_blocking_api_works_in_block_in_place() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let in_place = runtime.spawn(async { tokio::task::block_in_place(every_wait_works) });
    runtime.block_on(in_place).unwrap();
}

/// Dropping is allowed anywhere, even the last handle, which stops the runtime.
#[test]
fn a_blocking_store_can_be_dropped_within_an_async_runtime() {
    let (store, feed) = Store::open_memory();
    let snapshot = store.snapshot().unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        drop(store);
        drop(feed);
        drop(snapshot);
    });
}

/// A synchronous app reads the Change feed on a plain thread, until it ends.
#[test]
fn the_change_feed_is_an_iterator_that_ends_once_every_store_handle_is_dropped() {
    let (store, feed) = Store::open_memory();
    let reader = thread::spawn(move || feed.flat_map(batch_of).collect::<Vec<_>>());

    for path in ["a.txt", "b.txt"] {
        let mut staging = Staging::new();
        staging.write(path, "x").unwrap();
        store.commit(staging).unwrap();
    }
    drop(store);

    let mut read = reader.join().unwrap();
    read.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(paths_and_origins(&read), [("a.txt", Origin::Local), ("b.txt", Origin::Local)]);
}

#[test]
fn an_injected_external_change_reaches_the_change_feed() {
    let (store, mut feed) = Store::open_memory();

    store.inject_external_change("a.toml", ChangeKind::Removed).unwrap();

    let item = feed.next_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let batch = batch_of(item);
    assert_eq!(batch.len(), 1);
    assert_eq!(
        (batch[0].path.as_str(), batch[0].kind, batch[0].origin),
        ("a.toml", ChangeKind::Removed, Origin::External),
    );
}

/// Another Store's Commits are followed while the app isn't calling this one: they are on the
/// Change feed before the app next looks, and it needn't wait at all.
#[cfg(feature = "sqlite")]
#[test]
fn another_stores_commits_arrive_while_the_app_is_not_calling_the_store() {
    let location = tempfile::tempdir().unwrap();
    let options = || tidings::SqliteOptions::default().poll_interval(Duration::from_millis(10));
    let (_store, mut feed) = Store::open_sqlite(location.path(), options()).unwrap();
    let (other, _other_feed) = Store::open_sqlite(location.path(), options()).unwrap();

    let mut staging = Staging::new();
    staging.write("elsewhere.txt", "x").unwrap();
    other.commit(staging).unwrap();
    thread::sleep(Duration::from_secs(1));

    let item = feed.next_timeout(Duration::ZERO).expect("it should have arrived by now");
    let batch = batch_of(item.unwrap());
    assert_eq!(paths_and_origins(&batch), [("elsewhere.txt", Origin::External)],);
}

/// Edits in the Location are watched for while the app isn't calling the Store: they are on the
/// Change feed before the app next looks, and it needn't wait at all.
#[cfg(feature = "fs")]
#[test]
fn edits_in_the_location_arrive_while_the_app_is_not_calling_the_store() {
    let location = tempfile::tempdir().unwrap();
    let options = tidings::FsOptions::default().debounce_window(Duration::from_millis(20));
    let (_store, mut feed) = Store::open_fs(location.path(), options).unwrap();

    std::fs::write(location.path().join("settings.toml"), "a = 1\n").unwrap();
    thread::sleep(Duration::from_secs(1));

    let item = feed.next_timeout(Duration::ZERO).expect("it should have arrived by now");
    let batch = batch_of(item.unwrap());
    assert_eq!(paths_and_origins(&batch), [("settings.toml", Origin::External)],);
}

/// A Commit in progress holds its Store handle, so the runtime keeps running until the Commit is
/// finished and reported, even if every other handle is dropped meanwhile. Then the feed ends.
#[cfg(feature = "fs")]
#[test]
fn a_commit_in_progress_finishes_and_is_reported_when_every_other_handle_is_dropped() {
    use tidings::{FailurePoint, Pause};

    let location = tempfile::tempdir().unwrap();
    let pause = Pause::new();
    let options =
        tidings::FsOptions::default().pause_at(FailurePoint::AfterCommittedJournal, &pause);
    let (store, mut feed) = Store::open_fs(location.path(), options).unwrap();

    let committing = {
        let store = store.clone();
        thread::spawn(move || {
            let mut staging = Staging::new();
            staging.write("in-progress.txt", "x").unwrap();
            store.commit(staging)
        })
    };
    let waiting = tokio::runtime::Builder::new_current_thread().build().unwrap();
    waiting.block_on(pause.reached());
    drop(store);
    pause.release();
    committing.join().unwrap().unwrap();

    let item = feed.next_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let batch = batch_of(item);
    assert_eq!(paths_and_origins(&batch), [("in-progress.txt", Origin::Local)],);
    assert_eq!(feed.next_timeout(Duration::from_secs(5)), Ok(None));
    let (store, _feed) = Store::open_fs(location.path(), tidings::FsOptions::default()).unwrap();
    assert_eq!(store.read("in-progress.txt").unwrap().unwrap().contents(), "x");
}

// Helpers shared by the tests above.

/// A Store opened through the blocking API, with a Snapshot and its Change feed, to call in
/// places where blocking isn't allowed.
struct Handles {
    /// Where a Store on the filesystem or SQLite would be opened.
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    directory: tempfile::TempDir,
    store: Store,
    snapshot: Snapshot,
    feed: ChangeFeed,
}

impl Handles {
    fn new() -> Handles {
        let (store, feed) = Store::open_memory();
        let snapshot = store.snapshot().unwrap();
        Handles {
            #[cfg(any(feature = "fs", feature = "sqlite"))]
            directory: tempfile::tempdir().unwrap(),
            store,
            snapshot,
            feed,
        }
    }
}

/// Checks that every method that waits panics where it is called. Those that don't wait needn't.
fn every_wait_panics(handles: Handles) {
    let Handles { store, snapshot, mut feed, .. } = handles;
    #[cfg(feature = "fs")]
    let (location, options) = (handles.directory.path().join("fs"), tidings::FsOptions::default());
    #[cfg(feature = "fs")]
    assert_panics_here("open_fs", || Store::open_fs(location, options));
    #[cfg(feature = "sqlite")]
    let (location, options) =
        (handles.directory.path().join("sqlite"), tidings::SqliteOptions::default());
    #[cfg(feature = "sqlite")]
    assert_panics_here("open_sqlite", || Store::open_sqlite(location, options));
    assert_panics_here("read", || store.read("a.txt"));
    assert_panics_here("stat", || store.stat("a.txt"));
    assert_panics_here("list", || store.list(""));
    assert_panics_here("stat_prefix", || store.stat_prefix(""));
    assert_panics_here("snapshot", || store.snapshot());
    assert_panics_here("commit", || store.commit(Staging::new()));
    assert_panics_here("reading a Snapshot", || snapshot.read("a.txt"));
    assert_panics_here("stat through a Snapshot", || snapshot.stat("a.txt"));
    assert_panics_here("listing a Snapshot", || snapshot.list(""));
    assert_panics_here("next", || feed.next());
    assert_panics_here("next_timeout", || feed.next_timeout(Duration::ZERO));

    assert!(store.supports_snapshots());
    store.inject_external_change("a.txt", ChangeKind::Changed).unwrap();
    drop(Store::open_memory());
}

/// Uses every method that waits, where it is called, and checks each gives what it should.
fn every_wait_works() {
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    let directory = tempfile::tempdir().unwrap();
    let mut opened = Vec::new();
    opened.push(Store::open_memory());
    #[cfg(feature = "fs")]
    let options = tidings::FsOptions::default();
    #[cfg(feature = "fs")]
    opened.push(Store::open_fs(directory.path().join("fs"), options).unwrap());
    #[cfg(feature = "sqlite")]
    let options = tidings::SqliteOptions::default();
    #[cfg(feature = "sqlite")]
    opened.push(Store::open_sqlite(directory.path().join("sqlite"), options).unwrap());

    for (store, mut feed) in opened {
        let mut staging = Staging::new();
        staging.write("a.txt", "a").unwrap();
        let committed = store.commit(staging).unwrap();
        let revision = committed.revisions().values().next().copied();
        assert_eq!(store.read("a.txt").unwrap().unwrap().contents(), "a");
        assert_eq!(store.stat("a.txt").unwrap().map(|stat| stat.revision()), revision);
        assert_eq!(store.list("").unwrap().len(), 1);
        store.stat_prefix("").unwrap();
        if store.supports_snapshots() {
            let snapshot = store.snapshot().unwrap();
            assert_eq!(snapshot.read("a.txt").unwrap().unwrap().contents(), "a");
            assert_eq!(snapshot.stat("a.txt").unwrap().map(|stat| stat.revision()), revision);
            assert_eq!(snapshot.list("").unwrap().len(), 1);
        }
        let item = feed.next_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(paths_and_origins(&batch_of(item)), [("a.txt", Origin::Local)]);
        drop(store);
        assert_eq!(feed.next(), None);
    }
}

/// Checks that `call` panics, saying it was called in an async task, and naming where it was
/// called: the line calling this, where `call` must be written.
#[track_caller]
fn assert_panics_here<T>(doing: &str, call: impl FnOnce() -> T) {
    let here = std::panic::Location::caller();
    let panicked = match panic_location::catch(call) {
        Ok(_) => panic!("{doing} should panic in an async task"),
        Err(panicked) => panicked,
    };
    let message = match (
        panicked.payload.downcast_ref::<&str>(),
        panicked.payload.downcast_ref::<String>(),
    ) {
        (Some(message), _) => message.to_string(),
        (_, Some(message)) => message.clone(),
        _ => panic!("{doing} should panic with a message"),
    };
    assert!(message.contains("called in an async task"), "{doing}: {message}");
    assert_eq!(panicked.at, (here.file().to_owned(), here.line()), "{doing}: where it panicked");
}

/// Catching a panic together with where it happened, which its payload doesn't say.
mod panic_location {
    use std::any::Any;
    use std::cell::RefCell;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::Once;

    /// A panic that was caught.
    pub struct Caught {
        pub payload: Box<dyn Any + Send>,
        /// The file and line it happened at.
        pub at: (String, u32),
    }

    thread_local! {
        /// Where the last panic on this thread happened. Each thread has its own, so tests
        /// running at once don't mix theirs up.
        static LAST: RefCell<Option<(String, u32)>> = const { RefCell::new(None) };
    }

    /// Makes `call`, and gives what it gave, or its panic and where that happened. The last
    /// panic on the thread is the one caught: tokio's refusal to block comes first, then the
    /// Store's.
    pub fn catch<T>(call: impl FnOnce() -> T) -> Result<T, Caught> {
        static HOOK: Once = Once::new();
        HOOK.call_once(|| {
            let printing = panic::take_hook();
            panic::set_hook(Box::new(move |info| {
                let at = info.location().map(|at| (at.file().to_owned(), at.line()));
                LAST.with(|last| *last.borrow_mut() = at);
                printing(info);
            }));
        });
        panic::catch_unwind(AssertUnwindSafe(call)).map_err(|payload| Caught {
            payload,
            at: LAST.with(|last| last.borrow_mut().take()).unwrap_or_default(),
        })
    }
}

/// The Changes in `item`, which must be a batch.
fn batch_of(item: FeedItem) -> Vec<Change> {
    match item {
        FeedItem::Changes(batch) => batch,
        other => panic!("expected a batch of Changes, got {other:?}"),
    }
}
