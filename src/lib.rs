//! Text files for an application, in as many Stores as it wants, each in a directory it chooses,
//! held by the Backend it chooses: the filesystem, SQLite or memory. Writes happen only through
//! all-or-nothing Commits of a [`Staging`], and every Change is announced on the Store's
//! [`ChangeFeed`].
//!
//! The [`Store`] is async, on tokio. With the `blocking` feature, `blocking::Store` offers the
//! same for synchronous code.
//!
//! The terms used throughout (Store, Location, Path, Staging, Commit, Change...) are defined in the
//! crate's `CONTEXT.md`.

mod backend;
#[cfg(feature = "blocking")]
pub mod blocking;
mod change;
mod committed;
mod error;
mod file;
mod path;
mod precondition;
mod prefix;
mod revision;
mod snapshot;
mod staging;
mod store;

pub use backend::BackendKind;
#[cfg(feature = "fs")]
pub use backend::fs::FsOptions;
#[cfg(all(feature = "fs", feature = "testing"))]
pub use backend::fs::{FailurePoint, Pause};
#[cfg(feature = "sqlite")]
pub use backend::sqlite::SqliteOptions;
pub use change::{Change, ChangeFeed, ChangeKind, FeedItem, Origin};
pub use committed::Committed;
pub use error::{Error, Result};
pub use file::{File, Stat};
pub use path::{IntoPath, InvalidPathReason, Path};
pub use precondition::Precondition;
pub use prefix::{IntoPrefix, Prefix};
pub use revision::{ParseRevisionError, PrefixRevision, Revision};
pub use snapshot::Snapshot;
pub use staging::Staging;
pub use store::Store;
