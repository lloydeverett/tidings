//! The tests themselves. Each is an `async fn` taking the Backend's [`Fixture`], and is listed in
//! [`behaviour_suite!`] so that every Backend runs it.

use std::collections::BTreeSet;
use std::task::Poll;
use std::time::Duration;

use jiff::Timestamp;
use tidings::{
    ChangeKind, Committed, Error, FeedItem, File, InvalidPathReason, Origin, Path, Precondition,
    Revision, Staging,
};

use crate::api::{ChangeFeed, Snapshot, Store};
use crate::common::{assert_ended, assert_nothing_more, changes, changes_in_full, next_batch};

/// How a Backend opens a fresh, empty Store for one test, through the async API or the blocking
/// one. Each test makes its own Fixture, so a Backend that keeps Files on disk holds the test's
/// temporary Location in it, and each Store it opens is at that Location.
pub trait Fixture {
    async fn open(&self) -> Opened;
}

/// A freshly opened Store.
pub struct Opened {
    pub store: Store,
    pub feed: ChangeFeed,
}

/// A Store and its Change feed as an `open` function gives them, from either API.
impl<S: Into<Store>, F: Into<ChangeFeed>> From<(S, F)> for Opened {
    fn from((store, feed): (S, F)) -> Opened {
        Opened { store: store.into(), feed: feed.into() }
    }
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
            the_empty_prefix_deletes_the_whole_store,
            a_prefix_delete_and_writes_under_it_apply_in_the_order_staged,
            a_delete_and_a_write_in_one_commit_rename_a_file,
            a_commit_gives_every_file_its_time_and_returns_the_new_revisions,
            a_write_that_changes_nothing_is_left_out,
            a_commit_that_touches_nothing_announces_nothing,
            a_read_file_was_last_modified_by_its_commit,
            a_revision_is_decided_by_the_contents_alone,
            a_revision_written_as_text_reads_back_as_itself,
            a_prefix_revision_is_written_as_its_hash,
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
            a_prefix_revision_is_only_for_its_own_store_and_prefix,
            unread_changes_are_merged_per_path_and_the_latest_kind_wins,
            clones_of_a_store_share_its_files_and_its_change_feed,
            the_store_keeps_working_once_its_change_feed_is_dropped,
            a_commit_made_before_the_feed_is_first_read_is_reported,
            a_commits_changes_are_never_split_across_batches,
            concurrent_commits_reach_the_feed_in_the_order_they_were_made,
            a_cancelled_commit_finishes_or_never_happens_and_is_reported_if_it_finishes,
            a_snapshot_reads_the_store_as_it_was_when_taken,
            reads_through_a_snapshot_never_mix_commits,
            holding_a_snapshot_does_not_hold_up_commits,
            a_snapshot_outlives_the_store_without_keeping_the_feed_open,
            a_backend_without_snapshots_refuses_one,
        );
    };
    (@tests $fixture:expr; $($test:ident),* $(,)?) => {
        $(
            // Several threads, so that tests of concurrent Commits really run them at once.
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $test() {
                $crate::suite::$test(&$fixture).await;
            }
        )*
    };
}

pub async fn a_committed_write_can_be_read_back(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new();
    staging.write("settings.toml", "theme = \"dark\"\n").unwrap();
    store.commit(staging).await.unwrap();

    let file = read(&store, "settings.toml").await;
    assert_eq!(file.contents(), "theme = \"dark\"\n");
}

pub async fn reading_a_missing_file_gives_nothing(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    assert_eq!(store.read("settings.toml").await.unwrap(), None);
    assert_eq!(store.stat("settings.toml").await.unwrap(), None);
}

pub async fn stat_gives_the_modified_time_and_revision(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new();
    staging.write("notes/today.md", "# Today\n").unwrap();
    store.commit(staging).await.unwrap();

    let file = read(&store, "notes/today.md").await;
    let stat = store.stat("notes/today.md").await.unwrap().unwrap();
    assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
}

pub async fn list_gives_the_paths_under_a_prefix(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new();
    for path in ["settings.toml", "themes/dark.toml", "themes/light/main.toml", "themes2/x.toml"] {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();

    let themes = list(&store, "themes/").await;
    assert_eq!(themes, ["themes/dark.toml", "themes/light/main.toml"]);
    let everything = list(&store, "").await;
    assert_eq!(
        everything,
        ["settings.toml", "themes/dark.toml", "themes/light/main.toml", "themes2/x.toml"],
    );
    assert!(list(&store, "nothing/").await.is_empty());
}

pub async fn a_delete_removes_the_file_and_announces_it(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("old.txt", "old").unwrap();
    staging.write("kept.txt", "kept").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    let mut staging = Staging::new();
    staging.delete("old.txt").unwrap();
    // Deleting a Path that doesn't exist does nothing.
    staging.delete("never-existed.txt").unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(store.read("old.txt").await.unwrap(), None);
    assert_eq!(list(&store, "").await, ["kept.txt"]);
    assert_eq!(changes(&next_batch(&mut feed).await), [("old.txt", ChangeKind::Removed)]);
    assert_nothing_more(&mut feed).await;
}

pub async fn a_prefix_delete_removes_what_is_under_the_prefix_when_committed(
    fixture: &impl Fixture,
) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
    for path in ["themes/a.toml", "themes2/b.toml", "settings.toml"] {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    // Staged before `themes/deep/c.toml` exists, so only expanding it at Commit time deletes it.
    let mut delete_themes = Staging::new();
    delete_themes.delete_prefix("themes/").unwrap();
    let mut staging = Staging::new();
    staging.write("themes/deep/c.toml", "c").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;
    store.commit(delete_themes).await.unwrap();

    assert_eq!(list(&store, "").await, ["settings.toml", "themes2/b.toml"]);
    assert_eq!(
        changes(&next_batch(&mut feed).await),
        [("themes/a.toml", ChangeKind::Removed), ("themes/deep/c.toml", ChangeKind::Removed)],
    );
    assert_nothing_more(&mut feed).await;
}

pub async fn the_empty_prefix_deletes_the_whole_store(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("a.txt", "a").unwrap();
    staging.write("b/c.txt", "c").unwrap();
    store.commit(staging).await.unwrap();

    let mut staging = Staging::new();
    staging.delete_prefix("").unwrap();
    store.commit(staging).await.unwrap();

    assert!(list(&store, "").await.is_empty());
}

pub async fn a_prefix_delete_and_writes_under_it_apply_in_the_order_staged(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("drafts/old.md", "old").unwrap();
    staging.write("drafts/rewritten.md", "old").unwrap();
    store.commit(staging).await.unwrap();

    let mut staging = Staging::new();
    staging.write("drafts/before.md", "staged before the delete").unwrap();
    staging.delete_prefix("drafts/").unwrap();
    staging.write("drafts/rewritten.md", "staged after the delete").unwrap();
    staging.write("drafts/after.md", "staged after the delete").unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(list(&store, "").await, ["drafts/after.md", "drafts/rewritten.md"]);
    let rewritten = read(&store, "drafts/rewritten.md").await;
    assert_eq!(rewritten.contents(), "staged after the delete");
}

pub async fn a_delete_and_a_write_in_one_commit_rename_a_file(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("old-name.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    let file = read(&store, "old-name.toml").await;
    let mut rename = Staging::new();
    rename.delete(file.path()).unwrap();
    rename.write("new-name.toml", file.contents()).unwrap();
    store.commit(rename).await.unwrap();

    assert_eq!(list(&store, "").await, ["new-name.toml"]);
    assert_eq!(read(&store, "new-name.toml").await.contents(), "a = 1\n");
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
    let mut staging = Staging::new();
    staging.write("gone.txt", "gone").unwrap();
    store.commit(staging).await.unwrap();

    let paths = ["a.txt", "b/c.txt", "b/d/e.txt"];
    let mut staging = Staging::new();
    for path in paths {
        staging.write(path, path).unwrap();
    }
    staging.delete("gone.txt").unwrap();
    let committed = store.commit(staging).await.unwrap();

    let mut expected = Vec::new();
    for path in paths {
        let file = read(&store, path).await;
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
    let mut staging = Staging::new();
    staging.write("same.toml", "same = true\n").unwrap();
    staging.write("changed.toml", "changed = false\n").unwrap();
    let first = store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;
    // So the second Commit's timestamp is certainly later.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let mut staging = Staging::new();
    staging.write("same.toml", "same = true\n").unwrap();
    staging.write("changed.toml", "changed = true\n").unwrap();
    let second = store.commit(staging).await.unwrap();

    assert_ne!(first.timestamp(), second.timestamp());
    let same = read(&store, "same.toml").await;
    assert_eq!(same.modified(), first.timestamp());
    assert_eq!(read(&store, "changed.toml").await.modified(), second.timestamp());
    assert_eq!(changes(&next_batch(&mut feed).await), [("changed.toml", ChangeKind::Changed)]);
    assert_nothing_more(&mut feed).await;
    // The Revision is still given back, so the File can be written again safely.
    assert_eq!(second.revisions()[same.path()], same.revision());
}

pub async fn a_commit_that_touches_nothing_announces_nothing(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("a.txt", "a").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    store.commit(Staging::new()).await.unwrap();
    let mut unchanged = Staging::new();
    unchanged.write("a.txt", "a").unwrap();
    store.commit(unchanged).await.unwrap();
    let mut absent = Staging::new();
    absent.delete("absent.txt").unwrap();
    absent.delete_prefix("nothing/").unwrap();
    store.commit(absent).await.unwrap();

    assert_nothing_more(&mut feed).await;
}

pub async fn a_read_file_was_last_modified_by_its_commit(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let before = Timestamp::now();
    let mut staging = Staging::new();
    staging.write("notes/today.md", "# Today\n").unwrap();
    store.commit(staging).await.unwrap();
    let after = Timestamp::now();

    let file = read(&store, "notes/today.md").await;
    assert!(
        before <= file.modified() && file.modified() <= after,
        "{} should be between {before} and {after}",
        file.modified(),
    );
}

pub async fn a_revision_is_decided_by_the_contents_alone(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new();
    staging.write("a.txt", "same").unwrap();
    staging.write("b.txt", "same").unwrap();
    staging.write("c.txt", "different").unwrap();
    store.commit(staging).await.unwrap();

    let a = read(&store, "a.txt").await;
    let b = read(&store, "b.txt").await;
    let c = read(&store, "c.txt").await;
    assert_eq!(a.revision(), b.revision());
    assert_ne!(a.revision(), c.revision());

    let mut staging = Staging::new();
    staging.write("a.txt", "different").unwrap();
    store.commit(staging).await.unwrap();
    assert_eq!(read(&store, "a.txt").await.revision(), c.revision());
}

pub async fn a_revision_written_as_text_reads_back_as_itself(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let mut staging = Staging::new();
    staging.write("a.txt", "some text").unwrap();
    store.commit(staging).await.unwrap();

    let revision = read(&store, "a.txt").await.revision();
    let text = revision.to_string();
    assert_eq!(text.len(), 32);
    assert!(text.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)), "{text}");
    assert_eq!(text.parse::<Revision>().unwrap(), revision);
    assert_eq!(text.to_uppercase().parse::<Revision>().unwrap(), revision);

    for invalid in ["", "abc", &text[1..], &format!("{text}0"), &format!("+{}", &text[1..])] {
        assert!(invalid.parse::<Revision>().is_err(), "{invalid:?} parsed");
    }
    assert!(format!("{}g", &text[1..]).parse::<Revision>().is_err());
}

pub async fn a_prefix_revision_is_written_as_its_hash(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;

    let empty = store.stat_prefix("notes/").await.unwrap().to_string();
    let mut staging = Staging::new();
    staging.write("notes/a.txt", "a").unwrap();
    store.commit(staging).await.unwrap();
    let written = store.stat_prefix("notes/").await.unwrap().to_string();

    assert_eq!(written.len(), 32);
    assert!(written.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_ne!(empty, written);
    assert_eq!(store.stat_prefix("notes/").await.unwrap().to_string(), written);
}

pub async fn a_commit_announces_one_batch_of_local_changes(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;

    let mut staging = Staging::new();
    staging.write("settings.toml", "a = 1\n").unwrap();
    staging.write("themes/dark.toml", "b = 2\n").unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(
        changes_in_full(&next_batch(&mut feed).await),
        [
            ("settings.toml", ChangeKind::Changed, Origin::Local),
            ("themes/dark.toml", ChangeKind::Changed, Origin::Local),
        ],
    );
    assert_nothing_more(&mut feed).await;
}

pub async fn a_staging_is_built_without_the_store(fixture: &impl Fixture) {
    let mut staging = Staging::new();
    staging.write("built-first.txt", "before the Store was opened").unwrap();

    let Opened { store, feed: _feed } = fixture.open().await;
    store.commit(staging).await.unwrap();

    let file = read(&store, "built-first.txt").await;
    assert_eq!(file.contents(), "before the Store was opened");
}

pub async fn a_dropped_staging_writes_nothing(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;

    let mut dropped = Staging::new();
    dropped.write("abandoned.toml", "never = true\n").unwrap();
    drop(dropped);

    let mut committed = Staging::new();
    committed.write("kept.toml", "kept = true\n").unwrap();
    store.commit(committed).await.unwrap();

    assert_eq!(store.read("abandoned.toml").await.unwrap(), None);
    let batch = next_batch(&mut feed).await;
    let paths: Vec<_> = batch.iter().map(|change| change.path.as_str()).collect();
    assert_eq!(paths, ["kept.toml"]);
}

pub async fn an_invalid_path_is_refused_wherever_it_is_used(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let snapshot =
        if store.supports_snapshots() { Some(store.snapshot().await.unwrap()) } else { None };

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
        let mut staging = Staging::new();
        assert_refused("writing", path, expected, staging.write(path, "x").map(drop));
        assert_refused("deleting", path, expected, staging.delete(path).map(drop));
        assert_refused("reading", path, expected, store.read(path).await.map(drop));
        assert_refused("stat of", path, expected, store.stat(path).await.map(drop));
        let requiring = staging.require(path, Precondition::Absent).map(drop);
        assert_refused("requiring", path, expected, requiring);
        let writing = staging.write_requiring(path, "x", Precondition::Absent).map(drop);
        assert_refused("writing", path, expected, writing);
        let deleting = staging.delete_requiring(path, Precondition::Absent).map(drop);
        assert_refused("deleting", path, expected, deleting);
        if let Some(snapshot) = &snapshot {
            let reading = snapshot.read(path).await.map(drop);
            assert_refused("reading a Snapshot at", path, expected, reading);
            let stat = snapshot.stat(path).await.map(drop);
            assert_refused("stat through a Snapshot of", path, expected, stat);
        }
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
        let mut staging = Staging::new();
        let deleting = staging.delete_prefix(prefix).map(drop);
        assert_refused("deleting under", prefix, expected, deleting);
        assert_refused("listing", prefix, expected, store.list(prefix).await.map(drop));
        let stat = store.stat_prefix(prefix).await;
        let prefix_revision = store.stat_prefix("").await.unwrap();
        assert_refused("stat of", prefix, expected, stat.map(drop));
        let requiring = staging.require_prefix(prefix, prefix_revision).map(drop);
        assert_refused("requiring", prefix, expected, requiring);
        if let Some(snapshot) = &snapshot {
            let listing = snapshot.list(prefix).await.map(drop);
            assert_refused("listing a Snapshot under", prefix, expected, listing);
        }
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
    let mut staging = Staging::new();
    for path in allowed {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();

    for path in allowed {
        let file = read(&store, path).await;
        assert_eq!((file.path().as_str(), file.contents()), (path, path));
    }
}

pub async fn a_write_requiring_absence_creates_a_file_only_if_there_is_none(
    fixture: &impl Fixture,
) {
    let Opened { store, mut feed } = fixture.open().await;

    let mut create = Staging::new();
    create.write_requiring("id.txt", "first", Precondition::Absent).unwrap();
    store.commit(create).await.unwrap();
    next_batch(&mut feed).await;

    let mut create_again = Staging::new();
    create_again.write_requiring("id.txt", "second", Precondition::Absent).unwrap();
    create_again.write("other.txt", "other").unwrap();
    assert_conflict(store.commit(create_again).await, &["id.txt"]);

    assert_eq!(read(&store, "id.txt").await.contents(), "first");
    assert_eq!(store.read("other.txt").await.unwrap(), None);
    assert_nothing_more(&mut feed).await;
}

pub async fn writing_back_a_file_requires_it_unchanged_since_it_was_read(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("settings.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();

    // Someone else changes the File between our read and our write.
    let ours = read(&store, "settings.toml").await;
    let mut theirs = Staging::new();
    theirs.write("settings.toml", "a = 2\n").unwrap();
    store.commit(theirs).await.unwrap();
    let mut write_back = Staging::new();
    write_back.write_back(&ours, "a = 1\nb = 1\n");
    assert_conflict(store.commit(write_back).await, &["settings.toml"]);
    assert_eq!(read(&store, "settings.toml").await.contents(), "a = 2\n");

    // Read again, it goes through, and the Revision it gives back is good for the next write.
    let ours = read(&store, "settings.toml").await;
    let mut write_back = Staging::new();
    write_back.write_back(&ours, "a = 2\nb = 1\n");
    let committed = store.commit(write_back).await.unwrap();
    let mut again = Staging::new();
    let revision = committed.revisions()[ours.path()];
    again.write_requiring(ours.path(), "a = 3\n", Precondition::UnchangedSince(revision)).unwrap();
    store.commit(again).await.unwrap();
    assert_eq!(read(&store, "settings.toml").await.contents(), "a = 3\n");
}

pub async fn a_delete_can_require_the_file_unchanged(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("a.txt", "a").unwrap();
    store.commit(staging).await.unwrap();
    let read_a = read(&store, "a.txt").await;

    let mut change = Staging::new();
    change.write("a.txt", "changed").unwrap();
    store.commit(change).await.unwrap();
    let mut delete = Staging::new();
    delete.delete_requiring("a.txt", Precondition::UnchangedSince(read_a.revision())).unwrap();
    assert_conflict(store.commit(delete).await, &["a.txt"]);
    assert_eq!(read(&store, "a.txt").await.contents(), "changed");

    // Changed back, the contents are what they were, so it counts as unchanged.
    let mut change_back = Staging::new();
    change_back.write("a.txt", "a").unwrap();
    store.commit(change_back).await.unwrap();
    let mut delete = Staging::new();
    delete.delete_requiring("a.txt", Precondition::UnchangedSince(read_a.revision())).unwrap();
    store.commit(delete).await.unwrap();
    assert_eq!(store.read("a.txt").await.unwrap(), None);

    // Absent holds for a delete of a Path with no File, which does nothing.
    let mut delete = Staging::new();
    delete.delete_requiring("a.txt", Precondition::Absent).unwrap();
    store.commit(delete).await.unwrap();
}

pub async fn a_rename_that_conflicts_leaves_both_paths_as_they_were(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("old-name.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();
    let file = read(&store, "old-name.toml").await;
    let mut change = Staging::new();
    change.write("old-name.toml", "a = 2\n").unwrap();
    store.commit(change).await.unwrap();
    next_batch(&mut feed).await;

    let mut rename = Staging::new();
    rename.delete_requiring(file.path(), Precondition::UnchangedSince(file.revision())).unwrap();
    rename.write_requiring("new-name.toml", file.contents(), Precondition::Absent).unwrap();
    assert_conflict(store.commit(rename).await, &["old-name.toml"]);

    assert_eq!(list(&store, "").await, ["old-name.toml"]);
    assert_eq!(read(&store, "old-name.toml").await.contents(), "a = 2\n");
    assert_nothing_more(&mut feed).await;
}

pub async fn a_commit_can_require_a_file_it_does_not_write(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("rates.toml", "rate = 2\n").unwrap();
    store.commit(staging).await.unwrap();

    // Prices are worked out from the rates, so they are only written if the rates haven't moved.
    let rates = read(&store, "rates.toml").await;
    let unchanged = Precondition::UnchangedSince(rates.revision());
    let mut prices = Staging::new();
    prices.require(rates.path(), unchanged).unwrap();
    prices.require("lock.txt", Precondition::Absent).unwrap();
    prices.write("prices.toml", "price = 20\n").unwrap();
    store.commit(prices).await.unwrap();

    let mut change = Staging::new();
    change.write("rates.toml", "rate = 3\n").unwrap();
    change.write("lock.txt", "").unwrap();
    store.commit(change).await.unwrap();
    let mut prices = Staging::new();
    prices.require(rates.path(), unchanged).unwrap();
    prices.require("lock.txt", Precondition::Absent).unwrap();
    prices.write("prices.toml", "price = 30\n").unwrap();
    assert_conflict(store.commit(prices).await, &["lock.txt", "rates.toml"]);

    assert_eq!(read(&store, "prices.toml").await.contents(), "price = 20\n");
    assert_eq!(read(&store, "rates.toml").await.contents(), "rate = 3\n");
}

pub async fn a_prefix_revision_changes_when_a_file_under_the_prefix_does(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let empty_store = store.stat_prefix("").await.unwrap();
    let mut staging = Staging::new();
    staging.write("themes/dark.toml", "dark").unwrap();
    staging.write("settings.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();
    let themes = store.stat_prefix("themes/").await.unwrap();
    let whole_store = store.stat_prefix("").await.unwrap();
    assert_ne!(whole_store, empty_store);

    let mut steps: Vec<(&str, Staging)> = Vec::new();
    let mut added = Staging::new();
    added.write("themes/deep/light.toml", "light").unwrap();
    steps.push(("added", added));
    let mut changed = Staging::new();
    changed.write("themes/dark.toml", "darker").unwrap();
    steps.push(("changed", changed));
    let mut removed = Staging::new();
    removed.delete("themes/dark.toml").unwrap();
    steps.push(("removed", removed));
    let mut seen = vec![themes.clone()];
    for (what, staging) in steps {
        store.commit(staging).await.unwrap();
        let now = store.stat_prefix("themes/").await.unwrap();
        assert!(!seen.contains(&now), "a File under the Prefix was {what}");
        seen.push(now);
    }

    // Put back as it was, it is as it was. Changes outside the Prefix don't count.
    let mut back = Staging::new();
    back.write("themes/dark.toml", "dark").unwrap();
    back.delete("themes/deep/light.toml").unwrap();
    back.write("themes2/dark.toml", "outside").unwrap();
    store.commit(back).await.unwrap();
    assert_eq!(store.stat_prefix("themes/").await.unwrap(), themes);

    // The empty Prefix covers the whole Store.
    assert_ne!(store.stat_prefix("").await.unwrap(), whole_store);
}

pub async fn a_prefix_precondition_fails_when_a_file_is_added_under_the_prefix(
    fixture: &impl Fixture,
) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("inbox/1.eml", "one").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    // The index is worked out from everything in the inbox, so it holds only if nothing arrived.
    let inbox = store.stat_prefix("inbox/").await.unwrap();
    let mut index = Staging::new();
    index.require_prefix("inbox/", inbox.clone()).unwrap();
    index.write("index.txt", "1.eml").unwrap();
    let mut arrival = Staging::new();
    arrival.write("inbox/2.eml", "two").unwrap();
    store.commit(arrival).await.unwrap();
    next_batch(&mut feed).await;
    assert_conflict(store.commit(index).await, &["inbox/2.eml"]);
    assert_eq!(store.read("index.txt").await.unwrap(), None);
    assert_nothing_more(&mut feed).await;

    // With a Prefix Revision taken after the arrival, and changes only outside the Prefix, it
    // goes through.
    let inbox = store.stat_prefix("inbox/").await.unwrap();
    let mut outside = Staging::new();
    outside.write("inbox2/3.eml", "three").unwrap();
    store.commit(outside).await.unwrap();
    let mut index = Staging::new();
    index.require_prefix("inbox/", inbox).unwrap();
    index.write("index.txt", "1.eml 2.eml").unwrap();
    store.commit(index).await.unwrap();
    assert_eq!(read(&store, "index.txt").await.contents(), "1.eml 2.eml");
}

pub async fn a_prefix_conflict_names_the_paths_added_removed_or_changed(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    for path in ["a/changed.txt", "a/kept.txt", "a/removed.txt", "b.txt"] {
        staging.write(path, path).unwrap();
    }
    store.commit(staging).await.unwrap();
    let under_a = store.stat_prefix("a/").await.unwrap();
    let whole_store = store.stat_prefix("").await.unwrap();

    let mut changes = Staging::new();
    changes.write("a/added.txt", "added").unwrap();
    changes.write("a/changed.txt", "changed").unwrap();
    changes.delete("a/removed.txt").unwrap();
    changes.write("b.txt", "changed outside a/").unwrap();
    store.commit(changes).await.unwrap();

    let mut staging = Staging::new();
    staging.require_prefix("a/", under_a).unwrap();
    staging.write("summary.txt", "").unwrap();
    assert_conflict(
        store.commit(staging).await,
        &["a/added.txt", "a/changed.txt", "a/removed.txt"],
    );
    let mut staging = Staging::new();
    staging.require_prefix("", whole_store).unwrap();
    staging.write("summary.txt", "").unwrap();
    assert_conflict(
        store.commit(staging).await,
        &["a/added.txt", "a/changed.txt", "a/removed.txt", "b.txt"],
    );
    assert_eq!(store.read("summary.txt").await.unwrap(), None);
}

pub async fn a_precondition_stays_when_something_staged_later_replaces_it(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("drafts/a.md", "a").unwrap();
    staging.write("notes.md", "notes").unwrap();
    store.commit(staging).await.unwrap();
    let draft = read(&store, "drafts/a.md").await;
    let notes = read(&store, "notes.md").await;
    let staged_later = |staging: &mut Staging| {
        staging.write_back(&draft, "edited");
        staging.delete_prefix("drafts/").unwrap();
        staging.write_back(&notes, "edited");
        staging.write("notes.md", "replaced").unwrap();
    };

    let mut change = Staging::new();
    change.write("drafts/a.md", "changed").unwrap();
    change.write("notes.md", "changed").unwrap();
    store.commit(change).await.unwrap();
    let mut staging = Staging::new();
    staged_later(&mut staging);
    assert_conflict(store.commit(staging).await, &["drafts/a.md", "notes.md"]);
    assert_eq!(list(&store, "").await, ["drafts/a.md", "notes.md"]);

    // Once they hold again, what was staged later is what happens.
    let mut change_back = Staging::new();
    change_back.write("drafts/a.md", "a").unwrap();
    change_back.write("notes.md", "notes").unwrap();
    store.commit(change_back).await.unwrap();
    let mut staging = Staging::new();
    staged_later(&mut staging);
    store.commit(staging).await.unwrap();
    assert_eq!(list(&store, "").await, ["notes.md"]);
    assert_eq!(read(&store, "notes.md").await.contents(), "replaced");
}

pub async fn a_staging_without_preconditions_depends_on_nothing(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("a.toml", "a = 1\n").unwrap();
    staging.write("b.toml", "b = 1\n").unwrap();
    store.commit(staging).await.unwrap();

    // Staged after reading, then everything changes before the Commit.
    let mut staging = Staging::new();
    staging.write("a.toml", "a = 3\n").unwrap();
    staging.delete("b.toml").unwrap();
    staging.write("c.toml", "c = 3\n").unwrap();
    let mut change = Staging::new();
    change.write("a.toml", "a = 2\n").unwrap();
    change.write("b.toml", "b = 2\n").unwrap();
    change.write("c.toml", "c = 2\n").unwrap();
    store.commit(change).await.unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(list(&store, "").await, ["a.toml", "c.toml"]);
    assert_eq!(read(&store, "a.toml").await.contents(), "a = 3\n");
    assert_eq!(read(&store, "c.toml").await.contents(), "c = 3\n");
}

pub async fn a_path_differing_only_in_letter_case_is_refused(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
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
        let mut staging = Staging::new();
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
    assert_eq!(list(&store, "").await, ["Settings.toml", "themes/dark.toml"]);
    assert_nothing_more(&mut feed).await;

    // Writing a File again in its own letter case is fine, and so is renaming it to another letter
    // case in one Commit, since only one of them is left.
    let mut staging = Staging::new();
    staging.write("Settings.toml", "a = 2\n").unwrap();
    staging.write("themes/light.toml", "light").unwrap();
    store.commit(staging).await.unwrap();
    let mut rename = Staging::new();
    rename.delete("Settings.toml").unwrap();
    rename.write("settings.toml", "a = 2\n").unwrap();
    rename.delete_prefix("themes/").unwrap();
    rename.write("Themes/dark.toml", "dark").unwrap();
    store.commit(rename).await.unwrap();
    assert_eq!(list(&store, "").await, ["Themes/dark.toml", "settings.toml"]);
}

pub async fn a_file_cannot_be_under_another_file(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let mut staging = Staging::new();
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
        let mut staging = Staging::new();
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

    // A Path staged again after a Prefix delete keeps the Prefix there.
    let mut staging = Staging::new();
    staging.delete_prefix("d/").unwrap();
    staging.write("d/e", "e").unwrap();
    staging.write("d", "d").unwrap();
    match store.commit(staging).await {
        Err(Error::InvalidPath { path, reason: FileUnderFile }) => {
            assert!(["d", "d/e"].contains(&path.as_str()), "refused for {path:?}");
        }
        other => panic!("`d` beside a re-staged `d/e` should be refused, got {other:?}"),
    }

    // Preconditions are checked first, so a Commit that also breaks one is a Conflict.
    let mut staging = Staging::new();
    staging.require("a", Precondition::Absent).unwrap();
    staging.write("a/b", "clash").unwrap();
    assert_conflict(store.commit(staging).await, &["a"]);

    assert_eq!(list(&store, "").await, ["a", "d/e"]);
    assert_nothing_more(&mut feed).await;

    // A Path deleted in the same Commit doesn't count, so a File can be moved under its own name,
    // or to where the Files under it were.
    let mut rename = Staging::new();
    rename.delete("a").unwrap();
    rename.write("a/b", "a").unwrap();
    rename.delete_prefix("d/").unwrap();
    rename.write("d", "e").unwrap();
    store.commit(rename).await.unwrap();
    assert_eq!(list(&store, "").await, ["a/b", "d"]);
}

/// A Prefix Revision required for another Prefix, or in a Commit to another Store, could never
/// hold: that is a mistake in the app, not a Conflict, so the Commit is refused with an error of
/// its own, and writes nothing. Another Store is any other one opened, even on the same Location,
/// but a clone is the same Store.
pub async fn a_prefix_revision_is_only_for_its_own_store_and_prefix(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    let Opened { store: other, feed: _other_feed } = fixture.open().await;
    let themes = store.stat_prefix("themes/").await.unwrap();
    let requiring = |prefix: &str| {
        let mut staging = Staging::new();
        staging.require_prefix(prefix, themes.clone()).unwrap();
        staging.write("written.txt", "").unwrap();
        staging
    };

    for prefix in ["fonts/", ""] {
        let refused = store.commit(requiring(prefix)).await;
        assert_wrong_prefix_revision(refused, prefix);
    }
    assert_wrong_prefix_revision(other.commit(requiring("themes/")).await, "themes/");
    assert_eq!(store.read("written.txt").await.unwrap(), None);
    assert_eq!(other.read("written.txt").await.unwrap(), None);

    store.clone().commit(requiring("themes/")).await.unwrap();
    assert_eq!(read(&store, "written.txt").await.contents(), "");
}

/// Checks that `committed` was refused for a Prefix Revision required for `prefix` that wasn't
/// taken from the Store for that Prefix.
fn assert_wrong_prefix_revision(committed: tidings::Result<Committed>, prefix: &str) {
    match committed {
        Err(Error::WrongPrefixRevision { prefix: refused }) => {
            assert_eq!(refused.as_str(), prefix);
        }
        other => panic!("expected WrongPrefixRevision for {prefix:?}, got {other:?}"),
    }
}

pub async fn unread_changes_are_merged_per_path_and_the_latest_kind_wins(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;

    // Nothing is read until every Commit is made.
    let mut staging = Staging::new();
    staging.write("removed-last.txt", "1").unwrap();
    staging.write("changed-last.txt", "1").unwrap();
    store.commit(staging).await.unwrap();
    let mut staging = Staging::new();
    staging.delete("removed-last.txt").unwrap();
    staging.delete("changed-last.txt").unwrap();
    store.commit(staging).await.unwrap();
    let mut staging = Staging::new();
    staging.write("changed-last.txt", "2").unwrap();
    store.commit(staging).await.unwrap();
    // Many Commits to one Path are one Change, not one each.
    for i in 0..1000 {
        let mut staging = Staging::new();
        staging.write("often.txt", i.to_string()).unwrap();
        store.commit(staging).await.unwrap();
    }

    assert_eq!(
        changes_in_full(&next_batch(&mut feed).await),
        [
            ("changed-last.txt", ChangeKind::Changed, Origin::Local),
            ("often.txt", ChangeKind::Changed, Origin::Local),
            ("removed-last.txt", ChangeKind::Removed, Origin::Local),
        ],
    );
    assert_nothing_more(&mut feed).await;
}

pub async fn clones_of_a_store_share_its_files_and_its_change_feed(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    let clone = store.clone();

    // A clone in another task commits, and the original sees the File and the Change.
    let task = tokio::spawn(async move {
        let mut staging = Staging::new();
        staging.write("from-clone.toml", "a = 1\n").unwrap();
        clone.commit(staging).await.unwrap();
        clone
    });
    let clone = task.await.unwrap();
    assert_eq!(read(&store, "from-clone.toml").await.contents(), "a = 1\n");
    assert_eq!(changes(&next_batch(&mut feed).await), [("from-clone.toml", ChangeKind::Changed)]);

    // With the original gone, the clone keeps the Change feed going.
    drop(store);
    assert_nothing_more(&mut feed).await;
    let mut staging = Staging::new();
    staging.write("after-drop.toml", "b = 1\n").unwrap();
    clone.commit(staging).await.unwrap();
    assert_eq!(changes(&next_batch(&mut feed).await), [("after-drop.toml", ChangeKind::Changed)]);

    // Once the last handle is gone, what was recorded still arrives, then the feed ends.
    let mut staging = Staging::new();
    staging.write("last.toml", "c = 1\n").unwrap();
    clone.commit(staging).await.unwrap();
    drop(clone);
    assert_eq!(changes(&next_batch(&mut feed).await), [("last.toml", ChangeKind::Changed)]);
    assert_ended(&mut feed).await;
}

pub async fn the_store_keeps_working_once_its_change_feed_is_dropped(fixture: &impl Fixture) {
    let Opened { store, feed } = fixture.open().await;
    let mut staging = Staging::new();
    staging.write("before.txt", "unread").unwrap();
    store.commit(staging).await.unwrap();
    drop(feed);

    for i in 0..100 {
        let mut staging = Staging::new();
        staging.write(format!("after/{i}.txt"), "a").unwrap();
        staging.delete("before.txt").unwrap();
        store.commit(staging).await.unwrap();
    }
    let mut staging = Staging::new();
    staging.delete_prefix("after/").unwrap();
    staging.write("last.txt", "last").unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(list(&store, "").await, ["last.txt"]);
    assert_eq!(read(&store, "last.txt").await.contents(), "last");
}

pub async fn a_commit_made_before_the_feed_is_first_read_is_reported(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;

    // Straight after opening, from a clone, with the feed not yet read at all.
    let clone = store.clone();
    let mut staging = Staging::new();
    staging.write("first.txt", "first").unwrap();
    clone.commit(staging).await.unwrap();

    assert_eq!(
        changes_in_full(&next_batch(&mut feed).await),
        [("first.txt", ChangeKind::Changed, Origin::Local)],
    );
    assert_nothing_more(&mut feed).await;
}

pub async fn a_commits_changes_are_never_split_across_batches(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    // Many Files per Commit, so that a Commit recorded in parts would likely be read in parts.
    const FILES: usize = 20;

    // A thread of its own reads batches as they come while tasks commit, until the feed ends. On
    // its own thread, it can wake while a Commit is being recorded, rather than after.
    let reader = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async move {
            let mut batches = Vec::new();
            while let Some(item) = feed.next().await {
                match item {
                    FeedItem::Changes(batch) => batches.push(batch),
                    other => panic!("expected a batch of Changes, got {other:?}"),
                }
            }
            batches
        })
    });
    let committers: Vec<_> = (0..4)
        .map(|task| {
            let store = store.clone();
            tokio::spawn(async move {
                for commit in 0..25 {
                    let mut staging = Staging::new();
                    for file in 0..FILES {
                        staging.write(format!("{task}/{commit}/{file}"), "x").unwrap();
                    }
                    store.commit(staging).await.unwrap();
                    tokio::task::yield_now().await;
                }
            })
        })
        .collect();
    for committer in committers {
        committer.await.unwrap();
    }
    drop(store);
    let batches = reader.join().unwrap();

    // Each Commit wrote its Files under its own Prefix: all of them are in one batch.
    let mut seen = 0;
    for batch in &batches {
        let mut commits = std::collections::BTreeMap::<&str, usize>::new();
        for change in batch {
            let (commit, _file) = change.path.as_str().rsplit_once('/').unwrap();
            *commits.entry(commit).or_default() += 1;
        }
        for (commit, count) in commits {
            assert_eq!(count, FILES, "Commit {commit} was split across batches");
            seen += 1;
        }
    }
    assert_eq!(seen, 4 * 25);
}

pub async fn concurrent_commits_reach_the_feed_in_the_order_they_were_made(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;

    // In each round, tasks race to write and delete one Path. However the Commits land, their
    // merged Change must end as the last of them left the File: changed if it is there, removed
    // if it isn't. Every Commit has reached the feed once it returns, so each round is one batch.
    for round in 0..500 {
        let tasks: Vec<_> = (0..4)
            .map(|task| {
                let store = store.clone();
                tokio::spawn(async move {
                    for step in 0..4 {
                        let mut staging = Staging::new();
                        if (task + step) % 2 == 0 {
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

        let there = store.read("raced.txt").await.unwrap().is_some();
        let expected = if there { ChangeKind::Changed } else { ChangeKind::Removed };
        let batch = next_batch(&mut feed).await;
        assert_eq!(changes(&batch), [("raced.txt", expected)], "in round {round}");
    }
    assert_nothing_more(&mut feed).await;
}

pub async fn a_cancelled_commit_finishes_or_never_happens_and_is_reported_if_it_finishes(
    fixture: &impl Fixture,
) {
    let Opened { store, mut feed } = fixture.open().await;

    // Each Commit's future is polled once, then dropped. A Commit that has started by then
    // finishes anyway, so its File is written and its Change still arrives. One still waiting
    // for the Commits before it never happens. Where a Backend's Commit finishes in one poll, as
    // memory's does, none is left unfinished, and this only checks that each is reported.
    for i in 0..100 {
        let mut staging = Staging::new();
        staging.write(format!("cancelled/{i}.txt"), "x").unwrap();
        if let Poll::Ready(result) = poll_once(std::pin::pin!(store.commit(staging))).await {
            result.unwrap();
        }
    }

    // A Commit dropped while it waits for its turn never happens. While `holding` is kept and
    // not finished, it holds its turn, so `waiting` must wait. Where a Backend's Commit finishes
    // in one poll, as memory's does, `holding` is already done and there is nothing to wait for.
    let mut staging = Staging::new();
    staging.write("cancelled/holding.txt", "x").unwrap();
    let mut holding = std::pin::pin!(store.commit(staging));
    let mut waited = false;
    match poll_once(holding.as_mut()).await {
        Poll::Ready(result) => drop(result.unwrap()),
        Poll::Pending => {
            let mut staging = Staging::new();
            staging.write("cancelled/waiting.txt", "x").unwrap();
            let waiting = poll_once(std::pin::pin!(store.commit(staging))).await;
            assert!(waiting.is_pending(), "a Commit can't finish while another holds its turn");
            holding.await.unwrap();
            waited = true;
        }
    }
    // Commits are applied in turn, so once this one returns, every one that started has
    // finished.
    let mut staging = Staging::new();
    staging.write("last.txt", "x").unwrap();
    store.commit(staging).await.unwrap();

    let mut reported = BTreeSet::new();
    while !reported.contains("last.txt") {
        for change in next_batch(&mut feed).await {
            assert_eq!(change.kind, ChangeKind::Changed);
            reported.insert(change.path.as_str().to_owned());
        }
    }
    assert_nothing_more(&mut feed).await;
    reported.remove("last.txt");
    let written: BTreeSet<_> = list(&store, "cancelled/").await.into_iter().collect();
    assert_eq!(reported, written, "every Commit that happened, and only those, is reported");
    assert!(written.contains("cancelled/holding.txt"));
    if waited {
        assert!(!written.contains("cancelled/waiting.txt"), "a Commit dropped waiting happened");
    }
}

pub async fn a_snapshot_reads_the_store_as_it_was_when_taken(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    if !store.supports_snapshots() {
        return;
    }
    let mut staging = Staging::new();
    staging.write("changed.txt", "before").unwrap();
    staging.write("removed.txt", "removed").unwrap();
    staging.write("dir/kept.txt", "kept").unwrap();
    store.commit(staging).await.unwrap();
    let changed_before = read(&store, "changed.txt").await;
    let removed_before = read(&store, "removed.txt").await;

    let snapshot = store.snapshot().await.unwrap();
    let mut staging = Staging::new();
    staging.write("changed.txt", "after").unwrap();
    staging.delete("removed.txt").unwrap();
    staging.write("dir/added.txt", "added").unwrap();
    store.commit(staging).await.unwrap();

    // The Snapshot still gives the Store as it was, with each File's time and Revision.
    assert_eq!(snapshot.read("changed.txt").await.unwrap(), Some(changed_before.clone()));
    assert_eq!(snapshot.read("removed.txt").await.unwrap(), Some(removed_before.clone()));
    assert_eq!(snapshot.read("dir/added.txt").await.unwrap(), None);
    let stat = snapshot.stat("changed.txt").await.unwrap().unwrap();
    assert_eq!(
        (stat.modified(), stat.revision()),
        (changed_before.modified(), changed_before.revision()),
    );
    assert_eq!(snapshot.stat("dir/added.txt").await.unwrap(), None);
    assert_eq!(
        as_strings(&snapshot.list("").await.unwrap()),
        ["changed.txt", "dir/kept.txt", "removed.txt"]
    );
    assert_eq!(as_strings(&snapshot.list("dir/").await.unwrap()), ["dir/kept.txt"]);

    // The Store gives its Files as they are now.
    assert_eq!(read(&store, "changed.txt").await.contents(), "after");
    assert_eq!(store.read("removed.txt").await.unwrap(), None);
    assert_eq!(list(&store, "dir/").await, ["dir/added.txt", "dir/kept.txt"]);
}

pub async fn reads_through_a_snapshot_never_mix_commits(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    if !store.supports_snapshots() {
        return;
    }
    // Every Commit writes the same number to both Files, so they belong together.
    let commit_pair = async |store: &Store, n: usize| {
        let mut staging = Staging::new();
        staging.write("pair/a.txt", n.to_string()).unwrap();
        staging.write("pair/b.txt", n.to_string()).unwrap();
        store.commit(staging).await.unwrap();
    };
    let read_pair = async |snapshot: &Snapshot| {
        let a = snapshot.read("pair/a.txt").await.unwrap().unwrap();
        tokio::task::yield_now().await;
        let b = snapshot.read("pair/b.txt").await.unwrap().unwrap();
        (a.contents().to_owned(), b.contents().to_owned())
    };
    commit_pair(&store, 0).await;

    // A Commit lands between reading one File and the other.
    let snapshot = store.snapshot().await.unwrap();
    let a = snapshot.read("pair/a.txt").await.unwrap().unwrap();
    commit_pair(&store, 1).await;
    let b = snapshot.read("pair/b.txt").await.unwrap().unwrap();
    assert_eq!((a.contents(), b.contents()), ("0", "0"));

    // Commits land from another task while Snapshots are taken and read.
    const COMMITS: usize = 500;
    let committer = {
        let store = store.clone();
        tokio::spawn(async move {
            for n in 2..COMMITS {
                commit_pair(&store, n).await;
                tokio::task::yield_now().await;
            }
        })
    };
    while !committer.is_finished() {
        let snapshot = store.snapshot().await.unwrap();
        let (a, b) = read_pair(&snapshot).await;
        assert_eq!(a, b, "a Snapshot mixed two Commits");
    }
    committer.await.unwrap();
    let last = (COMMITS - 1).to_string();
    let snapshot = store.snapshot().await.unwrap();
    assert_eq!(read_pair(&snapshot).await, (last.clone(), last));
}

pub async fn holding_a_snapshot_does_not_hold_up_commits(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    if !store.supports_snapshots() {
        return;
    }
    let mut staging = Staging::new();
    staging.write("held.txt", "before").unwrap();
    store.commit(staging).await.unwrap();

    // One task keeps reading through the Snapshot while another commits to the Store.
    let snapshot = store.snapshot().await.unwrap();
    let committer = {
        let store = store.clone();
        tokio::spawn(async move {
            for i in 0..20 {
                let mut staging = Staging::new();
                staging.write("held.txt", format!("after {i}")).unwrap();
                staging.write(format!("new/{i}.txt"), "new").unwrap();
                store.commit(staging).await.unwrap();
            }
        })
    };
    let reading = async {
        while !committer.is_finished() {
            let file = snapshot.read("held.txt").await.unwrap().unwrap();
            assert_eq!(file.contents(), "before");
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), reading)
        .await
        .expect("Commits should not wait for the Snapshot");
    committer.await.unwrap();

    assert_eq!(read(&store, "held.txt").await.contents(), "after 19");
    assert_eq!(as_strings(&snapshot.list("").await.unwrap()), ["held.txt"]);
}

pub async fn a_snapshot_outlives_the_store_without_keeping_the_feed_open(fixture: &impl Fixture) {
    let Opened { store, mut feed } = fixture.open().await;
    if !store.supports_snapshots() {
        return;
    }
    let mut staging = Staging::new();
    staging.write("settings.toml", "a = 1\n").unwrap();
    store.commit(staging).await.unwrap();
    next_batch(&mut feed).await;

    let snapshot = store.snapshot().await.unwrap();
    drop(store);
    assert_ended(&mut feed).await;
    let file = snapshot.read("settings.toml").await.unwrap().unwrap();
    assert_eq!(file.contents(), "a = 1\n");
    assert_eq!(as_strings(&snapshot.list("").await.unwrap()), ["settings.toml"]);
}

pub async fn a_backend_without_snapshots_refuses_one(fixture: &impl Fixture) {
    let Opened { store, feed: _feed } = fixture.open().await;
    // Skipped where Snapshots are supported: the tests above cover them there.
    if store.supports_snapshots() {
        return;
    }

    match store.snapshot().await {
        Err(Error::Unsupported) => {}
        other => panic!("a Snapshot should be unsupported, got {other:?}"),
    }
}

// Helpers shared by the tests above.

/// Reads a File that the test expects to exist.
async fn read(store: &Store, path: &str) -> File {
    store.read(path).await.unwrap().unwrap_or_else(|| panic!("{path} should exist"))
}

/// Lists the Paths under `prefix`, as strings.
async fn list(store: &Store, prefix: &str) -> Vec<String> {
    as_strings(&store.list(prefix).await.unwrap())
}

/// Paths as strings, for comparing.
fn as_strings(paths: &[Path]) -> Vec<String> {
    paths.iter().map(|path| path.as_str().to_owned()).collect()
}

/// Polls `future` once, and gives what that gave.
async fn poll_once<F: Future>(mut future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
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
