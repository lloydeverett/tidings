//! One-shot commands: each run opens the Store, does one thing, and exits.

mod common;

use std::io::{BufRead, BufReader, Lines};
use std::process::{Child, ChildStdout};
use std::time::Duration;

use common::{Location, is_revision, tidings};

const BACKENDS: [&str; 2] = ["fs", "sqlite"];

/// The Revision a write printed, as `<revision>  <path>`.
fn written_revision(stdout: &str) -> String {
    let (revision, _) = stdout.trim_end().split_once("  ").unwrap();
    assert!(is_revision(revision), "{stdout:?}");
    revision.to_owned()
}

#[test]
fn a_location_with_no_store_is_refused_and_left_alone_unless_created() {
    let location = Location::empty();
    let run = location.run(&["list", "data"]).expect_code(1);
    assert!(run.stderr.contains("--create"), "{run:?}");
    assert_eq!(std::fs::read_dir(location.root()).unwrap().count(), 0);

    let run = location.run(&["--create", "list", "data"]).expect_code(1);
    assert!(run.stderr.contains("--backend"), "{run:?}");

    location.run(&["--create", "--backend", "sqlite", "list", "data"]).expect_success();
    // From now on the Backend is found from the location.
    location.write("data", "a.txt", "a");
    assert_eq!(location.read("data", "a.txt"), "a");
    location.run(&["--backend", "fs", "list", "data"]).expect_code(1);
}

#[test]
fn a_store_on_another_backend_is_refused() {
    let location = Location::with_store("fs");
    let run = location.run(&["--backend", "sqlite", "list", "data"]).expect_code(1);
    assert!(run.stderr.contains("fs"), "{run:?}");
    location.run(&["--backend", "fs", "list", "data"]).expect_success();
}

#[test]
fn a_location_is_given_by_exactly_one_of_root_and_identity() {
    let run = tidings().args(["list", "data"]).output().unwrap();
    assert_eq!(run.status.code(), Some(1));

    let location = Location::with_store("fs");
    let run = location.run(&["--identity", "com.example.app", "list", "data"]).expect_code(1);
    assert!(run.stderr.contains("--identity"), "{run:?}");
}

#[test]
fn an_identity_has_three_non_empty_parts() {
    for identity in ["example.app", "com..app", "com.example.app.more", ""] {
        let run = tidings().args(["--identity", identity, "list", "data"]).output().unwrap();
        assert_eq!(run.status.code(), Some(1), "{identity:?}");
        let stderr = String::from_utf8(run.stderr).unwrap();
        assert!(stderr.contains("TLD.AUTHOR.APP"), "{identity:?}: {stderr}");
    }
}

#[test]
fn the_location_and_backend_can_come_from_the_environment() {
    let location = Location::empty();
    let mut command = tidings();
    command.env("TIDINGS_ROOT", location.root()).env("TIDINGS_BACKEND", "fs");
    command.args(["--create", "write", "data", "a.txt", "--contents", "a"]);
    assert_eq!(command.output().unwrap().status.code(), Some(0));
    assert_eq!(location.read("data", "a.txt"), "a");
}

#[test]
fn the_memory_backend_is_only_for_the_shell() {
    let run = tidings().args(["--backend", "memory", "list", "data"]).output().unwrap();
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8(run.stderr).unwrap().contains("shell"));
}

#[test]
fn contents_are_written_and_read_back_exactly() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let contents = "first line\n  second, with no newline after";
        location.run_with_stdin(&["write", "data", "notes/a.txt"], contents).expect_success();
        assert_eq!(location.read("data", "notes/a.txt"), contents);

        location.write("config", "b.toml", "b = 1\n");
        assert_eq!(location.read("config", "b.toml"), "b = 1\n");

        let from = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(from.path(), "from a file").unwrap();
        let from = from.path().to_str().unwrap();
        location.run(&["write", "cache", "c.txt", "--from", from]).expect_success();
        assert_eq!(location.read("cache", "c.txt"), "from a file");
    }
}

#[test]
fn contents_come_from_one_place_only() {
    let location = Location::with_store("fs");
    let from = tempfile::NamedTempFile::new().unwrap();
    let args =
        ["write", "data", "a.txt", "--contents", "a", "--from", from.path().to_str().unwrap()];
    location.run(&args).expect_code(1);
    location.run(&["read", "data", "a.txt"]).expect_code(2);
}

#[test]
fn writing_prints_the_new_revision_which_stat_and_read_give_too() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let run = location.run(&["write", "data", "a.txt", "--contents", "a"]).expect_success();
        assert!(run.stdout.ends_with("  a.txt\n"), "{run:?}");
        let revision = written_revision(&run.stdout);

        let stat = location.run(&["stat", "data", "a.txt"]).expect_success().stdout;
        assert!(stat.contains(&format!("revision {revision}\n")), "{stat}");
        assert!(stat.contains("modified "), "{stat}");

        let json = location.run(&["--json", "stat", "data", "a.txt"]).expect_success().stdout;
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["path"], "a.txt");
        assert_eq!(json["revision"], revision.as_str());
        assert!(json["modified"].is_string());

        let json = location.run(&["--json", "read", "data", "a.txt"]).expect_success().stdout;
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["contents"], "a");
        assert_eq!(json["revision"], revision.as_str());
    }
}

#[test]
fn a_missing_file_exits_with_2() {
    let location = Location::with_store("fs");
    for command in ["read", "stat"] {
        let run = location.run(&[command, "data", "missing.txt"]).expect_code(2);
        assert_eq!(run.stdout, "");
        assert!(run.stderr.contains("missing.txt"), "{run:?}");
    }
}

#[test]
fn preconditions_that_fail_exit_with_3_and_write_nothing() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let run = location.run(&["write", "data", "a.txt", "--contents", "1", "--if-absent"]);
        let first = written_revision(&run.expect_success().stdout);

        let run = location.run(&["write", "data", "a.txt", "--contents", "2", "--if-absent"]);
        assert!(run.expect_code(3).stderr.contains("a.txt"));

        let args = ["write", "data", "a.txt", "--contents", "2", "--if-revision", &first];
        location.run(&args).expect_success();
        // `first` is stale now.
        let args = ["write", "data", "a.txt", "--contents", "3", "--if-revision", &first];
        location.run(&args).expect_code(3);
        location.run(&["delete", "data", "a.txt", "--if-revision", &first]).expect_code(3);
        assert_eq!(location.read("data", "a.txt"), "2");

        let args = ["write", "data", "a.txt", "--contents", "3", "--if-revision", "not-hex"];
        location.run(&args).expect_code(1);
        let args = ["write", "data", "a.txt", "--contents", "3", "--if-absent", "--if-revision"];
        location.run(&[args.as_slice(), &[&first]].concat()).expect_code(1);
    }
}

#[test]
fn listing_gives_the_paths_under_a_prefix_in_order() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        for path in ["notes/b.txt", "notes/a.txt", "other.txt"] {
            location.write("data", path, "x");
        }
        let all = location.run(&["list", "data"]).expect_success().stdout;
        assert_eq!(all, "notes/a.txt\nnotes/b.txt\nother.txt\n");
        let notes = location.run(&["list", "data", "notes/"]).expect_success().stdout;
        assert_eq!(notes, "notes/a.txt\nnotes/b.txt\n");
        let json = location.run(&["--json", "list", "data", "notes/"]).expect_success().stdout;
        assert_eq!(serde_json::from_str::<serde_json::Value>(&json).unwrap()[1], "notes/b.txt");
        assert_eq!(location.run(&["list", "config"]).expect_success().stdout, "");
    }
}

#[test]
fn a_prefix_revision_changes_when_a_file_under_it_does() {
    let location = Location::with_store("sqlite");
    location.write("data", "notes/a.txt", "a");
    let stat_prefix = || location.run(&["stat-prefix", "data", "notes/"]).expect_success().stdout;
    let before = stat_prefix();
    assert!(is_revision(before.trim_end()), "{before:?}");
    assert_eq!(stat_prefix(), before);
    location.write("data", "notes/a.txt", "changed");
    assert_ne!(stat_prefix(), before);
}

#[test]
fn deleting_removes_files_and_prefixes() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        for path in ["notes/a.txt", "notes/b.txt", "c.txt"] {
            location.write("data", path, "x");
        }
        location.run(&["delete", "data", "c.txt"]).expect_success();
        location.run(&["read", "data", "c.txt"]).expect_code(2);
        location.run(&["delete-prefix", "data", "notes/"]).expect_success();
        assert_eq!(location.run(&["list", "data"]).expect_success().stdout, "");
    }
}

#[test]
fn an_invalid_path_is_an_error() {
    let location = Location::with_store("fs");
    let run = location.run(&["write", "data", "CON", "--contents", "x"]).expect_code(1);
    assert!(run.stderr.contains("CON"), "{run:?}");
    location.run(&["list", "data", "notes"]).expect_code(1);
}

#[test]
fn shell_commands_are_refused_as_one_shot_commands() {
    let location = Location::with_store("fs");
    for command in ["stage", "commit", "discard", "require", "exit"] {
        location.run(&[command]).expect_code(1);
    }
}

/// `tidings watch` running, with its lines as they come.
struct Watch {
    process: Child,
    lines: Lines<BufReader<ChildStdout>>,
}

impl Watch {
    /// Starts `tidings <args>`, and waits until it says on stderr that it is watching: a Change
    /// made before then could be missed.
    fn start(location: &Location, args: &[&str]) -> Watch {
        let mut process = common::spawn(&mut location.command(args));
        let lines = BufReader::new(process.stdout.take().unwrap()).lines();
        let mut stderr = BufReader::new(process.stderr.take().unwrap()).lines();
        assert!(stderr.next().unwrap().unwrap().contains("watching"));
        Watch { process, lines }
    }

    /// The next line it printed.
    fn next_line(&mut self) -> String {
        self.lines.next().unwrap().unwrap()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn watch_prints_the_changes_to_the_areas_named() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let mut watch = Watch::start(&location, &["watch", "data"]);
        location.write("config", "ignored.toml", "x");
        std::thread::sleep(Duration::from_millis(300));
        location.write("data", "a.txt", "a");
        assert_eq!(watch.next_line(), "external changed data a.txt");
        location.run(&["delete", "data", "a.txt"]).expect_success();
        assert_eq!(watch.next_line(), "external removed data a.txt");
    }
}

#[test]
fn watch_prints_json_lines() {
    let location = Location::with_store("sqlite");
    let mut watch = Watch::start(&location, &["--json", "watch"]);
    location.write("cache", "a.txt", "a");
    let line: serde_json::Value = serde_json::from_str(&watch.next_line()).unwrap();
    assert_eq!(
        line,
        serde_json::json!({"origin": "external", "kind": "changed", "area": "cache", "path": "a.txt"})
    );
}
