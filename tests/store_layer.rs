//! What the Store layer does the same way on every Backend, tested once on memory rather than
//! through the behaviour suite: how it merges external Changes, which memory can only have
//! injected, and that a Store and its Snapshots can be shared between threads.

mod common;

use common::{assert_nothing_more, changes_in_full, next_batch};
use tidings::{ChangeFeed, ChangeKind, Origin, Snapshot, Staging, Store};

#[test]
fn a_store_and_its_futures_can_be_shared_between_threads() {
    fn clone_send_sync<T: Clone + Send + Sync>() {}
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>(_: T) {}
    clone_send_sync::<Store>();
    send_sync::<ChangeFeed>();
    send_sync::<Snapshot>();

    // Every future a Store or its Change feed gives, without running it.
    let (store, mut feed) = Store::open_memory();
    send(store.read("a"));
    send(store.stat("a"));
    send(store.list(""));
    send(store.stat_prefix(""));
    send(store.commit(Staging::new()));
    send(store.snapshot());
    send(feed.next());
    #[cfg(feature = "fs")]
    {
        send(Store::open_fs("location", tidings::FsOptions::default()));
    }
    #[cfg(feature = "sqlite")]
    {
        send(Store::open_sqlite("location", tidings::SqliteOptions::default()));
    }
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    {
        send(Store::detect("location"));
    }
}

/// And every future a Snapshot gives.
#[tokio::test]
async fn a_snapshots_futures_can_be_sent_between_threads() {
    fn send<T: Send>(_: T) {}
    let (store, _feed) = Store::open_memory();
    let snapshot = store.snapshot().await.unwrap();
    send(snapshot.read("a"));
    send(snapshot.stat("a"));
    send(snapshot.list(""));
}

/// Memory never observes external Changes, so they are injected into the Store layer, with the
/// `testing` feature that tidings' own tests always have.
#[tokio::test]
async fn a_merged_change_is_external_if_any_change_merged_into_it_was() {
    let (store, mut feed) = Store::open_memory();
    let commit = async |path: &str, write: bool| {
        let mut staging = Staging::new();
        if write {
            staging.write(path, "local").unwrap();
        } else {
            staging.delete(path).unwrap();
        }
        store.commit(staging).await.unwrap();
    };

    // External, then local: the local kind is the latest, but it stays external.
    store.inject_external_change("external-first.toml", ChangeKind::Changed).unwrap();
    commit("external-first.toml", true).await;
    commit("external-first.toml", false).await;
    // Local, then external.
    commit("local-first.toml", true).await;
    store.inject_external_change("local-first.toml", ChangeKind::Changed).unwrap();
    // Only local, beside them.
    commit("only-local.toml", true).await;

    assert_eq!(
        changes_in_full(&next_batch(&mut feed).await),
        [
            ("external-first.toml", ChangeKind::Removed, Origin::External),
            ("local-first.toml", ChangeKind::Changed, Origin::External),
            ("only-local.toml", ChangeKind::Changed, Origin::Local),
        ],
    );

    // Once read, a Path starts again: a new local Change to it is local.
    commit("local-first.toml", false).await;
    assert_eq!(
        changes_in_full(&next_batch(&mut feed).await),
        [("local-first.toml", ChangeKind::Removed, Origin::Local)],
    );
    assert_nothing_more(&mut feed).await;
}
