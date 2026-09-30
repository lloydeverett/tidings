//! One-shot commands: each run opens the Store, does one thing, and exits.

mod common;

use std::io::{BufRead, BufReader, Lines};
use std::process::{Child, ChildStdout};

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
    let run = location.run(&["store", "list"]).expect_code(1);
    assert!(run.stderr.contains("--create"), "{run:?}");
    assert_eq!(std::fs::read_dir(location.store()).unwrap().count(), 0);

    let run = location.run(&["--create", "store", "list"]).expect_code(1);
    assert!(run.stderr.contains("--backend"), "{run:?}");

    location.run(&["--create", "--backend", "sqlite", "store", "list"]).expect_success();
    // From now on the Backend is found from the location.
    location.write("a.txt", "a");
    assert_eq!(location.read("a.txt"), "a");
    location.run(&["--backend", "fs", "store", "list"]).expect_code(1);
}

#[test]
fn a_store_on_another_backend_is_refused() {
    let location = Location::with_store("fs");
    let run = location.run(&["--backend", "sqlite", "store", "list"]).expect_code(1);
    assert!(run.stderr.contains("fs"), "{run:?}");
    location.run(&["--backend", "fs", "store", "list"]).expect_success();
}

#[test]
fn the_store_is_named_by_its_location() {
    let run = tidings().args(["store", "list"]).output().unwrap();
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8(run.stderr).unwrap().contains("--store"));
}

/// A Store is where the app opens it, so the flags that named one from an App identity, or as a
/// directory holding Areas, are gone rather than kept with another meaning.
#[test]
fn identity_and_root_are_not_accepted() {
    let location = Location::with_store("fs");
    for flags in
        [&["--identity", "com.example.app"], &["--root", location.store().to_str().unwrap()]]
    {
        let run = tidings().args(flags).args(["store", "list"]).output().unwrap();
        assert_eq!(run.status.code(), Some(1), "{flags:?}");
        let stderr = String::from_utf8(run.stderr).unwrap();
        assert!(stderr.contains(flags[0]), "{flags:?}: {stderr}");
    }
}

#[test]
fn the_location_and_backend_can_come_from_the_environment() {
    let location = Location::empty();
    let mut command = tidings();
    command.env("TIDINGS_STORE", location.store()).env("TIDINGS_BACKEND", "fs");
    command.args(["--create", "store", "write", "a.txt", "--contents", "a"]);
    assert_eq!(command.output().unwrap().status.code(), Some(0));
    assert_eq!(location.read("a.txt"), "a");
}

#[test]
fn the_memory_backend_is_only_for_the_shell() {
    let run = tidings().args(["--backend", "memory", "store", "list"]).output().unwrap();
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8(run.stderr).unwrap().contains("shell"));
}

#[test]
fn contents_are_written_and_read_back_exactly() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let contents = "first line\n  second, with no newline after";
        location.run_with_stdin(&["store", "write", "notes/a.txt"], contents).expect_success();
        assert_eq!(location.read("notes/a.txt"), contents);

        location.write("b.toml", "b = 1\n");
        assert_eq!(location.read("b.toml"), "b = 1\n");

        let from = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(from.path(), "from a file").unwrap();
        let from = from.path().to_str().unwrap();
        location.run(&["store", "write", "c.txt", "--from", from]).expect_success();
        assert_eq!(location.read("c.txt"), "from a file");
    }
}

#[test]
fn contents_come_from_one_place_only() {
    let location = Location::with_store("fs");
    let from = tempfile::NamedTempFile::new().unwrap();
    let args =
        ["store", "write", "a.txt", "--contents", "a", "--from", from.path().to_str().unwrap()];
    location.run(&args).expect_code(1);
    location.run(&["store", "read", "a.txt"]).expect_code(2);
}

#[test]
fn writing_prints_the_new_revision_which_stat_and_read_give_too() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let run = location.run(&["store", "write", "a.txt", "--contents", "a"]).expect_success();
        assert!(run.stdout.ends_with("  a.txt\n"), "{run:?}");
        let revision = written_revision(&run.stdout);

        let stat = location.run(&["store", "stat", "a.txt"]).expect_success().stdout;
        assert!(stat.contains(&format!("revision {revision}\n")), "{stat}");
        assert!(stat.contains("modified "), "{stat}");

        let json = location.run(&["--json", "store", "stat", "a.txt"]).expect_success().stdout;
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["path"], "a.txt");
        assert_eq!(json["revision"], revision.as_str());
        assert!(json["modified"].is_string());

        let json = location.run(&["--json", "store", "read", "a.txt"]).expect_success().stdout;
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["contents"], "a");
        assert_eq!(json["revision"], revision.as_str());
    }
}

#[test]
fn a_missing_file_exits_with_2() {
    let location = Location::with_store("fs");
    for command in ["read", "stat"] {
        let run = location.run(&["store", command, "missing.txt"]).expect_code(2);
        assert_eq!(run.stdout, "");
        assert!(run.stderr.contains("missing.txt"), "{run:?}");
    }
}

#[test]
fn preconditions_that_fail_exit_with_3_and_write_nothing() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let run = location.run(&["store", "write", "a.txt", "--contents", "1", "--if-absent"]);
        let first = written_revision(&run.expect_success().stdout);

        let run = location.run(&["store", "write", "a.txt", "--contents", "2", "--if-absent"]);
        assert!(run.expect_code(3).stderr.contains("a.txt"));

        let args = ["store", "write", "a.txt", "--contents", "2", "--if-revision", &first];
        location.run(&args).expect_success();
        // `first` is stale now.
        let args = ["store", "write", "a.txt", "--contents", "3", "--if-revision", &first];
        location.run(&args).expect_code(3);
        location.run(&["store", "delete", "a.txt", "--if-revision", &first]).expect_code(3);
        assert_eq!(location.read("a.txt"), "2");

        let args = ["store", "write", "a.txt", "--contents", "3", "--if-revision", "not-hex"];
        location.run(&args).expect_code(1);
        let args = ["store", "write", "a.txt", "--contents", "3", "--if-absent", "--if-revision"];
        location.run(&[args.as_slice(), &[&first]].concat()).expect_code(1);
    }
}

#[test]
fn with_json_a_failure_is_one_json_object_on_stderr() {
    let location = Location::with_store("fs");
    location.write("a.txt", "a");
    let failure = |args: &[&str], code| {
        let run = location.run(&[&["--json", "store"], args].concat()).expect_code(code);
        assert_eq!(run.stdout, "", "{run:?}");
        assert_eq!(run.stderr.lines().count(), 1, "{run:?}");
        serde_json::from_str::<serde_json::Value>(&run.stderr).unwrap()
    };

    let args = ["write", "a.txt", "--contents", "b", "--if-absent"];
    let message = "Conflict: a Precondition did not hold for a.txt";
    let expected = serde_json::json!({"failure": "conflict", "message": message});
    assert_eq!(failure(&args, 3), expected);
    let expected = serde_json::json!({"failure": "missing", "message": "no File at b.txt"});
    assert_eq!(failure(&["read", "b.txt"], 2), expected);
    let message = "invalid path \"a.txt/b\": it would be under another File, or have other Files \
                   under it";
    let expected = serde_json::json!({"failure": "error", "message": message});
    assert_eq!(failure(&["write", "a.txt/b", "--contents", "b"], 1), expected);
}

#[test]
fn listing_gives_the_paths_under_a_prefix_in_order() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        for path in ["notes/b.txt", "notes/a.txt", "other.txt"] {
            location.write(path, "x");
        }
        let all = location.run(&["store", "list"]).expect_success().stdout;
        assert_eq!(all, "notes/a.txt\nnotes/b.txt\nother.txt\n");
        let notes = location.run(&["store", "list", "notes/"]).expect_success().stdout;
        assert_eq!(notes, "notes/a.txt\nnotes/b.txt\n");
        let json = location.run(&["--json", "store", "list", "notes/"]).expect_success().stdout;
        assert_eq!(serde_json::from_str::<serde_json::Value>(&json).unwrap()[1], "notes/b.txt");
    }
}

#[test]
fn a_prefix_revision_changes_when_a_file_under_it_does() {
    let location = Location::with_store("sqlite");
    location.write("notes/a.txt", "a");
    let stat_prefix = || location.run(&["store", "stat-prefix", "notes/"]).expect_success().stdout;
    let before = stat_prefix();
    assert!(is_revision(before.trim_end()), "{before:?}");
    assert_eq!(stat_prefix(), before);
    location.write("notes/a.txt", "changed");
    assert_ne!(stat_prefix(), before);
}

#[test]
fn deleting_removes_files_and_prefixes() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        for path in ["notes/a.txt", "notes/b.txt", "c.txt"] {
            location.write(path, "x");
        }
        location.run(&["store", "delete", "c.txt"]).expect_success();
        location.run(&["store", "read", "c.txt"]).expect_code(2);
        location.run(&["store", "delete-prefix", "notes/"]).expect_success();
        assert_eq!(location.run(&["store", "list"]).expect_success().stdout, "");
    }
}

#[test]
fn an_invalid_path_is_an_error() {
    let location = Location::with_store("fs");
    let run = location.run(&["store", "write", "CON", "--contents", "x"]).expect_code(1);
    assert!(run.stderr.contains("CON"), "{run:?}");
    location.run(&["store", "list", "notes"]).expect_code(1);
}

#[test]
fn shell_commands_are_refused_as_one_shot_commands() {
    let location = Location::with_store("fs");
    for command in ["stage", "commit", "discard", "require", "exit"] {
        location.run(&["store", command]).expect_code(1);
    }
}

/// Every command under `tidings store`.
const STORE_COMMANDS: [&str; 10] = [
    "read",
    "stat",
    "list",
    "stat-prefix",
    "write",
    "delete",
    "delete-prefix",
    "edit",
    "watch",
    "shell",
];

#[test]
fn the_store_commands_are_only_under_store() {
    let location = Location::with_store("fs");
    location.write("a.txt", "a");
    for command in STORE_COMMANDS {
        let run = location.run(&[command, "a.txt"]).expect_code(1);
        assert!(run.stderr.contains(command), "{command}: {run:?}");
    }
    assert_eq!(location.read("a.txt"), "a");
}

#[test]
fn help_lists_the_commands_where_they_live() {
    let top = tidings().arg("--help").output().unwrap();
    assert!(top.status.success());
    let top = String::from_utf8(top.stdout).unwrap();
    assert!(top.contains("store"), "{top}");
    for command in STORE_COMMANDS {
        let listed = top.lines().any(|line| line.split_whitespace().next() == Some(command));
        assert!(!listed, "{command} is listed at the top level: {top}");
    }

    let store = tidings().args(["store", "--help"]).output().unwrap();
    assert!(store.status.success());
    let store = String::from_utf8(store.stdout).unwrap();
    for command in STORE_COMMANDS {
        let listed = store.lines().any(|line| line.split_whitespace().next() == Some(command));
        assert!(listed, "{command} isn't listed under store: {store}");
    }
}

/// `tidings store watch` running, with its lines as they come.
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
fn watch_prints_the_changes_to_the_store() {
    for backend in BACKENDS {
        let location = Location::with_store(backend);
        let mut watch = Watch::start(&location, &["store", "watch"]);
        location.write("a.txt", "a");
        assert_eq!(watch.next_line(), "external changed a.txt");
        location.run(&["store", "delete", "a.txt"]).expect_success();
        assert_eq!(watch.next_line(), "external removed a.txt");
    }
}

#[test]
fn watch_prints_json_lines() {
    let location = Location::with_store("sqlite");
    let mut watch = Watch::start(&location, &["--json", "store", "watch"]);
    location.write("a.txt", "a");
    let line: serde_json::Value = serde_json::from_str(&watch.next_line()).unwrap();
    assert_eq!(line, serde_json::json!({"origin": "external", "kind": "changed", "path": "a.txt"}));
}
