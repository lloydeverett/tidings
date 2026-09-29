//! Working copies: `tidings sync` makes a folder hold an Area's Files, the person edits them with
//! ordinary tools (`std::fs` here), and `tidings commit` commits the edits back.

mod common;

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use common::{Location, Sync, tidings, wait_until};
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
    sync.wait_for("caught-up");
    sync.stop();
}

#[test]
fn sync_writes_every_file_into_a_missing_folder() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let parent = TempDir::new().unwrap();
        let folder = parent.path().join("cfg");
        let mut sync = Sync::start(&location, "config", &folder);
        let events = sync.wait_for("caught-up");
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
fn sync_finishes_a_working_copy_a_crash_left_without_a_record() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        // What a `sync` that stopped before writing the record leaves: `.tidings/` with its lock
        // and part of a record.
        let tidings = folder.path().join(".tidings");
        fs::create_dir(&tidings).unwrap();
        fs::write(tidings.join("lock"), "").unwrap();
        fs::write(tidings.join(".working-copy.a1B2c3"), "tidings work").unwrap();
        synced(&location, "config", folder.path());

        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 1\n");
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        commit_in(folder.path(), &[]).expect_success();
        assert_eq!(location.read("config", "app.toml"), "a = 2\n");
    }
}

#[test]
fn sync_into_a_folder_that_isnt_empty_makes_no_store() {
    let location = Location::empty();
    let folder = TempDir::new().unwrap();
    fs::write(folder.path().join("notes.txt"), "mine\n").unwrap();
    let folder_arg = folder.path().to_str().unwrap();
    let args = ["--backend", "fs", "--create", "sync", "config", folder_arg];
    let run = location.run(&args).expect_code(1);
    assert!(run.stderr.contains("isn't empty"), "{run:?}");
    assert!(fs::read_dir(location.root()).unwrap().next().is_none(), "a Store was made");
}

#[test]
fn a_second_commit_waiting_for_the_first_reads_the_bases_it_saved() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();

        // Both commits start while another command holds the lock, and wait for it.
        let lock = fs::File::create(folder.path().join(".tidings/lock")).unwrap();
        lock.lock().unwrap();
        let commits: Vec<_> = (0..2)
            .map(|_| {
                let mut command = tidings();
                command.arg("commit").current_dir(folder.path());
                common::spawn(&mut command)
            })
            .collect();
        thread::sleep(Duration::from_millis(500));
        // Both still wait, so that neither can have committed without the lock.
        let mut commits = commits;
        for commit in &mut commits {
            assert!(commit.try_wait().unwrap().is_none(), "{backend}: a commit didn't wait");
        }
        drop(lock);

        let mut runs: Vec<String> = commits
            .into_iter()
            .map(|commit| {
                let output = commit.wait_with_output().unwrap();
                let stdout = String::from_utf8(output.stdout).unwrap();
                let stderr = String::from_utf8(output.stderr).unwrap();
                assert_eq!(output.status.code(), Some(0), "{backend}: {stdout}{stderr}");
                stdout + &stderr
            })
            .collect();
        runs.sort();
        assert!(runs[0].contains("modified app.toml"), "{backend}: {runs:?}");
        assert!(runs[1].contains("nothing to commit"), "{backend}: {runs:?}");
        assert_eq!(location.read("config", "app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_works_while_sync_runs() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
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

#[test]
fn files_the_store_writes_and_deletes_appear_and_disappear() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");

        location.write("config", "new.toml", "new\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");
        assert_eq!(fs::read_to_string(folder.path().join("new.toml")).unwrap(), "new\n");

        location.write("config", "app.toml", "a = 2\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "updated"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 2\n");

        location.run(&["store", "delete", "config", "themes/dark.toml"]).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "removed"), ["themes/dark.toml"], "{backend}: {events:?}");
        // The directory the removal emptied is removed too.
        assert!(!folder.path().join("themes").exists(), "{backend}");
        sync.stop();

        // Every file was renamed into place from `.tidings/tmp/`, leaving nothing there.
        let tmp = fs::read_dir(folder.path().join(".tidings/tmp")).unwrap();
        assert_eq!(tmp.count(), 0, "{backend}");
    }
}

#[test]
fn a_removal_removes_only_the_directories_it_emptied() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("config", "deep/er/x.toml", "x\n");
        location.write("config", "mine/sub/y.toml", "y\n");
        location.write("config", "mine/kept.toml", "kept\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("mine/notes.txt"), "untracked\n").unwrap();

        let script = "stage config\ndelete config deep/er/x.toml\ndelete config mine/sub/y.toml\n\
                      delete config mine/kept.toml\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        let mut removed = paths(&events, "removed");
        removed.sort();
        assert_eq!(removed, ["deep/er/x.toml", "mine/kept.toml", "mine/sub/y.toml"], "{backend}");
        sync.stop();

        assert!(!folder.path().join("deep").exists(), "{backend}");
        assert!(!folder.path().join("mine/sub").exists(), "{backend}");
        // Still holding a file of the person's own, so left alone, as is the folder itself.
        assert!(folder.path().join("mine/notes.txt").is_file(), "{backend}");
        assert!(folder.path().join(".tidings").is_dir(), "{backend}");
    }
}

#[test]
fn a_file_takes_the_place_of_a_directory_and_the_reverse() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("config", "x/y.toml", "y\n");
        location.write("config", "z", "z\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");

        let script = "stage config\ndelete config x/y.toml\nwrite config x --contents x\n\
                      delete config z\nwrite config z/w.toml --contents w\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        let names: Vec<&str> =
            events.iter().map(|event| event["event"].as_str().unwrap()).collect();
        // Removals come first, making way for the writes.
        assert_eq!(names, ["removed", "removed", "created", "created", "caught-up"], "{backend}");
        sync.stop();

        assert_eq!(fs::read_to_string(folder.path().join("x")).unwrap(), "x", "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("z/w.toml")).unwrap(), "w", "{backend}");
    }
}

#[test]
fn only_changes_to_the_working_copys_area_are_acted_on() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");

        location.write("data", "other.toml", "data\n");
        location.write("config", "marker.toml", "marker\n");
        // Nothing, not even a reconcile that finds nothing to do, for the data Area's Change.
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["marker.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        sync.stop();
        assert!(!folder.path().join("other.toml").exists(), "{backend}");
    }
}

#[test]
fn a_file_committed_while_sync_starts_is_not_missed() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        location.write("config", "late.toml", "late\n");
        loop {
            let events = sync.wait_for("caught-up");
            if folder.path().join("late.toml").exists() {
                break;
            }
            assert!(paths(&events, "created").len() < 3, "{backend}: {events:?}");
        }
        sync.stop();
        assert_eq!(fs::read_to_string(folder.path().join("late.toml")).unwrap(), "late\n");
    }
}

#[test]
fn a_resync_reconciles_every_path() {
    let location = Location::with_store("fs");
    location.write("cache", "thumbnails/a.png", "a");
    location.write("cache", "b.txt", "b");
    let folder = TempDir::new().unwrap();
    let mut sync = Sync::start(&location, "cache", folder.path());
    sync.wait_for("caught-up");

    clear_the_cache_directory(&location);
    sync.wait_for("resync");
    sync.wait_for("caught-up");
    sync.stop();
    assert!(!folder.path().join("thumbnails").exists());
    assert!(!folder.path().join("b.txt").exists());
}

#[test]
fn clearing_the_cache_removes_the_unchanged_local_files() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("cache", "thumbnails/a.png", "a");
        location.write("cache", "b.txt", "b");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "cache", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("b.txt"), "mine").unwrap();

        location.run(&["store", "delete-prefix", "cache", ""]).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "removed"), ["thumbnails/a.png"], "{backend}: {events:?}");
        assert_eq!(paths(&events, "diverged"), ["b.txt"], "{backend}: {events:?}");
        sync.stop();
        assert!(!folder.path().join("thumbnails").exists(), "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("b.txt")).unwrap(), "mine");
    }
}

#[test]
fn a_path_changed_locally_and_in_the_store_is_diverged() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        fs::write(folder.path().join("added.toml"), "mine\n").unwrap();

        let script = "stage config\nwrite config app.toml --contents theirs\n\
                      write config themes/dark.toml --contents theirs\n\
                      write config added.toml --contents theirs\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        let diverged = ["added.toml", "app.toml", "themes/dark.toml"];
        assert_eq!(paths(&events, "diverged"), diverged, "{backend}: {events:?}");
        assert_eq!(events.len(), 4, "{backend}: {events:?}");
        for path in diverged {
            let message = format!("the Store's version is in .tidings/theirs/{path}");
            assert_eq!(message_for(&events, path), message, "{backend}");
            let theirs_file = format!(".tidings/theirs/{path}");
            assert_eq!(event_for(&events, path)["theirs"], theirs_file.as_str(), "{backend}");
            assert_eq!(fs::read_to_string(theirs(folder.path(), path)).unwrap(), "theirs");
        }
        sync.stop();

        // The local files are untouched.
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        assert!(!folder.path().join("themes/dark.toml").exists(), "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("added.toml")).unwrap(), "mine\n");
    }
}

#[test]
fn a_path_changed_locally_and_removed_in_the_store_is_diverged_with_no_theirs() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();

        location.run(&["store", "delete", "config", "app.toml"]).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(message_for(&events, "app.toml"), "removed in the Store", "{backend}");
        let theirs_field = event_for(&events, "app.toml").get("theirs");
        assert_eq!(theirs_field, Some(&serde_json::Value::Null), "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        sync.stop();

        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
    }
}

#[test]
fn the_same_change_on_both_sides_takes_the_stores_version_as_the_base_silently() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "same").unwrap();
        fs::write(folder.path().join("added.toml"), "same").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();

        let script = "stage config\nwrite config app.toml --contents same\n\
                      write config added.toml --contents same\n\
                      delete config themes/dark.toml\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();

        // Every Path took the Store's version as its Base, so nothing is a change.
        let run = commit_in(folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
    }
}

#[test]
fn a_further_store_change_refreshes_theirs() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("config", "app.toml", "theirs 1\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");

        location.write("config", "app.toml", "theirs 2\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        let refreshed = fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap();
        assert_eq!(refreshed, "theirs 2\n", "{backend}");

        // Removed in the Store, so `theirs` goes too.
        location.run(&["store", "delete", "config", "app.toml"]).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(message_for(&events, "app.toml"), "removed in the Store", "{backend}");
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");

        // A change elsewhere reconciles, but reports nothing more for the Diverged Path.
        location.write("config", "other.toml", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        sync.stop();
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
    }
}

#[test]
fn a_divergence_clears_once_the_local_file_equals_the_stores() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("config", "themes/dark.toml", "theirs\n");
        location.write("config", "app.toml", "theirs\n");
        sync.wait_for("diverged");
        sync.wait_for("caught-up");

        // The person merges by taking the Store's version; the next reconcile notices.
        fs::write(folder.path().join("app.toml"), "theirs\n").unwrap();
        location.write("config", "other.toml", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "resolved"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        sync.stop();
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
        // The emptied directories in `theirs/` go too.
        let left = fs::read_dir(folder.path().join(".tidings/theirs")).unwrap().count();
        assert_eq!(left, 0, "{backend}");

        let run = commit_in(folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[test]
fn a_divergence_survives_restarting_sync() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());
        // Diverged while `sync` was down, and found when it resumes.
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("config", "app.toml", "theirs\n");
        let mut sync = Sync::start(&location, "config", folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        sync.stop();

        // Still Diverged, and not reported again.
        let mut sync = Sync::start(&location, "config", folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        assert_eq!(fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap(), "theirs\n");

        // Merged while `sync` was down: the record still knew it was Diverged.
        fs::write(folder.path().join("app.toml"), "theirs\n").unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "resolved"), ["app.toml"], "{backend}: {events:?}");
        sync.stop();
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
    }
}

#[test]
fn a_file_that_isnt_text_where_the_store_changed_is_diverged() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), [0xff, 0xfe]).unwrap();

        location.write("config", "app.toml", "theirs\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        sync.stop();
        assert_eq!(fs::read(folder.path().join("app.toml")).unwrap(), [0xff, 0xfe]);
        assert_eq!(fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap(), "theirs\n");
    }
}

#[test]
fn a_directory_of_the_persons_files_where_a_file_must_go_is_diverged() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::create_dir(folder.path().join("x")).unwrap();
        fs::write(folder.path().join("x/mine"), "mine\n").unwrap();

        location.write("config", "x", "theirs\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["x"], "{backend}: {events:?}");
        let message = "x is a directory; the Store's version is in .tidings/theirs/x";
        assert_eq!(message_for(&events, "x"), message, "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("x/mine")).unwrap(), "mine\n");

        // Once the directory is gone, the next reconcile writes the File.
        fs::remove_dir_all(folder.path().join("x")).unwrap();
        location.write("config", "other", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["other", "x"], "{backend}: {events:?}");
        sync.stop();
        assert_eq!(fs::read_to_string(folder.path().join("x")).unwrap(), "theirs\n");
        assert!(!theirs(folder.path(), "x").exists(), "{backend}");
    }
}

#[test]
fn sync_doesnt_report_the_persons_own_commit_back() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::write(folder.path().join("added.toml"), "added\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        commit_in(folder.path(), &[]).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
    }
}

#[test]
fn sync_quiet_prints_only_what_needs_attention() {
    // A Resync, as in `a_resync_reconciles_every_path`, is the one thing here worth printing.
    for json in [true, false] {
        let location = Location::with_store("fs");
        location.write("cache", "a.txt", "a");
        let folder = TempDir::new().unwrap();
        let mut command = location.command(if json { &["--json"] } else { &[] });
        command.args(["sync", "--quiet", "cache"]).arg(folder.path());
        let mut sync = Sync::spawn(command);
        wait_until(|| folder.path().join("a.txt").exists());
        location.write("cache", "b.txt", "b");
        wait_until(|| folder.path().join("b.txt").exists());
        clear_the_cache_directory(&location);
        let expected = if json { r#"{"event":"resync"}"# } else { "resync" };
        assert_eq!(sync.next_line(), expected);
        wait_until(|| !folder.path().join("b.txt").exists());
        sync.stop();
    }
}

#[test]
fn ctrl_c_during_a_reconcile_lets_it_finish() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let writes: String =
            (0..300).map(|n| format!("write config f{n:03}.toml --contents {n}\n")).collect();
        let script = format!("stage config\n{writes}commit\n");
        location.run_with_stdin(&["store", "shell"], &script).expect_success();
        let folder = TempDir::new().unwrap();
        let sync = Sync::start(&location, "config", folder.path());
        // Stopped as soon as the first file is written, while the rest are still to come.
        let first = folder.path().join("f000.toml");
        while !first.exists() {
            thread::sleep(Duration::from_millis(1));
        }
        let written_by_then = fs::read_dir(folder.path()).unwrap().count();
        sync.stop();
        assert!(written_by_then < 301, "{backend}: sync wrote every file before it was stopped");

        // Every File was written, and the record saved, so nothing is an added file.
        let files = fs::read_dir(folder.path()).unwrap().count();
        assert_eq!(files, 301, "{backend}");
        let run = commit_in(folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[cfg(unix)]
#[test]
fn nothing_outside_the_folder_is_changed_through_a_symlinked_directory() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("config", "a/b", "b\n");
        location.write("config", "a/c", "c\n");
        let folder = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        // The directory moved out of the folder, with a symlink to it left in its place.
        fs::rename(folder.path().join("a"), outside.path().join("a")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("a"), folder.path().join("a")).unwrap();

        let script =
            "stage config\nwrite config a/c --contents theirs\ndelete config a/b\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        let mut diverged = paths(&events, "diverged");
        diverged.sort();
        assert_eq!(diverged, ["a/b", "a/c"], "{backend}: {events:?}");
        let message = "a is a symlink; the Store's version is in .tidings/theirs/a/c";
        assert_eq!(message_for(&events, "a/c"), message, "{backend}");
        assert_eq!(message_for(&events, "a/b"), "a is a symlink; removed in the Store");
        assert_eq!(fs::read_to_string(theirs(folder.path(), "a/c")).unwrap(), "theirs");
        assert_eq!(fs::read_to_string(outside.path().join("a/b")).unwrap(), "b\n", "{backend}");
        assert_eq!(fs::read_to_string(outside.path().join("a/c")).unwrap(), "c\n", "{backend}");
        assert!(folder.path().join("a").is_symlink(), "{backend}");

        // `sync` keeps running.
        location.write("config", "d", "d\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["d"], "{backend}: {events:?}");
        sync.stop();
    }
}

#[test]
fn a_path_blocked_by_a_local_file_is_diverged_and_the_rest_still_applied() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        // A file of the person's own where the Store will want a directory.
        fs::write(folder.path().join("q"), "mine\n").unwrap();

        let script = "stage config\nwrite config a --contents a\nwrite config q/r --contents r\n\
                      write config z --contents z\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["q/r"], "{backend}: {events:?}");
        let message = "q isn't a directory; the Store's version is in .tidings/theirs/q/r";
        assert_eq!(message_for(&events, "q/r"), message, "{backend}");
        assert_eq!(paths(&events, "created"), ["a", "z"], "{backend}: {events:?}");
        assert_eq!(fs::read_to_string(folder.path().join("q")).unwrap(), "mine\n", "{backend}");

        // `sync` keeps running.
        location.write("config", "later", "later\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["later"], "{backend}: {events:?}");
        sync.stop();

        // The applied Paths were recorded, so with the person's file gone nothing is an addition.
        fs::remove_file(folder.path().join("q")).unwrap();
        let run = commit_in(folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[test]
fn a_removal_under_a_local_file_needs_nothing_applied_so_takes_the_base_silently() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("config", "q/r", "r\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        // The person replaces the directory with a file of their own.
        fs::remove_dir_all(folder.path().join("q")).unwrap();
        fs::write(folder.path().join("q"), "mine\n").unwrap();

        location
            .run_with_stdin(&["store", "shell"], "stage config\ndelete config q/r\ncommit\n")
            .expect_success();
        // Local and Store are both absent at `q/r`, so the Store's absence becomes the Base.
        let events = sync.wait_for("caught-up");
        let names: Vec<&str> =
            events.iter().map(|event| event["event"].as_str().unwrap()).collect();
        assert_eq!(names, ["caught-up"], "{backend}");
        sync.stop();
        assert_eq!(fs::read_to_string(folder.path().join("q")).unwrap(), "mine\n", "{backend}");

        fs::remove_file(folder.path().join("q")).unwrap();
        let run = commit_in(folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[test]
fn a_change_under_a_local_file_to_a_path_with_a_base_is_diverged() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("config", "q/r", "r\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        // The person replaces the directory with a file of their own, deleting `q/r`.
        fs::remove_dir_all(folder.path().join("q")).unwrap();
        fs::write(folder.path().join("q"), "mine\n").unwrap();

        location.write("config", "q/r", "theirs\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["q/r"], "{backend}: {events:?}");
        sync.stop();
        assert_eq!(fs::read_to_string(folder.path().join("q")).unwrap(), "mine\n", "{backend}");
        assert_eq!(fs::read_to_string(theirs(folder.path(), "q/r")).unwrap(), "theirs\n");
    }
}

#[cfg(unix)]
#[test]
fn a_theirs_that_cant_be_written_is_an_error_and_sync_carries_on() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        synced(&location, "config", folder.path());
        let theirs_directory = folder.path().join(".tidings/theirs");
        std::os::unix::fs::symlink(outside.path(), &theirs_directory).unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("config", "app.toml", "theirs\n");

        // Quiet, since an error needs the person's attention.
        let mut command = location.command(&["--json"]);
        command.args(["sync", "--quiet", "config"]).arg(folder.path());
        let mut sync = Sync::spawn(command);
        // Only the error, since a *diverged* line would name a `theirs` file that wasn't written.
        let error: serde_json::Value = serde_json::from_str(&sync.next_line()).unwrap();
        assert_eq!(error["event"], "error", "{backend}: {error}");
        assert_eq!(error["path"], "app.toml", "{backend}: {error}");
        let message = "Diverged, but can't write .tidings/theirs/app.toml: \
                       theirs is a symlink in .tidings";
        assert_eq!(error["message"], message, "{backend}: {error}");

        // `sync` carries on with the other Paths.
        location.write("config", "later.toml", "later\n");
        wait_until(|| folder.path().join("later.toml").exists());
        sync.stop();
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none(), "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");

        // The Path was recorded Diverged, and its Base kept, so once `theirs` can be written, the
        // next reconcile writes it without reporting the Divergence again.
        fs::remove_file(&theirs_directory).unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        let written = fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap();
        assert_eq!(written, "theirs\n", "{backend}");
        let run = commit_in(folder.path(), &[]).expect_code(3);
        assert!(run.stderr.contains("app.toml"), "{backend}: {run:?}");
    }
}

#[test]
fn a_theirs_that_cant_be_written_is_reported_once_until_the_divergence_changes() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        // A directory of the person's where `theirs` would be written.
        let theirs_directory = theirs(folder.path(), "app.toml");
        fs::create_dir_all(&theirs_directory).unwrap();
        fs::write(theirs_directory.join("keep"), "keep\n").unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();

        location.write("config", "app.toml", "theirs 1\n");
        let events = sync.wait_for("caught-up");
        let names: Vec<&str> =
            events.iter().map(|event| event["event"].as_str().unwrap()).collect();
        assert_eq!(names, ["error", "caught-up"], "{backend}: {events:?}");
        let message = message_for(&events, "app.toml");
        let why = message.strip_prefix("Diverged, but can't write .tidings/theirs/app.toml: ");
        // Naming the file once, relative to the folder.
        assert!(why.is_some_and(|why| !why.contains('/')), "{backend}: {message}");

        // Tried again on every reconcile, but not reported again.
        location.write("config", "other.toml", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");

        // Until the Divergence changes.
        location.write("config", "app.toml", "theirs 2\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "error"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");

        // Once the directory is gone, the next reconcile writes `theirs`, silently.
        fs::remove_dir_all(&theirs_directory).unwrap();
        location.write("config", "another.toml", "another\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["another.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        sync.stop();
        let written = fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap();
        assert_eq!(written, "theirs 2\n", "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
    }
}

#[test]
fn a_theirs_that_cant_be_removed_is_an_error_and_sync_carries_on() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("themes/dark.toml"), "mine\n").unwrap();
        location.write("config", "app.toml", "theirs\n");
        location.write("config", "themes/dark.toml", "theirs\n");
        let mut diverged = Vec::new();
        while diverged.len() < 2 {
            let events = sync.wait_for("caught-up");
            diverged.extend(paths(&events, "diverged").into_iter().map(str::to_owned));
        }
        // The person puts a directory of their own in place of the `theirs` file, and the Store
        // goes back to its Base, so the Path is no longer Diverged, all the same.
        let replace_theirs = |path| {
            let theirs_file = theirs(folder.path(), path);
            fs::remove_file(&theirs_file).unwrap();
            fs::create_dir(&theirs_file).unwrap();
            fs::write(theirs_file.join("keep"), "keep\n").unwrap();
        };
        replace_theirs("app.toml");
        location.write("config", "app.toml", "a = 1\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "resolved"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(paths(&events, "error"), ["app.toml"], "{backend}: {events:?}");
        let error = events.iter().find(|event| event["event"] == "error").unwrap();
        let message = error["message"].as_str().unwrap();
        let why = message.strip_prefix("can't remove .tidings/theirs/app.toml: ");
        assert!(why.is_some_and(|why| !why.contains('/')), "{backend}: {message}");

        // Likewise, but removed in the Store, so Diverged with no `theirs`.
        replace_theirs("themes/dark.toml");
        location.run(&["store", "delete", "config", "themes/dark.toml"]).expect_success();
        let events = sync.wait_for("caught-up");
        let message = message_for(&events, "themes/dark.toml");
        assert_eq!(message, "removed in the Store", "{backend}: {events:?}");
        assert_eq!(paths(&events, "error"), ["themes/dark.toml"], "{backend}: {events:?}");

        // `sync` carries on, trying again without reporting either again.
        location.write("config", "other.toml", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        sync.stop();
        for path in ["app.toml", "themes/dark.toml"] {
            let kept = fs::read_to_string(theirs(folder.path(), path).join("keep")).unwrap();
            assert_eq!(kept, "keep\n", "{backend}");
        }

        // The record was saved: restarting reports neither Path again.
        let mut sync = Sync::start(&location, "config", folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        let run = commit_in(folder.path(), &[]).expect_code(3);
        assert!(run.stderr.contains("themes/dark.toml"), "{backend}: {run:?}");
        assert!(!run.stderr.contains("app.toml"), "{backend}: {run:?}");
    }
}

#[test]
fn a_missing_theirs_is_written_again() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());
        fs::write(folder.path().join("themes/dark.toml"), "mine\n").unwrap();
        location.write("config", "themes/dark.toml", "theirs\n");
        synced(&location, "config", folder.path());

        // Removed while `sync` is stopped, by the person, or as a crash between removing it and
        // saving the record leaves it.
        fs::remove_dir_all(folder.path().join(".tidings/theirs")).unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        let events = sync.wait_for("caught-up");
        sync.stop();
        // Diverged with the same Revision as before, so not reported again.
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        let written = fs::read_to_string(theirs(folder.path(), "themes/dark.toml")).unwrap();
        assert_eq!(written, "theirs\n", "{backend}");
    }
}

#[test]
fn a_divergence_clears_once_the_store_is_back_at_the_base() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");
        // `n` has no Base, and `app.toml` has one.
        fs::write(folder.path().join("n"), "mine\n").unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("config", "n", "theirs\n");
        location.write("config", "app.toml", "theirs\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml", "n"], "{backend}: {events:?}");

        // The Store goes back: `n` absent again, and `app.toml` holding its Base's contents, which
        // give its Base's Revision.
        location.run(&["store", "delete", "config", "n"]).expect_success();
        location.write("config", "app.toml", "a = 1\n");
        let mut resolved = Vec::new();
        while resolved.len() < 2 {
            let events = sync.wait_for("caught-up");
            assert!(paths(&events, "diverged").is_empty(), "{backend}: {events:?}");
            resolved.extend(paths(&events, "resolved").into_iter().map(str::to_owned));
        }
        resolved.sort();
        assert_eq!(resolved, ["app.toml", "n"], "{backend}");
        sync.stop();

        // Local edits stay local, and are changes to commit.
        assert_eq!(fs::read_to_string(folder.path().join("n")).unwrap(), "mine\n");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        assert!(!theirs(folder.path(), "n").exists(), "{backend}");
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
        let run = commit_in(folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\nadded n\n", "{backend}: {run:?}");
    }
}

#[test]
fn sync_quiet_prints_a_diverged_path() {
    for json in [true, false] {
        let location = Location::with_store("fs");
        location.write("config", "a", "a");
        let folder = TempDir::new().unwrap();
        let mut command = location.command(if json { &["--json"] } else { &[] });
        command.args(["sync", "--quiet", "config"]).arg(folder.path());
        let mut sync = Sync::spawn(command);
        wait_until(|| folder.path().join("a").exists());
        fs::write(folder.path().join("a"), "mine").unwrap();
        location.write("config", "a", "theirs");
        let message = "the Store's version is in .tidings/theirs/a";
        let expected = if json {
            let theirs = r#""theirs":".tidings/theirs/a""#;
            format!(r#"{{"event":"diverged","message":"{message}","path":"a",{theirs}}}"#)
        } else {
            format!("diverged a: {message}")
        };
        assert_eq!(sync.next_line(), expected);
        sync.stop();
    }
}

/// The Paths of the events in `events` named `name`, in order.
fn paths<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a str> {
    events
        .iter()
        .filter(|event| event["event"] == name)
        .map(|event| event["path"].as_str().unwrap())
        .collect()
}

/// The first event in `events` for `path`.
#[track_caller]
fn event_for<'a>(events: &'a [serde_json::Value], path: &str) -> &'a serde_json::Value {
    let event = events.iter().find(|event| event["path"] == path);
    event.unwrap_or_else(|| panic!("no event for {path}: {events:?}"))
}

/// The message of the first event in `events` for `path`.
#[track_caller]
fn message_for<'a>(events: &'a [serde_json::Value], path: &str) -> &'a str {
    event_for(events, path)["message"].as_str().unwrap_or_else(|| panic!("{events:?}"))
}

/// Where the Working copy that is `folder` keeps the Store's version of the Diverged `path`.
fn theirs(folder: &Path, path: &str) -> std::path::PathBuf {
    folder.join(".tidings/theirs").join(path)
}

/// Removes the cache Area's directory in `location`, a filesystem Store, as when the OS clears
/// the cache, which gives a Resync for the Area. SQLite gives one only after falling ten minutes
/// behind.
fn clear_the_cache_directory(location: &Location) {
    fs::remove_dir_all(location.root().join("cache")).unwrap();
}

#[test]
fn resuming_catches_up_on_what_the_store_did_meanwhile() {
    for backend in BACKENDS {
        for with_flags in [true, false] {
            let label = format!("{backend}, with Store flags: {with_flags}");
            let location = store_with_config(backend);
            location.write("config", "kept.toml", "kept\n");
            let folder = TempDir::new().unwrap();
            synced(&location, "config", folder.path());

            location.write("config", "app.toml", "a = 2\n");
            location.write("config", "kept.toml", "theirs\n");
            location.write("config", "new.toml", "new\n");
            location.run(&["store", "delete", "config", "themes/dark.toml"]).expect_success();
            location.write("config", "same.toml", "same\n");
            fs::write(folder.path().join("kept.toml"), "mine\n").unwrap();
            // A file with no Base holding what the Store has, as a reconcile that stopped partway
            // leaves, silently takes the Store's version as its Base.
            fs::write(folder.path().join("same.toml"), "same\n").unwrap();

            let mut sync = if with_flags {
                Sync::start(&location, "config", folder.path())
            } else {
                Sync::start_with(&[], "config", folder.path())
            };
            let events = sync.wait_for("caught-up");
            sync.stop();
            assert_eq!(paths(&events, "created"), ["new.toml"], "{label}: {events:?}");
            assert_eq!(paths(&events, "updated"), ["app.toml"], "{label}: {events:?}");
            assert_eq!(paths(&events, "removed"), ["themes/dark.toml"], "{label}: {events:?}");
            assert_eq!(paths(&events, "diverged"), ["kept.toml"], "{label}: {events:?}");
            assert_eq!(events.len(), 5, "{label}: {events:?}");
            assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 2\n");
            assert_eq!(fs::read_to_string(folder.path().join("kept.toml")).unwrap(), "mine\n");
            assert!(!folder.path().join("themes").exists(), "{label}");

            // It follows the Store again.
            let mut sync = Sync::start(&location, "config", folder.path());
            sync.wait_for("caught-up");
            location.write("config", "later.toml", "later\n");
            let events = sync.wait_for("caught-up");
            assert_eq!(paths(&events, "created"), ["later.toml"], "{label}: {events:?}");
            sync.stop();

            // The local edit, against its old Base, is a Conflict.
            let run = commit_in(folder.path(), &[]).expect_code(3);
            assert!(run.stderr.contains("kept.toml"), "{label}: {run:?}");
            // With it undone, nothing is a change: `same.toml` has its Base.
            fs::write(folder.path().join("kept.toml"), "kept\n").unwrap();
            let run = commit_in(folder.path(), &[]).expect_success();
            assert!(run.stderr.contains("nothing to commit"), "{label}: {run:?}");
        }
    }
}

#[test]
fn store_flags_that_dont_match_the_record_are_refused() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let other = Location::with_store(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        let folder_arg = folder.path().to_str().unwrap();
        let other_root = other.root().to_str().unwrap();
        let other_backend = if backend == "fs" { "sqlite" } else { "fs" };

        // Each a flag, or the environment variable standing in for one, and its value.
        let mismatches = [
            ("--root", other_root),
            ("--identity", "com.example.app"),
            ("--backend", other_backend),
            ("TIDINGS_ROOT", other_root),
            ("TIDINGS_BACKEND", other_backend),
        ];
        for (name, value) in mismatches {
            for command in [&["sync", "config", folder_arg][..], &["commit", "-C", folder_arg]] {
                let mut tidings = tidings();
                if name.starts_with("--") {
                    tidings.args([name, value]);
                } else {
                    tidings.env(name, value);
                }
                tidings.args(command);
                let run = common::run(tidings, "").expect_code(1);
                let label = format!("{backend} {name} {value} {command:?}");
                // Both sides of the difference are named.
                assert!(run.stderr.contains(value), "{label}: {run:?}");
                assert!(run.stderr.contains(location.root().to_str().unwrap()), "{label}: {run:?}");
            }
        }
        // Nothing was committed, and the folder is as it was.
        assert_eq!(location.read("config", "app.toml"), "a = 1\n", "{backend}");
        assert_eq!(other.run(&["store", "list", "config"]).expect_success().stdout, "");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 2\n");

        // Flags that match are fine, for `commit` as for `sync`.
        let root = location.root().to_str().unwrap();
        let mut command = tidings();
        command.args(["--root", root, "--backend", backend, "commit", "-C", folder_arg]);
        common::run(command, "").expect_success();
        assert_eq!(location.read("config", "app.toml"), "a = 2\n", "{backend}");
    }
}

#[test]
fn sync_for_a_different_area_is_refused() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, "config", folder.path());
        location.write("data", "d.txt", "d\n");

        let folder_arg = folder.path().to_str().unwrap();
        let run = location.run(&["sync", "data", folder_arg]).expect_code(1);
        assert!(run.stderr.contains("config") && run.stderr.contains("data"), "{run:?}");
        assert!(!folder.path().join("d.txt").exists(), "{backend}");
    }
}

#[test]
fn sync_into_a_folder_that_isnt_empty_changes_nothing_in_it() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        fs::write(folder.path().join("notes.txt"), "mine\n").unwrap();
        let folder_arg = folder.path().to_str().unwrap();
        let run = location.run(&["sync", "config", folder_arg]).expect_code(1);
        assert!(run.stderr.contains("isn't empty"), "{backend}: {run:?}");
        let names: Vec<_> =
            fs::read_dir(folder.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["notes.txt"], "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("notes.txt")).unwrap(), "mine\n");
    }
}

#[test]
fn a_second_sync_of_a_working_copy_is_refused() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, "config", folder.path());
        sync.wait_for("caught-up");

        let folder_arg = folder.path().to_str().unwrap();
        let run = location.run(&["sync", "config", folder_arg]).expect_code(1);
        assert!(run.stderr.contains("running"), "{backend}: {run:?}");

        // The first keeps running.
        location.write("config", "new.toml", "new\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");

        // The second is refused before it opens the Store: with the Store moved away, it says the
        // first is running, not that there is no Store, and makes nothing where the Store was.
        let moved = TempDir::new().unwrap();
        let moved_root = moved.path().join("store");
        fs::rename(location.root(), &moved_root).unwrap();
        let mut command = tidings();
        command.args(["sync", "config"]).arg(folder.path());
        let run = common::run(command, "");
        let root_was_made = location.root().exists();
        fs::rename(&moved_root, location.root()).unwrap();
        let run = run.expect_code(1);
        assert!(run.stderr.contains("running already"), "{backend}: {run:?}");
        assert!(!root_was_made, "{backend}: {run:?}");
        sync.stop();
    }
}

#[test]
fn sync_on_a_memory_store_is_refused() {
    let parent = TempDir::new().unwrap();
    let folder = parent.path().join("cfg");
    let mut command = tidings();
    command.args(["--backend", "memory", "sync", "config"]).arg(&folder);
    let run = common::run(command, "").expect_code(1);
    assert!(run.stderr.contains("no other process"), "{run:?}");
    assert!(!folder.exists());
}

#[test]
fn a_working_copy_keeps_working_after_its_folder_is_moved() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let parent = TempDir::new().unwrap();
        let folder = parent.path().join("cfg");
        synced(&location, "config", &folder);
        let moved = parent.path().join("moved");
        fs::rename(&folder, &moved).unwrap();

        fs::write(moved.join("app.toml"), "a = 2\n").unwrap();
        commit_in(&moved, &[]).expect_success();
        assert_eq!(location.read("config", "app.toml"), "a = 2\n", "{backend}");

        location.write("config", "new.toml", "new\n");
        let mut sync = Sync::start_with(&[], "config", &moved);
        let events = sync.wait_for("caught-up");
        sync.stop();
        assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        assert!(!folder.exists(), "{backend}");
    }
}

#[test]
fn a_working_copy_whose_store_has_gone_fails_to_open() {
    for backend in BACKENDS {
        let parent = TempDir::new().unwrap();
        let root = parent.path().join("store");
        let root_arg = root.to_str().unwrap();
        let mut command = tidings();
        command.args(["--root", root_arg, "--backend", backend, "--create", "store", "write"]);
        command.args(["config", "app.toml", "--contents", "a = 1\n"]);
        common::run(command, "").expect_success();
        let folder = parent.path().join("cfg");
        let mut sync = Sync::start_with(&["--root", root_arg], "config", &folder);
        sync.wait_for("caught-up");
        sync.stop();
        fs::remove_dir_all(&root).unwrap();
        fs::write(folder.join("new.toml"), "new\n").unwrap();

        let folder_arg = folder.to_str().unwrap();
        let invocations = [&["commit", "-C", folder_arg][..], &["sync", "config", folder_arg]];
        for invocation in invocations {
            // It says the Working copy's Store is missing, and not to make one with `--create`,
            // which would sync the folder with an empty Store.
            let mut command = tidings();
            command.args(invocation);
            let run = common::run(command, "").expect_code(1);
            let label = format!("{backend} {invocation:?}");
            assert!(run.stderr.contains("missing"), "{label}: {run:?}");
            assert!(run.stderr.contains(root_arg), "{label}: {run:?}");
            assert!(!run.stderr.contains("--create"), "{label}: {run:?}");

            // `--create` is refused, alone or with the Store flags the record has.
            let with_create: [&[&str]; 2] =
                [&["--create"], &["--root", root_arg, "--backend", backend, "--create"]];
            for flags in with_create {
                let mut command = tidings();
                command.args(flags).args(invocation);
                let run = common::run(command, "").expect_code(1);
                assert!(run.stderr.contains("--create"), "{label} {flags:?}: {run:?}");
            }
        }
        assert!(!root.exists(), "{backend}: a Store was made");
        assert_eq!(fs::read_to_string(folder.join("app.toml")).unwrap(), "a = 1\n", "{backend}");
        assert_eq!(fs::read_to_string(folder.join("new.toml")).unwrap(), "new\n", "{backend}");
    }
}

#[test]
fn two_working_copies_of_one_area_both_follow_it() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let (first, second) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let mut syncs = [
            Sync::start(&location, "config", first.path()),
            Sync::start(&location, "config", second.path()),
        ];
        for sync in &mut syncs {
            sync.wait_for("caught-up");
        }

        location.write("config", "new.toml", "new\n");
        for sync in &mut syncs {
            let events = sync.wait_for("caught-up");
            assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");
        }

        // A commit from one reaches the other as a Change from the Store.
        fs::write(first.path().join("app.toml"), "a = 2\n").unwrap();
        commit_in(first.path(), &[]).expect_success();
        let [first_sync, mut second_sync] = syncs;
        let events = second_sync.wait_for("caught-up");
        assert_eq!(paths(&events, "updated"), ["app.toml"], "{backend}: {events:?}");
        let app = fs::read_to_string(second.path().join("app.toml")).unwrap();
        assert_eq!(app, "a = 2\n", "{backend}");
        first_sync.stop();
        second_sync.stop();
    }
}
