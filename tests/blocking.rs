//! What only the blocking API does: it panics inside an async runtime, its runtime runs between
//! calls, and it lives as long as its handles. The behaviour suite in tests/behaviour runs through
//! it too, on every Backend, for everything else.

use std::panic::{self, AssertUnwindSafe};
use std::thread;
use std::time::Duration;

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

/// Every method, called from within an async runtime, panics and says why, rather than blocking
/// the runtime or deadlocking.
#[test]
fn calling_the_blocking_api_from_within_an_async_runtime_panics() {
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    let root = tempfile::tempdir().unwrap();
    let (store, mut feed) = Store::open_memory();
    let snapshot = store.snapshot(Area::Data).unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _inside = runtime.enter();
    assert_panics_in_a_runtime("open_memory", Store::open_memory);
    #[cfg(feature = "fs")]
    assert_panics_in_a_runtime("open_fs", || {
        Store::open_fs(&app(), tidings::FsOptions::default().root_override(root.path()))
    });
    #[cfg(feature = "sqlite")]
    assert_panics_in_a_runtime("open_sqlite", || {
        Store::open_sqlite(&app(), tidings::SqliteOptions::default().root_override(root.path()))
    });
    assert_panics_in_a_runtime("read", || store.read(Area::Data, "a.txt"));
    assert_panics_in_a_runtime("stat", || store.stat(Area::Data, "a.txt"));
    assert_panics_in_a_runtime("list", || store.list(Area::Data, ""));
    assert_panics_in_a_runtime("stat_prefix", || store.stat_prefix(Area::Data, ""));
    assert_panics_in_a_runtime("supports_snapshots", || store.supports_snapshots());
    assert_panics_in_a_runtime("snapshot", || store.snapshot(Area::Data));
    assert_panics_in_a_runtime("commit", || store.commit(Staging::new(Area::Data)));
    assert_panics_in_a_runtime("inject_external_change", || {
        store.inject_external_change(Area::Data, "a.txt", ChangeKind::Changed)
    });
    assert_panics_in_a_runtime("reading a Snapshot", || snapshot.read("a.txt"));
    assert_panics_in_a_runtime("stat through a Snapshot", || snapshot.stat("a.txt"));
    assert_panics_in_a_runtime("listing a Snapshot", || snapshot.list(""));
    assert_panics_in_a_runtime("next", || feed.next());
    assert_panics_in_a_runtime("next_timeout", || feed.next_timeout(Duration::ZERO));
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
    let reader = thread::spawn(move || feed.flat_map(changes).collect::<Vec<_>>());

    for path in ["a.txt", "b.txt"] {
        let mut staging = Staging::new(Area::Data);
        staging.write(path, "x").unwrap();
        store.commit(staging).unwrap();
    }
    drop(store);

    let mut read = reader.join().unwrap();
    read.sort_by(|a, b| a.path.cmp(&b.path));
    let read: Vec<_> = read.iter().map(|change| (change.path.as_str(), change.origin)).collect();
    assert_eq!(read, [("a.txt", Origin::Local), ("b.txt", Origin::Local)]);
}

#[test]
fn an_injected_external_change_reaches_the_change_feed() {
    let (store, mut feed) = Store::open_memory();

    store.inject_external_change(Area::Config, "a.toml", ChangeKind::Removed).unwrap();

    let item = feed.next_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let batch = changes(item);
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
    let batch = changes(item.unwrap());
    assert_eq!(
        batch.iter().map(|change| (change.path.as_str(), change.origin)).collect::<Vec<_>>(),
        [("elsewhere.txt", Origin::External)],
    );
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
    let batch = changes(item.unwrap());
    assert_eq!(
        batch.iter().map(|change| (change.path.as_str(), change.origin)).collect::<Vec<_>>(),
        [("settings.toml", Origin::External)],
    );
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
    let batch = changes(item);
    assert_eq!(
        batch.iter().map(|change| (change.path.as_str(), change.origin)).collect::<Vec<_>>(),
        [("in-progress.txt", Origin::Local)],
    );
    assert_eq!(feed.next_timeout(Duration::from_secs(5)), Ok(None));
    let (store, _feed) =
        Store::open_fs(&app(), tidings::FsOptions::default().root_override(root.path())).unwrap();
    assert_eq!(store.read(Area::Data, "in-progress.txt").unwrap().unwrap().contents(), "x");
}

// Helpers shared by the tests above.

/// Checks that `call`, made within an async runtime, panics, saying so.
#[track_caller]
fn assert_panics_in_a_runtime<T>(doing: &str, call: impl FnOnce() -> T) {
    let panicked = match panic::catch_unwind(AssertUnwindSafe(call)) {
        Ok(_) => panic!("{doing} should panic within an async runtime"),
        Err(panicked) => panicked,
    };
    let message = match (panicked.downcast_ref::<&str>(), panicked.downcast_ref::<String>()) {
        (Some(message), _) => message.to_string(),
        (_, Some(message)) => message.clone(),
        _ => panic!("{doing} should panic with a message"),
    };
    assert!(message.contains("called from within an async runtime"), "{doing}: {message}");
}

/// The Changes in `item`, which must be a batch.
fn changes(item: FeedItem) -> Vec<Change> {
    match item {
        FeedItem::Changes(batch) => batch,
        other => panic!("expected a batch of Changes, got {other:?}"),
    }
}

#[cfg(any(feature = "fs", feature = "sqlite"))]
fn app() -> tidings::AppIdentity {
    tidings::AppIdentity::new("tidings tests", "tidings", "org")
}
