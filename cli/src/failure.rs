//! How a command fails, and the exit code each failure gives.

use std::fmt;
use std::process::ExitCode;

use serde_json::{Value, json};
use tidings::Area;

use crate::output::{area_name, event_json, event_line};
use crate::working_copy::{Reconciled, SyncEvent};

/// A command that failed: what to tell the person, and the exit code.
#[derive(Debug)]
pub struct Failure {
    kind: FailureKind,
    message: String,
    /// Each Path that stopped a Working copy's commit, and what was found there, if that is how it
    /// failed.
    paths: Option<StoppedPaths>,
}

/// The Paths that stopped a Working copy's commit, each with what was found there.
#[derive(Debug)]
enum StoppedPaths {
    /// It was refused because these are Diverged, each a [`SyncEvent::Diverged`].
    Diverged(Vec<SyncEvent>),
    /// A Conflict for these, after which each was reconciled as `sync` would.
    Conflict(Vec<Reconciled>),
}

/// Each exit code other than success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    /// Exit code 1: anything else, such as bad arguments or a Backend failing.
    Error,
    /// Exit code 2: `read` or `stat` found no File.
    Missing,
    /// Exit code 3: a Precondition didn't hold, so the Commit wrote nothing, or a Working copy's
    /// commit was refused because a Path it would commit is Diverged.
    Conflict,
}

impl Failure {
    /// Anything else that went wrong, which exits with 1.
    pub fn error(message: impl Into<String>) -> Failure {
        Failure { kind: FailureKind::Error, message: message.into(), paths: None }
    }

    /// Nothing was committed, because of a Conflict or a Divergence, which exits with 3.
    pub fn conflict(message: impl Into<String>) -> Failure {
        Failure { kind: FailureKind::Conflict, message: message.into(), paths: None }
    }

    /// A Working copy's commit was refused because the Paths in `diverged`, each a
    /// [`SyncEvent::Diverged`], are Diverged, which exits with 3.
    pub fn diverged(diverged: Vec<SyncEvent>) -> Failure {
        Failure {
            paths: Some(StoppedPaths::Diverged(diverged)),
            ..Failure::conflict(
                "can't commit Diverged Paths: merge each and `tidings resolve` it, or `tidings \
                 discard` it, or name only other paths to commit",
            )
        }
    }

    /// A Working copy's commit met a Conflict, so nothing was committed, and `reconciled` is what
    /// became of each Path it named. Exits with 3.
    pub fn conflicted(reconciled: Vec<Reconciled>) -> Failure {
        Failure {
            paths: Some(StoppedPaths::Conflict(reconciled)),
            ..Failure::conflict(
                "Conflict: the Store changed these since their Base, so nothing was committed",
            )
        }
    }

    /// There is no File at `path` in `area`.
    pub fn missing(area: Area, path: &str) -> Failure {
        let message = format!("no File at {} {path}", area_name(area));
        Failure { kind: FailureKind::Missing, message, paths: None }
    }

    /// Adds `note` to the end of the message.
    pub fn with_note(mut self, note: impl fmt::Display) -> Failure {
        self.message = format!("{}: {note}", self.message);
        self
    }

    /// Adds `context` to the start of the message.
    pub fn in_context(mut self, context: impl fmt::Display) -> Failure {
        self.message = format!("{context}: {}", self.message);
        self
    }

    /// The exit code: 1, 2 or 3, as [`FailureKind`] says.
    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(match self.kind {
            FailureKind::Error => 1,
            FailureKind::Missing => 2,
            FailureKind::Conflict => 3,
        })
    }

    /// Prints this on stderr: for a person, or, with `json`, as one JSON object if it has Paths to
    /// give, as in
    /// `{"failure": "conflict", "message": "…", "paths": [{"event": "diverged", …}]}`.
    /// Each Path is given as `sync --json` gives an event, or, for one that took the Store's
    /// version as its Base, as `{"event": "same", "path": "…"}`. Other failures are printed for a
    /// person, with `json` or without.
    pub fn print(&self, json: bool) {
        match &self.paths {
            Some(paths) if json => {
                let (failure, paths) = match paths {
                    StoppedPaths::Diverged(diverged) => {
                        ("diverged", diverged.iter().map(event_json).collect())
                    }
                    StoppedPaths::Conflict(reconciled) => {
                        ("conflict", reconciled.iter().map(reconciled_json).collect::<Vec<_>>())
                    }
                };
                eprintln!(
                    "{}",
                    json!({"failure": failure, "message": self.message, "paths": paths})
                );
            }
            _ => eprintln!("tidings: {self}"),
        }
    }
}

/// What became of a Path a Conflict named, as `sync --json` gives an event.
fn reconciled_json(reconciled: &Reconciled) -> Value {
    match reconciled {
        Reconciled::Event(event) => event_json(event),
        Reconciled::TookStoresVersion(path) => json!({"event": "same", "path": path.as_str()}),
    }
}

/// What became of a Path a Conflict named, as a line for a person, as `sync` gives an event.
fn reconciled_line(reconciled: &Reconciled) -> String {
    match reconciled {
        Reconciled::Event(event) => event_line(event),
        Reconciled::TookStoresVersion(path) => {
            format!("same {path}: the Store has the same contents, which are now its Base")
        }
    }
}

/// The message, then each Path on a line of its own, indented.
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        let lines: Vec<String> = match &self.paths {
            None => Vec::new(),
            Some(StoppedPaths::Diverged(diverged)) => diverged.iter().map(event_line).collect(),
            Some(StoppedPaths::Conflict(reconciled)) => {
                reconciled.iter().map(reconciled_line).collect()
            }
        };
        lines.iter().try_for_each(|line| write!(f, "\n  {line}"))
    }
}

impl From<tidings::Error> for Failure {
    fn from(error: tidings::Error) -> Failure {
        match error {
            tidings::Error::Conflict { paths } => {
                let paths: Vec<&str> = paths.iter().map(|path| path.as_str()).collect();
                Failure::conflict(format!(
                    "Conflict: a Precondition did not hold for {}",
                    paths.join(", ")
                ))
            }
            error => Failure::error(error.to_string()),
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Failure {
        Failure::error(error.to_string())
    }
}
