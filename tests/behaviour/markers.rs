//! Backend markers: a Location records the Backend that holds it, so that a Store on another
//! Backend refuses to open it, and [`Store::detect`] can tell which Backend a Location has. These
//! tests open both Backends at the same Location, so they need both.

use std::path::Path as FsPath;
use std::time::Duration;

use tempfile::TempDir;
use tidings::{BackendKind, ChangeFeed, Error, FsOptions, SqliteOptions, Staging, Store};

use crate::api::call_blocking;

const BOTH: [BackendKind; 2] = [BackendKind::Fs, BackendKind::Sqlite];

/// Opens a Store on `kind` at the Location `location`.
async fn open(kind: BackendKind, location: &FsPath) -> tidings::Result<(Store, ChangeFeed)> {
    match kind {
        BackendKind::Fs => {
            let options = FsOptions::default().debounce_window(Duration::from_millis(20));
            Store::open_fs(location, options).await
        }
        BackendKind::Sqlite => {
            let options = SqliteOptions::default().poll_interval(Duration::from_millis(10));
            Store::open_sqlite(location, options).await
        }
    }
}

/// The other Backend.
fn other(kind: BackendKind) -> BackendKind {
    match kind {
        BackendKind::Fs => BackendKind::Sqlite,
        BackendKind::Sqlite => BackendKind::Fs,
    }
}

async fn detect(location: &FsPath) -> tidings::Result<Option<BackendKind>> {
    Store::detect(location).await
}

/// Every name under `directory`, relative to it, in order, directories ending in `/`. SQLite's
/// `-wal` and `-shm` files are left out: they go when a dropped Store's last connection closes,
/// which can be after the Store has gone.
fn names_under(directory: &FsPath) -> Vec<String> {
    let mut names = Vec::new();
    let mut to_visit = vec![directory.to_path_buf()];
    while let Some(visiting) = to_visit.pop() {
        for entry in std::fs::read_dir(&visiting).unwrap() {
            let path = entry.unwrap().path();
            let name = path.strip_prefix(directory).unwrap().to_string_lossy().replace('\\', "/");
            if name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            if path.is_dir() {
                names.push(format!("{name}/"));
                to_visit.push(path);
            } else {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

fn assert_wrong_backend<T>(result: tidings::Result<T>, found: BackendKind) {
    match result {
        Err(Error::WrongBackend { found: got }) => assert_eq!(got, found),
        Err(other) => panic!("expected WrongBackend {{ {found:?} }}, got {other:?}"),
        Ok(_) => panic!("expected WrongBackend {{ {found:?} }}, but it succeeded"),
    }
}

#[tokio::test]
async fn detect_finds_nothing_where_no_store_was_opened_and_creates_nothing() {
    let location = tempfile::tempdir().unwrap();
    let missing = location.path().join("missing");
    assert_eq!(detect(&missing).await.unwrap(), None);
    assert!(!missing.exists());

    std::fs::create_dir_all(location.path().join("notes")).unwrap();
    std::fs::write(location.path().join("notes/a.txt"), "a").unwrap();
    assert_eq!(detect(location.path()).await.unwrap(), None);
    assert_eq!(names_under(location.path()), ["notes/", "notes/a.txt"]);
}

#[tokio::test]
async fn opening_marks_the_location_with_its_backend_which_detect_finds() {
    for kind in BOTH {
        let location = tempfile::tempdir().unwrap();
        let (_store, _feed) = open(kind, location.path()).await.unwrap();
        assert_eq!(detect(location.path()).await.unwrap(), Some(kind));
        // And through the blocking API.
        let found = call_blocking(|| tidings::blocking::Store::detect(location.path()));
        assert_eq!(found.unwrap(), Some(kind));
    }
}

#[tokio::test]
async fn a_store_on_another_backend_is_refused_and_changes_nothing() {
    for kind in BOTH {
        let location = tempfile::tempdir().unwrap();
        let (store, _feed) = open(kind, location.path()).await.unwrap();
        let mut staging = Staging::new();
        staging.write("notes/a.txt", "a").unwrap();
        store.commit(staging).await.unwrap();
        drop(store);
        let before = names_under(location.path());

        assert_wrong_backend(open(other(kind), location.path()).await, kind);
        assert_eq!(names_under(location.path()), before);

        // The Store's own Backend still opens it, with its Files.
        let (store, _feed) = open(kind, location.path()).await.unwrap();
        assert_eq!(store.read("notes/a.txt").await.unwrap().unwrap().contents(), "a");
    }
}

#[tokio::test]
async fn a_marker_naming_no_backend_is_an_error_and_is_left_as_it_is() {
    for kind in BOTH {
        let location = tempfile::tempdir().unwrap();
        drop(open(kind, location.path()).await.unwrap());
        let marker = location.path().join(".tidings/backend");
        std::fs::write(&marker, "floppy\n").unwrap();

        for opening in BOTH {
            match open(opening, location.path()).await {
                Err(Error::Backend(error)) => {
                    assert!(error.to_string().contains("floppy"), "{error}");
                }
                other => panic!("expected a Backend error, got {:?}", other.map(|_| ())),
            }
        }
        assert!(matches!(detect(location.path()).await, Err(Error::Backend(_))));
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "floppy\n");
    }
}

/// A marker that stays empty, as when a crash came between making it and writing it, is an error
/// too, once it has been read again for a moment, and is left for a person to remove.
#[tokio::test]
async fn a_marker_that_stays_empty_is_an_error_and_is_left_as_it_is() {
    for kind in BOTH {
        let location = tempfile::tempdir().unwrap();
        drop(open(kind, location.path()).await.unwrap());
        let marker = location.path().join(".tidings/backend");
        std::fs::write(&marker, "").unwrap();

        match open(kind, location.path()).await {
            Err(Error::Backend(error)) => assert!(error.to_string().contains("empty"), "{error}"),
            other => panic!("expected a Backend error, got {:?}", other.map(|_| ())),
        }
        assert!(matches!(detect(location.path()).await, Err(Error::Backend(_))));
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "");
    }
}

#[tokio::test]
async fn a_tidings_directory_without_a_marker_is_unmarked() {
    for kind in BOTH {
        let location = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(location.path().join(".tidings")).unwrap();
        assert_eq!(detect(location.path()).await.unwrap(), None);
        drop(open(kind, location.path()).await.unwrap());
        assert_eq!(detect(location.path()).await.unwrap(), Some(kind));
    }
}

#[tokio::test]
async fn the_filesystem_adopts_a_location_that_already_holds_files() {
    let location = tempfile::tempdir().unwrap();
    std::fs::write(location.path().join("settings.toml"), "a = 1\n").unwrap();

    let (store, _feed) = open(BackendKind::Fs, location.path()).await.unwrap();
    let file = store.read("settings.toml").await.unwrap().unwrap();
    assert_eq!(file.contents(), "a = 1\n");
    assert_eq!(detect(location.path()).await.unwrap(), Some(BackendKind::Fs));
}

#[tokio::test]
async fn sqlite_adopts_a_location_that_already_holds_files_without_showing_them() {
    let location = tempfile::tempdir().unwrap();
    std::fs::write(location.path().join("settings.toml"), "a = 1\n").unwrap();

    let (store, _feed) = open(BackendKind::Sqlite, location.path()).await.unwrap();
    assert_eq!(store.list("").await.unwrap(), []);
    assert_eq!(detect(location.path()).await.unwrap(), Some(BackendKind::Sqlite));
    let settings = std::fs::read_to_string(location.path().join("settings.toml"));
    assert_eq!(settings.unwrap(), "a = 1\n");
}

/// Everything a Backend keeps in a Location is in `.tidings/`, so SQLite's database, which is
/// `.tidings/store.sqlite3`, doesn't show as a File to anyone looking at the Location.
#[tokio::test]
async fn sqlite_keeps_everything_in_the_tidings_directory() {
    let location = tempfile::tempdir().unwrap();
    let (store, _feed) = open(BackendKind::Sqlite, location.path()).await.unwrap();
    let mut staging = Staging::new();
    staging.write("a.txt", "a").unwrap();
    store.commit(staging).await.unwrap();

    assert_eq!(
        names_under(location.path()),
        [".tidings/", ".tidings/backend", ".tidings/store.sqlite3"]
    );
}

/// Stores on both Backends opening a fresh Location at once: one of them gets it, and the other
/// is refused, leaving the Location marked for the one that got it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_both_backends_open_a_fresh_location_at_once_one_gets_it() {
    for _ in 0..20 {
        let location: TempDir = tempfile::tempdir().unwrap();
        let (fs, sqlite) = tokio::join!(
            open(BackendKind::Fs, location.path()),
            open(BackendKind::Sqlite, location.path())
        );
        let winner = match (fs, sqlite) {
            (Ok(_), Err(error)) => {
                assert!(matches!(error, Error::WrongBackend { found: BackendKind::Fs }));
                BackendKind::Fs
            }
            (Err(error), Ok(_)) => {
                assert!(matches!(error, Error::WrongBackend { found: BackendKind::Sqlite }));
                BackendKind::Sqlite
            }
            (fs, sqlite) => {
                panic!("expected one to open, got {:?} and {:?}", fs.err(), sqlite.err())
            }
        };
        assert_eq!(detect(location.path()).await.unwrap(), Some(winner));
    }
}
