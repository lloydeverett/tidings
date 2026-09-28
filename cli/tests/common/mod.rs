//! Running the `tidings` binary, as a person or a script would, against a temporary Root override.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use tempfile::TempDir;

/// What a run of `tidings` gave.
#[derive(Debug)]
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// Fails the test unless the run exited with `code`, showing what it printed.
    #[track_caller]
    pub fn expect_code(self, code: i32) -> Run {
        assert_eq!(self.code, code, "exit code of a run that gave {self:#?}");
        self
    }

    /// Fails the test unless the run succeeded, exiting with 0.
    #[track_caller]
    pub fn expect_success(self) -> Run {
        self.expect_code(0)
    }
}

/// A temporary directory holding a Store, which each run is pointed at with `--root`.
pub struct Location {
    directory: TempDir,
}

impl Location {
    /// An empty location, with no Store in it yet.
    pub fn empty() -> Location {
        Location { directory: tempfile::tempdir().unwrap() }
    }

    /// A location holding a new Store on `backend` (`fs` or `sqlite`).
    pub fn with_store(backend: &str) -> Location {
        let location = Location::empty();
        location.run(&["--backend", backend, "--create", "list", "data"]).expect_success();
        location
    }

    /// The directory the Store's Areas are in.
    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// `tidings --root <root> <args>`, with nothing on stdin.
    pub fn run(&self, args: &[&str]) -> Run {
        self.run_with_stdin(args, "")
    }

    /// `tidings --root <root> <args>`, with `stdin` on stdin.
    pub fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Run {
        run(self.command(args), stdin)
    }

    /// The command for `tidings --root <root> <args>`, to add to.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = tidings();
        command.arg("--root").arg(self.root()).args(args);
        command
    }

    /// Writes `contents` to `path` in `area`, failing the test if it can't.
    pub fn write(&self, area: &str, path: &str, contents: &str) {
        self.run(&["write", area, path, "--contents", contents]).expect_success();
    }

    /// Reads `path` in `area`, failing the test if it can't.
    pub fn read(&self, area: &str, path: &str) -> String {
        self.run(&["read", area, path]).expect_success().stdout
    }
}

/// The `tidings` command, with none of the environment variables it reads set.
pub fn tidings() -> Command {
    let mut command = Command::new(binary());
    for variable in ["TIDINGS_ROOT", "TIDINGS_IDENTITY", "TIDINGS_BACKEND", "VISUAL", "EDITOR"] {
        command.env_remove(variable);
    }
    command
}

/// The `tidings` binary Cargo built for these tests.
pub fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tidings"))
}

/// Runs `command` to the end with `stdin` on its stdin.
pub fn run(mut command: Command, stdin: &str) -> Run {
    let mut child = spawn(&mut command);
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    Run {
        code: output.status.code().expect("tidings was killed by a signal"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// Starts `command` with every stream piped.
pub fn spawn(command: &mut Command) -> Child {
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
}

/// Whether `text` is a Revision as the CLI writes one: 32 lowercase hexadecimal digits.
pub fn is_revision(text: &str) -> bool {
    text.len() == 32 && text.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}
