//! How a command fails, and the exit code each failure gives.

use std::fmt;
use std::process::ExitCode;

use serde_json::{Value, json};
use tidings::Area;

use crate::output::{EventParts, area_name};
use crate::working_copy::{Reconciled, SyncEvent};

/// A command that failed: what to tell the person, and the exit code.
#[derive(Debug)]
pub struct Failure {
    kind: FailureKind,
    message: String,
    /// What was found at, or became of, each Path that stopped a Working copy's commit, if that is
    /// how it failed.
    outcomes: Option<Outcomes>,
}

/// What was found at, or became of, each Path that stopped a Working copy's commit.
#[derive(Debug)]
enum Outcomes {
    /// It was refused because these Paths are Diverged, each a [`SyncEvent::Diverged`].
    Diverged(Vec<SyncEvent>),
    /// A Conflict, after which each Path it named was reconciled as `sync` would.
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
        Failure { kind: FailureKind::Error, message: message.into(), outcomes: None }
    }

    /// Nothing was committed, which exits with 3: because of a Conflict, or, with
    /// [`Outcomes::Diverged`], because Paths it would commit are Diverged.
    fn conflict(message: impl Into<String>, outcomes: Option<Outcomes>) -> Failure {
        Failure { kind: FailureKind::Conflict, message: message.into(), outcomes }
    }

    /// A Working copy's commit was refused because the Paths in `diverged`, each a
    /// [`SyncEvent::Diverged`], are Diverged, which exits with 3.
    pub fn diverged(diverged: Vec<SyncEvent>) -> Failure {
        Failure::conflict(
            "can't commit Diverged Paths: merge each and `tidings resolve` it, or `tidings \
             discard` it, or name only other paths to commit",
            Some(Outcomes::Diverged(diverged)),
        )
    }

    /// A Working copy's commit met a Conflict, so nothing was committed, and `reconciled` is what
    /// became of each Path it named. Exits with 3.
    pub fn conflicted(reconciled: Vec<Reconciled>) -> Failure {
        Failure::conflict(
            "Conflict: the Store changed these since their Base, so nothing was committed",
            Some(Outcomes::Conflict(reconciled)),
        )
    }

    /// There is no File at `path` in `area`.
    pub fn missing(area: Area, path: &str) -> Failure {
        let message = format!("no File at {} {path}", area_name(area));
        Failure { kind: FailureKind::Missing, message, outcomes: None }
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

    /// Prints this on stderr: for a person, or, with `json`, as one JSON object on one line, as in
    /// `{"failure": "missing", "message": "no File at data a.txt"}`. The failure is `error`,
    /// `missing` or `conflict`, as [`FailureKind`] says, or `diverged` for a commit refused
    /// because of Diverged Paths. A Working copy's commit stopped by Paths also gives
    /// `"paths": […]`, each as `sync --json` gives an event, or, for one that took the Store's
    /// version as its Base, as `{"event": "same", "path": "…", "message": "…"}`.
    pub fn print(&self, json: bool) {
        if !json {
            eprintln!("tidings: {self}");
            return;
        }
        let failure = match (self.kind, &self.outcomes) {
            (_, Some(Outcomes::Diverged(_))) => "diverged",
            (FailureKind::Error, _) => "error",
            (FailureKind::Missing, _) => "missing",
            (FailureKind::Conflict, _) => "conflict",
        };
        let mut object = json!({"failure": failure, "message": self.message});
        if self.outcomes.is_some() {
            let paths: Vec<Value> = self.outcome_parts().iter().map(EventParts::json).collect();
            object["paths"] = json!(paths);
        }
        eprintln!("{object}");
    }

    /// What each of [`Failure::outcomes`] says, as `sync` gives an event.
    fn outcome_parts(&self) -> Vec<EventParts<'_>> {
        match &self.outcomes {
            None => Vec::new(),
            Some(Outcomes::Diverged(diverged)) => diverged.iter().map(EventParts::of).collect(),
            Some(Outcomes::Conflict(reconciled)) => {
                reconciled.iter().map(reconciled_parts).collect()
            }
        }
    }
}

/// What became of a Path a Conflict named, as `sync` gives an event.
fn reconciled_parts(reconciled: &Reconciled) -> EventParts<'_> {
    match reconciled {
        Reconciled::Event(event) => EventParts::of(event),
        Reconciled::TookStoresFile(path) => EventParts {
            name: "same",
            path: Some(path),
            message: Some("the Store has the same contents, which are now its Base".to_owned()),
            theirs: None,
        },
    }
}

/// The message, then each Path on a line of its own, indented.
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        self.outcome_parts().iter().try_for_each(|parts| write!(f, "\n  {}", parts.line()))
    }
}

impl From<tidings::Error> for Failure {
    fn from(error: tidings::Error) -> Failure {
        match error {
            tidings::Error::Conflict { paths } => {
                let paths: Vec<&str> = paths.iter().map(|path| path.as_str()).collect();
                let message =
                    format!("Conflict: a Precondition did not hold for {}", paths.join(", "));
                Failure::conflict(message, None)
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
