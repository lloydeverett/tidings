//! The behaviour suite: one set of tests, written once against the public API, that every Backend
//! must pass. Each Backend instantiates the whole suite in its own module below.

#[path = "../common/mod.rs"]
mod common;
#[macro_use]
mod suite;
// Only SQLite can have a second Store on the same storage so far.
#[cfg(feature = "sqlite")]
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
