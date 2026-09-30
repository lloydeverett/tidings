use std::path::PathBuf;

use crate::{BackendKind, InvalidPathReason, Path, Prefix};

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
    /// tidings show the Commit already, and the next Commit to the Store, or opening a Store on
    /// its Location, finishes it. Until then, every Commit to the Store first tries to finish it,
    /// and isn't made if it still can't.
    #[error("the Commit happened, but isn't finished yet")]
    Pending,
    /// The Location belongs to another Backend: its Backend marker names `found`. Opening a
    /// Store on the filesystem or SQLite gives it, without changing anything, for a Location
    /// marked for the other Backend.
    #[error("the Location belongs to the {found} Backend")]
    WrongBackend {
        /// The Backend its marker names.
        found: BackendKind,
    },
    /// The Location is inside `outer`, a directory holding a `.tidings/` directory, as another
    /// Store's Location and a Working copy do. Opening a Store on the filesystem or SQLite gives
    /// it, before anything is made, since the two would claim the same files. `outer` is as found
    /// with every symlink on the way resolved.
    #[error(
        "the Location is inside {}, which holds another Store or a Working copy",
        outer.display()
    )]
    NestedLocation {
        /// The directory the Location is inside.
        outer: PathBuf,
    },
    /// The Location is a Working copy's folder: its `.tidings/` holds the record the `tidings`
    /// command keeps there. Opening a Store on the filesystem or SQLite, and
    /// [`Store::detect`](crate::Store::detect), give it, without changing anything, since the
    /// Store's files would mix with the Working copy's.
    #[error("the Location {} is a Working copy's folder", location.display())]
    LocationIsWorkingCopy {
        /// The Location, as it was given.
        location: PathBuf,
    },
    /// A Staging required a [`PrefixRevision`](crate::PrefixRevision) for `prefix` that was taken
    /// for another Prefix, or from another Store than the one it was committed to. It could never
    /// hold, so the Commit was refused, and nothing was written.
    #[error("the Prefix Revision required for {prefix} was taken for another Prefix or Store")]
    WrongPrefixRevision {
        /// The Prefix it was required for.
        prefix: Prefix,
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
    #[cfg(any(feature = "fs", feature = "sqlite"))]
    pub(crate) fn backend(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
        Error::Backend(error.into())
    }
}

/// A `Result` whose error is tidings' [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
