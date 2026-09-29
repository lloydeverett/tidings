//! Running the `tidings` binary, as a person or a script would, against a temporary Root override.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
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
        location.run(&["--backend", backend, "--create", "store", "list", "data"]).expect_success();
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
        self.run(&["store", "write", area, path, "--contents", contents]).expect_success();
    }

    /// Reads `path` in `area`, failing the test if it can't.
    pub fn read(&self, area: &str, path: &str) -> String {
        self.run(&["store", "read", area, path]).expect_success().stdout
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

/// How long [`Sync::wait_for`] waits for an event, and [`wait_until`] for its condition, before
/// failing the test.
const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

/// Waits until `condition` holds, failing the test if it doesn't within [`EVENT_TIMEOUT`].
#[track_caller]
pub fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "waited too long");
        thread::sleep(Duration::from_millis(50));
    }
}

/// A running `tidings sync --json`, with the events it prints as they come.
pub struct Sync {
    process: Child,
    lines: Receiver<String>,
}

impl Sync {
    /// Starts `tidings --root <root> --json sync <area> <folder>`.
    pub fn start(location: &Location, area: &str, folder: &Path) -> Sync {
        Sync::start_with(&["--root", location.root().to_str().unwrap()], area, folder)
    }

    /// Starts `tidings <flags> --json sync <area> <folder>`, as for a Working copy that knows its
    /// Store when `flags` is empty.
    pub fn start_with(flags: &[&str], area: &str, folder: &Path) -> Sync {
        let mut command = tidings();
        command.args(flags).args(["--json", "sync", area]).arg(folder);
        Sync::spawn(command)
    }

    /// Starts `command`, which runs `sync`, reading its stdout on another thread so that waiting
    /// for a line can time out.
    pub fn spawn(mut command: Command) -> Sync {
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut process = command.spawn().unwrap();
        let stdout = BufReader::new(process.stdout.take().unwrap());
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in stdout.lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Sync { process, lines }
    }

    /// The next line `sync` prints, failing the test if none comes in time.
    #[track_caller]
    pub fn next_line(&mut self) -> String {
        match self.lines.recv_timeout(EVENT_TIMEOUT) {
            Ok(line) => line,
            Err(error) => panic!("no line from sync: {error}"),
        }
    }

    /// The events `sync` prints up to and including the first whose `event` is `event`, failing
    /// the test if it doesn't come in time.
    #[track_caller]
    pub fn wait_for(&mut self, event: &str) -> Vec<Value> {
        let mut events = Vec::new();
        loop {
            let line = self.next_line();
            let value: Value = serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("{error}: sync printed {line:?}"));
            let found = value["event"] == event;
            events.push(value);
            if found {
                return events;
            }
        }
    }

    /// Stops `sync` as Ctrl-C would, with SIGINT, and fails the test unless it exits with 0.
    #[track_caller]
    pub fn stop(mut self) {
        let pid = self.process.id().to_string();
        let killed = Command::new("kill").args(["-INT", &pid]).status().unwrap();
        assert!(killed.success());
        let status = self.process.wait().unwrap();
        assert_eq!(status.code(), Some(0), "sync stopped with {status}");
    }
}

impl Drop for Sync {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
