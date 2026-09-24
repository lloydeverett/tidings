//! Tests for Backends where a second Store can be opened on the same storage, standing in for
//! another process: SQLite and the filesystem. Each test calls its Fixture's `open` twice, and
//! both Stores share the Fixture's Root override. Each is listed in [`two_stores_suite!`].

use std::collections::BTreeMap;
use std::time::Duration;

use tidings::{Area, ChangeFeed, ChangeKind, Error, FeedItem, Origin, Staging, Store};

use crate::common::{assert_nothing_more, changes_in_full, next_batch};
use crate::suite::{Fixture, Opened};

/// Instantiates every test here for one Backend's [`Fixture`].
macro_rules! two_stores_suite {
    ($fixture:expr) => {
        two_stores_suite!(@tests $fixture;
            another_stores_commit_arrives_as_one_batch_of_external_changes,
            commits_made_before_a_store_is_opened_are_not_reported_to_it,
            another_stores_commits_are_never_split_across_batches,
            commits_from_both_stores_reach_each_feed_in_the_order_they_were_applied,
            concurrent_commits_from_both_stores_keep_preconditions_exact,
        );
    };
    (@tests $fixture:expr; $($test:ident),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $test() {
                $crate::two_stores::$test(&$fixture).await;
            }
        )*
    };
}

pub async fn another_stores_commit_arrives_as_one_batch_of_external_changes(
    fixture: &impl Fixture,
) {
    let Opened { store: first, feed: mut first_feed } = fixture.open().await;
    let Opened { store: second, feed: mut second_feed } = fixture.open().await;

    let mut staging = Staging::new(Area::Config);
    staging.write("settings.toml", "a = 1\n").unwrap();
    staging.write("themes/dark.toml", "b = 2\n").unwrap();
    first.commit(staging).await.unwrap();

    // The second Store sees the File, and is told about it as external.
    assert_eq!(
        changes_in_full(&next_batch(&mut second_feed).await),
        [
            (Area::Config, "settings.toml", ChangeKind::Changed, Origin::External),
            (Area::Config, "themes/dark.toml", ChangeKind::Changed, Origin::External),
        ],
    );
    let file = second.read(Area::Config, "settings.toml").await.unwrap().unwrap();
    assert_eq!(file.contents(), "a = 1\n");
    // The first Store is told once, as local.
    assert_eq!(
        changes_in_full(&next_batch(&mut first_feed).await),
        [
            (Area::Config, "settings.toml", ChangeKind::Changed, Origin::Local),
            (Area::Config, "themes/dark.toml", ChangeKind::Changed, Origin::Local),
        ],
    );

    // And the other way round.
    let mut staging = Staging::new(Area::Data);
    staging.delete("missing.txt").unwrap();
    staging.write("from-second.txt", "x").unwrap();
    second.commit(staging).await.unwrap();
    assert_eq!(
        changes_in_full(&next_batch(&mut first_feed).await),
        [(Area::Data, "from-second.txt", ChangeKind::Changed, Origin::External)],
    );
    assert_eq!(
        changes_in_full(&next_batch(&mut second_feed).await),
        [(Area::Data, "from-second.txt", ChangeKind::Changed, Origin::Local)],
    );

    assert_nothing_more(&mut first_feed).await;
    assert_nothing_more(&mut second_feed).await;
}

pub async fn commits_made_before_a_store_is_opened_are_not_reported_to_it(fixture: &impl Fixture) {
    let Opened { store: first, feed: _first_feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("before.txt", "x").unwrap();
    first.commit(staging).await.unwrap();

    let Opened { store: second, feed: mut second_feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("after.txt", "x").unwrap();
    first.commit(staging).await.unwrap();

    assert_eq!(
        changes_in_full(&next_batch(&mut second_feed).await),
        [(Area::Data, "after.txt", ChangeKind::Changed, Origin::External)],
    );
    assert_nothing_more(&mut second_feed).await;
    // It still sees what was there.
    assert!(second.read(Area::Data, "before.txt").await.unwrap().is_some());
}

pub async fn another_stores_commits_are_never_split_across_batches(fixture: &impl Fixture) {
    let Opened { store: first, feed: _first_feed } = fixture.open().await;
    let Opened { store: _second, feed: mut second_feed } = fixture.open().await;
    // Many Files per Commit, so that a Commit recorded in parts would likely be read in parts.
    const FILES: usize = 20;
    const COMMITS: usize = 50;

    // A thread of its own reads batches as they come while the first Store commits, until it has
    // seen every File. On its own thread, it can wake while a Commit is being recorded.
    let reader = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        runtime.block_on(async move {
            let mut batches = Vec::new();
            let mut seen = 0;
            while seen < FILES * COMMITS {
                match next_item_of(&mut second_feed).await {
                    FeedItem::Changes(batch) => {
                        seen += batch.len();
                        batches.push(batch);
                    }
                    other => panic!("expected a batch of Changes, got {other:?}"),
                }
            }
            batches
        })
    });
    for commit in 0..COMMITS {
        let mut staging = Staging::new(Area::Data);
        for file in 0..FILES {
            staging.write(format!("{commit}/{file}"), "x").unwrap();
        }
        first.commit(staging).await.unwrap();
    }
    let batches = reader.join().unwrap();

    // Each Commit wrote its Files under its own Prefix: all of them are in one batch.
    let mut seen = 0;
    for batch in &batches {
        let mut commits = BTreeMap::<&str, usize>::new();
        for change in batch {
            assert_eq!(change.origin, Origin::External);
            let (commit, _file) = change.path.as_str().rsplit_once('/').unwrap();
            *commits.entry(commit).or_default() += 1;
        }
        for (commit, count) in commits {
            assert_eq!(count, FILES, "Commit {commit} was split across batches");
            seen += 1;
        }
    }
    assert_eq!(seen, COMMITS);
}

pub async fn commits_from_both_stores_reach_each_feed_in_the_order_they_were_applied(
    fixture: &impl Fixture,
) {
    let Opened { store: first, feed: mut first_feed } = fixture.open().await;
    let Opened { store: second, feed: mut second_feed } = fixture.open().await;

    // In each round, tasks on both Stores race to write and delete one Path. However the Commits
    // land, what each feed says of the Path must end as the last of them left the File: changed
    // if it is there, removed if it isn't. Once the round is over, the second Store commits a
    // marker, which reaches each feed after every Commit of the round.
    for round in 0..150 {
        let tasks: Vec<_> = (0..4)
            .map(|task| {
                let store = if task % 2 == 0 { first.clone() } else { second.clone() };
                tokio::spawn(async move {
                    for step in 0..4 {
                        let mut staging = Staging::new(Area::Data);
                        if (task / 2 + step) % 2 == 0 {
                            staging.write("raced.txt", format!("{round} {task} {step}")).unwrap();
                        } else {
                            staging.delete("raced.txt").unwrap();
                        }
                        store.commit(staging).await.unwrap();
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let marker = format!("round-{round}.txt");
        let mut staging = Staging::new(Area::Data);
        staging.write(marker.as_str(), "x").unwrap();
        second.commit(staging).await.unwrap();

        let there = first.read(Area::Data, "raced.txt").await.unwrap().is_some();
        let expected = if there { ChangeKind::Changed } else { ChangeKind::Removed };
        for feed in [&mut first_feed, &mut second_feed] {
            assert_eq!(
                kind_until(feed, "raced.txt", &marker).await,
                Some(expected),
                "round {round}"
            );
        }
    }
}

pub async fn concurrent_commits_from_both_stores_keep_preconditions_exact(fixture: &impl Fixture) {
    let Opened { store: first, feed: _first_feed } = fixture.open().await;
    let Opened { store: second, feed: _second_feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("counter.txt", "0").unwrap();
    first.commit(staging).await.unwrap();

    // Tasks on both Stores each add one to the counter many times, writing back what they read.
    // A write back that isn't exact loses an addition. SQLite's "database is busy" while the
    // other Store commits must be waited out, not given as an error.
    const TASKS: usize = 4;
    const ADDITIONS: usize = 25;
    let tasks: Vec<_> = (0..TASKS)
        .map(|task| {
            let store = if task % 2 == 0 { first.clone() } else { second.clone() };
            tokio::spawn(async move {
                let mut conflicts = 0;
                for _ in 0..ADDITIONS {
                    while !add_one(&store).await {
                        conflicts += 1;
                    }
                }
                conflicts
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }

    let counter = second.read(Area::Data, "counter.txt").await.unwrap().unwrap();
    assert_eq!(counter.contents(), (TASKS * ADDITIONS).to_string());
}

// Helpers shared by the tests above.

/// Adds one to the counter, if nobody changed it in between. Gives whether it did.
async fn add_one(store: &Store) -> bool {
    let counter = store.read(Area::Data, "counter.txt").await.unwrap().unwrap();
    let next = counter.contents().parse::<usize>().unwrap() + 1;
    let mut staging = Staging::new(Area::Data);
    staging.write_back(&counter, next.to_string());
    match store.commit(staging).await {
        Ok(_) => true,
        Err(Error::Conflict { .. }) => false,
        Err(other) => panic!("expected the Commit to succeed or conflict, got {other:?}"),
    }
}

/// Reads the Change feed until a Change to `marker` arrives, and gives the kind of the last Change
/// to `path` before it, or in the same batch.
async fn kind_until(feed: &mut ChangeFeed, path: &str, marker: &str) -> Option<ChangeKind> {
    let mut kind = None;
    loop {
        let batch = next_batch(feed).await;
        let mut marked = false;
        for change in batch {
            if change.path.as_str() == path {
                kind = Some(change.kind);
            }
            marked |= change.path.as_str() == marker;
        }
        if marked {
            return kind;
        }
    }
}

/// Waits for the next item on the Change feed, failing the test if none arrives in time.
async fn next_item_of(feed: &mut ChangeFeed) -> FeedItem {
    tokio::time::timeout(Duration::from_secs(5), feed.next())
        .await
        .expect("the Change feed should have sent something by now")
        .expect("the Change feed should not have ended")
}
