//! Text files for an application, in three Areas (config, data and cache), held by a Backend: the
//! filesystem, SQLite or memory. Writes happen only through all-or-nothing Commits of a
//! [`Staging`], and every Change is announced on the Store's [`ChangeFeed`].
//!
//! The terms used throughout (Store, Area, Path, Staging, Commit, Change...) are defined in the
//! crate's `CONTEXT.md`.

mod area;
mod backend;
mod change;
mod error;
mod file;
mod path;
mod revision;
mod staging;
mod store;

pub use area::Area;
pub use change::{Change, ChangeFeed, ChangeKind, FeedItem, Origin};
pub use error::{Error, Result};
pub use file::File;
pub use path::{IntoPath, Path};
pub use revision::Revision;
pub use staging::Staging;
pub use store::Store;
