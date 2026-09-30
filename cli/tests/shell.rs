//! `tidings store shell` in script mode, reading its commands from stdin, as a script or heredoc
//! does. Interactive mode needs a terminal, so these tests don't cover it.

mod common;

use common::{Location, Run, is_revision, run, tidings};

/// Runs `script` in a shell on the memory Backend.
fn in_memory(script: &str) -> Run {
    let mut command = tidings();
    command.args(["--backend", "memory", "store", "shell"]);
    run(command, script)
}

/// Runs `script` in a shell on the Store at `location`.
fn at(location: &Location, script: &str) -> Run {
    location.run_with_stdin(&["store", "shell"], script)
}

#[test]
fn a_staging_is_committed_all_at_once() {
    let location = Location::with_store("fs");
    let script = "
        stage
        write a.txt --contents one
        write notes/b.txt --contents two
        # Nothing is written until the commit.
        list
        commit
        list
    ";
    let run = at(&location, script).expect_success();
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{run:?}");
    assert!(lines[0].ends_with("  a.txt") && is_revision(&lines[0][..32]), "{run:?}");
    assert!(lines[1].ends_with("  notes/b.txt"), "{run:?}");
    assert_eq!(&lines[2..], ["a.txt", "notes/b.txt"]);
    assert_eq!(location.read("notes/b.txt"), "two");
}

#[test]
fn a_discarded_staging_writes_nothing() {
    let run = in_memory(
        "stage\nwrite a.txt --contents a\ndiscard\nlist\nwrite b.txt --contents b\nlist\n",
    );
    let run = run.expect_success();
    assert!(run.stdout.ends_with("  b.txt\nb.txt\n"), "{run:?}");
}

#[test]
fn only_one_staging_is_open_at_a_time() {
    for script in ["stage\nstage\n", "commit\n", "discard\n", "require a.txt absent\n"] {
        in_memory(script).expect_code(1);
    }
}

#[test]
fn a_script_stops_at_the_first_failure_with_its_exit_code() {
    let run = in_memory("read missing.txt\nwrite a.txt --contents a\n").expect_code(2);
    assert!(run.stderr.contains("line 1"), "{run:?}");

    let location = Location::with_store("sqlite");
    let run = at(&location, "list\nfrobnicate\nwrite a.txt --contents a\n");
    let run = run.expect_code(1);
    assert!(run.stderr.contains("line 2"), "{run:?}");
    location.run(&["store", "read", "a.txt"]).expect_code(2);
}

#[test]
fn a_required_revision_that_is_stale_is_a_conflict() {
    let location = Location::with_store("sqlite");
    location.write("a.txt", "1");
    let stat = location.run(&["--json", "store", "stat", "a.txt"]).expect_success().stdout;
    let stat: serde_json::Value = serde_json::from_str(&stat).unwrap();
    let revision = stat["revision"].as_str().unwrap();

    let script = |revision: &str| {
        format!("stage\nrequire a.txt {revision}\nwrite b.txt --contents b\ncommit\n")
    };
    at(&location, &script(revision)).expect_success();
    location.write("a.txt", "2");
    let run = at(&location, &script(revision)).expect_code(3);
    assert!(run.stderr.contains("a.txt"), "{run:?}");
    at(&location, "stage\nrequire b.txt absent\ncommit\n").expect_code(3);
    at(&location, "stage\nrequire a.txt not-hex\n").expect_code(1);
}

#[test]
fn a_prefix_is_required_unchanged_since_its_last_stat_prefix() {
    let run = in_memory(
        "
        write notes/a.txt --contents a
        stat-prefix notes/
        stage
        require-prefix notes/
        write other.txt --contents unrelated
        commit
        stage
        require-prefix notes/
        discard
        write notes/b.txt --contents new
        stage
        require-prefix notes/
        write other.txt --contents again
        commit
        ",
    );
    let run = run.expect_code(3);
    assert!(run.stderr.contains("notes/b.txt"), "{run:?}");

    let run = in_memory("stage\nrequire-prefix notes/\n").expect_code(1);
    assert!(run.stderr.contains("stat-prefix"), "{run:?}");
}

#[test]
fn read_prints_the_contents_exactly_as_stored() {
    let run = in_memory("write a.txt --contents x\nread a.txt\nlist\n");
    let run = run.expect_success();
    assert!(run.stdout.ends_with("  a.txt\nxa.txt\n"), "{run:?}");
}

#[test]
fn contents_in_the_shell_take_escapes() {
    let run = in_memory(
        r#"write a.txt --contents 'a\nb\tc\\n'
read a.txt
"#,
    );
    let run = run.expect_success();
    assert!(run.stdout.ends_with("a\nb\tc\\n"), "{run:?}");
    in_memory("write a.txt\n").expect_code(1);
}

#[test]
fn changes_are_printed_on_stderr_once_the_feed_is_on() {
    let run = in_memory(
        "write a.txt --contents a\nfeed on\nwrite b.txt --contents b\ndelete b.txt\n\
         feed off\nwrite c.txt --contents c\n",
    );
    let run = run.expect_success();
    assert_eq!(run.stderr, "local changed b.txt\nlocal removed b.txt\n");
}

#[test]
fn json_applies_to_every_command_and_change() {
    let mut command = tidings();
    command.args(["--backend", "memory", "--json", "store", "shell"]);
    let run = run(command, "feed on\nwrite a.txt --contents a\nread a.txt\nlist\n");
    let run = run.expect_success();
    let lines: Vec<serde_json::Value> =
        run.stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert!(lines[0]["revisions"]["a.txt"].is_string(), "{run:?}");
    assert_eq!(lines[1]["contents"], "a");
    assert_eq!(lines[2], serde_json::json!(["a.txt"]));
    let change: serde_json::Value = serde_json::from_str(run.stderr.trim_end()).unwrap();
    assert_eq!(change["path"], "a.txt");
}

#[test]
fn help_and_exit() {
    let run = in_memory("help\nexit\nwrite a.txt --contents a\n").expect_success();
    for command in ["stage", "require-prefix", "commit", "feed", "write", "edit"] {
        assert!(run.stdout.contains(command), "{command}: {run:?}");
    }
    assert!(!run.stdout.contains("watch"), "{run:?}");
    assert!(!run.stdout.contains("a.txt"), "{run:?}");
}

#[test]
fn an_open_staging_left_at_the_end_is_discarded() {
    let location = Location::with_store("fs");
    let run = at(&location, "stage\nwrite a.txt --contents a\n").expect_success();
    assert!(run.stderr.contains("discarded"), "{run:?}");
    location.run(&["store", "read", "a.txt"]).expect_code(2);
}

#[test]
fn the_shell_opens_stores_as_one_shot_commands_do() {
    let location = Location::empty();
    location.run_with_stdin(&["store", "shell"], "list\n").expect_code(1);
    let run = location.run_with_stdin(&["--backend", "fs", "--create", "store", "shell"], "list\n");
    run.expect_success();
    // The memory Backend has no location, and is always new.
    let run = location.run_with_stdin(&["--backend", "memory", "store", "shell"], "list\n");
    run.expect_code(1);
    let mut command = tidings();
    command.args(["--backend", "memory", "--create", "store", "shell"]);
    common::run(command, "list\n").expect_code(1);
}
