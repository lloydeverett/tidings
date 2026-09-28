//! Backend markers: each Area records the Backend that holds it, so that a Store on another
//! Backend refuses to open it, and [`Store::detect`] can tell which Backend a location has. These
//! tests open both Backends on the same Root override, so they need both.

use std::path::Path as FsPath;
use std::time::Duration;

use tempfile::TempDir;
use tidings::{Area, BackendKind, ChangeFeed, Error, FsOptions, SqliteOptions, Staging, Store};

use crate::api::call_blocking;
use crate::common::app;

const BOTH: [BackendKind; 2] = [BackendKind::Fs, BackendKind::Sqlite];

/// Opens a Store on `kind` under the Root override `root`.
async fn open(kind: BackendKind, root: &FsPath) -> tidings::Result<(Store, ChangeFeed)> {
    match kind {
        BackendKind::Fs => {
            let options =
                FsOptions::default().root_override(root).debounce_window(Duration::from_millis(20));
            Store::open_fs(&app(), options).await
        }
        BackendKind::Sqlite => {
            let options = SqliteOptions::default()
                .root_override(root)
                .poll_interval(Duration::from_millis(10));
            Store::open_sqlite(&app(), options).await
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

async fn detect(root: &FsPath) -> tidings::Result<Option<BackendKind>> {
    Store::detect(&app(), Some(root)).await
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

fn assert_wrong_backend<T>(result: tidings::Result<T>, area: Area, found: BackendKind) {
    match result {
        Err(Error::WrongBackend { area: got_area, found: got_found }) => {
            assert_eq!((got_area, got_found), (area, found));
        }
        Err(other) => panic!("expected WrongBackend {{ {area:?}, {found:?} }}, got {other:?}"),
        Ok(_) => panic!("expected WrongBackend {{ {area:?}, {found:?} }}, but it succeeded"),
    }
}

#[tokio::test]
async fn detect_finds_nothing_where_no_store_was_opened_and_creates_nothing() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    assert_eq!(detect(&missing).await.unwrap(), None);
    assert!(!missing.exists());

    std::fs::create_dir_all(root.path().join("data/notes")).unwrap();
    std::fs::write(root.path().join("data/notes/a.txt"), "a").unwrap();
    assert_eq!(detect(root.path()).await.unwrap(), None);
    assert_eq!(names_under(root.path()), ["data/", "data/notes/", "data/notes/a.txt"]);
}

#[tokio::test]
async fn opening_marks_every_area_with_its_backend_which_detect_finds() {
    for kind in BOTH {
        let root = tempfile::tempdir().unwrap();
        let (_store, _feed) = open(kind, root.path()).await.unwrap();
        assert_eq!(detect(root.path()).await.unwrap(), Some(kind));
        // And through the blocking API.
        let found = call_blocking(|| tidings::blocking::Store::detect(&app(), Some(root.path())));
        assert_eq!(found.unwrap(), Some(kind));
    }
}

#[tokio::test]
async fn a_store_on_another_backend_is_refused_and_changes_nothing() {
    for kind in BOTH {
        let root = tempfile::tempdir().unwrap();
        let (store, _feed) = open(kind, root.path()).await.unwrap();
        let mut staging = Staging::new(Area::Data);
        staging.write("notes/a.txt", "a").unwrap();
        store.commit(staging).await.unwrap();
        drop(store);
        let before = names_under(root.path());

        assert_wrong_backend(open(other(kind), root.path()).await, Area::Config, kind);
        assert_eq!(names_under(root.path()), before);

        // The Store's own Backend still opens it, with its Files.
        let (store, _feed) = open(kind, root.path()).await.unwrap();
        assert_eq!(store.read(Area::Data, "notes/a.txt").await.unwrap().unwrap().contents(), "a");
    }
}

#[tokio::test]
async fn every_area_is_checked_before_any_is_marked() {
    for kind in BOTH {
        // Only the Cache is marked, as if the others had been removed.
        let root = tempfile::tempdir().unwrap();
        drop(open(kind, root.path()).await.unwrap());
        std::fs::remove_dir_all(root.path().join("config")).unwrap();
        std::fs::remove_dir_all(root.path().join("data")).unwrap();
        let before = names_under(root.path());

        assert_wrong_backend(open(other(kind), root.path()).await, Area::Cache, kind);
        assert_eq!(names_under(root.path()), before);
        assert_eq!(detect(root.path()).await.unwrap(), Some(kind));
    }
}

#[tokio::test]
async fn areas_marked_by_different_backends_are_refused_by_both_and_named_by_detect() {
    let fs = tempfile::tempdir().unwrap();
    let sqlite = tempfile::tempdir().unwrap();
    drop(open(BackendKind::Fs, fs.path()).await.unwrap());
    drop(open(BackendKind::Sqlite, sqlite.path()).await.unwrap());
    // The data Area of the filesystem Store, moved in with the SQLite Store's.
    std::fs::remove_dir_all(sqlite.path().join("data")).unwrap();
    std::fs::rename(fs.path().join("data"), sqlite.path().join("data")).unwrap();

    match detect(sqlite.path()).await {
        Err(Error::MixedBackends { marked }) => assert_eq!(
            marked,
            [
                (Area::Config, BackendKind::Sqlite),
                (Area::Data, BackendKind::Fs),
                (Area::Cache, BackendKind::Sqlite),
            ],
        ),
        other => panic!("expected MixedBackends, got {other:?}"),
    }
    assert_wrong_backend(
        open(BackendKind::Sqlite, sqlite.path()).await,
        Area::Data,
        BackendKind::Fs,
    );
    assert_wrong_backend(
        open(BackendKind::Fs, sqlite.path()).await,
        Area::Config,
        BackendKind::Sqlite,
    );
}

#[tokio::test]
async fn a_marker_naming_no_backend_is_an_error_and_is_left_as_it_is() {
    for kind in BOTH {
        let root = tempfile::tempdir().unwrap();
        drop(open(kind, root.path()).await.unwrap());
        let marker = root.path().join("data/.tidings/backend");
        std::fs::write(&marker, "floppy\n").unwrap();

        for opening in BOTH {
            match open(opening, root.path()).await {
                Err(Error::Backend(error)) => {
                    assert!(error.to_string().contains("floppy"), "{error}");
                }
                other => panic!("expected a Backend error, got {:?}", other.map(|_| ())),
            }
        }
        assert!(matches!(detect(root.path()).await, Err(Error::Backend(_))));
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "floppy\n");
    }
}

#[tokio::test]
async fn a_tidings_directory_without_a_marker_is_unmarked() {
    for kind in BOTH {
        let root = tempfile::tempdir().unwrap();
        for area in ["config", "data", "cache"] {
            std::fs::create_dir_all(root.path().join(area).join(".tidings")).unwrap();
        }
        assert_eq!(detect(root.path()).await.unwrap(), None);
        drop(open(kind, root.path()).await.unwrap());
        assert_eq!(detect(root.path()).await.unwrap(), Some(kind));
    }
}

#[tokio::test]
async fn the_filesystem_adopts_areas_that_already_hold_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("config")).unwrap();
    std::fs::write(root.path().join("config/settings.toml"), "a = 1\n").unwrap();

    let (store, _feed) = open(BackendKind::Fs, root.path()).await.unwrap();
    let file = store.read(Area::Config, "settings.toml").await.unwrap().unwrap();
    assert_eq!(file.contents(), "a = 1\n");
    assert_eq!(detect(root.path()).await.unwrap(), Some(BackendKind::Fs));
}

#[tokio::test]
async fn sqlite_adopts_areas_that_already_hold_files_without_showing_them() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("config")).unwrap();
    std::fs::write(root.path().join("config/settings.toml"), "a = 1\n").unwrap();

    let (store, _feed) = open(BackendKind::Sqlite, root.path()).await.unwrap();
    assert_eq!(store.list(Area::Config, "").await.unwrap(), []);
    assert_eq!(detect(root.path()).await.unwrap(), Some(BackendKind::Sqlite));
    assert_eq!(
        std::fs::read_to_string(root.path().join("config/settings.toml")).unwrap(),
        "a = 1\n"
    );
}

/// Everything a Backend keeps in an Area's directory is in `.tidings/`, so SQLite's databases
/// don't show as Files to anyone looking at the directory.
#[tokio::test]
async fn sqlite_keeps_everything_in_the_tidings_directory() {
    let root = tempfile::tempdir().unwrap();
    let (store, _feed) = open(BackendKind::Sqlite, root.path()).await.unwrap();
    for area in [Area::Config, Area::Data, Area::Cache] {
        let mut staging = Staging::new(area);
        staging.write("a.txt", "a").unwrap();
        store.commit(staging).await.unwrap();
    }
    for area in ["config", "data", "cache"] {
        let names: Vec<_> = std::fs::read_dir(root.path().join(area))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [".tidings"], "in {area}");
    }
}

/// Stores on both Backends opening a fresh location at once: one of them gets it, and the other
/// is refused, leaving every Area marked for the one that got it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_both_backends_open_a_fresh_location_at_once_one_gets_it() {
    for _ in 0..20 {
        let root: TempDir = tempfile::tempdir().unwrap();
        let (fs, sqlite) = tokio::join!(
            open(BackendKind::Fs, root.path()),
            open(BackendKind::Sqlite, root.path())
        );
        let winner = match (fs, sqlite) {
            (Ok(_), Err(error)) => {
                assert!(matches!(error, Error::WrongBackend { found: BackendKind::Fs, .. }));
                BackendKind::Fs
            }
            (Err(error), Ok(_)) => {
                assert!(matches!(error, Error::WrongBackend { found: BackendKind::Sqlite, .. }));
                BackendKind::Sqlite
            }
            (fs, sqlite) => {
                panic!("expected one to open, got {:?} and {:?}", fs.err(), sqlite.err())
            }
        };
        assert_eq!(detect(root.path()).await.unwrap(), Some(winner));
    }
}
