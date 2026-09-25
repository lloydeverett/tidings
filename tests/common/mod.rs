//! Helpers for reading the Change feed, shared by every test crate. Each crate uses only some of
//! them.
#![allow(dead_code)]

use std::time::Duration;

use tidings::blocking::TimedOut;
use tidings::{Area, Change, ChangeFeed, ChangeKind, FeedItem, Origin};

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

/// Waits for the next item on the Change feed, failing the test if none arrives in time.
pub async fn next_item(feed: &mut impl Feed) -> FeedItem {
    feed.next_within(Duration::from_secs(5))
        .await
        .expect("the Change feed should have sent something by now")
        .expect("the Change feed should not have ended")
}

/// Waits for the next batch of Changes, sorted by Area and Path so it can be compared.
pub async fn next_batch(feed: &mut impl Feed) -> Vec<Change> {
    match next_item(feed).await {
        FeedItem::Changes(mut batch) => {
            batch.sort_by(|a, b| (a.area, &a.path).cmp(&(b.area, &b.path)));
            batch
        }
        other => panic!("expected a batch of Changes, got {other:?}"),
    }
}

/// Each Change's Path and kind, for comparing, where the Area and Origin go without saying.
pub fn changes(batch: &[Change]) -> Vec<(&str, ChangeKind)> {
    batch.iter().map(|change| (change.path.as_str(), change.kind)).collect()
}

/// Each Change's Path and Origin, for comparing, where the Area and kind go without saying.
pub fn paths_and_origins(batch: &[Change]) -> Vec<(&str, Origin)> {
    batch.iter().map(|change| (change.path.as_str(), change.origin)).collect()
}

/// Everything about each Change, for comparing.
pub fn changes_in_full(batch: &[Change]) -> Vec<(Area, &str, ChangeKind, Origin)> {
    batch
        .iter()
        .map(|change| (change.area, change.path.as_str(), change.kind, change.origin))
        .collect()
}

/// The App identity every test opens its Stores for, each on a Root override of its own.
#[cfg(any(feature = "fs", feature = "sqlite"))]
pub fn app() -> tidings::AppIdentity {
    tidings::AppIdentity::new("tidings tests", "tidings", "org")
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
        let item = feed
            .next_within(Duration::from_secs(5))
            .await
            .expect("the Change feed should have ended by now");
        assert_eq!(item, None);
    }
}
