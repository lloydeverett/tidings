//! Helpers for reading the Change feed, shared by every test crate. Each crate uses only some of
//! them.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use tidings::blocking::TimedOut;
use tidings::{Change, ChangeFeed, ChangeKind, FeedItem, Origin};

/// A Change feed the helpers can wait on: the async one, or the behaviour suite's stand-in, which
/// can also be the blocking one. tidings' own tests always have the `blocking` feature, so the
/// async one gives the blocking one's [`TimedOut`] too.
pub trait Feed {
    /// Waits up to `timeout` for the next item, giving what `next` would, or [`TimedOut`] if
    /// nothing arrived in time.
    fn next_within(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<Option<FeedItem>, TimedOut>>;
}

impl Feed for ChangeFeed {
    async fn next_within(&mut self, timeout: Duration) -> Result<Option<FeedItem>, TimedOut> {
        tokio::time::timeout(timeout, self.next()).await.map_err(|_| TimedOut)
    }
}

/// How long to wait for something the Change feed should send before failing the test. Long, so
/// that a test fails only when it never comes: on a busy machine the platform's watcher can take
/// seconds to deliver events, or to watch a directory again. A test that passes doesn't wait it
/// out.
const ARRIVAL: Duration = Duration::from_secs(60);

/// Waits for the next item on the Change feed, failing the test if none arrives within
/// [`ARRIVAL`].
pub async fn next_item(feed: &mut impl Feed) -> FeedItem {
    feed.next_within(ARRIVAL)
        .await
        .expect("the Change feed should have sent something by now")
        .expect("the Change feed should not have ended")
}

/// Waits for the next batch of Changes, sorted by Path so it can be compared.
pub async fn next_batch(feed: &mut impl Feed) -> Vec<Change> {
    match next_item(feed).await {
        FeedItem::Changes(mut batch) => {
            batch.sort_by(|a, b| a.path.cmp(&b.path));
            batch
        }
        other => panic!("expected a batch of Changes, got {other:?}"),
    }
}

/// Reads the Change feed until a batch holds a Change to `marker`, and gives every Change read,
/// merged as the feed merges unread Changes to a Path, sorted by Path. Events arrive in order, so
/// a Change still to come for what was done before `marker` was written comes before it or with
/// it: what this gives doesn't depend on how long the watcher takes. A Resync fails the test,
/// unless `skipping_resyncs`, when it is skipped.
pub async fn changes_until(
    feed: &mut impl Feed,
    marker: &str,
    skipping_resyncs: bool,
) -> Vec<Change> {
    let mut merged = BTreeMap::new();
    loop {
        let batch = match next_item(feed).await {
            FeedItem::Changes(batch) => batch,
            FeedItem::Resync if skipping_resyncs => continue,
            other => panic!("expected a batch of Changes, got {other:?}"),
        };
        let marked = batch.iter().any(|change| change.path.as_str() == marker);
        for change in batch {
            merged.insert(change.path.clone(), change);
        }
        if marked {
            return merged.into_values().collect();
        }
    }
}

/// Each Change's Path and kind, for comparing, where the Origin goes without saying.
pub fn changes(batch: &[Change]) -> Vec<(&str, ChangeKind)> {
    batch.iter().map(|change| (change.path.as_str(), change.kind)).collect()
}

/// Each Change's Path and Origin, for comparing, where the kind goes without saying.
pub fn paths_and_origins(batch: &[Change]) -> Vec<(&str, Origin)> {
    batch.iter().map(|change| (change.path.as_str(), change.origin)).collect()
}

/// Everything about each Change, for comparing.
pub fn changes_in_full(batch: &[Change]) -> Vec<(&str, ChangeKind, Origin)> {
    batch.iter().map(|change| (change.path.as_str(), change.kind, change.origin)).collect()
}

/// Checks that nothing more arrives on the Change feed for a short while.
pub async fn assert_nothing_more(feed: &mut impl Feed) {
    if let Ok(item) = feed.next_within(Duration::from_millis(200)).await {
        panic!("expected nothing more on the Change feed, got {item:?}");
    }
}

/// Checks that the Change feed has ended, and stays ended.
pub async fn assert_ended(feed: &mut impl Feed) {
    for _ in 0..2 {
        let item =
            feed.next_within(ARRIVAL).await.expect("the Change feed should have ended by now");
        assert_eq!(item, None);
    }
}
