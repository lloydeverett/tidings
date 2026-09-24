//! The behaviour suite: one set of tests, written once against the public API, that every Backend
//! must pass. Each Backend instantiates the whole suite in its own module below.

#[path = "../common/mod.rs"]
mod common;
#[macro_use]
mod suite;

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
    use tempfile::TempDir;
    use tidings::{AppIdentity, SqliteOptions, Store};

    use crate::suite::{Fixture, Opened};

    /// Opens each test's Store under a temporary Root override of its own, removed when the test
    /// ends.
    struct Sqlite {
        root: TempDir,
    }

    impl Sqlite {
        fn new() -> Sqlite {
            Sqlite { root: tempfile::tempdir().unwrap() }
        }
    }

    impl Fixture for Sqlite {
        async fn open(&self) -> Opened {
            let app = AppIdentity::new("tidings tests", "tidings", "org");
            let options = SqliteOptions::default().root_override(self.root.path());
            let (store, feed) = Store::open_sqlite(&app, options).await.unwrap();
            Opened { store, feed }
        }
    }

    behaviour_suite!(Sqlite::new());

    /// The suite's Snapshot tests run only where Snapshots are supported, so this makes sure they
    /// run on SQLite.
    #[tokio::test]
    async fn sqlite_supports_snapshots() {
        let Opened { store, feed: _feed } = Sqlite::new().open().await;
        assert!(store.supports_snapshots());
    }
}
