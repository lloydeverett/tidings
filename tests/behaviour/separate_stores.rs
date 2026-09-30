//! Tests of two Stores at different Locations, which have nothing to do with each other: each has
//! its own Files, Change feed and Resyncs, whether they are on the same Backend or on the
//! filesystem and SQLite together. Each test takes a Fixture for each Store, and is listed in
//! [`separate_stores_suite!`].

use std::path::{Path as FsPath, PathBuf};

use tidings::{ChangeKind, Error, FeedItem, Origin, Staging};

use crate::common::{assert_nothing_more, changes_in_full, changes_until, next_batch, next_item};
use crate::suite::{Fixture, Opened};

/// A Fixture whose Stores are at a Location on disk, which a test can remove.
pub trait Located: Fixture {
    fn location(&self) -> PathBuf;
}

/// Removes the Location, as the OS does when it clears a cache. It is moved away first, so that
/// the Store never sees it half removed.
pub fn remove_the_location(location: &FsPath) {
    let moved = location.with_extension("removed");
    std::fs::rename(location, &moved).unwrap();
    std::fs::remove_dir_all(&moved).unwrap();
}

/// Instantiates every test here for two Fixtures, each opening its Stores at a Location of its
/// own.
macro_rules! separate_stores_suite {
    ($first:expr, $second:expr) => {
        separate_stores_suite!(@tests $first, $second;
            stores_at_different_locations_have_their_own_files_and_feeds,
            a_prefix_revision_from_one_store_is_refused_by_a_commit_to_another,
            removing_one_stores_location_resyncs_only_that_store,
        );
    };
    (@tests $first:expr, $second:expr; $($test:ident),* $(,)?) => {
        mod separate_stores {
            use super::*;
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $test() {
                    $crate::separate_stores::$test(&$first, &$second).await;
                }
            )*
        }
    };
}

/// The same Path in two Stores is two Files, and each Store's Change feed tells only of its own
/// Commits.
pub async fn stores_at_different_locations_have_their_own_files_and_feeds(
    first: &impl Located,
    second: &impl Located,
) {
    let Opened { store: one, feed: mut one_feed } = first.open().await;
    let Opened { store: other, feed: mut other_feed } = second.open().await;

    let mut staging = Staging::new();
    staging.write("settings.toml", "one").unwrap();
    one.commit(staging).await.unwrap();
    assert_eq!(other.read("settings.toml").await.unwrap(), None);
    assert!(other.list("").await.unwrap().is_empty());
    let mut staging = Staging::new();
    staging.write("settings.toml", "other").unwrap();
    other.commit(staging).await.unwrap();

    assert_eq!(one.read("settings.toml").await.unwrap().unwrap().contents(), "one");
    assert_eq!(other.read("settings.toml").await.unwrap().unwrap().contents(), "other");
    for feed in [&mut one_feed, &mut other_feed] {
        assert_eq!(
            changes_in_full(&next_batch(feed).await),
            [("settings.toml", ChangeKind::Changed, Origin::Local)],
        );
        assert_nothing_more(feed).await;
    }
}

/// A Prefix Revision belongs to the Store that took it, so a Commit to another Store refuses it,
/// even for a Prefix whose Files are the same in both.
pub async fn a_prefix_revision_from_one_store_is_refused_by_a_commit_to_another(
    first: &impl Located,
    second: &impl Located,
) {
    let Opened { store: one, feed: _one_feed } = first.open().await;
    let Opened { store: other, feed: _other_feed } = second.open().await;
    let themes = one.stat_prefix("themes/").await.unwrap();

    let mut staging = Staging::new();
    staging.require_prefix("themes/", themes).unwrap();
    staging.write("themes/dark.toml", "dark").unwrap();
    match other.commit(staging).await {
        Err(Error::WrongPrefixRevision { prefix }) => assert_eq!(prefix.as_str(), "themes/"),
        refused => panic!("expected WrongPrefixRevision, got {refused:?}"),
    }
    assert_eq!(other.read("themes/dark.toml").await.unwrap(), None);
}

/// Removing one Store's Location, as the OS clears a cache, gives that Store a Resync and nothing
/// to the other, which keeps its Files. Its Files can be reported removed before the Resync.
pub async fn removing_one_stores_location_resyncs_only_that_store(
    first: &impl Located,
    second: &impl Located,
) {
    let Opened { store: one, feed: mut one_feed } = first.open().await;
    let Opened { store: other, feed: mut other_feed } = second.open().await;
    for store in [&one, &other] {
        let mut staging = Staging::new();
        staging.write("kept.txt", "kept").unwrap();
        store.commit(staging).await.unwrap();
    }
    next_batch(&mut one_feed).await;
    next_batch(&mut other_feed).await;

    remove_the_location(&first.location());
    // The filesystem's watcher may look at the Commit's own events only once the Location is gone,
    // before it has the Location's, and so rightly report the File removed before the Resync.
    loop {
        match next_item(&mut one_feed).await {
            FeedItem::Resync => break,
            FeedItem::Changes(batch) => assert_eq!(
                changes_in_full(&batch),
                [("kept.txt", ChangeKind::Removed, Origin::External)],
            ),
        }
    }
    // Committed to again once made again. The filesystem's watcher can report the removal again,
    // late: see the README's Consistency section.
    let mut staging = Staging::new();
    staging.write("marker.txt", "").unwrap();
    one.commit(staging).await.unwrap();
    let read = changes_until(&mut one_feed, "marker.txt", true).await;
    assert_eq!(changes_in_full(&read), [("marker.txt", ChangeKind::Changed, Origin::Local)]);

    assert_nothing_more(&mut other_feed).await;
    assert_eq!(other.read("kept.txt").await.unwrap().unwrap().contents(), "kept");
    assert_eq!(one.read("kept.txt").await.unwrap(), None);
}
