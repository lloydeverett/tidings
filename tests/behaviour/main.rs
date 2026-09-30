//! The behaviour suite: one set of tests, written once against the public API, that every Backend
//! must pass. Each Backend instantiates the whole suite in its own module below, once through the
//! async API and once, in a `blocking` module of its own, through the blocking API.

mod api;
#[path = "../common/mod.rs"]
mod common;
#[cfg(all(feature = "fs", feature = "sqlite"))]
mod markers;
#[cfg(any(feature = "fs", feature = "sqlite"))]
#[macro_use]
mod separate_stores;
#[macro_use]
mod suite;
#[cfg(any(feature = "fs", feature = "sqlite"))]
#[macro_use]
mod two_stores;

mod memory {
    use crate::suite::{Fixture, Opened};

    struct Memory;

    impl Fixture for Memory {
        async fn open(&self) -> Opened {
            tidings::Store::open_memory().into()
        }
    }

    behaviour_suite!(Memory);

    mod blocking {
        use crate::api::call_blocking;
        use crate::suite::{Fixture, Opened};

        struct BlockingMemory;

        impl Fixture for BlockingMemory {
            async fn open(&self) -> Opened {
                call_blocking(tidings::blocking::Store::open_memory).into()
            }
        }

        behaviour_suite!(BlockingMemory);
    }

    /// The suite's Snapshot tests run only where Snapshots are supported, so this makes sure they
    /// run on memory.
    #[test]
    fn memory_supports_snapshots() {
        let (store, _feed) = tidings::Store::open_memory();
        assert!(store.supports_snapshots());
    }
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use std::path::PathBuf;
    use std::time::Duration;

    use tempfile::TempDir;
    use tidings::{
        BackendKind, ChangeKind, Error, FeedItem, Origin, Precondition, SqliteOptions, Staging,
    };

    use crate::api::Store;
    use crate::common::{assert_nothing_more, changes_in_full, next_batch, next_item};
    use crate::separate_stores::{Located, remove_the_location};
    use crate::suite::{Fixture, Opened};

    /// Opens each test's Store at a Location in a temporary directory of its own, removed when
    /// the test ends. Each Store it opens is at the same Location, and checks for the others'
    /// Commits often.
    pub(crate) struct Sqlite {
        directory: TempDir,
    }

    impl Sqlite {
        pub(crate) fn new() -> Sqlite {
            Sqlite { directory: tempfile::tempdir().unwrap() }
        }

        /// The Location each Store is opened at. Opening makes it.
        pub(crate) fn location(&self) -> PathBuf {
            self.directory.path().join("store")
        }

        /// The options each Store is opened with: checking often.
        fn usual_options(&self) -> SqliteOptions {
            SqliteOptions::default().poll_interval(Duration::from_millis(10))
        }

        /// Opens a Store at the Location with the options `options` makes of the usual ones.
        async fn open_with(&self, options: impl FnOnce(SqliteOptions) -> SqliteOptions) -> Opened {
            let options = options(self.usual_options());
            tidings::Store::open_sqlite(self.location(), options).await.unwrap().into()
        }
    }

    impl Fixture for Sqlite {
        async fn open(&self) -> Opened {
            self.open_with(|options| options).await
        }
    }

    impl Located for Sqlite {
        fn location(&self) -> PathBuf {
            Sqlite::location(self)
        }
    }

    behaviour_suite!(Sqlite::new());
    two_stores_suite!(Sqlite::new());
    separate_stores_suite!(Sqlite::new(), Sqlite::new());

    mod blocking {
        use super::Sqlite;
        use crate::api::call_blocking;
        use crate::suite::{Fixture, Opened};

        /// Opens each Store through the blocking API, as [`Sqlite`] does through the async one.
        pub(crate) struct BlockingSqlite(pub(crate) Sqlite);

        impl Fixture for BlockingSqlite {
            async fn open(&self) -> Opened {
                let (location, options) = (self.0.location(), self.0.usual_options());
                call_blocking(|| tidings::blocking::Store::open_sqlite(location, options))
                    .unwrap()
                    .into()
            }
        }

        behaviour_suite!(BlockingSqlite(Sqlite::new()));
        two_stores_suite!(BlockingSqlite(Sqlite::new()));
    }

    /// The suite's Snapshot tests run only where Snapshots are supported, so this makes sure they
    /// run on SQLite.
    #[tokio::test]
    async fn sqlite_supports_snapshots() {
        let Opened { store, feed: _feed } = Sqlite::new().open().await;
        assert!(store.supports_snapshots());
    }

    /// Opening makes the Location, and any of its parents that are missing, with the database in
    /// its `.tidings/`.
    #[tokio::test]
    async fn opening_makes_the_location_and_its_parents() {
        let directory = tempfile::tempdir().unwrap();
        let location = directory.path().join("not/there/yet");
        let (_store, _feed) =
            tidings::Store::open_sqlite(&location, SqliteOptions::default()).await.unwrap();
        assert!(location.join(".tidings/store.sqlite3").is_file());
        let detected = tidings::Store::detect(&location).await.unwrap();
        assert_eq!(detected, Some(BackendKind::Sqlite));
    }

    /// The change log is pruned, so it doesn't grow without limit. A Store the log was pruned
    /// past has missed Commits, and gets a Resync instead of their Changes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_store_that_the_change_log_was_pruned_past_gets_a_resync() {
        let fixture = Sqlite::new();
        // This one reads the log only when it commits, which the test chooses.
        let Opened { store: behind, mut feed } =
            fixture.open_with(|options| options.poll_interval(Duration::from_secs(3600))).await;
        // Each of this one's Commits prunes every Commit before it.
        let Opened { store: pruning, feed: _pruning_feed } =
            fixture.open_with(|options| options.change_log_retention(Duration::ZERO)).await;
        let commit = async |store: &Store, path: &str| {
            let mut staging = Staging::new();
            staging.write(path, "x").unwrap();
            store.commit(staging).await.unwrap();
            // So that the next Commit is later, and prunes this one.
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        // An unread Change, before the other Store commits.
        commit(&behind, "unread.txt").await;
        // The first is pruned by the second.
        commit(&pruning, "pruned.txt").await;
        commit(&pruning, "kept.txt").await;
        // Committing, the Store reads the log, and finds what it hadn't read pruned.
        commit(&behind, "after.txt").await;

        // The Resync takes the place of the unread Changes, and of those recorded after it until
        // it is read.
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        assert_nothing_more(&mut feed).await;

        // Once it is read, Changes are reported again.
        commit(&pruning, "later.txt").await;
        commit(&behind, "again.txt").await;
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [
                ("again.txt", ChangeKind::Changed, Origin::Local),
                ("later.txt", ChangeKind::Changed, Origin::External),
            ],
        );
        assert_nothing_more(&mut feed).await;
    }

    /// A Location may vanish, as a cache's does when the OS clears it. SQLite has its database
    /// open, and would go on using it unseen, so each poll checks that it is still there. Once it
    /// is gone, the Store makes the Location again, marks it and makes the database again, and
    /// gets a Resync, since its Files are gone.
    #[tokio::test]
    async fn a_location_removed_while_running_is_made_again_with_a_resync() {
        let fixture = Sqlite::new();
        let Opened { store, mut feed } = fixture.open().await;
        let mut staging = Staging::new();
        staging.write("thumbnails/a.png", "a").unwrap();
        store.commit(staging).await.unwrap();
        next_batch(&mut feed).await;

        remove_the_location(&fixture.location());
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        assert!(fixture.location().join(".tidings/store.sqlite3").is_file());
        let detected = tidings::Store::detect(fixture.location()).await.unwrap();
        assert_eq!(detected, Some(BackendKind::Sqlite));
        assert_eq!(store.read("thumbnails/a.png").await.unwrap(), None);

        let mut staging = Staging::new();
        staging.write("new.txt", "x").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("new.txt", ChangeKind::Changed, Origin::Local)],
        );
        assert_nothing_more(&mut feed).await;
        // It is the database at the Location: another Store opened there sees the Commit.
        let Opened { store: other, feed: _other_feed } = fixture.open().await;
        assert_eq!(other.read("new.txt").await.unwrap().unwrap().contents(), "x");
    }

    /// A Commit notices a removed Location before polling does. If the Commit then fails, as a
    /// Conflict against the empty database made again, the Resync still comes.
    #[tokio::test]
    async fn a_commit_that_fails_after_finding_the_location_removed_keeps_the_resync() {
        let fixture = Sqlite::new();
        // It never polls, so only its Commits look.
        let Opened { store, mut feed } =
            fixture.open_with(|options| options.poll_interval(Duration::from_secs(3600))).await;
        let mut staging = Staging::new();
        staging.write("a.txt", "a").unwrap();
        let revision = store.commit(staging).await.unwrap().revisions().values().next().copied();
        next_batch(&mut feed).await;

        remove_the_location(&fixture.location());
        let mut staging = Staging::new();
        let unchanged = Precondition::UnchangedSince(revision.unwrap());
        staging.write_requiring("a.txt", "b", unchanged).unwrap();
        assert!(matches!(store.commit(staging).await, Err(Error::Conflict { .. })));

        let mut staging = Staging::new();
        staging.write("b.txt", "b").unwrap();
        store.commit(staging).await.unwrap();
        // The Resync takes the place of the Commit's Change.
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        assert_nothing_more(&mut feed).await;
    }

    /// A Snapshot, and a read, notice a removed Location before polling does too: the Snapshot is
    /// of the database made again, and the read reads it, rather than the removed one.
    #[tokio::test]
    async fn a_snapshot_and_a_read_after_the_location_is_removed_see_the_database_made_again() {
        let fixture = Sqlite::new();
        // It never polls, so only its Snapshots, reads and Commits look.
        let Opened { store, mut feed } =
            fixture.open_with(|options| options.poll_interval(Duration::from_secs(3600))).await;
        let mut staging = Staging::new();
        staging.write("a.txt", "a").unwrap();
        store.commit(staging).await.unwrap();
        next_batch(&mut feed).await;

        remove_the_location(&fixture.location());
        let snapshot = store.snapshot().await.unwrap();
        assert_eq!(snapshot.read("a.txt").await.unwrap(), None);
        assert!(fixture.location().join(".tidings/store.sqlite3").is_file());
        assert_eq!(store.read("a.txt").await.unwrap(), None);

        let mut staging = Staging::new();
        staging.write("b.txt", "b").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        assert_nothing_more(&mut feed).await;
    }
    /// A SQLite Location can't be inside another Store's Location, or inside a Working copy,
    /// whose folder holds a `.tidings/` too. Opening one there is refused, naming the directory it
    /// is inside, before anything is made. The Location itself holding `.tidings/` is usual.
    #[tokio::test]
    async fn opening_inside_another_location_or_a_working_copy_is_refused() {
        let fixture = Sqlite::new();
        let Opened { store: _store, feed: _feed } = fixture.open().await;
        let working_copy = fixture.directory.path().join("working copy");
        std::fs::create_dir_all(working_copy.join(".tidings")).unwrap();

        for outer in [fixture.location(), working_copy] {
            let inner = outer.join("deeper/inner");
            match tidings::Store::open_sqlite(&inner, SqliteOptions::default()).await {
                Err(Error::NestedLocation { outer: named }) => {
                    assert_eq!(named, outer.canonicalize().unwrap());
                }
                other => {
                    panic!("opening inside {} should be refused, got {other:?}", outer.display())
                }
            }
            assert!(!outer.join("deeper").exists());
        }
        let Opened { store: _again, feed: _again_feed } = fixture.open().await;
    }
}

#[cfg(feature = "fs")]
mod fs {
    use std::path::{Path as FsPath, PathBuf};
    use std::time::{Duration, SystemTime};

    use tempfile::TempDir;
    use tidings::{
        BackendKind, Change, ChangeKind, Error, FailurePoint, FeedItem, FsOptions,
        InvalidPathReason, Origin, Pause, Revision, Staging,
    };

    use crate::api::{ChangeFeed, Store};
    use crate::common::{
        assert_nothing_more, changes, changes_in_full, changes_until, next_batch, next_item,
    };
    use crate::separate_stores::Located;
    use crate::suite::{Fixture, Opened};

    /// Opens each test's Store at a Location in a temporary directory of its own, removed when
    /// the test ends. Each Store it opens is at the same Location. Tests make what they need
    /// outside the Location in the temporary directory.
    pub(crate) struct Fs {
        directory: TempDir,
    }

    impl Fs {
        pub(crate) fn new() -> Fs {
            Fs { directory: tempfile::tempdir().unwrap() }
        }

        /// The options each Store is opened with: with a short debounce window.
        fn usual_options(&self) -> FsOptions {
            FsOptions::default().debounce_window(Duration::from_millis(20))
        }

        /// Opens a Store at the Location with the options `options` makes of the usual ones.
        async fn open_with(&self, options: impl FnOnce(FsOptions) -> FsOptions) -> Opened {
            let options = options(self.usual_options());
            tidings::Store::open_fs(self.on_disk(""), options).await.unwrap().into()
        }

        /// Where `path` is on disk. The empty Path is the Location.
        fn on_disk(&self, path: &str) -> PathBuf {
            let location = self.directory.path().join("store");
            if path.is_empty() { location } else { location.join(path) }
        }

        /// Writes `contents` to `path` directly, as another program would.
        fn write_directly(&self, path: &str, contents: impl AsRef<[u8]>) {
            let file = self.on_disk(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, contents).unwrap();
        }
    }

    impl Fixture for Fs {
        async fn open(&self) -> Opened {
            self.open_with(|options| options).await
        }
    }

    impl Located for Fs {
        fn location(&self) -> PathBuf {
            self.on_disk("")
        }
    }

    behaviour_suite!(Fs::new());
    // Not `in_step:`: see the README's Consistency section.
    two_stores_suite!(committing: Fs::new());
    two_stores_suite!(seeing_each_other: Fs::new());
    separate_stores_suite!(Fs::new(), Fs::new());

    mod blocking {
        use super::Fs;
        use crate::api::call_blocking;
        use crate::suite::{Fixture, Opened};

        /// Opens each Store through the blocking API, as [`Fs`] does through the async one.
        struct BlockingFs(Fs);

        impl Fixture for BlockingFs {
            async fn open(&self) -> Opened {
                let (location, options) = (self.0.on_disk(""), self.0.usual_options());
                call_blocking(|| tidings::blocking::Store::open_fs(location, options))
                    .unwrap()
                    .into()
            }
        }

        behaviour_suite!(BlockingFs(Fs::new()));
        two_stores_suite!(committing: BlockingFs(Fs::new()));
        two_stores_suite!(seeing_each_other: BlockingFs(Fs::new()));
    }

    /// A Location can't be inside another Store's Location, or inside a Working copy, whose folder
    /// holds a `.tidings/` too. Opening one there is refused, naming the directory it is inside,
    /// before anything is made. The Location itself holding `.tidings/` is usual.
    #[tokio::test]
    async fn opening_inside_another_location_or_a_working_copy_is_refused() {
        let fixture = Fs::new();
        let Opened { store: _store, feed: _feed } = fixture.open().await;
        let working_copy = fixture.directory.path().join("working copy");
        std::fs::create_dir_all(working_copy.join(".tidings")).unwrap();

        for outer in [fixture.on_disk(""), working_copy] {
            let inner = outer.join("deeper/inner");
            match tidings::Store::open_fs(&inner, fixture.usual_options()).await {
                Err(Error::NestedLocation { outer: named }) => {
                    assert_eq!(named, outer.canonicalize().unwrap());
                }
                other => {
                    panic!("opening inside {} should be refused, got {other:?}", outer.display())
                }
            }
            assert!(!outer.join("deeper").exists());
        }
        let Opened { store: _again, feed: _again_feed } = fixture.open().await;
    }

    /// A Location reached through a symlink is checked where it really is, so a link from outside
    /// into another Store's Location doesn't get around the refusal.
    #[cfg(unix)]
    #[tokio::test]
    async fn opening_inside_another_location_through_a_symlink_is_refused() {
        let fixture = Fs::new();
        let Opened { store: _store, feed: _feed } = fixture.open().await;
        std::fs::create_dir(fixture.on_disk("sub")).unwrap();
        let link = fixture.directory.path().join("link");
        std::os::unix::fs::symlink(fixture.on_disk("sub"), &link).unwrap();

        for inner in [link.clone(), link.join("inner")] {
            match tidings::Store::open_fs(&inner, fixture.usual_options()).await {
                Err(Error::NestedLocation { outer }) => {
                    assert_eq!(outer, fixture.on_disk("").canonicalize().unwrap());
                }
                other => panic!("opening {} should be refused, got {other:?}", inner.display()),
            }
        }
        assert!(std::fs::read_dir(fixture.on_disk("sub")).unwrap().next().is_none());
    }

    /// The suite's Snapshot tests are skipped where Snapshots aren't supported, so this makes sure
    /// the filesystem is one of those, and that the suite's refusal test runs there.
    #[tokio::test]
    async fn fs_does_not_support_snapshots() {
        let Opened { store, feed: _feed } = Fs::new().open().await;
        assert!(!store.supports_snapshots());
    }

    /// A person editing, making or deleting a File in the Location, while the app runs,
    /// is reported as an external Change once the events have settled.
    #[tokio::test]
    async fn edits_made_directly_in_the_location_arrive_as_external_changes() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;

        fixture.write_directly("settings.toml", "a = 1\n");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("settings.toml", ChangeKind::Changed, Origin::External)],
        );
        fixture.write_directly("settings.toml", "a = 2\n");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("settings.toml", ChangeKind::Changed, Origin::External)],
        );
        std::fs::remove_file(fixture.on_disk("settings.toml")).unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("settings.toml", ChangeKind::Removed, Origin::External)],
        );
        assert_nothing_more(&mut feed).await;
    }

    /// An editor saves a File with a burst of events, and the File can be half written in
    /// between. The burst is held back until it settles, and gives one Change, after which the
    /// File reads whole.
    #[tokio::test]
    async fn an_editors_burst_of_events_gives_one_change_once_it_settles() {
        let fixture = Fs::new();
        let window = Duration::from_millis(300);
        let Opened { store, mut feed } =
            fixture.open_with(|options| options.debounce_window(window)).await;
        fixture.write_directly("settings.toml", "a = 1\n");
        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);

        // As vim does by default: move the File to a backup, write it again in two goes, remove
        // the backup.
        let file = fixture.on_disk("settings.toml");
        let backup = fixture.on_disk("settings.toml~");
        std::fs::rename(&file, &backup).unwrap();
        let mut writing = std::fs::File::create(&file).unwrap();
        std::io::Write::write_all(&mut writing, b"a = ").unwrap();
        std::io::Write::flush(&mut writing).unwrap();
        std::io::Write::write_all(&mut writing, b"2\n").unwrap();
        drop(writing);
        std::fs::remove_file(&backup).unwrap();

        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);
        let read = store.read("settings.toml").await.unwrap().unwrap();
        assert_eq!(read.contents(), "a = 2\n");
        assert_nothing_more(&mut feed).await;
    }

    /// An event that leaves a File's contents as they were is no Change: a File read, its times
    /// or permissions changed, or its contents written again as they were. Some of those events
    /// look like writes, so they are told apart by comparing Revisions. Only a File that has
    /// changed since the Store opened has a known Revision, so a File there before gets only the
    /// events that can't be writes: see the README's Consistency section. On macOS, FSEvents can
    /// give an event for a File made a moment before as its creation again, or give its creation
    /// late: so a File there before can get one Change, and nothing after, since its Revision is
    /// then known. Whether it does depends on how busy the machine is.
    #[tokio::test]
    async fn events_that_leave_a_files_contents_as_they_were_are_dropped() {
        let fixture = Fs::new();
        fixture.write_directly("before.txt", "there before");
        let Opened { store: _store, mut feed } = fixture.open().await;
        // How many Changes before.txt got: see the doc.
        let mut before = 0;
        fixture.write_directly("during.txt", "a");
        let read = changes_leaving_out_before(&mut feed, "during.txt", &mut before).await;
        assert_eq!(changes(&read), [("during.txt", ChangeKind::Changed)]);

        let set_readonly = |path: &str, readonly: bool| {
            let file = fixture.on_disk(path);
            let mut permissions = std::fs::metadata(&file).unwrap().permissions();
            permissions.set_readonly(readonly);
            std::fs::set_permissions(file, permissions).unwrap();
        };
        // As `touch` does, both times at once, which is no write.
        let both = std::fs::FileTimes::new()
            .set_accessed(SystemTime::UNIX_EPOCH)
            .set_modified(SystemTime::UNIX_EPOCH);
        let touch_without_writing = |path: &str| {
            std::fs::read(fixture.on_disk(path)).unwrap();
            let file = std::fs::File::open(fixture.on_disk(path)).unwrap();
            file.set_times(both).unwrap();
            set_readonly(path, true);
            set_readonly(path, false);
        };
        touch_without_writing("before.txt");
        touch_without_writing("during.txt");
        // The modification time alone, which is also how a write shows, and the contents as
        // they were.
        let file = std::fs::File::open(fixture.on_disk("during.txt")).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH).unwrap();
        fixture.write_directly("during.txt", "a");
        fixture.write_directly("marker.txt", "1");
        let read = changes_leaving_out_before(&mut feed, "marker.txt", &mut before).await;
        assert_eq!(changes(&read), [("marker.txt", ChangeKind::Changed)]);
        // Again, once before.txt's Revision may be known.
        touch_without_writing("before.txt");
        fixture.write_directly("marker.txt", "2");
        let read = changes_leaving_out_before(&mut feed, "marker.txt", &mut before).await;
        assert_eq!(changes(&read), [("marker.txt", ChangeKind::Changed)]);
        assert!(before <= 1, "before.txt got {before} Changes");
        if !cfg!(target_os = "macos") {
            assert_eq!(before, 0);
        }
    }

    /// Reads up to `marker` as [`changes_until`] does, leaving out a Change to before.txt, which
    /// it adds to `before`, for
    /// [`events_that_leave_a_files_contents_as_they_were_are_dropped`].
    async fn changes_leaving_out_before(
        feed: &mut ChangeFeed,
        marker: &str,
        before: &mut usize,
    ) -> Vec<Change> {
        let mut read = changes_until(feed, marker, false).await;
        read.retain(|change| {
            let is_before = change.path.as_str() == "before.txt";
            if is_before {
                assert_eq!(change.kind, ChangeKind::Changed);
                *before += 1;
            }
            !is_before
        });
        read
    }

    /// Names on disk that no Path has give no Change: tidings' own `.tidings/`, names like its
    /// temporary files', and names another program made that aren't Paths.
    #[tokio::test]
    async fn events_for_names_that_are_not_paths_are_ignored() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;
        fixture.write_directly(".tidings/other.txt", "x");
        fixture.write_directly(".kept.txt.tidings-0123456789abcdef0123456789abcdef-0", "x");
        fixture.write_directly("cafe\u{301}.txt", "x");
        fixture.write_directly("re\u{301}sume\u{301}/cv.txt", "x");
        fixture.write_directly("kept.txt", "x");

        assert_eq!(changes(&next_batch(&mut feed).await), [("kept.txt", ChangeKind::Changed)]);
        assert_nothing_more(&mut feed).await;
    }

    /// A directory moved into the Location with Files in it gives a Change for each of them, even
    /// though they got there before the directory was watched. One removed gives a Change for
    /// each File that was in it.
    #[tokio::test]
    async fn a_directory_moved_in_or_removed_gives_a_change_for_each_file_in_it() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;
        let outside = fixture.directory.path().join("outside");
        std::fs::create_dir_all(outside.join("deeper")).unwrap();
        std::fs::write(outside.join("a.toml"), "a").unwrap();
        std::fs::write(outside.join("deeper/b.toml"), "b").unwrap();

        std::fs::rename(&outside, fixture.on_disk("themes")).unwrap();
        let expected =
            [("themes/a.toml", ChangeKind::Changed), ("themes/deeper/b.toml", ChangeKind::Changed)];
        assert_eq!(changes(&next_batch(&mut feed).await), expected);

        std::fs::remove_dir_all(fixture.on_disk("themes")).unwrap();
        let expected =
            [("themes/a.toml", ChangeKind::Removed), ("themes/deeper/b.toml", ChangeKind::Removed)];
        assert_eq!(changes(&next_batch(&mut feed).await), expected);
        assert_nothing_more(&mut feed).await;
    }

    /// A symlink to a directory is left out of the Store, so a directory replaced by one is
    /// reported removed, and edits under it aren't reported.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_replaced_by_a_symlink_to_one_is_reported_removed() {
        let fixture = Fs::new();
        fixture.write_directly("themes/dark.toml", "dark");
        let outside = fixture.directory.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("dark.toml"), "dark").unwrap();
        let Opened { store: _store, mut feed } = fixture.open().await;

        std::fs::remove_dir_all(fixture.on_disk("themes")).unwrap();
        std::os::unix::fs::symlink(&outside, fixture.on_disk("themes")).unwrap();
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("themes/dark.toml", ChangeKind::Removed)],
        );

        std::fs::write(outside.join("dark.toml"), "edited").unwrap();
        std::fs::write(outside.join("light.toml"), "light").unwrap();
        assert_nothing_more(&mut feed).await;
    }

    /// A File replaced by renaming another over it looks newly made to the watcher. Removed or
    /// renamed away straight after, before its events have settled, it is still reported
    /// removed.
    #[tokio::test]
    async fn a_file_replaced_then_removed_straight_away_is_reported_removed() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;
        fixture.write_directly("removed.txt", "old");
        fixture.write_directly("moved.txt", "old");
        // Read up to the last write, as a busy machine can have their events settle apart.
        assert_eq!(
            changes(&changes_until(&mut feed, "moved.txt", false).await),
            [("moved.txt", ChangeKind::Changed), ("removed.txt", ChangeKind::Changed)],
        );
        // Quiet for a while, as the Files usually are when someone replaces them.
        tokio::time::sleep(Duration::from_millis(200)).await;

        for path in ["removed.txt", "moved.txt"] {
            let replacement = fixture.on_disk("replacement~");
            std::fs::write(&replacement, "new").unwrap();
            std::fs::rename(&replacement, fixture.on_disk(path)).unwrap();
        }
        std::fs::remove_file(fixture.on_disk("removed.txt")).unwrap();
        std::fs::rename(
            fixture.on_disk("moved.txt"),
            fixture.directory.path().join("moved away.txt"),
        )
        .unwrap();
        // On a busy machine, the watcher can look at a File before it is removed, and report it
        // changed first. Either way, it is reported removed in the end, with nothing else.
        fixture.write_directly("marker.txt", "");
        assert_eq!(
            changes(&changes_until(&mut feed, "marker.txt", false).await),
            [
                ("marker.txt", ChangeKind::Changed),
                ("moved.txt", ChangeKind::Removed),
                ("removed.txt", ChangeKind::Removed),
            ],
        );
    }

    /// The Location is a directory, made when the Store opens with any of its parents that are
    /// missing, and each File is a file in it that people can see.
    #[tokio::test]
    async fn the_location_is_a_directory_people_can_see_the_files_in() {
        let fixture = Fs::new();
        let location = fixture.directory.path().join("not/there/yet");
        let (store, _feed) =
            tidings::Store::open_fs(&location, FsOptions::default()).await.unwrap();
        assert!(location.is_dir());
        let detected = tidings::Store::detect(&location).await.unwrap();
        assert_eq!(detected, Some(BackendKind::Fs));

        let mut staging = Staging::new();
        staging.write("themes/dark.toml", "dark = true\n").unwrap();
        store.commit(staging).await.unwrap();
        let on_disk = std::fs::read_to_string(location.join("themes/dark.toml"));
        assert_eq!(on_disk.unwrap(), "dark = true\n");
    }

    #[tokio::test]
    async fn files_other_programs_make_are_read_and_listed() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly("settings.toml", "a = 1\n");
        fixture.write_directly("themes/dark.toml", "dark = true\n");

        assert_eq!(list(&store).await, ["settings.toml", "themes/dark.toml"]);
        let file = store.read("themes/dark.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "dark = true\n");
        let stat = store.stat("themes/dark.toml").await.unwrap().unwrap();
        assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
        // A directory is not a File.
        assert_eq!(store.read("themes").await.unwrap(), None);
    }

    /// Another program can make names on disk that no Path has. Those Files are left out of
    /// listings and Prefix Revisions, and so is everything under such directories.
    #[tokio::test]
    async fn names_on_disk_that_are_not_paths_are_left_out() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly("kept.txt", "kept");
        let before = store.stat_prefix("").await.unwrap();

        // Not in NFC form.
        fixture.write_directly("cafe\u{301}.txt", "x");
        fixture.write_directly("re\u{301}sume\u{301}/cv.txt", "x");
        // Named like tidings' own, or like its temporary files.
        fixture.write_directly(".tidings/other.txt", "x");
        fixture.write_directly(".kept.txt.tidings-0123456789abcdef0123456789abcdef-0", "x");
        // Names Windows can't hold, which Windows can't make either.
        #[cfg(not(windows))]
        {
            fixture.write_directly("CON", "x");
            fixture.write_directly("what?/a.txt", "x");
        }
        // Not valid UTF-8, which macOS's filesystems can't hold.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::OsStr::from_bytes(b"not \xff utf-8.txt");
            std::fs::write(fixture.on_disk("").join(name), "x").unwrap();
        }

        assert_eq!(list(&store).await, ["kept.txt"]);
        assert_eq!(store.stat_prefix("").await.unwrap(), before);
    }

    /// A File another program wrote that isn't valid UTF-8 can't be read as text, but it is
    /// still there: it is listed, has a Revision, and a Commit can replace it.
    #[tokio::test]
    async fn a_file_that_is_not_utf8_is_listed_but_reading_it_says_so() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly("image.png", [0x89, b'P', b'N', b'G', 0xff, 0xfe]);

        match store.read("image.png").await {
            Err(Error::NotText { path }) => assert_eq!(path.as_str(), "image.png"),
            other => panic!("expected NotText, got {other:?}"),
        }
        assert_eq!(list(&store).await, ["image.png"]);
        let stat = store.stat("image.png").await.unwrap();
        assert!(stat.is_some());

        let mut staging = Staging::new();
        staging.write("image.png", "text now").unwrap();
        store.commit(staging).await.unwrap();
        let file = store.read("image.png").await.unwrap().unwrap();
        assert_eq!(file.contents(), "text now");
    }

    #[tokio::test]
    async fn each_file_a_commit_writes_has_its_timestamp_as_modification_time_on_disk() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new();
        staging.write("a.txt", "a").unwrap();
        staging.write("b/c.txt", "c").unwrap();
        let committed = store.commit(staging).await.unwrap();

        for path in ["a.txt", "b/c.txt"] {
            let modified = std::fs::metadata(fixture.on_disk(path)).unwrap().modified();
            assert_eq!(modified.unwrap(), SystemTime::from(committed.timestamp()), "{path}");
        }
    }

    /// People link config files in from elsewhere, such as a repository of their dotfiles. A
    /// write goes through the link, to the File it points to, and the link stays a link.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_to_a_symlink_writes_the_file_it_points_to_and_leaves_the_link() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let dotfiles = fixture.directory.path().join("dotfiles");
        std::fs::create_dir_all(dotfiles.join("app")).unwrap();
        std::fs::write(dotfiles.join("app/settings.toml"), "a = 1\n").unwrap();
        // One link that is relative, to another that isn't.
        std::os::unix::fs::symlink(dotfiles.join("app/settings.toml"), dotfiles.join("current"))
            .unwrap();
        let link = fixture.on_disk("settings.toml");
        std::os::unix::fs::symlink("../dotfiles/current", &link).unwrap();

        let file = store.read("settings.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "a = 1\n");
        let mut staging = Staging::new();
        staging.write_back(&file, "a = 2\n");
        store.commit(staging).await.unwrap();

        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(std::fs::symlink_metadata(dotfiles.join("current")).unwrap().is_symlink());
        let target = std::fs::read_to_string(dotfiles.join("app/settings.toml")).unwrap();
        assert_eq!(target, "a = 2\n");
        let file = store.read("settings.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "a = 2\n");
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// A symlinked File is watched through its link: an edit to the file it points to, outside
    /// the Location or in it, arrives as a Change for the linking Path. Links made, changed and
    /// removed while the Store runs are followed too.
    #[cfg(unix)]
    #[tokio::test]
    async fn edits_to_a_symlinks_target_arrive_as_changes_for_the_linking_path() {
        use std::os::unix::fs::symlink;
        let fixture = Fs::new();
        let dotfiles = fixture.directory.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::write(dotfiles.join("settings.toml"), "a = 1\n").unwrap();
        fixture.write_directly("real.toml", "real");
        symlink(dotfiles.join("settings.toml"), fixture.on_disk("settings.toml")).unwrap();
        symlink("real.toml", fixture.on_disk("alias.toml")).unwrap();
        let Opened { store: _store, mut feed } = fixture.open().await;

        // Linked when the Store opened.
        std::fs::write(dotfiles.join("settings.toml"), "a = 2\n").unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("settings.toml", ChangeKind::Changed, Origin::External)],
        );
        fixture.write_directly("real.toml", "real, edited");
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("alias.toml", ChangeKind::Changed), ("real.toml", ChangeKind::Changed)],
        );

        // Replaced the way editors save, by renaming a new file over it.
        std::fs::write(dotfiles.join("settings.toml.new"), "a = 3\n").unwrap();
        std::fs::rename(dotfiles.join("settings.toml.new"), dotfiles.join("settings.toml"))
            .unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);

        // Linked while the Store runs, to a file in another directory outside the Location.
        let elsewhere = fixture.directory.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("theme.toml"), "dark").unwrap();
        symlink(elsewhere.join("theme.toml"), fixture.on_disk("theme.toml")).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);
        std::fs::write(elsewhere.join("theme.toml"), "light").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);

        // Linked elsewhere, the way `ln -sf` does it: a new link renamed over the old one.
        std::fs::write(elsewhere.join("other.toml"), "other").unwrap();
        symlink(elsewhere.join("other.toml"), elsewhere.join("new link")).unwrap();
        std::fs::rename(elsewhere.join("new link"), fixture.on_disk("theme.toml")).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);
        std::fs::write(elsewhere.join("other.toml"), "other, edited").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);
        std::fs::write(elsewhere.join("theme.toml"), "no longer linked").unwrap();
        assert_nothing_more(&mut feed).await;

        // Unlinked: the target's edits are nothing to the Store any more.
        std::fs::remove_file(fixture.on_disk("theme.toml")).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Removed)]);
        std::fs::write(elsewhere.join("theme.toml"), "blue").unwrap();
        assert_nothing_more(&mut feed).await;
    }

    /// A dotfiles setup can link through several links: here to `current`, which points to the
    /// file itself. Each link on the way is watched, so retargeting `current` is a Change, and
    /// edits then go by where it points now.
    #[cfg(unix)]
    #[tokio::test]
    async fn every_link_in_a_chain_of_symlinks_is_followed() {
        use std::os::unix::fs::symlink;
        let fixture = Fs::new();
        let dotfiles = fixture.directory.path().join("dotfiles");
        for version in ["app", "app2"] {
            std::fs::create_dir_all(dotfiles.join(version)).unwrap();
            std::fs::write(dotfiles.join(version).join("s.toml"), version).unwrap();
        }
        symlink("app/s.toml", dotfiles.join("current")).unwrap();
        fixture.write_directly("unrelated.toml", "");
        symlink(dotfiles.join("current"), fixture.on_disk("s.toml")).unwrap();
        let Opened { store, mut feed } = fixture.open().await;

        std::fs::write(dotfiles.join("app/s.toml"), "app, edited").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("s.toml", ChangeKind::Changed)]);

        symlink("app2/s.toml", dotfiles.join("current.new")).unwrap();
        std::fs::rename(dotfiles.join("current.new"), dotfiles.join("current")).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("s.toml", ChangeKind::Changed)]);
        let file = store.read("s.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "app2");
        std::fs::write(dotfiles.join("app2/s.toml"), "app2, edited").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("s.toml", ChangeKind::Changed)]);
        std::fs::write(dotfiles.join("app/s.toml"), "app, no longer linked").unwrap();
        assert_nothing_more(&mut feed).await;
    }

    /// Removing the Location, as the OS clears a cache, while the app runs, is safe: it is made
    /// and marked again, and watched again, and the Store gets a Resync, since its Files are gone.
    /// On macOS, FSEvents can report the removal, and the Location made again, late, and the Store
    /// then gets another Resync: see the README's Consistency section.
    #[tokio::test]
    async fn a_location_removed_while_running_is_made_again_with_a_resync() {
        let fixture = Fs::new();
        let Opened { store, mut feed } = fixture.open().await;
        let mut staging = Staging::new();
        staging.write("thumbnails/a.png", "a").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("thumbnails/a.png", ChangeKind::Changed)]
        );

        std::fs::remove_dir_all(fixture.on_disk("")).unwrap();
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        assert!(fixture.on_disk("").is_dir());
        let detected = tidings::Store::detect(fixture.on_disk("")).await.unwrap();
        assert_eq!(detected, Some(BackendKind::Fs));
        assert_eq!(settle_skipping_resyncs(&fixture, &mut feed).await, []);

        // Watched again.
        fixture.write_directly("new.txt", "x");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("new.txt", ChangeKind::Changed, Origin::External)],
        );
        let mut staging = Staging::new();
        staging.write("thumbnails/a.png", "a").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("thumbnails/a.png", ChangeKind::Changed, Origin::Local)],
        );
        // Settled, so that the Commit's events aren't looked at after the rename, which would
        // rightly report its File removed.
        assert_eq!(settle_skipping_resyncs(&fixture, &mut feed).await, []);

        // Renamed away, the same, and what happens to it where it went is no Change.
        let moved = fixture.directory.path().join("old store");
        std::fs::rename(fixture.on_disk(""), &moved).unwrap();
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        assert!(fixture.on_disk("").is_dir());
        std::fs::write(moved.join("new.txt"), "y").unwrap();
        assert_eq!(settle_skipping_resyncs(&fixture, &mut feed).await, []);
        fixture.write_directly("new.txt", "z");
        assert_eq!(changes(&next_batch(&mut feed).await), [("new.txt", ChangeKind::Changed)]);
        assert_nothing_more(&mut feed).await;
    }

    /// Waits until the watcher has looked at every event so far, skipping late Resyncs, and gives
    /// the other Changes it read, for
    /// [`a_location_removed_while_running_is_made_again_with_a_resync`]. It writes a marker File
    /// twice, reading up to each Change to it. What the watcher gives for earlier events comes
    /// before the first Change, or with it; the second write comes after all that, so its Change
    /// comes after it all too. A Resync takes the place of the Changes not read yet, the marker's
    /// too, so after one it starts again, writing the marker twice more.
    async fn settle_skipping_resyncs(fixture: &Fs, feed: &mut ChangeFeed) -> Vec<Change> {
        let (mut read, mut written, mut settled) = (Vec::new(), 0, 0);
        while settled < 2 {
            written += 1;
            fixture.write_directly("marker.txt", written.to_string());
            loop {
                match next_item(feed).await {
                    FeedItem::Resync => {
                        (read, settled) = (Vec::new(), 0);
                        break;
                    }
                    FeedItem::Changes(batch) => {
                        let marked =
                            batch.iter().any(|change| change.path.as_str() == "marker.txt");
                        read.extend(
                            batch.into_iter().filter(|change| change.path.as_str() != "marker.txt"),
                        );
                        if marked {
                            settled += 1;
                            break;
                        }
                    }
                }
            }
        }
        read
    }

    /// If watching fails, the Store gets a Resync, since Changes may have been missed, and watching
    /// goes on.
    #[tokio::test]
    async fn a_failure_to_watch_gives_a_resync() {
        let fixture = Fs::new();
        // The watcher's first events fail, so they must be the test's. A Store opened first
        // makes the Location, and it has had every event of that once it reports a later write:
        // on a busy machine, they could otherwise reach the failing Store late.
        {
            let Opened { store: _store, mut feed } = fixture.open().await;
            fixture.write_directly("opened.txt", "x");
            changes_until(&mut feed, "opened.txt", false).await;
        }
        let Opened { store: _store, mut feed } =
            fixture.open_with(|options| options.fail_at(FailurePoint::WatchingFails)).await;

        // The watcher loses its watch of the Location, as it can when it fails, so the Location is
        // watched again: directories made meanwhile too.
        fixture.write_directly("missed/a.txt", "x");
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        // The File's own events can settle after the directory's, and after the Resync, which
        // listed it without reading it, as when a Store opens: they then give it a Change.
        fixture.write_directly("seen.txt", "x");
        let read = changes_until(&mut feed, "seen.txt", false).await;
        let late = [("missed/a.txt", ChangeKind::Changed), ("seen.txt", ChangeKind::Changed)];
        assert!(changes(&read) == late[1..] || changes(&read) == late, "{read:?}");
        fixture.write_directly("missed/a.txt", "y");
        assert_eq!(changes(&next_batch(&mut feed).await), [("missed/a.txt", ChangeKind::Changed)]);
        assert_nothing_more(&mut feed).await;
    }

    /// A Location that can't be watched, as when the platform's limit on watches is reached, is
    /// tried again, waiting longer each time. The Store opens meanwhile. It gets a Resync
    /// straight away, since its Changes aren't reported, and another once the Location is
    /// watched, since they were missed until then.
    #[tokio::test]
    async fn a_location_that_cant_be_watched_is_resynced_and_tried_again() {
        let fixture = Fs::new();
        // It fails when the Store opens, and once more when it is tried again.
        let failing = FailurePoint::WatchingTheLocationFails { times: 2 };
        let Opened { store: _store, mut feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;

        for _ in 0..2 {
            assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        }
        assert_nothing_more(&mut feed).await;
        fixture.write_directly("seen.txt", "x");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("seen.txt", ChangeKind::Changed, Origin::External)]
        );
    }

    /// A symlink in the Location can point to another File in it. While the Location isn't
    /// watched yet, following the link must not watch its directory apart from it: unwatching
    /// that when the link changes would stop the Location's own watch.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_in_a_location_not_watched_yet_leaves_its_watch_alone() {
        let fixture = Fs::new();
        fixture.write_directly("t.toml", "t");
        fixture.write_directly("u.toml", "u");
        let link = fixture.on_disk("l.toml");
        std::os::unix::fs::symlink(fixture.on_disk("t.toml"), &link).unwrap();
        // Watching fails once, when the Store opens.
        let failing = FailurePoint::WatchingTheLocationFails { times: 1 };
        let Opened { store: _store, mut feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        // Watched again after a window.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync);
        assert_nothing_more(&mut feed).await;

        let new_link = fixture.on_disk("new link");
        std::os::unix::fs::symlink(fixture.on_disk("u.toml"), &new_link).unwrap();
        std::fs::rename(&new_link, &link).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("l.toml", ChangeKind::Changed)]);
        fixture.write_directly("t.toml", "t, edited");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("t.toml", ChangeKind::Changed, Origin::External)],
        );
    }

    /// No File is read when the Store opens, since they can be large, so a File there before has
    /// no known Revision. Its first write is reported even with its contents as they were. From
    /// then on its Revision is known, and a write like that is dropped.
    #[tokio::test]
    async fn the_first_write_of_a_file_there_before_the_store_opened_is_reported() {
        let fixture = Fs::new();
        fixture.write_directly("before.txt", "same");
        let Opened { store: _store, mut feed } = fixture.open().await;
        fixture.write_directly("before.txt", "same");
        let batch = next_batch(&mut feed).await;
        assert_eq!(changes(&batch), [("before.txt", ChangeKind::Changed)]);
        fixture.write_directly("before.txt", "same");
        assert_nothing_more(&mut feed).await;
    }

    /// Something on disk that isn't a File can have the name of a File a Commit writes, or of a
    /// directory the Commit must make for one. An empty directory makes way for the File. Anything
    /// else refuses the Commit before it happens, as a File would.
    #[tokio::test]
    async fn what_is_on_disk_but_not_a_file_makes_way_or_refuses_the_commit() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        std::fs::create_dir_all(fixture.on_disk("empty/inside")).unwrap();
        let mut staging = Staging::new();
        staging.write("empty", "a File now").unwrap();
        store.commit(staging).await.unwrap();
        let file = store.read("empty").await.unwrap().unwrap();
        assert_eq!(file.contents(), "a File now");

        // A directory with a name in it that no Path has.
        fixture.write_directly("held/cafe\u{301}.txt", "x");
        let mut refused = vec![("held", "held")];
        // A symlink to nothing, where a directory would have to go.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("nowhere", fixture.on_disk("dangling")).unwrap();
            refused.push(("dangling/a.txt", "dangling/a.txt"));
        }
        for (path, expected) in refused {
            let mut staging = Staging::new();
            staging.write(path, "x").unwrap();
            staging.write("fine.txt", "fine").unwrap();
            match store.commit(staging).await {
                Err(Error::InvalidPath { path, reason: InvalidPathReason::FileUnderFile }) => {
                    assert_eq!(path, expected);
                }
                other => panic!("writing {path} should be refused, got {other:?}"),
            }
        }
        assert_eq!(list(&store).await, ["empty"]);
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// A Commit stopped at any point, as if the process had died there, is all there or not there
    /// at all once a Store opens the Location again, with no temporary file left behind. So is one
    /// whose rename keeps failing, which reads show as all there before then. Each of these runs
    /// every shape of Commit that [`Shape`] names through every point.
    #[tokio::test]
    async fn a_write_is_all_or_nothing_wherever_it_stops() {
        stop_everywhere(Shape::WRITE).await;
    }

    #[tokio::test]
    async fn a_write_and_a_delete_are_all_or_nothing_wherever_they_stop() {
        stop_everywhere(Shape::WRITE_AND_DELETE).await;
    }

    #[tokio::test]
    async fn a_prefix_delete_is_all_or_nothing_wherever_it_stops() {
        stop_everywhere(Shape::PREFIX_DELETE).await;
    }

    #[tokio::test]
    async fn moves_between_a_file_and_a_prefix_are_all_or_nothing_wherever_they_stop() {
        stop_everywhere(Shape::MOVES).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_through_a_symlink_is_all_or_nothing_wherever_it_stops() {
        stop_everywhere(Shape::THROUGH_A_SYMLINK).await;
    }

    /// A Store that was already open when another one's Commit was interrupted, as another
    /// process's might be, finishes it before it commits.
    #[tokio::test]
    async fn the_next_commit_finishes_a_commit_that_was_interrupted_once_committed() {
        let fixture = Fs::new();
        let Opened { store: other, feed: _other_feed } = fixture.open().await;
        other.commit(before_moves()).await.unwrap();

        let Opened { store, feed: _feed } =
            fixture.open_with(|options| options.fail_at(FailurePoint::AfterCommittedJournal)).await;
        let stopped = store.commit(moves()).await;
        assert!(matches!(stopped, Err(Error::Backend(_))), "{stopped:?}");
        drop(store);

        let mut staging = Staging::new();
        staging.write("later.txt", "later").unwrap();
        other.commit(staging).await.unwrap();
        assert!(other.read("later.txt").await.unwrap().is_some());
        let mut staging = Staging::new();
        staging.delete("later.txt").unwrap();
        other.commit(staging).await.unwrap();
        assert_moved(&other).await;
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// Finishing a Commit again deletes a File only if it is still the one the Commit deleted. A
    /// File another program wrote there since stays.
    #[tokio::test]
    async fn finishing_a_commit_again_keeps_a_file_written_since_where_it_deleted_one() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new();
        staging.write("old.txt", "old").unwrap();
        store.commit(staging).await.unwrap();
        drop(store);

        let Opened { store, feed: _feed } =
            fixture.open_with(|options| options.fail_at(FailurePoint::AfterRename(0))).await;
        let mut rename = Staging::new();
        rename.delete("old.txt").unwrap();
        rename.write("new.txt", "old").unwrap();
        assert!(matches!(store.commit(rename).await, Err(Error::Backend(_))));
        drop(store);
        fixture.write_directly("old.txt", "written since");

        let Opened { store, feed: _feed } = fixture.open().await;
        assert_eq!(list(&store).await, ["new.txt", "old.txt"]);
        let since = store.read("old.txt").await.unwrap().unwrap();
        assert_eq!(since.contents(), "written since");
    }

    /// Finishing a Commit again never follows a symlink to a directory made since: what it writes
    /// or deletes under one is outside the Store, so it is left as it is.
    #[cfg(unix)]
    #[tokio::test]
    async fn finishing_a_commit_again_leaves_what_is_under_a_directory_link_made_since() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new();
        staging.write("x/y.txt", "y").unwrap();
        store.commit(staging).await.unwrap();
        drop(store);

        let Opened { store, feed: _feed } =
            fixture.open_with(|options| options.fail_at(FailurePoint::AfterCommittedJournal)).await;
        let mut staging = Staging::new();
        staging.delete("x/y.txt").unwrap();
        staging.write("d/e.txt", "e").unwrap();
        assert!(matches!(store.commit(staging).await, Err(Error::Backend(_))));
        drop(store);
        let [elsewhere_x, elsewhere_d] =
            ["elsewhere x", "elsewhere d"].map(|name| fixture.directory.path().join(name));
        std::fs::rename(fixture.on_disk("x"), &elsewhere_x).unwrap();
        std::os::unix::fs::symlink(&elsewhere_x, fixture.on_disk("x")).unwrap();
        std::fs::create_dir(&elsewhere_d).unwrap();
        std::os::unix::fs::symlink(&elsewhere_d, fixture.on_disk("d")).unwrap();

        let Opened { store, feed: _feed } = fixture.open().await;
        assert_eq!(list(&store).await, Vec::<String>::new());
        assert_eq!(std::fs::read_to_string(elsewhere_x.join("y.txt")).unwrap(), "y");
        assert!(std::fs::read_dir(&elsewhere_d).unwrap().next().is_none());
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// Two Paths can be the same file on disk, through a symlink. A Commit that writes or deletes
    /// both is refused, since which of them wins would depend on the order they land in.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_commit_to_two_paths_that_are_the_same_file_is_refused() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly("b", "b");
        std::os::unix::fs::symlink("b", fixture.on_disk("a")).unwrap();
        std::os::unix::fs::symlink("b", fixture.on_disk("c")).unwrap();

        type Stage = fn(&mut Staging);
        let refused: [(&str, Stage); 3] = [
            ("write a, delete b", |staging| {
                staging.write("a", "new a").unwrap();
                staging.delete("b").unwrap();
            }),
            ("write a, write b", |staging| {
                staging.write("a", "new a").unwrap();
                staging.write("b", "new b").unwrap();
            }),
            ("write a, write c", |staging| {
                staging.write("a", "new a").unwrap();
                staging.write("c", "new c").unwrap();
            }),
        ];
        for (doing, stage) in refused {
            let mut staging = Staging::new();
            stage(&mut staging);
            match store.commit(staging).await {
                Err(Error::InvalidPath { reason: InvalidPathReason::SameFile, .. }) => {}
                other => panic!("{doing} should be refused, got {other:?}"),
            }
        }
        for (path, contents) in [("a", "b"), ("b", "b"), ("c", "b")] {
            let file = store.read(path).await.unwrap().unwrap();
            assert_eq!(file.contents(), contents, "{path}");
        }
        assert!(std::fs::symlink_metadata(fixture.on_disk("a")).unwrap().is_symlink());

        // A delete of the link itself, and a write of the File, are two files.
        let mut staging = Staging::new();
        staging.delete("a").unwrap();
        staging.write("b", "new b").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(store.read("a").await.unwrap(), None);
    }

    /// A symlink to a directory in the Location isn't a Prefix: it, and everything under it, are
    /// left out, as names that aren't Paths are. A Commit that would write through one is refused
    /// before anything is written, and a Prefix delete leaves it alone.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_to_a_directory_is_left_out_of_the_store() {
        use std::os::unix::fs::symlink;
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly("real/x", "x");
        let outside = fixture.directory.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("y"), "y").unwrap();
        symlink("real", fixture.on_disk("linked")).unwrap();
        symlink(&outside, fixture.on_disk("elsewhere")).unwrap();

        assert_eq!(list(&store).await, ["real/x"]);
        assert_eq!(store.list("linked/").await.unwrap(), Vec::<tidings::Path>::new());
        assert_eq!(store.read("linked/x").await.unwrap(), None);
        assert_eq!(store.stat("elsewhere/y").await.unwrap(), None);

        for path in ["linked", "linked/x", "elsewhere/y", "elsewhere/new/z"] {
            let mut staging = Staging::new();
            staging.write(path, "new").unwrap();
            match store.commit(staging).await {
                Err(Error::InvalidPath { reason: InvalidPathReason::DirectoryLink, .. }) => {}
                other => panic!("writing {path} should be refused, got {other:?}"),
            }
        }
        assert_eq!(std::fs::read_to_string(fixture.on_disk("real/x")).unwrap(), "x");
        assert_eq!(std::fs::read_to_string(outside.join("y")).unwrap(), "y");
        assert!(!outside.join("new").exists());
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());

        let mut staging = Staging::new();
        staging.delete_prefix("").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(list(&store).await, Vec::<String>::new());
        assert_eq!(std::fs::read_to_string(outside.join("y")).unwrap(), "y");
        let link = std::fs::symlink_metadata(fixture.on_disk("elsewhere")).unwrap();
        assert!(link.is_symlink());
    }

    /// A directory in the Location that holds a `.tidings/`, as another Store's Location or a
    /// Working copy does, is outside the Store, as a symlink to a directory is: what is under it
    /// isn't listed, read, or in a Prefix Revision.
    #[tokio::test]
    async fn a_directory_holding_another_store_or_a_working_copy_is_left_out() {
        let fixture = Fs::new();
        let (inner, _inner_feed) =
            tidings::Store::open_fs(fixture.on_disk("inner"), fixture.usual_options())
                .await
                .unwrap();
        let mut staging = Staging::new();
        staging.write("a.txt", "a").unwrap();
        inner.commit(staging).await.unwrap();
        std::fs::create_dir_all(fixture.on_disk("themes/copy/.tidings")).unwrap();
        fixture.write_directly("themes/copy/b.txt", "b");
        fixture.write_directly("themes/dark.toml", "dark");
        let Opened { store, feed: _feed } = fixture.open().await;

        assert_eq!(list(&store).await, ["themes/dark.toml"]);
        assert_eq!(store.list("inner/").await.unwrap(), Vec::<tidings::Path>::new());
        assert_eq!(store.list("themes/copy/").await.unwrap(), Vec::<tidings::Path>::new());
        assert_eq!(store.read("inner/a.txt").await.unwrap(), None);
        assert_eq!(store.stat("themes/copy/b.txt").await.unwrap(), None);

        let prefix_revisions = async || {
            let all = store.stat_prefix("").await.unwrap().to_string();
            (all, store.stat_prefix("themes/").await.unwrap().to_string())
        };
        let before = prefix_revisions().await;
        let mut staging = Staging::new();
        staging.write("c.txt", "c").unwrap();
        inner.commit(staging).await.unwrap();
        fixture.write_directly("themes/copy/b.txt", "edited");
        assert_eq!(prefix_revisions().await, before);
    }

    /// The Location can itself be a symlink to a directory, as when a person keeps an app's config
    /// in a dotfiles repo. It is followed once, when the Store opens.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_location_that_is_a_symlink_is_followed() {
        let fixture = Fs::new();
        let dotfiles = fixture.directory.path().join("dotfiles/app");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::write(dotfiles.join("a.toml"), "a").unwrap();
        let link = fixture.on_disk("");
        std::os::unix::fs::symlink(&dotfiles, &link).unwrap();
        let Opened { store, mut feed } = fixture.open().await;
        assert_eq!(list(&store).await, ["a.toml"]);

        let mut staging = Staging::new();
        staging.write("b.toml", "b").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(std::fs::read_to_string(dotfiles.join("b.toml")).unwrap(), "b");
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        next_batch(&mut feed).await;

        std::fs::write(dotfiles.join("c.toml"), "c").unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [("c.toml", ChangeKind::Changed, Origin::External)],
        );
    }

    /// A Location that is a symlink is resolved once, when the Store opens: re-pointing or
    /// removing the link while it is open changes nothing until it opens again.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_location_link_is_resolved_only_at_open() {
        let fixture = Fs::new();
        let [first, second] = ["first", "second"].map(|name| fixture.directory.path().join(name));
        for directory in [&first, &second] {
            std::fs::create_dir(directory).unwrap();
        }
        let link = fixture.on_disk("");
        std::os::unix::fs::symlink(&first, &link).unwrap();
        let Opened { store, mut feed } = fixture.open().await;

        std::fs::remove_file(&link).unwrap();
        let mut staging = Staging::new();
        staging.write("a.toml", "a").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(std::fs::read_to_string(first.join("a.toml")).unwrap(), "a");
        next_batch(&mut feed).await;

        std::os::unix::fs::symlink(&second, &link).unwrap();
        std::fs::write(second.join("b.toml"), "b").unwrap();
        assert_nothing_more(&mut feed).await;
        assert_eq!(list(&store).await, ["a.toml"]);
        drop(store);
        drop(feed);

        let Opened { store, feed: _feed } = fixture.open().await;
        assert_eq!(list(&store).await, ["b.toml"]);
    }

    /// Files with the same name in different directories are different files, even while their
    /// directories don't exist yet.
    #[tokio::test]
    async fn files_named_alike_in_directories_still_to_be_made_are_different_files() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new();
        for path in ["x.txt", "new/x.txt", "new/deeper/x.txt"] {
            staging.write(path, path).unwrap();
        }
        store.commit(staging).await.unwrap();
        assert_eq!(list(&store).await, ["new/deeper/x.txt", "new/x.txt", "x.txt"]);
    }

    /// A write through a symlink to a File that doesn't exist yet makes the File, if the
    /// directory it would be in exists. tidings never makes a directory outside the Location, so
    /// otherwise the write is refused, before anything is written.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_through_a_symlink_never_makes_a_directory() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let outside = fixture.directory.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let link = |path: &str, to: &FsPath| {
            std::os::unix::fs::symlink(to, fixture.on_disk(path)).unwrap();
        };
        link("new.toml", &outside.join("new.toml"));
        link("deep.toml", &outside.join("deep/er/y.toml"));
        link("nowhere.toml", FsPath::new("/nonexistent/q.toml"));

        let mut staging = Staging::new();
        staging.write("new.toml", "new").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(std::fs::read_to_string(outside.join("new.toml")).unwrap(), "new");

        for path in ["deep.toml", "nowhere.toml"] {
            let mut staging = Staging::new();
            staging.write(path, "x").unwrap();
            match store.commit(staging).await {
                Err(Error::Backend(error)) => {
                    let said = error.to_string();
                    assert!(said.contains("a directory that doesn't exist"), "{path}: {said}");
                }
                other => panic!("writing {path} should be refused, got {other:?}"),
            }
        }
        assert!(!outside.join("deep").exists());
        assert!(!FsPath::new("/nonexistent").exists());
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// A rename that fails, as one does on Windows while another program has the File open, is
    /// tried again after a moment. If it works then, the Commit finishes as usual.
    #[tokio::test]
    async fn a_rename_that_fails_for_a_moment_is_tried_again() {
        let fixture = Fs::new();
        let point = FailurePoint::RenameFails { n: 1, times: 1 };
        let Opened { store, mut feed } = fixture.open_with(|options| options.fail_at(point)).await;
        let mut staging = Staging::new();
        staging.write("a.txt", "a").unwrap();
        staging.write("b.txt", "b").unwrap();
        store.commit(staging).await.unwrap();

        assert_eq!(list(&store).await, ["a.txt", "b.txt"]);
        let file = store.read("b.txt").await.unwrap().unwrap();
        assert_eq!(file.contents(), "b");
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("a.txt", ChangeKind::Changed), ("b.txt", ChangeKind::Changed)],
        );
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// A rename that keeps failing gives `Pending`: the Commit has happened, and its Changes
    /// arrive once, but it isn't finished. Until it is, reads through tidings show it, and a
    /// Store can still open. The next Commit finishes it first, or if it still can't, isn't made.
    #[tokio::test]
    async fn a_rename_that_keeps_failing_gives_pending_and_reads_show_the_commit() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        store.commit(before_moves()).await.unwrap();
        drop(store);
        let Opened { store: other, feed: _other_feed } = fixture.open().await;

        // `moves` writes `a/b`, `d` and `kept.txt`, in that order. Its deletes are made and `a/b`
        // is renamed, but `d` is never renamed, so neither is `kept.txt`.
        let failing = FailurePoint::RenameFails { n: 1, times: usize::MAX };
        let Opened { store, mut feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;
        let pending = store.commit(moves()).await;
        assert!(matches!(pending, Err(Error::Pending)), "{pending:?}");
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [
                ("a", ChangeKind::Removed),
                ("a/b", ChangeKind::Changed),
                ("d", ChangeKind::Changed),
                ("d/e", ChangeKind::Removed),
                ("kept.txt", ChangeKind::Changed),
            ],
        );
        assert_moved(&store).await;
        let mut modified = Vec::new();
        for path in ["a/b", "d", "kept.txt"] {
            let file = store.read(path).await.unwrap().unwrap();
            let stat = store.stat(path).await.unwrap().unwrap();
            assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
            modified.push(stat.modified());
        }
        assert!(modified.iter().all(|time| *time == modified[0]), "{modified:?}");
        // Written as text, to compare with another Store's.
        let prefix_revision = store.stat_prefix("").await.unwrap().to_string();

        // Finishing it still fails, so the next Commit through this Store isn't made.
        let mut staging = Staging::new();
        staging.write("later.txt", "later").unwrap();
        match store.commit(staging).await {
            Err(Error::Backend(error)) => {
                let said = error.to_string();
                assert!(said.contains("this Commit wasn't made"), "{said}");
                assert!(said.contains("Try again later"), "{said}");
                assert!(error.source().is_some(), "{error:?}");
            }
            other => panic!("the next Commit should be refused, got {other:?}"),
        }
        assert_moved(&store).await;
        assert_nothing_more(&mut feed).await;

        // A Store opens while finishing still fails, and shows the Commit too.
        drop(store);
        let Opened { store, feed: _feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;
        assert_moved(&store).await;
        assert_eq!(store.stat_prefix("").await.unwrap().to_string(), prefix_revision);

        // Another Store's next Commit finishes it, and its Preconditions hold against it.
        let kept = other.read("kept.txt").await.unwrap().unwrap();
        let others = other.stat_prefix("").await.unwrap();
        assert_eq!(others.to_string(), prefix_revision);
        let mut staging = Staging::new();
        staging.write_back(&kept, "newer");
        staging.require_prefix("", others).unwrap();
        other.commit(staging).await.unwrap();
        for store in [&store, &other] {
            assert_eq!(list(store).await, ["a/b", "d", "kept.txt"]);
            let kept = store.read("kept.txt").await.unwrap().unwrap();
            assert_eq!(kept.contents(), "newer");
        }
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// A Commit that gave `Pending` is reported when it is made, as reads show it then, even
    /// where none of its Files has changed on disk yet: here its first rename fails, so none is
    /// renamed. When it is finished later, by another Store's Commit here, its Files land, and
    /// nobody is told of them again: not the Store that made it, not a Store that was open
    /// already and was told of it as external, and not one opened while it was still pending.
    #[tokio::test]
    async fn a_pending_commit_is_reported_once_and_not_again_when_it_is_finished() {
        let fixture = Fs::new();
        let Opened { store: other, feed: mut other_feed } = fixture.open().await;
        let failing = FailurePoint::RenameFails { n: 0, times: usize::MAX };
        let Opened { store, mut feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;
        let pending = store.commit(staged(&[("a.txt", "a"), ("b.txt", "b")], &[])).await;
        assert!(matches!(pending, Err(Error::Pending)), "{pending:?}");

        let written = [("a.txt", ChangeKind::Changed), ("b.txt", ChangeKind::Changed)];
        let batch = next_batch(&mut feed).await;
        assert!(batch.iter().all(|change| change.origin == Origin::Local));
        assert_eq!(changes(&batch), written);
        let batch = next_batch(&mut other_feed).await;
        assert!(batch.iter().all(|change| change.origin == Origin::External));
        assert_eq!(changes(&batch), written);
        // Its rename fails here too, so it stays pending.
        let Opened { store: _later, feed: mut later_feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;

        let mut staging = Staging::new();
        staging.write("later.txt", "later").unwrap();
        other.commit(staging).await.unwrap();
        let later = [("later.txt", ChangeKind::Changed)];
        assert_eq!(changes(&next_batch(&mut other_feed).await), later);
        assert_eq!(changes(&next_batch(&mut feed).await), later);
        assert_eq!(changes(&next_batch(&mut later_feed).await), later);
        for feed in [&mut feed, &mut other_feed, &mut later_feed] {
            assert_nothing_more(feed).await;
        }
        let written = [("a.txt", "a"), ("b.txt", "b"), ("later.txt", "later")];
        assert_eq!(contents(&other).await, written.map(|(p, c)| (p.to_owned(), c.to_owned())));
    }

    /// A Commit whose future is dropped once it has started still finishes, in the background,
    /// and its Changes arrive. Here it is dropped for certain while it is held half way through,
    /// with its journal `prepared` and one temporary file written.
    #[tokio::test]
    async fn a_commit_dropped_part_way_through_still_finishes_and_is_reported() {
        let fixture = Fs::new();
        let pause = Pause::new();
        let point = FailurePoint::AfterTemporaryFile(0);
        let Opened { store, mut feed } =
            fixture.open_with(|options| options.pause_at(point, &pause)).await;

        let commit = store.commit(staged(&[("a.txt", "a"), ("b.txt", "b")], &[]));
        tokio::select! {
            biased;
            finished = commit => panic!("the Commit finished while held: {finished:?}"),
            () = pause.reached() => {}
        }
        // The Commit's future is dropped. Nothing of it shows yet.
        assert_eq!(list(&store).await, Vec::<String>::new());
        pause.release();

        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("a.txt", ChangeKind::Changed), ("b.txt", ChangeKind::Changed)],
        );
        assert_nothing_more(&mut feed).await;
        let written = [("a.txt", "a"), ("b.txt", "b")].map(|(p, c)| (p.to_owned(), c.to_owned()));
        assert_eq!(contents(&store).await, written);
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    /// A dropped Commit that ends in `Pending` has happened, so its Changes arrive, once, from
    /// the task that finishes it.
    #[tokio::test]
    async fn a_dropped_commit_that_ends_pending_is_reported_once() {
        let fixture = Fs::new();
        let pause = Pause::new();
        let failing = FailurePoint::RenameFails { n: 0, times: usize::MAX };
        let Opened { store, mut feed } = fixture
            .open_with(|options| {
                options.pause_at(FailurePoint::AfterCommittedJournal, &pause).fail_at(failing)
            })
            .await;

        let commit = store.commit(staged(&[("a.txt", "a"), ("b.txt", "b")], &[]));
        tokio::select! {
            biased;
            finished = commit => panic!("the Commit finished while held: {finished:?}"),
            () = pause.reached() => {}
        }
        pause.release();

        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("a.txt", ChangeKind::Changed), ("b.txt", ChangeKind::Changed)],
        );
        assert_nothing_more(&mut feed).await;
        let written = [("a.txt", "a"), ("b.txt", "b")].map(|(p, c)| (p.to_owned(), c.to_owned()));
        assert_eq!(contents(&store).await, written);
    }

    /// Writing the journal as `committed` can report a failure once it is written, as when forcing
    /// its directory to disk fails. The Commit has happened then, so it is finished, and reported.
    #[tokio::test]
    async fn a_commit_whose_journal_was_committed_despite_an_error_finishes_and_is_reported() {
        let fixture = Fs::new();
        let failing = FailurePoint::CommittedJournalFails;
        let Opened { store, mut feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;
        store.commit(staged(&[("a.txt", "a")], &[])).await.unwrap();

        assert_eq!(changes(&next_batch(&mut feed).await), [("a.txt", ChangeKind::Changed)]);
        let written = [("a.txt".to_owned(), "a".to_owned())];
        assert_eq!(contents(&store).await, written);
        assert_eq!(temporary_files(fixture.directory.path()), Vec::<PathBuf>::new());
    }

    // Helpers shared by the tests above.

    /// A shape of Commit that the crash tests stop at every point: what the Store holds before,
    /// and the Commit.
    struct Shape {
        /// Makes what the Store holds before, with the Staging it gives and anything else.
        set_up: fn(&Fs) -> Staging,
        commit: fn() -> Staging,
        /// How many Files the Commit writes.
        writes: usize,
    }

    impl Shape {
        /// Writes one File and makes another in a new directory.
        const WRITE: Shape = Shape {
            set_up: |_| staged(&[("kept.txt", "old")], &[]),
            commit: || staged(&[("kept.txt", "new"), ("new/file.txt", "new")], &[]),
            writes: 2,
        };
        const WRITE_AND_DELETE: Shape = Shape {
            set_up: |_| staged(&[("kept.txt", "old"), ("gone.txt", "gone")], &[]),
            commit: || staged(&[("kept.txt", "new")], &["gone.txt"]),
            writes: 1,
        };
        /// Only deletes, so its journal is written only once, as `committed`.
        const PREFIX_DELETE: Shape = Shape {
            set_up: |_| staged(&[("p/1", "1"), ("p/q/2", "2"), ("kept.txt", "old")], &[]),
            commit: || {
                let mut staging = Staging::new();
                staging.delete_prefix("p/").unwrap();
                staging
            },
            writes: 0,
        };
        /// [`moves`].
        const MOVES: Shape = Shape { set_up: |_| before_moves(), commit: moves, writes: 3 };
        /// Writes `settings.toml`, a symlink to a File outside the Location, and `kept.txt`.
        #[cfg(unix)]
        const THROUGH_A_SYMLINK: Shape = Shape {
            set_up: |fixture| {
                let dotfiles = fixture.directory.path().join("dotfiles");
                std::fs::create_dir_all(&dotfiles).unwrap();
                std::fs::write(dotfiles.join("settings.toml"), "old").unwrap();
                let link = fixture.on_disk("settings.toml");
                std::fs::create_dir_all(link.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink("../dotfiles/settings.toml", link).unwrap();
                staged(&[("kept.txt", "old")], &[])
            },
            commit: || staged(&[("kept.txt", "new"), ("settings.toml", "new")], &[]),
            writes: 2,
        };
    }

    /// What a Commit gives when it meets a failure point, and what it leaves.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Outcome {
        /// `Backend`, stopped there before its journal was committed: it never happens.
        Discarded,
        /// `Backend`, stopped there once its journal was committed: it is finished later.
        Interrupted,
        /// `Pending`: it is finished later, and reads show it finished already.
        Pending,
        /// It succeeds: the point is never reached, or the failure there is got over.
        Finished,
    }

    /// Every failure point a Commit that writes `writes` Files can meet, with its [`Outcome`]. A
    /// Commit that writes none never reaches the points before its journal is committed.
    fn every_point(writes: usize) -> Vec<(FailurePoint, Outcome)> {
        use FailurePoint::*;
        use Outcome::*;
        let before = if writes > 0 { Discarded } else { Finished };
        let mut points = vec![(AfterPreparedJournal, before)];
        points.extend((0..writes).map(|n| (AfterTemporaryFile(n), Discarded)));
        points.extend([
            (CommittedJournalFails, Finished),
            (AfterCommittedJournal, Interrupted),
            (AfterDeletes, Interrupted),
        ]);
        points.extend((0..writes).map(|n| (AfterRename(n), Interrupted)));
        points.extend((0..writes).map(|n| (RenameFails { n, times: usize::MAX }, Pending)));
        points
    }

    /// Stops `shape`'s Commit at every point in turn, each at a Location of its own, and checks
    /// that it is all there or not there at all, through a Store opened afterwards. Before that
    /// Store finishes it, a Store that was open already, as in another process, reads it the same
    /// way. A rename that keeps failing gives `Pending`, and reads show the Commit all there
    /// already. What all there looks like comes from committing the shape without stopping.
    async fn stop_everywhere(shape: Shape) {
        let reference = Fs::new();
        let Opened { store, feed: _feed } = reference.open().await;
        store.commit((shape.set_up)(&reference)).await.unwrap();
        store.commit((shape.commit)()).await.unwrap();
        let applied = state(&store).await;

        for (point, outcome) in every_point(shape.writes) {
            let fixture = Fs::new();
            let Opened { store, feed: _feed } = fixture.open().await;
            store.commit((shape.set_up)(&fixture)).await.unwrap();
            let before = state(&store).await;
            let links = symlinks(fixture.directory.path());
            drop(store);
            let Opened { store: open_already, feed: _open_already_feed } = fixture.open().await;

            let Opened { store, feed: _feed } =
                fixture.open_with(|options| options.fail_at(point)).await;
            let stopped = store.commit((shape.commit)()).await;
            match (outcome, &stopped) {
                (Outcome::Discarded | Outcome::Interrupted, Err(Error::Backend(error))) => {
                    let said = error.to_string();
                    let stop = format!("stopped at the failure point {point:?}");
                    assert!(said.contains(&stop), "{point:?}: {said}");
                }
                (Outcome::Pending, Err(Error::Pending)) => {
                    assert_eq!(state(&store).await, applied, "{point:?}, pending");
                }
                (Outcome::Finished, Ok(_)) => {}
                _ => panic!("{point:?} should give {outcome:?}, got {stopped:?}"),
            }
            drop(store);
            let expected = if outcome == Outcome::Discarded { &before } else { &applied };
            assert_eq!(&state(&open_already).await, expected, "{point:?}, open already");

            let Opened { store, feed: _feed } = fixture.open().await;
            assert_eq!(&state(&store).await, expected, "{point:?}");
            assert_eq!(
                temporary_files(fixture.directory.path()),
                Vec::<PathBuf>::new(),
                "{point:?}"
            );
            assert_eq!(symlinks(fixture.directory.path()), links, "{point:?}");
        }
    }

    /// What reads through a Store show of it: each Path's contents, and its Revision from stat,
    /// and the Prefix Revision of each Prefix the crash tests' shapes use, written as text, since
    /// they are taken from different Stores.
    #[derive(Debug, PartialEq)]
    struct State {
        contents: Vec<(String, String)>,
        revisions: Vec<Revision>,
        prefix_revisions: Vec<String>,
    }

    async fn state(store: &Store) -> State {
        let mut revisions = Vec::new();
        for path in list(store).await {
            let file = store.read(path.as_str()).await.unwrap().unwrap();
            let stat = store.stat(path.as_str()).await.unwrap().unwrap();
            assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
            revisions.push(stat.revision());
        }
        let mut prefix_revisions = Vec::new();
        for prefix in ["", "a/", "d/", "new/", "p/", "p/q/"] {
            prefix_revisions.push(store.stat_prefix(prefix).await.unwrap().to_string());
        }
        State { contents: contents(store).await, revisions, prefix_revisions }
    }

    /// A Staging that writes `writes` and deletes `deletes`.
    fn staged(writes: &[(&str, &str)], deletes: &[&str]) -> Staging {
        let mut staging = Staging::new();
        for (path, contents) in writes {
            staging.write(*path, *contents).unwrap();
        }
        for path in deletes {
            staging.delete(*path).unwrap();
        }
        staging
    }

    /// Each Path in the Store, with its contents.
    async fn contents(store: &Store) -> Vec<(String, String)> {
        let mut contents = Vec::new();
        for path in list(store).await {
            let file = store.read(path.as_str()).await.unwrap().unwrap();
            contents.push((path, file.contents().to_owned()));
        }
        contents
    }

    /// A Commit that writes the Files [`moves`] moves.
    fn before_moves() -> Staging {
        staged(&[("a", "a"), ("d/e", "e"), ("kept.txt", "old")], &[])
    }

    /// A Commit that moves the File `a` to `a/b`, and the File `d/e` to `d`, and changes
    /// `kept.txt`.
    fn moves() -> Staging {
        let mut staging = Staging::new();
        staging.delete("a").unwrap();
        staging.write("a/b", "moved a").unwrap();
        staging.delete_prefix("d/").unwrap();
        staging.write("d", "moved e").unwrap();
        staging.write("kept.txt", "new").unwrap();
        staging
    }

    /// Checks that the Commit [`moves`] makes happened, all of it.
    async fn assert_moved(store: &Store) {
        assert_eq!(list(store).await, ["a/b", "d", "kept.txt"]);
        for (path, contents) in [("a/b", "moved a"), ("d", "moved e"), ("kept.txt", "new")] {
            let file = store.read(path).await.unwrap().unwrap();
            assert_eq!(file.contents(), contents, "{path}");
        }
    }

    /// Lists every Path in the Store, as strings.
    async fn list(store: &Store) -> Vec<String> {
        let paths = store.list("").await.unwrap();
        paths.iter().map(|path| path.as_str().to_owned()).collect()
    }

    /// Every symlink under `directory`, which isn't followed.
    fn symlinks(directory: &FsPath) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            if file_type.is_symlink() {
                found.push(entry.path());
            } else if file_type.is_dir() {
                found.extend(symlinks(&entry.path()));
            }
        }
        found.sort();
        found
    }

    /// Every file under `directory` named like one of tidings' temporary files.
    fn temporary_files(directory: &FsPath) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                found.extend(temporary_files(&entry.path()));
            } else if entry.file_name().to_string_lossy().contains(".tidings-") {
                found.push(entry.path());
            }
        }
        found
    }
}

/// A Store on the filesystem and one on SQLite, each at a Location of its own, as an app keeps
/// its config where people can edit it and its data where Snapshots hold.
#[cfg(all(feature = "fs", feature = "sqlite"))]
mod fs_and_sqlite {
    use crate::fs::Fs;
    use crate::sqlite::Sqlite;

    separate_stores_suite!(Fs::new(), Sqlite::new());
}
