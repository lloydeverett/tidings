//! Working copies: `tidings sync` makes a folder hold a Store's Files, the person edits them with
//! ordinary tools (`std::fs` here), and `tidings commit` commits the edits back.

mod common;

use std::collections::BTreeMap;
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
    location.write("app.toml", "a = 1\n");
    location.write("themes/dark.toml", "bg = \"black\"\n");
    location
}

/// Syncs the Store at `location` into `folder`, waits until it's caught up, and stops it, giving
/// the events it reported, the last being *caught-up*.
fn synced(location: &Location, folder: &Path) -> Vec<serde_json::Value> {
    let mut sync = Sync::start(location, folder);
    let events = sync.wait_for("caught-up");
    sync.stop();
    events
}

/// Commits in `folder`, checking that it succeeds with nothing to commit, which `label` names.
fn commits_nothing(label: &str, folder: &Path) {
    let run = run_in("commit", folder, &[]).expect_success();
    assert!(run.stdout.is_empty() && run.stderr.contains("nothing to commit"), "{label}: {run:?}");
}

#[test]
fn sync_writes_every_file_into_a_missing_folder() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let parent = TempDir::new().unwrap();
        let folder = parent.path().join("cfg");
        let mut sync = Sync::start(&location, &folder);
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
    let mut command = location.command(&["sync"]);
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
        synced(&location, folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::create_dir(folder.path().join("keys")).unwrap();
        fs::write(folder.path().join("keys/vim.toml"), "mode = \"normal\"\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        // No Store flags: the Working copy knows its Store.
        let run = run_in("commit", folder.path(), &[]).expect_success();
        let lines: Vec<&str> = run.stdout.lines().collect();
        assert_eq!(
            lines,
            ["modified app.toml", "added keys/vim.toml", "deleted themes/dark.toml"],
            "{backend}"
        );

        assert_eq!(location.read("app.toml"), "a = 2\n");
        assert_eq!(location.read("keys/vim.toml"), "mode = \"normal\"\n");
        location.run(&["store", "read", "themes/dark.toml"]).expect_code(2);

        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stdout.is_empty() && run.stderr.contains("nothing to commit"), "{run:?}");
    }
}

#[test]
fn committed_revisions_become_the_bases() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::write(folder.path().join("new.toml"), "new\n").unwrap();
        let run = run_in("commit", folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let committed = json["committed"].as_array().unwrap();
        assert_eq!(committed.len(), 2, "{json}");
        for change in committed {
            let path = change["path"].as_str().unwrap();
            let stat = location.run(&["--json", "store", "stat", path]).expect_success();
            let stat: serde_json::Value = serde_json::from_str(&stat.stdout).unwrap();
            assert_eq!(change["revision"], stat["revision"], "{json}");
        }

        // Committing again from the new Bases is no Conflict.
        fs::write(folder.path().join("app.toml"), "a = 3\n").unwrap();
        fs::remove_file(folder.path().join("new.toml")).unwrap();
        let run = run_in("commit", folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(json["committed"][1]["change"], "deleted", "{json}");
        assert!(json["committed"][1]["revision"].is_null(), "{json}");
        assert_eq!(location.read("app.toml"), "a = 3\n");
        location.run(&["store", "read", "new.toml"]).expect_code(2);
    }
}

#[test]
fn a_conflict_commits_nothing_and_marks_each_conflicting_path_diverged_as_sync_would() {
    // A Working copy whose commit meets a Conflict for several Paths, each changed in the Store
    // while `sync` isn't running.
    let conflicting = |backend| {
        let location = store_with_config(backend);
        location.write("gone.toml", "gone\n");
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let script = "stage\nwrite app.toml --contents theirs\n\
                      write created.toml --contents theirs\n\
                      write themes/dark.toml --contents theirs\n\
                      write same.toml --contents same\n\
                      delete gone.toml\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("created.toml"), "mine\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        fs::write(folder.path().join("same.toml"), "same").unwrap();
        fs::write(folder.path().join("gone.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("unrelated.toml"), "mine\n").unwrap();
        (location, folder)
    };
    let same_message = "the Store has the same contents, which are now its Base";
    for backend in BACKENDS {
        let (_location, folder) = conflicting(backend);
        let run = run_in("commit", folder.path(), &[]).expect_code(3);
        let lines: Vec<&str> = run.stderr.lines().collect();
        let theirs_in = |path: &str| format!("the Store's version is in .tidings/theirs/{path}");
        let diverged = |path: &str| format!("  diverged {path}: {}", theirs_in(path));
        assert_eq!(
            lines,
            [
                "tidings: Conflict: the Store changed these since their Base, so nothing was \
                 committed"
                    .to_owned(),
                diverged("app.toml"),
                diverged("created.toml"),
                "  diverged gone.toml: removed in the Store".to_owned(),
                format!("  same same.toml: {same_message}"),
                diverged("themes/dark.toml"),
            ],
            "{backend}: {run:?}"
        );

        let (location, folder) = conflicting(backend);
        let run = run_in("commit", folder.path(), &["--json"]).expect_code(3);
        let failure: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
        let theirs_in = |path: &str| format!("the Store's version is in .tidings/theirs/{path}");
        let diverged = |path: &str| {
            serde_json::json!({
                "event": "diverged",
                "path": path,
                "message": theirs_in(path),
                "theirs": format!(".tidings/theirs/{path}"),
            })
        };
        assert_eq!(
            failure,
            serde_json::json!({
                "failure": "conflict",
                "message": "Conflict: the Store changed these since their Base, so nothing was \
                            committed",
                "paths": [
                    diverged("app.toml"),
                    diverged("created.toml"),
                    {
                        "event": "diverged",
                        "path": "gone.toml",
                        "message": "removed in the Store",
                        "theirs": null,
                    },
                    {"event": "same", "path": "same.toml", "message": same_message},
                    diverged("themes/dark.toml"),
                ],
            }),
            "{backend}: {run:?}"
        );

        // Nothing was committed, and the local files are untouched.
        assert_eq!(location.read("app.toml"), "theirs", "{backend}");
        assert_eq!(location.read("created.toml"), "theirs", "{backend}");
        location.run(&["store", "read", "unrelated.toml"]).expect_code(2);
        let local = fs::read_to_string(folder.path().join("app.toml")).unwrap();
        assert_eq!(local, "mine\n", "{backend}");
        assert!(!folder.path().join("themes/dark.toml").exists(), "{backend}");
        for path in ["app.toml", "created.toml", "themes/dark.toml"] {
            let written = fs::read_to_string(theirs(folder.path(), path)).unwrap();
            assert_eq!(written, "theirs", "{backend}: {path}");
        }
        assert!(!theirs(folder.path(), "gone.toml").exists(), "{backend}");

        // Recorded exactly as `sync` would have: it finds nothing more to do or report.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        // `same.toml` took the Store's version as its Base, so it isn't a change.
        let run = run_in("commit", folder.path(), &["same.toml"]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[test]
fn a_conflict_while_sync_runs_leaves_the_path_diverged_either_way() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");

        // Another command holds the lock, so `sync` can't reconcile the Store's change before
        // `commit` starts; once it is released, either may go first, but never both at once.
        let lock = fs::File::create(folder.path().join(".tidings/lock")).unwrap();
        lock.lock().unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
        let mut command = tidings();
        command.args(["--json", "commit"]).current_dir(folder.path());
        let commit = common::spawn(&mut command);
        // Only so that `commit` is likely to be waiting for the lock by the time it's released, so
        // that a Conflict is what's usually tested. Nothing it does before then can be seen from
        // here, and the test holds whichever goes first.
        thread::sleep(Duration::from_millis(200));
        drop(lock);
        let output = commit.wait_with_output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        // A Conflict, or refused as Diverged if `sync` went first.
        assert_eq!(output.status.code(), Some(3), "{backend}: {stderr}");
        let failure: serde_json::Value = serde_json::from_str(&stderr).unwrap();
        assert!(["conflict", "diverged"].contains(&failure["failure"].as_str().unwrap()));
        let failed = failure["paths"].as_array().unwrap();
        assert_eq!(paths(failed, "diverged"), ["app.toml"], "{backend}: {stderr}");

        // `sync` reports the Divergence at most once, whichever found it.
        location.write("later.toml", "later\n");
        let mut events = Vec::new();
        loop {
            let batch = sync.wait_for("caught-up");
            let done = paths(&batch, "created").contains(&"later.toml");
            events.extend(batch);
            if done {
                break;
            }
        }
        assert!(paths(&events, "diverged").len() <= 1, "{backend}: {events:?}");
        sync.stop();
        assert_eq!(location.read("app.toml"), "theirs\n", "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        assert_eq!(fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap(), "theirs\n");
    }
}

#[cfg(unix)]
#[test]
fn a_conflict_whose_theirs_cant_be_written_is_diverged_all_the_same() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        synced(&location, folder.path());
        std::os::unix::fs::symlink(outside.path(), folder.path().join(".tidings/theirs")).unwrap();
        location.write("app.toml", "theirs\n");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();

        let run = run_in("commit", folder.path(), &[]).expect_code(3);
        let error = "  error app.toml: Diverged, but can't write .tidings/theirs/app.toml: \
                     theirs is a symlink in .tidings";
        assert!(run.stderr.lines().any(|line| line == error), "{backend}: {run:?}");
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none(), "{backend}");

        // Recorded Diverged, so the next commit is refused up front.
        let run = run_in("commit", folder.path(), &[]).expect_code(3);
        assert!(run.stderr.contains("Diverged"), "{backend}: {run:?}");
    }
}

/// Runs `test` on each backend, with `sync` both running and stopped: it's given a label for its
/// failures, a Store from [`store_with_config`], and a folder synced from it, where `diverge`
/// has made at least one Path Diverged while `sync` ran.
fn with_a_diverged_path(diverge: impl Fn(&Location, &Path), test: impl Fn(&str, &Location, &Path)) {
    for backend in BACKENDS {
        for sync_running in [false, true] {
            let label = format!("{backend}, sync running: {sync_running}");
            let location = store_with_config(backend);
            let folder = TempDir::new().unwrap();
            let mut sync = Sync::start(&location, folder.path());
            sync.wait_for("caught-up");
            diverge(&location, folder.path());
            sync.wait_for("diverged");
            let running = if sync_running {
                Some(sync)
            } else {
                sync.stop();
                None
            };
            test(&label, &location, folder.path());
            if let Some(sync) = running {
                sync.stop();
            }
        }
    }
}

#[test]
fn a_full_commit_while_a_path_is_diverged_is_refused() {
    let diverge = |location: &Location, folder: &Path| {
        fs::write(folder.join("app.toml"), "mine\n").unwrap();
        fs::remove_file(folder.join("themes/dark.toml")).unwrap();
        fs::write(folder.join("new.toml"), "new\n").unwrap();
        location.write("app.toml", "theirs\n");
        // `themes/dark.toml` is deleted on both sides, so isn't Diverged.
        location.run(&["store", "delete", "themes/dark.toml"]).expect_success();
    };
    with_a_diverged_path(diverge, |label, location, folder| {
        let message = "can't commit Diverged Paths: merge each and `tidings resolve` it, or \
                       `tidings discard` it, or name only other paths to commit";
        let run = run_in("commit", folder, &[]).expect_code(3);
        let lines: Vec<&str> = run.stderr.lines().collect();
        assert_eq!(
            lines,
            [
                &format!("tidings: {message}")[..],
                "  diverged app.toml: the Store's version is in .tidings/theirs/app.toml",
            ],
            "{label}: {run:?}"
        );
        let run = run_in("commit", folder, &["--json"]).expect_code(3);
        let failure: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
        let expected = serde_json::json!({
            "failure": "diverged",
            "message": message,
            "paths": [{
                "event": "diverged",
                "path": "app.toml",
                "message": "the Store's version is in .tidings/theirs/app.toml",
                "theirs": ".tidings/theirs/app.toml",
            }],
        });
        assert_eq!(failure, expected, "{label}: {run:?}");
        // Nothing was committed, not even the changes that aren't Diverged.
        location.run(&["store", "read", "new.toml"]).expect_code(2);
    });
}

#[test]
fn naming_a_diverged_path_is_refused_and_naming_only_others_goes_ahead() {
    let diverge = |location: &Location, folder: &Path| {
        fs::write(folder.join("themes/dark.toml"), "mine\n").unwrap();
        fs::write(folder.join("themes/light.toml"), "light\n").unwrap();
        fs::write(folder.join("app.toml"), "a = 2\n").unwrap();
        location.write("themes/dark.toml", "theirs\n");
    };
    with_a_diverged_path(diverge, |label, location, folder| {
        // A directory holding a Diverged Path names it too.
        for named in [&["--json", "themes/dark.toml"][..], &["--json", "app.toml", "themes"]] {
            let run = run_in("commit", folder, named).expect_code(3);
            let failure: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
            assert_eq!(failure["failure"], "diverged", "{label}: {run:?}");
            let refused = failure["paths"].as_array().unwrap();
            assert_eq!(paths(refused, "diverged"), ["themes/dark.toml"], "{label}: {run:?}");
            assert_eq!(location.read("app.toml"), "a = 1\n", "{label}");
        }

        let run = run_in("commit", folder, &["app.toml", "themes/light.toml"]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\nadded themes/light.toml\n", "{label}");
        assert_eq!(location.read("app.toml"), "a = 2\n", "{label}");
        assert_eq!(location.read("themes/light.toml"), "light\n", "{label}");
    });
}

#[test]
fn commit_commits_only_the_paths_named_relative_to_the_current_directory() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        location.write("themes/light.toml", "bg = \"white\"\n");
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::write(folder.path().join("themes/dark.toml"), "bg = \"navy\"\n").unwrap();
        fs::remove_file(folder.path().join("themes/light.toml")).unwrap();
        fs::create_dir_all(folder.path().join("themes/extra")).unwrap();
        fs::write(folder.path().join("themes/extra/red.toml"), "bg = \"red\"\n").unwrap();
        fs::write(folder.path().join("notes.toml"), "notes\n").unwrap();
        let themes = folder.path().join("themes");

        // A deleted file can be named too.
        let run = run_in("commit", &themes, &["dark.toml", "light.toml"]).expect_success();
        assert_eq!(run.stdout, "modified themes/dark.toml\ndeleted themes/light.toml\n");
        assert_eq!(location.read("app.toml"), "a = 1\n", "{backend}");

        // A directory means everything under it, with or without a trailing slash.
        let run = run_in("commit", &themes, &["extra/"]).expect_success();
        assert_eq!(run.stdout, "added themes/extra/red.toml\n", "{backend}");
        let run = run_in("commit", &themes, &["../app.toml", "."]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\n", "{backend}");
        location.run(&["store", "read", "notes.toml"]).expect_code(2);

        // Through a symlink to the folder, as with `-C`, and absolute.
        #[cfg(unix)]
        {
            let links = TempDir::new().unwrap();
            let link = links.path().join("link");
            std::os::unix::fs::symlink(folder.path(), &link).unwrap();
            let notes = link.join("notes.toml");
            let args = ["-C", folder.path().to_str().unwrap(), notes.to_str().unwrap()];
            let run = run_in("commit", links.path(), &args).expect_success();
            assert_eq!(run.stdout, "added notes.toml\n", "{backend}");
        }
    }
}

#[test]
fn a_path_outside_the_working_copy_or_in_its_record_is_refused() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        let elsewhere = TempDir::new().unwrap();
        fs::write(elsewhere.path().join("app.toml"), "elsewhere\n").unwrap();
        let outside = elsewhere.path().join("app.toml");

        let refusals = [
            (outside.to_str().unwrap(), "is outside the Working copy"),
            ("..", "is outside the Working copy"),
            (".tidings/working-copy", "is in .tidings"),
            (".tidings", "is in .tidings"),
            (".TIDINGS/theirs/app.toml", "is in .tidings"),
            ("missing.toml", "no such file"),
        ];
        for (named, why) in refusals {
            let run = run_in("commit", folder.path(), &["app.toml", named]).expect_code(1);
            assert!(run.stderr.contains(named) && run.stderr.contains(why), "{backend}: {run:?}");
        }
        // Nothing was committed.
        assert_eq!(location.read("app.toml"), "a = 1\n", "{backend}");
    }
}

#[cfg(unix)]
#[test]
fn a_pending_commit_updates_the_bases_says_so_and_succeeds() {
    use std::os::unix::fs::PermissionsExt;

    // Only the filesystem gives `Pending`: here because a File to delete is in a directory that
    // can't be changed, as when another program has it open on Windows.
    let location = Location::with_store("fs");
    location.write("locked/a.toml", "a\n");
    location.write("locked/c.toml", "c\n");
    let folder = TempDir::new().unwrap();
    synced(&location, folder.path());
    let locked = location.store().join("locked");
    // `commit` in the folder with `args`, while `locked` can't be changed.
    let commit_while_locked = |args: &[&str]| {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let run = run_in("commit", folder.path(), args);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        run
    };
    fs::remove_file(folder.path().join("locked/a.toml")).unwrap();
    fs::write(folder.path().join("b.toml"), "b\n").unwrap();

    let run = commit_while_locked(&["--json"]).expect_success();
    let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(json["pending"], true, "{json}");
    // No other Commit landed in between, so reconciling the committed Paths found nothing.
    assert_eq!(json["events"], serde_json::json!([]), "{json}");
    let committed = json["committed"].as_array().unwrap();
    assert_eq!(committed[0]["path"], "b.toml", "{json}");
    let stat = location.run(&["--json", "store", "stat", "b.toml"]).expect_success();
    let stat: serde_json::Value = serde_json::from_str(&stat.stdout).unwrap();
    assert_eq!(committed[0]["revision"], stat["revision"], "{json}");
    assert_eq!(committed[1]["path"], "locked/a.toml", "{json}");
    assert!(committed[1]["revision"].is_null(), "{json}");

    // The Bases were updated as on success, so nothing is left to commit, and `sync` reports
    // nothing.
    let run = run_in("commit", folder.path(), &[]).expect_success();
    assert!(run.stderr.contains("nothing to commit"), "{run:?}");
    let mut sync = Sync::start(&location, folder.path());
    let events = sync.wait_for("caught-up");
    assert_eq!(events.len(), 1, "{events:?}");
    sync.stop();
    location.run(&["store", "read", "locked/a.toml"]).expect_code(2);

    // For a person, it says what was committed, then that it isn't finished.
    fs::remove_file(folder.path().join("locked/c.toml")).unwrap();
    let run = commit_while_locked(&[]).expect_success();
    assert_eq!(run.stdout, "deleted locked/c.toml\n", "{run:?}");
    assert!(run.stderr.contains("the Commit happened, but isn't finished yet"), "{run:?}");
    location.run(&["store", "read", "locked/c.toml"]).expect_code(2);
}

#[test]
fn sync_finishes_a_working_copy_a_crash_left_without_a_record() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        // What a `sync` that stopped before writing the record leaves: `.tidings/` with its lock,
        // its ignore file and part of a record.
        let tidings = folder.path().join(".tidings");
        fs::create_dir(&tidings).unwrap();
        fs::write(tidings.join("lock"), "").unwrap();
        fs::write(tidings.join("ignore"), "*.log\n").unwrap();
        fs::write(tidings.join(".ignore.d4E5f6"), "*.l").unwrap();
        fs::write(tidings.join(".working-copy.a1B2c3"), "tidings work").unwrap();
        synced(&location, folder.path());
        // Written whole, so it is kept.
        assert_eq!(fs::read_to_string(tidings.join("ignore")).unwrap(), "*.log\n");

        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 1\n");
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(location.read("app.toml"), "a = 2\n");
    }
}

#[test]
fn sync_into_a_folder_that_isnt_empty_makes_no_store() {
    let location = Location::empty();
    let folder = TempDir::new().unwrap();
    fs::write(folder.path().join("notes.txt"), "mine\n").unwrap();
    let folder_arg = folder.path().to_str().unwrap();
    let args = ["--backend", "fs", "--create", "sync", folder_arg];
    let run = location.run(&args).expect_code(1);
    assert!(run.stderr.contains("isn't empty"), "{run:?}");
    assert!(fs::read_dir(location.store()).unwrap().next().is_none(), "a Store was made");
}

#[test]
fn a_second_commit_waiting_for_the_first_reads_the_bases_it_saved() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
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
        assert_eq!(location.read("app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_works_while_sync_runs() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        run_in("commit", folder.path(), &[]).expect_success();
        sync.stop();
        assert_eq!(location.read("app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_finds_the_working_copy_from_a_subdirectory() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        run_in("commit", &folder.path().join("themes"), &[]).expect_success();
        assert_eq!(location.read("app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_takes_the_working_copy_from_dash_c() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        let elsewhere = TempDir::new().unwrap();
        let folder_arg = folder.path().to_str().unwrap();
        run_in("commit", elsewhere.path(), &["-C", folder_arg]).expect_success();
        assert_eq!(location.read("app.toml"), "a = 2\n");
    }
}

#[test]
fn commit_outside_a_working_copy_fails() {
    let elsewhere = TempDir::new().unwrap();
    // A `.tidings/` directory alone, as a filesystem Store's Location has, isn't a Working copy.
    fs::create_dir(elsewhere.path().join(".tidings")).unwrap();
    let run = run_in("commit", elsewhere.path(), &[]).expect_code(1);
    assert!(run.stderr.contains("sync") && run.stderr.contains("-C"), "{run:?}");

    let folder = elsewhere.path().to_str().unwrap();
    run_in("commit", elsewhere.path(), &["-C", folder]).expect_code(1);
}

/// `tidings <subcommand> <args>`, with no Store flags, run in `directory`.
fn run_in(subcommand: &str, directory: &Path, args: &[&str]) -> common::Run {
    let mut command = tidings();
    command.arg(subcommand).args(args).current_dir(directory);
    common::run(command, "")
}

#[test]
fn files_the_store_writes_and_deletes_appear_and_disappear() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");

        location.write("new.toml", "new\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");
        assert_eq!(fs::read_to_string(folder.path().join("new.toml")).unwrap(), "new\n");

        location.write("app.toml", "a = 2\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "updated"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 2\n");

        location.run(&["store", "delete", "themes/dark.toml"]).expect_success();
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
        location.write("deep/er/x.toml", "x\n");
        location.write("mine/sub/y.toml", "y\n");
        location.write("mine/kept.toml", "kept\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("mine/notes.txt"), "untracked\n").unwrap();

        let script = "stage\ndelete deep/er/x.toml\ndelete mine/sub/y.toml\n\
                      delete mine/kept.toml\ncommit\n";
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
        location.write("x/y.toml", "y\n");
        location.write("z", "z\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");

        let script = "stage\ndelete x/y.toml\nwrite x --contents x\n\
                      delete z\nwrite z/w.toml --contents w\ncommit\n";
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
fn only_changes_to_the_working_copys_store_are_acted_on() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let other = Location::with_store(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");

        other.write("other.toml", "other\n");
        location.write("marker.toml", "marker\n");
        // Nothing, not even a reconcile that finds nothing to do, for the other Store's Change.
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
        let mut sync = Sync::start(&location, folder.path());
        location.write("late.toml", "late\n");
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
    location.write("thumbnails/a.png", "a");
    location.write("b.txt", "b");
    let folder = TempDir::new().unwrap();
    let mut sync = Sync::start(&location, folder.path());
    sync.wait_for("caught-up");

    remove_the_location(&location);
    sync.wait_for("resync");
    sync.wait_for("caught-up");
    sync.stop();
    assert!(!folder.path().join("thumbnails").exists());
    assert!(!folder.path().join("b.txt").exists());
}

#[test]
fn deleting_every_file_in_the_store_removes_the_unchanged_local_files() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("thumbnails/a.png", "a");
        location.write("b.txt", "b");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("b.txt"), "mine").unwrap();

        location.run(&["store", "delete-prefix", ""]).expect_success();
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        fs::write(folder.path().join("added.toml"), "mine\n").unwrap();

        let script = "stage\nwrite app.toml --contents theirs\n\
                      write themes/dark.toml --contents theirs\n\
                      write added.toml --contents theirs\ncommit\n";
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();

        location.run(&["store", "delete", "app.toml"]).expect_success();
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "same").unwrap();
        fs::write(folder.path().join("added.toml"), "same").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();

        let script = "stage\nwrite app.toml --contents same\n\
                      write added.toml --contents same\n\
                      delete themes/dark.toml\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();

        // Every Path took the Store's version as its Base, so nothing is a change.
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
    }
}

#[test]
fn a_further_store_change_refreshes_theirs() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs 1\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");

        location.write("app.toml", "theirs 2\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        let refreshed = fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap();
        assert_eq!(refreshed, "theirs 2\n", "{backend}");

        // Removed in the Store, so `theirs` goes too.
        location.run(&["store", "delete", "app.toml"]).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(message_for(&events, "app.toml"), "removed in the Store", "{backend}");
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");

        // A change elsewhere reconciles, but reports nothing more for the Diverged Path.
        location.write("other.toml", "other\n");
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("themes/dark.toml", "theirs\n");
        location.write("app.toml", "theirs\n");
        sync.wait_for("diverged");
        sync.wait_for("caught-up");

        // The person merges by taking the Store's version; the next reconcile notices.
        fs::write(folder.path().join("app.toml"), "theirs\n").unwrap();
        location.write("other.toml", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "resolved"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        sync.stop();
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
        // The emptied directories in `theirs/` go too.
        let left = fs::read_dir(folder.path().join(".tidings/theirs")).unwrap().count();
        assert_eq!(left, 0, "{backend}");

        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[test]
fn a_divergence_survives_restarting_sync() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        // Diverged while `sync` was down, and found when it resumes.
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        sync.stop();

        // Still Diverged, and not reported again.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        assert_eq!(fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap(), "theirs\n");

        // Merged while `sync` was down: the record still knew it was Diverged.
        fs::write(folder.path().join("app.toml"), "theirs\n").unwrap();
        let mut sync = Sync::start(&location, folder.path());
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), [0xff, 0xfe]).unwrap();

        location.write("app.toml", "theirs\n");
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::create_dir(folder.path().join("x")).unwrap();
        fs::write(folder.path().join("x/mine"), "mine\n").unwrap();

        location.write("x", "theirs\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["x"], "{backend}: {events:?}");
        let message = "x is a directory; the Store's version is in .tidings/theirs/x";
        assert_eq!(message_for(&events, "x"), message, "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("x/mine")).unwrap(), "mine\n");

        // Once the directory is gone, the next reconcile writes the File.
        fs::remove_dir_all(folder.path().join("x")).unwrap();
        location.write("other", "other\n");
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::write(folder.path().join("added.toml"), "added\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        run_in("commit", folder.path(), &[]).expect_success();
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
        location.write("a.txt", "a");
        let folder = TempDir::new().unwrap();
        let mut command = location.command(if json { &["--json"] } else { &[] });
        command.args(["sync", "--quiet"]).arg(folder.path());
        let mut sync = Sync::spawn(command);
        wait_until(|| folder.path().join("a.txt").exists());
        location.write("b.txt", "b");
        wait_until(|| folder.path().join("b.txt").exists());
        remove_the_location(&location);
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
        commit_all(&location, &numbered_files("v1", 300), &[]);
        let folder = TempDir::new().unwrap();
        let sync = Sync::start(&location, folder.path());
        // Stopped as soon as the first file is written, while the rest are still to come.
        let first = folder.path().join("f000");
        while !first.exists() {
            thread::sleep(Duration::from_millis(1));
        }
        let written_by_then = fs::read_dir(folder.path()).unwrap().count();
        sync.stop();
        assert!(written_by_then < 301, "{backend}: sync wrote every file before it was stopped");

        // Every File was written, and the record saved, so nothing is an added file.
        let files = fs::read_dir(folder.path()).unwrap().count();
        assert_eq!(files, 301, "{backend}");
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[cfg(unix)]
#[test]
fn nothing_outside_the_folder_is_changed_through_a_symlinked_directory() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("a/b", "b\n");
        location.write("a/c", "c\n");
        let folder = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        // The directory moved out of the folder, with a symlink to it left in its place.
        fs::rename(folder.path().join("a"), outside.path().join("a")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("a"), folder.path().join("a")).unwrap();

        let script = "stage\nwrite a/c --contents theirs\ndelete a/b\ncommit\n";
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
        location.write("d", "d\n");
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        // A file of the person's own where the Store will want a directory.
        fs::write(folder.path().join("q"), "mine\n").unwrap();

        let script = "stage\nwrite a --contents a\nwrite q/r --contents r\n\
                      write z --contents z\ncommit\n";
        location.run_with_stdin(&["store", "shell"], script).expect_success();
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["q/r"], "{backend}: {events:?}");
        let message = "q isn't a directory; the Store's version is in .tidings/theirs/q/r";
        assert_eq!(message_for(&events, "q/r"), message, "{backend}");
        assert_eq!(paths(&events, "created"), ["a", "z"], "{backend}: {events:?}");
        assert_eq!(fs::read_to_string(folder.path().join("q")).unwrap(), "mine\n", "{backend}");

        // `sync` keeps running.
        location.write("later", "later\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["later"], "{backend}: {events:?}");
        sync.stop();

        // The applied Paths were recorded, so once the person's file is gone and `sync` has
        // written `q/r`, nothing is an addition.
        fs::remove_file(folder.path().join("q")).unwrap();
        synced(&location, folder.path());
        assert_eq!(fs::read_to_string(folder.path().join("q/r")).unwrap(), "r", "{backend}");
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[test]
fn a_removal_under_a_local_file_needs_nothing_applied_so_takes_the_base_silently() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("q/r", "r\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        // The person replaces the directory with a file of their own.
        fs::remove_dir_all(folder.path().join("q")).unwrap();
        fs::write(folder.path().join("q"), "mine\n").unwrap();

        location
            .run_with_stdin(&["store", "shell"], "stage\ndelete q/r\ncommit\n")
            .expect_success();
        // Local and Store are both absent at `q/r`, so the Store's absence becomes the Base.
        let events = sync.wait_for("caught-up");
        let names: Vec<&str> =
            events.iter().map(|event| event["event"].as_str().unwrap()).collect();
        assert_eq!(names, ["caught-up"], "{backend}");
        sync.stop();
        assert_eq!(fs::read_to_string(folder.path().join("q")).unwrap(), "mine\n", "{backend}");

        fs::remove_file(folder.path().join("q")).unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[test]
fn a_change_under_a_local_file_to_a_path_with_a_base_is_diverged() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        location.write("q/r", "r\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        // The person replaces the directory with a file of their own, deleting `q/r`.
        fs::remove_dir_all(folder.path().join("q")).unwrap();
        fs::write(folder.path().join("q"), "mine\n").unwrap();

        location.write("q/r", "theirs\n");
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
        synced(&location, folder.path());
        let theirs_directory = folder.path().join(".tidings/theirs");
        std::os::unix::fs::symlink(outside.path(), &theirs_directory).unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");

        // Quiet, since an error needs the person's attention.
        let mut command = location.command(&["--json"]);
        command.args(["sync", "--quiet"]).arg(folder.path());
        let mut sync = Sync::spawn(command);
        // Only the error, since a *diverged* line would name a `theirs` file that wasn't written.
        let error: serde_json::Value = serde_json::from_str(&sync.next_line()).unwrap();
        assert_eq!(error["event"], "error", "{backend}: {error}");
        assert_eq!(error["path"], "app.toml", "{backend}: {error}");
        let message = "Diverged, but can't write .tidings/theirs/app.toml: \
                       theirs is a symlink in .tidings";
        assert_eq!(error["message"], message, "{backend}: {error}");

        // `sync` carries on with the other Paths.
        location.write("later.toml", "later\n");
        wait_until(|| folder.path().join("later.toml").exists());
        sync.stop();
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none(), "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");

        // The Path was recorded Diverged, and its Base kept, so once `theirs` can be written, the
        // next reconcile writes it without reporting the Divergence again.
        fs::remove_file(&theirs_directory).unwrap();
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        let written = fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap();
        assert_eq!(written, "theirs\n", "{backend}");
        let run = run_in("commit", folder.path(), &[]).expect_code(3);
        assert!(run.stderr.contains("app.toml"), "{backend}: {run:?}");
    }
}

#[test]
fn the_sweep_leaves_a_directory_where_a_diverged_paths_theirs_goes() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let in_the_way = theirs(folder.path(), "app.toml");
        fs::create_dir_all(&in_the_way).unwrap();
        fs::write(in_the_way.join("precious"), "precious\n").unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");

        // Starting, `sync` reconciles every Path, then sweeps stale `theirs` files.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        sync.stop();
        let message = message_for(&events, "app.toml");
        assert!(message.contains("can't write .tidings/theirs/app.toml"), "{backend}: {events:?}");
        let precious = fs::read_to_string(in_the_way.join("precious")).unwrap();
        assert_eq!(precious, "precious\n", "{backend}");
        let local = fs::read_to_string(folder.path().join("app.toml")).unwrap();
        assert_eq!(local, "mine\n", "{backend}");
    }
}

#[test]
fn a_theirs_that_cant_be_written_is_reported_once_until_the_divergence_changes() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        // A directory of the person's where `theirs` would be written.
        let theirs_directory = theirs(folder.path(), "app.toml");
        fs::create_dir_all(&theirs_directory).unwrap();
        fs::write(theirs_directory.join("keep"), "keep\n").unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();

        location.write("app.toml", "theirs 1\n");
        let events = sync.wait_for("caught-up");
        let names: Vec<&str> =
            events.iter().map(|event| event["event"].as_str().unwrap()).collect();
        assert_eq!(names, ["error", "caught-up"], "{backend}: {events:?}");
        let message = message_for(&events, "app.toml");
        let why = message.strip_prefix("Diverged, but can't write .tidings/theirs/app.toml: ");
        // Naming the file once, relative to the folder.
        assert!(why.is_some_and(|why| !why.contains('/')), "{backend}: {message}");

        // Tried again on every reconcile, but not reported again.
        location.write("other.toml", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");

        // Until the Divergence changes.
        location.write("app.toml", "theirs 2\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "error"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");

        // Once the directory is gone, the next reconcile writes `theirs`, silently.
        fs::remove_dir_all(&theirs_directory).unwrap();
        location.write("another.toml", "another\n");
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("themes/dark.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
        location.write("themes/dark.toml", "theirs\n");
        let events = wait_for_count(&mut sync, "diverged", 2);
        let diverged = sorted_paths(&events, "diverged");
        assert_eq!(diverged, ["app.toml", "themes/dark.toml"], "{backend}: {events:?}");
        // The person puts a directory of their own in place of the `theirs` file, and the Store
        // goes back to its Base, so the Path is no longer Diverged, all the same.
        let replace_theirs = |path| {
            let theirs_file = theirs(folder.path(), path);
            fs::remove_file(&theirs_file).unwrap();
            fs::create_dir(&theirs_file).unwrap();
            fs::write(theirs_file.join("keep"), "keep\n").unwrap();
        };
        replace_theirs("app.toml");
        location.write("app.toml", "a = 1\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "resolved"), ["app.toml"], "{backend}: {events:?}");
        assert_eq!(paths(&events, "error"), ["app.toml"], "{backend}: {events:?}");
        let error = events.iter().find(|event| event["event"] == "error").unwrap();
        let message = error["message"].as_str().unwrap();
        let why = message.strip_prefix("can't remove .tidings/theirs/app.toml: ");
        assert!(why.is_some_and(|why| !why.contains('/')), "{backend}: {message}");

        // Likewise, but removed in the Store, so Diverged with no `theirs`.
        replace_theirs("themes/dark.toml");
        location.run(&["store", "delete", "themes/dark.toml"]).expect_success();
        let events = sync.wait_for("caught-up");
        let message = message_for(&events, "themes/dark.toml");
        assert_eq!(message, "removed in the Store", "{backend}: {events:?}");
        assert_eq!(paths(&events, "error"), ["themes/dark.toml"], "{backend}: {events:?}");

        // `sync` carries on, trying again without reporting either again.
        location.write("other.toml", "other\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        sync.stop();
        for path in ["app.toml", "themes/dark.toml"] {
            let kept = fs::read_to_string(theirs(folder.path(), path).join("keep")).unwrap();
            assert_eq!(kept, "keep\n", "{backend}");
        }

        // The record was saved: restarting reports neither Path again.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        let run = run_in("commit", folder.path(), &[]).expect_code(3);
        assert!(run.stderr.contains("themes/dark.toml"), "{backend}: {run:?}");
        assert!(!run.stderr.contains("app.toml"), "{backend}: {run:?}");
    }
}

#[test]
fn a_missing_theirs_is_written_again() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("themes/dark.toml"), "mine\n").unwrap();
        location.write("themes/dark.toml", "theirs\n");
        synced(&location, folder.path());

        // Removed while `sync` is stopped, by the person, or as a crash between removing it and
        // saving the record leaves it.
        fs::remove_dir_all(folder.path().join(".tidings/theirs")).unwrap();
        let mut sync = Sync::start(&location, folder.path());
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
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        // `n` has no Base, and `app.toml` has one.
        fs::write(folder.path().join("n"), "mine\n").unwrap();
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("n", "theirs\n");
        location.write("app.toml", "theirs\n");
        // Two Commits, which `sync` may take in one batch or two.
        let events = wait_for_count(&mut sync, "diverged", 2);
        assert_eq!(sorted_paths(&events, "diverged"), ["app.toml", "n"], "{backend}: {events:?}");

        // The Store goes back: `n` absent again, and `app.toml` holding its Base's contents, which
        // give its Base's Revision.
        location.run(&["store", "delete", "n"]).expect_success();
        location.write("app.toml", "a = 1\n");
        let events = wait_for_count(&mut sync, "resolved", 2);
        assert!(paths(&events, "diverged").is_empty(), "{backend}: {events:?}");
        assert_eq!(sorted_paths(&events, "resolved"), ["app.toml", "n"], "{backend}: {events:?}");
        sync.stop();

        // Local edits stay local, and are changes to commit.
        assert_eq!(fs::read_to_string(folder.path().join("n")).unwrap(), "mine\n");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        assert!(!theirs(folder.path(), "n").exists(), "{backend}");
        assert!(!theirs(folder.path(), "app.toml").exists(), "{backend}");
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\nadded n\n", "{backend}: {run:?}");
    }
}

#[test]
fn sync_quiet_prints_a_diverged_path() {
    for json in [true, false] {
        let location = Location::with_store("fs");
        location.write("a", "a");
        let folder = TempDir::new().unwrap();
        let mut command = location.command(if json { &["--json"] } else { &[] });
        command.args(["sync", "--quiet"]).arg(folder.path());
        let mut sync = Sync::spawn(command);
        wait_until(|| folder.path().join("a").exists());
        fs::write(folder.path().join("a"), "mine").unwrap();
        location.write("a", "theirs");
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

/// The Paths of the events in `events` named `name`, sorted, as when they may have come in more
/// than one batch.
fn sorted_paths<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a str> {
    let mut paths = paths(events, name);
    paths.sort_unstable();
    paths
}

/// The events `sync` prints up to the first *caught-up* by which it has printed `count` events
/// named `name`. The Changes of separate Commits can come to `sync` in one batch or in several,
/// each reconciled and reported on its own, so waiting for one *caught-up* isn't enough.
#[track_caller]
fn wait_for_count(sync: &mut Sync, name: &str, count: usize) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    while paths(&events, name).len() < count {
        events.extend(sync.wait_for("caught-up"));
    }
    events
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

/// Removes the Store's Location, as when the OS clears a cache, which gives the Store a Resync.
fn remove_the_location(location: &Location) {
    fs::remove_dir_all(location.store()).unwrap();
}

#[test]
fn resuming_catches_up_on_what_the_store_did_meanwhile() {
    for backend in BACKENDS {
        for with_flags in [true, false] {
            let label = format!("{backend}, with Store flags: {with_flags}");
            let location = store_with_config(backend);
            location.write("kept.toml", "kept\n");
            let folder = TempDir::new().unwrap();
            synced(&location, folder.path());

            location.write("app.toml", "a = 2\n");
            location.write("kept.toml", "theirs\n");
            location.write("new.toml", "new\n");
            location.run(&["store", "delete", "themes/dark.toml"]).expect_success();
            location.write("same.toml", "same\n");
            fs::write(folder.path().join("kept.toml"), "mine\n").unwrap();
            // A file with no Base holding what the Store has, as a reconcile that stopped partway
            // leaves, silently takes the Store's version as its Base.
            fs::write(folder.path().join("same.toml"), "same\n").unwrap();

            let mut sync = if with_flags {
                Sync::start(&location, folder.path())
            } else {
                Sync::start_with(&[], folder.path())
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
            let mut sync = Sync::start(&location, folder.path());
            sync.wait_for("caught-up");
            location.write("later.toml", "later\n");
            let events = sync.wait_for("caught-up");
            assert_eq!(paths(&events, "created"), ["later.toml"], "{label}: {events:?}");
            sync.stop();

            // The local edit, against its old Base, is Diverged, so committing it is refused.
            let run = run_in("commit", folder.path(), &[]).expect_code(3);
            assert!(run.stderr.contains("kept.toml"), "{label}: {run:?}");
            // With it undone, `sync` takes the Store's version, and nothing is a change:
            // `same.toml` has its Base.
            fs::write(folder.path().join("kept.toml"), "kept\n").unwrap();
            synced(&location, folder.path());
            let run = run_in("commit", folder.path(), &[]).expect_success();
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
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        let folder_arg = folder.path().to_str().unwrap();
        let other_store = other.store().to_str().unwrap();
        let other_backend = if backend == "fs" { "sqlite" } else { "fs" };

        // Each a flag, or the environment variable standing in for one, and its value.
        let mismatches = [
            ("--store", other_store),
            ("--backend", other_backend),
            ("TIDINGS_STORE", other_store),
            ("TIDINGS_BACKEND", other_backend),
        ];
        for (name, value) in mismatches {
            for command in [&["sync", folder_arg][..], &["commit", "-C", folder_arg]] {
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
                assert!(
                    run.stderr.contains(location.store().to_str().unwrap()),
                    "{label}: {run:?}"
                );
            }
        }
        // Nothing was committed, and the folder is as it was.
        assert_eq!(location.read("app.toml"), "a = 1\n", "{backend}");
        assert_eq!(other.run(&["store", "list"]).expect_success().stdout, "");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 2\n");

        // Flags that match are fine, for `commit` as for `sync`.
        let store = location.store().to_str().unwrap();
        let mut command = tidings();
        command.args(["--store", store, "--backend", backend, "commit", "-C", folder_arg]);
        common::run(command, "").expect_success();
        assert_eq!(location.read("app.toml"), "a = 2\n", "{backend}");
    }
}

#[test]
fn sync_into_a_folder_that_isnt_empty_changes_nothing_in_it() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        fs::write(folder.path().join("notes.txt"), "mine\n").unwrap();
        let folder_arg = folder.path().to_str().unwrap();
        let run = location.run(&["sync", folder_arg]).expect_code(1);
        assert!(run.stderr.contains("isn't empty"), "{backend}: {run:?}");
        let names: Vec<_> =
            fs::read_dir(folder.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["notes.txt"], "{backend}");
        assert_eq!(fs::read_to_string(folder.path().join("notes.txt")).unwrap(), "mine\n");
    }
}

/// Every name under `directory`, relative to it, in order. SQLite's `-wal` and `-shm` files are
/// left out: they go when a Store's last connection closes, which can be after the command ends.
fn names_under(directory: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut to_visit = vec![directory.to_path_buf()];
    while let Some(visiting) = to_visit.pop() {
        for entry in fs::read_dir(&visiting).unwrap() {
            let path = entry.unwrap().path();
            let name = path.strip_prefix(directory).unwrap().to_string_lossy().into_owned();
            if name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            if path.is_dir() {
                to_visit.push(path);
            }
            names.push(name);
        }
    }
    names.sort();
    names
}

/// A folder can't be both a Store's Location and a Working copy of it, since the Store's files
/// would mix with the Working copy's: `sync` into the Store's own Location is refused, whether the
/// Store is there already or `--create` would make it, and nothing is made or changed.
#[test]
fn sync_into_the_stores_own_location_is_refused() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let before = names_under(location.store());
        let run = location.run(&["sync", location.store().to_str().unwrap()]).expect_code(1);
        assert!(run.stderr.contains("is the Store's Location"), "{backend}: {run:?}");
        assert_eq!(names_under(location.store()), before, "{backend}");
        assert_eq!(location.read("app.toml"), "a = 1\n", "{backend}");

        let location = Location::empty();
        let store = location.store().to_str().unwrap();
        let run = location.run(&["--backend", backend, "--create", "sync", store]).expect_code(1);
        assert!(run.stderr.contains("is the Store's Location"), "{backend}: {run:?}");
        assert!(fs::read_dir(location.store()).unwrap().next().is_none(), "{backend}: made");
    }
}

/// A Working copy's folder can't be a Store's Location either: the library refuses to open a Store
/// there, so a `store` command pointed at one fails and changes nothing.
#[test]
fn a_working_copys_folder_is_refused_as_a_stores_location() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let before = names_under(folder.path());
        let mut command = tidings();
        command.arg("--store").arg(folder.path()).args(["--backend", backend, "--create"]);
        command.args(["store", "list"]);
        let run = common::run(command, "").expect_code(1);
        assert!(run.stderr.contains("is a Working copy's folder"), "{backend}: {run:?}");
        assert_eq!(names_under(folder.path()), before, "{backend}");
    }
}

#[test]
fn a_second_sync_of_a_working_copy_is_refused() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");

        let folder_arg = folder.path().to_str().unwrap();
        let run = location.run(&["sync", folder_arg]).expect_code(1);
        assert!(run.stderr.contains("running"), "{backend}: {run:?}");

        // The first keeps running.
        location.write("new.toml", "new\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");

        // The second is refused before it opens the Store: with the Store moved away, it says the
        // first is running, not that there is no Store, and makes nothing where the Store was.
        let moved = TempDir::new().unwrap();
        let moved_store = moved.path().join("store");
        fs::rename(location.store(), &moved_store).unwrap();
        let mut command = tidings();
        command.args(["sync"]).arg(folder.path());
        let run = common::run(command, "");
        let store_was_made = location.store().exists();
        fs::rename(&moved_store, location.store()).unwrap();
        let run = run.expect_code(1);
        assert!(run.stderr.contains("running already"), "{backend}: {run:?}");
        assert!(!store_was_made, "{backend}: {run:?}");
        sync.stop();
    }
}

#[test]
fn sync_on_a_memory_store_is_refused() {
    let parent = TempDir::new().unwrap();
    let folder = parent.path().join("cfg");
    let mut command = tidings();
    command.args(["--backend", "memory", "sync"]).arg(&folder);
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
        synced(&location, &folder);
        let moved = parent.path().join("moved");
        fs::rename(&folder, &moved).unwrap();

        fs::write(moved.join("app.toml"), "a = 2\n").unwrap();
        run_in("commit", &moved, &[]).expect_success();
        assert_eq!(location.read("app.toml"), "a = 2\n", "{backend}");

        location.write("new.toml", "new\n");
        let mut sync = Sync::start_with(&[], &moved);
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
        let store = parent.path().join("store");
        let store_arg = store.to_str().unwrap();
        let mut command = tidings();
        command.args(["--store", store_arg, "--backend", backend, "--create", "store", "write"]);
        command.args(["app.toml", "--contents", "a = 1\n"]);
        common::run(command, "").expect_success();
        let folder = parent.path().join("cfg");
        let mut sync = Sync::start_with(&["--store", store_arg], &folder);
        sync.wait_for("caught-up");
        sync.stop();
        fs::remove_dir_all(&store).unwrap();
        fs::write(folder.join("new.toml"), "new\n").unwrap();

        let folder_arg = folder.to_str().unwrap();
        let invocations = [&["commit", "-C", folder_arg][..], &["sync", folder_arg]];
        for invocation in invocations {
            // It says the Working copy's Store is missing, and not to make one with `--create`,
            // which would sync the folder with an empty Store.
            let mut command = tidings();
            command.args(invocation);
            let run = common::run(command, "").expect_code(1);
            let label = format!("{backend} {invocation:?}");
            assert!(run.stderr.contains("missing"), "{label}: {run:?}");
            assert!(run.stderr.contains(store_arg), "{label}: {run:?}");
            assert!(!run.stderr.contains("--create"), "{label}: {run:?}");

            // `--create` is refused, alone or with the Store flags the record has.
            let with_create: [&[&str]; 2] =
                [&["--create"], &["--store", store_arg, "--backend", backend, "--create"]];
            for flags in with_create {
                let mut command = tidings();
                command.args(flags).args(invocation);
                let run = common::run(command, "").expect_code(1);
                assert!(run.stderr.contains("--create"), "{label} {flags:?}: {run:?}");
            }
        }
        assert!(!store.exists(), "{backend}: a Store was made");
        assert_eq!(fs::read_to_string(folder.join("app.toml")).unwrap(), "a = 1\n", "{backend}");
        assert_eq!(fs::read_to_string(folder.join("new.toml")).unwrap(), "new\n", "{backend}");
    }
}

#[test]
fn two_working_copies_of_one_store_both_follow_it() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let (first, second) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let mut syncs =
            [Sync::start(&location, first.path()), Sync::start(&location, second.path())];
        for sync in &mut syncs {
            sync.wait_for("caught-up");
        }

        location.write("new.toml", "new\n");
        for sync in &mut syncs {
            let events = sync.wait_for("caught-up");
            assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");
        }

        // A commit from one reaches the other as a Change from the Store.
        fs::write(first.path().join("app.toml"), "a = 2\n").unwrap();
        run_in("commit", first.path(), &[]).expect_success();
        let [first_sync, mut second_sync] = syncs;
        let events = second_sync.wait_for("caught-up");
        assert_eq!(paths(&events, "updated"), ["app.toml"], "{backend}: {events:?}");
        let app = fs::read_to_string(second.path().join("app.toml")).unwrap();
        assert_eq!(app, "a = 2\n", "{backend}");
        first_sync.stop();
        second_sync.stop();
    }
}

#[test]
fn a_new_working_copy_ignores_editor_and_os_leftovers() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let ignore = fs::read_to_string(folder.path().join(".tidings/ignore")).unwrap();
        let patterns: Vec<&str> =
            ignore.lines().filter(|line| !line.is_empty() && !line.starts_with('#')).collect();
        assert_eq!(patterns, [".*.sw?", "*~", "4913", ".DS_Store", "Thumbs.db", ".#*"]);

        // What vim leaves behind while saving, and what macOS and Windows drop into folders.
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::write(folder.path().join(".app.toml.swp"), [0xff, 0xfe]).unwrap();
        fs::write(folder.path().join("app.toml~"), "a = 1\n").unwrap();
        fs::write(folder.path().join("themes/4913"), "").unwrap();
        fs::write(folder.path().join("themes/.DS_Store"), [0]).unwrap();
        fs::write(folder.path().join("Thumbs.db"), "").unwrap();
        fs::write(folder.path().join(".#app.toml"), "").unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\n", "{backend}: {run:?}");
        assert_eq!(location.read("app.toml"), "a = 2\n", "{backend}");
        location.run(&["store", "read", "app.toml~"]).expect_code(2);
    }
}

#[test]
fn removing_a_pattern_from_the_ignore_file_lets_a_matching_file_be_committed() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join(".DS_Store"), "mine\n").unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");

        // Each command reads the ignore file afresh.
        let ignore = folder.path().join(".tidings/ignore");
        let without = fs::read_to_string(&ignore).unwrap().replace(".DS_Store\n", "");
        fs::write(&ignore, without).unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "added .DS_Store\n", "{backend}: {run:?}");
        assert_eq!(location.read(".DS_Store"), "mine\n", "{backend}");
    }
}

#[test]
fn a_store_file_matching_the_ignore_file_is_synced_and_its_edits_committed() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        location.write("notes.txt~", "theirs\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert!(paths(&events, "created").contains(&"notes.txt~"), "{backend}: {events:?}");

        location.write("4913", "four\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["4913"], "{backend}: {events:?}");
        sync.stop();
        assert_eq!(fs::read_to_string(folder.path().join("4913")).unwrap(), "four\n");

        // Having a Base, they are tracked like any other File.
        fs::write(folder.path().join("notes.txt~"), "mine\n").unwrap();
        fs::remove_file(folder.path().join("4913")).unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "modified notes.txt~\ndeleted 4913\n", "{backend}: {run:?}");
        assert_eq!(location.read("notes.txt~"), "mine\n", "{backend}");
        location.run(&["store", "read", "4913"]).expect_code(2);
    }
}

#[cfg(unix)]
#[test]
fn a_file_that_cant_be_a_file_refuses_the_commit_until_it_is_ignored() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        // A name Windows can't hold, contents that aren't text, a symlink and a named pipe.
        fs::write(folder.path().join("what?.toml"), "q\n").unwrap();
        fs::write(folder.path().join("themes/logo.png"), [0x89, 0x50, 0xff]).unwrap();
        std::os::unix::fs::symlink("app.toml", folder.path().join("link.toml")).unwrap();
        let pipe = folder.path().join("themes/pipe");
        assert!(std::process::Command::new("mkfifo").arg(&pipe).status().unwrap().success());

        let run = run_in("commit", folder.path(), &[]).expect_code(1);
        let listed: Vec<&str> = run.stderr.lines().skip(1).map(str::trim).collect();
        assert_eq!(
            listed,
            [
                "invalid link.toml: is a symlink",
                "invalid themes/logo.png: isn't UTF-8 text",
                "invalid themes/pipe: isn't a regular file",
                "invalid what?.toml: isn't a valid Path: a segment is not a name every platform \
                 accepts",
            ],
            "{backend}: {run:?}"
        );
        assert_eq!(location.read("app.toml"), "a = 1\n", "{backend}");

        // With `--json`, the same as data.
        let run = run_in("commit", folder.path(), &["--json", "themes"]).expect_code(1);
        let json: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
        assert_eq!(json["failure"], "invalid", "{backend}: {json}");
        let names: Vec<&str> =
            json["paths"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap()).collect();
        assert_eq!(names, ["themes/logo.png", "themes/pipe"], "{backend}: {json}");

        // Only a file that would be committed refuses it.
        let run = run_in("commit", folder.path(), &["app.toml"]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\n", "{backend}: {run:?}");

        // Ignoring them lets the commit go ahead.
        let ignore = folder.path().join(".tidings/ignore");
        let patterns = "what?.toml\nlink.toml\n*.png\nthemes/pipe\n";
        fs::write(&ignore, fs::read_to_string(&ignore).unwrap() + patterns).unwrap();
        fs::write(folder.path().join("keys.toml"), "k\n").unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "added keys.toml\n", "{backend}: {run:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_tracked_path_replaced_by_a_symlink_is_invalid_not_deleted() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let dark = folder.path().join("themes/dark.toml");
        fs::remove_file(&dark).unwrap();
        std::os::unix::fs::symlink("../app.toml", &dark).unwrap();
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        // Even a pattern matching it doesn't leave out a Path with a Base.
        let ignore = folder.path().join(".tidings/ignore");
        fs::write(&ignore, fs::read_to_string(&ignore).unwrap() + "*.toml\n").unwrap();

        let run = run_in("commit", folder.path(), &["themes/dark.toml"]).expect_code(1);
        assert!(
            run.stderr.contains("invalid themes/dark.toml: is a symlink"),
            "{backend}: {run:?}"
        );
        run_in("commit", folder.path(), &[]).expect_code(1);
        assert_eq!(location.read("themes/dark.toml"), "bg = \"black\"\n", "{backend}");

        let run = run_in("commit", folder.path(), &["app.toml"]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\n", "{backend}: {run:?}");
    }
}

#[test]
fn empty_directories_are_never_committed() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::create_dir_all(folder.path().join("keys/empty")).unwrap();
        fs::create_dir(folder.path().join("what?")).unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        let nothing = run.stdout.is_empty() && run.stderr.contains("nothing to commit");
        assert!(nothing, "{backend}: {run:?}");
        let run = run_in("commit", folder.path(), &["keys"]).expect_code(1);
        assert!(run.stderr.contains("keys: no such file"), "{backend}: {run:?}");
    }
}

#[test]
fn naming_an_ignored_file_says_it_is_ignored() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("themes/dark.toml~"), "old\n").unwrap();
        fs::create_dir(folder.path().join("backup")).unwrap();
        fs::write(folder.path().join("backup/app.toml~"), "old\n").unwrap();

        let run = run_in("commit", folder.path(), &["themes/dark.toml~"]).expect_code(1);
        let why = "themes/dark.toml~: is left out by .tidings/ignore";
        assert!(run.stderr.contains(why), "{backend}: {run:?}");
        let run = run_in("commit", folder.path(), &["backup"]).expect_code(1);
        let why = "backup: holds only files .tidings/ignore leaves out";
        assert!(run.stderr.contains(why), "{backend}: {run:?}");
        // A directory holding other files has nothing to commit.
        let run = run_in("commit", folder.path(), &["themes"]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");
    }
}

#[cfg(unix)]
#[test]
fn an_ignore_file_that_isnt_a_regular_file_refuses_the_commit() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join(".x.swp"), "swap\n").unwrap();
        // A symlink to patterns elsewhere is never followed, nor taken for no patterns.
        let elsewhere = TempDir::new().unwrap();
        let ignore = folder.path().join(".tidings/ignore");
        fs::rename(&ignore, elsewhere.path().join("ignore")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path().join("ignore"), &ignore).unwrap();

        let run = run_in("commit", folder.path(), &[".x.swp"]).expect_code(1);
        assert!(run.stderr.contains(".tidings/ignore: is a symlink"), "{backend}: {run:?}");
        let run = run_in("commit", folder.path(), &["--json"]).expect_code(1);
        let json: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
        assert_eq!(json["failure"], "error", "{backend}: {json}");
        assert!(json["message"].as_str().unwrap().contains(".tidings/ignore"), "{json}");
        location.run(&["store", "read", ".x.swp"]).expect_code(2);

        fs::remove_file(&ignore).unwrap();
        fs::create_dir(&ignore).unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_code(1);
        let why = ".tidings/ignore: isn't a regular file";
        assert!(run.stderr.contains(why), "{backend}: {run:?}");
        location.run(&["store", "read", ".x.swp"]).expect_code(2);

        // A missing one, as when the person removed it, leaves nothing out.
        fs::remove_dir(&ignore).unwrap();
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "added .x.swp\n", "{backend}: {run:?}");
    }
}

#[test]
fn a_file_in_an_ignored_directory_is_ignored_whatever_re_includes_it() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        location.write("build/tracked.txt", "t\n");
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let ignore = folder.path().join(".tidings/ignore");
        let patterns = "build/\n!build/keep\n*.log\n!keep.log\n";
        fs::write(&ignore, fs::read_to_string(&ignore).unwrap() + patterns).unwrap();
        fs::write(folder.path().join("build/keep"), "k\n").unwrap();
        fs::write(folder.path().join("build/out.o"), "o\n").unwrap();
        fs::write(folder.path().join("debug.log"), "d\n").unwrap();
        fs::write(folder.path().join("keep.log"), "k\n").unwrap();
        // A Path with a Base is committed, even in an ignored directory.
        fs::write(folder.path().join("build/tracked.txt"), "t2\n").unwrap();

        // As in git, a file can't be re-included if a directory it is in is left out, but one
        // whose directory isn't can.
        let run = run_in("commit", folder.path(), &[]).expect_success();
        let lines: Vec<&str> = run.stdout.lines().collect();
        assert_eq!(lines, ["modified build/tracked.txt", "added keep.log"], "{backend}: {run:?}");
        let run = run_in("commit", folder.path(), &["build/keep"]).expect_code(1);
        let why = "build/keep: is left out by .tidings/ignore";
        assert!(run.stderr.contains(why), "{backend}: {run:?}");
        location.run(&["store", "read", "build/keep"]).expect_code(2);
    }
}

#[test]
fn a_malformed_pattern_refuses_the_commit_naming_its_line() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("new.txt"), "new\n").unwrap();
        let ignore = folder.path().join(".tidings/ignore");
        for pattern in ["[z-a]", "a{b", "\\"] {
            fs::write(&ignore, format!("*.log\n# a comment\n{pattern}\n")).unwrap();
            let run = run_in("commit", folder.path(), &[]).expect_code(1);
            let line = format!(".tidings/ignore:3: error parsing glob '{pattern}'");
            assert!(run.stderr.contains(&line), "{backend}: {run:?}");
        }
        location.run(&["store", "read", "new.txt"]).expect_code(2);
    }
}

#[test]
fn a_byte_order_mark_doesnt_spoil_the_first_pattern() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("secret.txt"), "s\n").unwrap();
        fs::write(folder.path().join("new.txt"), "new\n").unwrap();
        // As git does, the mark some editors save is skipped, not taken as part of the pattern.
        fs::write(folder.path().join(".tidings/ignore"), "\u{feff}secret.txt\n").unwrap();

        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "added new.txt\n", "{backend}: {run:?}");
        location.run(&["store", "read", "secret.txt"]).expect_code(2);
    }
}

#[test]
fn an_ignore_file_that_isnt_utf8_refuses_the_commit_naming_its_line() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("new.txt"), "new\n").unwrap();
        fs::write(folder.path().join(".tidings/ignore"), b"*.log\n\xffnew.txt\n").unwrap();

        let run = run_in("commit", folder.path(), &[]).expect_code(1);
        let line = ".tidings/ignore:2: isn't UTF-8 text";
        assert!(run.stderr.contains(line), "{backend}: {run:?}");
        location.run(&["store", "read", "new.txt"]).expect_code(2);
    }
}

#[test]
fn status_lists_what_commit_would_commit() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        location.write("keep.toml", "keep\n");
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());

        let run = run_in("status", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "nothing to commit\nsync isn't running\n", "{backend}: {run:?}");

        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        fs::write(folder.path().join("keys.toml"), "k\n").unwrap();
        fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
        // Ignored, and an empty directory, neither of which `commit` would commit.
        fs::write(folder.path().join(".DS_Store"), "junk").unwrap();
        fs::create_dir(folder.path().join("empty")).unwrap();
        let run = run_in("status", folder.path(), &[]).expect_success();
        assert_eq!(
            run.stdout,
            "modified app.toml\nadded keys.toml\ndeleted themes/dark.toml\nsync isn't running\n",
            "{backend}: {run:?}"
        );

        let run = run_in("status", folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let expected = serde_json::json!({
            "paths": [
                {"event": "modified", "path": "app.toml"},
                {"event": "added", "path": "keys.toml"},
                {"event": "deleted", "path": "themes/dark.toml"},
            ],
            "syncing": false,
        });
        assert_eq!(json, expected, "{backend}: {run:?}");
    }
}

#[test]
fn status_compares_contents_not_modified_times() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());

        // As `touch` does, and as an editor that writes the same text again does.
        let app = fs::File::options().write(true).open(folder.path().join("app.toml")).unwrap();
        app.set_modified(std::time::SystemTime::now() + Duration::from_secs(60)).unwrap();
        fs::write(folder.path().join("themes/dark.toml"), "bg = \"black\"\n").unwrap();
        let run = run_in("status", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "nothing to commit\nsync isn't running\n", "{backend}: {run:?}");
    }
}

#[cfg(unix)]
#[test]
fn status_lists_diverged_paths_and_files_that_cant_be_files() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        location.write("gone.toml", "gone\n");
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("gone.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
        location.run(&["store", "delete", "gone.toml"]).expect_success();
        wait_for_count(&mut sync, "diverged", 2);
        sync.stop();
        // A tracked Path made a symlink is invalid, not deleted, and so is a file that isn't text.
        let dark = folder.path().join("themes/dark.toml");
        fs::remove_file(&dark).unwrap();
        std::os::unix::fs::symlink("../app.toml", &dark).unwrap();
        fs::write(folder.path().join("logo.png"), [0x89, 0x50, 0xff]).unwrap();

        let run = run_in("status", folder.path(), &[]).expect_success();
        let expected = "diverged app.toml: the Store's version is in .tidings/theirs/app.toml\n\
                        diverged gone.toml: removed in the Store\n\
                        invalid logo.png: isn't UTF-8 text\n\
                        invalid themes/dark.toml: is a symlink\n\
                        sync isn't running\n";
        assert_eq!(run.stdout, expected, "{backend}: {run:?}");

        let run = run_in("status", folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let expected = serde_json::json!({
            "paths": [
                {
                    "event": "diverged",
                    "path": "app.toml",
                    "message": "the Store's version is in .tidings/theirs/app.toml",
                    "theirs": ".tidings/theirs/app.toml",
                },
                {
                    "event": "diverged",
                    "path": "gone.toml",
                    "message": "removed in the Store",
                    "theirs": null,
                },
                {"event": "invalid", "path": "logo.png", "message": "isn't UTF-8 text"},
                {"event": "invalid", "path": "themes/dark.toml", "message": "is a symlink"},
            ],
            "syncing": false,
        });
        assert_eq!(json, expected, "{backend}: {run:?}");
    }
}

#[test]
fn status_says_whether_sync_is_running_without_disturbing_it() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");

        let run = run_in("status", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "nothing to commit\nsync is running\n", "{backend}: {run:?}");
        let run = run_in("status", folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(json["syncing"], true, "{backend}: {run:?}");
        // `sync` still runs, and still follows the Store.
        location.write("new.toml", "new\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "created"), ["new.toml"], "{backend}: {events:?}");
        sync.stop();

        let run = run_in("status", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "nothing to commit\nsync isn't running\n", "{backend}: {run:?}");
        // Nor does `status` keep a `sync` from starting.
        synced(&location, folder.path());
    }
}

#[test]
fn status_waits_for_the_working_copys_lock() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();

        // As when `sync` is halfway through a reconcile.
        let lock = fs::File::create(folder.path().join(".tidings/lock")).unwrap();
        lock.lock().unwrap();
        let mut command = tidings();
        command.arg("status").current_dir(folder.path());
        let mut status = common::spawn(&mut command);
        thread::sleep(Duration::from_millis(500));
        assert!(status.try_wait().unwrap().is_none(), "{backend}: status didn't wait");
        drop(lock);

        let output = status.wait_with_output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(0), "{backend}: {stdout}");
        assert_eq!(stdout, "modified app.toml\nsync isn't running\n", "{backend}");
    }
}

#[test]
fn status_finds_the_working_copy_by_walking_up_or_from_dash_c() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
        let expected = "modified app.toml\nsync isn't running\n";

        let run = run_in("status", &folder.path().join("themes"), &[]).expect_success();
        assert_eq!(run.stdout, expected, "{backend}: {run:?}");
        let elsewhere = TempDir::new().unwrap();
        let run = run_in("status", elsewhere.path(), &["-C", folder.path().to_str().unwrap()]);
        assert_eq!(run.expect_success().stdout, expected, "{backend}");

        // Store flags given must match the record.
        let other = elsewhere.path().to_str().unwrap();
        let run = run_in("status", folder.path(), &["--store", other]).expect_code(1);
        assert!(run.stderr.contains(other), "{backend}: {run:?}");
    }

    let elsewhere = TempDir::new().unwrap();
    // A `.tidings/` directory alone, as a filesystem Store's Location has, isn't a Working copy.
    fs::create_dir(elsewhere.path().join(".tidings")).unwrap();
    let run = run_in("status", elsewhere.path(), &[]).expect_code(1);
    assert!(run.stderr.contains("sync") && run.stderr.contains("-C"), "{run:?}");
    let run = run_in("status", elsewhere.path(), &["--json"]).expect_code(1);
    let failure: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
    assert_eq!(failure["failure"], "error", "{run:?}");
}

#[test]
fn status_refuses_a_malformed_ignore_file_as_commit_does() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let ignore = folder.path().join(".tidings/ignore");
        fs::write(&ignore, "*.bak\na{b\n").unwrap();

        let run = run_in("status", folder.path(), &[]).expect_code(1);
        assert!(run.stderr.contains(".tidings/ignore:2: "), "{backend}: {run:?}");
        let run = run_in("status", folder.path(), &["--json"]).expect_code(1);
        let failure: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
        assert_eq!(failure["failure"], "error", "{backend}: {run:?}");
    }
}

#[test]
fn discard_takes_the_stores_version_of_each_changed_path_but_leaves_added_files() {
    let diverge = |location: &Location, folder: &Path| {
        fs::write(folder.join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
    };
    with_a_diverged_path(diverge, |label, location, folder| {
        fs::remove_file(folder.join("themes/dark.toml")).unwrap();
        fs::write(folder.join("new.toml"), "new\n").unwrap();

        let run = run_in("discard", folder, &[]).expect_success();
        let took = ": took the Store's version";
        let expected =
            [format!("discarded app.toml{took}"), format!("discarded themes/dark.toml{took}")];
        assert_eq!(run.stdout.lines().collect::<Vec<_>>(), expected, "{label}: {run:?}");
        let read = |path: &str| fs::read_to_string(folder.join(path)).unwrap();
        assert_eq!(read("app.toml"), "theirs\n", "{label}");
        assert_eq!(read("themes/dark.toml"), "bg = \"black\"\n", "{label}");
        // An added file is left alone unless named.
        assert_eq!(read("new.toml"), "new\n", "{label}");
        assert!(!theirs(folder, "app.toml").exists(), "{label}");

        // The Store's versions are the Bases, so only the added file is a change.
        let run = run_in("status", folder, &[]).expect_success();
        assert!(run.stdout.starts_with("added new.toml\nsync is"), "{label}: {run:?}");
        run_in("commit", folder, &[]).expect_success();
        assert_eq!(location.read("app.toml"), "theirs\n", "{label}");

        let run = run_in("discard", folder, &[]).expect_success();
        assert!(run.stdout.is_empty(), "{label}: {run:?}");
        assert!(run.stderr.contains("nothing to discard"), "{label}: {run:?}");
    });
}

#[test]
fn discard_where_the_store_removed_the_file_removes_it_and_its_emptied_directories() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        location.write("gone/deep/a.toml", "a\n");
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("gone/deep/a.toml"), "mine\n").unwrap();
        location.run(&["store", "delete", "gone/deep/a.toml"]).expect_success();

        // With `sync` not running, the Path is only modified, not yet Diverged.
        let run = run_in("discard", folder.path(), &["--json"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let expected = serde_json::json!({
            "discarded": [{"path": "gone/deep/a.toml", "change": "modified", "revision": null}],
            "events": [],
        });
        assert_eq!(json, expected, "{backend}: {run:?}");
        assert!(!folder.path().join("gone").exists(), "{backend}");

        // A running `sync` finds nothing to do.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
    }
}

#[test]
fn discard_named_paths_relative_to_the_current_directory_removes_only_added_files_named_themselves()
{
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("themes/dark.toml"), "mine\n").unwrap();
        fs::create_dir_all(folder.path().join("themes/new")).unwrap();
        fs::write(folder.path().join("themes/new/light.toml"), "light\n").unwrap();
        fs::write(folder.path().join("keep.toml"), "keep\n").unwrap();

        // A directory means everything under it, but an added file only if named itself.
        let themes = folder.path().join("themes");
        let run = run_in("discard", &themes, &["--json", "."]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let revision = location.run(&["--json", "store", "stat", "themes/dark.toml"]);
        let revision: serde_json::Value = serde_json::from_str(&revision.stdout).unwrap();
        let revision = &revision["revision"];
        let expected = serde_json::json!({
            "discarded": [
                {"path": "themes/dark.toml", "change": "modified", "revision": revision},
            ],
            "events": [],
        });
        assert_eq!(json, expected, "{backend}: {run:?}");
        assert_eq!(fs::read_to_string(themes.join("dark.toml")).unwrap(), "bg = \"black\"\n");
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        let light = themes.join("new/light.toml");
        assert_eq!(fs::read_to_string(&light).unwrap(), "light\n", "{backend}");
        let run = run_in("discard", &themes, &["new"]).expect_success();
        assert!(run.stderr.contains("nothing to discard"), "{backend}: {run:?}");
        assert!(light.exists(), "{backend}");

        let run = run_in("discard", &themes, &["new/light.toml"]).expect_success();
        let removed = "discarded themes/new/light.toml: removed, since the Store has no File \
                       there\n";
        assert_eq!(run.stdout, removed, "{backend}: {run:?}");
        assert!(!themes.join("new").exists(), "{backend}");

        let run = run_in("discard", &themes, &["../keep.toml"]).expect_success();
        let removed = "discarded keep.toml: removed, since the Store has no File there\n";
        assert_eq!(run.stdout, removed, "{backend}: {run:?}");
        assert!(!folder.path().join("keep.toml").exists(), "{backend}");

        // A name matching nothing, or an ignored file, is refused, and nothing is discarded.
        fs::write(folder.path().join(".DS_Store"), "junk").unwrap();
        for name in ["nope.toml", ".DS_Store"] {
            let run = run_in("discard", folder.path(), &["app.toml", name]).expect_code(1);
            assert!(run.stderr.contains(name), "{backend}: {run:?}");
            assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "mine\n");
        }
        let run = run_in("discard", folder.path(), &[".DS_Store"]).expect_code(1);
        assert!(run.stderr.contains("to discard it"), "{backend}: {run:?}");
        run_in("discard", folder.path(), &["/"]).expect_code(1);
        assert!(folder.path().join(".DS_Store").exists(), "{backend}");
    }
}

#[cfg(unix)]
#[test]
fn discard_replaces_what_isnt_a_file_without_following_it_or_leaving_the_folder() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        location.write("gone.toml", "gone\n");
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.toml");
        fs::write(&target, "outside\n").unwrap();
        for path in ["app.toml", "gone.toml"] {
            fs::remove_file(folder.path().join(path)).unwrap();
            std::os::unix::fs::symlink(&target, folder.path().join(path)).unwrap();
        }
        location.run(&["store", "delete", "gone.toml"]).expect_success();
        fs::write(folder.path().join("logo.png"), [0x89, 0x50, 0xff]).unwrap();

        // An invalid file with no Base is left alone unless named itself, as an added one is.
        let run = run_in("discard", folder.path(), &["."]).expect_success();
        let expected = "discarded app.toml: took the Store's version\n\
                        discarded gone.toml: removed, since the Store has no File there\n";
        assert_eq!(run.stdout, expected, "{backend}: {run:?}");
        let app = folder.path().join("app.toml");
        assert!(fs::symlink_metadata(&app).unwrap().is_file(), "{backend}");
        assert_eq!(fs::read_to_string(&app).unwrap(), "a = 1\n", "{backend}");
        assert!(fs::symlink_metadata(folder.path().join("gone.toml")).is_err(), "{backend}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "outside\n", "{backend}");
        assert!(folder.path().join("logo.png").exists(), "{backend}");
        let run = run_in("discard", folder.path(), &["logo.png"]).expect_success();
        assert!(run.stdout.starts_with("discarded logo.png: removed"), "{backend}: {run:?}");
        assert!(!folder.path().join("logo.png").exists(), "{backend}");

        // A symlinked directory on the way keeps the Store's version out, and nothing is
        // discarded.
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        let outside_themes = outside.path().join("themes");
        fs::create_dir(&outside_themes).unwrap();
        fs::write(outside_themes.join("dark.toml"), "outside\n").unwrap();
        fs::remove_dir_all(folder.path().join("themes")).unwrap();
        std::os::unix::fs::symlink(&outside_themes, folder.path().join("themes")).unwrap();
        let run = run_in("discard", folder.path(), &[]).expect_code(1);
        let lines: Vec<&str> = run.stderr.lines().collect();
        assert_eq!(lines[1..], ["  blocked themes/dark.toml: themes is a symlink"], "{run:?}");
        let run = run_in("discard", folder.path(), &["--json"]).expect_code(1);
        let failure: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
        assert_eq!(failure["failure"], "blocked", "{backend}: {run:?}");
        assert_eq!(fs::read_to_string(outside_themes.join("dark.toml")).unwrap(), "outside\n");
        assert_eq!(fs::read_to_string(&app).unwrap(), "mine\n", "{backend}");
    }
}

#[test]
fn resolve_takes_the_version_merged_against_as_the_base_so_the_commit_goes_through() {
    let diverge = |location: &Location, folder: &Path| {
        fs::write(folder.join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
    };
    with_a_diverged_path(diverge, |label, location, folder| {
        fs::write(folder.join("app.toml"), "merged\n").unwrap();
        let run = run_in("resolve", &folder.join("themes"), &["../app.toml"]).expect_success();
        let expected = "resolved app.toml: its Base is now the Store's version it was merged \
                        with\n";
        assert_eq!(run.stdout, expected, "{label}: {run:?}");
        assert!(!theirs(folder, "app.toml").exists(), "{label}");
        assert_eq!(fs::read_to_string(folder.join("app.toml")).unwrap(), "merged\n", "{label}");
        let run = run_in("status", folder, &[]).expect_success();
        assert!(run.stdout.starts_with("modified app.toml\nsync is"), "{label}: {run:?}");

        let run = run_in("commit", folder, &[]).expect_success();
        assert_eq!(run.stdout, "modified app.toml\n", "{label}: {run:?}");
        assert_eq!(location.read("app.toml"), "merged\n", "{label}");
    });
}

#[test]
fn resolve_after_the_store_changed_again_still_conflicts() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs 1\n");
        sync.wait_for("diverged");
        sync.stop();
        // Changed again while `sync` isn't running, so `theirs` still holds the first.
        location.write("app.toml", "theirs 2\n");

        fs::write(folder.path().join("app.toml"), "merged\n").unwrap();
        let run = run_in("resolve", folder.path(), &["--json", "app.toml"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        assert!(common::is_revision(json["resolved"][0]["revision"].as_str().unwrap()), "{json}");
        let run = run_in("commit", folder.path(), &[]).expect_code(3);
        assert!(run.stderr.contains("diverged app.toml"), "{backend}: {run:?}");
        assert_eq!(location.read("app.toml"), "theirs 2\n", "{backend}");
        let local = fs::read_to_string(folder.path().join("app.toml")).unwrap();
        assert_eq!(local, "merged\n", "{backend}");
        let theirs_file = fs::read_to_string(theirs(folder.path(), "app.toml")).unwrap();
        assert_eq!(theirs_file, "theirs 2\n", "{backend}");

        // Nor does a resumed `sync` take the merge for an unchanged file and overwrite it.
        run_in("resolve", folder.path(), &["app.toml"]).expect_success();
        location.write("app.toml", "theirs 3\n");
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        sync.stop();
        let local = fs::read_to_string(folder.path().join("app.toml")).unwrap();
        assert_eq!(local, "merged\n", "{backend}");
    }
}

#[test]
fn resolve_after_the_store_changed_again_takes_the_contents_merged_against_as_the_bases() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs 1\n");
        sync.wait_for("diverged");
        sync.stop();
        // Changed again while `sync` isn't running, so the Store no longer holds what `theirs`
        // does.
        location.write("app.toml", "theirs 2\n");

        // The merge is exactly the Store's version merged against, so nothing is changed since.
        fs::write(folder.path().join("app.toml"), "theirs 1\n").unwrap();
        run_in("resolve", folder.path(), &["app.toml"]).expect_success();
        let run = run_in("status", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "nothing to commit\nsync isn't running\n", "{backend}: {run:?}");
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert!(run.stderr.contains("nothing to commit"), "{backend}: {run:?}");

        // So `sync` takes the Store's newer version.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "updated"), ["app.toml"], "{backend}: {events:?}");
        sync.stop();
        let local = fs::read_to_string(folder.path().join("app.toml")).unwrap();
        assert_eq!(local, "theirs 2\n", "{backend}");
    }
}

#[test]
fn a_merge_made_in_theirs_after_the_store_changed_again_isnt_overwritten() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs 1\n");
        sync.wait_for("diverged");
        sync.stop();
        // Changed again while `sync` isn't running, so the Store no longer holds what `theirs`
        // does.
        location.write("app.toml", "theirs 2\n");

        // The person merges in `theirs` itself, and copies the merge into place.
        let theirs_file = theirs(folder.path(), "app.toml");
        fs::write(&theirs_file, "merged\n").unwrap();
        fs::copy(&theirs_file, folder.path().join("app.toml")).unwrap();
        run_in("resolve", folder.path(), &["app.toml"]).expect_success();
        let run = run_in("status", folder.path(), &[]).expect_success();
        assert!(run.stdout.starts_with("modified app.toml\n"), "{backend}: {run:?}");

        // So `sync` doesn't take the merge for an unchanged file and put the Store's File over it.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        sync.stop();
        assert!(paths(&events, "updated").is_empty(), "{backend}: {events:?}");
        assert_eq!(paths(&events, "diverged"), ["app.toml"], "{backend}: {events:?}");
        let local = fs::read_to_string(folder.path().join("app.toml")).unwrap();
        assert_eq!(local, "merged\n", "{backend}");
    }
}

#[test]
fn resolve_where_the_store_removed_the_file_leaves_no_base() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        location.run(&["store", "delete", "app.toml"]).expect_success();
        sync.wait_for("diverged");
        sync.stop();

        let run = run_in("resolve", folder.path(), &["--json", "app.toml"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let expected = serde_json::json!({
            "resolved": [{"path": "app.toml", "revision": null}],
            "events": [],
        });
        assert_eq!(json, expected, "{backend}: {run:?}");
        let run = run_in("resolve", folder.path(), &[]);
        assert_eq!(run.code, 1, "{backend}: {run:?}");
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "added app.toml\n", "{backend}: {run:?}");
        assert_eq!(location.read("app.toml"), "mine\n", "{backend}");
    }
}

#[test]
fn resolve_refuses_a_path_that_isnt_diverged_and_resolves_nothing() {
    let diverge = |location: &Location, folder: &Path| {
        fs::write(folder.join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
    };
    with_a_diverged_path(diverge, |label, _location, folder| {
        fs::write(folder.join("themes/dark.toml"), "mine\n").unwrap();
        for named in [&["app.toml", "themes/dark.toml"][..], &["app.toml", "."], &["nope.toml"]] {
            let run = run_in("resolve", folder, named).expect_code(1);
            let refused = named.last().unwrap();
            let message = format!("can't resolve what isn't Diverged: {refused}");
            assert!(run.stderr.contains(&message), "{label}: {run:?}");
        }
        let folder_arg = folder.to_str().unwrap();
        let run = run_in("resolve", folder, &["--json", "-C", folder_arg, "themes/dark.toml"]);
        let failure: serde_json::Value = serde_json::from_str(&run.expect_code(1).stderr).unwrap();
        assert_eq!(failure["failure"], "error", "{label}");
        assert!(theirs(folder, "app.toml").exists(), "{label}");
        let run = run_in("status", folder, &[]).expect_success();
        assert!(run.stdout.starts_with("diverged app.toml"), "{label}: {run:?}");
    });
}

#[test]
fn discard_and_resolve_find_the_working_copy_and_check_the_store_flags() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        let elsewhere = TempDir::new().unwrap();
        let other = elsewhere.path().to_str().unwrap();

        for subcommand in ["discard", "resolve"] {
            let run = run_in(subcommand, folder.path(), &["--store", other, "app.toml"]);
            assert!(run.expect_code(1).stderr.contains(other), "{backend} {subcommand}");
            let run = run_in(subcommand, elsewhere.path(), &["app.toml"]).expect_code(1);
            assert!(run.stderr.contains("-C"), "{backend}: {run:?}");
        }
        let folder_arg = folder.path().to_str().unwrap();
        let run = run_in("discard", elsewhere.path(), &["-C", folder_arg]).expect_success();
        assert_eq!(run.stdout, "discarded app.toml: took the Store's version\n", "{backend}");
    }
}

#[cfg(unix)]
#[test]
fn a_theirs_that_cant_be_removed_is_an_error_but_the_path_is_settled_all_the_same() {
    use std::os::unix::fs::PermissionsExt;

    let diverge = |location: &Location, folder: &Path| {
        fs::write(folder.join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
    };
    with_a_diverged_path(diverge, |label, _location, folder| {
        // Nothing can be removed from `.tidings/theirs/`.
        let theirs_directory = folder.join(".tidings/theirs");
        fs::set_permissions(&theirs_directory, fs::Permissions::from_mode(0o555)).unwrap();

        fs::write(folder.join("app.toml"), "merged\n").unwrap();
        let run = run_in("resolve", folder, &["--json", "app.toml"]).expect_success();
        let json: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let events = json["events"].as_array().unwrap();
        assert_eq!(paths(events, "error"), ["app.toml"], "{label}: {run:?}");
        let message = message_for(events, "app.toml");
        assert!(message.starts_with("can't remove .tidings/theirs/app.toml: "), "{label}: {run:?}");
        assert!(theirs(folder, "app.toml").exists(), "{label}");
        let run = run_in("status", folder, &[]).expect_success();
        assert!(run.stdout.starts_with("modified app.toml\n"), "{label}: {run:?}");

        let run = run_in("discard", folder, &[]).expect_success();
        let lines: Vec<&str> = run.stdout.lines().collect();
        assert_eq!(lines[0], "discarded app.toml: took the Store's version", "{label}: {run:?}");
        assert!(lines[1].starts_with("error app.toml: can't remove"), "{label}: {run:?}");
        fs::set_permissions(&theirs_directory, fs::Permissions::from_mode(0o755)).unwrap();
    });
}

#[test]
fn discard_and_resolve_while_sync_runs_leave_it_nothing_to_report() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("app.toml"), "mine\n").unwrap();
        fs::write(folder.path().join("themes/dark.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
        location.write("themes/dark.toml", "theirs\n");
        wait_for_count(&mut sync, "diverged", 2);

        let run = run_in("discard", folder.path(), &["app.toml"]).expect_success();
        assert_eq!(run.stdout, "discarded app.toml: took the Store's version\n", "{backend}");
        fs::write(folder.path().join("themes/dark.toml"), "merged\n").unwrap();
        run_in("resolve", folder.path(), &["themes/dark.toml"]).expect_success();
        assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "theirs\n");
        let dark = fs::read_to_string(folder.path().join("themes/dark.toml")).unwrap();
        assert_eq!(dark, "merged\n", "{backend}");
        assert!(!folder.path().join(".tidings/theirs/app.toml").exists(), "{backend}");
        assert!(!folder.path().join(".tidings/theirs/themes").exists(), "{backend}");
        assert_eq!(location.read("app.toml"), "theirs\n", "{backend}");
        assert_eq!(location.read("themes/dark.toml"), "theirs\n", "{backend}");

        // Committing the merge, then another Commit to the Store: `sync` reports only the new
        // File, neither settled Path, nor the person's own Commit.
        let run = run_in("commit", folder.path(), &[]).expect_success();
        assert_eq!(run.stdout, "modified themes/dark.toml\n", "{backend}: {run:?}");
        assert_eq!(location.read("themes/dark.toml"), "merged\n", "{backend}");
        location.write("other.toml", "other\n");
        let events = wait_for_count(&mut sync, "created", 1);
        let reported = events.iter().filter(|event| event["event"] != "caught-up").count();
        assert_eq!(reported, 1, "{backend}: {events:?}");
        assert_eq!(paths(&events, "created"), ["other.toml"], "{backend}: {events:?}");
        sync.stop();
        let run = run_in("status", folder.path(), &[]).expect_success();
        assert!(run.stdout.starts_with("nothing to commit\n"), "{backend}: {run:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_theirs_that_couldnt_be_removed_is_removed_once_sync_restarts() {
    use std::os::unix::fs::PermissionsExt;

    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        let mut sync = Sync::start(&location, folder.path());
        sync.wait_for("caught-up");
        fs::write(folder.path().join("themes/dark.toml"), "mine\n").unwrap();
        location.write("themes/dark.toml", "theirs\n");
        wait_for_count(&mut sync, "diverged", 1);

        // The Store goes back to the Base while nothing can be removed from where `theirs` is.
        let theirs_themes = folder.path().join(".tidings/theirs/themes");
        fs::set_permissions(&theirs_themes, fs::Permissions::from_mode(0o555)).unwrap();
        location.write("themes/dark.toml", "bg = \"black\"\n");
        let events = sync.wait_for("caught-up");
        assert_eq!(paths(&events, "resolved"), ["themes/dark.toml"], "{backend}: {events:?}");
        assert_eq!(paths(&events, "error"), ["themes/dark.toml"], "{backend}: {events:?}");
        sync.stop();
        fs::set_permissions(&theirs_themes, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(theirs(folder.path(), "themes/dark.toml").exists(), "{backend}");

        // Removed, with the directory it emptied, and nothing reported.
        let mut sync = Sync::start(&location, folder.path());
        let events = sync.wait_for("caught-up");
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        sync.stop();
        assert!(!theirs_themes.exists(), "{backend}");
        assert!(folder.path().join(".tidings/theirs").exists(), "{backend}");
    }
}

#[cfg(unix)]
#[test]
fn discard_resolve_and_a_resync_remove_every_stale_theirs_without_following_symlinks() {
    let diverge = |location: &Location, folder: &Path| {
        fs::write(folder.join("app.toml"), "mine\n").unwrap();
        location.write("app.toml", "theirs\n");
    };
    with_a_diverged_path(diverge, |label, _location, folder| {
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("kept.toml"), "outside\n").unwrap();
        // What a crash, or a `theirs` that couldn't be removed, leaves: files for Paths that
        // aren't Diverged, one of them a symlink out of the folder.
        let plant = || {
            fs::create_dir_all(theirs(folder, "old/deep")).unwrap();
            fs::write(theirs(folder, "old/deep/a.toml"), "stale\n").unwrap();
            let link = theirs(folder, "link");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        };
        plant();
        let run = run_in("discard", folder, &["themes"]).expect_success();
        assert!(run.stderr.contains("nothing to discard"), "{label}: {run:?}");
        for stale in ["old", "link"] {
            assert!(fs::symlink_metadata(theirs(folder, stale)).is_err(), "{label}: {stale}");
        }
        assert!(outside.path().join("kept.toml").exists(), "{label}");
        // A Diverged Path's `theirs` is kept.
        assert!(theirs(folder, "app.toml").exists(), "{label}");

        plant();
        run_in("resolve", folder, &["app.toml"]).expect_success();
        let left: Vec<_> = fs::read_dir(folder.join(".tidings/theirs")).unwrap().collect();
        assert!(left.is_empty(), "{label}: {left:?}");
        assert!(outside.path().join("kept.toml").exists(), "{label}");
    });

    // A Resync reconciles every Path, so removes them too.
    let location = Location::with_store("fs");
    location.write("b.txt", "b");
    let folder = TempDir::new().unwrap();
    let mut sync = Sync::start(&location, folder.path());
    sync.wait_for("caught-up");
    fs::create_dir_all(theirs(folder.path(), "old")).unwrap();
    fs::write(theirs(folder.path(), "old/a.png"), "stale").unwrap();
    remove_the_location(&location);
    sync.wait_for("resync");
    sync.wait_for("caught-up");
    sync.stop();
    assert!(!theirs(folder.path(), "old").exists());
}

#[test]
fn a_record_in_an_unknown_format_or_version_is_refused_and_kept() {
    let location = store_with_config("fs");
    let folder = TempDir::new().unwrap();
    synced(&location, folder.path());
    let record = folder.path().join(".tidings/working-copy");
    let text = fs::read_to_string(&record).unwrap();
    let rest = text.strip_prefix("tidings working-copy 1\n").unwrap_or_else(|| panic!("{text}"));

    fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
    let folder_arg = folder.path().to_str().unwrap();
    // Every command, with what it says of a record in a version it doesn't know, and of one in a
    // format it doesn't know, which it doesn't take for a Working copy's.
    let commands: [(&[&str], &str); 5] = [
        (&["status", "-C", folder_arg], "is not a Working copy"),
        (&["commit", "-C", folder_arg], "is not a Working copy"),
        (&["discard", "-C", folder_arg], "is not a Working copy"),
        (&["resolve", "-C", folder_arg, "app.toml"], "is not a Working copy"),
        (&["sync", folder_arg], "isn't empty"),
    ];
    for first_line in ["tidings working-copy 2", "tidings working-copy-next 1"] {
        let unknown = format!("{first_line}\n{rest}");
        fs::write(&record, &unknown).unwrap();
        for (args, not_a_record) in commands {
            let run = location.run(args).expect_code(1);
            let why = if first_line.ends_with(" 2") {
                "in a format this version doesn't know"
            } else {
                not_a_record
            };
            assert!(run.stderr.contains(why), "{first_line}, {args:?}: {run:?}");
            assert_eq!(fs::read_to_string(&record).unwrap(), unknown, "{args:?}");
        }
    }
    // Nothing was committed, discarded or synced.
    assert_eq!(fs::read_to_string(folder.path().join("app.toml")).unwrap(), "a = 2\n");
    assert_eq!(location.read("app.toml"), "a = 1\n");
}

/// A record from while a Store held Areas has the same first line, but names a Root override or an
/// App identity, and an Area, in place of the Store's Location. Nothing reads it as it was: it is
/// in a format this version doesn't know, and is kept for the person to deal with.
#[test]
fn a_record_from_before_a_store_was_one_location_is_refused_and_kept() {
    let location = store_with_config("fs");
    let folder = TempDir::new().unwrap();
    synced(&location, folder.path());
    let record = folder.path().join(".tidings/working-copy");
    let text = fs::read_to_string(&record).unwrap();
    let store = location.store().to_str().unwrap();
    let old_root = format!("root\t{store}\nbackend\tfs\narea\tconfig\n");
    let old_identity = "identity\tcom.example.app\nbackend\tfs\narea\tconfig\n";
    let new = format!("store\t{store}\nbackend\tfs\n");
    assert!(text.contains(&new), "{text}");

    fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
    let folder_arg = folder.path().to_str().unwrap();
    for old in [old_root.as_str(), old_identity] {
        let old = text.replace(&new, old);
        fs::write(&record, &old).unwrap();
        for args in [
            &["status", "-C", folder_arg][..],
            &["commit", "-C", folder_arg],
            &["sync", folder_arg],
        ] {
            let run = location.run(args).expect_code(1);
            let why = "in a format this version doesn't know";
            assert!(run.stderr.contains(why), "{args:?}: {run:?}");
            assert_eq!(fs::read_to_string(&record).unwrap(), old, "{args:?}");
        }
    }
    assert_eq!(location.read("app.toml"), "a = 1\n");
}

#[test]
fn discard_refuses_a_named_file_whose_name_cant_be_a_path_as_commit_and_resolve_do() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        fs::create_dir(folder.path().join("sub")).unwrap();
        let bad = folder.path().join("sub/bad\\name");
        fs::write(&bad, "mine\n").unwrap();

        for subcommand in ["discard", "commit", "resolve"] {
            let run = run_in(subcommand, folder.path(), &["sub/bad\\name"]).expect_code(1);
            assert!(run.stderr.contains("bad\\name"), "{backend} {subcommand}: {run:?}");
            assert!(bad.exists(), "{backend} {subcommand}");
        }
        let run = run_in("discard", folder.path(), &["sub/bad\\name"]).expect_code(1);
        assert!(run.stderr.contains("isn't a valid Path"), "{backend}: {run:?}");
        let run = run_in("discard", folder.path(), &["--json", "sub/bad\\name"]).expect_code(1);
        let failure: serde_json::Value = serde_json::from_str(&run.stderr).unwrap();
        assert_eq!(failure["failure"], "error", "{backend}: {run:?}");

        // Covered by a named directory, or by no name at all, it is left alone.
        for named in [&["sub"][..], &[]] {
            let run = run_in("discard", folder.path(), named).expect_success();
            assert!(run.stderr.contains("nothing to discard"), "{backend}: {run:?}");
            assert_eq!(fs::read_to_string(&bad).unwrap(), "mine\n", "{backend}");
        }
    }
}

/// `count` Files, `f000` onwards, each holding its number after `phase`, as in `v1-000`, by Path.
fn numbered_files(phase: &str, count: usize) -> BTreeMap<String, String> {
    (0..count).map(|i| (format!("f{i:03}"), format!("{phase}-{i:03}"))).collect()
}

/// Commits `files` to the Store at `location` as one Commit, with a delete of each of
/// `deleted`.
fn commit_all(location: &Location, files: &BTreeMap<String, String>, deleted: &[String]) {
    let mut script = String::from("stage\n");
    for (path, contents) in files {
        script.push_str(&format!("write {path} --contents {contents}\n"));
    }
    for path in deleted {
        script.push_str(&format!("delete {path}\n"));
    }
    script.push_str("commit\n");
    location.run_with_stdin(&["store", "shell"], &script).expect_success();
}

/// Every file in `folder`, outside `.tidings/`, by its name relative to it, with its contents.
fn files_in(folder: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut directories = vec![folder.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let at = entry.unwrap().path();
            if at == folder.join(".tidings") {
                continue;
            }
            if at.is_dir() {
                directories.push(at);
                continue;
            }
            let name = at.strip_prefix(folder).unwrap().to_str().unwrap().replace('\\', "/");
            files.insert(name, fs::read_to_string(&at).unwrap());
        }
    }
    files
}

/// Kills the `sync` of the Store at `location` into `folder` once `started` says it has begun
/// applying what the Store holds, then resumes it, and checks that the resumed one applies the
/// rest, making the folder hold `expected`, and reports nothing but events named `applied`, and
/// that nothing is then left to commit, nor anything in `.tidings/tmp/`.
fn kill_and_resume(
    label: &str,
    location: &Location,
    folder: &Path,
    started: impl Fn() -> bool,
    applied: &[&str],
    expected: &BTreeMap<String, String>,
) {
    let sync = Sync::start(location, folder);
    wait_until(&started);
    sync.kill();

    let events = synced(location, folder);
    for event in &events[..events.len() - 1] {
        assert!(applied.contains(&event["event"].as_str().unwrap()), "{label}: {events:?}");
    }
    assert!(files_in(folder) == *expected, "{label}: the folder doesn't match the Store");
    let tmp = folder.join(".tidings/tmp");
    let left = fs::read_dir(&tmp).map(Iterator::count).unwrap_or(0);
    assert_eq!(left, 0, "{label}: files left in {}", tmp.display());
    commits_nothing(label, folder);
}

#[test]
fn killing_sync_while_it_applies_then_resuming_leaves_the_folder_matching_the_store() {
    const COUNT: usize = 300;
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let first = numbered_files("v1", COUNT);
        commit_all(&location, &first, &[]);
        let folder = TempDir::new().unwrap();
        let wc = folder.path();

        // Killed while it writes the Files into a new Working copy.
        let started = || wc.join("f000").exists();
        kill_and_resume(
            &format!("{backend}, creating"),
            &location,
            wc,
            started,
            &["created"],
            &first,
        );

        // Killed while it updates some Files and removes the others, which it does first.
        let mut second = numbered_files("v2", COUNT);
        let odd: Vec<String> = second.keys().skip(1).step_by(2).cloned().collect();
        second.retain(|path, _| !odd.contains(path));
        commit_all(&location, &second, &odd);
        let started = || fs::read_to_string(wc.join("f000")).is_ok_and(|now| now == "v2-000");
        let applied = ["updated", "removed"];
        kill_and_resume(&format!("{backend}, updating"), &location, wc, started, &applied, &second);
    }
}

/// The command run next on a Working copy that a crash left, in [`heals`].
#[derive(Clone, Copy, Debug)]
enum Next {
    Commit,
    Sync,
}

/// Checks that `next` heals the Working copy that is `folder`, whose folder holds what the Store
/// does though its record says otherwise, as a crash between changing the one and saving the other
/// leaves it: saying nothing to commit, or reporting nothing, and then leaving nothing to commit,
/// so that the next edit commits with no Conflict.
fn heals(label: &str, next: Next, location: &Location, folder: &Path) {
    match next {
        Next::Commit => commits_nothing(label, folder),
        Next::Sync => {
            let events = synced(location, folder);
            assert_eq!(events.len(), 1, "{label}: {events:?}");
        }
    }
    let run = run_in("status", folder, &[]).expect_success();
    assert!(run.stdout.starts_with("nothing to commit\n"), "{label}: {run:?}");
    fs::write(folder.join("app.toml"), "a = 3\n").unwrap();
    run_in("commit", folder, &[]).expect_success();
    assert_eq!(location.read("app.toml"), "a = 3\n", "{label}");
}

#[test]
fn a_commit_whose_record_update_was_lost_is_healed_by_the_next_command() {
    for backend in BACKENDS {
        for (next, further_edit) in
            [(Next::Commit, false), (Next::Sync, false), (Next::Commit, true)]
        {
            let further = if further_edit { " with a further edit" } else { "" };
            let label = format!("{backend}, then {next:?}{further}");
            let location = store_with_config(backend);
            let folder = TempDir::new().unwrap();
            synced(&location, folder.path());
            let record = folder.path().join(".tidings/working-copy");
            let before = fs::read(&record).unwrap();
            fs::write(folder.path().join("app.toml"), "a = 2\n").unwrap();
            fs::write(folder.path().join("new.toml"), "new\n").unwrap();
            fs::remove_file(folder.path().join("themes/dark.toml")).unwrap();
            run_in("commit", folder.path(), &[]).expect_success();
            // As a crash between the Commit and saving the record leaves it.
            fs::write(&record, &before).unwrap();

            if further_edit {
                // Only the further edit is committed, with no Conflict.
                fs::write(folder.path().join("more.toml"), "more\n").unwrap();
                let run = run_in("commit", folder.path(), &[]).expect_success();
                assert_eq!(run.stdout, "added more.toml\n", "{label}: {run:?}");
                assert_eq!(location.read("more.toml"), "more\n", "{label}");
            }
            heals(&label, next, &location, folder.path());
            assert_eq!(location.read("new.toml"), "new\n", "{label}");
            location.run(&["store", "read", "themes/dark.toml"]).expect_code(2);
        }
    }
}

#[test]
fn files_sync_changed_but_didnt_record_are_healed_by_the_next_command() {
    for backend in BACKENDS {
        for next in [Next::Commit, Next::Sync] {
            let label = format!("{backend}, then {next:?}");
            let location = store_with_config(backend);
            let folder = TempDir::new().unwrap();
            synced(&location, folder.path());
            let record = folder.path().join(".tidings/working-copy");
            let before = fs::read(&record).unwrap();
            location.write("app.toml", "a = 2\n");
            location.write("new.toml", "new\n");
            location.run(&["store", "delete", "themes/dark.toml"]).expect_success();
            synced(&location, folder.path());
            // As a crash between changing the folder and saving the record leaves it.
            fs::write(&record, &before).unwrap();

            heals(&label, next, &location, folder.path());
            assert_eq!(fs::read_to_string(folder.path().join("new.toml")).unwrap(), "new\n");
            assert!(!folder.path().join("themes").exists(), "{label}");
        }
    }
}

#[test]
fn leftovers_of_an_interrupted_run_are_removed_by_the_next_sync() {
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        // What a run killed while writing a file, or the record, leaves in `.tidings/`.
        let tidings = folder.path().join(".tidings");
        let leftovers = [tidings.join("tmp/.tmpA1b2C3"), tidings.join(".working-copy.x1Y2z3")];
        fs::create_dir_all(tidings.join("tmp")).unwrap();
        for leftover in &leftovers {
            fs::write(leftover, "a = hal").unwrap();
        }
        // A symlink there is removed, not followed.
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("kept.toml");
        fs::write(&target, "kept\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, tidings.join("tmp/.tmpLink")).unwrap();

        let events = synced(&location, folder.path());
        assert_eq!(events.len(), 1, "{backend}: {events:?}");
        assert_eq!(fs::read_dir(tidings.join("tmp")).unwrap().count(), 0, "{backend}");
        for leftover in &leftovers {
            assert!(!leftover.exists(), "{backend}: {} is left", leftover.display());
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "kept\n", "{backend}");
        commits_nothing(backend, folder.path());
    }
}

#[cfg(unix)]
#[test]
fn sync_never_writes_through_a_symlinked_tmp_directory() {
    use std::os::unix::fs::PermissionsExt;
    for backend in BACKENDS {
        let location = store_with_config(backend);
        let folder = TempDir::new().unwrap();
        synced(&location, folder.path());
        // `.tidings/tmp` leads outside the folder, to a directory nothing may be written in.
        let outside = TempDir::new().unwrap();
        let kept = outside.path().join("kept.toml");
        fs::write(&kept, "kept\n").unwrap();
        fs::set_permissions(outside.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let tmp = folder.path().join(".tidings/tmp");
        let _ = fs::remove_dir_all(&tmp);
        std::os::unix::fs::symlink(outside.path(), &tmp).unwrap();
        location.write("app.toml", "a = 2\n");

        let events = synced(&location, folder.path());
        fs::set_permissions(outside.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(events.len(), 2, "{backend}: {events:?}");
        assert_eq!(events[0]["event"], "updated", "{backend}: {events:?}");
        let app = fs::read_to_string(folder.path().join("app.toml")).unwrap();
        assert_eq!(app, "a = 2\n", "{backend}");
        assert!(fs::symlink_metadata(&tmp).unwrap().is_dir(), "{backend}");
        let names: Vec<_> = fs::read_dir(outside.path()).unwrap().map(|e| e.unwrap()).collect();
        assert_eq!(names.len(), 1, "{backend}: {names:?}");
        assert_eq!(fs::read_to_string(&kept).unwrap(), "kept\n", "{backend}");
    }
}
