//! What the Store layer does the same way on every Backend, tested once on memory rather than
//! through the behaviour suite: how it merges external Changes, which memory can only have
//! injected, and that a Store can be shared between threads.

use tidings::{Area, ChangeFeed, Staging, Store};

#[test]
fn a_store_and_its_futures_can_be_shared_between_threads() {
    fn clone_send_sync<T: Clone + Send + Sync>() {}
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>(_: T) {}
    clone_send_sync::<Store>();
    send_sync::<ChangeFeed>();

    // Every future a Store or its Change feed gives, without running it.
    let (store, mut feed) = Store::open_memory();
    send(store.read(Area::Config, "a"));
    send(store.stat(Area::Config, "a"));
    send(store.list(Area::Config, ""));
    send(store.stat_prefix(Area::Config, ""));
    send(store.commit(Staging::new(Area::Config)));
    send(feed.next());
}

/// Memory never observes external Changes, so they are injected into the Store layer.
#[cfg(feature = "testing")]
mod external {
    use std::time::Duration;

    use tidings::{Area, Change, ChangeFeed, ChangeKind, FeedItem, Origin, Staging, Store};

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
        store
            .inject_external_change(Area::Config, "external-first.toml", ChangeKind::Changed)
            .unwrap();
        commit(Area::Config, "external-first.toml", true).await;
        commit(Area::Config, "external-first.toml", false).await;
        // Local, then external.
        commit(Area::Config, "local-first.toml", true).await;
        store
            .inject_external_change(Area::Config, "local-first.toml", ChangeKind::Changed)
            .unwrap();
        // Only local, beside them.
        commit(Area::Config, "only-local.toml", true).await;
        // The same Path in another Area is merged apart.
        commit(Area::Data, "local-first.toml", true).await;

        let batch = next_batch(&mut feed).await;
        let seen: Vec<_> = batch
            .iter()
            .map(|change| (change.area, change.path.as_str(), change.kind, change.origin))
            .collect();
        assert_eq!(
            seen,
            [
                (Area::Config, "external-first.toml", ChangeKind::Removed, Origin::External),
                (Area::Config, "local-first.toml", ChangeKind::Changed, Origin::External),
                (Area::Config, "only-local.toml", ChangeKind::Changed, Origin::Local),
                (Area::Data, "local-first.toml", ChangeKind::Changed, Origin::Local),
            ],
        );

        // Once read, a Path starts again: a new local Change to it is local.
        commit(Area::Config, "local-first.toml", false).await;
        let batch = next_batch(&mut feed).await;
        let seen: Vec<_> =
            batch.iter().map(|change| (change.path.as_str(), change.kind, change.origin)).collect();
        assert_eq!(seen, [("local-first.toml", ChangeKind::Removed, Origin::Local)]);
        let nothing_more = tokio::time::timeout(Duration::from_millis(200), feed.next()).await;
        assert!(nothing_more.is_err(), "expected nothing more, got {nothing_more:?}");
    }

    /// Waits for the next batch of Changes, sorted by Area and Path so it can be compared.
    async fn next_batch(feed: &mut ChangeFeed) -> Vec<Change> {
        let item = tokio::time::timeout(Duration::from_secs(5), feed.next())
            .await
            .expect("the Change feed should have sent something by now")
            .expect("the Change feed should not have ended");
        match item {
            FeedItem::Changes(mut batch) => {
                batch.sort_by(|a, b| (a.area, &a.path).cmp(&(b.area, &b.path)));
                batch
            }
            other => panic!("expected a batch of Changes, got {other:?}"),
        }
    }
}
