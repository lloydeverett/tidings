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
    /// The Store's Backend failed, for example because SQLite gave an error. It wraps the
    /// underlying error.
    #[error("the Store's Backend failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    /// The Backend failed with `error`.
    #[cfg(feature = "sqlite")]
    pub(crate) fn backend(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
        Error::Backend(error.into())
    }
}

/// What a tokio task gave, as it `joined`: its own result, or its panic, raised again here.
#[cfg(feature = "sqlite")]
pub(crate) fn joined<T>(joined: Result<Result<T>, tokio::task::JoinError>) -> Result<T> {
    match joined {
        Ok(result) => result,
        Err(error) => match error.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // The runtime is shutting down.
            Err(error) => Err(Error::backend(error)),
        },
    }
}

/// A `Result` whose error is tidings' [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
