//! The tests themselves. Each is an `async fn` taking the Backend's [`Fixture`], and is listed in
//! [`behaviour_suite!`] so that every Backend runs it.

use std::time::Duration;

use jiff::Timestamp;
use tidings::{
    Area, Change, ChangeFeed, ChangeKind, Committed, Error, FeedItem, File, InvalidPathReason,
    Origin, Precondition, Staging, Store,
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
            a_write_requiring_absence_creates_a_file_only_if_there_is_none,
            writing_back_a_file_requires_it_unchanged_since_it_was_read,
            a_delete_can_require_the_file_unchanged,
            a_rename_that_conflicts_leaves_both_paths_as_they_were,
            a_commit_can_require_a_file_it_does_not_write,
            a_prefix_revision_changes_when_a_file_under_the_prefix_does,
            a_prefix_precondition_fails_when_a_file_is_added_under_the_prefix,
            a_prefix_conflict_names_the_paths_added_removed_or_changed,
            a_precondition_stays_when_something_staged_later_replaces_it,
            a_staging_without_preconditions_depends_on_nothing,
            a_path_differing_only_in_letter_case_is_refused,
            a_file_cannot_be_under_another_file,
            a_prefix_revision_is_only_for_its_own_area_and_prefix,
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
        let requiring = staging.require(path, Precondition::Absent).map(drop);
        assert_refused("requiring", path, expected, requiring);
        let writing = staging.write_requiring(path, "x", Precondition::Absent).map(drop);
        assert_refused("writing", path, expected, writing);
        let deleting = staging.delete_requiring(path, Precondition::Absent).map(drop);
        assert_refused("deleting", path, expected, deleting);
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
        let stat = store.stat_prefix(Area::Data, prefix).await;
        let prefix_revision = store.stat_prefix(Area::Data, "").await.unwrap();
        assert_refused("stat of", prefix, expected, stat.map(drop));
        let requiring = staging.require_prefix(prefix, prefix_revision).map(drop);
        assert_refused("requiring", prefix, expected, requiring);
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

pub async fn a_write_requiring_absence_creates_a_file_only_if_there_is_none(
    fixture: &impl Fixture,
) {
    let Opened { store, mut feed } = fixture.open().await;

    let mut create = Staging::new(Area::Data);
    create.write_requiring("id.txt", "first", Precondition::Absent).unwrap();
    store.commit(create).await.unwrap();
    next_batch(&mut feed).await;

    let mut create_again = Staging::new(Area::Data);
    create_again.write_requiring("id.txt", "second", Precondition::Absent).unwrap();
    create_again.write("other.txt", "other").unwrap();
    assert_conflict(store.commit(create_again).await, &["id.txt"]);

    assert_eq!(read(&store, Area::Data, "id.txt").await.contents(), "first");
    assert_eq!(store.read(Area::Data, "other.txt").await.unwrap(), None);
    assert_nothing_more(&mut feed).await;
}

pub async fn writing_back_a_file_requires_it_unchanged_since_it_was_read(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Config);
    staging.write("settings.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();

    // Someone else changes the File between our read and our write.
    let ours = read(&store, Area::Config, "settings.toml").await;
    let mut theirs = Staging::new(Area::Config);
    theirs.write("settings.toml", "a = 2\n").unwrap();
    store.commit(theirs).await.unwrap();
    let mut write_back = Staging::new(Area::Config);
    write_back.write_back(&ours, "a = 1\nb = 1\n");
    assert_conflict(store.commit(write_back).await, &["settings.toml"]);
    assert_eq!(read(&store, Area::Config, "settings.toml").await.contents(), "a = 2\n");

    // Read again, it goes through, and the Revision it gives back is good for the next write.
    let ours = read(&store, Area::Config, "settings.toml").await;
    let mut write_back = Staging::new(Area::Config);
    write_back.write_back(&ours, "a = 2\nb = 1\n");
    let committed = store.commit(write_back).await.unwrap();
    let mut again = Staging::new(Area::Config);
    let revision = committed.revisions()[ours.path()];
    again.write_requiring(ours.path(), "a = 3\n", Precondition::UnchangedSince(revision)).unwrap();
    store.commit(again).await.unwrap();
    assert_eq!(read(&store, Area::Config, "settings.toml").await.contents(), "a = 3\n");
}

pub async fn a_delete_can_require_the_file_unchanged(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("a.txt", "a").unwrap();
    store.commit(staging).await.unwrap();
    let read_a = read(&store, Area::Data, "a.txt").await;

    let mut change = Staging::new(Area::Data);
    change.write("a.txt", "changed").unwrap();
    store.commit(change).await.unwrap();
    let mut delete = Staging::new(Area::Data);
    delete.delete_requiring("a.txt", Precondition::UnchangedSince(read_a.revision())).unwrap();
    assert_conflict(store.commit(delete).await, &["a.txt"]);
    assert_eq!(read(&store, Area::Data, "a.txt").await.contents(), "changed");

    // Changed back, the contents are what they were, so it counts as unchanged.
    let mut change_back = Staging::new(Area::Data);
    change_back.write("a.txt", "a").unwrap();
    store.commit(change_back).await.unwrap();
    let mut delete = Staging::new(Area::Data);
    delete.delete_requiring("a.txt", Precondition::UnchangedSince(read_a.revision())).unwrap();
    store.commit(delete).await.unwrap();
    assert_eq!(store.read(Area::Data, "a.txt").await.unwrap(), None);

    // Absent holds for a delete of a Path with no File, which does nothing.
    let mut delete = Staging::new(Area::Data);
    delete.delete_requiring("a.txt", Precondition::Absent).unwrap();
    store.commit(delete).await.unwrap();
}

pub async fn a_rename_that_conflicts_leaves_both_paths_as_they_were(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Config);
    staging.write("old-name.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();
    let file = read(&store, Area::Config, "old-name.toml").await;
    let mut change = Staging::new(Area::Config);
    change.write("old-name.toml", "a = 2\n").unwrap();
    store.commit(change).await.unwrap();
    next_batch(&mut feed).await;
    next_batch(&mut feed).await;

    let mut rename = Staging::new(Area::Config);
    rename.delete_requiring(file.path(), Precondition::UnchangedSince(file.revision())).unwrap();
    rename.write_requiring("new-name.toml", file.contents(), Precondition::Absent).unwrap();
    assert_conflict(store.commit(rename).await, &["old-name.toml"]);

    assert_eq!(list(&store, Area::Config, "").await, ["old-name.toml"]);
    assert_eq!(read(&store, Area::Config, "old-name.toml").await.contents(), "a = 2\n");
    assert_nothing_more(&mut feed).await;
}

pub async fn a_commit_can_require_a_file_it_does_not_write(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("rates.toml", "rate = 2\n").unwrap();
    store.commit(staging).await.unwrap();

    // Prices are worked out from the rates, so they are only written if the rates haven't moved.
    let rates = read(&store, Area::Data, "rates.toml").await;
    let unchanged = Precondition::UnchangedSince(rates.revision());
    let mut prices = Staging::new(Area::Data);
    prices.require(rates.path(), unchanged).unwrap();
    prices.require("lock.txt", Precondition::Absent).unwrap();
    prices.write("prices.toml", "price = 20\n").unwrap();
    store.commit(prices).await.unwrap();

    let mut change = Staging::new(Area::Data);
    change.write("rates.toml", "rate = 3\n").unwrap();
    change.write("lock.txt", "").unwrap();
    store.commit(change).await.unwrap();
    let mut prices = Staging::new(Area::Data);
    prices.require(rates.path(), unchanged).unwrap();
    prices.require("lock.txt", Precondition::Absent).unwrap();
    prices.write("prices.toml", "price = 30\n").unwrap();
    assert_conflict(store.commit(prices).await, &["lock.txt", "rates.toml"]);

    assert_eq!(read(&store, Area::Data, "prices.toml").await.contents(), "price = 20\n");
    assert_eq!(read(&store, Area::Data, "rates.toml").await.contents(), "rate = 3\n");
}

pub async fn a_prefix_revision_changes_when_a_file_under_the_prefix_does(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let empty_area = store.stat_prefix(Area::Config, "").await.unwrap();
    let mut staging = Staging::new(Area::Config);
    staging.write("themes/dark.toml", "dark").unwrap();
    staging.write("settings.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();
    let themes = store.stat_prefix(Area::Config, "themes/").await.unwrap();
    let area = store.stat_prefix(Area::Config, "").await.unwrap();
    assert_ne!(area, empty_area);

    let mut steps: Vec<(&str, Staging)> = Vec::new();
    let mut added = Staging::new(Area::Config);
    added.write("themes/deep/light.toml", "light").unwrap();
    steps.push(("added", added));
    let mut changed = Staging::new(Area::Config);
    changed.write("themes/dark.toml", "darker").unwrap();
    steps.push(("changed", changed));
    let mut removed = Staging::new(Area::Config);
    removed.delete("themes/dark.toml").unwrap();
    steps.push(("removed", removed));
    let mut seen = vec![themes.clone()];
    for (what, staging) in steps {
        store.commit(staging).await.unwrap();
        let now = store.stat_prefix(Area::Config, "themes/").await.unwrap();
        assert!(!seen.contains(&now), "a File under the Prefix was {what}");
        seen.push(now);
    }

    // Put back as it was, it is as it was. Changes outside the Prefix, or in another Area, don't
    // count.
    let mut back = Staging::new(Area::Config);
    back.write("themes/dark.toml", "dark").unwrap();
    back.delete("themes/deep/light.toml").unwrap();
    back.write("themes2/dark.toml", "outside").unwrap();
    store.commit(back).await.unwrap();
    let mut other_area = Staging::new(Area::Data);
    other_area.write("themes/dark.toml", "elsewhere").unwrap();
    store.commit(other_area).await.unwrap();
    assert_eq!(store.stat_prefix(Area::Config, "themes/").await.unwrap(), themes);

    // The empty Prefix covers the whole Area.
    assert_ne!(store.stat_prefix(Area::Config, "").await.unwrap(), area);
}

pub async fn a_prefix_precondition_fails_when_a_file_is_added_under_the_prefix(
    fixture: &impl Fixture,
) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("inbox/1.eml", "one").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    // The index is worked out from everything in the inbox, so it holds only if nothing arrived.
    let inbox = store.stat_prefix(Area::Data, "inbox/").await.unwrap();
    let mut index = Staging::new(Area::Data);
    index.require_prefix("inbox/", inbox.clone()).unwrap();
    index.write("index.txt", "1.eml").unwrap();
    let mut arrival = Staging::new(Area::Data);
    arrival.write("inbox/2.eml", "two").unwrap();
    store.commit(arrival).await.unwrap();
    next_batch(&mut feed).await;
    assert_conflict(store.commit(index).await, &["inbox/2.eml"]);
    assert_eq!(store.read(Area::Data, "index.txt").await.unwrap(), None);
    assert_nothing_more(&mut feed).await;

    // With a Prefix Revision taken after the arrival, and changes only outside the Prefix, it
    // goes through.
    let inbox = store.stat_prefix(Area::Data, "inbox/").await.unwrap();
    let mut outside = Staging::new(Area::Data);
    outside.write("inbox2/3.eml", "three").unwrap();
    store.commit(outside).await.unwrap();
    let mut index = Staging::new(Area::Data);
    index.require_prefix("inbox/", inbox).unwrap();
    index.write("index.txt", "1.eml 2.eml").unwrap();
    store.commit(index).await.unwrap();
    assert_eq!(read(&store, Area::Data, "index.txt").await.contents(), "1.eml 2.eml");
}

pub async fn a_prefix_conflict_names_the_paths_added_removed_or_changed(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Cache);
    for path in ["a/changed.txt", "a/kept.txt", "a/removed.txt", "b.txt"] {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();
    let under_a = store.stat_prefix(Area::Cache, "a/").await.unwrap();
    let whole_area = store.stat_prefix(Area::Cache, "").await.unwrap();

    let mut changes = Staging::new(Area::Cache);
    changes.write("a/added.txt", "added").unwrap();
    changes.write("a/changed.txt", "changed").unwrap();
    changes.delete("a/removed.txt").unwrap();
    changes.write("b.txt", "changed outside a/").unwrap();
    store.commit(changes).await.unwrap();

    let mut staging = Staging::new(Area::Cache);
    staging.require_prefix("a/", under_a).unwrap();
    staging.write("summary.txt", "").unwrap();
    assert_conflict(
        store.commit(staging).await,
        &["a/added.txt", "a/changed.txt", "a/removed.txt"],
    );
    let mut staging = Staging::new(Area::Cache);
    staging.require_prefix("", whole_area).unwrap();
    staging.write("summary.txt", "").unwrap();
    assert_conflict(
        store.commit(staging).await,
        &["a/added.txt", "a/changed.txt", "a/removed.txt", "b.txt"],
    );
    assert_eq!(store.read(Area::Cache, "summary.txt").await.unwrap(), None);
}

pub async fn a_precondition_stays_when_something_staged_later_replaces_it(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("drafts/a.md", "a").unwrap();
    staging.write("notes.md", "notes").unwrap();
    store.commit(staging).await.unwrap();
    let draft = read(&store, Area::Data, "drafts/a.md").await;
    let notes = read(&store, Area::Data, "notes.md").await;
    let staged_later = |staging: &mut Staging| {
        staging.write_back(&draft, "edited");
        staging.delete_prefix("drafts/").unwrap();
        staging.write_back(&notes, "edited");
        staging.write("notes.md", "replaced").unwrap();
    };

    let mut change = Staging::new(Area::Data);
    change.write("drafts/a.md", "changed").unwrap();
    change.write("notes.md", "changed").unwrap();
    store.commit(change).await.unwrap();
    let mut staging = Staging::new(Area::Data);
    staged_later(&mut staging);
    assert_conflict(store.commit(staging).await, &["drafts/a.md", "notes.md"]);
    assert_eq!(list(&store, Area::Data, "").await, ["drafts/a.md", "notes.md"]);

    // Once they hold again, what was staged later is what happens.
    let mut change_back = Staging::new(Area::Data);
    change_back.write("drafts/a.md", "a").unwrap();
    change_back.write("notes.md", "notes").unwrap();
    store.commit(change_back).await.unwrap();
    let mut staging = Staging::new(Area::Data);
    staged_later(&mut staging);
    store.commit(staging).await.unwrap();
    assert_eq!(list(&store, Area::Data, "").await, ["notes.md"]);
    assert_eq!(read(&store, Area::Data, "notes.md").await.contents(), "replaced");
}

pub async fn a_staging_without_preconditions_depends_on_nothing(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Config);
    staging.write("a.toml", "a = 1\n").unwrap();
    staging.write("b.toml", "b = 1\n").unwrap();
    store.commit(staging).await.unwrap();

    // Staged after reading, then everything changes before the Commit.
    let mut staging = Staging::new(Area::Config);
    staging.write("a.toml", "a = 3\n").unwrap();
    staging.delete("b.toml").unwrap();
    staging.write("c.toml", "c = 3\n").unwrap();
    let mut change = Staging::new(Area::Config);
    change.write("a.toml", "a = 2\n").unwrap();
    change.write("b.toml", "b = 2\n").unwrap();
    change.write("c.toml", "c = 2\n").unwrap();
    store.commit(change).await.unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(list(&store, Area::Config, "").await, ["a.toml", "c.toml"]);
    assert_eq!(read(&store, Area::Config, "a.toml").await.contents(), "a = 3\n");
    assert_eq!(read(&store, Area::Config, "c.toml").await.contents(), "c = 3\n");
}

pub async fn a_path_differing_only_in_letter_case_is_refused(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Config);
    staging.write("Settings.toml", "a = 1\n").unwrap();
    staging.write("themes/dark.toml", "dark").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    // Each Commit also writes a File that would be fine alone, which must not be written either.
    // Where two Paths in the Commit clash, either may be the one refused.
    let clashes: [&[&str]; 8] = [
        // With an existing Path.
        &["settings.toml"],
        // With a Prefix: on a case-insensitive filesystem, `Themes/` is the `themes/` directory.
        &["Themes/light.toml"],
        &["THEMES/DARK.TOML"],
        // With another Path in the same Commit.
        &["new.toml", "NEW.toml"],
        &["fonts/a.ttf", "FONTS/b.ttf"],
        // Letters that are the same only once folded or uppercased, or only uppercased as Windows
        // does: `ß` and `ss`, the long `ſ` and `s`, the dotless `ı` and `I`.
        &["stra\u{df}e.txt", "STRASSE.txt"],
        &["\u{17f}.txt", "s.txt"],
        &["\u{131}.txt", "I.txt"],
    ];
    for paths in clashes {
        let mut staging = Staging::new(Area::Config);
        for path in paths {
            staging.write(*path, "clash").unwrap();
        }
        staging.write("fine.toml", "fine").unwrap();
        match store.commit(staging).await {
            Err(Error::InvalidPath { path, reason: InvalidPathReason::LetterCaseClash }) => {
                assert!(paths.contains(&path.as_str()), "{paths:?} refused for {path:?}");
            }
            other => panic!("{paths:?} should clash, got {other:?}"),
        }
    }
    assert_eq!(list(&store, Area::Config, "").await, ["Settings.toml", "themes/dark.toml"]);
    assert_nothing_more(&mut feed).await;

    // Writing a File again in its own letter case is fine, and so is renaming it to another letter
    // case in one Commit, since only one of them is left.
    let mut staging = Staging::new(Area::Config);
    staging.write("Settings.toml", "a = 2\n").unwrap();
    staging.write("themes/light.toml", "light").unwrap();
    store.commit(staging).await.unwrap();
    let mut rename = Staging::new(Area::Config);
    rename.delete("Settings.toml").unwrap();
    rename.write("settings.toml", "a = 2\n").unwrap();
    rename.delete_prefix("themes/").unwrap();
    rename.write("Themes/dark.toml", "dark").unwrap();
    store.commit(rename).await.unwrap();
    assert_eq!(list(&store, Area::Config, "").await, ["Themes/dark.toml", "settings.toml"]);
}

pub async fn a_file_cannot_be_under_another_file(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new(Area::Data);
    staging.write("a", "a").unwrap();
    staging.write("d/e", "e").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    // A filesystem can't hold a File named like the directory other Files are in. Each Commit
    // also writes a File that would be fine alone, which must not be written either. Where two
    // Paths in the Commit clash, either may be the one refused.
    use InvalidPathReason::{FileUnderFile, LetterCaseClash};
    let clashes: [(&[&str], InvalidPathReason); 7] = [
        // Under an existing File.
        (&["a/b"], FileUnderFile),
        (&["a/b/c"], FileUnderFile),
        // Where existing Files are under it.
        (&["d"], FileUnderFile),
        // Both in the same Commit.
        (&["n", "n/m"], FileUnderFile),
        // Where it would be one of those, but for letter case.
        (&["D"], LetterCaseClash),
        (&["A/b"], LetterCaseClash),
        (&["N", "n/m"], LetterCaseClash),
    ];
    for (paths, expected) in clashes {
        let mut staging = Staging::new(Area::Data);
        for path in paths {
            staging.write(*path, "clash").unwrap();
        }
        staging.write("fine.txt", "fine").unwrap();
        match store.commit(staging).await {
            Err(Error::InvalidPath { path, reason }) if reason == expected => {
                assert!(paths.contains(&path.as_str()), "{paths:?} refused for {path:?}");
            }
            other => panic!("{paths:?} should be refused for {expected:?}, got {other:?}"),
        }
    }
    assert_eq!(list(&store, Area::Data, "").await, ["a", "d/e"]);
    assert_nothing_more(&mut feed).await;

    // A Path deleted in the same Commit doesn't count, so a File can be moved under its own name,
    // or to where the Files under it were.
    let mut rename = Staging::new(Area::Data);
    rename.delete("a").unwrap();
    rename.write("a/b", "a").unwrap();
    rename.delete_prefix("d/").unwrap();
    rename.write("d", "e").unwrap();
    store.commit(rename).await.unwrap();
    assert_eq!(list(&store, Area::Data, "").await, ["a/b", "d"]);
}

pub async fn a_prefix_revision_is_only_for_its_own_area_and_prefix(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let themes = store.stat_prefix(Area::Config, "themes/").await.unwrap();

    // Requiring it for anything else is a mistake in the app, not a Conflict, so it panics.
    for (area, prefix) in [(Area::Config, "fonts/"), (Area::Config, ""), (Area::Data, "themes/")] {
        let requiring = std::panic::catch_unwind(|| {
            let mut staging = Staging::new(area);
            let _ = staging.require_prefix(prefix, themes.clone());
        });
        assert!(requiring.is_err(), "requiring it for {area:?} {prefix:?} should panic");
    }

    let mut staging = Staging::new(Area::Config);
    staging.require_prefix("themes/", themes).unwrap();
    store.commit(staging).await.unwrap();
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

/// Checks that a Commit failed with a Conflict on exactly `expected`.
fn assert_conflict(result: Result<Committed, Error>, expected: &[&str]) {
    match result {
        Err(Error::Conflict { paths }) => {
            let paths: Vec<_> = paths.iter().map(|path| path.as_str()).collect();
            assert_eq!(paths, expected);
        }
        other => panic!("expected a Conflict on {expected:?}, got {other:?}"),
    }
}

/// Checks that `result`, from `doing` something with `path`, is refused for `expected`.
fn assert_refused(doing: &str, path: &str, expected: InvalidPathReason, result: Result<(), Error>) {
    match result {
        Err(Error::InvalidPath { reason, .. }) => assert_eq!(reason, expected, "{doing} {path:?}"),
        other => panic!("{doing} {path:?} should be refused, got {other:?}"),
    }
}
