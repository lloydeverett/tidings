//! The behaviour suite: one set of tests, written once against the public API, that every Backend
//! must pass. Each Backend instantiates the whole suite in its own module below, once through the
//! async API and once, in a `blocking` module of its own, through the blocking API.

mod api;
#[path = "../common/mod.rs"]
mod common;
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
        use crate::api::off_runtime;
        use crate::suite::{Fixture, Opened};

        struct BlockingMemory;

        impl Fixture for BlockingMemory {
            async fn open(&self) -> Opened {
                off_runtime(tidings::blocking::Store::open_memory).into()
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
    use std::time::Duration;

    use tempfile::TempDir;
    use tidings::{AppIdentity, Area, ChangeKind, FeedItem, Origin, SqliteOptions, Staging};

    use crate::api::Store;
    use crate::common::{assert_nothing_more, changes_in_full, next_batch, next_item};
    use crate::suite::{Fixture, Opened};

    /// Opens each test's Store under a temporary Root override of its own, removed when the test
    /// ends. Each Store it opens is on the same Root override, and checks for the others' Commits
    /// often.
    struct Sqlite {
        root: TempDir,
    }

    impl Sqlite {
        fn new() -> Sqlite {
            Sqlite { root: tempfile::tempdir().unwrap() }
        }

        /// The options each Store is opened with: on the Root override, checking often.
        fn usual_options(&self) -> SqliteOptions {
            SqliteOptions::default()
                .root_override(self.root.path())
                .poll_interval(Duration::from_millis(10))
        }

        /// Opens a Store on the Root override with the options `options` makes of the usual
        /// ones.
        async fn open_with(&self, options: impl FnOnce(SqliteOptions) -> SqliteOptions) -> Opened {
            let options = options(self.usual_options());
            tidings::Store::open_sqlite(&app(), options).await.unwrap().into()
        }
    }

    impl Fixture for Sqlite {
        async fn open(&self) -> Opened {
            self.open_with(|options| options).await
        }
    }

    fn app() -> AppIdentity {
        AppIdentity::new("tidings tests", "tidings", "org")
    }

    behaviour_suite!(Sqlite::new());
    two_stores_suite!(Sqlite::new());

    mod blocking {
        use super::{Sqlite, app};
        use crate::api::off_runtime;
        use crate::suite::{Fixture, Opened};

        /// Opens each Store through the blocking API, as [`Sqlite`] does through the async one.
        struct BlockingSqlite(Sqlite);

        impl Fixture for BlockingSqlite {
            async fn open(&self) -> Opened {
                let options = self.0.usual_options();
                off_runtime(|| tidings::blocking::Store::open_sqlite(&app(), options))
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

    /// The change log is pruned, so it doesn't grow without limit. A Store the log was pruned
    /// past has missed Commits, and gets a Resync for the Area instead of their Changes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_store_that_the_change_log_was_pruned_past_gets_a_resync() {
        let fixture = Sqlite::new();
        // This one reads the log only when it commits, which the test chooses.
        let Opened { store: behind, mut feed } =
            fixture.open_with(|options| options.poll_interval(Duration::from_secs(3600))).await;
        // Each of this one's Commits prunes every Commit before it.
        let Opened { store: pruning, feed: _pruning_feed } =
            fixture.open_with(|options| options.change_log_retention(Duration::ZERO)).await;
        let commit = async |store: &Store, area: Area, path: &str| {
            let mut staging = Staging::new(area);
            staging.write(path, "x").unwrap();
            store.commit(staging).await.unwrap();
            // So that the next Commit is later, and prunes this one.
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        // Unread Changes to both Areas, before the other Store commits.
        commit(&behind, Area::Data, "unread.txt").await;
        commit(&behind, Area::Config, "unread.toml").await;
        // The first is pruned by the second.
        commit(&pruning, Area::Data, "pruned.txt").await;
        commit(&pruning, Area::Data, "kept.txt").await;
        // Committing, the Store reads the log, and finds what it hadn't read pruned.
        commit(&behind, Area::Data, "after.txt").await;

        // The Resync comes first. It takes the place of the Area's unread Changes, and of those
        // recorded after it until it is read. Other Areas' Changes are kept.
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync(Area::Data));
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Config, "unread.toml", ChangeKind::Changed, Origin::Local)],
        );
        assert_nothing_more(&mut feed).await;

        // Once it is read, Changes to the Area are reported again.
        commit(&pruning, Area::Data, "later.txt").await;
        commit(&behind, Area::Data, "again.txt").await;
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [
                (Area::Data, "again.txt", ChangeKind::Changed, Origin::Local),
                (Area::Data, "later.txt", ChangeKind::Changed, Origin::External),
            ],
        );
        assert_nothing_more(&mut feed).await;
    }
}

#[cfg(feature = "fs")]
mod fs {
    use std::path::{Path as FsPath, PathBuf};
    use std::time::{Duration, SystemTime};

    use tempfile::TempDir;
    use tidings::{
        AppIdentity, Area, ChangeKind, Error, FailurePoint, FeedItem, FsOptions, InvalidPathReason,
        Origin, Pause, PrefixRevision, Revision, Staging,
    };

    use crate::api::Store;
    use crate::common::{assert_nothing_more, changes, changes_in_full, next_batch, next_item};
    use crate::suite::{Fixture, Opened};

    /// Opens each test's Store under a temporary Root override of its own, removed when the test
    /// ends. Each Store it opens is on the same Root override.
    struct Fs {
        root: TempDir,
    }

    impl Fs {
        fn new() -> Fs {
            Fs { root: tempfile::tempdir().unwrap() }
        }

        /// The options each Store is opened with: on the Root override, with a short debounce
        /// window.
        fn usual_options(&self) -> FsOptions {
            FsOptions::default()
                .root_override(self.root.path())
                .debounce_window(Duration::from_millis(20))
        }

        /// Opens a Store on the Root override with the options `options` makes of the usual
        /// ones.
        async fn open_with(&self, options: impl FnOnce(FsOptions) -> FsOptions) -> Opened {
            let options = options(self.usual_options());
            tidings::Store::open_fs(&app(), options).await.unwrap().into()
        }

        /// Where `path` in `area` is on disk.
        fn on_disk(&self, area: Area, path: &str) -> PathBuf {
            let directory = match area {
                Area::Config => "config",
                Area::Data => "data",
                Area::Cache => "cache",
            };
            self.root.path().join(directory).join(path)
        }

        /// Writes `contents` to `path` in `area` directly, as another program would.
        fn write_directly(&self, area: Area, path: &str, contents: impl AsRef<[u8]>) {
            let file = self.on_disk(area, path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, contents).unwrap();
        }
    }

    impl Fixture for Fs {
        async fn open(&self) -> Opened {
            self.open_with(|options| options).await
        }
    }

    fn app() -> AppIdentity {
        AppIdentity::new("tidings tests", "tidings", "org")
    }

    behaviour_suite!(Fs::new());
    // Not `in_step:`: see the README's Consistency section.
    two_stores_suite!(committing: Fs::new());
    two_stores_suite!(seeing_each_other: Fs::new());

    mod blocking {
        use super::{Fs, app};
        use crate::api::off_runtime;
        use crate::suite::{Fixture, Opened};

        /// Opens each Store through the blocking API, as [`Fs`] does through the async one.
        struct BlockingFs(Fs);

        impl Fixture for BlockingFs {
            async fn open(&self) -> Opened {
                let options = self.0.usual_options();
                off_runtime(|| tidings::blocking::Store::open_fs(&app(), options)).unwrap().into()
            }
        }

        behaviour_suite!(BlockingFs(Fs::new()));
        two_stores_suite!(committing: BlockingFs(Fs::new()));
        two_stores_suite!(seeing_each_other: BlockingFs(Fs::new()));
    }

    /// The suite's Snapshot tests are skipped where Snapshots aren't supported, so this makes sure
    /// the filesystem is one of those, and that the suite's refusal test runs there.
    #[tokio::test]
    async fn fs_does_not_support_snapshots() {
        let Opened { store, feed: _feed } = Fs::new().open().await;
        assert!(!store.supports_snapshots());
    }

    /// A person editing, making or deleting a File in an Area's directory, while the app runs,
    /// is reported as an external Change once the events have settled.
    #[tokio::test]
    async fn edits_made_directly_in_an_area_arrive_as_external_changes() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;

        fixture.write_directly(Area::Config, "settings.toml", "a = 1\n");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Config, "settings.toml", ChangeKind::Changed, Origin::External)],
        );
        fixture.write_directly(Area::Config, "settings.toml", "a = 2\n");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Config, "settings.toml", ChangeKind::Changed, Origin::External)],
        );
        std::fs::remove_file(fixture.on_disk(Area::Config, "settings.toml")).unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Config, "settings.toml", ChangeKind::Removed, Origin::External)],
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
        fixture.write_directly(Area::Config, "settings.toml", "a = 1\n");
        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);

        // As vim does by default: move the File to a backup, write it again in two goes, remove
        // the backup.
        let file = fixture.on_disk(Area::Config, "settings.toml");
        let backup = fixture.on_disk(Area::Config, "settings.toml~");
        std::fs::rename(&file, &backup).unwrap();
        let mut writing = std::fs::File::create(&file).unwrap();
        std::io::Write::write_all(&mut writing, b"a = ").unwrap();
        std::io::Write::flush(&mut writing).unwrap();
        std::io::Write::write_all(&mut writing, b"2\n").unwrap();
        drop(writing);
        std::fs::remove_file(&backup).unwrap();

        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);
        let read = store.read(Area::Config, "settings.toml").await.unwrap().unwrap();
        assert_eq!(read.contents(), "a = 2\n");
        assert_nothing_more(&mut feed).await;
    }

    /// An event that leaves a File's contents as they were is no Change: a File read, its times
    /// or permissions changed, or its contents written again as they were. Some of those events
    /// look like writes, so they are told apart by comparing Revisions. Only a File that has
    /// changed since the Store opened has a known Revision, so a File there before gets only the
    /// events that can't be writes: see the README's Limitations.
    #[tokio::test]
    async fn events_that_leave_a_files_contents_as_they_were_are_dropped() {
        // Config's Files are read when the Store opens, so their Revisions are known: even a
        // File there before gets none of these events.
        let fixture = Fs::new();
        fixture.write_directly(Area::Config, "settings.toml", "a = 1\n");
        let Opened { store: _store, mut feed } = fixture.open().await;
        let file = fixture.on_disk(Area::Config, "settings.toml");
        std::fs::File::open(&file).unwrap().set_modified(SystemTime::UNIX_EPOCH).unwrap();
        fixture.write_directly(Area::Config, "settings.toml", "a = 1\n");
        assert_nothing_more(&mut feed).await;
        fixture.write_directly(Area::Config, "settings.toml", "a = 2\n");
        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);

        // In the other Areas, only once a File has changed.
        let fixture = Fs::new();
        fixture.write_directly(Area::Data, "before.txt", "there before");
        let Opened { store: _store, mut feed } = fixture.open().await;
        fixture.write_directly(Area::Data, "during.txt", "a");
        assert_eq!(changes(&next_batch(&mut feed).await), [("during.txt", ChangeKind::Changed)]);

        let set_readonly = |path: &str, readonly: bool| {
            let file = fixture.on_disk(Area::Data, path);
            let mut permissions = std::fs::metadata(&file).unwrap().permissions();
            permissions.set_readonly(readonly);
            std::fs::set_permissions(file, permissions).unwrap();
        };
        // As `touch` does, both times at once, which is no write.
        let both = std::fs::FileTimes::new()
            .set_accessed(SystemTime::UNIX_EPOCH)
            .set_modified(SystemTime::UNIX_EPOCH);
        for path in ["before.txt", "during.txt"] {
            std::fs::read(fixture.on_disk(Area::Data, path)).unwrap();
            let file = std::fs::File::open(fixture.on_disk(Area::Data, path)).unwrap();
            file.set_times(both).unwrap();
            set_readonly(path, true);
            set_readonly(path, false);
        }
        // The modification time alone, which is also how a write shows, and the contents as
        // they were.
        let file = std::fs::File::open(fixture.on_disk(Area::Data, "during.txt")).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH).unwrap();
        fixture.write_directly(Area::Data, "during.txt", "a");
        assert_nothing_more(&mut feed).await;
    }

    /// Names on disk that no Path has give no Change: tidings' own `.tidings/`, names like its
    /// temporary files', and names another program made that aren't Paths.
    #[tokio::test]
    async fn events_for_names_that_are_not_paths_are_ignored() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;
        fixture.write_directly(Area::Data, ".tidings/other.txt", "x");
        fixture.write_directly(
            Area::Data,
            ".kept.txt.tidings-0123456789abcdef0123456789abcdef-0",
            "x",
        );
        fixture.write_directly(Area::Data, "cafe\u{301}.txt", "x");
        fixture.write_directly(Area::Data, "re\u{301}sume\u{301}/cv.txt", "x");
        fixture.write_directly(Area::Data, "kept.txt", "x");

        assert_eq!(changes(&next_batch(&mut feed).await), [("kept.txt", ChangeKind::Changed)]);
        assert_nothing_more(&mut feed).await;
    }

    /// A directory moved into an Area with Files in it gives a Change for each of them, even
    /// though they got there before the directory was watched. One removed gives a Change for
    /// each File that was in it.
    #[tokio::test]
    async fn a_directory_moved_in_or_removed_gives_a_change_for_each_file_in_it() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;
        let outside = fixture.root.path().join("outside");
        std::fs::create_dir_all(outside.join("deeper")).unwrap();
        std::fs::write(outside.join("a.toml"), "a").unwrap();
        std::fs::write(outside.join("deeper/b.toml"), "b").unwrap();

        std::fs::rename(&outside, fixture.on_disk(Area::Config, "themes")).unwrap();
        let expected =
            [("themes/a.toml", ChangeKind::Changed), ("themes/deeper/b.toml", ChangeKind::Changed)];
        assert_eq!(changes(&next_batch(&mut feed).await), expected);

        std::fs::remove_dir_all(fixture.on_disk(Area::Config, "themes")).unwrap();
        let expected =
            [("themes/a.toml", ChangeKind::Removed), ("themes/deeper/b.toml", ChangeKind::Removed)];
        assert_eq!(changes(&next_batch(&mut feed).await), expected);
        assert_nothing_more(&mut feed).await;
    }

    /// A File replaced by renaming another over it looks newly made to the watcher. Removed or
    /// renamed away straight after, before its events have settled, it is still reported
    /// removed.
    #[tokio::test]
    async fn a_file_replaced_then_removed_straight_away_is_reported_removed() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } = fixture.open().await;
        fixture.write_directly(Area::Data, "removed.txt", "old");
        fixture.write_directly(Area::Data, "moved.txt", "old");
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("moved.txt", ChangeKind::Changed), ("removed.txt", ChangeKind::Changed)],
        );
        // Quiet for a while, as the Files usually are when someone replaces them.
        tokio::time::sleep(Duration::from_millis(200)).await;

        for path in ["removed.txt", "moved.txt"] {
            let replacement = fixture.on_disk(Area::Data, "replacement~");
            std::fs::write(&replacement, "new").unwrap();
            std::fs::rename(&replacement, fixture.on_disk(Area::Data, path)).unwrap();
        }
        std::fs::remove_file(fixture.on_disk(Area::Data, "removed.txt")).unwrap();
        std::fs::rename(
            fixture.on_disk(Area::Data, "moved.txt"),
            fixture.root.path().join("moved away.txt"),
        )
        .unwrap();
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("moved.txt", ChangeKind::Removed), ("removed.txt", ChangeKind::Removed)],
        );
        assert_nothing_more(&mut feed).await;
    }

    #[tokio::test]
    async fn each_area_is_a_directory_people_can_see_the_files_in() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        for area in [Area::Config, Area::Data, Area::Cache] {
            assert!(fixture.on_disk(area, "").is_dir(), "{area:?}");
        }

        let mut staging = Staging::new(Area::Config);
        staging.write("themes/dark.toml", "dark = true\n").unwrap();
        store.commit(staging).await.unwrap();
        let on_disk = std::fs::read_to_string(fixture.on_disk(Area::Config, "themes/dark.toml"));
        assert_eq!(on_disk.unwrap(), "dark = true\n");
        assert!(!fixture.on_disk(Area::Data, "themes").exists());
    }

    #[tokio::test]
    async fn files_other_programs_make_are_read_and_listed() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly(Area::Config, "settings.toml", "a = 1\n");
        fixture.write_directly(Area::Config, "themes/dark.toml", "dark = true\n");

        assert_eq!(list(&store, Area::Config).await, ["settings.toml", "themes/dark.toml"]);
        let file = store.read(Area::Config, "themes/dark.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "dark = true\n");
        let stat = store.stat(Area::Config, "themes/dark.toml").await.unwrap().unwrap();
        assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
        // A directory is not a File.
        assert_eq!(store.read(Area::Config, "themes").await.unwrap(), None);
    }

    /// Another program can make names on disk that no Path has. Those Files are left out of
    /// listings and Prefix Revisions, and so is everything under such directories.
    #[tokio::test]
    async fn names_on_disk_that_are_not_paths_are_left_out() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly(Area::Data, "kept.txt", "kept");
        let before = store.stat_prefix(Area::Data, "").await.unwrap();

        // Not in NFC form.
        fixture.write_directly(Area::Data, "cafe\u{301}.txt", "x");
        fixture.write_directly(Area::Data, "re\u{301}sume\u{301}/cv.txt", "x");
        // Named like tidings' own, or like its temporary files.
        fixture.write_directly(Area::Data, ".tidings/other.txt", "x");
        fixture.write_directly(
            Area::Data,
            ".kept.txt.tidings-0123456789abcdef0123456789abcdef-0",
            "x",
        );
        // Names Windows can't hold, which Windows can't make either.
        #[cfg(not(windows))]
        {
            fixture.write_directly(Area::Data, "CON", "x");
            fixture.write_directly(Area::Data, "what?/a.txt", "x");
        }
        // Not valid UTF-8.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::OsStr::from_bytes(b"not \xff utf-8.txt");
            std::fs::write(fixture.on_disk(Area::Data, "").join(name), "x").unwrap();
        }

        assert_eq!(list(&store, Area::Data).await, ["kept.txt"]);
        assert_eq!(store.stat_prefix(Area::Data, "").await.unwrap(), before);
    }

    /// A File another program wrote that isn't valid UTF-8 can't be read as text, but it is
    /// still there: it is listed, has a Revision, and a Commit can replace it.
    #[tokio::test]
    async fn a_file_that_is_not_utf8_is_listed_but_reading_it_says_so() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly(Area::Cache, "image.png", [0x89, b'P', b'N', b'G', 0xff, 0xfe]);

        match store.read(Area::Cache, "image.png").await {
            Err(Error::NotText { path }) => assert_eq!(path.as_str(), "image.png"),
            other => panic!("expected NotText, got {other:?}"),
        }
        assert_eq!(list(&store, Area::Cache).await, ["image.png"]);
        let stat = store.stat(Area::Cache, "image.png").await.unwrap();
        assert!(stat.is_some());

        let mut staging = Staging::new(Area::Cache);
        staging.write("image.png", "text now").unwrap();
        store.commit(staging).await.unwrap();
        let file = store.read(Area::Cache, "image.png").await.unwrap().unwrap();
        assert_eq!(file.contents(), "text now");
    }

    #[tokio::test]
    async fn each_file_a_commit_writes_has_its_timestamp_as_modification_time_on_disk() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new(Area::Data);
        staging.write("a.txt", "a").unwrap();
        staging.write("b/c.txt", "c").unwrap();
        let committed = store.commit(staging).await.unwrap();

        for path in ["a.txt", "b/c.txt"] {
            let modified = std::fs::metadata(fixture.on_disk(Area::Data, path)).unwrap().modified();
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
        let dotfiles = fixture.root.path().join("dotfiles");
        std::fs::create_dir_all(dotfiles.join("app")).unwrap();
        std::fs::write(dotfiles.join("app/settings.toml"), "a = 1\n").unwrap();
        // One link that is relative, to another that isn't.
        std::os::unix::fs::symlink(dotfiles.join("app/settings.toml"), dotfiles.join("current"))
            .unwrap();
        let link = fixture.on_disk(Area::Config, "settings.toml");
        std::os::unix::fs::symlink("../dotfiles/current", &link).unwrap();

        let file = store.read(Area::Config, "settings.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "a = 1\n");
        let mut staging = Staging::new(Area::Config);
        staging.write_back(&file, "a = 2\n");
        store.commit(staging).await.unwrap();

        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(std::fs::symlink_metadata(dotfiles.join("current")).unwrap().is_symlink());
        let target = std::fs::read_to_string(dotfiles.join("app/settings.toml")).unwrap();
        assert_eq!(target, "a = 2\n");
        let file = store.read(Area::Config, "settings.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "a = 2\n");
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
    }

    /// A symlinked File is watched through its link: an edit to the file it points to, outside
    /// the Area or in it, arrives as a Change for the linking Path. Links made, changed and
    /// removed while the Store runs are followed too.
    #[cfg(unix)]
    #[tokio::test]
    async fn edits_to_a_symlinks_target_arrive_as_changes_for_the_linking_path() {
        use std::os::unix::fs::symlink;
        let fixture = Fs::new();
        let dotfiles = fixture.root.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::write(dotfiles.join("settings.toml"), "a = 1\n").unwrap();
        fixture.write_directly(Area::Config, "real.toml", "real");
        symlink(dotfiles.join("settings.toml"), fixture.on_disk(Area::Config, "settings.toml"))
            .unwrap();
        symlink("real.toml", fixture.on_disk(Area::Config, "alias.toml")).unwrap();
        let Opened { store: _store, mut feed } = fixture.open().await;

        // Linked when the Store opened.
        std::fs::write(dotfiles.join("settings.toml"), "a = 2\n").unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Config, "settings.toml", ChangeKind::Changed, Origin::External)],
        );
        fixture.write_directly(Area::Config, "real.toml", "real, edited");
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("alias.toml", ChangeKind::Changed), ("real.toml", ChangeKind::Changed)],
        );

        // Replaced the way editors save, by renaming a new file over it.
        std::fs::write(dotfiles.join("settings.toml.new"), "a = 3\n").unwrap();
        std::fs::rename(dotfiles.join("settings.toml.new"), dotfiles.join("settings.toml"))
            .unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);

        // Linked while the Store runs, to a file in another directory outside the Area.
        let elsewhere = fixture.root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("theme.toml"), "dark").unwrap();
        symlink(elsewhere.join("theme.toml"), fixture.on_disk(Area::Config, "theme.toml")).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);
        std::fs::write(elsewhere.join("theme.toml"), "light").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);

        // Linked elsewhere, the way `ln -sf` does it: a new link renamed over the old one.
        std::fs::write(elsewhere.join("other.toml"), "other").unwrap();
        symlink(elsewhere.join("other.toml"), elsewhere.join("new link")).unwrap();
        std::fs::rename(elsewhere.join("new link"), fixture.on_disk(Area::Config, "theme.toml"))
            .unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);
        std::fs::write(elsewhere.join("other.toml"), "other, edited").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("theme.toml", ChangeKind::Changed)]);
        std::fs::write(elsewhere.join("theme.toml"), "no longer linked").unwrap();
        assert_nothing_more(&mut feed).await;

        // Unlinked: the target's edits are nothing to the Area any more.
        std::fs::remove_file(fixture.on_disk(Area::Config, "theme.toml")).unwrap();
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
        let dotfiles = fixture.root.path().join("dotfiles");
        for version in ["app", "app2"] {
            std::fs::create_dir_all(dotfiles.join(version)).unwrap();
            std::fs::write(dotfiles.join(version).join("s.toml"), version).unwrap();
        }
        symlink("app/s.toml", dotfiles.join("current")).unwrap();
        fixture.write_directly(Area::Config, "unrelated.toml", "");
        symlink(dotfiles.join("current"), fixture.on_disk(Area::Config, "s.toml")).unwrap();
        let Opened { store, mut feed } = fixture.open().await;

        std::fs::write(dotfiles.join("app/s.toml"), "app, edited").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("s.toml", ChangeKind::Changed)]);

        symlink("app2/s.toml", dotfiles.join("current.new")).unwrap();
        std::fs::rename(dotfiles.join("current.new"), dotfiles.join("current")).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("s.toml", ChangeKind::Changed)]);
        let file = store.read(Area::Config, "s.toml").await.unwrap().unwrap();
        assert_eq!(file.contents(), "app2");
        std::fs::write(dotfiles.join("app2/s.toml"), "app2, edited").unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("s.toml", ChangeKind::Changed)]);
        std::fs::write(dotfiles.join("app/s.toml"), "app, no longer linked").unwrap();
        assert_nothing_more(&mut feed).await;
    }

    /// Clearing the Cache by removing its directory, while the app runs, is safe: the directory
    /// is made again and watched again, and the Area gets a Resync, since its Files are gone.
    #[tokio::test]
    async fn an_area_directory_removed_while_running_is_made_again_with_a_resync() {
        let fixture = Fs::new();
        let Opened { store, mut feed } = fixture.open().await;
        let mut staging = Staging::new(Area::Cache);
        staging.write("thumbnails/a.png", "a").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("thumbnails/a.png", ChangeKind::Changed)]
        );

        std::fs::remove_dir_all(fixture.on_disk(Area::Cache, "")).unwrap();
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync(Area::Cache));
        assert!(fixture.on_disk(Area::Cache, "").is_dir());
        assert_nothing_more(&mut feed).await;

        // Watched again.
        fixture.write_directly(Area::Cache, "new.txt", "x");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Cache, "new.txt", ChangeKind::Changed, Origin::External)],
        );
        let mut staging = Staging::new(Area::Cache);
        staging.write("thumbnails/a.png", "a").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Cache, "thumbnails/a.png", ChangeKind::Changed, Origin::Local)],
        );
        assert_nothing_more(&mut feed).await;

        // Renamed away, the same, and what happens to it where it went is no Change.
        let moved = fixture.root.path().join("old cache");
        std::fs::rename(fixture.on_disk(Area::Cache, ""), &moved).unwrap();
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync(Area::Cache));
        assert!(fixture.on_disk(Area::Cache, "").is_dir());
        std::fs::write(moved.join("new.txt"), "y").unwrap();
        assert_nothing_more(&mut feed).await;
        fixture.write_directly(Area::Cache, "new.txt", "z");
        assert_eq!(changes(&next_batch(&mut feed).await), [("new.txt", ChangeKind::Changed)]);
        assert_nothing_more(&mut feed).await;
    }

    /// If watching fails, the Areas it concerns get a Resync, since Changes to them may have been
    /// missed, and watching goes on.
    #[tokio::test]
    async fn a_failure_to_watch_gives_a_resync() {
        let fixture = Fs::new();
        let Opened { store: _store, mut feed } =
            fixture.open_with(|options| options.fail_at(FailurePoint::WatchingFails)).await;

        // The watcher loses its watches of the Area, as it can when it fails, so the Area is
        // watched again: directories made meanwhile too.
        fixture.write_directly(Area::Data, "missed/a.txt", "x");
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync(Area::Data));
        assert_nothing_more(&mut feed).await;
        fixture.write_directly(Area::Data, "seen.txt", "x");
        assert_eq!(changes(&next_batch(&mut feed).await), [("seen.txt", ChangeKind::Changed)]);
        fixture.write_directly(Area::Data, "missed/a.txt", "y");
        assert_eq!(changes(&next_batch(&mut feed).await), [("missed/a.txt", ChangeKind::Changed)]);
        assert_nothing_more(&mut feed).await;
    }

    /// An Area that can't be watched, as when the platform's limit on watches is reached, is
    /// tried again, waiting longer each time. The Store opens meanwhile. The Area gets a Resync
    /// straight away, since its Changes aren't reported, and another once it is watched, since
    /// they were missed until then.
    #[tokio::test]
    async fn an_area_that_cant_be_watched_is_resynced_and_tried_again() {
        let fixture = Fs::new();
        // Each Area fails when the Store opens, and Config once more after that. A longer window,
        // so that the Resyncs at open are read before the retries' come.
        let failing = FailurePoint::WatchingAnAreaFails { times: 4 };
        let window = Duration::from_millis(100);
        let Opened { store: _store, mut feed } =
            fixture.open_with(|options| options.fail_at(failing).debounce_window(window)).await;

        let mut resynced = Vec::new();
        for _ in 0..6 {
            match next_item(&mut feed).await {
                FeedItem::Resync(area) => resynced.push(area),
                other => panic!("expected a Resync, got {other:?}"),
            }
        }
        use Area::*;
        assert_eq!(resynced, [Config, Data, Cache, Data, Cache, Config]);
        assert_nothing_more(&mut feed).await;
        for area in [Config, Data, Cache] {
            fixture.write_directly(area, "seen.txt", "x");
            let batch = next_batch(&mut feed).await;
            assert_eq!(
                changes_in_full(&batch),
                [(area, "seen.txt", ChangeKind::Changed, Origin::External)]
            );
        }
    }

    /// A symlink in one Area can point into another. While that other Area isn't watched yet,
    /// following the link must not watch its directory apart from it: unwatching that when the
    /// link changes would stop the Area's own watch.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_into_an_area_not_watched_yet_leaves_its_watch_alone() {
        let fixture = Fs::new();
        fixture.write_directly(Area::Config, "t.toml", "t");
        fixture.write_directly(Area::Config, "u.toml", "u");
        std::fs::create_dir_all(fixture.on_disk(Area::Data, "")).unwrap();
        let link = fixture.on_disk(Area::Data, "l.toml");
        std::os::unix::fs::symlink(fixture.on_disk(Area::Config, "t.toml"), &link).unwrap();
        // Config, watched first, fails once, when the Store opens.
        let failing = FailurePoint::WatchingAnAreaFails { times: 1 };
        let Opened { store: _store, mut feed } =
            fixture.open_with(|options| options.fail_at(failing)).await;
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync(Area::Config));
        // Watched again after a window.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(next_item(&mut feed).await, FeedItem::Resync(Area::Config));
        assert_nothing_more(&mut feed).await;

        let new_link = fixture.on_disk(Area::Data, "new link");
        std::os::unix::fs::symlink(fixture.on_disk(Area::Config, "u.toml"), &new_link).unwrap();
        std::fs::rename(&new_link, &link).unwrap();
        assert_eq!(changes(&next_batch(&mut feed).await), [("l.toml", ChangeKind::Changed)]);
        fixture.write_directly(Area::Config, "t.toml", "t, edited");
        assert_eq!(
            changes_in_full(&next_batch(&mut feed).await),
            [(Area::Config, "t.toml", ChangeKind::Changed, Origin::External)],
        );
    }

    /// A Config File the Store can't read doesn't stop it opening. Its Revision isn't known, so
    /// events for it are reported.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_config_file_that_cant_be_read_doesnt_stop_the_store_opening() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fs::new();
        fixture.write_directly(Area::Config, "secret.toml", "secret");
        let secret = fixture.on_disk(Area::Config, "secret.toml");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
        let Opened { store: _store, mut feed } = fixture.open().await;
        fixture.write_directly(Area::Config, "settings.toml", "a = 1\n");
        assert_eq!(changes(&next_batch(&mut feed).await), [("settings.toml", ChangeKind::Changed)]);
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// Something on disk that isn't a File can have the name of a File a Commit writes, or of a
    /// directory the Commit must make for one. An empty directory makes way for the File. Anything
    /// else refuses the Commit before it happens, as a File would.
    #[tokio::test]
    async fn what_is_on_disk_but_not_a_file_makes_way_or_refuses_the_commit() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        std::fs::create_dir_all(fixture.on_disk(Area::Data, "empty/inside")).unwrap();
        let mut staging = Staging::new(Area::Data);
        staging.write("empty", "a File now").unwrap();
        store.commit(staging).await.unwrap();
        let file = store.read(Area::Data, "empty").await.unwrap().unwrap();
        assert_eq!(file.contents(), "a File now");

        // A directory with a name in it that no Path has.
        fixture.write_directly(Area::Data, "held/cafe\u{301}.txt", "x");
        let mut refused = vec![("held", "held")];
        // A symlink to nothing, where a directory would have to go.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("nowhere", fixture.on_disk(Area::Data, "dangling")).unwrap();
            refused.push(("dangling/a.txt", "dangling/a.txt"));
        }
        for (path, expected) in refused {
            let mut staging = Staging::new(Area::Data);
            staging.write(path, "x").unwrap();
            staging.write("fine.txt", "fine").unwrap();
            match store.commit(staging).await {
                Err(Error::InvalidPath { path, reason: InvalidPathReason::FileUnderFile }) => {
                    assert_eq!(path, expected);
                }
                other => panic!("writing {path} should be refused, got {other:?}"),
            }
        }
        assert_eq!(list(&store, Area::Data).await, ["empty"]);
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
    }

    /// A Commit stopped at any point, as if the process had died there, is all there or not
    /// there at all once a Store opens the Area again, with no temporary file left behind. So is
    /// one whose rename keeps failing, which reads show as all there before then. Each of these
    /// runs every shape of Commit that [`Shape`] names through every point.
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

        let mut staging = Staging::new(Area::Data);
        staging.write("later.txt", "later").unwrap();
        other.commit(staging).await.unwrap();
        assert!(other.read(Area::Data, "later.txt").await.unwrap().is_some());
        let mut staging = Staging::new(Area::Data);
        staging.delete("later.txt").unwrap();
        other.commit(staging).await.unwrap();
        assert_moved(&other).await;
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
    }

    /// Finishing a Commit again deletes a File only if it is still the one the Commit deleted. A
    /// File another program wrote there since stays.
    #[tokio::test]
    async fn finishing_a_commit_again_keeps_a_file_written_since_where_it_deleted_one() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new(Area::Data);
        staging.write("old.txt", "old").unwrap();
        store.commit(staging).await.unwrap();
        drop(store);

        let Opened { store, feed: _feed } =
            fixture.open_with(|options| options.fail_at(FailurePoint::AfterRename(0))).await;
        let mut rename = Staging::new(Area::Data);
        rename.delete("old.txt").unwrap();
        rename.write("new.txt", "old").unwrap();
        assert!(matches!(store.commit(rename).await, Err(Error::Backend(_))));
        drop(store);
        fixture.write_directly(Area::Data, "old.txt", "written since");

        let Opened { store, feed: _feed } = fixture.open().await;
        assert_eq!(list(&store, Area::Data).await, ["new.txt", "old.txt"]);
        let since = store.read(Area::Data, "old.txt").await.unwrap().unwrap();
        assert_eq!(since.contents(), "written since");
    }

    /// Two Paths can be the same file on disk, through a symlink. A Commit that writes or deletes
    /// both is refused, since which of them wins would depend on the order they land in.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_commit_to_two_paths_that_are_the_same_file_is_refused() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly(Area::Data, "b", "b");
        std::os::unix::fs::symlink("b", fixture.on_disk(Area::Data, "a")).unwrap();
        std::os::unix::fs::symlink("b", fixture.on_disk(Area::Data, "c")).unwrap();
        std::os::unix::fs::symlink("sub", fixture.on_disk(Area::Data, "linked")).unwrap();
        fixture.write_directly(Area::Data, "sub/x", "x");

        type Stage = fn(&mut Staging);
        let refused: [(&str, Stage); 4] = [
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
            ("through a directory", |staging| {
                staging.write("linked/x", "new x").unwrap();
                staging.delete("sub/x").unwrap();
            }),
        ];
        for (doing, stage) in refused {
            let mut staging = Staging::new(Area::Data);
            stage(&mut staging);
            match store.commit(staging).await {
                Err(Error::InvalidPath { reason: InvalidPathReason::SameFile, .. }) => {}
                other => panic!("{doing} should be refused, got {other:?}"),
            }
        }
        for (path, contents) in [("a", "b"), ("b", "b"), ("c", "b"), ("sub/x", "x")] {
            let file = store.read(Area::Data, path).await.unwrap().unwrap();
            assert_eq!(file.contents(), contents, "{path}");
        }
        assert!(std::fs::symlink_metadata(fixture.on_disk(Area::Data, "a")).unwrap().is_symlink());

        // A delete of the link itself, and a write of the File, are two files.
        let mut staging = Staging::new(Area::Data);
        staging.delete("a").unwrap();
        staging.write("b", "new b").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(store.read(Area::Data, "a").await.unwrap(), None);
    }

    /// A directory link in the Area lists its Files under both names. Deleting a File under both
    /// is safe, since the second delete finds nothing, so a Prefix delete covering both works.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_prefix_delete_covers_files_listed_under_a_directory_link_too() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly(Area::Data, "real/x", "x");
        std::os::unix::fs::symlink("real", fixture.on_disk(Area::Data, "linked")).unwrap();
        assert_eq!(list(&store, Area::Data).await, ["linked/x", "real/x"]);

        let mut staging = Staging::new(Area::Data);
        staging.delete_prefix("").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(list(&store, Area::Data).await, Vec::<String>::new());
    }

    /// Files with the same name in different directories are different files, even while their
    /// directories don't exist yet.
    #[tokio::test]
    async fn files_named_alike_in_directories_still_to_be_made_are_different_files() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new(Area::Data);
        for path in ["x.txt", "new/x.txt", "new/deeper/x.txt"] {
            staging.write(path, path).unwrap();
        }
        store.commit(staging).await.unwrap();
        assert_eq!(list(&store, Area::Data).await, ["new/deeper/x.txt", "new/x.txt", "x.txt"]);
    }

    /// A write through a symlink to a File that doesn't exist yet makes the File, if the
    /// directory it would be in exists. tidings never makes a directory outside the Area, so
    /// otherwise the write is refused, before anything is written.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_through_a_symlink_never_makes_a_directory() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let outside = fixture.root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let link = |path: &str, to: &FsPath| {
            std::os::unix::fs::symlink(to, fixture.on_disk(Area::Config, path)).unwrap();
        };
        link("new.toml", &outside.join("new.toml"));
        link("deep.toml", &outside.join("deep/er/y.toml"));
        link("nowhere.toml", FsPath::new("/nonexistent/q.toml"));

        let mut staging = Staging::new(Area::Config);
        staging.write("new.toml", "new").unwrap();
        store.commit(staging).await.unwrap();
        assert_eq!(std::fs::read_to_string(outside.join("new.toml")).unwrap(), "new");

        for path in ["deep.toml", "nowhere.toml"] {
            let mut staging = Staging::new(Area::Config);
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
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
    }

    /// A rename that fails, as one does on Windows while another program has the File open, is
    /// tried again after a moment. If it works then, the Commit finishes as usual.
    #[tokio::test]
    async fn a_rename_that_fails_for_a_moment_is_tried_again() {
        let fixture = Fs::new();
        let point = FailurePoint::RenameFails { n: 1, times: 1 };
        let Opened { store, mut feed } = fixture.open_with(|options| options.fail_at(point)).await;
        let mut staging = Staging::new(Area::Data);
        staging.write("a.txt", "a").unwrap();
        staging.write("b.txt", "b").unwrap();
        store.commit(staging).await.unwrap();

        assert_eq!(list(&store, Area::Data).await, ["a.txt", "b.txt"]);
        let file = store.read(Area::Data, "b.txt").await.unwrap().unwrap();
        assert_eq!(file.contents(), "b");
        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("a.txt", ChangeKind::Changed), ("b.txt", ChangeKind::Changed)],
        );
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
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
            let file = store.read(Area::Data, path).await.unwrap().unwrap();
            let stat = store.stat(Area::Data, path).await.unwrap().unwrap();
            assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
            modified.push(stat.modified());
        }
        assert!(modified.iter().all(|time| *time == modified[0]), "{modified:?}");
        let prefix_revision = store.stat_prefix(Area::Data, "").await.unwrap();

        // Finishing it still fails, so the next Commit through this Store isn't made.
        let mut staging = Staging::new(Area::Data);
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
        assert_eq!(store.stat_prefix(Area::Data, "").await.unwrap(), prefix_revision);

        // Another Store's next Commit finishes it, and its Preconditions hold against it.
        let kept = other.read(Area::Data, "kept.txt").await.unwrap().unwrap();
        let mut staging = Staging::new(Area::Data);
        staging.write_back(&kept, "newer");
        staging.require_prefix("", prefix_revision.clone()).unwrap();
        other.commit(staging).await.unwrap();
        for store in [&store, &other] {
            assert_eq!(list(store, Area::Data).await, ["a/b", "d", "kept.txt"]);
            let kept = store.read(Area::Data, "kept.txt").await.unwrap().unwrap();
            assert_eq!(kept.contents(), "newer");
        }
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
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

        let mut staging = Staging::new(Area::Data);
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
        assert_eq!(list(&store, Area::Data).await, Vec::<String>::new());
        pause.release();

        assert_eq!(
            changes(&next_batch(&mut feed).await),
            [("a.txt", ChangeKind::Changed), ("b.txt", ChangeKind::Changed)],
        );
        assert_nothing_more(&mut feed).await;
        let written = [("a.txt", "a"), ("b.txt", "b")].map(|(p, c)| (p.to_owned(), c.to_owned()));
        assert_eq!(contents(&store).await, written);
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
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
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
    }

    // Helpers shared by the tests above.

    /// A shape of Commit that the crash tests stop at every point: what the Area holds before,
    /// and the Commit.
    struct Shape {
        /// Makes what the Area holds before, with the Staging it gives and anything else.
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
                let mut staging = Staging::new(Area::Data);
                staging.delete_prefix("p/").unwrap();
                staging
            },
            writes: 0,
        };
        /// [`moves`].
        const MOVES: Shape = Shape { set_up: |_| before_moves(), commit: moves, writes: 3 };
        /// Writes `settings.toml`, a symlink to a File outside the Area, and `kept.txt`.
        #[cfg(unix)]
        const THROUGH_A_SYMLINK: Shape = Shape {
            set_up: |fixture| {
                let dotfiles = fixture.root.path().join("dotfiles");
                std::fs::create_dir_all(&dotfiles).unwrap();
                std::fs::write(dotfiles.join("settings.toml"), "old").unwrap();
                let link = fixture.on_disk(Area::Data, "settings.toml");
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

    /// Stops `shape`'s Commit at every point in turn, each on an Area of its own, and checks that
    /// it is all there or not there at all, through a Store opened afterwards. Before that Store
    /// finishes it, a Store that was open already, as in another process, reads it the same way.
    /// A rename that keeps failing gives `Pending`, and reads show the Commit all there already.
    /// What all there looks like comes from committing the shape without stopping.
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
            let links = symlinks(fixture.root.path());
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
            assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new(), "{point:?}");
            assert_eq!(symlinks(fixture.root.path()), links, "{point:?}");
        }
    }

    /// What reads through a Store show of the Data Area: each Path's contents, and its Revision
    /// from stat, and the Prefix Revision of each Prefix the crash tests' shapes use.
    #[derive(Debug, PartialEq)]
    struct State {
        contents: Vec<(String, String)>,
        revisions: Vec<Revision>,
        prefix_revisions: Vec<PrefixRevision>,
    }

    async fn state(store: &Store) -> State {
        let mut revisions = Vec::new();
        for path in list(store, Area::Data).await {
            let file = store.read(Area::Data, path.as_str()).await.unwrap().unwrap();
            let stat = store.stat(Area::Data, path.as_str()).await.unwrap().unwrap();
            assert_eq!((stat.modified(), stat.revision()), (file.modified(), file.revision()));
            revisions.push(stat.revision());
        }
        let mut prefix_revisions = Vec::new();
        for prefix in ["", "a/", "d/", "new/", "p/", "p/q/"] {
            prefix_revisions.push(store.stat_prefix(Area::Data, prefix).await.unwrap());
        }
        State { contents: contents(store).await, revisions, prefix_revisions }
    }

    /// A Staging for the Data Area that writes `writes` and deletes `deletes`.
    fn staged(writes: &[(&str, &str)], deletes: &[&str]) -> Staging {
        let mut staging = Staging::new(Area::Data);
        for (path, contents) in writes {
            staging.write(*path, *contents).unwrap();
        }
        for path in deletes {
            staging.delete(*path).unwrap();
        }
        staging
    }

    /// Each Path in the Data Area, with its contents.
    async fn contents(store: &Store) -> Vec<(String, String)> {
        let mut contents = Vec::new();
        for path in list(store, Area::Data).await {
            let file = store.read(Area::Data, path.as_str()).await.unwrap().unwrap();
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
        let mut staging = Staging::new(Area::Data);
        staging.delete("a").unwrap();
        staging.write("a/b", "moved a").unwrap();
        staging.delete_prefix("d/").unwrap();
        staging.write("d", "moved e").unwrap();
        staging.write("kept.txt", "new").unwrap();
        staging
    }

    /// Checks that the Commit [`moves`] makes happened, all of it.
    async fn assert_moved(store: &Store) {
        assert_eq!(list(store, Area::Data).await, ["a/b", "d", "kept.txt"]);
        for (path, contents) in [("a/b", "moved a"), ("d", "moved e"), ("kept.txt", "new")] {
            let file = store.read(Area::Data, path).await.unwrap().unwrap();
            assert_eq!(file.contents(), contents, "{path}");
        }
    }

    /// Lists every Path in `area`, as strings.
    async fn list(store: &Store, area: Area) -> Vec<String> {
        let paths = store.list(area, "").await.unwrap();
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
