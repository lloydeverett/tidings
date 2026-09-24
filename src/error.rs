use crate::{InvalidPathReason, Path};

/// Everything that can go wrong in tidings.
///
/// A missing File is not an error: reading one gives `Ok(None)`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A Path or Prefix is not allowed.
    #[error("invalid path {path:?}: {reason}")]
    InvalidPath {
        /// The Path or Prefix as it was given.
        path: String,
        /// Which rule it breaks.
        reason: InvalidPathReason,
    },
    /// The File at `path` isn't valid UTF-8, so it can't be read as text. It is still listed. Only
    /// the filesystem can hold one, written there by another program.
    #[error("{path} is not valid UTF-8 text")]
    NotText {
        /// The File's Path.
        path: Path,
    },
    /// A Commit was refused because at least one of its Preconditions didn't hold. Nothing was
    /// written.
    #[error("conflict: a Precondition failed for {paths:?}")]
    Conflict {
        /// The Paths whose Precondition failed, in order. For a Prefix, the Paths under it that
        /// were added, removed or changed.
        paths: Vec<Path>,
    },
    /// The Commit has happened, and its Changes are on the Change feed, but it isn't finished yet.
    /// Only the filesystem gives it, when a File can't be replaced even after trying again for a
    /// moment, as happens on Windows while another program has the File open. Reads through
    /// tidings show the Commit already, and the next Commit to the Area, or opening a Store on it,
    /// finishes it. Until then, every Commit to the Area first tries to finish it, and isn't made
    /// if it still can't.
    #[error("the Commit happened, but isn't finished yet")]
    Pending,
    /// The Store's Backend can't do this. A Snapshot on the filesystem gives it: check
    /// [`Store::supports_snapshots`](crate::Store::supports_snapshots) first.
    #[error("not supported by this Store's Backend")]
    Unsupported,
    /// The Store's Backend failed, for example because SQLite gave an error. It wraps the
    /// underlying error.
    #[error("the Store's Backend failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    /// The Backend failed with `error`.
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    pub(crate) fn backend(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
        Error::Backend(error.into())
    }
}

/// A `Result` whose error is tidings' [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
