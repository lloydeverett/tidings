//! What only the blocking API does: it panics in an async task, its runtime runs between
//! calls, and it lives as long as its handles. The behaviour suite in tests/behaviour runs through
//! it too, on every Backend, for everything else.

mod common;

use std::panic::{self, AssertUnwindSafe};
use std::thread;
use std::time::Duration;

#[cfg(any(feature = "fs", feature = "sqlite"))]
use common::app;
use common::paths_and_origins;
use tidings::blocking::{ChangeFeed, Snapshot, Store};
use tidings::{Area, Change, ChangeKind, FeedItem, Origin, Staging};

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
    let opened = Opened::new();
    runtime.block_on(runtime.spawn(async move { every_wait_panics(opened) })).unwrap();
}

/// And in a current-thread runtime's `block_on`, which runs its async tasks.
#[test]
fn waiting_in_an_async_task_panics_on_a_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let opened = Opened::new();
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
    let snapshot = store.snapshot(Area::Data).unwrap();

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
        let mut staging = Staging::new(Area::Data);
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

    store.inject_external_change(Area::Config, "a.toml", ChangeKind::Removed).unwrap();

    let item = feed.next_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let batch = batch_of(item);
    assert_eq!(batch.len(), 1);
    assert_eq!(
        (batch[0].area, batch[0].path.as_str(), batch[0].kind, batch[0].origin),
        (Area::Config, "a.toml", ChangeKind::Removed, Origin::External),
    );
}

/// Another Store's Commits are followed while the app isn't calling this one: they are on the
/// Change feed before the app next looks, and it needn't wait at all.
#[cfg(feature = "sqlite")]
#[test]
fn another_stores_commits_arrive_while_the_app_is_not_calling_the_store() {
    let root = tempfile::tempdir().unwrap();
    let options = || {
        tidings::SqliteOptions::default()
            .root_override(root.path())
            .poll_interval(Duration::from_millis(10))
    };
    let (_store, mut feed) = Store::open_sqlite(&app(), options()).unwrap();
    let (other, _other_feed) = Store::open_sqlite(&app(), options()).unwrap();

    let mut staging = Staging::new(Area::Data);
    staging.write("elsewhere.txt", "x").unwrap();
    other.commit(staging).unwrap();
    thread::sleep(Duration::from_secs(1));

    let item = feed.next_timeout(Duration::ZERO).expect("it should have arrived by now");
    let batch = batch_of(item.unwrap());
    assert_eq!(paths_and_origins(&batch), [("elsewhere.txt", Origin::External)],);
}

/// Edits in an Area's directory are watched for while the app isn't calling the Store: they are
/// on the Change feed before the app next looks, and it needn't wait at all.
#[cfg(feature = "fs")]
#[test]
fn edits_in_an_area_arrive_while_the_app_is_not_calling_the_store() {
    let root = tempfile::tempdir().unwrap();
    let options = tidings::FsOptions::default()
        .root_override(root.path())
        .debounce_window(Duration::from_millis(20));
    let (_store, mut feed) = Store::open_fs(&app(), options).unwrap();

    std::fs::write(root.path().join("config").join("settings.toml"), "a = 1\n").unwrap();
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

    let root = tempfile::tempdir().unwrap();
    let pause = Pause::new();
    let options = tidings::FsOptions::default()
        .root_override(root.path())
        .pause_at(FailurePoint::AfterCommittedJournal, &pause);
    let (store, mut feed) = Store::open_fs(&app(), options).unwrap();

    let committing = {
        let store = store.clone();
        thread::spawn(move || {
            let mut staging = Staging::new(Area::Data);
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
    let (store, _feed) =
        Store::open_fs(&app(), tidings::FsOptions::default().root_override(root.path())).unwrap();
    assert_eq!(store.read(Area::Data, "in-progress.txt").unwrap().unwrap().contents(), "x");
}

// Helpers shared by the tests above.

/// A Store opened through the blocking API, with a Snapshot and its Change feed, to call in
/// places where blocking isn't allowed.
struct Opened {
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    root: tempfile::TempDir,
    store: Store,
    snapshot: Snapshot,
    feed: ChangeFeed,
}

impl Opened {
    fn new() -> Opened {
        let (store, feed) = Store::open_memory();
        let snapshot = store.snapshot(Area::Data).unwrap();
        Opened {
            #[cfg(any(feature = "fs", feature = "sqlite"))]
            root: tempfile::tempdir().unwrap(),
            store,
            snapshot,
            feed,
        }
    }
}

/// Checks that every method that waits panics where it is called. Those that don't wait needn't.
fn every_wait_panics(opened: Opened) {
    let Opened { store, snapshot, mut feed, .. } = opened;
    #[cfg(feature = "fs")]
    assert_panics_here("open_fs", || {
        Store::open_fs(&app(), tidings::FsOptions::default().root_override(opened.root.path()))
    });
    #[cfg(feature = "sqlite")]
    assert_panics_here("open_sqlite", || {
        let options = tidings::SqliteOptions::default().root_override(opened.root.path());
        Store::open_sqlite(&app(), options)
    });
    assert_panics_here("read", || store.read(Area::Data, "a.txt"));
    assert_panics_here("stat", || store.stat(Area::Data, "a.txt"));
    assert_panics_here("list", || store.list(Area::Data, ""));
    assert_panics_here("stat_prefix", || store.stat_prefix(Area::Data, ""));
    assert_panics_here("snapshot", || store.snapshot(Area::Data));
    assert_panics_here("commit", || store.commit(Staging::new(Area::Data)));
    assert_panics_here("reading a Snapshot", || snapshot.read("a.txt"));
    assert_panics_here("stat through a Snapshot", || snapshot.stat("a.txt"));
    assert_panics_here("listing a Snapshot", || snapshot.list(""));
    assert_panics_here("next", || feed.next());
    assert_panics_here("next_timeout", || feed.next_timeout(Duration::ZERO));

    assert!(store.supports_snapshots());
    store.inject_external_change(Area::Data, "a.txt", ChangeKind::Changed).unwrap();
    drop(Store::open_memory());
}

/// Uses every method that waits, where it is called, and checks each gives what it should.
fn every_wait_works() {
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    let root = tempfile::tempdir().unwrap();
    let mut opened = Vec::new();
    opened.push(Store::open_memory());
    #[cfg(feature = "fs")]
    opened.push(
        Store::open_fs(&app(), tidings::FsOptions::default().root_override(root.path().join("fs")))
            .unwrap(),
    );
    #[cfg(feature = "sqlite")]
    opened.push(
        Store::open_sqlite(
            &app(),
            tidings::SqliteOptions::default().root_override(root.path().join("sqlite")),
        )
        .unwrap(),
    );

    for (store, mut feed) in opened {
        let mut staging = Staging::new(Area::Data);
        staging.write("a.txt", "a").unwrap();
        let committed = store.commit(staging).unwrap();
        let revision = committed.revisions().values().next().copied();
        assert_eq!(store.read(Area::Data, "a.txt").unwrap().unwrap().contents(), "a");
        assert_eq!(store.stat(Area::Data, "a.txt").unwrap().map(|stat| stat.revision()), revision);
        assert_eq!(store.list(Area::Data, "").unwrap().len(), 1);
        store.stat_prefix(Area::Data, "").unwrap();
        if store.supports_snapshots() {
            let snapshot = store.snapshot(Area::Data).unwrap();
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

/// Checks that `call` panics where it is made, saying it was called in an async task.
#[track_caller]
fn assert_panics_here<T>(doing: &str, call: impl FnOnce() -> T) {
    let panicked = match panic::catch_unwind(AssertUnwindSafe(call)) {
        Ok(_) => panic!("{doing} should panic in an async task"),
        Err(panicked) => panicked,
    };
    let message = match (panicked.downcast_ref::<&str>(), panicked.downcast_ref::<String>()) {
        (Some(message), _) => message.to_string(),
        (_, Some(message)) => message.clone(),
        _ => panic!("{doing} should panic with a message"),
    };
    assert!(message.contains("called in an async task"), "{doing}: {message}");
}

/// The Changes in `item`, which must be a batch.
fn batch_of(item: FeedItem) -> Vec<Change> {
    match item {
        FeedItem::Changes(batch) => batch,
        other => panic!("expected a batch of Changes, got {other:?}"),
    }
}
