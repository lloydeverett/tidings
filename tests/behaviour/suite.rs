//! The tests themselves. Each is an `async fn` taking the Backend's [`Fixture`], and is listed in
//! [`behaviour_suite!`] so that every Backend runs it.

use std::time::Duration;

use jiff::Timestamp;
use tidings::{
    Area, Change, ChangeFeed, ChangeKind, Error, FeedItem, File, InvalidPathReason, Origin,
    Staging, Store,
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
            reading_a_missing_file_gives_nothing,
            stat_gives_the_modified_time_and_revision,
            list_gives_the_paths_under_a_prefix,
            a_delete_removes_the_file_and_announces_it,
            a_prefix_delete_removes_what_is_under_the_prefix_when_committed,
            the_empty_prefix_deletes_the_whole_area,
            a_prefix_delete_and_writes_under_it_apply_in_the_order_staged,
            a_delete_and_a_write_in_one_commit_rename_a_file,
            a_commit_gives_every_file_its_time_and_returns_the_new_revisions,
            a_write_that_changes_nothing_is_left_out,
            a_commit_that_touches_nothing_announces_nothing,
            a_read_file_was_last_modified_by_its_commit,
            a_revision_is_decided_by_the_contents_alone,
            a_commit_announces_one_batch_of_local_changes,
            a_staging_is_built_without_the_store,
            a_dropped_staging_writes_nothing,
            an_invalid_path_is_refused_wherever_it_is_used,
            every_allowed_path_can_be_written_and_read_back,
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

    let file = read(&store, Area::Config, "settings.toml").await;
    assert_eq!(file.contents(), "theme = \"dark\"\n");
}

pub async fn reading_a_missing_file_gives_nothing(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    assert_eq!(store.read(Area::Config, "settings.toml").await.unwrap(), None);
    assert_eq!(store.stat(Area::Config, "settings.toml").await.unwrap(), None);
}

pub async fn stat_gives_the_modified_time_and_revision(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new(Area::Data);
    staging.write("notes/today.md", "# Today\n").unwrap();
    store.commit(staging).await.unwrap();

    let file = read(&store, Area::Data, "notes/today.md").await;
    let stat = store.stat(Area::Data, "notes/today.md").await.unwrap().unwrap();
    assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
}

pub async fn list_gives_the_paths_under_a_prefix(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new(Area::Config);
    for path in ["settings.toml", "themes/dark.toml", "themes/light/main.toml", "themes2/x.toml"] {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();
    let mut other_area = Staging::new(Area::Data);
    other_area.write("themes/elsewhere.toml", "").unwrap();
    store.commit(other_area).await.unwrap();

    let themes = list(&store, Area::Config, "themes/").await;
    assert_eq!(themes, ["themes/dark.toml", "themes/light/main.toml"]);
    let everything = list(&store, Area::Config, "").await;
    assert_eq!(
        everything,
        ["settings.toml", "themes/dark.toml", "themes/light/main.toml", "themes2/x.toml"],
    );
    assert!(list(&store, Area::Config, "nothing/").await.is_empty());
    assert!(list(&store, Area::Cache, "").await.is_empty());
}

pub async fn a_delete_removes_the_file_and_announces_it(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("old.txt", "old").unwrap();
    staging.write("kept.txt", "kept").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    let mut staging = Staging::new(Area::Data);
    staging.delete("old.txt").unwrap();
    // Deleting a Path that doesn't exist does nothing.
    staging.delete("never-existed.txt").unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(store.read(Area::Data, "old.txt").await.unwrap(), None);
    assert_eq!(list(&store, Area::Data, "").await, ["kept.txt"]);
    assert_eq!(changes(&next_batch(&mut feed).await), [("old.txt", ChangeKind::Removed)]);
    assert_nothing_more(&mut feed).await;
}

pub async fn a_prefix_delete_removes_what_is_under_the_prefix_when_committed(
    fixture: &impl Fixture,
) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Cache);
    for path in ["themes/a.toml", "themes2/b.toml", "settings.toml"] {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    // Staged before `themes/deep/c.toml` exists, so only expanding it at Commit time deletes it.
    let mut delete_themes = Staging::new(Area::Cache);
    delete_themes.delete_prefix("themes/").unwrap();
    let mut staging = Staging::new(Area::Cache);
    staging.write("themes/deep/c.toml", "c").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;
    store.commit(delete_themes).await.unwrap();

    assert_eq!(list(&store, Area::Cache, "").await, ["settings.toml", "themes2/b.toml"]);
    assert_eq!(
        changes(&next_batch(&mut feed).await),
        [("themes/a.toml", ChangeKind::Removed), ("themes/deep/c.toml", ChangeKind::Removed)],
    );
    assert_nothing_more(&mut feed).await;
}

pub async fn the_empty_prefix_deletes_the_whole_area(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    for area in [Area::Config, Area::Data] {
        let mut staging = Staging::new(area);
        staging.write("a.txt", "a").unwrap();
        staging.write("b/c.txt", "c").unwrap();
        store.commit(staging).await.unwrap();
    }

    let mut staging = Staging::new(Area::Config);
    staging.delete_prefix("").unwrap();
    store.commit(staging).await.unwrap();

    assert!(list(&store, Area::Config, "").await.is_empty());
    assert_eq!(list(&store, Area::Data, "").await, ["a.txt", "b/c.txt"]);
}

pub async fn a_prefix_delete_and_writes_under_it_apply_in_the_order_staged(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("drafts/old.md", "old").unwrap();
    staging.write("drafts/rewritten.md", "old").unwrap();
    store.commit(staging).await.unwrap();

    let mut staging = Staging::new(Area::Data);
    staging.write("drafts/before.md", "staged before the delete").unwrap();
    staging.delete_prefix("drafts/").unwrap();
    staging.write("drafts/rewritten.md", "staged after the delete").unwrap();
    staging.write("drafts/after.md", "staged after the delete").unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(list(&store, Area::Data, "").await, ["drafts/after.md", "drafts/rewritten.md"]);
    let rewritten = read(&store, Area::Data, "drafts/rewritten.md").await;
    assert_eq!(rewritten.contents(), "staged after the delete");
}

pub async fn a_delete_and_a_write_in_one_commit_rename_a_file(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Config);
    staging.write("old-name.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    let file = read(&store, Area::Config, "old-name.toml").await;
    let mut rename = Staging::new(Area::Config);
    rename.delete(file.path()).unwrap();
    rename.write("new-name.toml", file.contents()).unwrap();
    store.commit(rename).await.unwrap();

    assert_eq!(list(&store, Area::Config, "").await, ["new-name.toml"]);
    assert_eq!(read(&store, Area::Config, "new-name.toml").await.contents(), "a = 1\n");
    assert_eq!(
        changes(&next_batch(&mut feed).await),
        [("new-name.toml", ChangeKind::Changed), ("old-name.toml", ChangeKind::Removed)],
    );
    assert_nothing_more(&mut feed).await;
}

pub async fn a_commit_gives_every_file_its_time_and_returns_the_new_revisions(
    fixture: &impl Fixture,
) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("gone.txt", "gone").unwrap();
    store.commit(staging).await.unwrap();

    let paths = ["a.txt", "b/c.txt", "b/d/e.txt"];
    let mut staging = Staging::new(Area::Data);
    for path in paths {
        staging.write(path, path).unwrap();
    }
    staging.delete("gone.txt").unwrap();
    let committed = store.commit(staging).await.unwrap();

    let mut expected = Vec::new();
    for path in paths {
        let file = read(&store, Area::Data, path).await;
        assert_eq!(file.modified(), committed.timestamp(), "{path}");
        expected.push((path, file.revision()));
    }
    // Only the Paths written have a new Revision, not the one deleted.
    let revisions: Vec<_> =
        committed.revisions().iter().map(|(path, revision)| (path.as_str(), *revision)).collect();
    assert_eq!(revisions, expected);
}

pub async fn a_write_that_changes_nothing_is_left_out(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Config);
    staging.write("same.toml", "same = true\n").unwrap();
    staging.write("changed.toml", "changed = false\n").unwrap();
    let first = store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;
    // So the second Commit's timestamp is certainly later.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let mut staging = Staging::new(Area::Config);
    staging.write("same.toml", "same = true\n").unwrap();
    staging.write("changed.toml", "changed = true\n").unwrap();
    let second = store.commit(staging).await.unwrap();

    assert_ne!(first.timestamp(), second.timestamp());
    let same = read(&store, Area::Config, "same.toml").await;
    assert_eq!(same.modified(), first.timestamp());
    assert_eq!(read(&store, Area::Config, "changed.toml").await.modified(), second.timestamp());
    assert_eq!(changes(&next_batch(&mut feed).await), [("changed.toml", ChangeKind::Changed)]);
    assert_nothing_more(&mut feed).await;
    // The Revision is still given back, so the File can be written again safely.
    assert_eq!(second.revisions()[same.path()], same.revision());
}

pub async fn a_commit_that_touches_nothing_announces_nothing(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("a.txt", "a").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    store.commit(Staging::new(Area::Data)).await.unwrap();
    let mut unchanged = Staging::new(Area::Data);
    unchanged.write("a.txt", "a").unwrap();
    store.commit(unchanged).await.unwrap();
    let mut absent = Staging::new(Area::Data);
    absent.delete("absent.txt").unwrap();
    absent.delete_prefix("nothing/").unwrap();
    store.commit(absent).await.unwrap();

    assert_nothing_more(&mut feed).await;
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
    assert_nothing_more(&mut feed).await;
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

    // One Path for each rule. tests/paths.rs has the full table.
    let refused = [
        ("", InvalidPathReason::Empty),
        ("/etc/passwd", InvalidPathReason::NotRelative),
        ("a//b", InvalidPathReason::EmptySegment),
        ("trailing/", InvalidPathReason::EmptySegment),
        ("a/../b.txt", InvalidPathReason::DotSegment),
        ("CON", InvalidPathReason::UnportableName),
        ("aux.txt", InvalidPathReason::UnportableName),
        ("trailing.", InvalidPathReason::UnportableName),
        ("tab\there", InvalidPathReason::UnportableName),
        ("cafe\u{301}.txt", InvalidPathReason::NotNfc),
        (".tidings/lock", InvalidPathReason::Reserved),
    ];
    for (path, expected) in refused {
        let mut staging = Staging::new(Area::Data);
        assert_refused("writing", path, expected, staging.write(path, "x").map(drop));
        assert_refused("deleting", path, expected, staging.delete(path).map(drop));
        assert_refused("reading", path, expected, store.read(Area::Data, path).await.map(drop));
        assert_refused("stat of", path, expected, store.stat(Area::Data, path).await.map(drop));
    }

    // One Prefix for each rule a Prefix can break. tests/paths.rs has the full table.
    let refused = [
        ("themes", InvalidPathReason::NoTrailingSlash),
        ("/", InvalidPathReason::NotRelative),
        ("/etc/", InvalidPathReason::NotRelative),
        ("a//", InvalidPathReason::EmptySegment),
        ("../", InvalidPathReason::DotSegment),
        ("CON/", InvalidPathReason::UnportableName),
        ("cafe\u{301}/", InvalidPathReason::NotNfc),
        (".tidings/", InvalidPathReason::Reserved),
    ];
    for (prefix, expected) in refused {
        let mut staging = Staging::new(Area::Data);
        let deleting = staging.delete_prefix(prefix).map(drop);
        assert_refused("deleting under", prefix, expected, deleting);
        assert_refused("listing", prefix, expected, store.list(Area::Data, prefix).await.map(drop));
    }
}

pub async fn every_allowed_path_can_be_written_and_read_back(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let allowed = [
        ".hidden",
        "with space.txt",
        "semi;colon.js",
        "console.log",
        "caf\u{e9}/r\u{e9}sum\u{e9}.md",
        "\u{65e5}\u{672c}\u{8a9e}/\u{30e1}\u{30e2}.txt",
        "notes/.tidings",
    ];
    let mut staging = Staging::new(Area::Data);
    for path in allowed {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();

    for path in allowed {
        let file = read(&store, Area::Data, path).await;
        assert_eq!((file.path().as_str(), file.contents()), (path, path));
    }
}

// Helpers shared by the tests above.

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

/// Each Change's Path and kind, for comparing. The tests here only commit from this Store, to one
/// Area at a time.
fn changes(batch: &[Change]) -> Vec<(&str, ChangeKind)> {
    batch.iter().map(|change| (change.path.as_str(), change.kind)).collect()
}

/// Reads a File that the test expects to exist.
async fn read(store: &Store, area: Area, path: &str) -> File {
    store
        .read(area, path)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{path} should exist in {area:?}"))
}

/// Lists the Paths under `prefix`, as strings.
async fn list(store: &Store, area: Area, prefix: &str) -> Vec<String> {
    let paths = store.list(area, prefix).await.unwrap();
    paths.iter().map(|path| path.as_str().to_owned()).collect()
}

/// Checks that nothing more arrives on the Change feed for a short while.
async fn assert_nothing_more(feed: &mut ChangeFeed) {
    if let Ok(item) = tokio::time::timeout(Duration::from_millis(200), feed.next()).await {
        panic!("expected nothing more on the Change feed, got {item:?}");
    }
}

/// Checks that `result`, from `doing` something with `path`, is refused for `expected`.
fn assert_refused(doing: &str, path: &str, expected: InvalidPathReason, result: Result<(), Error>) {
    match result {
        Err(Error::InvalidPath { reason, .. }) => assert_eq!(reason, expected, "{doing} {path:?}"),
        other => panic!("{doing} {path:?} should be refused, got {other:?}"),
    }
}
