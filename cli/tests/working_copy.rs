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
        sync.stop();
        assert!(!folder.path().join("thumbnails").exists(), "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("b.txt")).unwrap(), "mine");
    }
}

#[test]
fn a_local_change_is_left_alone_when_the_store_changes_the_path() {
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
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();

        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        assert!(!folder.path().join("themes/dark.toml").exists(), "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("added.toml")).unwrap(), "mine\n");
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
        let mut blocked = paths(&events, "error");
        blocked.sort();
        assert_eq!(blocked, ["a/b", "a/c"], "{backend}: {events:?}");
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
fn a_path_blocked_by_a_local_file_is_reported_and_the_rest_still_applied() {
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
        assert_eq!(paths(&events, "error"), ["q/r"], "{backend}: {events:?}");
        let error = events.iter().find(|event| event["event"] == "error").unwrap();
        assert_eq!(error["message"], "q isn't a directory", "{backend}: {events:?}");
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
fn sync_quiet_prints_a_blocked_path() {
    for json in [true, false] {
        let location = Location::with_store("fs");
        location.write("config", "a", "a");
        let folder = TempDir::new().unwrap();
        let mut command = location.command(if json { &["--json"] } else { &[] });
        command.args(["sync", "--quiet", "config"]).arg(folder.path());
        let mut sync = Sync::spawn(command);
        wait_until(|| folder.path().join("a").exists());
        fs::write(folder.path().join("q"), "mine\n").unwrap();
        location.write("config", "q/r", "r");
        let expected = if json {
            r#"{"event":"error","message":"q isn't a directory","path":"q/r"}"#
        } else {
            "error q/r: q isn't a directory"
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
            assert_eq!(events.len(), 4, "{label}: {events:?}");
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
