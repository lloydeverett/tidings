//! What the Store layer does the same way on every Backend, tested once on memory rather than
//! through the behaviour suite: how it merges external Changes, which memory can only have
//! injected, and that a Store and its Snapshots can be shared between threads.

mod common;

use common::{assert_nothing_more, changes_in_full, next_batch};
use tidings::{Area, ChangeFeed, ChangeKind, Origin, Snapshot, Staging, Store};

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
    send(store.read(Area::Config, "a"));
    send(store.stat(Area::Config, "a"));
    send(store.list(Area::Config, ""));
    send(store.stat_prefix(Area::Config, ""));
    send(store.commit(Staging::new(Area::Config)));
    send(store.snapshot(Area::Config));
    send(feed.next());
    #[cfg(feature = "sqlite")]
    {
        let app = tidings::AppIdentity::new("tidings tests", "tidings", "org");
        send(Store::open_sqlite(&app, tidings::SqliteOptions::default()));
    }
}

/// And every future a Snapshot gives.
#[tokio::test]
async fn a_snapshots_futures_can_be_sent_between_threads() {
    fn send<T: Send>(_: T) {}
    let (store, _feed) = Store::open_memory();
    let snapshot = store.snapshot(Area::Config).await.unwrap();
    send(snapshot.read("a"));
    send(snapshot.stat("a"));
    send(snapshot.list(""));
}

/// Memory never observes external Changes, so they are injected into the Store layer, with the
/// `testing` feature that tidings' own tests always have.
#[tokio::test]
async fn a_merged_change_is_external_if_any_change_merged_into_it_was() {
    let (store, mut feed) = Store::open_memory();
    let commit = async |area: Area, path: &str, write: bool| {
        let mut staging = Staging::new(area);
        if write {
            staging.write(path, "local").unwrap();
        } else {
            staging.delete(path).unwrap();
        }
        store.commit(staging).await.unwrap();
    };

    // External, then local: the local kind is the latest, but it stays external.
    store.inject_external_change(Area::Config, "external-first.toml", ChangeKind::Changed).unwrap();
    commit(Area::Config, "external-first.toml", true).await;
    commit(Area::Config, "external-first.toml", false).await;
    // Local, then external.
    commit(Area::Config, "local-first.toml", true).await;
    store.inject_external_change(Area::Config, "local-first.toml", ChangeKind::Changed).unwrap();
    // Only local, beside them.
    commit(Area::Config, "only-local.toml", true).await;
    // The same Path in another Area is merged apart.
    commit(Area::Data, "local-first.toml", true).await;

    assert_eq!(
        changes_in_full(&next_batch(&mut feed).await),
        [
            (Area::Config, "external-first.toml", ChangeKind::Removed, Origin::External),
            (Area::Config, "local-first.toml", ChangeKind::Changed, Origin::External),
            (Area::Config, "only-local.toml", ChangeKind::Changed, Origin::Local),
            (Area::Data, "local-first.toml", ChangeKind::Changed, Origin::Local),
        ],
    );

    // Once read, a Path starts again: a new local Change to it is local.
    commit(Area::Config, "local-first.toml", false).await;
    assert_eq!(
        changes_in_full(&next_batch(&mut feed).await),
        [(Area::Config, "local-first.toml", ChangeKind::Removed, Origin::Local)],
    );
    assert_nothing_more(&mut feed).await;
}
