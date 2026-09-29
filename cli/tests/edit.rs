//! `edit`, with an "editor" that is a shell command changing the file it's given.

mod common;

use std::process::Command;

use common::{Location, Run, run};

/// `tidings --root <root> [shell args] edit ...`, with `$VISUAL` set to `sh -c '<script>'`, which
/// is given the file to edit as `$0`, and `$TIDINGS` to this binary.
fn editing(location: &Location, args: &[&str], script: &str, stdin: &str) -> Run {
    let mut command: Command = location.command(args);
    command.env("VISUAL", format!("sh -c '{script}'"));
    command.env("TIDINGS", common::binary());
    command.env("ROOT", location.root());
    run(command, stdin)
}

/// An editor script that saves `text`, then has someone else write `theirs` to data `a.txt`
/// before the edit is written back.
fn saving_while_someone_writes(text: &str) -> String {
    let write = r#""$TIDINGS" --root "$ROOT" store write data a.txt --contents theirs"#;
    format!(r#"printf {text} > "$0" && {write}"#)
}

/// The file a failure's message says the edited text is kept in, which is the last word of it.
/// Removes the file, and gives what it held.
#[track_caller]
fn kept_text(run: &Run) -> String {
    let kept = run.stderr.trim_end().rsplit(' ').next().unwrap();
    let text = std::fs::read_to_string(kept).unwrap_or_else(|error| panic!("{error}: {run:?}"));
    std::fs::remove_file(kept).unwrap();
    text
}

#[test]
fn an_edited_file_is_written_back() {
    let location = Location::with_store("fs");
    location.write("config", "app.toml", "a = 1\n");
    let run =
        editing(&location, &["store", "edit", "config", "app.toml"], r#"echo "b = 2" >> "$0""#, "");
    run.expect_success();
    assert_eq!(location.read("config", "app.toml"), "a = 1\nb = 2\n");
}

#[test]
fn a_missing_file_starts_empty_and_is_created() {
    let location = Location::with_store("sqlite");
    let script = r#"test ! -s "$0" && printf new > "$0""#;
    editing(&location, &["store", "edit", "data", "notes/new.txt"], script, "").expect_success();
    assert_eq!(location.read("data", "notes/new.txt"), "new");
}

#[test]
fn an_unchanged_buffer_commits_nothing() {
    let location = Location::with_store("fs");
    let run = editing(&location, &["store", "edit", "data", "a.txt"], "true", "").expect_success();
    assert!(run.stderr.contains("unchanged"), "{run:?}");
    location.run(&["store", "read", "data", "a.txt"]).expect_code(2);
}

#[test]
fn a_file_changed_while_it_was_edited_is_a_conflict_that_keeps_the_edit() {
    let location = Location::with_store("fs");
    location.write("data", "a.txt", "original");
    let script = saving_while_someone_writes("mine");
    let run = editing(&location, &["store", "edit", "data", "a.txt"], &script, "").expect_code(3);
    assert_eq!(location.read("data", "a.txt"), "theirs");
    assert_eq!(kept_text(&run), "mine");
}

#[test]
fn a_file_created_while_it_was_edited_is_a_conflict() {
    let location = Location::with_store("sqlite");
    let script = saving_while_someone_writes("mine");
    let run = editing(&location, &["store", "edit", "data", "a.txt"], &script, "").expect_code(3);
    assert_eq!(kept_text(&run), "mine");
}

#[test]
fn quitting_the_editor_with_a_failure_cancels_the_edit() {
    let location = Location::with_store("fs");
    let script = r#"printf changed > "$0"; exit 1"#;
    let run = editing(&location, &["store", "edit", "data", "a.txt"], script, "").expect_success();
    assert!(run.stderr.contains("cancelled"), "{run:?}");
    location.run(&["store", "read", "data", "a.txt"]).expect_code(2);

    // In the shell, nothing is staged, and a script carries on.
    let shell = "stage data\nedit data a.txt\ncommit\nlist data\n";
    let run = editing(&location, &["store", "shell"], script, shell).expect_success();
    assert!(run.stdout.is_empty(), "{run:?}");
}

#[test]
fn with_no_editor_set_edit_fails() {
    let location = Location::with_store("fs");
    let run = location.run(&["store", "edit", "data", "a.txt"]).expect_code(1);
    assert!(run.stderr.contains("EDITOR"), "{run:?}");
}

#[test]
fn in_a_staging_the_edit_is_staged_with_its_precondition() {
    let location = Location::with_store("fs");
    location.write("data", "a.txt", "original");
    let shell = "stage data\nedit data a.txt\nlist data\ncommit\n";
    let run =
        editing(&location, &["store", "shell"], r#"printf mine > "$0""#, shell).expect_success();
    assert!(run.stdout.starts_with("a.txt\n"), "{run:?}");
    assert_eq!(location.read("data", "a.txt"), "mine");
}

#[test]
fn a_staged_edit_that_conflicts_at_the_commit_is_kept() {
    let location = Location::with_store("fs");
    location.write("data", "a.txt", "original");
    let script = saving_while_someone_writes("again");
    let shell = "stage data\nedit data a.txt\ncommit\n";
    let run = editing(&location, &["store", "shell"], &script, shell).expect_code(3);
    assert_eq!(location.read("data", "a.txt"), "theirs");
    assert_eq!(kept_text(&run), "again");
}
