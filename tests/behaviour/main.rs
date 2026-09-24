//! The behaviour suite: one set of tests, written once against the public API, that every Backend
//! must pass. Each Backend instantiates the whole suite in its own module below.

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
            let (store, feed) = tidings::Store::open_memory();
            Opened { store, feed }
        }
    }

    behaviour_suite!(Memory);

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
    use tidings::{AppIdentity, Area, ChangeKind, FeedItem, Origin, SqliteOptions, Staging, Store};

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

        /// Opens a Store on the Root override with the options `options` makes of the usual
        /// ones.
        async fn open_with(&self, options: impl FnOnce(SqliteOptions) -> SqliteOptions) -> Opened {
            let app = AppIdentity::new("tidings tests", "tidings", "org");
            let usual = SqliteOptions::default()
                .root_override(self.root.path())
                .poll_interval(Duration::from_millis(10));
            let (store, feed) = Store::open_sqlite(&app, options(usual)).await.unwrap();
            Opened { store, feed }
        }
    }

    impl Fixture for Sqlite {
        async fn open(&self) -> Opened {
            self.open_with(|options| options).await
        }
    }

    behaviour_suite!(Sqlite::new());
    two_stores_suite!(Sqlite::new());

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
    use std::time::SystemTime;

    use tempfile::TempDir;
    use tidings::{
        AppIdentity, Area, Error, FailurePoint, FsOptions, InvalidPathReason, Staging, Store,
    };

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

        /// Opens a Store on the Root override with the options `options` makes of the usual
        /// ones.
        async fn open_with(&self, options: impl FnOnce(FsOptions) -> FsOptions) -> Opened {
            let app = AppIdentity::new("tidings tests", "tidings", "org");
            let usual = FsOptions::default().root_override(self.root.path());
            let (store, feed) = Store::open_fs(&app, options(usual)).await.unwrap();
            Opened { store, feed }
        }

        /// Where `path` in `area` is on disk.
        fn on_disk(&self, area: &str, path: &str) -> PathBuf {
            self.root.path().join(area).join(path)
        }

        /// Writes `contents` to `path` in `area` directly, as another program would.
        fn write_directly(&self, area: &str, path: &str, contents: impl AsRef<[u8]>) {
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

    behaviour_suite!(Fs::new());
    two_stores_suite!(committing: Fs::new());

    /// The suite's Snapshot tests are skipped where Snapshots aren't supported, so this makes sure
    /// the filesystem is one of those, and that the suite's refusal test runs there.
    #[tokio::test]
    async fn fs_does_not_support_snapshots() {
        let Opened { store, feed: _feed } = Fs::new().open().await;
        assert!(!store.supports_snapshots());
    }

    #[tokio::test]
    async fn each_area_is_a_directory_people_can_see_the_files_in() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        for area in ["config", "data", "cache"] {
            assert!(fixture.on_disk(area, "").is_dir(), "{area}");
            assert!(fixture.on_disk(area, ".tidings").is_dir(), "{area}");
        }

        let mut staging = Staging::new(Area::Config);
        staging.write("themes/dark.toml", "dark = true\n").unwrap();
        store.commit(staging).await.unwrap();
        let on_disk = std::fs::read_to_string(fixture.on_disk("config", "themes/dark.toml"));
        assert_eq!(on_disk.unwrap(), "dark = true\n");
        assert!(!fixture.on_disk("data", "themes").exists());
    }

    #[tokio::test]
    async fn files_other_programs_make_are_read_and_listed() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        fixture.write_directly("config", "settings.toml", "a = 1\n");
        fixture.write_directly("config", "themes/dark.toml", "dark = true\n");

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
        fixture.write_directly("data", "kept.txt", "kept");
        let before = store.stat_prefix(Area::Data, "").await.unwrap();

        // Not in NFC form.
        fixture.write_directly("data", "cafe\u{301}.txt", "x");
        fixture.write_directly("data", "re\u{301}sume\u{301}/cv.txt", "x");
        // Named like tidings' own, or like its temporary files.
        fixture.write_directly("data", ".tidings/other.txt", "x");
        fixture.write_directly("data", ".kept.txt.tidings-0123456789abcdef0123456789abcdef-0", "x");
        // Names Windows can't hold, which Windows can't make either.
        #[cfg(not(windows))]
        {
            fixture.write_directly("data", "CON", "x");
            fixture.write_directly("data", "what?/a.txt", "x");
        }
        // Not valid UTF-8.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::OsStr::from_bytes(b"not \xff utf-8.txt");
            std::fs::write(fixture.on_disk("data", "").join(name), "x").unwrap();
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
        fixture.write_directly("cache", "image.png", [0x89, b'P', b'N', b'G', 0xff, 0xfe]);

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
            let modified = std::fs::metadata(fixture.on_disk("data", path)).unwrap().modified();
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
        let link = fixture.on_disk("config", "settings.toml");
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

    /// Something on disk that isn't a File can have the name of a File a Commit writes, or of a
    /// directory the Commit must make for one. An empty directory makes way for the File. Anything
    /// else refuses the Commit before it happens, as a File would.
    #[tokio::test]
    async fn what_is_on_disk_but_not_a_file_makes_way_or_refuses_the_commit() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        std::fs::create_dir_all(fixture.on_disk("data", "empty/inside")).unwrap();
        let mut staging = Staging::new(Area::Data);
        staging.write("empty", "a File now").unwrap();
        store.commit(staging).await.unwrap();
        let file = store.read(Area::Data, "empty").await.unwrap().unwrap();
        assert_eq!(file.contents(), "a File now");

        // A directory with a name in it that no Path has.
        fixture.write_directly("data", "held/cafe\u{301}.txt", "x");
        let mut refused = vec![("held", "held")];
        // A symlink to nothing, where a directory would have to go.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("nowhere", fixture.on_disk("data", "dangling")).unwrap();
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

    /// A crash before the journal is committed leaves a Commit that never happened, which the
    /// next Store to open the Area discards, temporary files and all.
    #[tokio::test]
    async fn a_commit_interrupted_before_it_was_committed_never_happens() {
        for point in [FailurePoint::AfterPreparedJournal, FailurePoint::AfterTemporaryFile(1)] {
            let fixture = Fs::new();
            let Opened { store, feed: _feed } = fixture.open().await;
            let mut staging = Staging::new(Area::Data);
            staging.write("kept.txt", "old").unwrap();
            staging.write("gone.txt", "gone").unwrap();
            store.commit(staging).await.unwrap();
            drop(store);

            let Opened { store, feed: _feed } =
                fixture.open_with(|options| options.fail_at(point)).await;
            let mut staging = Staging::new(Area::Data);
            staging.write("kept.txt", "new").unwrap();
            staging.write("new/file.txt", "new").unwrap();
            staging.delete("gone.txt").unwrap();
            let stopped = store.commit(staging).await;
            assert!(matches!(stopped, Err(Error::Backend(_))), "{point:?}: {stopped:?}");
            drop(store);

            let Opened { store, feed: _feed } = fixture.open().await;
            let listed = list(&store, Area::Data).await;
            assert_eq!(listed, ["gone.txt", "kept.txt"], "{point:?}");
            let kept = store.read(Area::Data, "kept.txt").await.unwrap().unwrap();
            assert_eq!(kept.contents(), "old", "{point:?}");
            assert!(!fixture.on_disk("data", "new").exists(), "{point:?}");
            assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new(), "{point:?}");
        }
    }

    /// A crash once the journal is committed leaves a Commit that has happened, which the next
    /// Store to open the Area finishes. That includes moving Files where directories were, and
    /// directories where Files were.
    #[tokio::test]
    async fn a_commit_interrupted_once_it_was_committed_is_finished_when_a_store_opens() {
        let fixture = Fs::new();
        let Opened { store, feed: _feed } = fixture.open().await;
        let mut staging = Staging::new(Area::Data);
        staging.write("a", "a").unwrap();
        staging.write("d/e", "e").unwrap();
        staging.write("kept.txt", "old").unwrap();
        store.commit(staging).await.unwrap();
        drop(store);

        let Opened { store, feed: _feed } =
            fixture.open_with(|options| options.fail_at(FailurePoint::AfterCommittedJournal)).await;
        let stopped = store.commit(moves()).await;
        assert!(matches!(stopped, Err(Error::Backend(_))), "{stopped:?}");
        drop(store);

        let Opened { store, feed: _feed } = fixture.open().await;
        assert_moved(&store).await;
        assert_eq!(temporary_files(fixture.root.path()), Vec::<PathBuf>::new());
    }

    /// A Store that was already open when another one's Commit was interrupted, as another
    /// process's might be, finishes it before it commits.
    #[tokio::test]
    async fn the_next_commit_finishes_a_commit_that_was_interrupted_once_committed() {
        let fixture = Fs::new();
        let Opened { store: other, feed: _other_feed } = fixture.open().await;
        let mut staging = Staging::new(Area::Data);
        staging.write("a", "a").unwrap();
        staging.write("d/e", "e").unwrap();
        staging.write("kept.txt", "old").unwrap();
        other.commit(staging).await.unwrap();

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

    // Helpers shared by the tests above.

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
