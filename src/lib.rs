//! Text files for an application, in three Areas (config, data and cache), held by a Backend: the
//! filesystem, SQLite or memory. Writes happen only through all-or-nothing Commits of a
//! [`Staging`], and every Change is announced on the Store's [`ChangeFeed`].
//!
//! The terms used throughout (Store, Area, Path, Staging, Commit, Change...) are defined in the
//! crate's `CONTEXT.md`.

#[cfg(feature = "sqlite")]
mod app;
mod area;
mod backend;
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

#[cfg(feature = "sqlite")]
pub use app::AppIdentity;
pub use area::Area;
#[cfg(feature = "sqlite")]
pub use backend::sqlite::SqliteOptions;
pub use change::{Change, ChangeFeed, ChangeKind, FeedItem, Origin};
pub use committed::Committed;
pub use error::{Error, Result};
pub use file::{File, Stat};
pub use path::{IntoPath, InvalidPathReason, Path};
pub use precondition::Precondition;
pub use prefix::{IntoPrefix, Prefix};
pub use revision::{PrefixRevision, Revision};
pub use snapshot::Snapshot;
pub use staging::Staging;
pub use store::Store;
