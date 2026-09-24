//! The tests themselves. Each is an `async fn` taking the Backend's [`Fixture`], and is listed in
//! [`behaviour_suite!`] so that every Backend runs it.

use std::time::Duration;

use jiff::Timestamp;
use tidings::{
    Area, Change, ChangeFeed, ChangeKind, Error, FeedItem, File, Origin, Staging, Store,
};

/// How a Backend opens a fresh, empty Store for one test.
pub trait Fixture {
    async fn open(&self) -> Opened;
}

/// A freshly opened Store. Backends that keep files on disk will also hold their temporary Root
/// override here, so it lives as long as the test.
pub struct Opened {
    pub store: Store,
    pub feed: ChangeFeed,
}

/// Instantiates every test in the suite for one Backend's [`Fixture`].
macro_rules! behaviour_suite {
    ($fixture:expr) => {
        behaviour_suite!(@tests $fixture;
            a_committed_write_can_be_read_back,
            a_read_file_was_last_modified_by_its_commit,
            a_revision_is_decided_by_the_contents_alone,
            a_commit_announces_one_batch_of_local_changes,
            a_staging_is_built_without_the_store,
            a_dropped_staging_writes_nothing,
            an_invalid_path_is_refused_wherever_it_is_used,
        );
    };
    (@tests $fixture:expr; $($test:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $test() {
                $crate::suite::$test(&$fixture).await;
            }
        )*
    };
}

pub async fn a_committed_write_can_be_read_back(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new(Area::Config);
    staging.write("settings.toml", "theme = \"dark\"\n").unwrap();
    store.commit(staging).await.unwrap();

    let file = store.read(Area::Config, "settings.toml").await.unwrap().unwrap();
    assert_eq!(file.contents(), "theme = \"dark\"\n");
}

pub async fn a_read_file_was_last_modified_by_its_commit(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let before = Timestamp::now();
    let mut staging = Staging::new(Area::Data);
    staging.write("notes/today.md", "# Today\n").unwrap();
    store.commit(staging).await.unwrap();
    let after = Timestamp::now();

    let file = read(&store, Area::Data, "notes/today.md").await;
    assert!(
        before <= file.modified() && file.modified() <= after,
        "{} should be between {before} and {after}",
        file.modified(),
    );
}

pub async fn a_revision_is_decided_by_the_contents_alone(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new(Area::Cache);
    staging.write("a.txt", "same").unwrap();
    staging.write("b.txt", "same").unwrap();
    staging.write("c.txt", "different").unwrap();
    store.commit(staging).await.unwrap();

    let a = read(&store, Area::Cache, "a.txt").await;
    let b = read(&store, Area::Cache, "b.txt").await;
    let c = read(&store, Area::Cache, "c.txt").await;
    assert_eq!(a.revision(), b.revision());
    assert_ne!(a.revision(), c.revision());

    let mut staging = Staging::new(Area::Cache);
    staging.write("a.txt", "different").unwrap();
    store.commit(staging).await.unwrap();
    assert_eq!(read(&store, Area::Cache, "a.txt").await.revision(), c.revision());
}

/// Waits for the next item on the Change feed, failing the test if none arrives in time.
async fn next_item(feed: &mut ChangeFeed) -> FeedItem {
    tokio::time::timeout(Duration::from_secs(5), feed.next())
        .await
        .expect("the Change feed should have sent something by now")
        .expect("the Change feed should not have ended")
}

/// Waits for the next batch of Changes, sorted by Area and Path so it can be compared.
async fn next_batch(feed: &mut ChangeFeed) -> Vec<Change> {
    match next_item(feed).await {
        FeedItem::Changes(mut batch) => {
            batch.sort_by(|a, b| (a.area, &a.path).cmp(&(b.area, &b.path)));
            batch
        }
        other => panic!("expected a batch of Changes, got {other:?}"),
    }
}

/// Reads a File that the test expects to exist.
async fn read(store: &Store, area: Area, path: &str) -> File {
    store
        .read(area, path)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{path} should exist in {area:?}"))
}

pub async fn a_commit_announces_one_batch_of_local_changes(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;

    let mut staging = Staging::new(Area::Config);
    staging.write("settings.toml", "a = 1\n").unwrap();
    staging.write("themes/dark.toml", "b = 2\n").unwrap();
    store.commit(staging).await.unwrap();

    let batch = next_batch(&mut feed).await;
    let seen: Vec<_> = batch
        .iter()
        .map(|change| (change.area, change.path.as_str(), change.kind, change.origin))
        .collect();
    assert_eq!(
        seen,
        [
            (Area::Config, "settings.toml", ChangeKind::Changed, Origin::Local),
            (Area::Config, "themes/dark.toml", ChangeKind::Changed, Origin::Local),
        ],
    );
}

pub async fn a_staging_is_built_without_the_store(fixture: &impl Fixture) {
    let mut staging = Staging::new(Area::Data);
    staging.write("built-first.txt", "before the Store was opened").unwrap();

    let Opened { store, feed: _feed } = fixture.open().await;
    store.commit(staging).await.unwrap();

    let file = read(&store, Area::Data, "built-first.txt").await;
    assert_eq!(file.contents(), "before the Store was opened");
}

pub async fn a_dropped_staging_writes_nothing(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;

    let mut dropped = Staging::new(Area::Config);
    dropped.write("abandoned.toml", "never = true\n").unwrap();
    drop(dropped);

    let mut committed = Staging::new(Area::Config);
    committed.write("kept.toml", "kept = true\n").unwrap();
    store.commit(committed).await.unwrap();

    assert_eq!(store.read(Area::Config, "abandoned.toml").await.unwrap(), None);
    let batch = next_batch(&mut feed).await;
    let paths: Vec<_> = batch.iter().map(|change| change.path.as_str()).collect();
    assert_eq!(paths, ["kept.toml"]);
}

pub async fn an_invalid_path_is_refused_wherever_it_is_used(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    for path in ["", "/etc/passwd", "a//b", "trailing/"] {
        let mut staging = Staging::new(Area::Data);
        assert!(
            matches!(staging.write(path, "x"), Err(Error::InvalidPath { .. })),
            "writing {path:?} should be refused",
        );
        assert!(
            matches!(store.read(Area::Data, path).await, Err(Error::InvalidPath { .. })),
            "reading {path:?} should be refused",
        );
    }
}
