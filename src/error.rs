/// Everything that can go wrong in tidings.
///
/// A missing File is not an error: reading one gives `Ok(None)`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A Path is not allowed.
    #[error("invalid path {path:?}: {reason}")]
    InvalidPath {
        /// The Path as it was given.
        path: String,
        /// Which rule it breaks.
        reason: String,
    },
}

/// A `Result` whose error is tidings' [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
