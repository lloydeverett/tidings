//! How a command fails, and the exit code each failure gives.

use std::fmt;
use std::process::ExitCode;

use tidings::Area;

use crate::output::area_name;

/// A command that failed: what to tell the person, and the exit code.
#[derive(Debug)]
pub struct Failure {
    kind: FailureKind,
    message: String,
}

/// Each exit code other than success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    /// Exit code 1: anything else, such as bad arguments or a Backend failing.
    Error,
    /// Exit code 2: `read` or `stat` found no File.
    Missing,
    /// Exit code 3: a Precondition didn't hold, so the Commit wrote nothing.
    Conflict,
}

impl Failure {
    /// Anything else that went wrong, which exits with 1.
    pub fn error(message: impl Into<String>) -> Failure {
        Failure { kind: FailureKind::Error, message: message.into() }
    }

    /// There is no File at `path` in `area`.
    pub fn missing(area: Area, path: &str) -> Failure {
        let message = format!("no File at {} {path}", area_name(area));
        Failure { kind: FailureKind::Missing, message }
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
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<tidings::Error> for Failure {
    fn from(error: tidings::Error) -> Failure {
        match error {
            tidings::Error::Conflict { paths } => {
                let paths: Vec<&str> = paths.iter().map(|path| path.as_str()).collect();
                let message =
                    format!("Conflict: a Precondition did not hold for {}", paths.join(", "));
                Failure { kind: FailureKind::Conflict, message }
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
