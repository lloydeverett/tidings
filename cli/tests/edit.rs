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

#[test]
fn an_edited_file_is_written_back() {
    let location = Location::with_store("fs");
    location.write("config", "app.toml", "a = 1\n");
    let run = editing(&location, &["edit", "config", "app.toml"], r#"echo "b = 2" >> "$0""#, "");
    run.expect_success();
    assert_eq!(location.read("config", "app.toml"), "a = 1\nb = 2\n");
}

#[test]
fn a_missing_file_starts_empty_and_is_created() {
    let location = Location::with_store("sqlite");
    let script = r#"test ! -s "$0" && printf new > "$0""#;
    editing(&location, &["edit", "data", "notes/new.txt"], script, "").expect_success();
    assert_eq!(location.read("data", "notes/new.txt"), "new");
}

#[test]
fn an_unchanged_buffer_commits_nothing() {
    let location = Location::with_store("fs");
    let run = editing(&location, &["edit", "data", "a.txt"], "true", "").expect_success();
    assert!(run.stderr.contains("unchanged"), "{run:?}");
    location.run(&["read", "data", "a.txt"]).expect_code(2);
}

#[test]
fn a_file_changed_while_it_was_edited_is_a_conflict_that_keeps_the_edit() {
    let location = Location::with_store("fs");
    location.write("data", "a.txt", "original");
    // The editor saves, and meanwhile someone else writes the File.
    let script =
        r#"printf mine > "$0" && "$TIDINGS" --root "$ROOT" write data a.txt --contents theirs"#;
    let run = editing(&location, &["edit", "data", "a.txt"], script, "").expect_code(3);
    assert_eq!(location.read("data", "a.txt"), "theirs");
    let kept = run.stderr.trim_end().rsplit(' ').next().unwrap();
    assert_eq!(std::fs::read_to_string(kept).unwrap(), "mine", "{run:?}");
    std::fs::remove_file(kept).unwrap();
}

#[test]
fn a_file_created_while_it_was_edited_is_a_conflict() {
    let location = Location::with_store("sqlite");
    let script =
        r#"printf mine > "$0" && "$TIDINGS" --root "$ROOT" write data a.txt --contents theirs"#;
    let run = editing(&location, &["edit", "data", "a.txt"], script, "").expect_code(3);
    let kept = run.stderr.trim_end().rsplit(' ').next().unwrap();
    std::fs::remove_file(kept).unwrap();
}

#[test]
fn an_editor_that_fails_commits_nothing() {
    let location = Location::with_store("fs");
    let script = r#"printf changed > "$0"; exit 1"#;
    editing(&location, &["edit", "data", "a.txt"], script, "").expect_code(1);
    location.run(&["read", "data", "a.txt"]).expect_code(2);
}

#[test]
fn with_no_editor_set_edit_fails() {
    let location = Location::with_store("fs");
    let run = location.run(&["edit", "data", "a.txt"]).expect_code(1);
    assert!(run.stderr.contains("EDITOR"), "{run:?}");
}

#[test]
fn in_a_staging_the_edit_is_staged_with_its_precondition() {
    let location = Location::with_store("fs");
    location.write("data", "a.txt", "original");
    let script = r#"printf mine > "$0""#;
    let shell = "stage data\nedit data a.txt\nlist data\ncommit\n";
    let run = editing(&location, &["shell"], script, shell).expect_success();
    assert!(run.stdout.starts_with("a.txt\n"), "{run:?}");
    assert_eq!(location.read("data", "a.txt"), "mine");

    // Changed by someone else before the commit.
    let script =
        r#"printf again > "$0" && "$TIDINGS" --root "$ROOT" write data a.txt --contents theirs"#;
    editing(&location, &["shell"], script, shell).expect_code(3);
    assert_eq!(location.read("data", "a.txt"), "theirs");
}
