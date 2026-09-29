//! Working copies: `tidings sync` makes a folder hold an Area's Files, the person edits them with
//! ordinary tools (`std::fs` here), and `tidings commit` commits the edits back.

mod common;

use std::fs;
use std::path::Path;

use common::{Location, Sync, tidings};
use tempfile::TempDir;

const BACKENDS: [&str; 2] = ["fs", "sqlite"];

/// A Store on `backend` holding two config Files, one under a Prefix.
fn store_with_config(backend: &str) -> Location {
    let location = Location::with_store(backend);
    location.write("config", "app.toml", "a = 1\n");
    location.write("config", "themes/dark.toml", "bg = \"black\"\n");
    location
}

/// Syncs `area` into `folder`, waits until it's caught up, and stops it.
fn synced(location: &Location, area: &str, folder: &Path) {
    let mut sync = Sync::start(location, area, folder);
    sync.wait_for("caught up");
    sync.stop();
}

#[test]
fn sync_writes_every_file_into_a_missing_folder() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let parent = TempDir::new().unwrap();
        let folder = parent.path().join("cfg");
        let mut sync = Sync::start(&location, "config", &folder);
        let events = sync.wait_for("caught up");
        sync.stop();

        let created: Vec<&str> = events
            .iter()
            .filter(|event| event["event"] == "created")
            .map(|event| event["path"].as_str().unwrap())
            .collect();
        assert_eq!(created, ["app.toml", "themes/dark.toml"], "{backend}: {events:?}");
        assert_eq!(fs::read_to_string(folder.join("app.toml")).unwrap(), "a = 1\n");
        let dark = fs::read_to_string(folder.join("themes/dark.toml")).unwrap();
        assert_eq!(dark, "bg = \"black\"\n");
        assert!(folder.join(".tidings").is_dir());
    }
}

#[test]
fn sync_prints_a_line_per_file_for_a_person_and_defaults_to_the_current_directory() {
    let location = store_with_config("fs");
    let folder = TempDir::new().unwrap();
    let mut command = location.command(&["sync", "config"]);
    command.current_dir(folder.path());
    let mut sync = Sync::spawn(command);
    assert_eq!(sync.next_line(), "created app.toml");
    assert_eq!(sync.next_line(), "created themes/dark.toml");
    assert_eq!(sync.next_line(), "caught up");
    sync.stop();
    assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 1\n");
}

#[test]
fn edits_additions_and_deletions_are_committed_together() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::create_dir(folder.path().join("keys")).unwrap();
        fs::write(folder.path().join("keys/vim.toml"), "mode = \"normal\"\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        // No Store flags: the Working copy knows its Store.
        let run = commit_in(folder.path(), &[]).expect_success();
        let lines: Vec<&str> = run.stdout.lines().collect();
        assert_eq!(
            lines,
            ["modified app.toml", "added keys/vim.toml", "deleted themes/dark.toml"],
            "{backend}"
        );

        assert_eq!(location.read("config", "app.toml"), "a = 2\n");
        assert_eq!(location.read("config", "keys/vim.toml"), "mode = \"normal\"\n");
        location.run(&["store", "read", "config", "themes/dark.toml"]).expect_code(2);

        let run = commit_in(folder.path(), &[]).expect_success();
        assert!(run.stdout.is_empty() && run.stderr.contains("nothing to commit"), "{run:?}");
    }
}

#[test]
fn committed_revisions_become_the_bases() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::write(folder.path().join("new.toml"), "new\n").unwrap();
        let run = commit_in(folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let committed = json["committed"].as_array().unwrap();
        assert_eq!(committed.len(), 2, "{json}");
        for change in committed {
            let path = change["path"].as_str().unwrap();
            let stat = location.run(&["--json", "store", "stat", "config", path]).expect_success();
            let stat: serde_json::Value = serde_json::from_str(&stat.stdout).unwrap();
            assert_eq!(change["revision"], stat["revision"], "{json}");
        }

        // Committing again from the new Bases is no Conflict.
        fs::write(folder.path().join("app.toml"), "a = 3\n").unwrap();
        fs::remove_file(folder.path().join("new.toml")).unwrap();
        let run = commit_in(folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(json["committed"][1]["change"], "deleted", "{json}");
        assert!(json["committed"][1]["revision"].is_null(), "{json}");
        assert_eq!(location.read("config", "app.toml"), "a = 3\n");
        location.run(&["store", "read", "config", "new.toml"]).expect_code(2);
    }
}

#[test]
fn a_file_changed_in_the_store_since_its_base_is_a_conflict() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());

        location.write("config", "app.toml", "theirs\n");
        location.write("config", "created.toml", "theirs\n");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("created.toml"), "mine\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        let run = commit_in(folder.path(), &[]).expect_code(3);
        assert!(run.stderr.contains("app.toml"), "{run:?}");

        // Nothing was committed.
        assert_eq!(location.read("config", "app.toml"), "theirs\n");
        assert_eq!(location.read("config", "created.toml"), "theirs\n");
        assert_eq!(location.read("config", "themes/dark.toml"), "bg = \"black\"\n");
    }
}

#[test]
fn commit_works_while_sync_runs() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught up");
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        commit_in(folder.path(), &[]).expect_success();
        sync.stop();
        assert_eq!(location.read("config", "app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_finds_the_working_copy_from_a_subdirectory() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        commit_in(&folder.path().join("themes"), &[]).expect_success();
        assert_eq!(location.read("config", "app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_takes_the_working_copy_from_dash_c() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        let elsewhere = TempDir::new().unwrap();
        let folder_arg = folder.path().to_str().unwrap();
        commit_in(elsewhere.path(), &["-C", folder_arg]).expect_success();
        assert_eq!(location.read("config", "app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_outside_a_working_copy_fails() {
    let elsewhere = TempDir::new().unwrap();
    // A `.tidings/` directory alone, as a filesystem Area has, isn't a Working copy.
    fs::create_dir(elsewhere.path().join(".tidings")).unwrap();
    let run = commit_in(elsewhere.path(), &[]).expect_code(1);
    assert!(run.stderr.contains("sync") && run.stderr.contains("-C"), "{run:?}");

    let folder = elsewhere.path().to_str().unwrap();
    commit_in(elsewhere.path(), &["-C", folder]).expect_code(1);
}

/// `tidings commit <args>`, with no Store flags, run in `directory`.
fn commit_in(directory: &Path, args: &[&str]) -> common::Run {
    let mut command = tidings();
    command.arg("commit").args(args).current_dir(directory);
    common::run(command, "")
}
