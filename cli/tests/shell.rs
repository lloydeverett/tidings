//! `tidings shell` in script mode, reading its commands from stdin, as a script or heredoc does.
//! Interactive mode needs a terminal, so these tests don't cover it.

mod common;

use common::{Location, Run, is_revision, run, tidings};

/// Runs `script` in a shell on the memory Backend.
fn in_memory(script: &str) -> Run {
    let mut command = tidings();
    command.args(["--backend", "memory", "shell"]);
    run(command, script)
}

/// Runs `script` in a shell on the Store at `location`.
fn at(location: &Location, script: &str) -> Run {
    location.run_with_stdin(&["shell"], script)
}

#[test]
fn a_staging_is_committed_all_at_once() {
    let location = Location::with_store("fs");
    let script = "
        stage data
        write data a.txt --contents one
        write data notes/b.txt --contents two
        # Nothing is written until the commit.
        list data
        commit
        list data
    ";
    let run = at(&location, script).expect_success();
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{run:?}");
    assert!(lines[0].ends_with("  a.txt") && is_revision(&lines[0][..32]), "{run:?}");
    assert!(lines[1].ends_with("  notes/b.txt"), "{run:?}");
    assert_eq!(&lines[2..], ["a.txt", "notes/b.txt"]);
    assert_eq!(location.read("data", "notes/b.txt"), "two");
}

#[test]
fn a_discarded_staging_writes_nothing() {
    let run = in_memory(
        "stage data\nwrite data a.txt --contents a\ndiscard\nlist data\nwrite data b.txt --contents b\nlist data\n",
    );
    let run = run.expect_success();
    assert!(run.stdout.ends_with("  b.txt\nb.txt\n"), "{run:?}");
}

#[test]
fn only_one_staging_is_open_and_only_for_its_area() {
    for script in [
        "stage data\nstage data\n",
        "stage data\nwrite config a --contents a\n",
        "commit\n",
        "discard\n",
        "require a.txt absent\n",
    ] {
        in_memory(script).expect_code(1);
    }
}

#[test]
fn a_script_stops_at_the_first_failure_with_its_exit_code() {
    let run = in_memory("read data missing.txt\nwrite data a.txt --contents a\n").expect_code(2);
    assert!(run.stderr.contains("line 1"), "{run:?}");

    let location = Location::with_store("sqlite");
    let run = at(&location, "list data\nfrobnicate\nwrite data a.txt --contents a\n");
    let run = run.expect_code(1);
    assert!(run.stderr.contains("line 2"), "{run:?}");
    location.run(&["read", "data", "a.txt"]).expect_code(2);
}

#[test]
fn a_required_revision_that_is_stale_is_a_conflict() {
    let location = Location::with_store("sqlite");
    location.write("data", "a.txt", "1");
    let stat = location.run(&["--json", "stat", "data", "a.txt"]).expect_success().stdout;
    let stat: serde_json::Value = serde_json::from_str(&stat).unwrap();
    let revision = stat["revision"].as_str().unwrap();

    let script = |revision: &str| {
        format!("stage data\nrequire a.txt {revision}\nwrite data b.txt --contents b\ncommit\n")
    };
    at(&location, &script(revision)).expect_success();
    location.write("data", "a.txt", "2");
    let run = at(&location, &script(revision)).expect_code(3);
    assert!(run.stderr.contains("a.txt"), "{run:?}");
    at(&location, "stage data\nrequire b.txt absent\ncommit\n").expect_code(3);
    at(&location, "stage data\nrequire a.txt not-hex\n").expect_code(1);
}

#[test]
fn a_prefix_is_required_unchanged_since_its_last_stat_prefix() {
    let run = in_memory(
        "
        write data notes/a.txt --contents a
        stat-prefix data notes/
        stage data
        require-prefix notes/
        write data other.txt --contents unrelated
        commit
        stage data
        require-prefix notes/
        discard
        write data notes/b.txt --contents new
        stage data
        require-prefix notes/
        write data other.txt --contents again
        commit
        ",
    );
    let run = run.expect_code(3);
    assert!(run.stderr.contains("notes/b.txt"), "{run:?}");

    let run = in_memory("stage data\nrequire-prefix notes/\n").expect_code(1);
    assert!(run.stderr.contains("stat-prefix"), "{run:?}");
}

#[test]
fn contents_in_the_shell_take_escapes() {
    let run = in_memory(
        r#"write data a.txt --contents 'a\nb\tc\\n'
read data a.txt
"#,
    );
    let run = run.expect_success();
    assert!(run.stdout.ends_with("a\nb\tc\\n\n"), "{run:?}");
    in_memory("write data a.txt\n").expect_code(1);
}

#[test]
fn changes_are_printed_on_stderr_once_the_feed_is_on() {
    let run = in_memory(
        "write data a.txt --contents a\nfeed on\nwrite data b.txt --contents b\ndelete data b.txt\nfeed off\nwrite data c.txt --contents c\n",
    );
    let run = run.expect_success();
    assert_eq!(run.stderr, "local changed data b.txt\nlocal removed data b.txt\n");
}

#[test]
fn json_applies_to_every_command_and_change() {
    let mut command = tidings();
    command.args(["--backend", "memory", "--json", "shell"]);
    let run = run(command, "feed on\nwrite data a.txt --contents a\nread data a.txt\nlist data\n");
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
    let run = in_memory("help\nexit\nwrite data a.txt --contents a\n").expect_success();
    for command in ["stage", "require-prefix", "commit", "feed", "write", "edit"] {
        assert!(run.stdout.contains(command), "{command}: {run:?}");
    }
    assert!(!run.stdout.contains("watch"), "{run:?}");
    assert!(!run.stdout.contains("a.txt"), "{run:?}");
}

#[test]
fn an_open_staging_left_at_the_end_is_discarded() {
    let location = Location::with_store("fs");
    let run = at(&location, "stage data\nwrite data a.txt --contents a\n").expect_success();
    assert!(run.stderr.contains("discarded"), "{run:?}");
    location.run(&["read", "data", "a.txt"]).expect_code(2);
}

#[test]
fn the_shell_opens_stores_as_one_shot_commands_do() {
    let location = Location::empty();
    location.run_with_stdin(&["shell"], "list data\n").expect_code(1);
    let run = location.run_with_stdin(&["--backend", "fs", "--create", "shell"], "list data\n");
    run.expect_success();
    let run = location.run_with_stdin(&["--backend", "memory", "shell"], "list data\n");
    run.expect_code(1);
}
