//! Helpers for reading the Change feed, shared by every test crate. Each crate uses only some of
//! them.
#![allow(dead_code)]

use std::time::Duration;

use tidings::{Area, Change, ChangeFeed, ChangeKind, FeedItem, Origin};

/// Waits for the next item on the Change feed, failing the test if none arrives in time.
pub async fn next_item(feed: &mut ChangeFeed) -> FeedItem {
    tokio::time::timeout(Duration::from_secs(5), feed.next())
        .await
        .expect("the Change feed should have sent something by now")
        .expect("the Change feed should not have ended")
}

/// Waits for the next batch of Changes, sorted by Area and Path so it can be compared.
pub async fn next_batch(feed: &mut ChangeFeed) -> Vec<Change> {
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

/// Everything about each Change, for comparing.
pub fn changes_in_full(batch: &[Change]) -> Vec<(Area, &str, ChangeKind, Origin)> {
    batch
        .iter()
        .map(|change| (change.area, change.path.as_str(), change.kind, change.origin))
        .collect()
}

/// Checks that nothing more arrives on the Change feed for a short while.
pub async fn assert_nothing_more(feed: &mut ChangeFeed) {
    if let Ok(item) = tokio::time::timeout(Duration::from_millis(200), feed.next()).await {
        panic!("expected nothing more on the Change feed, got {item:?}");
    }
}

/// Checks that the Change feed has ended, and stays ended.
pub async fn assert_ended(feed: &mut ChangeFeed) {
    for _ in 0..2 {
        let item = tokio::time::timeout(Duration::from_secs(5), feed.next())
            .await
            .expect("the Change feed should have ended by now");
        assert_eq!(item, None);
    }
}
