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
    /// A Commit was refused because at least one of its Preconditions didn't hold. Nothing was
    /// written.
    #[error("conflict: a Precondition failed for {paths:?}")]
    Conflict {
        /// The Paths whose Precondition failed, in order. For a Prefix, the Paths under it that
        /// were added, removed or changed.
        paths: Vec<Path>,
    },
    /// The Store's Backend can't do this. A Snapshot on the filesystem gives it: check
    /// [`Store::supports_snapshots`](crate::Store::supports_snapshots) first.
    #[error("not supported by this Store's Backend")]
    Unsupported,
}

/// A `Result` whose error is tidings' [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
